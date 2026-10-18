//! CNRBOT-C original-content validation: local Cops & Robbers bots
//! drive the retail SF and London street graphs through the retail
//! city's collision, with production vehicles and the real gold step.
//!
//! Evidence level: original-content validation (retail nav graph,
//! collision, vehicle data, deadly water; no rendering, no audio, no
//! play-test). Opt-in (`MM2_RETAIL=<dir>`); skipped without the
//! operator's install, and says so.
//!
//! Legs automated here, per city: (1) one bot drives from a street
//! start to the loose gold, takes it and delivers it; (2) a second bot
//! pursues that carrier. What is *not* claimed: that a steal succeeds
//! in a given street layout — the test reports whether the pursuer
//! reached the carrier and whether the gold changed hands, and asserts
//! only the contact.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::city::load_city;
use mm2_app::cnr::{
    CnrEvent, CnrHost, cnr_host_step, enroll_cnr_participants, reconcile_gold_load,
};
use mm2_app::cnr_bot_drive::{CnrBotBrain, CnrBotDriver, cnr_bot_drive};
use mm2_app::cnr_bots::{
    BotSlot, CnrBot, PlannedBot, lane_slots, load_graph, spawn_bots, stock_vehicle_ids,
};
use mm2_app::contracts;
use mm2_assets::Vfs;
use mm2_content::cnr::{CnrContent, CnrSettings, GoldMass};
use mm2_formats::{psdl::Psdl, water::WaterDef};
use mm2_game::gold::{CnrVariant, GoldEvent};
use mm2_game::{
    ImpactEvent, Player, PlayerId, Session, SessionAuthority, SessionConfig, SessionEntity,
    SessionMode, SessionPhase, WorldMode,
};
use mm2_vehicle::VehiclePlugin;

const HZ: u32 = 60;

#[derive(Resource)]
struct VfsRes(Vfs);

#[derive(Resource, Default)]
struct Events(Vec<GoldEvent>);

fn collect(mut reader: MessageReader<CnrEvent>, mut out: ResMut<Events>) {
    out.0.extend(reader.read().map(|e| e.0));
}

fn mount(retail: &std::path::Path) -> Vfs {
    let mut vfs = Vfs::new();
    mm2_assets::mount_install(&mut vfs, retail, &mm2_assets::InstallMount::default()).unwrap();
    vfs
}

/// Whether a car on the road can meet `p`: a routable vehicle lane
/// passes within 15 m horizontally and 3 m vertically — the measure
/// `docs/research/cnr.md` uses for "on a lane". Most retail sites are
/// not (plazas, parks, another level), and a street-following bot
/// cannot take or deliver those; that is a property of the data, so the
/// legs below run on a seed whose opening draw is all on lanes.
fn on_lane(graph: &mm2_game::NavGraph, p: Vec3) -> bool {
    graph
        .nearest_lane(p.into(), &mm2_game::LaneQuery::vehicles(30.0))
        .is_some_and(|h| {
            (h.point[0] - p.x).hypot(h.point[2] - p.z) <= 15.0 && (h.point[1] - p.y).abs() <= 3.0
        })
}

/// The first `n` seeds whose opening gold and hideout both lie on a lane.
fn lane_seeds(vfs: &Vfs, city: &str, graph: &mm2_game::NavGraph, n: usize) -> Vec<u64> {
    let content = CnrContent::load(vfs, city);
    let settings = CnrSettings::default();
    (0..4096u64)
        .filter(|&seed| {
            let host = CnrHost::from_content(
                &content,
                &settings,
                HZ,
                1,
                mm2_game::ObjectId {
                    generation: 1,
                    slot: 1,
                },
                seed,
                &[],
            )
            .unwrap();
            let sites = host.game.sites();
            on_lane(graph, sites.gold) && on_lane(graph, sites.hideout)
        })
        .take(n)
        .collect()
}

