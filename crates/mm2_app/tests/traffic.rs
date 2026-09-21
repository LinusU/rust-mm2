//! F10-A.2 ambient-traffic runtime tests: a synthetic city install
//! (PSDL + BAI + aimap + `va_*` assets) drives the real
//! `load_session_world` → `ambient_setup` → `plan_ambient` → spawn path
//! — cars materialise as session-owned kinematic bodies on authored
//! lanes, `drive_ambient` walks them along the network in the authored
//! direction, dead ends despawn and the recycler respawns to the
//! density target, an event `[Density] 0.0` authors the population
//! off, and teardown removes everything.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::camera::CameraMode;
use mm2_app::contracts::{self, ImpactFilter};
use mm2_app::session::{self, SelectedCar, SessionControl, SpawnPoint, TunedVehicle};
use mm2_app::traffic::{AmbientCar, AmbientTraffic};
use mm2_assets::Vfs;
use mm2_game::{
    DevOverrides, EventRef, EventTableKind, ImpactEvent, Mm2Vfs, Session, SessionConfig,
    SessionMode, SessionPhase, SpawnPose, WorldMode, advance_session_tick,
    despawn_session_entities,
};
use mm2_vehicle::{VehicleConfig, VehiclePlugin};

// ---------------------------------------------------------------------------
// Synthetic install fixtures
// ---------------------------------------------------------------------------

fn write(dir: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

fn push_v3(d: &mut Vec<u8>, v: [f32; 3]) {
    for f in v {
        d.extend_from_slice(&f.to_le_bytes());
    }
}

fn push_f32s(d: &mut Vec<u8>, v: &[f32]) {
    for f in v {
        d.extend_from_slice(&f.to_le_bytes());
    }
}

/// The same `CAI1` fixture `tests/nav_overlay.rs` uses: two roads
/// chained through one intersection at the origin — road 0 spans
/// z −30..−4 (end joins the intersection), road 1 spans z 4..30
/// (start joins it). One vehicle lane + one sidewalk per side.
fn bai_bytes() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"CAI1");
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&2u16.to_le_bytes());

    let write_road =
        |d: &mut Vec<u8>, id: u16, z0: f32, z1: f32, end: (u32, u32), start: (u32, u32)| {
            d.extend_from_slice(&id.to_le_bytes());
            d.extend_from_slice(&2u16.to_le_bytes()); // nSections
            d.extend_from_slice(&0u16.to_le_bytes()); // flags
            d.extend_from_slice(&1u16.to_le_bytes()); // nRooms
            d.extend_from_slice(&1u16.to_le_bytes()); // room 1
            d.extend_from_slice(&7.5f32.to_le_bytes()); // half_width
            d.extend_from_slice(&15.0f32.to_le_bytes()); // base_speed
            for side in [1f32, -1f32] {
                for n in [1u16, 0, 0, 1, 0] {
                    // lanes, trams, trains, sidewalks, ambientTypes
                    d.extend_from_slice(&n.to_le_bytes());
                }
                for _ in 0..2 {
                    for s in [0f32, (z1 - z0).abs()] {
                        d.extend_from_slice(&s.to_le_bytes());
                    }
                }
                for e in [5.0f32, 9.5] {
                    d.extend_from_slice(&e.to_le_bytes());
                }
                d.extend_from_slice(&[0xCDu8; 40]);
                for off in [3.75f32 * side, 8.5f32 * side] {
                    for z in [z0, z1] {
                        push_v3(d, [off, 0.0, z]);
                    }
                }
                for z in [z0, z1] {
                    push_v3(d, [7.5 * side, 0.0, z]);
                }
                for z in [z0, z1] {
                    push_v3(d, [9.5 * side, 0.0, z]);
                }
            }
            for s in [0f32, (z1 - z0).abs()] {
                d.extend_from_slice(&s.to_le_bytes());
            }
            for z in [z0, z1] {
                push_v3(d, [0.0, 0.0, z]);
            }
            for _ in 0..2 {
                push_v3(d, [1.0, 0.0, 0.0]);
            }
            for _ in 0..2 {
                push_v3(d, [0.0, 1.0, 0.0]);
            }
            for _ in 0..2 {
                push_v3(d, [0.0, 0.0, 1.0]);
            }
            for _ in 0..2 {
                push_v3(d, [0.0, 0.0, 1.0]);
            }
            // The file stores `end` first, then `start`.
            for (intersection, road_index) in [end, start] {
                d.extend_from_slice(&intersection.to_le_bytes());
                d.extend_from_slice(&0xCDCDu16.to_le_bytes());
                d.extend_from_slice(&0u16.to_le_bytes());
                d.extend_from_slice(&0u16.to_le_bytes());
                d.extend_from_slice(&road_index.to_le_bytes());
                push_v3(d, [0.0; 3]);
                push_v3(d, [0.0; 3]);
            }
        };
    write_road(
        &mut d,
        0,
        -30.0,
        -4.0,
        (0, 0),
        (0, mm2_formats::bai::END_FILL),
    );
    write_road(
        &mut d,
        1,
        4.0,
        30.0,
        (0, mm2_formats::bai::END_FILL),
        (0, 1),
    );

    d.extend_from_slice(&0u16.to_le_bytes()); // intersection id
    d.extend_from_slice(&1u16.to_le_bytes()); // room
    push_v3(&mut d, [0.0, 0.0, 0.0]);
    d.extend_from_slice(&2u16.to_le_bytes());
    for r in [0u32, 1] {
        d.extend_from_slice(&r.to_le_bytes());
    }
    d.extend_from_slice(&0u32.to_le_bytes()); // culling rooms
    d
}

