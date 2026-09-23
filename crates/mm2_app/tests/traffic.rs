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
use mm2_app::traffic::{AmbientCar, AmbientDrive, AmbientTraffic, TrafficSignal};
use mm2_assets::Vfs;
use mm2_formats::bai::{Side, VehicleRule};
use mm2_game::{
    DevOverrides, EventRef, EventTableKind, ImpactEvent, LaneCursor, LaneId, LaneKind, Mm2Vfs,
    Player, PlayerControl, PlayerId, PlayerVehicle, Session, SessionConfig, SessionMode,
    SessionPhase, SignalAspect, SpawnPolicy, SpawnPose, StuckWindow, WorldMode,
    advance_session_tick, despawn_session_entities,
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
///
/// `r0_end`/`r1_start` are the authored `vehicleRule` codes on the two
/// junction-connected ends: `r0_end` rules the forward approach into
/// the junction (road 0's right lane), `r1_start` the backward
/// approach (road 1's left lane). `bai_bytes` authors `NeverStop` so
/// the plain fixture keeps its free-flow semantics.
fn bai_bytes() -> Vec<u8> {
    bai_with_rules(3, 3)
}

fn bai_with_rules(r0_end: u16, r1_start: u16) -> Vec<u8> {
    bai_with_lights((r0_end, None), (r1_start, None))
}

/// `bai_with_rules` plus authored `trafficLightOrigin` markers on the
/// two junction-connected ends — `None` writes the zero "no light"
/// marker. The axis authors a fixed `[0,1,0]` whenever a light
/// exists; its convention is unverified and the runtime ignores it.
fn bai_with_lights(r0_end: (u16, Option<[f32; 3]>), r1_start: (u16, Option<[f32; 3]>)) -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"CAI1");
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&2u16.to_le_bytes());

    let write_road = |d: &mut Vec<u8>,
                      id: u16,
                      z0: f32,
                      z1: f32,
                      end: (u32, u32, u16, Option<[f32; 3]>),
                      start: (u32, u32, u16, Option<[f32; 3]>)| {
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
        for (intersection, road_index, rule, light) in [end, start] {
            d.extend_from_slice(&intersection.to_le_bytes());
            d.extend_from_slice(&0xCDCDu16.to_le_bytes());
            d.extend_from_slice(&rule.to_le_bytes());
            d.extend_from_slice(&0u16.to_le_bytes());
            d.extend_from_slice(&road_index.to_le_bytes());
            push_v3(d, light.unwrap_or([0.0; 3]));
            push_v3(d, light.map_or([0.0; 3], |_| [0.0, 1.0, 0.0]));
        }
    };
    write_road(
        &mut d,
        0,
        -30.0,
        -4.0,
        (0, 0, r0_end.0, r0_end.1),
        (0, mm2_formats::bai::END_FILL, 0, None),
    );
    write_road(
        &mut d,
        1,
        4.0,
        30.0,
        (0, mm2_formats::bai::END_FILL, 0, None),
        (0, 1, r1_start.0, r1_start.1),
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

/// One-room PSDL quad, same as `tests/nav_overlay.rs` but widened to
/// span the fixture's whole play space — the BAI lanes at x ±3.75 and
/// the player's quarantine spawn at z=200 all need ground under them
/// (a parked blocker or a bubble centre cannot stand on nothing).
fn synthetic_psdl() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"PSD0");
    d.extend_from_slice(&2u32.to_le_bytes());
    let verts: &[[f32; 3]] = &[
        [-40., 0., -40.],
        [-40., 0., 240.],
        [40., 0., 240.],
        [40., 0., -40.],
    ];
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
    push_f32s(&mut d, &[-40., 0., -40.]);
    push_f32s(&mut d, &[40., 6., 240.]);
    push_f32s(&mut d, &[0., 3., 100.]);
    push_f32s(&mut d, &[160.]);
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
        .init_resource::<mm2_app::damage::DamageReport>()
        .init_resource::<mm2_app::stuck::StuckReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<mm2_app::recovery::RecoveryReport>()
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<mm2_app::spark_fx::SparkFxReport>()
        .init_resource::<mm2_app::texel_fx::TexelDamageReport>()
        .init_resource::<SessionControl>()
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(FixedLast, mm2_app::pause::sync_physics_pause)
        .add_systems(
            FixedLast,
            (
                contracts::collect_impacts,
                contracts::publish_vehicle_telemetry,
                mm2_app::traffic::knock_ambient,
                mm2_app::traffic::drive_ambient,
                mm2_app::traffic::maintain_ambient,
                mm2_app::traffic::drive_signals,
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

/// Teleport the player vehicle — the production `Position` write a
/// `ResetVehicle` lands on, minus the event bookkeeping the driving
/// systems read.
fn teleport_player(app: &mut App, to: Vec3) {
    let mut q = app.world_mut().query_filtered::<(
        &mut Position,
        &mut LinearVelocity,
        &mut AngularVelocity,
        &mut Transform,
    ), With<PlayerVehicle>>();
    let (mut pos, mut lv, mut av, mut t) =
        q.single_mut(app.world_mut()).expect("one player vehicle");
    pos.0 = to;
    lv.0 = Vec3::ZERO;
    av.0 = Vec3::ZERO;
    *t = Transform::from_translation(to);
}

fn player_pos(app: &mut App) -> Vec3 {
    app.world_mut()
        .query_filtered::<&Position, With<PlayerVehicle>>()
        .iter(app.world())
        .next()
        .map(|p| p.0)
        .expect("player vehicle")
}

fn car_state(app: &mut App, car: Entity) -> Option<(Vec3, f32, LaneCursor)> {
    app.world_mut()
        .query::<(Entity, &AmbientCar, &Position)>()
        .iter(app.world())
        .find(|(e, _, _)| *e == car)
        .map(|(_, c, p)| (p.0, c.speed, c.cursor.clone()))
}

/// Spawn a bare lane follower — the `AmbientCar` component plus the
/// kinematic hull `spawn_ambient_car` stamps — straight onto a lane
/// pose. The class index is inert (asset loading never runs for it);
/// `drive_ambient` only needs the cursor and the speed fields.
fn spawn_follower(app: &mut App, lane: LaneId, along: f32, target_speed: f32) -> Entity {
    let (pos, rot) = {
        let traffic = app.world().resource::<AmbientTraffic>();
        let sample = traffic
            .graph()
            .sample_lane(lane, along)
            .expect("a live lane samples");
        let pos = Vec3::from(sample.position);
        let tangent = Vec3::from(sample.tangent).normalize_or(Vec3::NEG_Z);
        let yaw = (-tangent.x).atan2(-tangent.z);
        let pitch = tangent.y.clamp(-1.0, 1.0).asin();
        (pos, Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0))
    };
    app.world_mut()
        .spawn((
            AmbientCar {
                class: 0,
                drive: mm2_app::traffic::AmbientDrive::Lane,
                cursor: LaneCursor::new(lane, along),
                target_speed,
                speed: target_speed.max(0.0),
                stuck: StuckWindow::new(pos.to_array()),
            },
            RigidBody::Kinematic,
            Collider::cuboid(1.8, 0.9, 3.2),
            Position(pos),
            Rotation(rot),
            LinearVelocity::ZERO,
            AngularVelocity::ZERO,
            Transform::from_translation(pos).with_rotation(rot),
        ))
        .id()
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

/// F10-B.1 obstruction response: a participant parked on the lane is a
/// corridor blocker — the follower brakes to the hold gap and waits
/// instead of driving through it, then pulls away when it clears. Runs
/// on the `[Density] 0.0` install so the only car is the spawned
/// follower — the full-density fixture saturates its ~100 m of lanes
/// and keeps the corridor legitimately occupied.
#[test]
fn a_parked_participant_holds_traffic_and_clearing_releases_it() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/test.psdl", synthetic_psdl());
    write(d, "city/test.bai", bai_bytes());
    write(d, "city/test.aimap", density0_aimap());
    ambient_assets(d, "va_test_a");
    ambient_assets(d, "va_test_b");
    let mut app = test_app(city_config(), vfs_of(d));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));

    let lane = {
        let traffic = app.world().resource::<AmbientTraffic>();
        traffic
            .graph()
            .lanes()
            .iter()
            .find(|l| l.arc.is_some() && l.length > 22.0)
            .map(|l| l.id)
            .expect("the fixture authors routable lanes")
    };
    let spot = {
        let traffic = app.world().resource::<AmbientTraffic>();
        Vec3::from(
            traffic
                .graph()
                .sample_lane(lane, 20.0)
                .expect("the lane samples")
                .position,
        )
    };
    // The parked player vehicle is the blocker — the participant an
    // ambient car is most likely to meet. The widened PSDL ground
    // holds it on the lane.
    teleport_player(&mut app, spot + Vec3::Y * 0.4);
    let car = spawn_follower(&mut app, lane, 10.0, 15.0);

    // Three seconds at 15 m/s: the follower must brake to a hold
    // behind the parked player — never closing to contact range —
    // and stay held. Tracking the closest approach stops a drive-
    // through from passing the test.
    let mut min_gap = f32::MAX;
    for _ in 0..360 {
        app.update();
        let (pos, _, _) = car_state(&mut app, car).expect("the held car despawned");
        min_gap = min_gap.min(pos.distance(player_pos(&mut app)));
    }
    assert!(
        min_gap >= 4.0,
        "the follower closed to {min_gap} m of the parked player"
    );
    let (pos, speed, _) = car_state(&mut app, car).unwrap();
    let gap = pos.distance(player_pos(&mut app));
    assert!(
        speed <= 1.0 && gap >= 4.0,
        "expected a hold: speed {speed} gap {gap}"
    );
    assert!(
        app.world().resource::<AmbientTraffic>().queued >= 1,
        "the held car never reported queued"
    );

    // Clear the lane — the held car pulls away again. A dead-end
    // despawn counts as resumed: it had to drive to reach the end.
    teleport_player(&mut app, Vec3::new(0.0, 1.5, 200.0));
    let mut resumed = false;
    for _ in 0..240 {
        app.update();
        match car_state(&mut app, car) {
            None => {
                resumed = true;
                break;
            }
            Some((_, speed, _)) if speed > 2.0 => {
                resumed = true;
                break;
            }
            _ => {}
        }
    }
    assert!(resumed, "the released car never resumed");
}