/// The retail city under a local Cops & Robbers session that is
/// `Playing`, with the bot driver and the gold step wired as in the app.
fn city_app(retail: &std::path::Path, city: &str, seed: u64) -> (App, Vec<BotSlot>) {
    let vfs = mount(retail);
    let surfaces = mm2_content::load_surface_tables(&vfs).unwrap();
    let (psdl_bytes, _) = vfs.read_path(&format!("city/{city}.psdl")).unwrap();
    let psdl = Psdl::parse(&psdl_bytes).unwrap();
    let (water_bytes, _) = vfs.read_path(&format!("city/{city}.water")).unwrap();
    let def = WaterDef::parse(std::str::from_utf8(&water_bytes).unwrap()).unwrap();
    let water = mm2_app::water::CityWater::build(&def, &psdl, surfaces.as_ref());
    let graph = load_graph(&vfs, city).unwrap();
    let slots: Vec<BotSlot> = lane_slots(&graph, Some(&water))
        .into_iter()
        .filter(|(_, wet)| !wet)
        .map(|(s, _)| s)
        .collect();

    let mut session = Session::new();
    session
        .begin(SessionConfig {
            world: WorldMode::City {
                psdl: format!("city/{city}.psdl"),
            },
            mode: SessionMode::CopsAndRobbers(CnrSettings {
                variant: CnrVariant::FreeForAll,
                ..CnrSettings::default()
            }),
            authority: SessionAuthority::Local,
            seed,
            ..SessionConfig::default()
        })
        .unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    let gold = session.mint_object_id();
    let host = CnrHost::from_content(
        &CnrContent::load(&vfs, city),
        &CnrSettings {
            variant: CnrVariant::FreeForAll,
            gold_mass: GoldMass::QuarterTon,
            ..CnrSettings::default()
        },
        HZ,
        session.generation(),
        gold,
        seed,
        &[],
    )
    .unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .insert_resource(Time::<Fixed>::from_hz(f64::from(HZ)))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / f64::from(HZ),
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(VfsRes(vfs))
        .insert_resource(session)
        .insert_resource(host)
        .insert_resource(water)
        .insert_resource(CnrBotBrain::new(Some(std::sync::Arc::new(graph))))
        .add_message::<ImpactEvent>()
        .add_message::<CnrEvent>()
        .add_message::<mm2_game::RecoveryEvent>()
        .init_resource::<Events>()
        .init_resource::<contracts::ImpactFilter>()
        .add_systems(FixedUpdate, cnr_bot_drive)
        .add_systems(
            FixedLast,
            (
                contracts::collect_impacts,
                enroll_cnr_participants,
                cnr_host_step,
                reconcile_gold_load,
                collect,
            )
                .chain(),
        );
    app.finish();
    app.cleanup();
    let psdl = format!("city/{city}.psdl");
    app.world_mut()
        .run_system_once(
            move |mut commands: Commands,
                  vfs: Res<VfsRes>,
                  mut meshes: ResMut<Assets<Mesh>>,
                  mut images: ResMut<Assets<Image>>,
                  mut materials: ResMut<Assets<StandardMaterial>>| {
                let mut s = Session::new();
                load_city(
                    &mut commands,
                    &vfs.0,
                    &psdl,
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    SessionEntity(1),
                    &mut s,
                )
                .expect("retail city loads");
            },
        )
        .unwrap();
    for _ in 0..3 {
        app.update();
    }
    (app, slots)
}

/// The lane start nearest `distance` metres (XZ) from `from`, not in
/// `taken`.
fn start_near(slots: &[BotSlot], from: Vec3, distance: f32, taken: &[BotSlot]) -> BotSlot {
    *slots
        .iter()
        .filter(|s| !taken.contains(s))
        .min_by(|a, b| {
            let d = |s: &BotSlot| {
                ((s.position.x - from.x).hypot(s.position.z - from.z) - distance).abs()
            };
            d(a).total_cmp(&d(b))
        })
        .unwrap()
}

