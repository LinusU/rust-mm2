//! CNRBOT-A original-content validation: the full 15-bot Cops & Robbers
//! roster is placed and spawned over the retail SF and London lanes.
//!
//! Evidence level: original-content validation (real nav graph, deadly
//! water, collision and vehicle data; no rendering, no driving). Opt-in
//! (`MM2_RETAIL=<dir>`); skipped without the operator's install, and
//! says so.

use avian3d::prelude::*;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use mm2_app::cnr_bots::{
    BOT_SPACING, CnrBot, HUMAN_CLEARANCE, SITE_CLEARANCE, lane_slots, plan_bots, spawn_bots,
};
use mm2_assets::Vfs;
use mm2_content::CnrContent;
use mm2_formats::{psdl::Psdl, water::WaterDef};
use mm2_game::{
    MAX_CNR_BOTS, ObjectIdentity, Player, PlayerControl, Session, SessionConfig, SessionEntity,
};

use crate::golden_gate::{VfsRes, retail_city_app};

#[test]
fn retail_full_roster_is_placed_safely_and_spawned_in_each_city() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set; the retail bot roster was NOT checked");
        return;
    };
    let mut vfs = Vfs::new();
    mm2_assets::mount_install(&mut vfs, &retail, &mm2_assets::InstallMount::default()).unwrap();
    let surfaces = mm2_content::load_surface_tables(&vfs).unwrap();

    for city in ["sf", "london"] {
        let (psdl_bytes, _) = vfs.read_path(&format!("city/{city}.psdl")).unwrap();
        let psdl = Psdl::parse(&psdl_bytes).unwrap();
        let (water_bytes, _) = vfs.read_path(&format!("city/{city}.water")).unwrap();
        let def = WaterDef::parse(std::str::from_utf8(&water_bytes).unwrap()).unwrap();
        let water = mm2_app::water::CityWater::build(&def, &psdl, surfaces.as_ref());
        let graph = mm2_app::cnr_bots::load_graph(&vfs, city).unwrap();
        let candidates = lane_slots(&graph, Some(&water));
        let wet = candidates.iter().filter(|(_, w)| *w).count();
        eprintln!(
            "{city}: {} lane poses, {wet} in deadly water",
            candidates.len()
        );
        assert!(candidates.len() > 100, "{city}: a real road network");

        let sites: Vec<Vec3> = CnrContent::load(&vfs, city)
            .sites
            .into_iter()
            .map(|p| Vec3::new(p[0], p[1], -p[2]))
            .collect();
        assert!(!sites.is_empty(), "{city}: gold sites");
        let human = candidates[0].0.position;

        let plan = plan_bots(&vfs, &candidates, MAX_CNR_BOTS, 7, human, &sites)
            .unwrap_or_else(|e| panic!("{city}: {e}"));
        assert_eq!(plan.len(), MAX_CNR_BOTS as usize);
        // Same inputs, same roster.
        let again = plan_bots(&vfs, &candidates, MAX_CNR_BOTS, 7, human, &sites).unwrap();
        for (a, b) in plan.iter().zip(&again) {
            assert_eq!(a.slot, b.slot, "{city}: deterministic placement");
            assert_eq!(a.def.id, b.def.id, "{city}: deterministic cars");
        }
        for (i, a) in plan.iter().enumerate() {
            assert!(!water.is_deadly(a.slot.position), "{city} bot {i} in water");
            assert!(a.slot.position.distance(human) >= HUMAN_CLEARANCE);
            assert!(
                sites
                    .iter()
                    .all(|s| a.slot.position.distance(*s) >= SITE_CLEARANCE)
            );
            for b in &plan[i + 1..] {
                assert!(a.slot.position.distance(b.slot.position) >= BOT_SPACING);
            }
        }
        let distinct: std::collections::BTreeSet<_> = plan.iter().map(|p| &p.def.id).collect();
        assert!(distinct.len() >= 2, "{city}: more than one car model");

        // Spawn them in the loaded city: every start has ground under it.
        let slots: Vec<_> = plan.iter().map(|p| p.slot).collect();
        let mut app = retail_city_app(vfs_clone(&retail), &format!("city/{city}.psdl"));
        app.world_mut()
            .run_system_once(move |spatial: SpatialQuery| {
                for (i, s) in slots.iter().enumerate() {
                    let hit = spatial.cast_ray(
                        s.position + Vec3::Y * 3.0,
                        Dir3::NEG_Y,
                        8.0,
                        true,
                        &SpatialQueryFilter::default(),
                    );
                    assert!(
                        hit.is_some(),
                        "{city} bot {i}: no ground under {:?}",
                        s.position
                    );
                }
            })
            .unwrap();
        let mut plan = Some(plan);
        let ids = app
            .world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      vfs: Res<VfsRes>,
                      mut meshes: ResMut<Assets<Mesh>>,
                      mut images: ResMut<Assets<Image>>,
                      mut materials: ResMut<Assets<StandardMaterial>>| {
                    let mut session = Session::new();
                    session.begin(SessionConfig::default()).unwrap();
                    spawn_bots(
                        &mut commands,
                        &vfs.0,
                        &mut meshes,
                        &mut images,
                        &mut materials,
                        plan.take().unwrap(),
                        SessionEntity(1),
                        &mut session,
                    )
                    .len()
                },
            )
            .unwrap();
        assert_eq!(ids, MAX_CNR_BOTS as usize);
        app.update();
        let world = app.world_mut();
        let mut q = world.query::<(&CnrBot, &Player, &ObjectIdentity, Has<RigidBody>)>();
        let bots: Vec<_> = q.iter(world).collect();
        assert_eq!(bots.len(), MAX_CNR_BOTS as usize, "{city}");
        assert!(
            bots.iter()
                .all(|(_, p, _, body)| p.control == PlayerControl::Ai && *body)
        );
        let players: std::collections::BTreeSet<_> = bots.iter().map(|(_, p, _, _)| p.id).collect();
        let objects: std::collections::BTreeSet<_> = bots.iter().map(|(_, _, o, _)| o.0).collect();
        assert_eq!(players.len(), bots.len(), "{city}: distinct PlayerIds");
        assert_eq!(objects.len(), bots.len(), "{city}: distinct ObjectIds");
        // Session-owned, so exit, restart and rematch remove every bot
        // with the rest of the world.
        let mut owned = world.query_filtered::<&SessionEntity, With<CnrBot>>();
        assert_eq!(owned.iter(world).count(), MAX_CNR_BOTS as usize);
    }
}