/// F10 spec req 2's union-of-interest leg at runtime: a remote
/// player's bubble keeps ambient cars alive past the local player's
/// recycle radius — a car beside a far-away participant is a live
/// interaction (F10-AC04 "near any player"), not churn. The control
/// run recycles the whole network when no second player covers it.
#[test]
fn a_remote_players_bubble_holds_traffic_the_local_player_left() {
    let install = city_install();
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    assert!(!ambient_cars(&mut app).is_empty());

    // A second player far south — every fixture lane sits inside its
    // 400 m bubble (270–330 m away) yet past the local player's once
    // the local teleports out.
    app.world_mut().spawn((
        Player {
            id: PlayerId(90),
            control: PlayerControl::Remote,
        },
        Position(Vec3::new(0.0, 0.0, -300.0)),
    ));
    teleport_player(&mut app, Vec3::new(0.0, 0.0, 2000.0));
    run(&mut app, 4);
    assert!(
        !ambient_cars(&mut app).is_empty(),
        "the remote player's bubble must keep the population alive"
    );
    assert_eq!(
        app.world().resource::<AmbientTraffic>().recycled,
        0,
        "no car sits past every player's bubble"
    );

    // Control: the same departure with no second player recycles the
    // whole network — the old single-bubble behaviour.
    let install = city_install();
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    teleport_player(&mut app, Vec3::new(0.0, 0.0, 2000.0));
    run(&mut app, 4);
    assert!(
        ambient_cars(&mut app).is_empty(),
        "cars past the only bubble must recycle"
    );
    assert!(app.world().resource::<AmbientTraffic>().recycled > 0);
}

/// The union covers the draw too: with the local player gone, fresh
/// cars materialise only inside the remote player's band — and never
/// on the far side of the city where no player looks.
#[test]
fn respawns_draw_inside_a_remote_players_band() {
    let install = city_install();
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    assert!(!ambient_cars(&mut app).is_empty());

    app.world_mut().spawn((
        Player {
            id: PlayerId(90),
            control: PlayerControl::Remote,
        },
        Position(Vec3::new(0.0, 0.0, -300.0)),
    ));
    teleport_player(&mut app, Vec3::new(0.0, 0.0, 2000.0));
    // Remove the initial plan — refills can now come only from the
    // remote player's band (the lanes sit 270–330 m from it).
    for (e, _) in ambient_cars(&mut app) {
        app.world_mut().despawn(e);
    }
    run(&mut app, 6);
    let cars = ambient_cars(&mut app);
    assert!(!cars.is_empty(), "the remote player's band must repopulate");
    for (_, pos) in &cars {
        let d = pos.distance(Vec3::new(0.0, 0.0, -300.0));
        assert!(
            (60.0..=400.0).contains(&d),
            "spawn outside the remote band at {d} m: {pos:?}"
        );
    }
    assert_eq!(
        app.world().resource::<AmbientTraffic>().recycled,
        0,
        "the remote bubble holds the network — nothing was collectable"
    );
}

/// The roster ships but `[Density] 0.0` authors the population off —
/// manually spawned followers are then the only cars on the network,
/// so the queue leg is deterministic.
fn density0_aimap() -> String {
    "[Ambient Types/Density]\n2\nva_test_a 0.5 0\nva_test_b 1.0 0\n[Density]\n0.0\n".to_string()
}

/// Ambient cars are corridor blockers too: a participant standing on
/// the lane holds a queue of followers, each at a bounded gap behind
/// the next — the synthetic queue leg of F10-AC02's checklist.
#[test]
fn ambient_cars_queue_behind_a_blocker() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/test.psdl", synthetic_psdl());
    write(d, "city/test.bai", bai_bytes());
    write(d, "city/test.aimap", density0_aimap());
    ambient_assets(d, "va_test_a");
    ambient_assets(d, "va_test_b");
    let mut app = test_app(city_config(), vfs_of(d));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    assert!(
        ambient_cars(&mut app).is_empty(),
        "[Density] 0.0 plans nothing"
    );

    // A lane long enough for the blocker plus two queued followers.
    let lane = {
        let traffic = app.world().resource::<AmbientTraffic>();
        traffic
            .graph()
            .lanes()
            .iter()
            .find(|l| l.arc.is_some() && l.length > 22.0)
            .map(|l| l.id)
            .expect("the fixture authors routable lanes")
    };
    // A bare participant standing on the lane heads the queue — the
    // sense reads `Player` + `Position`, the same contract an opponent
    // or remote driver carries.
    let blocker_at = {
        let traffic = app.world().resource::<AmbientTraffic>();
        Vec3::from(
            traffic
                .graph()
                .sample_lane(lane, 20.0)
                .expect("the lane samples")
                .position,
        )
    };
    app.world_mut().spawn((
        Player {
            id: PlayerId(90),
            control: PlayerControl::Ai,
        },
        Position(blocker_at),
    ));
    let front = spawn_follower(&mut app, lane, 10.0, 15.0);
    let rear = spawn_follower(&mut app, lane, 2.0, 15.0);

    // Track closest approaches: neither follower may close to contact
    // range of what it queues behind.
    let mut min_front = f32::MAX;
    let mut min_rear = f32::MAX;
    for _ in 0..360 {
        app.update();
        if let Some((fp, _, _)) = car_state(&mut app, front) {
            min_front = min_front.min(fp.distance(blocker_at));
            if let Some((rp, _, _)) = car_state(&mut app, rear) {
                min_rear = min_rear.min(rp.distance(fp));
            }
        }
    }
    assert!(
        min_front >= 4.0,
        "the head car reached {min_front} m of the blocker"
    );
    assert!(
        min_rear >= 4.0,
        "the tail car reached {min_rear} m of the head car"
    );
    let (_, fspeed, _) = car_state(&mut app, front).expect("front despawned");
    let (_, rspeed, _) = car_state(&mut app, rear).expect("rear despawned");
    assert!(
        fspeed <= 1.0 && rspeed <= 1.0,
        "the queue never held: {fspeed}/{rspeed}"
    );
    assert!(
        app.world().resource::<AmbientTraffic>().queued >= 2,
        "held followers did not report queued"
    );
}