/// One-room PSDL quad, same as `tests/nav_overlay.rs`.
fn synthetic_psdl() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"PSD0");
    d.extend_from_slice(&2u32.to_le_bytes());
    let verts: &[[f32; 3]] = &[[10., 0., 0.], [10., 0., 10.], [20., 0., 10.], [20., 0., 0.]];
    d.extend_from_slice(&(verts.len() as u32).to_le_bytes());
    for v in verts {
        push_f32s(&mut d, v);
    }
    let heights = [0.15f32, 2.0, 6.0];
    d.extend_from_slice(&(heights.len() as u32).to_le_bytes());
    push_f32s(&mut d, &heights);
    d.extend_from_slice(&1u32.to_le_bytes());
    d.extend_from_slice(&2u32.to_le_bytes()); // nRooms
    d.extend_from_slice(&0u32.to_le_bytes()); // junctions
    let mut room = Vec::new();
    room.extend_from_slice(&4u32.to_le_bytes());
    room.extend_from_slice(&6u32.to_le_bytes());
    for v in [0u16, 1, 2, 3] {
        room.extend_from_slice(&v.to_le_bytes());
        room.extend_from_slice(&0u16.to_le_bytes());
    }
    for w in [0x06u16 << 3, 2, 0, 1, 2, 3] {
        room.extend_from_slice(&w.to_le_bytes());
    }
    d.extend_from_slice(&room);
    d.extend_from_slice(&[0u8; 2]);
    d.extend_from_slice(&[0u8; 2]);
    push_f32s(&mut d, &[10., 0., 0.]);
    push_f32s(&mut d, &[20., 6., 10.]);
    push_f32s(&mut d, &[15., 3., 5.]);
    push_f32s(&mut d, &[10.]);
    d.extend_from_slice(&0u32.to_le_bytes());
    d
}

/// One quad geometry chunk (4 verts, 2 tris) centred on `c`.
fn quad_geo(c: [f32; 3], hx: f32, hy: f32, hz: f32) -> Vec<u8> {
    let mut geo = Vec::new();
    geo.extend_from_slice(&1u32.to_le_bytes());
    geo.extend_from_slice(&4u32.to_le_bytes());
    geo.extend_from_slice(&6u32.to_le_bytes());
    geo.extend_from_slice(&1u32.to_le_bytes());
    geo.extend_from_slice(&0x112u32.to_le_bytes());
    geo.extend_from_slice(&1u16.to_le_bytes());
    geo.extend_from_slice(&0u16.to_le_bytes());
    geo.extend_from_slice(&(-1i32).to_le_bytes());
    geo.extend_from_slice(&3i32.to_le_bytes());
    geo.extend_from_slice(&4u32.to_le_bytes());
    for p in [
        [c[0] - hx, c[1] - hy, c[2] - hz],
        [c[0] + hx, c[1] - hy, c[2] + hz],
        [c[0] + hx, c[1] + hy, c[2] - hz],
        [c[0] - hx, c[1] + hy, c[2] + hz],
    ] {
        for v in p {
            geo.extend_from_slice(&v.to_le_bytes());
        }
        for n in [0.0f32, 1.0, 0.0] {
            geo.extend_from_slice(&n.to_le_bytes());
        }
        for uv in [0.0f32, 0.0] {
            geo.extend_from_slice(&uv.to_le_bytes());
        }
    }
    geo.extend_from_slice(&6u32.to_le_bytes());
    for i in [0u16, 1, 2, 0, 3, 1] {
        geo.extend_from_slice(&i.to_le_bytes());
    }
    geo
}