fn spawn(app: &mut App, slots: Vec<BotSlot>) -> Vec<(Entity, PlayerId)> {
    let mut plan = Some(
        slots
            .into_iter()
            .map(|slot| {
                let vfs = &app.world().resource::<VfsRes>().0;
                let ids = stock_vehicle_ids(vfs);
                PlannedBot {
                    def: mm2_content::load_opponent(vfs, &ids[0], 0).unwrap(),
                    slot,
                }
            })
            .collect::<Vec<_>>(),
    );
    app.world_mut()
        .run_system_once(
            move |mut commands: Commands,
                  vfs: Res<VfsRes>,
                  mut meshes: ResMut<Assets<Mesh>>,
                  mut images: ResMut<Assets<Image>>,
                  mut materials: ResMut<Assets<StandardMaterial>>,
                  mut session: ResMut<Session>| {
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
    let mut q = world.query::<(Entity, &CnrBot, &Player)>();
    let mut v: Vec<_> = q.iter(world).map(|(e, b, p)| (b.index, e, p.id)).collect();
    v.sort();
    v.into_iter().map(|(_, e, id)| (e, id)).collect()
}

fn pos(app: &App, e: Entity) -> Vec3 {
    app.world().get::<Position>(e).unwrap().0
}

/// What one city/seed run did.
#[derive(Debug)]
struct Outcome {
    picked: Option<u32>,
    delivered: Option<u32>,
    /// Closest the second bot came to the carrier while it carried, m.
    closest: f32,
    /// Whether the second bot ended up with the gold by ramming.
    stolen: bool,
    streets: u64,
    reanchors: u32,
    refused: u32,
    why_stopped: &'static str,
}

/// Two bots in `city` on `seed`: A starts ~60 m from the gold, B ~150 m. A is run until it delivers, sits [`GIVE_UP`] seconds inside a
/// 15 m circle, has not taken the gold by [`PICKUP_BY`], or [`BUDGET`]
/// seconds pass.
fn run(retail: &std::path::Path, city: &str, seed: u64) -> Outcome {
    let (mut app, slots) = city_app(retail, city, seed);
    let gold = app.world().resource::<CnrHost>().game.sites().gold;
    let a_slot = start_near(&slots, gold, 60.0, &[]);
    let b_slot = start_near(&slots, gold, 150.0, &[a_slot]);
    let bots = spawn(&mut app, vec![a_slot, b_slot]);
    let ((a, a_id), (b, b_id)) = (bots[0], bots[1]);
    let mut out = Outcome {
        picked: None,
        delivered: None,
        closest: f32::MAX,
        stolen: false,
        streets: 0,
        reanchors: 0,
        refused: 0,
        why_stopped: "budget",
    };
    let (mut still_at, mut still_since) = (pos(&app, a), 0u32);
    for tick in 0..HZ * BUDGET {
        app.update();
        for e in &app.world().resource::<Events>().0 {
            match e {
                GoldEvent::Picked {
                    player,
                    recovered: false,
                    ..
                } if *player == a_id => {
                    out.picked.get_or_insert(tick);
                }
                GoldEvent::Picked { player, .. } if *player == b_id => out.stolen = true,
                GoldEvent::Delivered { player, .. } if *player == a_id => {
                    out.delivered.get_or_insert(tick);
                }
                _ => {}
            }
        }
        if out.delivered.is_some() {
            out.why_stopped = "delivered";
            break;
        }
        if out.picked.is_some() {
            out.closest = out.closest.min(pos(&app, a).distance(pos(&app, b)));
        }
        // Given up on: A has not left a 15 m circle for GIVE_UP seconds,
        // or has not taken the gold within PICKUP_BY seconds.
        if tick % 30 == 0 {
            let here = pos(&app, a);
            if here.distance(still_at) > 15.0 {
                (still_at, still_since) = (here, tick);
            }
        }
        if tick > still_since + HZ * GIVE_UP {
            out.why_stopped = "stationary";
            break;
        }
        if out.picked.is_none() && tick > HZ * PICKUP_BY {
            out.why_stopped = "no pickup";
            break;
        }
    }
    let da = app.world().get::<CnrBotDriver>(a).unwrap();
    let db = app.world().get::<CnrBotDriver>(b).unwrap();
    out.streets = app.world().resource::<CnrBotBrain>().stats.streets;
    out.reanchors = da.reanchors + db.reanchors;
    out.refused = da.refused + db.refused;
    eprintln!("{city} seed {seed}: {out:?}");
    out
}

/// Seconds of simulated time one run may take, and how long A may go
/// without getting closer to the end of its line.
const BUDGET: u32 = 300;
const GIVE_UP: u32 = 60;
const PICKUP_BY: u32 = 120;
/// Seeds tried per city.
const SEEDS: usize = 3;

#[test]
fn retail_bots_pick_up_deliver_and_pursue_in_each_city() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set; the retail bot drive was NOT checked");
        return;
    };
    for city in ["sf", "london"] {
        let vfs = mount(&retail);
        let graph = load_graph(&vfs, city).unwrap();
        let seeds = lane_seeds(&vfs, city, &graph, SEEDS);
        assert!(!seeds.is_empty(), "{city}: seeds with an all-on-lane draw");
        let outcomes: Vec<(u64, Outcome)> = seeds
            .into_iter()
            .map(|seed| (seed, run(&retail, city, seed)))
            .collect();
        // Every run reports (see the eprintln above). The legs work on
        // the seeds where the map lets them, so at least one seed must
        // deliver and at least one must see the pursuer close on the
        // carrier; a seed that does neither is a finding, not a pass.
        let delivered = outcomes
            .iter()
            .filter(|(_, o)| o.delivered.is_some())
            .count();
        let pursued = outcomes.iter().filter(|(_, o)| o.closest < 30.0).count();
        eprintln!(
            "{city}: {delivered} of {} seeds delivered, {pursued} saw the pursuer within 30 m",
            outcomes.len()
        );
        assert!(delivered > 0, "{city}: no seed delivered: {outcomes:?}");
        assert!(
            pursued > 0,
            "{city}: no pursuer closed on a carrier: {outcomes:?}"
        );
        assert!(
            outcomes.iter().all(|(_, o)| o.streets > 0),
            "{city}: street lines"
        );
    }
}