/// The maintainer runs under the same live-phase gate as the driver —
/// a paused session freezes the population rather than churning
/// recycle/respawn work behind the pause overlay.
#[test]
fn maintain_ambient_holds_during_pause() {
    let install = city_install();
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    run(&mut app, 180); // let the recycler actually churn first
    let (spawns0, recycled0) = {
        let t = app.world().resource::<AmbientTraffic>();
        (t.spawned, t.recycled)
    };
    let frozen: Vec<Vec3> = ambient_cars(&mut app).iter().map(|(_, p)| *p).collect();

    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .expect("a local session pauses");
    run(&mut app, 180);
    let t = app.world().resource::<AmbientTraffic>();
    assert_eq!(
        (t.spawned, t.recycled),
        (spawns0, recycled0),
        "paused traffic kept churning"
    );
    // Kinematic cars may drift one stale physics step before the pause
    // takes hold (the recorded Avian quirk) — half a metre of slack,
    // not the metres a live drive would cover.
    let now: Vec<Vec3> = ambient_cars(&mut app).iter().map(|(_, p)| *p).collect();
    assert_eq!(frozen.len(), now.len());
    for (a, b) in frozen.iter().zip(&now) {
        assert!(a.distance(*b) < 0.5, "paused car drifted {a:?} -> {b:?}");
    }

    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Playing)
        .expect("a paused session resumes");
    run(&mut app, 60);
}

// ---------------------------------------------------------------------------
// F10-B.2 junction rules — authored vehicleRule gates on the lane end
// ---------------------------------------------------------------------------

/// The `[Density] 0.0` install with caller-authored `vehicleRule`
/// codes on the junction's two approach ends — manually spawned
/// followers are the only cars, so junction behaviour is deterministic.
fn junction_install(r0_end: u16, r1_start: u16) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/test.psdl", synthetic_psdl());
    write(d, "city/test.bai", bai_with_rules(r0_end, r1_start));
    write(d, "city/test.aimap", density0_aimap());
    ambient_assets(d, "va_test_a");
    ambient_assets(d, "va_test_b");
    tmp
}

/// The fixture's single vehicle lane of a road side.
fn lane(road: u16, side: Side) -> LaneId {
    LaneId {
        road,
        side,
        index: 0,
        kind: LaneKind::Vehicle,
    }
}

/// Authored `StopSign` on both junction approaches: two followers
/// converging from opposite directions must each stand at their stop
/// line and take the junction in arrival order — the documented
/// "longest waiting vehicle drives first".
#[test]
fn a_stop_sign_serialises_competing_approaches_in_arrival_order() {
    let install = junction_install(0, 0);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));

    let lane_r0 = lane(0, Side::Right);
    let lane_r1 = lane(1, Side::Left);
    // A starts much closer to its stop line — it must win the junction.
    let a = spawn_follower(&mut app, lane_r0, 20.0, 15.0);
    let b = spawn_follower(&mut app, lane_r1, 8.0, 15.0);

    let (mut a_stood, mut b_stood) = (false, false);
    let (mut a_crossed, mut b_crossed) = (usize::MAX, usize::MAX);
    let mut held_seen = 0usize;
    for tick in 0..1200 {
        app.update();
        held_seen = held_seen.max(app.world().resource::<AmbientTraffic>().junction_held);
        if let Some((_, speed, cur)) = car_state(&mut app, a) {
            if cur.lane == lane_r0 {
                if cur.along >= 23.0 && speed <= 1.0 {
                    a_stood = true;
                }
            } else if a_crossed == usize::MAX {
                a_crossed = tick;
            }
        }
        if let Some((_, speed, cur)) = car_state(&mut app, b) {
            if cur.lane == lane_r1 {
                if cur.along >= 23.0 && speed <= 1.0 {
                    b_stood = true;
                }
            } else if b_crossed == usize::MAX {
                b_crossed = tick;
            }
        }
        if a_crossed != usize::MAX && b_crossed != usize::MAX {
            break;
        }
    }
    assert!(a_stood, "A never stood at its stop line");
    assert!(b_stood, "B never stood at its stop line");
    assert!(held_seen >= 1, "no car ever reported junction-held");
    // A crossing never observed would read `usize::MAX` — a car that
    // stalls short of the line must fail here, not sort first.
    assert_ne!(a_crossed, usize::MAX, "A never took the junction");
    assert_ne!(b_crossed, usize::MAX, "B never took the junction");
    assert!(
        a_crossed < b_crossed,
        "the first arrival must take the junction first: {a_crossed}/{b_crossed}"
    );
}

/// The review-caught regression: a follower queued behind the head
/// car resumes from rest several metres short of its stop line. The
/// brake ramp decays the remaining distance geometrically, so without
/// the stop-line tolerance the f32 cursor asymptotes a hair short of
/// the line — never registering in the FCFS queue — and the queue
/// deadlocks forever behind a departed head.
#[test]
fn a_queued_follower_reaches_the_line_and_takes_its_turn() {
    let install = junction_install(0, 0);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));

    let lane_r0 = lane(0, Side::Right);
    // Same approach: A near its stop line, B ~7 m behind — B queues
    // on the corridor sense while A stands, dwells and crosses, then
    // must roll up to the line from a standstill.
    let a = spawn_follower(&mut app, lane_r0, 20.0, 15.0);
    let b = spawn_follower(&mut app, lane_r0, 13.0, 15.0);

    let (mut a_crossed, mut b_crossed) = (usize::MAX, usize::MAX);
    let mut b_stood = false;
    let mut b_registered = false;
    for tick in 0..3600 {
        app.update();
        if let Some((_, speed, cur)) = car_state(&mut app, b)
            && cur.lane == lane_r0
            && cur.along >= 23.0
            && speed <= 1.0
        {
            b_stood = true;
        }
        for (car, crossed) in [(a, &mut a_crossed), (b, &mut b_crossed)] {
            if *crossed == usize::MAX
                && car_state(&mut app, car).is_none_or(|(_, _, c)| c.lane != lane_r0)
            {
                *crossed = tick;
            }
        }
        // After the head departs the queue must seat B — the register
        // the old `dist <= 0` line test never let it reach.
        if a_crossed != usize::MAX && b_crossed == usize::MAX {
            b_registered |= app.world().resource::<AmbientTraffic>().junctions.waiting() >= 1;
        }
        if b_crossed != usize::MAX {
            break;
        }
    }
    assert_ne!(
        a_crossed,
        usize::MAX,
        "the head car never took the junction"
    );
    assert!(b_stood, "the queued follower never stood at the stop line");
    assert!(
        b_registered,
        "the follower never entered the FCFS queue after the head departed"
    );
    assert_ne!(
        b_crossed,
        usize::MAX,
        "the queued follower stalled short of the line — the queue deadlocked"
    );
    assert!(
        a_crossed < b_crossed,
        "the queue served out of order: {a_crossed}/{b_crossed}"
    );
}