/// A `va_*` package: one body chunk only — no wheels, no `.mtx` parts.
fn va_pkg() -> Vec<u8> {
    let mut d = b"PKG3".to_vec();
    let geo = quad_geo([0.0, 0.5, 0.0], 0.9, 0.5, 1.6);
    d.extend_from_slice(b"FILE");
    d.push("body_h".len() as u8 + 1);
    d.extend_from_slice(b"body_h");
    d.push(0);
    d.extend_from_slice(&(geo.len() as u32).to_le_bytes());
    d.extend_from_slice(&geo);
    d
}

/// Bound box under the quad — underside at local ~0.
fn va_bnd() -> String {
    let mut s = "version: 1.01\nverts: 8\nmaterials: 1\nedges: 0\npolys: 6\n\n".to_string();
    for v in [
        [-0.9f32, 0.05, -1.6],
        [0.9, 0.05, -1.6],
        [0.9, 0.9, -1.6],
        [-0.9, 0.9, -1.6],
        [-0.9, 0.05, 1.6],
        [0.9, 0.05, 1.6],
        [0.9, 0.9, 1.6],
        [-0.9, 0.9, 1.6],
    ] {
        s.push_str(&format!("v {} {} {}\n", v[0], v[1], v[2]));
    }
    s.push_str("mtl default {\n  elasticity: 0.1\n  friction: 0.5\n}\n");
    for quad in [
        [0, 4, 5, 1],
        [0, 1, 2, 3],
        [4, 7, 6, 5],
        [0, 3, 7, 4],
        [1, 5, 6, 2],
        [3, 2, 6, 7],
    ] {
        s.push_str(&format!(
            "quad {} {} {} {} 0\n",
            quad[0], quad[1], quad[2], quad[3]
        ));
    }
    s
}

const VA_TUNE: &str = "type: a\n\
aiVehicleData {\n\
  Mass 500.0\n\
  Size 1.8 0.9 3.2\n\
  Elasticity 0.1\n\
  Friction 0.5\n\
  MaxDamage 70000.0\n\
  PtxThresh 70000.0\n\
  Spring 17000.0\n\
  Damping 1200.0\n\
  Limit 0.07\n\
  RubberSpring 12000.0\n\
  RubberDamp 600.0\n\
}\n";

/// The city aimap: a two-class roster plus `[Density] 0.25` — authored
/// below the session-config default 0.5, so a correct density chain
/// targets 0.25 × 32 = 8, not 16.
fn city_aimap() -> String {
    "[Ambient Types/Density]\n2\nva_test_a 0.5 0\nva_test_b 1.0 0\n[Density]\n0.25\n".to_string()
}

fn ambient_assets(dir: &Path, id: &str) {
    write(dir, &format!("tune/vehicle/{id}.aivehicledata"), VA_TUNE);
    write(dir, &format!("geometry/{id}.pkg"), va_pkg());
    write(dir, &format!("bound/{id}_bound.bnd"), va_bnd());
}

/// The synthetic city install: PSDL + BAI + aimap + two ambient classes.
fn city_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/test.psdl", synthetic_psdl());
    write(d, "city/test.bai", bai_bytes());
    write(d, "city/test.aimap", city_aimap());
    ambient_assets(d, "va_test_a");
    ambient_assets(d, "va_test_b");
    tmp
}

