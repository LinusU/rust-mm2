//! #1443 original-content validation: nothing but the road stands in a
//! car's way on the Golden Gate Bridge deck.
//!
//! Evidence level: original-content validation of the loaded SF collision
//! (downward rays through the real `load_city` colliders). It does not
//! judge how the bridge looks or drives; `docs/research/placement.md`
//! records the drive that found the defect.
//!
//! Opt-in (`MM2_RETAIL=<dir>`); skipped without the operator's install,
//! and says so.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::city::load_city;
use mm2_assets::Vfs;
use mm2_game::SessionEntity;

#[derive(Resource)]
struct VfsRes(Vfs);

/// A physics-enabled world holding the loaded retail city.
fn retail_city_app(vfs: Vfs, psdl: &str) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .add_plugins(TransformPlugin)
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(VfsRes(vfs));
    app.finish();
    app.cleanup();
    let psdl = psdl.to_string();
    app.world_mut()
        .run_system_once(
            move |mut commands: Commands,
                  vfs: Res<VfsRes>,
                  mut meshes: ResMut<Assets<Mesh>>,
                  mut images: ResMut<Assets<Image>>,
                  mut materials: ResMut<Assets<StandardMaterial>>| {
                let mut session = mm2_game::Session::new();
                load_city(
                    &mut commands,
                    &vfs.0,
                    &psdl,
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    SessionEntity(1),
                    &mut session,
                )
                .expect("retail city loads");
            },
        )
        .expect("load_city system runs");
    for _ in 0..3 {
        app.update();
    }
    app
}

/// The deck's road lanes: x spans the 20 m carriageway of rooms 560–571
/// (−1790.3…−1770.3); the samples keep 1.5 m off both kerbs.
const LANE_X: [f32; 5] = [-1788.8, -1785.0, -1780.3, -1775.5, -1771.8];
/// The suspended span between the approaches: Marin end of the Marin
/// side piece (z −2070) to the San Francisco side piece's far end.
const DECK_Z: std::ops::RangeInclusive<i32> = -2070..=-990;
/// What a car's wheels and hull can meet above the road: a collider
/// rising less than this over the surface is a kerb, not an obstacle.
const KERB: f32 = 0.2;
/// Headroom a car needs; collision above this is a gantry, not a wall.
const CLEARANCE: f32 = 3.0;

#[test]
fn nothing_but_the_road_stands_in_a_lane_of_the_golden_gate_deck() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set; the Golden Gate deck was NOT checked");
        return;
    };
    let mut vfs = Vfs::new();
    mm2_assets::mount_install(&mut vfs, &retail, &mm2_assets::InstallMount::default()).unwrap();
    let mut app = retail_city_app(vfs, "city/sf.psdl");

    app.world_mut()
        .run_system_once(|spatial: SpatialQuery, names: Query<&Name>| {
            let mut offences = Vec::new();
            let mut sampled = 0usize;
            for z in DECK_Z.step_by(2) {
                for x in LANE_X {
                    let mut hits = spatial.ray_hits(
                        Vec3::new(x, 60.0, z as f32),
                        Dir3::NEG_Y,
                        80.0,
                        64,
                        true,
                        &SpatialQueryFilter::default(),
                    );
                    hits.sort_by(|a, b| a.distance.total_cmp(&b.distance));
                    let name = |e: Entity| names.get(e).map(|n| n.to_string()).unwrap_or_default();
                    // The road is the highest city-room collider under the
                    // sample: the surface the wheels settle on.
                    let Some(deck) = hits
                        .iter()
                        .find(|h| !name(h.entity).starts_with("prop-"))
                        .map(|h| 60.0 - h.distance)
                    else {
                        continue;
                    };
                    sampled += 1;
                    for h in &hits {
                        let y = 60.0 - h.distance;
                        let n = name(h.entity);
                        if n.starts_with("prop-") && y > deck + KERB && y < deck + CLEARANCE {
                            offences.push(format!(
                                "({x}, {z}): {n} at +{:.2} m over the road",
                                y - deck
                            ));
                        }
                    }
                }
            }
            (sampled, offences)
        })
        .map(|(n, o)| {
            assert!(n > 2000, "only {n} deck samples found a road");
            assert!(
                o.is_empty(),
                "{} invisible obstacles on the deck, first 10:\n{}",
                o.len(),
                o.iter().take(10).cloned().collect::<Vec<_>>().join("\n")
            );
        })
        .expect("deck rays run");
}