/// Authored `TrafficLight` on both junction approaches: the two-member
/// signal cycle admits one road at a time — a car may only transfer
/// while its own road holds the phase green.
#[test]
fn a_traffic_light_admits_only_the_green_member_road() {
    let install = junction_install(1, 1);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    {
        // A quick cycle keeps the window bounded: two-second greens,
        // half-second all-red.
        let mut t = app.world_mut().resource_mut::<AmbientTraffic>();
        t.junctions.policy.green_ticks = 120;
        t.junctions.policy.clear_ticks = 60;
    }
    let lane_r0 = lane(0, Side::Right);
    let lane_r1 = lane(1, Side::Left);
    let a = spawn_follower(&mut app, lane_r0, 20.0, 15.0);
    let b = spawn_follower(&mut app, lane_r1, 20.0, 15.0);

    // Two fixed drive steps run per update, so the transfer tick's
    // phase is the current or previous green read.
    let mut prev_green = None;
    let mut crossed = [false; 2];
    for _ in 0..2000 {
        app.update();
        let green = {
            let t = app.world().resource::<AmbientTraffic>();
            t.junctions.green_road(t.graph(), 0)
        };
        for (car, approach, road) in [(a, lane_r0, 0u16), (b, lane_r1, 1u16)] {
            let i = road as usize;
            if crossed[i] {
                continue;
            }
            match car_state(&mut app, car) {
                // Still approaching = on the approach lane with no
                // crossing committed. A committed crossing counts as
                // crossed now: the gate admitted the car this tick,
                // while the lane-id change only lands after the
                // interior path completes — possibly phases later.
                Some((_, _, cur)) if cur.lane == approach && cur.crossing.is_none() => {}
                Some(_) => {
                    assert!(
                        green == Some(road) || prev_green == Some(road),
                        "road-{road} approach crossed on {green:?} (prev {prev_green:?})"
                    );
                    crossed[i] = true;
                }
                None => panic!("car despawned before an observed transfer"),
            }
        }
        prev_green = green;
        if crossed == [true, true] {
            break;
        }
    }
    assert_eq!(crossed, [true, true], "both approaches must get a green");
}

/// F10-B.9 (operator report 4 item 1): a follower on a free-flow
/// approach crosses the junction interior in continuous steps — the
/// box is traversed, not teleported. `crossings` counts the commit
/// and the `jumps` watchdog stays at zero.
#[test]
fn a_lane_follower_crosses_the_junction_interior_without_a_jump() {
    let install = junction_install(3, 3); // NeverStop: free flow
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    let lane_r0 = lane(0, Side::Right);
    let car = spawn_follower(&mut app, lane_r0, 10.0, 15.0);

    // Watch the pose every update: the follower must be observed
    // inside the box (z −4..4) on its crossing path, and every
    // per-update move must stay within a driven step — the old
    // transfer jumped the ~8 m interior in one tick.
    let mut prev = car_state(&mut app, car).expect("the car despawned").0;
    let (mut saw_inside, mut max_step) = (false, 0.0f32);
    for _ in 0..600 {
        app.update();
        let Some((pos, _, cur)) = car_state(&mut app, car) else {
            break;
        };
        max_step = max_step.max(pos.distance(prev));
        prev = pos;
        if cur.lane != lane_r0 {
            break;
        }
        if cur.crossing.is_some() && (-4.0..=4.0).contains(&pos.z) {
            saw_inside = true;
        }
    }
    assert!(saw_inside, "the car never traversed the junction interior");
    // Two fixed drive steps run per update at ≤15 m/s — a real move
    // stays under a metre; the teleport would read ~8 m at once.
    assert!(
        max_step < 1.0,
        "pose discontinuity: {max_step} m in one update"
    );
    let t = app.world().resource::<AmbientTraffic>();
    assert!(t.crossings >= 1, "no committed crossing was recorded");
    assert_eq!(t.jumps, 0, "the continuity watchdog counted a teleport");
}

/// A held red proves the gate closes, not just that greens admit:
/// spawn the follower at the stop line while the *other* member holds
/// green — its own red is then guaranteed for at least that green's
/// length — and it must stand until its own road's phase comes.
#[test]
fn a_red_window_holds_the_approach_until_its_road_greens() {
    let install = junction_install(1, 1);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    {
        // Long phases so the guaranteed red comfortably outlasts the
        // decel ramp from spawn speed to a standstill at the line.
        let mut t = app.world_mut().resource_mut::<AmbientTraffic>();
        t.junctions.policy.green_ticks = 480;
        t.junctions.policy.clear_ticks = 120;
    }
    let lane_r0 = lane(0, Side::Right);

    // Wait for road 1's green — road 0 is then red for the whole phase.
    assert!(
        run_until(&mut app, 1500, |a| {
            let t = a.world().resource::<AmbientTraffic>();
            t.junctions.green_road(t.graph(), 0) == Some(1)
        }),
        "road 1 never held green"
    );
    let car = spawn_follower(&mut app, lane_r0, 22.0, 15.0);

    // On red the car must never pass the stop line (along 23.5 → z
    // −6.5) and must brake down to a standstill at it — `junction_speed`
    // decelerates at the policy rate rather than snapping to zero, so
    // the hold is judged by position, not by an instant speed of 0.
    let mut red_ticks = 0usize;
    let mut stood_at_line = false;
    let mut released = false;
    for _ in 0..600 {
        app.update();
        let green = {
            let t = app.world().resource::<AmbientTraffic>();
            t.junctions.green_road(t.graph(), 0)
        };
        let Some((pos, speed, cur)) = car_state(&mut app, car) else {
            break;
        };
        if cur.lane != lane_r0 {
            released = true;
            break;
        }
        if green != Some(0) {
            red_ticks += 1;
            assert!(pos.z < -6.4, "crossed the stop line on red: {pos:?}");
            stood_at_line |= cur.along >= 23.0 && speed <= 1.0;
        }
    }
    assert!(
        red_ticks >= 30,
        "spawned inside road 1's green, road 0's red ended after {red_ticks} ticks"
    );
    assert!(stood_at_line, "the held car never stood at its stop line");
    assert!(released, "the held car never got its green");
}