/// City config with the player quarantined far south of the roads so
/// the 60 m spawn bubble does not cover the whole ~110 m fixture
/// network — `dev.spawn` is the designed escape hatch for exactly
/// this kind of evidence run.
fn city_config() -> SessionConfig {
    SessionConfig {
        world: WorldMode::City {
            psdl: "city/test.psdl".into(),
        },
        dev: DevOverrides {
            spawn: Some(SpawnPose {
                position: Vec3::new(0.0, 1.5, 200.0),
                yaw: 0.0,
            }),
            ..DevOverrides::default()
        },
        ..SessionConfig::default()
    }
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

// ---------------------------------------------------------------------------
// App harness — the production schedule minus window/input
// ---------------------------------------------------------------------------

fn test_app(config: SessionConfig, vfs: Vfs) -> App {
    let mut session = Session::new();
    session.begin(config).unwrap();

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
        .insert_resource(session)
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(TunedVehicle(VehicleConfig::default()))
        .insert_resource(SelectedCar {
            def: None,
            paint: 0,
        })
        .insert_resource(SpawnPoint {
            position: Vec3::new(0.0, 1.5, 0.0),
            yaw: 0.0,
            trailers: Vec::new(),
        })
        .insert_resource(CameraMode::Chase)
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ImpactFilter>()
        .init_resource::<SessionControl>()
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (
                contracts::collect_impacts,
                contracts::publish_vehicle_telemetry,
                mm2_app::traffic::drive_ambient,
                mm2_app::traffic::maintain_ambient,
            )
                .chain(),
        )
        .add_systems(
            Update,
            (
                session::load_session_world.run_if(session::loading),
                session::session_control_input,
                (
                    despawn_session_entities.run_if(session::unloading),
                    session::drive_session,
                )
                    .chain(),
            ),
        );
    app.finish();
    app.cleanup();
    app
}

fn run(app: &mut App, updates: usize) {
    for _ in 0..updates {
        app.update();
    }
}

fn run_until(app: &mut App, max: usize, mut pred: impl FnMut(&mut App) -> bool) -> bool {
    for _ in 0..max {
        app.update();
        if pred(app) {
            return true;
        }
    }
    false
}

fn phase_is(app: &mut App, phase: SessionPhase) -> bool {
    *app.world().resource::<Session>().phase() == phase
}

fn ambient_cars(app: &mut App) -> Vec<(Entity, Vec3)> {
    app.world_mut()
        .query_filtered::<(Entity, &Position), With<AmbientCar>>()
        .iter(app.world())
        .map(|(e, p)| (e, p.0))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// A city session with a rostered aimap spawns the seeded plan's cars
/// as session-owned kinematic bodies on authored lanes — and the same
/// seed replays the identical placement set.
#[test]
fn session_spawns_the_seeded_plan_on_authored_lanes() {
    let install = city_install();
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Playing)),
        "city session never reached Playing"
    );
    let (target, spawned) = {
        let traffic = app
            .world()
            .get_resource::<AmbientTraffic>()
            .expect("rostered city carries the resource");
        (traffic.target, traffic.spawned)
    };
    // Density chain: the event layer is absent, the authored table
    // dial is absent, city aimap [Density] 0.25 beats the config
    // default 0.5 — 0.25 × 32 = 8.
    assert_eq!(target, 8);
    assert!(spawned > 0, "the plan must place cars");
    let cars = ambient_cars(&mut app);
    assert_eq!(cars.len(), spawned);
    // Every spawn sits on a fixture lane (x ±3.75, z −30..30) outside
    // the player's 60 m bubble at z=200.
    for (_, pos) in &cars {
        assert!(pos.is_finite(), "non-finite spawn {pos:?}");
        assert!(pos.x.abs() > 2.0 && pos.x.abs() < 5.0, "off lane: {pos:?}");
        assert!((-30.0..=30.0).contains(&pos.z), "off network: {pos:?}");
        assert!(
            pos.distance(Vec3::new(0.0, 1.5, 200.0)) > 60.0,
            "inside the player bubble: {pos:?}"
        );
    }

    // Same seed → identical placement set on a fresh app.
    let mut app2 = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app2, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    let mut a: Vec<Vec3> = cars.iter().map(|(_, p)| *p).collect();
    a.sort_by_key(|p| p.to_array().map(|c| c.to_bits()));
    let mut b: Vec<Vec3> = ambient_cars(&mut app2).iter().map(|(_, p)| *p).collect();
    b.sort_by_key(|p| p.to_array().map(|c| c.to_bits()));
    assert_eq!(a, b, "seeded placement must replay identically");
}