fn vfs_clone(retail: &std::path::Path) -> Vfs {
    let mut vfs = Vfs::new();
    mm2_assets::mount_install(&mut vfs, retail, &mm2_assets::InstallMount::default()).unwrap();
    vfs
}

/// CNR-MP-BOTS-B: a host session spawns its bots with ids above the
/// wire seats, no wire identity, and only when it is the host.
#[test]
fn retail_host_bots_carry_ids_clear_of_the_wire_seats() {
    use mm2_app::cnr_bots::NET_BOT_PLAYER_BASE;
    use mm2_app::netdrive::NetPlayer;
    use mm2_game::{SessionAuthority, SessionMode, WorldMode};

    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set; the host bot ids were NOT checked");
        return;
    };
    let vfs = vfs_clone(&retail);
    let graph = mm2_app::cnr_bots::load_graph(&vfs, "sf").unwrap();
    let candidates = lane_slots(&graph, None);
    let plan = plan_bots(&vfs, &candidates, 4, 3, Vec3::ZERO, &[]).unwrap();

    let mut app = retail_city_app(vfs_clone(&retail), "city/sf.psdl");
    let mut plan = Some(plan);
    app.world_mut()
        .run_system_once(
            move |mut commands: Commands,
                  vfs: Res<VfsRes>,
                  mut meshes: ResMut<Assets<Mesh>>,
                  mut images: ResMut<Assets<Image>>,
                  mut materials: ResMut<Assets<StandardMaterial>>| {
                let mut session = Session::new();
                session
                    .begin(SessionConfig {
                        world: WorldMode::City {
                            psdl: "city/sf.psdl".to_string(),
                        },
                        mode: SessionMode::CopsAndRobbers(Default::default()),
                        authority: SessionAuthority::Host,
                        cnr_bots: 4,
                        ..SessionConfig::default()
                    })
                    .unwrap();
                spawn_bots(
                    &mut commands,
                    &vfs.0,
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    plan.take().unwrap(),
                    SessionEntity(1),
                    &mut session,
                );
            },
        )
        .unwrap();
    app.update();
    let world = app.world_mut();
    let mut q = world.query::<(&CnrBot, &Player, Has<NetPlayer>)>();
    let mut bots: Vec<_> = q
        .iter(world)
        .map(|(b, p, net)| (b.index, p.id.0, net))
        .collect();
    bots.sort();
    assert_eq!(
        bots,
        (0..4)
            // A hosted bot publishes under its own id (CNR-MP-BOTS-C).
            .map(|i| (i, NET_BOT_PLAYER_BASE + u16::from(i), true))
            .collect::<Vec<_>>()
    );
}