/// The authored `AlwaysStop` end never releases: the follower brakes
/// to the stop line and stands while the junction's other approach —
/// authored `NeverStop` — flows through freely. Per-approach rules at
/// a mixed junction, the retail norm.
#[test]
fn an_always_stop_end_never_releases_while_the_never_stop_flows() {
    let install = junction_install(2, 3);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));

    let lane_r0 = lane(0, Side::Right);
    let lane_r1 = lane(1, Side::Left);
    let held = spawn_follower(&mut app, lane_r0, 18.0, 15.0);
    let free = spawn_follower(&mut app, lane_r1, 10.0, 15.0);

    let mut free_crossed = false;
    let mut held_seen = 0usize;
    for _ in 0..360 {
        app.update();
        held_seen = held_seen.max(app.world().resource::<AmbientTraffic>().junction_held);
        let (pos, _, cur) = car_state(&mut app, held).expect("the held car despawned");
        assert!(
            pos.z < -4.0,
            "an AlwaysStop car entered the junction: {pos:?}"
        );
        assert_eq!(cur.lane, lane_r0, "an AlwaysStop car transferred");
        if car_state(&mut app, free).is_none_or(|(_, _, c)| c.lane != lane_r1) {
            free_crossed = true;
        }
    }
    assert!(free_crossed, "the NeverStop approach never crossed");
    assert!(held_seen >= 1, "the held car never reported junction-held");
    let (_, speed, cur) = car_state(&mut app, held).unwrap();
    assert!(
        speed <= 1.0 && cur.along >= 23.0,
        "not standing at the line: {cur:?} v={speed}"
    );
}

/// F10-AC04's junction leg: a transfer that would land inside a live
/// blocker is rejected — the car holds at its lane end and retries
/// rather than materialising inside a junction queue.
#[test]
fn an_occupied_exit_lane_holds_the_transfer_at_the_lane_end() {
    let install = junction_install(3, 3);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));

    let lane_r0 = lane(0, Side::Right);
    // Parked off the follower's corridor (3.75 m lateral, beyond the
    // 2.4 m half-width) but inside the entry clearance of the
    // r1-right landing — only the transfer check can see it.
    let blocker = app
        .world_mut()
        .spawn((
            Player {
                id: PlayerId(92),
                control: PlayerControl::Ai,
            },
            Position(Vec3::new(0.0, 0.0, 2.0)),
        ))
        .id();
    let car = spawn_follower(&mut app, lane_r0, 15.0, 15.0);

    for _ in 0..240 {
        app.update();
        let (pos, _, cur) = car_state(&mut app, car).expect("the held car despawned");
        assert_eq!(cur.lane, lane_r0, "transferred into an occupied exit");
        assert!(pos.z < -3.0, "entered the junction box: {pos:?}");
        assert!(
            pos.distance(Vec3::new(0.0, 0.0, 2.0)) > 3.5,
            "materialised inside the blocker: {pos:?}"
        );
    }
    // Clearing the exit releases the turn.
    app.world_mut().despawn(blocker);
    assert!(
        run_until(&mut app, 240, |a| {
            car_state(a, car).is_none_or(|(_, _, c)| c.lane != lane_r0)
        }),
        "the freed exit was never taken"
    );
}

// ---------------------------------------------------------------------------
// F10-B.3 bounded stuck recovery
// ---------------------------------------------------------------------------

/// A follower penned behind a parked participant forever makes no
/// progress: once `window_ticks` pass without `min_displacement` of
/// movement it leaves the world — despawn into the pool the
/// maintainer refills is the bounded recovery, never a drive-through
/// — and every car penned the same way recovers the same way, counted
/// as `traffic.stuck` outcomes (F10-AC05's stuck reporting leg).
#[test]
fn a_penned_car_is_recycled_after_the_stuck_window() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/test.psdl", synthetic_psdl());
    write(d, "city/test.bai", bai_bytes());
    write(d, "city/test.aimap", density0_aimap());
    ambient_assets(d, "va_test_a");
    ambient_assets(d, "va_test_b");
    let mut app = test_app(city_config(), vfs_of(d));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    {
        // A two-second window: comfortably past the approach-and-
        // brake, far under any wait the junction rules could
        // legitimately impose.
        let mut t = app.world_mut().resource_mut::<AmbientTraffic>();
        t.stuck_policy.window_ticks = 240;
    }
    let lane = {
        let traffic = app.world().resource::<AmbientTraffic>();
        traffic
            .graph()
            .lanes()
            .iter()
            .find(|l| l.arc.is_some() && l.length > 22.0)
            .map(|l| l.id)
            .expect("the fixture authors routable lanes")
    };
    let blocker_at = {
        let traffic = app.world().resource::<AmbientTraffic>();
        Vec3::from(
            traffic
                .graph()
                .sample_lane(lane, 20.0)
                .expect("the lane samples")
                .position,
        )
    };
    app.world_mut().spawn((
        Player {
            id: PlayerId(93),
            control: PlayerControl::Ai,
        },
        Position(blocker_at),
    ));

    // Two fixed drive ticks run per update (120 Hz under a 1/60
    // manual clock), so the 240-tick window cannot fire before ~120
    // updates of standing.
    for (i, spawn_at) in [10.0f32, 8.0].iter().enumerate() {
        let car = spawn_follower(&mut app, lane, *spawn_at, 15.0);
        let mut min_gap = f32::MAX;
        let mut despawned_at = None;
        for tick in 0..600 {
            app.update();
            match car_state(&mut app, car) {
                Some((pos, _, _)) => min_gap = min_gap.min(pos.distance(blocker_at)),
                None => {
                    despawned_at = Some(tick);
                    break;
                }
            }
        }
        let at = despawned_at.expect("the penned car was never recovered");
        assert!(
            at >= 100,
            "car {i} despawned at update {at} — the 240-tick window never ran"
        );
        assert!(
            min_gap >= 4.0,
            "car {i} reached {min_gap} m of the blocker — the recovery drove through it"
        );
        assert_eq!(
            app.world().resource::<AmbientTraffic>().stuck,
            i + 1,
            "each penned car must count as a stuck outcome"
        );
    }
    // The lane ahead of the blocker is empty because the cars left
    // the world — nothing teleported through it.
    for (_, pos) in ambient_cars(&mut app) {
        assert!(
            pos.distance(blocker_at) >= 4.0,
            "a car materialised past the pen: {pos:?}"
        );
    }
}

/// The window must not trip on a legitimate hold: a car standing at a
/// red light waits at most one member cycle — far under the designed
/// bound — then crosses on its green, its window resetting on the
/// first metres of progress.
#[test]
fn a_signal_wait_shorter_than_the_window_never_recovers() {
    let install = junction_install(1, 1);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    {
        let mut t = app.world_mut().resource_mut::<AmbientTraffic>();
        // A 1200-tick window triples the worst hold this junction can
        // impose (one member cycle: 240 green + 120 all-red).
        t.stuck_policy.window_ticks = 1200;
        t.junctions.policy.green_ticks = 240;
        t.junctions.policy.clear_ticks = 120;
    }
    let lane_r0 = lane(0, Side::Right);

    // Spawn inside road 1's green so the car's whole red stands ahead
    // of it.
    assert!(
        run_until(&mut app, 1500, |a| {
            let t = a.world().resource::<AmbientTraffic>();
            t.junctions.green_road(t.graph(), 0) == Some(1)
        }),
        "road 1 never held green"
    );
    let car = spawn_follower(&mut app, lane_r0, 20.0, 15.0);

    // 1000 updates = 2000 drive ticks > the window: the car must
    // cross on its green long before then, alive throughout.
    let mut crossed = false;
    for _ in 0..1000 {
        app.update();
        match car_state(&mut app, car) {
            Some((_, _, cur)) if cur.lane == lane_r0 => {}
            Some(_) => {
                crossed = true;
                break;
            }
            None => panic!("a legitimate signal wait tripped the stuck recovery"),
        }
    }
    assert!(crossed, "the held car never got its green");
    assert_eq!(
        app.world().resource::<AmbientTraffic>().stuck,
        0,
        "a legitimate red wait counted as stuck"
    );
}