/// `drive_ambient` walks every car forward along its lane in the
/// authored direction — finite poses throughout — and cars that run
/// out of road despawn while the recycler refills the population.
#[test]
fn cars_drive_the_network_and_dead_ends_recycle() {
    let install = city_install();
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    let spawned = app.world().resource::<AmbientTraffic>().spawned;
    let before: std::collections::HashMap<Entity, Vec3> =
        ambient_cars(&mut app).into_iter().collect();

    run(&mut app, 60); // 1 s: every car moves along its lane.
    let after: std::collections::HashMap<Entity, Vec3> =
        ambient_cars(&mut app).into_iter().collect();
    // Cars present in both snapshots must have advanced; cars that
    // already dead-ended were despawned and any replacements joined
    // mid-window — they only owe a finite pose.
    let mut survivors = 0;
    let mut moved = 0;
    for (e, p) in &after {
        assert!(p.is_finite(), "non-finite pose {p:?}");
        if let Some(b) = before.get(e) {
            survivors += 1;
            if b.distance(*p) > 1.0 {
                moved += 1;
            }
        }
    }
    assert_eq!(moved, survivors, "every surviving car advances");

    // Ten more seconds: the whole network dead-ends — every lane's
    // run is ≤ ~52 m at 15 m/s — so dead-end despawns must fire while
    // the recycler keeps the population at target.
    run(&mut app, 600);
    let (dead_ends, total_spawned, target) = {
        let traffic = app.world().resource::<AmbientTraffic>();
        (traffic.dead_ends, traffic.spawned, traffic.target)
    };
    assert!(dead_ends > 0, "lanes dead-end on this fixture");
    assert!(total_spawned > spawned, "the recycler respawned");
    let active = ambient_cars(&mut app).len();
    assert_eq!(active, target, "population refills to the density bound");
    for (_, pos) in &ambient_cars(&mut app) {
        assert!(pos.is_finite());
        assert!(pos.distance(Vec3::new(0.0, 1.5, 200.0)) <= 400.0);
    }
}

/// An event aimap's `[Density] 0.0` authors the population off — the
/// event layer wins over the city's own `[Density] 0.5` (F10-AC06's
/// consumption leg, synthetic).
#[test]
fn event_aimap_density_zero_authors_traffic_off() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/testcity.psdl", synthetic_psdl());
    write(d, "city/testcity.bai", bai_bytes());
    write(d, "city/testcity.aimap", city_aimap());
    ambient_assets(d, "va_test_a");
    ambient_assets(d, "va_test_b");
    // A checkpoint event whose aimap authors Density 0 — the roster
    // table stays absent so the city's roster carries over.
    write(
        d,
        "race/testcity/mmracedata.csv",
        "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty\nnone,0,0,0,0,0,0.5,0.0,1,50,1,0,0,0,0,0,0.5,0.0,1,40,1\n",
    );
    write(d, "race/testcity/race0.aimap", "[Density]\n0.0\n");
    write(
        d,
        "race/testcity/race0waypoints.csv",
        "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n60,0,140,0,15,0,0,0,\n110,0,140,0,15,0,0,0,\n180,0,140,0,15,0,0,0,\n",
    );
    let config = SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "testcity".into(),
            table: EventTableKind::Checkpoint,
            index: 0,
        }),
        world: WorldMode::City {
            psdl: "city/testcity.psdl".into(),
        },
        ..SessionConfig::default()
    };
    let mut app = test_app(config, vfs_of(d));
    assert!(
        run_until(&mut app, 30, |a| {
            matches!(
                a.world().resource::<Session>().phase(),
                SessionPhase::Playing | SessionPhase::Countdown
            )
        }),
        "event session never left Loading"
    );
    let traffic = app
        .world()
        .get_resource::<AmbientTraffic>()
        .expect("the event still carries the ambient resource");
    assert_eq!(traffic.target, 0, "event [Density] 0.0 authors zero");
    assert!(ambient_cars(&mut app).is_empty());
    run(&mut app, 120);
    assert!(
        ambient_cars(&mut app).is_empty(),
        "the recycler must not outdraw the authored zero"
    );
}

/// Session teardown removes the resource and every car — a restart
/// replans under the same seed rather than leaking the old population.
#[test]
fn teardown_removes_traffic_and_restart_replans() {
    let install = city_install();
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    assert!(!ambient_cars(&mut app).is_empty());

    app.world_mut().resource_mut::<SessionControl>().quit = true;
    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Menu)),
        "quit never reached Menu"
    );
    assert!(app.world().get_resource::<AmbientTraffic>().is_none());
    assert!(
        ambient_cars(&mut app).is_empty(),
        "ambient cars leaked past teardown"
    );

    // Restart through the real intent path: the same seed replans the
    // same initial set on a fresh generation.
    app.world_mut().resource_mut::<SessionControl>().restart = true;
    assert!(run_until(&mut app, 30, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    let first: Vec<Vec3> = {
        let mut v: Vec<Vec3> = ambient_cars(&mut app).iter().map(|(_, p)| *p).collect();
        v.sort_by_key(|p| p.to_array().map(|c| c.to_bits()));
        v
    };
    assert!(!first.is_empty(), "restart replanned the population");
}