// ---------------------------------------------------------------------------
// F10-B.4 spawn occupancy — refill draws reject occupied space (F10-AC04)
// ---------------------------------------------------------------------------

/// The maintainer's refill draw rejects space a live ambient car
/// occupies: keep one parked car, drop the rest, and bind an
/// exclusion box wider than the whole fixture so every draw must land
/// occupied — deterministic proof of the rejection, not a hope that
/// the seeded draw happens to sample near the blocker.
#[test]
fn respawns_reject_space_a_live_car_occupies() {
    let install = city_install();
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    let cars = ambient_cars(&mut app);
    assert!(!cars.is_empty(), "the plan must seed a population");
    let (keeper, _) = cars[0];
    for (e, _) in cars.iter().skip(1) {
        app.world_mut().entity_mut(*e).despawn();
    }
    {
        // Pin the keeper so it cannot dead-end out of the test, and
        // widen the exclusion box past the fixture network.
        let mut q = app.world_mut().query::<&mut AmbientCar>();
        let mut car = q.get_mut(app.world_mut(), keeper).expect("keeper");
        car.target_speed = 0.0;
        car.speed = 0.0;
        let mut t = app.world_mut().resource_mut::<AmbientTraffic>();
        t.policy.spawn_clearance = 1.0e6;
        t.policy.spawn_half_width = 1.0e6;
        t.policy.spawn_max_rise = 1.0e6;
    }
    let spawned0 = app.world().resource::<AmbientTraffic>().spawned;
    run(&mut app, 120);
    let t = app.world().resource::<AmbientTraffic>();
    assert_eq!(t.spawned, spawned0, "a draw landed in occupied space");
    assert_eq!(
        ambient_cars(&mut app).len(),
        1,
        "only the parked keeper remains"
    );

    // Control: the same session under the default box refills — the
    // rejection above was the box, not a broken respawner.
    app.world_mut().resource_mut::<AmbientTraffic>().policy = SpawnPolicy::default();
    run(&mut app, 120);
    assert!(
        app.world().resource::<AmbientTraffic>().spawned > spawned0,
        "the default box must allow refills"
    );
}

/// The same rejection covers participant-occupied space — the
/// `Player` + `Position` contract AI opponents and remote drivers
/// share. With no ambient cars left, a parked participant's box
/// still refuses every refill draw.
#[test]
fn respawns_reject_space_a_participant_occupies() {
    let install = city_install();
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    assert!(
        !ambient_cars(&mut app).is_empty(),
        "the plan must seed a population"
    );
    for (e, _) in ambient_cars(&mut app) {
        app.world_mut().entity_mut(e).despawn();
    }
    // A bare participant standing south of the network — inside the
    // union bubble and past every lane's 60 m exclusion, so draws
    // reach the occupied-space check the box performs.
    app.world_mut().spawn((
        Player {
            id: PlayerId(94),
            control: PlayerControl::Ai,
        },
        Position(Vec3::new(3.75, 0.0, -150.0)),
    ));
    {
        let mut t = app.world_mut().resource_mut::<AmbientTraffic>();
        t.policy.spawn_clearance = 1.0e6;
        t.policy.spawn_half_width = 1.0e6;
        t.policy.spawn_max_rise = 1.0e6;
    }
    let spawned0 = app.world().resource::<AmbientTraffic>().spawned;
    run(&mut app, 120);
    let t = app.world().resource::<AmbientTraffic>();
    assert_eq!(t.spawned, spawned0, "a draw landed in occupied space");
    assert!(ambient_cars(&mut app).is_empty(), "no respawn may land");

    // Same control: the default box refills again.
    app.world_mut().resource_mut::<AmbientTraffic>().policy = SpawnPolicy::default();
    run(&mut app, 120);
    assert!(
        app.world().resource::<AmbientTraffic>().spawned > spawned0,
        "the default box must allow refills"
    );
}

/// A participant parked in the junction box is invisible to the
/// corridor sense (6.75 m lateral > the 2.4 m half-width) and to the
/// landing check (9.7 m from the turn's landing > the 6 m entry
/// clearance) — only the box-yield sees it. The green-lit approach
/// must stand at its stop line through a whole green while the box
/// is occupied, then cross once it clears (F10-AC02's right-of-way
/// leg; the "player blocks intersection" edge case).
#[test]
fn a_participant_in_the_box_yields_the_green_approach() {
    let install = junction_install(1, 1);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    {
        let mut t = app.world_mut().resource_mut::<AmbientTraffic>();
        t.junctions.policy.green_ticks = 120;
        t.junctions.policy.clear_ticks = 60;
    }
    // Inside the zone (|XZ| 4.2 < the 7 m radius) but outside every
    // other check's reach.
    let blocker = app
        .world_mut()
        .spawn((
            Player {
                id: PlayerId(95),
                control: PlayerControl::Ai,
            },
            Position(Vec3::new(-3.0, 0.0, -3.0)),
        ))
        .id();
    let lane_r0 = lane(0, Side::Right);
    let car = spawn_follower(&mut app, lane_r0, 15.0, 15.0);

    // The car must reach its stop line and stand through at least one
    // whole green — a turn that ignored the occupied box would cross
    // on the first green it met.
    let mut stood_on_green = 0usize;
    let mut held_seen = 0usize;
    for _ in 0..300 {
        app.update();
        held_seen = held_seen.max(app.world().resource::<AmbientTraffic>().junction_held);
        let green = {
            let t = app.world().resource::<AmbientTraffic>();
            t.junctions.green_road(t.graph(), 0)
        };
        let (pos, speed, cur) = car_state(&mut app, car).expect("the yielded car despawned");
        assert_eq!(cur.lane, lane_r0, "entered an occupied box");
        // The stop line sits at z = −6.5; the lane end at −4. Held at
        // the line means z < −5 — the occupied-exit revert alone would
        // leave the car oscillating at the lane end (along ≈ 26).
        assert!(
            pos.z < -5.0,
            "past the stop line inside an occupied box: {pos:?}"
        );
        assert!(
            cur.along <= 24.0,
            "oscillating at the lane end — the yield never ran: {cur:?}"
        );
        if green == Some(0) && cur.along >= 23.0 && speed <= 1.0 {
            stood_on_green += 1;
        }
    }
    assert!(
        held_seen >= 1,
        "the yielded car never reported junction-held"
    );
    assert!(
        stood_on_green >= 40,
        "the car did not stand through a green: {stood_on_green}"
    );

    // Clearing the box releases the yield on a later green.
    app.world_mut().despawn(blocker);
    assert!(
        run_until(&mut app, 1500, |a| {
            car_state(a, car).is_none_or(|(_, _, c)| c.lane != lane_r0)
        }),
        "the cleared box never released the approach"
    );
}

/// An ambient car parked inside the box on a lane bound for a dead
/// end — not for this junction — occupies it: the FCFS-registered
/// stop-sign head stands at its stop line through its dwell instead
/// of transferring and reverting at the lane end, which is what the
/// occupied-exit check alone would produce.
#[test]
fn an_ambient_car_in_the_box_yields_the_stop_sign_head() {
    let install = junction_install(0, 0);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));

    // Parked mid-box on road 1's forward lane (z ≈ 4.5, inside the
    // 7 m zone); its downstream end is a dead end, so `bound_for`
    // does not shield it from occupying the junction.
    let occupant = spawn_follower(&mut app, lane(1, Side::Right), 0.5, 0.0);
    let lane_r0 = lane(0, Side::Right);
    let car = spawn_follower(&mut app, lane_r0, 18.0, 15.0);

    let mut stood_at_line = false;
    for _ in 0..240 {
        app.update();
        let (pos, speed, cur) = car_state(&mut app, car).expect("the yielded car despawned");
        assert_eq!(cur.lane, lane_r0, "entered an occupied box");
        assert!(
            cur.along <= 24.0,
            "held at the lane end instead of the stop line — the yield never ran: {cur:?}"
        );
        assert!(
            pos.z < -5.0,
            "past the stop line inside an occupied box: {pos:?}"
        );
        stood_at_line |= cur.along >= 23.0 && speed <= 1.0;
    }
    assert!(
        stood_at_line,
        "the yielded car never stood at its stop line"
    );
    assert!(
        app.world().resource::<AmbientTraffic>().junctions.waiting() >= 1,
        "the yielded head never registered in the FCFS queue"
    );

    // Clearing the box releases the head — its dwell long elapsed.
    app.world_mut().despawn(occupant);
    assert!(
        run_until(&mut app, 240, |a| {
            car_state(a, car).is_none_or(|(_, _, c)| c.lane != lane_r0)
        }),
        "the cleared box never released the stop-sign head"
    );
}

// ---------------------------------------------------------------------------
// F10-B.6 collision handover — a hard hit flips the follower to dynamic
// ---------------------------------------------------------------------------

/// A heavy dynamic striker parked mid-lane is invisible to the
/// corridor sense (only `Player`s and ambient cars are blockers), so
/// the follower drives into it at full speed: the first contact's
/// impulse estimate (~15 m/s × 1300 kg) clears `KnockPolicy` and the
/// same entity flips to `RigidBody::Dynamic` with its lane cursor
/// frozen — the solver owns the wreck from then on, and no duplicate
/// body appears (the spec's "dynamic/kinematic handover" edge case;
/// F10-AC03's collision-fidelity leg).
#[test]
fn a_hard_hit_hands_the_follower_to_dynamics() {
    let install = junction_install(0, 0);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));

    let lane_r0 = lane(0, Side::Right);
    let car = spawn_follower(&mut app, lane_r0, 10.0, 15.0);
    // A 1300 kg striker resting mid-lane 4 m ahead of the follower.
    let spot = {
        let t = app.world().resource::<AmbientTraffic>();
        let s = t
            .graph()
            .sample_lane(lane_r0, 14.0)
            .expect("a live lane samples");
        Vec3::from(s.position) + Vec3::Y * 0.55
    };
    let striker = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.8, 1.1, 1.8),
            Mass(1300.0),
            CollisionEventsEnabled,
            Position(spot),
            Transform::from_translation(spot),
        ))
        .id();

    assert!(
        run_until(&mut app, 120, |a| {
            a.world().resource::<AmbientTraffic>().knocked >= 1
        }),
        "the hard hit never knocked the follower"
    );

    let (drive, body, kick, cursor) = {
        let mut q = app
            .world_mut()
            .query::<(Entity, &AmbientCar, &RigidBody, &LinearVelocity)>();
        let (_, c, rb, lv) = q
            .iter(app.world())
            .find(|(e, ..)| *e == car)
            .expect("the knocked car despawned");
        assert!(matches!(rb, RigidBody::Dynamic), "stayed kinematic");
        assert_eq!(c.drive, AmbientDrive::Knocked);
        // Bounded energy: the kick adds at most the approach speed
        // along the contact normal — nothing artificial.
        assert!(
            lv.0.is_finite() && lv.0.length() <= 35.0,
            "unbounded kick velocity: {lv:?}"
        );
        (c.drive, *rb, lv.0, c.cursor.clone())
    };
    let _ = (drive, body, kick);

    // The lane driver never advances a knocked cursor again, while the
    // solver keeps posing the same entity — no duplicate body.
    run(&mut app, 60);
    let (frozen, pos, still_dynamic) = {
        let mut q = app
            .world_mut()
            .query::<(Entity, &AmbientCar, &Position, &RigidBody)>();
        let (_, c, p, rb) = q
            .iter(app.world())
            .find(|(e, ..)| *e == car)
            .expect("the wreck despawned");
        (c.cursor.clone(), p.0, matches!(rb, RigidBody::Dynamic))
    };
    assert!(still_dynamic);
    assert_eq!(frozen, cursor, "the lane driver re-posed a knocked car");
    assert!(pos.is_finite(), "the solver diverged: {pos:?}");
    let knocked = app
        .world_mut()
        .query::<&AmbientCar>()
        .iter(app.world())
        .filter(|c| c.drive == AmbientDrive::Knocked)
        .count();
    assert_eq!(knocked, 1, "a duplicate wreck appeared");

    // The striker was physically shoved — the pair really collided.
    let p = app
        .world()
        .get::<Position>(striker)
        .expect("the striker despawned")
        .0;
    assert!(
        p.distance(spot) > 0.3,
        "the striker never felt the impact: {p:?}"
    );
}

/// A 50 kg cone-mass striker only registers ~750 N·s of estimated
/// impulse — under the designed 4000 floor — so the follower stays a
/// kinematic lane follower and drives on (spec: a light touch must
/// not hand over).
#[test]
fn a_light_touch_leaves_the_car_lane_following() {
    let install = junction_install(0, 0);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));

    let lane_r0 = lane(0, Side::Right);
    let car = spawn_follower(&mut app, lane_r0, 10.0, 15.0);
    let spot = {
        let t = app.world().resource::<AmbientTraffic>();
        let s = t
            .graph()
            .sample_lane(lane_r0, 14.0)
            .expect("a live lane samples");
        Vec3::from(s.position) + Vec3::Y * 0.55
    };
    app.world_mut().spawn((
        RigidBody::Dynamic,
        Collider::cuboid(1.8, 1.1, 1.8),
        Mass(50.0),
        CollisionEventsEnabled,
        Position(spot),
        Transform::from_translation(spot),
    ));

    // ~9 s — long past the striker and several contact ticks.
    run(&mut app, 90);
    assert_eq!(
        app.world().resource::<AmbientTraffic>().knocked,
        0,
        "a light touch knocked the follower"
    );
    let (drive, body, cur) = {
        let mut q = app.world_mut().query::<(Entity, &AmbientCar, &RigidBody)>();
        let (_, c, rb) = q
            .iter(app.world())
            .find(|(e, ..)| *e == car)
            .expect("the car despawned");
        (c.drive, *rb, c.cursor.clone())
    };
    assert_eq!(drive, AmbientDrive::Lane);
    assert!(matches!(body, RigidBody::Kinematic));
    assert!(
        cur.lane != lane_r0 || cur.along > 20.0,
        "the car never drove past the light striker: {cur:?}"
    );
}

/// A knocked wreck lying inside the junction box — its lane cursor
/// still nominally bound for this junction — is not shielded by
/// `bound_for` (only `Lane`-mode cars are) and so occupies the box:
/// the green-lit approach stands at its stop line through a whole
/// green until the wreck is removed (F10-AC03: a wreck remains a
/// physical obstacle in the box).
#[test]
fn a_knocked_wreck_occupies_the_junction_box() {
    let install = junction_install(1, 1);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    {
        let mut t = app.world_mut().resource_mut::<AmbientTraffic>();
        t.junctions.policy.green_ticks = 120;
        t.junctions.policy.clear_ticks = 60;
    }

    let lane_r0 = lane(0, Side::Right);
    // The wreck: knocked on the approach lane, lying inside the 7 m
    // zone but 6.75 m lateral of the approach corridor — only the
    // box-yield sees it. Its cursor still points at this junction, so
    // a `Lane`-mode car here would be bound_for-excluded; `Knocked`
    // must not be.
    let wreck_pos = Vec3::new(-3.0, 0.45, -3.0);
    let wreck = app
        .world_mut()
        .spawn((
            AmbientCar {
                class: 0,
                drive: AmbientDrive::Knocked,
                cursor: LaneCursor::new(lane_r0, 26.0),
                target_speed: 0.0,
                speed: 0.0,
                stuck: StuckWindow::new(wreck_pos.to_array()),
            },
            RigidBody::Dynamic,
            Collider::cuboid(1.8, 0.9, 3.2),
            Mass(1200.0),
            CollisionEventsEnabled,
            Position(wreck_pos),
            LinearVelocity::ZERO,
            AngularVelocity::ZERO,
            Transform::from_translation(wreck_pos),
        ))
        .id();
    let car = spawn_follower(&mut app, lane_r0, 15.0, 15.0);

    let mut stood_on_green = 0usize;
    let mut held_seen = 0usize;
    for _ in 0..300 {
        app.update();
        held_seen = held_seen.max(app.world().resource::<AmbientTraffic>().junction_held);
        let green = {
            let t = app.world().resource::<AmbientTraffic>();
            t.junctions.green_road(t.graph(), 0)
        };
        let (pos, speed, cur) = car_state(&mut app, car).expect("the yielded car despawned");
        assert_eq!(cur.lane, lane_r0, "entered an occupied box");
        assert!(
            pos.z < -5.0,
            "past the stop line inside an occupied box: {pos:?}"
        );
        assert!(
            cur.along <= 24.0,
            "oscillating at the lane end — the yield never ran: {cur:?}"
        );
        if green == Some(0) && cur.along >= 23.0 && speed <= 1.0 {
            stood_on_green += 1;
        }
    }
    assert!(
        held_seen >= 1,
        "the yielded car never reported junction-held"
    );
    assert!(
        stood_on_green >= 40,
        "the car did not stand through a green: {stood_on_green}"
    );

    // Clearing the wreck releases the yield on a later green.
    app.world_mut().despawn(wreck);
    assert!(
        run_until(&mut app, 1500, |a| {
            car_state(a, car).is_none_or(|(_, _, c)| c.lane != lane_r0)
        }),
        "the cleared box never released the approach"
    );
}

// ---------------------------------------------------------------------------
// F10-B.7 authored signal indicators
// ---------------------------------------------------------------------------

/// A city install whose junction ends author `trafficLightOrigin`
/// markers — `r0_end`/`r1_start` are `(vehicleRule, Option<origin>)`.
fn signal_install(
    r0_end: (u16, Option<[f32; 3]>),
    r1_start: (u16, Option<[f32; 3]>),
) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/test.psdl", synthetic_psdl());
    write(d, "city/test.bai", bai_with_lights(r0_end, r1_start));
    write(d, "city/test.aimap", city_aimap());
    ambient_assets(d, "va_test_a");
    ambient_assets(d, "va_test_b");
    tmp
}

/// The live signal indicators as `(junction, road, rule, aspect,
/// position)` — unordered.
fn signal_heads(app: &mut App) -> Vec<(u16, u16, Option<VehicleRule>, SignalAspect, Vec3)> {
    app.world_mut()
        .query::<(&TrafficSignal, &Transform)>()
        .iter(app.world())
        .map(|(s, t)| (s.junction, s.road, s.rule, s.aspect, t.translation))
        .collect()
}

/// An authored nonzero light origin spawns one session-owned
/// indicator at the authored position on the approach's arc — and
/// session teardown removes it with everything else (F10-B.7).
#[test]
fn authored_signals_spawn_at_their_origins_and_despawn_on_teardown() {
    let r0_light = [2.0, 5.5, -2.0];
    let r1_light = [-2.0, 5.5, 2.0];
    let install = signal_install((1, Some(r0_light)), (1, Some(r1_light)));
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));

    let (signals, dropped) = {
        let t = app.world().resource::<AmbientTraffic>();
        (t.signals, t.signals_dropped)
    };
    assert_eq!((signals, dropped), (2, 0), "each authored head spawns");
    let heads = signal_heads(&mut app);
    assert_eq!(heads.len(), 2);
    let mut by_road: std::collections::HashMap<u16, (SignalAspect, Vec3)> =
        heads.iter().map(|(_, r, _, a, p)| (*r, (*a, *p))).collect();
    assert_eq!(by_road.len(), 2);
    for (ix, _, rule, _, _) in &heads {
        assert_eq!(*ix, 0, "the heads face the only junction");
        assert_eq!(*rule, Some(VehicleRule::TrafficLight));
    }
    assert_eq!(
        by_road.remove(&0).map(|(_, p)| p),
        Some(Vec3::from(r0_light)),
        "road 0's head stands at its authored origin"
    );
    assert_eq!(
        by_road.remove(&1).map(|(_, p)| p),
        Some(Vec3::from(r1_light)),
        "road 1's head stands at its authored origin"
    );

    // Teardown is the ordinary session sweep — the indicators are
    // session-owned like the cars.
    app.world_mut().resource_mut::<SessionControl>().quit = true;
    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Menu)),
        "quit never reached Menu"
    );
    assert!(
        signal_heads(&mut app).is_empty(),
        "signal indicators leaked past teardown"
    );
}

/// `drive_signals` keeps every head's aspect on the authoritative
/// phase: over several cycles each lit member sees green and red, no
/// tick greens two members at once, and the all-red clearance reds
/// both — the same admission `gate` enforces on the cars (F10-AC02's
/// signal leg, presentation).
#[test]
fn signal_heads_track_the_junction_phase() {
    let install = signal_install((1, Some([2.0, 5.5, -2.0])), (1, Some([-2.0, 5.5, 2.0])));
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    // Shorten the authored-independent phase so a few updates sweep
    // whole cycles — the timings are designed values either way.
    {
        let mut t = app.world_mut().resource_mut::<AmbientTraffic>();
        t.junctions.policy.green_ticks = 4;
        t.junctions.policy.clear_ticks = 2;
    }

    let mut green_seen = [false, false];
    let mut red_seen = [false, false];
    let mut saw_all_red = false;
    for _ in 0..30 {
        app.update();
        let heads = signal_heads(&mut app);
        assert_eq!(heads.len(), 2);
        let greens = heads.iter().filter(|h| h.3 == SignalAspect::Green).count();
        assert!(greens <= 1, "two members green at once: {heads:?}");
        for (ix, road, _, aspect, _) in &heads {
            assert_eq!(*ix, 0);
            match aspect {
                SignalAspect::Green => green_seen[*road as usize] = true,
                SignalAspect::Red => red_seen[*road as usize] = true,
                SignalAspect::Stop => panic!("a light member showed the stop aspect"),
            }
        }
        if greens == 0 {
            saw_all_red = true;
        }
    }
    assert_eq!(green_seen, [true, true], "each member greens");
    assert_eq!(red_seen, [true, true], "each member reds");
    assert!(saw_all_red, "the clearance slice never showed");
}

/// An authored origin implausibly far from its junction is junk
/// data, not a lamp — the sanity bound drops it and the counter
/// reports the loss rather than hiding it.
#[test]
fn a_wild_signal_origin_drops_and_stays_counted() {
    let install = signal_install((1, Some([500.0, 5.5, 500.0])), (3, None));
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    let (signals, dropped) = {
        let t = app.world().resource::<AmbientTraffic>();
        (t.signals, t.signals_dropped)
    };
    assert_eq!((signals, dropped), (0, 1));
    assert!(signal_heads(&mut app).is_empty());
}
