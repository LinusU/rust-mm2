//! F10-A.2 ambient-traffic runtime tests: a synthetic city install
//! (PSDL + BAI + aimap + `va_*` assets) drives the real
//! `load_session_world` → `ambient_setup` → `plan_ambient` → spawn path
//! — cars materialise as session-owned kinematic bodies on authored
//! lanes, `drive_ambient` walks them along the network in the authored
//! direction, dead ends despawn and the recycler respawns to the
//! density target, an event `[Density] 0.0` authors the population
//! off, and teardown removes everything.

use std::path::Path;
use std::time::{Duration, Instant};

use avian3d::prelude::*;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::cablecar::CableCar;
use mm2_app::camera::CameraMode;
use mm2_app::contracts::{self, ImpactFilter};
use mm2_app::session::{self, SelectedCar, SessionControl, SpawnPoint, TunedVehicle};
use mm2_app::traffic::{AmbientCar, AmbientDrive, AmbientTraffic, RoadObstacle, TrafficSignal};
use mm2_app::worldtraffic::CAR_CABLE;
use mm2_assets::Vfs;
use mm2_formats::bai::{Side, VehicleRule};
use mm2_game::cablecar::{CableMotion, CableRoute};
use mm2_game::{
    DamageEvent, DamageSpec, DevOverrides, EventRef, EventTableKind, ImpactEvent, LaneCursor,
    LaneId, LaneKind, Mm2Vfs, ObjectIdentity, Player, PlayerControl, PlayerId, PlayerVehicle,
    Session, SessionAuthority, SessionConfig, SessionMode, SessionPhase, SignalAspect, SpawnPolicy,
    SpawnPose, StuckWindow, VehicleDamage, WorldMode, advance_session_tick,
    despawn_session_entities,
};
use mm2_net::{Impair, Message};
use mm2_vehicle::{VehicleConfig, VehiclePlugin};

use crate::support::{
    ambient_assets, bai_bytes, bai_with_lane_offsets, bai_with_lights, bai_with_rules, city_aimap,
    city_install, synthetic_psdl,
};

// ---------------------------------------------------------------------------
// Synthetic install fixtures
// ---------------------------------------------------------------------------

fn write(dir: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
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
        .insert_resource(SpawnPoint::new(Vec3::new(0.0, 1.5, 0.0), 0.0))
        .insert_resource(CameraMode::Chase)
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .add_message::<DamageEvent>()
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
                // Same consumer ordering main.rs runs: the deduped
                // impact stream feeds damage before the handover
                // reads the same solver edges.
                mm2_app::damage::apply_impact_damage,
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
            // The production spawn shape: authored mass, the event
            // flag the handover and the transfer math read, and the
            // solver speed bounds the flipped wreck runs under
            // (F10-B.13).
            Mass(1200.0),
            CollisionEventsEnabled,
            MaxLinearSpeed(mm2_game::MAX_BANGER_LINEAR_SPEED),
            MaxAngularSpeed(mm2_game::MAX_BANGER_ANGULAR_SPEED),
            Position(pos),
            Rotation(rot),
            LinearVelocity::ZERO,
            AngularVelocity::ZERO,
            Transform::from_translation(pos).with_rotation(rot),
        ))
        .id()
}

/// `spawn_follower` with a caller-chosen hull width and mass — the
/// multi-edge drain tests need a follower wide enough to reach two
/// obstacles in one step, and cars light enough that one mutual
/// orientation falls under the impulse floor.
fn spawn_shaped_follower(
    app: &mut App,
    lane: LaneId,
    along: f32,
    target_speed: f32,
    width: f32,
    mass: f32,
) -> Entity {
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
            Collider::cuboid(width, 0.9, 3.2),
            Mass(mass),
            CollisionEventsEnabled,
            MaxLinearSpeed(mm2_game::MAX_BANGER_LINEAR_SPEED),
            MaxAngularSpeed(mm2_game::MAX_BANGER_ANGULAR_SPEED),
            Position(pos),
            Rotation(rot),
            LinearVelocity::ZERO,
            AngularVelocity::ZERO,
            Transform::from_translation(pos).with_rotation(rot),
        ))
        .id()
}

/// Park a knocked wreck at `pos` — `drive_ambient` never re-poses it,
/// it counts as a blocker for the corridor and landing checks, and
/// being bound for nothing it would occupy a junction zone it sat
/// inside. Kinematic so it stays where the test puts it.
fn spawn_wreck(app: &mut App, pos: Vec3) -> Entity {
    app.world_mut()
        .spawn((
            AmbientCar {
                class: 0,
                drive: AmbientDrive::Knocked,
                cursor: LaneCursor::new(lane(0, Side::Right), 0.0),
                target_speed: 0.0,
                speed: 0.0,
                stuck: StuckWindow::new(pos.to_array()),
            },
            RigidBody::Kinematic,
            Collider::cuboid(1.8, 0.9, 3.2),
            Position(pos),
            Rotation(Quat::IDENTITY),
            LinearVelocity::ZERO,
            AngularVelocity::ZERO,
            Transform::from_translation(pos),
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

/// A one-event install: the same synthetic city with an authored
/// checkpoint-race row, so a session can carry `SessionMode::Event`.
fn event_install() -> (tempfile::TempDir, SessionConfig) {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/testcity.psdl", synthetic_psdl());
    write(d, "city/testcity.bai", bai_bytes());
    write(d, "city/testcity.aimap", city_aimap());
    ambient_assets(d, "va_test_a");
    ambient_assets(d, "va_test_b");
    write(
        d,
        "race/testcity/mmracedata.csv",
        "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty\nnone,0,0,0,0,0,0.5,0.0,1,50,1,0,0,0,0,0,0.5,0.0,1,40,1\n",
    );
    // A non-zero density: Local would field traffic here, so the
    // networked legs prove the policy, not an authored zero.
    write(d, "race/testcity/race0.aimap", "[Density]\n0.5\n");
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
    (tmp, config)
}

fn replica_present(app: &App) -> bool {
    app.world()
        .get_resource::<mm2_app::worldtraffic::TrafficReplica>()
        .is_some()
}

/// F26-A policy (an enhanced one — MP-4 only documents "no ambient
/// traffic ... in MP races"): a networked *Cruise* fields the host's
/// traffic and a client only copies it; a networked *race* fields none
/// on either side. The `Host` leg runs the ordinary seeded population;
/// the `Remote` leg simulates no car, no signal and no second world —
/// it holds the replica the host's frames land on.
#[test]
fn networked_cruise_traffic_is_the_hosts_and_a_clients_is_a_replica() {
    let install = city_install();
    for authority in [SessionAuthority::Host, SessionAuthority::Remote] {
        let mut config = city_config();
        config.authority = authority;
        // Dev overrides are not network-legal (the lobby refuses to
        // advertise them) — the networked legs run a clean config.
        config.dev = DevOverrides::default();
        let mut app = test_app(config, vfs_of(install.path()));
        assert!(
            run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Playing)),
            "{authority:?}: city session never reached Playing"
        );
        let signals = app
            .world_mut()
            .query_filtered::<Entity, With<TrafficSignal>>()
            .iter(app.world())
            .count();
        match authority {
            SessionAuthority::Host => {
                assert!(
                    app.world().get_resource::<AmbientTraffic>().is_some(),
                    "Host: the authority simulates the population"
                );
                assert!(!replica_present(&app), "Host: no replica");
                assert!(!ambient_cars(&mut app).is_empty(), "Host: seeded cars");
            }
            _ => {
                assert!(
                    app.world().get_resource::<AmbientTraffic>().is_none(),
                    "Remote: no simulated population"
                );
                assert!(replica_present(&app), "Remote: the replica");
                assert!(
                    ambient_cars(&mut app).is_empty(),
                    "Remote: no lane follower"
                );
                assert_eq!(signals, 0, "Remote: no signal heads");
            }
        }
        // The local participant still loads and drives.
        let mut q = app
            .world_mut()
            .query_filtered::<&Player, With<PlayerVehicle>>();
        q.single(app.world()).expect("the local car");
    }

    // A networked race fields no ambient traffic on either side (MP-4).
    let (race, race_config) = event_install();
    for authority in [SessionAuthority::Host, SessionAuthority::Remote] {
        let mut config = race_config.clone();
        config.authority = authority;
        let mut app = test_app(config, vfs_of(race.path()));
        assert!(
            run_until(&mut app, 30, |a| {
                matches!(
                    a.world().resource::<Session>().phase(),
                    SessionPhase::Playing | SessionPhase::Countdown
                )
            }),
            "{authority:?}: event session never left Loading ({:?})",
            app.world().resource::<Session>().phase()
        );
        assert!(
            app.world().get_resource::<AmbientTraffic>().is_none(),
            "{authority:?}: MP-4 — no ambient-traffic resource in a networked race"
        );
        assert!(!replica_present(&app), "{authority:?}: no replica either");
        assert!(ambient_cars(&mut app).is_empty());
    }

    // Positive controls: the same installs under `Local` still field
    // traffic — the policy changed nothing single-player, race or roam.
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    assert!(
        !ambient_cars(&mut app).is_empty(),
        "Local: the seeded plan still spawns"
    );
    assert!(!replica_present(&app), "Local: no replica");
    let mut app = test_app(race_config, vfs_of(race.path()));
    assert!(run_until(&mut app, 30, |a| {
        matches!(
            a.world().resource::<Session>().phase(),
            SessionPhase::Playing | SessionPhase::Countdown
        )
    }));
    assert!(
        app.world()
            .get_resource::<AmbientTraffic>()
            .is_some_and(|t| t.target > 0),
        "Local race: the authored density stands"
    );
}

/// A `Remote` client app with the replication consumer wired the way
/// the application wires it.
fn client_app(install: &Path) -> App {
    let mut config = city_config();
    config.authority = SessionAuthority::Remote;
    config.dev = DevOverrides::default();
    let mut app = test_app(config, vfs_of(install));
    app.init_resource::<mm2_app::netdrive::RemoteSnaps>();
    app.add_systems(Update, mm2_app::worldtraffic::apply_traffic);
    app
}

/// What the host's `publish_traffic` would put on the wire this frame:
/// the production row collector over the host app's live cars, taken
/// through the real frame codec.
fn host_frame(
    app: &mut App,
    ledger: &mut mm2_app::worldtraffic::TrafficLedger,
    roster: Option<u64>,
) -> Message {
    let generation = app.world().resource::<Session>().generation();
    let wire = app.world().resource::<Session>().wire_generation();
    let tick = app.world().resource::<Session>().tick();
    let digest =
        mm2_app::worldtraffic::roster_digest(app.world().resource::<AmbientTraffic>().roster());
    let mut q = app.world_mut().query::<(
        Entity,
        &AmbientCar,
        &Position,
        &Rotation,
        &LinearVelocity,
        &mm2_game::SessionEntity,
    )>();
    let mut cq = app.world_mut().query::<(
        Entity,
        &CableCar,
        &Position,
        &Rotation,
        &LinearVelocity,
        &mm2_game::SessionEntity,
    )>();
    // The production order (`publish_traffic`): cable cars first.
    let (rows, omitted) = ledger.collect(
        generation,
        cq.iter(app.world())
            .filter(|(.., owner)| owner.0 == generation)
            .map(|(e, c, p, r, v, _)| (e, c.circuit, CAR_CABLE, p.0, r.0, v.0))
            .chain(
                q.iter(app.world())
                    .filter(|(.., owner)| owner.0 == generation)
                    .map(|(e, c, p, r, v, _)| {
                        (
                            e,
                            c.class,
                            mm2_app::worldtraffic::drive_state(c.drive),
                            p.0,
                            r.0,
                            v.0,
                        )
                    }),
            ),
    );
    assert_eq!(omitted, 0);
    let frame = Message::Traffic {
        generation: wire,
        tick,
        roster: roster.unwrap_or(digest),
        rows,
    };
    Message::decode(&frame.encode().expect("a bounded frame")).expect("the codec round-trips")
}

fn deliver(client: &mut App, frame: Message) {
    let Message::Traffic {
        generation,
        tick,
        roster,
        rows,
    } = frame
    else {
        panic!("not a traffic frame");
    };
    client
        .world_mut()
        .resource_mut::<mm2_app::netdrive::RemoteSnaps>()
        .push_traffic(generation, tick, roster, rows);
}

fn copies(app: &mut App) -> Vec<(u32, u16, Vec3, Entity)> {
    let mut v: Vec<_> = app
        .world_mut()
        .query::<(Entity, &mm2_app::worldtraffic::TrafficCopy, &Position)>()
        .iter(app.world())
        .map(|(e, c, p)| (c.id, c.class, p.0, e))
        .collect();
    v.sort_by_key(|(id, ..)| *id);
    v
}

/// F26-A end to end through the production code on both sides: a
/// host's seeded population, published through the real row collector
/// and frame codec, appears on a `Remote` client as one kinematic copy
/// per car — same class, same pose — follows the host's cars as they
/// drive, refuses a host whose roster differs, drops another session's
/// rows, and retires copies once frames stop. The client never owns a
/// lane follower. Same-process, wire codec only: the socket and
/// real-process legs live in `network.rs`.
#[test]
fn a_client_copies_the_hosts_traffic_and_retires_it_when_frames_stop() {
    let install = city_install();
    let mut config = city_config();
    config.authority = SessionAuthority::Host;
    config.dev = DevOverrides::default();
    let mut host = test_app(config, vfs_of(install.path()));
    let mut client = client_app(install.path());
    assert!(run_until(&mut host, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    assert!(run_until(&mut client, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    assert_eq!(
        host.world().resource::<Session>().wire_generation(),
        client.world().resource::<Session>().wire_generation(),
        "both fixtures run the same session generation"
    );
    let host_cars = ambient_cars(&mut host);
    assert!(host_cars.len() >= 2, "the seeded plan fields several cars");

    // Before any frame the client has no traffic of its own.
    assert!(copies(&mut client).is_empty());

    let mut ledger = mm2_app::worldtraffic::TrafficLedger::default();
    let frame = host_frame(&mut host, &mut ledger, None);
    let Message::Traffic { rows: sent, .. } = &frame else {
        unreachable!()
    };
    assert_eq!(
        sent.len(),
        host_cars.len(),
        "every live car rides the frame"
    );
    let sent = sent.clone();
    deliver(&mut client, frame);
    run(&mut client, 2);
    let made = copies(&mut client);
    assert_eq!(made.len(), sent.len(), "one copy per host car");
    for (copy, row) in made.iter().zip(&sent) {
        assert_eq!((copy.0, copy.1), (row.id, row.class));
        assert!(
            copy.2.distance(Vec3::from_array(row.pos)) < 1.0,
            "the copy sits where the host's car is"
        );
    }
    // A copy is a posed kinematic body with the class's real model
    // underneath — never a lane follower.
    let world = client.world_mut();
    for (.., entity) in &made {
        assert_eq!(world.get::<RigidBody>(*entity), Some(&RigidBody::Kinematic));
        assert!(world.get::<AmbientCar>(*entity).is_none());
        assert!(
            world
                .get::<Children>(*entity)
                .is_some_and(|c| !c.is_empty()),
            "the class's model is attached"
        );
    }
    assert!(
        ambient_cars(&mut client).is_empty(),
        "no lane follower on a client"
    );
    {
        let stage = client
            .world()
            .resource::<mm2_app::netdrive::RemoteSnaps>()
            .traffic();
        assert_eq!(stage.landed(), sent.len() as u64);
        assert_eq!((stage.mismatched(), stage.unresolved()), (0, 0));
    }

    // The host's cars drive on; the copies follow, as the same entities.
    run(&mut host, 60);
    let moved = host_frame(&mut host, &mut ledger, None);
    let Message::Traffic { rows: after, .. } = &moved else {
        unreachable!()
    };
    let after = after.clone();
    let kept: Vec<&_> = after
        .iter()
        .filter(|r| sent.iter().any(|s| s.id == r.id))
        .collect();
    assert!(!kept.is_empty(), "some of the first cars still drive");
    assert!(
        kept.iter().any(|r| {
            let before = sent.iter().find(|s| s.id == r.id).unwrap();
            Vec3::from_array(r.pos).distance(Vec3::from_array(before.pos)) > 1.0
        }),
        "the host's cars moved"
    );
    deliver(&mut client, moved);
    run(&mut client, 2);
    let followed = copies(&mut client);
    for row in &kept {
        let before = made.iter().find(|m| m.0 == row.id).unwrap();
        let now = followed
            .iter()
            .find(|m| m.0 == row.id)
            .expect("still alive");
        assert_eq!(now.3, before.3, "the same copy entity follows the car");
        // Within the velocity carry of two updates (~15 m/s × 33 ms).
        assert!(
            now.2.distance(Vec3::from_array(row.pos)) < 1.0,
            "copy {:?} vs row {:?} (before {:?})",
            now.2,
            row.pos,
            before.2
        );
    }

    // A host whose roster differs poses nothing and spawns nothing new.
    let alive = copies(&mut client).len();
    let foreign = host_frame(&mut host, &mut ledger, Some(0xdead_beef));
    deliver(&mut client, foreign);
    run(&mut client, 2);
    assert_eq!(copies(&mut client).len(), alive);
    assert!(
        client
            .world()
            .resource::<mm2_app::netdrive::RemoteSnaps>()
            .traffic()
            .mismatched()
            >= 1,
        "the refusal is counted"
    );

    // Frames stop: the copies retire after the TTL (fixed ticks), not
    // before.
    run(&mut client, 20);
    assert!(
        !copies(&mut client).is_empty(),
        "one lost frame does not erase the population"
    );
    let ticks_needed = mm2_app::worldtraffic::COPY_TTL_TICKS as usize + 20;
    run(&mut client, ticks_needed);
    assert!(
        copies(&mut client).is_empty(),
        "silent copies retire; a recycled car does not linger"
    );
    assert_eq!(
        client
            .world()
            .resource::<mm2_app::worldtraffic::TrafficReplica>()
            .live(),
        0
    );
}

/// F26-AC01/AC03 (traffic, v19) under impairment: the host's real
/// population, collected through the production row collector and the
/// frame codec, crosses a real loopback socket and a seeded
/// `ImpairProxy` in every cell of the shared matrix, and lands on a
/// `Remote` client through the production `apply_traffic`. During the
/// storm the client only ever holds copies of cars the host fielded
/// (never more than the host's ids), and once the link is clean the
/// next frame brings every host car to its copy, pose and class —
/// within a deadline, whatever loss ate: a frame is the whole state.
/// The recorded rows are `eprintln!`ed.
///
/// Evidence level: synthetic integration (a synthetic city and `va_*`
/// classes, real sockets) — the host's lobby/relevancy wiring is the
/// `net_app` legs' business; this cell feeds the frames the way
/// `publish_traffic` does and receives them off the wire.
#[test]
fn the_traffic_copies_converge_through_each_impairment_cell() {
    use std::collections::BTreeSet;
    use std::sync::mpsc;

    use mm2_net::{Client, Host, HostConfig, ImpairProxy, LinkDir, hello};

    let install = city_install();
    for (index, (name, impair)) in crate::support::impair_cells().into_iter().enumerate() {
        let mut config = city_config();
        config.authority = SessionAuthority::Host;
        config.dev = DevOverrides::default();
        let mut host_sim = test_app(config, vfs_of(install.path()));
        let mut client_sim = client_app(install.path());
        assert!(run_until(&mut host_sim, 12, |a| phase_is(
            a,
            SessionPhase::Playing
        )));
        assert!(run_until(&mut client_sim, 12, |a| phase_is(
            a,
            SessionPhase::Playing
        )));

        // The wire: a bare lobby host, the client dialled through the
        // proxy, its frames forwarded to this thread.
        let host = Host::listen_loopback(&HostConfig::new(1)).unwrap();
        let proxy = ImpairProxy::loopback_seeded(host.addr(), 300 + index as u64).unwrap();
        let mut peer = Client::join(
            proxy.addr(),
            &hello("net-impair-test".into(), name.into(), 1),
        )
        .expect("join through the proxy failed");
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            while let Ok(msg) = peer.recv() {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });

        let mut ledger = mm2_app::worldtraffic::TrafficLedger::default();
        let mut fielded: BTreeSet<u32> = BTreeSet::new();
        let send = |host: &Host, frame: &Message| host.ctl().broadcast(frame).unwrap();
        let drain = |client: &mut App, wait: Duration| {
            let mut newest = None;
            while let Ok(msg) = rx.recv_timeout(wait) {
                if let Message::Traffic { tick, .. } = &msg {
                    newest = newest.max(Some(*tick));
                    deliver(client, msg);
                    run(client, 1);
                }
            }
            newest
        };

        proxy.set(LinkDir::Down, impair);
        for _ in 0..40 {
            // Enough host updates that successive frames carry
            // different session ticks.
            run(&mut host_sim, 6);
            let frame = host_frame(&mut host_sim, &mut ledger, None);
            if let Message::Traffic { rows, .. } = &frame {
                fielded.extend(rows.iter().map(|r| r.id));
            }
            send(&host, &frame);
            drain(&mut client_sim, Duration::from_millis(4));
            let held: BTreeSet<u32> = copies(&mut client_sim).iter().map(|c| c.0).collect();
            assert!(
                held.is_subset(&fielded),
                "cell {name}: a copy of a car the host never fielded: {held:?} vs {fielded:?}"
            );
        }
        let down = proxy.stats(LinkDir::Down);
        assert_eq!(
            down.overflowed, 0,
            "cell {name} overflowed a lane: {down:?}"
        );
        if impair.delay > Duration::ZERO || impair.jitter > Duration::ZERO {
            assert!(down.delayed > 0, "cell {name} delayed nothing: {down:?}");
        }
        if impair.loss >= 0.10 {
            assert!(down.dropped > 0, "cell {name} dropped nothing: {down:?}");
        }
        if impair.duplicate > 0.0 {
            assert!(
                down.duplicated > 0,
                "cell {name} duplicated nothing: {down:?}"
            );
        }
        if impair.reorder > 0.0 {
            assert!(
                down.reordered > 0,
                "cell {name} reordered nothing: {down:?}"
            );
        }

        // The storm over: one more frame is the whole state, and the
        // client must reach it. Frames still held in the lane flush
        // first; the final one is recognised by its tick.
        proxy.set(LinkDir::Down, Impair::default());
        run(&mut host_sim, 6);
        let final_frame = host_frame(&mut host_sim, &mut ledger, None);
        let Message::Traffic {
            tick: final_tick,
            rows: final_rows,
            ..
        } = final_frame.clone()
        else {
            unreachable!()
        };
        send(&host, &final_frame);
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let seen = drain(&mut client_sim, Duration::from_millis(20));
            if seen.is_some_and(|t| t >= final_tick) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "cell {name}: the final traffic frame never arrived (want tick {final_tick}, saw {seen:?}, {:?})",
                proxy.stats(LinkDir::Down)
            );
        }
        let made = copies(&mut client_sim);
        assert!(!final_rows.is_empty(), "cell {name}: the host fields cars");
        for row in &final_rows {
            let copy = made
                .iter()
                .find(|c| c.0 == row.id)
                .unwrap_or_else(|| panic!("cell {name}: no copy of host car {}", row.id));
            assert_eq!(copy.1, row.class, "cell {name}: car {} class", row.id);
            // A copy keeps its row's velocity while the client's own
            // updates run, so it sits a step or two past the pose; a
            // stale or misplaced copy would be a lane or more away.
            assert!(
                copy.2.distance(Vec3::from_array(row.pos)) < 2.5,
                "cell {name}: copy {:?} is not where the host's car is {:?}",
                copy.2,
                row.pos
            );
        }
        let stage = client_sim
            .world()
            .resource::<mm2_app::netdrive::RemoteSnaps>()
            .traffic();
        eprintln!(
            "traffic cell={name} cars={} copies={} landed={} stale={} unresolved={} mismatched={} down={down:?}",
            final_rows.len(),
            made.len(),
            stage.landed(),
            stage.stale(),
            stage.unresolved(),
            stage.mismatched(),
        );
        assert_eq!(stage.mismatched(), 0, "cell {name}: rosters disagreed");
        assert_eq!(stage.unresolved(), 0, "cell {name}: a row named no car");

        host.ctl().shutdown().unwrap();
        drop(proxy);
        let _ = reader.join();
    }
}

/// Put one host cable car on a straight 100 m line, moving at cruise
/// speed, owned by the host's current generation.
fn spawn_host_cable_car(host: &mut App) -> Entity {
    let generation = host.world().resource::<Session>().generation();
    let route = CableRoute::new(&[vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(100.0, 0.0, 0.0)]])
        .expect("a straight line is a route");
    host.world_mut()
        .spawn((
            CableCar {
                circuit: 0,
                motion: CableMotion::new(&route, 0, 0.5),
                lift: 0.0,
                nose: 4.0,
                tail: 4.0,
                half_width: 1.25,
            },
            RigidBody::Kinematic,
            Position(Vec3::new(10.0, 1.0, 40.0)),
            Rotation(Quat::from_rotation_y(0.5)),
            LinearVelocity(Vec3::new(15.0, 0.0, 0.0)),
            mm2_game::SessionEntity(generation),
        ))
        .id()
}

fn cable_copies(app: &mut App) -> Vec<(u32, Vec3, Entity)> {
    let mut v: Vec<_> = app
        .world_mut()
        .query::<(Entity, &mm2_app::worldtraffic::TrafficCopy, &Position)>()
        .iter(app.world())
        .filter(|(_, c, _)| c.state == CAR_CABLE)
        .map(|(e, c, p)| (c.id, p.0, e))
        .collect();
    v.sort_by_key(|(id, ..)| *id);
    v
}

/// F28-B.5 end to end through the production code on both sides: a
/// host's cable car, published through the real row collector and frame
/// codec, appears on a `Remote` client as one kinematic copy with the
/// retail model's collider and render parts — never a `CableCar` — and
/// follows the host's car as it moves. A client that joins later finds
/// the car wherever the host has it (AC05's late-join leg).
#[test]
fn a_client_copies_the_hosts_cable_car_and_a_late_joiner_finds_it_in_place() {
    let install = city_install();
    ambient_assets(install.path(), "va_cablecar_f");
    let mut config = city_config();
    config.authority = SessionAuthority::Host;
    config.dev = DevOverrides::default();
    let mut host = test_app(config, vfs_of(install.path()));
    let mut client = client_app(install.path());
    assert!(run_until(&mut host, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    assert!(run_until(&mut client, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    let car = spawn_host_cable_car(&mut host);

    let mut ledger = mm2_app::worldtraffic::TrafficLedger::default();
    let frame = host_frame(&mut host, &mut ledger, None);
    let Message::Traffic { rows, .. } = &frame else {
        unreachable!()
    };
    let cable_row = *rows
        .iter()
        .find(|r| r.state == CAR_CABLE)
        .expect("the cable car rides the frame");
    assert_eq!(cable_row.id, 0, "the cable cars take the lowest ids");
    deliver(&mut client, frame);
    run(&mut client, 2);

    let made = cable_copies(&mut client);
    assert_eq!(made.len(), 1, "one copy for the one host car");
    let (id, pos, copy) = made[0];
    assert_eq!(id, cable_row.id);
    assert!(pos.distance(Vec3::new(10.0, 1.0, 40.0)) < 0.5, "{pos:?}");
    let world = client.world_mut();
    assert_eq!(world.get::<RigidBody>(copy), Some(&RigidBody::Kinematic));
    assert!(
        world.get::<CableCar>(copy).is_none(),
        "a client never drives a cable car"
    );
    assert!(world.get::<Collider>(copy).is_some(), "the car collides");
    assert!(
        world.get::<Children>(copy).is_some_and(|c| !c.is_empty()),
        "the model is attached"
    );
    assert!(world.get::<mm2_app::traffic::RoadObstacle>(copy).is_none());

    // The host's car moves on; the same copy follows it.
    let moved_to = Vec3::new(40.0, 1.0, 40.0);
    host.world_mut().get_mut::<Position>(car).unwrap().0 = moved_to;
    host.world_mut()
        .get_mut::<Transform>(car)
        .unwrap()
        .translation = moved_to;
    let frame = host_frame(&mut host, &mut ledger, None);
    run(&mut host, 1);
    deliver(&mut client, frame);
    run(&mut client, 2);
    let followed = cable_copies(&mut client);
    assert_eq!(followed.len(), 1);
    assert_eq!(followed[0].2, copy, "the same entity follows the car");
    assert!(followed[0].1.distance(Vec3::new(40.0, 1.0, 40.0)) < 1.0);

    // A late joiner: a fresh client sees the car where the host has it
    // now, from one frame and no history.
    let mut late = client_app(install.path());
    assert!(run_until(&mut late, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    assert!(cable_copies(&mut late).is_empty());
    let now = host.world().get::<Position>(car).unwrap().0;
    assert!(now.x >= 40.0, "the host's car has driven on: {now:?}");
    let frame = host_frame(&mut host, &mut ledger, None);
    deliver(&mut late, frame);
    run(&mut late, 2);
    let joined = cable_copies(&mut late);
    assert_eq!(joined.len(), 1);
    assert!(
        joined[0].1.distance(now) < 1.0,
        "joined at {:?}, host at {now:?}",
        joined[0].1
    );

    // Frames stop (the host's car is gone): the copy retires like any
    // other.
    host.world_mut().despawn(car);
    run(
        &mut client,
        mm2_app::worldtraffic::COPY_TTL_TICKS as usize + 20,
    );
    assert!(cable_copies(&mut client).is_empty());
}

/// An install that cannot supply the cable car's model: the client
/// counts the rows unresolved and spawns nothing — no stand-in box —
/// and the ambient copies beside it are unaffected.
#[test]
fn a_client_without_the_cable_model_counts_the_rows_and_spawns_nothing() {
    let install = city_install();
    let mut config = city_config();
    config.authority = SessionAuthority::Host;
    config.dev = DevOverrides::default();
    let mut host = test_app(config, vfs_of(install.path()));
    let mut client = client_app(install.path());
    assert!(run_until(&mut host, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    assert!(run_until(&mut client, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    let ambient = ambient_cars(&mut host).len();
    assert!(ambient >= 2);
    spawn_host_cable_car(&mut host);
    let mut ledger = mm2_app::worldtraffic::TrafficLedger::default();
    let frame = host_frame(&mut host, &mut ledger, None);
    deliver(&mut client, frame);
    run(&mut client, 2);
    assert!(cable_copies(&mut client).is_empty());
    assert_eq!(copies(&mut client).len(), ambient, "the traffic landed");
    let stage = client
        .world()
        .resource::<mm2_app::netdrive::RemoteSnaps>()
        .traffic();
    assert_eq!(stage.unresolved(), 1, "the cable row is counted");
    assert_eq!(stage.landed(), ambient as u64);
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

/// F11-AC05 (aimap half): an event's traffic exceptions live in the
/// session's `AmbientTraffic` and die with it. Unloading the event
/// back to the menu removes them, and a following Cruise session on
/// the same city loads the city's own aimap again — roster, density
/// and an empty exception list — from the untouched install.
#[test]
fn unloading_an_event_removes_its_aimap_exceptions_but_not_the_citys() {
    let (tmp, event_config) = event_install();
    let d = tmp.path();
    // Close road 1 and slow road 0 for this event only.
    write(
        d,
        "race/testcity/race0.aimap",
        "[Exceptions]\n2\n1\t0.00\t0\n0\t0.50\t8\n",
    );
    let mut app = test_app(event_config, vfs_of(d));
    assert!(
        run_until(&mut app, 30, |a| {
            matches!(
                a.world().resource::<Session>().phase(),
                SessionPhase::Playing | SessionPhase::Countdown
            )
        }),
        "event session never left Loading"
    );
    {
        let traffic = app.world().resource::<AmbientTraffic>();
        let roads: Vec<u32> = traffic
            .overrides()
            .exceptions
            .iter()
            .map(|e| e.road)
            .collect();
        assert_eq!(roads, vec![1, 0], "the event's exceptions are applied");
        assert!(traffic.overrides().is_closed(1));
    }

    app.world_mut().resource_mut::<SessionControl>().quit = true;
    assert!(
        run_until(&mut app, 30, |a| phase_is(a, SessionPhase::Menu)),
        "quit never reached Menu"
    );
    assert!(
        app.world().get_resource::<AmbientTraffic>().is_none(),
        "the event's exceptions leaked past unload"
    );

    // The same city, now as Cruise: the city's own aimap is intact.
    // The quit intent is level-triggered, so release it first or it
    // tears the new session straight back down.
    app.world_mut().resource_mut::<SessionControl>().quit = false;
    app.world_mut()
        .resource_mut::<Session>()
        .begin(SessionConfig {
            world: WorldMode::City {
                psdl: "city/testcity.psdl".into(),
            },
            dev: DevOverrides {
                spawn: Some(SpawnPose {
                    position: Vec3::new(0.0, 1.5, 200.0),
                    yaw: 0.0,
                }),
                ..DevOverrides::default()
            },
            ..SessionConfig::default()
        })
        .unwrap();
    let reached = run_until(&mut app, 300, |a| phase_is(a, SessionPhase::Playing));
    assert!(
        reached,
        "the follow-up Cruise never reached Playing: {:?}",
        app.world().resource::<Session>().phase()
    );
    let traffic = app.world().resource::<AmbientTraffic>();
    assert!(
        traffic.overrides().exceptions.is_empty(),
        "the event's exceptions carried into the next session"
    );
    assert!(!traffic.overrides().is_closed(1));
    assert_eq!(traffic.roster().entries.len(), 2, "the city roster loads");
    assert_eq!(traffic.target, 8, "the city's own [Density] 0.25 applies");
    assert!(!ambient_cars(&mut app).is_empty());
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

/// Run one follower at 15 m/s down a lane toward a stationary cable-car
/// body (`RoadObstacle`) 20 m along it, facing `facing` (+1 along the
/// lane, −1 head-on), for `obstacle` true; without the component the
/// body is invisible to the sense. Returns the closest the follower's
/// centre came to the body's centre and its final speed.
fn follower_toward_cable_car(facing: f32, obstacle: bool) -> (f32, f32) {
    const HALF_LENGTH: f32 = 4.36;
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
    let (lane, at, tangent) = {
        let traffic = app.world().resource::<AmbientTraffic>();
        let lane = traffic
            .graph()
            .lanes()
            .iter()
            .find(|l| l.arc.is_some() && l.length > 22.0)
            .map(|l| l.id)
            .expect("the fixture authors routable lanes");
        let s = traffic.graph().sample_lane(lane, 20.0).unwrap();
        (lane, Vec3::from(s.position), Vec3::from(s.tangent))
    };
    let rot = mm2_game::movers::mover_rotation(tangent * facing);
    let mut body = app.world_mut().spawn((
        Position(at),
        Rotation(rot),
        Transform::from_translation(at).with_rotation(rot),
    ));
    if obstacle {
        body.insert(RoadObstacle {
            nose: HALF_LENGTH,
            tail: HALF_LENGTH,
        });
    }
    let follower = spawn_follower(&mut app, lane, 2.0, 15.0);
    let mut closest = f32::MAX;
    for _ in 0..360 {
        app.update();
        if let Some((fp, _, _)) = car_state(&mut app, follower) {
            closest = closest.min(fp.distance(at));
        }
    }
    let speed = car_state(&mut app, follower).map_or(f32::NAN, |(_, v, _)| v);
    (closest, speed)
}

/// The sensing points follow the body's own extents: a long nose and a
/// short tail put the end points at different distances, each two
/// metres inside its end, and a body shorter than the inset is only
/// sensed at its origin.
#[test]
fn an_obstacles_sense_points_follow_its_nose_and_tail() {
    let at = Vec3::new(10.0, 1.0, -5.0);
    let rot = mm2_game::movers::mover_rotation(Vec3::X);
    let obstacle = RoadObstacle {
        nose: 6.0,
        tail: 3.0,
    };
    let along = |p: Vec3| (p - at).dot(Vec3::X);
    let pts: Vec<f32> = obstacle.sense_points(at, rot).map(along).collect();
    assert_eq!(pts.len(), 3);
    assert!(pts[0].abs() < 1e-4);
    assert!((pts[1] - 4.0).abs() < 1e-4, "nose point {}", pts[1]);
    assert!((pts[2] + 1.0).abs() < 1e-4, "tail point {}", pts[2]);
    let stub = RoadObstacle {
        nose: 1.0,
        tail: 1.0,
    };
    assert!(stub.sense_points(at, rot).all(|p| p.distance(at) < 1e-4));
}

/// Ambient cars queue behind a cable car as behind any car: held short
/// of its tail when it points away, and short of its nose head-on,
/// whose edge is 4.36 m from the origin — never inside the body.
#[test]
fn ambient_cars_queue_behind_a_cable_car_on_the_lane() {
    for facing in [1.0, -1.0] {
        let (closest, speed) = follower_toward_cable_car(facing, true);
        assert!(
            closest >= 4.36 + 2.0,
            "facing {facing}: the follower's centre reached {closest} m of the tram's centre"
        );
        assert!(speed <= 1.0, "facing {facing}: never held ({speed} m/s)");
    }
}

/// The same body without the sense component is not seen: the follower
/// drives into it, so the test above is the sense's doing.
#[test]
fn an_unmarked_body_on_the_lane_is_not_sensed() {
    let (closest, _) = follower_toward_cable_car(1.0, false);
    assert!(
        closest < 3.0,
        "the follower stopped {closest} m short anyway"
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

/// `junction_install` with two driving lanes per side — the
/// same-tick-arrival test needs parallel lanes of one member road so
/// two cars can be rolling through an open gate at once.
fn two_lane_install(r0_end: u16, r1_start: u16) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/test.psdl", synthetic_psdl());
    write(
        d,
        "city/test.bai",
        bai_with_lane_offsets(&[2.5, 6.0], (r0_end, None), (r1_start, None)),
    );
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

/// Vehicle lane `index` of a road side (inner→outer).
fn lane_at(road: u16, side: Side, index: u16) -> LaneId {
    LaneId {
        road,
        side,
        index,
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

/// F10-B.11 app-side: two cars on parallel lanes of the same member
/// road roll through the same signal green at identical speed —
/// they reach the lane end in the same drive tick. The
/// blocker/bound-for snapshots are frame-start, so without the
/// same-tick entry record the second car reads an empty box and both
/// commit to the interior at once. The box serialises them: at most
/// one committed crossing is ever active, and the held car still
/// takes the junction once the box clears.
#[test]
fn a_same_tick_second_arrival_waits_for_the_committed_crossing() {
    let install = two_lane_install(1, 1); // TrafficLight on both roads
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));

    // Both on road 0's parallel right lanes — the same signal member,
    // so a single green admits both at once. Same `along` and target
    // speed give identical cursor motion: whenever they reach the
    // lane end (rolling through a green, or leaving the stop line
    // together when a held red clears) they commit the same tick.
    let lanes = [lane_at(0, Side::Right, 0), lane_at(0, Side::Right, 1)];
    let cars = [
        spawn_follower(&mut app, lanes[0], 20.0, 15.0),
        spawn_follower(&mut app, lanes[1], 20.0, 15.0),
    ];

    let mut crossed = [false; 2];
    let mut shared = 0usize;
    for _ in 0..3600 {
        app.update();
        let mut inside = 0;
        for (i, (car, ln)) in cars.iter().zip(lanes).enumerate() {
            let Some((_, _, cur)) = car_state(&mut app, *car) else {
                continue;
            };
            if cur.crossing.is_some() {
                inside += 1;
            }
            // "Took the junction" = committed a crossing or landed on
            // the exit lane — the lane id only changes on landing.
            crossed[i] |= cur.crossing.is_some() || cur.lane != ln;
        }
        shared = shared.max(inside);
        if crossed == [true, true] {
            break;
        }
    }
    assert_eq!(
        crossed,
        [true, true],
        "both eligible cars must take the junction — no deadlock"
    );
    assert_eq!(shared, 1, "two cars shared the junction interior at once");
}

/// F10-B.11 review regression — a *mixed-rule* junction, the common
/// retail case: a `NeverStop` car's gate never consults the same-tick
/// record, so it can enter and roll back (a permanently blocked
/// landing) over a gated car's live claim in one drive tick. The
/// rollback must shed only its own claim — a junction-keyed release
/// strips the gated car's and re-opens the box for a third car
/// evaluated later that tick.
///
/// Choreography: A and C sit on parallel lanes of the signal member
/// road (identical speed profiles → same-tick lane end), B is re-armed
/// at its `NeverStop` lane end before every update so it enters and
/// rolls back on whichever drive tick A commits. One fixed step per
/// update makes that coverage exact, and spawn order fixes the
/// in-tick evaluation order A → B → C. B's landing lanes are blocked
/// by wrecks parked above the zone's vertical band — inside the
/// landing clearance sphere, outside the occupancy zone, so the box
/// itself reads empty to the gated approaches.
#[test]
fn a_rolled_back_free_flow_entry_keeps_the_committed_claim() {
    // TrafficLight on road 0's approach, NeverStop on road 1's.
    let install = two_lane_install(1, 3);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    // One 120 Hz drive tick per update: the per-update re-arm below
    // then covers every drive tick, not every other one.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 120.0,
    )));
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

    let lanes = [lane_at(0, Side::Right, 0), lane_at(0, Side::Right, 1)];
    let b_lane = lane_at(1, Side::Left, 1);
    let a = spawn_follower(&mut app, lanes[0], 20.0, 15.0);
    // B on the far left lane: standing at its line it sits >8 m off
    // A's and C's road-1 landings, so it never shadows them.
    let b = spawn_follower(&mut app, b_lane, 20.0, 15.0);
    let c = spawn_follower(&mut app, lanes[1], 20.0, 15.0);
    // Road 0's left lanes are where B's transfer lands (along=0 sits
    // at z=−4): a wreck above each landing point blocks the transfer
    // forever without occupying the box.
    spawn_wreck(&mut app, Vec3::new(-2.5, 5.5, -4.0));
    spawn_wreck(&mut app, Vec3::new(-6.0, 5.5, -4.0));

    let lane_end = {
        let t = app.world().resource::<AmbientTraffic>();
        t.graph().lane(b_lane).expect("a live lane").length
    };
    let mut crossed = [false; 2];
    let mut shared = 0usize;
    for _ in 0..3600 {
        // Re-arm B a hair short of its lane end at entry speed — it
        // enters and rolls back on this update's drive tick, so its
        // rollback always lands inside A's commit tick.
        if let Some(mut car) = app.world_mut().get_mut::<AmbientCar>(b) {
            car.cursor = LaneCursor::new(b_lane, lane_end - 0.01);
            car.speed = 8.0;
        }
        app.update();
        let mut inside = 0;
        for (i, (car, ln)) in [(a, lanes[0]), (c, lanes[1])].iter().enumerate() {
            let Some((_, _, cur)) = car_state(&mut app, *car) else {
                continue;
            };
            inside += usize::from(cur.crossing.is_some());
            crossed[i] |= cur.crossing.is_some() || cur.lane != *ln;
        }
        shared = shared.max(inside);
        if crossed == [true, true] {
            break;
        }
    }
    assert_eq!(
        crossed,
        [true, true],
        "both eligible cars must take the junction — no deadlock"
    );
    assert_eq!(
        shared, 1,
        "a rolled-back free-flow entry stripped the committed car's claim"
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
    let (spot, travel) = {
        let t = app.world().resource::<AmbientTraffic>();
        let s = t
            .graph()
            .sample_lane(lane_r0, 14.0)
            .expect("a live lane samples");
        (
            Vec3::from(s.position) + Vec3::Y * 0.55,
            Vec3::from(s.tangent).normalize_or(Vec3::NEG_Z),
        )
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
        assert!(
            lv.0.is_finite() && lv.0.length() <= 35.0,
            "unbounded kick velocity: {lv:?}"
        );
        (c.drive, *rb, lv.0, c.cursor.clone())
    };
    let _ = (drive, body);

    // F10-B.12: the handover is a momentum-conserving transfer, not a
    // wall response plus a free kick. The 1200 kg follower at 15 m/s
    // carries all the momentum in; splitting the exchange at the
    // inelastic common velocity (~`m·v/(m_s+m_w)` = 7.2 m/s) slows the
    // wreck to its share while the striker correction rewrites the
    // ~15 m/s the kinematic shove gave the block up to the same share
    // — one `(1+e)·v·μ` impulse charged exactly once. The app runs two
    // fixed steps per update,
    // so the read sees one solver step after the write: ground
    // pushout and contact friction scrub a little more off the fresh
    // dynamic wreck. Assert the physical ranges — the exact share is
    // proved by `a_light_striker_shares_the_exchange_not_its_speed`.
    let striker_v = app
        .world()
        .get::<LinearVelocity>(striker)
        .expect("the striker despawned")
        .0
        .dot(travel);
    let wreck_v = kick.dot(travel);
    assert!(
        wreck_v > 1.0 && wreck_v < 10.0,
        "the wreck kept neither its speed nor its share: {wreck_v}"
    );
    assert!(
        striker_v > 1.0 && striker_v < 11.0,
        "the striker kept the wall response instead of its share: {striker_v}"
    );
    let momentum = 1200.0 * wreck_v + 1300.0 * striker_v;
    assert!(
        momentum <= 1200.0 * 15.0 * 1.05 && momentum > 0.0,
        "the exchange injected momentum: {momentum}"
    );

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
    // The handover fires once: 60 more ticks of the wreck resting
    // against or re-touching the striker add no further flips.
    assert_eq!(
        app.world().resource::<AmbientTraffic>().knocked,
        1,
        "a repeated contact edge re-knocked the wreck"
    );

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

/// A light striker cannot yeet a heavy parked car at its own speed:
/// the transfer splits the exchange — a 400 kg block sliding into a
/// parked 1200 kg follower hands over only its momentum share, so the
/// wreck leaves at the inelastic common velocity (~`m_s·v/(m_s+m_w)`)
/// and the striker keeps the same speed instead of stopping dead
/// against an infinite-mass wall plus a free approach-speed kick on
/// the car (F10-B.12; F10-AC03's "no extreme energy injection" leg).
#[test]
fn a_light_striker_shares_the_exchange_not_its_speed() {
    let install = junction_install(0, 0);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));

    let lane_r0 = lane(0, Side::Right);
    // Parked follower mid-lane — the striker does the moving.
    let car = spawn_follower(&mut app, lane_r0, 15.0, 0.0);
    let (spot, travel) = {
        let t = app.world().resource::<AmbientTraffic>();
        let s = t
            .graph()
            .sample_lane(lane_r0, 10.0)
            .expect("a live lane samples");
        (
            Vec3::from(s.position) + Vec3::Y * 0.55,
            Vec3::from(s.tangent).normalize_or(Vec3::NEG_Z),
        )
    };
    let striker = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.8, 1.1, 1.8),
            Mass(400.0),
            CollisionEventsEnabled,
            Position(spot),
            LinearVelocity(travel * 25.0),
            AngularVelocity::ZERO,
            Transform::from_translation(spot),
        ))
        .id();

    // The striker's speed the update before the flip is the momentum
    // baseline — friction bleeds a little on the slide in.
    let mut pre_speed = 0.0f32;
    let mut knocked = false;
    for _ in 0..240 {
        app.update();
        if app.world().resource::<AmbientTraffic>().knocked >= 1 {
            knocked = true;
            break;
        }
        pre_speed = app
            .world()
            .get::<LinearVelocity>(striker)
            .map(|v| v.0.dot(travel))
            .unwrap_or(0.0);
    }
    assert!(knocked, "the sliding striker never knocked the parked car");
    assert!(
        pre_speed > 15.0,
        "the striker never got up to speed: {pre_speed}"
    );

    let wreck_v = app
        .world()
        .get::<LinearVelocity>(car)
        .expect("the wreck despawned")
        .0
        .dot(travel);
    let striker_v = app
        .world()
        .get::<LinearVelocity>(striker)
        .expect("the striker despawned")
        .0
        .dot(travel);
    let common = 400.0 * pre_speed / (400.0 + 1200.0);
    // The wreck launches at its transfer share — under the old flat
    // kick it would have left at ~`pre_speed`.
    assert!(
        (wreck_v - common).abs() < 2.5 && wreck_v < 0.6 * pre_speed,
        "the wreck took the striker's speed instead of its share: {wreck_v} (pre {pre_speed})"
    );
    // And the striker keeps its share rather than bouncing off a wall.
    assert!(
        (striker_v - common).abs() < 2.5,
        "the striker stopped against a wall response: {striker_v} (common {common})"
    );
    let momentum = 1200.0 * wreck_v + 400.0 * striker_v;
    assert!(
        (momentum - 400.0 * pre_speed).abs() / (400.0 * pre_speed) < 0.25,
        "the exchange did not conserve momentum: {momentum}"
    );
    // F10-B.13: a plain dynamic body striker — no `Player`, no
    // `AmbientCar` — counts in the `x` class.
    let traffic = app.world().resource::<AmbientTraffic>();
    assert_eq!(traffic.knocked, 1);
    assert_eq!(traffic.knocked_by_other, 1);
    assert_eq!(traffic.knocked_by_participant, 0);
    assert_eq!(traffic.knocked_by_ambient, 0);
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

/// F10-AC03's damage leg end to end: the session's real *player*
/// vehicle — `Player` + `ObjectIdentity` + a dynamic collider body —
/// striking a parked lane follower flips it like any striker, the
/// same solver edge feeds the deduplicated impact stream, and the
/// striker's `VehicleDamage` accrues `severity × other_mass`. The
/// handover also counts the striker's class: `kns=` `p`, which `kn=`
/// alone could not name (F10-B.13).
#[test]
fn a_participant_striker_takes_damage_and_names_the_class() {
    let install = junction_install(0, 0);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));

    // The spawned player vehicle carries no `vehcardamage` in this
    // fixture (`SelectedCar::def` is None), so stamp authored-style
    // bounds — the same shape `load_session_world` inserts for a
    // stock car.
    let player = app
        .world_mut()
        .query_filtered::<Entity, With<PlayerVehicle>>()
        .single(app.world())
        .expect("one player vehicle");
    app.world_mut()
        .entity_mut(player)
        .insert(VehicleDamage::new(DamageSpec {
            impact_threshold: 1500.0,
            med_damage: 80_000.0,
            max_damage: 187_500.0,
            regenerate_rate: 0.0,
        }));

    let lane_r0 = lane(0, Side::Right);
    // A parked follower mid-lane — minted an identity like the
    // production spawn so the pair resolves as two named
    // participants rather than player-vs-world.
    let car = spawn_follower(&mut app, lane_r0, 15.0, 0.0);
    let car_id = app.world_mut().resource_mut::<Session>().mint_object_id();
    app.world_mut()
        .entity_mut(car)
        .insert(ObjectIdentity(car_id));

    // Slide the player in down-lane at ~20 m/s — the same strike the
    // synthetic block strikers deliver.
    let (spot, travel) = {
        let t = app.world().resource::<AmbientTraffic>();
        let s = t
            .graph()
            .sample_lane(lane_r0, 9.0)
            .expect("a live lane samples");
        (
            Vec3::from(s.position) + Vec3::Y * 0.55,
            Vec3::from(s.tangent).normalize_or(Vec3::NEG_Z),
        )
    };
    teleport_player(&mut app, spot);
    app.world_mut()
        .get_mut::<LinearVelocity>(player)
        .expect("player velocity")
        .0 = travel * 20.0;

    assert!(
        run_until(&mut app, 90, |a| {
            a.world().resource::<AmbientTraffic>().knocked >= 1
        }),
        "the sliding player never knocked the parked car"
    );
    let traffic = app.world().resource::<AmbientTraffic>();
    assert_eq!(traffic.knocked, 1);
    assert_eq!(
        traffic.knocked_by_participant, 1,
        "the player striker did not count as a participant"
    );
    assert_eq!(traffic.knocked_by_ambient, 0);
    assert_eq!(traffic.knocked_by_other, 0);

    // The same edge fed the impact stream: the striker accrued
    // `severity × follower mass` against its authored bounds.
    assert!(
        app.world().resource::<ImpactFilter>().emitted >= 1,
        "the strike emitted no impact event"
    );
    assert!(
        app.world()
            .resource::<mm2_app::damage::DamageReport>()
            .applied
            >= 1,
        "the strike applied no damage"
    );
    let total = app
        .world()
        .get::<VehicleDamage>(player)
        .expect("player damage")
        .total();
    assert!(
        total > 1500.0,
        "the participant took no damage from the hit: {total}"
    );

    // The flipped wreck keeps the spawn-stamped solver bounds and
    // stays one dynamic body on the same entity.
    let wreck = app.world().entity(car);
    assert!(wreck.get::<MaxLinearSpeed>().is_some());
    assert!(wreck.get::<MaxAngularSpeed>().is_some());
    assert!(matches!(wreck.get::<RigidBody>(), Some(RigidBody::Dynamic)));
}

/// A knocked wreck sliding into a queued lane car flips it too — and
/// the striker classifies as ambient, not participant (the `kns=`
/// `a` leg): a pile-up and a player hit now read differently on the
/// record (F10-B.13).
#[test]
fn a_wreck_striker_counts_as_ambient() {
    let install = junction_install(0, 0);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));

    let lane_r0 = lane(0, Side::Right);
    let car = spawn_follower(&mut app, lane_r0, 15.0, 0.0);
    // A wreck already dynamic — the shape a handover leaves —
    // sliding down-lane into the parked follower.
    let (spot, rot, travel) = {
        let t = app.world().resource::<AmbientTraffic>();
        let s = t
            .graph()
            .sample_lane(lane_r0, 9.0)
            .expect("a live lane samples");
        let tangent = Vec3::from(s.tangent).normalize_or(Vec3::NEG_Z);
        let yaw = (-tangent.x).atan2(-tangent.z);
        let pitch = tangent.y.clamp(-1.0, 1.0).asin();
        (
            Vec3::from(s.position) + Vec3::Y * 0.55,
            Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0),
            tangent,
        )
    };
    app.world_mut().spawn((
        AmbientCar {
            class: 0,
            drive: AmbientDrive::Knocked,
            cursor: LaneCursor::new(lane_r0, 9.0),
            target_speed: 0.0,
            speed: 0.0,
            stuck: StuckWindow::new(spot.to_array()),
        },
        RigidBody::Dynamic,
        Collider::cuboid(1.8, 0.9, 3.2),
        Mass(1200.0),
        CollisionEventsEnabled,
        // The production wreck's solver bounds — a flipped lane car
        // keeps the `MaxLinearSpeed`/`MaxAngularSpeed` its spawn
        // stamped (F10-B.13).
        MaxLinearSpeed(mm2_game::MAX_BANGER_LINEAR_SPEED),
        MaxAngularSpeed(mm2_game::MAX_BANGER_ANGULAR_SPEED),
        Position(spot),
        Rotation(rot),
        LinearVelocity(travel * 15.0),
        AngularVelocity::ZERO,
        Transform::from_translation(spot).with_rotation(rot),
    ));

    assert!(
        run_until(&mut app, 90, |a| {
            a.world().resource::<AmbientTraffic>().knocked >= 1
        }),
        "the sliding wreck never knocked the parked car"
    );
    let traffic = app.world().resource::<AmbientTraffic>();
    assert_eq!(traffic.knocked, 1);
    assert_eq!(
        traffic.knocked_by_ambient, 1,
        "the wreck striker did not count as ambient"
    );
    assert_eq!(traffic.knocked_by_participant, 0);
    assert_eq!(traffic.knocked_by_other, 0);
    let (drive, body) = {
        let mut q = app.world_mut().query::<(Entity, &AmbientCar, &RigidBody)>();
        let (_, c, rb) = q
            .iter(app.world())
            .find(|(e, ..)| *e == car)
            .expect("the struck car despawned");
        (c.drive, *rb)
    };
    assert_eq!(drive, AmbientDrive::Knocked);
    assert!(matches!(body, RigidBody::Dynamic));
}

/// Two strikers reaching one lane car in the same tick produce one
/// handover, not two (F10-B.14's same-tick pileup edge): the decide
/// pass records both edges while the car is still `Lane`, the apply
/// pass's `Lane` re-check lets the first flip win, and the second
/// striker keeps its solver wall response — the wreck takes a single
/// transfer's launch and the record counts one knock with one
/// striker class.
#[test]
fn a_same_tick_pileup_flips_the_car_once() {
    let install = junction_install(0, 0);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));

    let lane_r0 = lane(0, Side::Right);
    // The driving follower does the approaching: two strikers rest
    // side by side across its path with their front faces at the same
    // lane-along, so the car's front reaches both in the same solver
    // step and both edges land in one `knock_ambient` drain.
    let car = spawn_follower(&mut app, lane_r0, 10.0, 15.0);
    let (spot, travel) = {
        let t = app.world().resource::<AmbientTraffic>();
        let s = t
            .graph()
            .sample_lane(lane_r0, 14.0)
            .expect("a live lane samples");
        (
            Vec3::from(s.position) + Vec3::Y * 0.55,
            Vec3::from(s.tangent).normalize_or(Vec3::NEG_Z),
        )
    };
    let lateral = Vec3::new(-travel.z, 0.0, travel.x);
    let mut strikers = Vec::new();
    for dx in [-0.7f32, 0.7] {
        let pos = spot + lateral * dx;
        strikers.push(
            app.world_mut()
                .spawn((
                    RigidBody::Dynamic,
                    Collider::cuboid(1.2, 1.1, 1.8),
                    Mass(1300.0),
                    CollisionEventsEnabled,
                    Position(pos),
                    Transform::from_translation(pos),
                ))
                .id(),
        );
    }

    // Prove the staging really is same-tick: both pairs' contact
    // begins on the same update, and the flip lands in that update's
    // drain.
    let mut first_touch = [usize::MAX; 2];
    let mut knocked_at = usize::MAX;
    for update in 0..240 {
        app.update();
        for (i, striker) in strikers.iter().enumerate() {
            if first_touch[i] == usize::MAX {
                let striker = *striker;
                let touching = app
                    .world_mut()
                    .run_system_once(move |c: Collisions| c.contains(striker, car))
                    .expect("the collisions query runs");
                if touching {
                    first_touch[i] = update;
                }
            }
        }
        if app.world().resource::<AmbientTraffic>().knocked >= 1 {
            knocked_at = update;
            break;
        }
    }
    assert_eq!(
        first_touch[0], first_touch[1],
        "the strikers did not contact on the same tick: {first_touch:?}"
    );
    assert_ne!(
        knocked_at,
        usize::MAX,
        "the same-tick pileup never knocked the car"
    );
    // The contact pair marks touching a fixed step before the
    // `CollisionStart` edge drains, so the flip can lag the shared
    // touch update by one — both edges lag together into one drain.
    assert!(
        knocked_at >= first_touch[0] && knocked_at <= first_touch[0] + 1,
        "the flip did not land in the drain the edges arrived in: \
         touch {first_touch:?} knock {knocked_at}"
    );

    let traffic = app.world().resource::<AmbientTraffic>();
    assert_eq!(
        traffic.knocked, 1,
        "a second same-tick edge re-knocked the wreck"
    );
    // Exactly one striker class is charged — both blocks are `x`.
    assert_eq!(traffic.knocked_by_other, 1);
    assert_eq!(traffic.knocked_by_participant, 0);
    assert_eq!(traffic.knocked_by_ambient, 0);

    // One flip: the wreck is the same entity, dynamic, carrying a
    // single transfer's launch — a double handover would roughly
    // double it (the `a_hard_hit` band).
    let (drive, body, kick) = {
        let mut q = app
            .world_mut()
            .query::<(Entity, &AmbientCar, &RigidBody, &LinearVelocity)>();
        let (_, c, rb, lv) = q
            .iter(app.world())
            .find(|(e, ..)| *e == car)
            .expect("the knocked car despawned");
        (c.drive, *rb, lv.0)
    };
    assert_eq!(drive, AmbientDrive::Knocked);
    assert!(matches!(body, RigidBody::Dynamic));
    let wreck_v = kick.dot(travel);
    assert!(
        wreck_v > 1.0 && wreck_v < 11.0,
        "the wreck took more than one transfer's launch: {wreck_v}"
    );

    // Exactly one striker was corrected: the winning edge rewrote its
    // striker to the exchange share while the losing edge — dropped
    // at the apply pass's `Lane` re-check — keeps the faster
    // kinematic wall shove the solver gave it. Two corrected
    // strikers would share the low band; two uncorrected would share
    // the high one.
    let speeds: Vec<f32> = strikers
        .iter()
        .map(|striker| {
            app.world()
                .get::<LinearVelocity>(*striker)
                .expect("a striker despawned")
                .0
                .dot(travel)
        })
        .collect();
    let (min, max) = (
        speeds.iter().copied().fold(f32::INFINITY, f32::min),
        speeds.iter().copied().fold(f32::NEG_INFINITY, f32::max),
    );
    for (i, v) in speeds.iter().enumerate() {
        assert!(v.is_finite(), "striker {i} diverged: {speeds:?}");
    }
    assert!(
        min > 1.0 && min < 11.0,
        "no striker holds the corrected exchange share: {speeds:?}"
    );
    assert!(
        max - min > 4.0,
        "both strikers took the same response — no winner/loser split: {speeds:?}"
    );

    // Re-contacts against the resting wreck add no further flips.
    run(&mut app, 60);
    assert_eq!(
        app.world().resource::<AmbientTraffic>().knocked,
        1,
        "a repeated contact edge re-knocked the wreck"
    );
    assert!(
        app.world()
            .get::<Position>(car)
            .expect("the wreck despawned")
            .0
            .is_finite(),
        "the solver diverged on the pileup"
    );
}

/// Three strikers reaching one lane car in the same tick still
/// produce one handover — B.14's two-striker edge extended to the
/// disclosed 3+ pileup (F10-B.15): the first committed edge flips the
/// car and corrects its striker, the other two drop at the apply
/// pass's `Lane` re-check and their strikers keep the solver's wall
/// shove. One knock, one striker class, one transfer's launch.
#[test]
fn a_same_tick_three_striker_pileup_flips_the_car_once() {
    let install = junction_install(0, 0);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));

    let lane_r0 = lane(0, Side::Right);
    // The driving follower does the approaching: three strikers rest
    // shoulder to shoulder across its path with their front faces at
    // the same lane-along, so all three pairs begin touching on the
    // same solver step and drain into one `knock_ambient` call.
    let car = spawn_follower(&mut app, lane_r0, 10.0, 15.0);
    let (spot, travel) = {
        let t = app.world().resource::<AmbientTraffic>();
        let s = t
            .graph()
            .sample_lane(lane_r0, 14.0)
            .expect("a live lane samples");
        (
            Vec3::from(s.position) + Vec3::Y * 0.55,
            Vec3::from(s.tangent).normalize_or(Vec3::NEG_Z),
        )
    };
    let lateral = Vec3::new(-travel.z, 0.0, travel.x);
    let mut strikers = Vec::new();
    for dx in [-1.0f32, 0.0, 1.0] {
        let pos = spot + lateral * dx;
        strikers.push(
            app.world_mut()
                .spawn((
                    RigidBody::Dynamic,
                    Collider::cuboid(1.2, 1.1, 1.8),
                    Mass(1300.0),
                    CollisionEventsEnabled,
                    Position(pos),
                    Transform::from_translation(pos),
                ))
                .id(),
        );
    }

    // Prove the staging really is same-tick: all three pairs' contact
    // begins on the same update, and the flip lands in that update's
    // drain.
    let mut first_touch = [usize::MAX; 3];
    let mut knocked_at = usize::MAX;
    for update in 0..240 {
        app.update();
        for (i, striker) in strikers.iter().enumerate() {
            if first_touch[i] == usize::MAX {
                let striker = *striker;
                let touching = app
                    .world_mut()
                    .run_system_once(move |c: Collisions| c.contains(striker, car))
                    .expect("the collisions query runs");
                if touching {
                    first_touch[i] = update;
                }
            }
        }
        if app.world().resource::<AmbientTraffic>().knocked >= 1 {
            knocked_at = update;
            break;
        }
    }
    assert_eq!(
        first_touch, [first_touch[0]; 3],
        "the strikers did not contact on the same tick: {first_touch:?}"
    );
    assert_ne!(
        knocked_at,
        usize::MAX,
        "the same-tick pileup never knocked the car"
    );
    assert!(
        knocked_at >= first_touch[0] && knocked_at <= first_touch[0] + 1,
        "the flip did not land in the drain the edges arrived in: \
         touch {first_touch:?} knock {knocked_at}"
    );

    let traffic = app.world().resource::<AmbientTraffic>();
    assert_eq!(
        traffic.knocked, 1,
        "a second same-tick edge re-knocked the wreck"
    );
    assert_eq!(traffic.knocked_by_other, 1);
    assert_eq!(traffic.knocked_by_participant, 0);
    assert_eq!(traffic.knocked_by_ambient, 0);

    // One flip: the wreck carries a single transfer's launch, and
    // exactly one striker was corrected — the two losing edges kept
    // the faster kinematic wall shove.
    let (drive, body, kick) = {
        let mut q = app
            .world_mut()
            .query::<(Entity, &AmbientCar, &RigidBody, &LinearVelocity)>();
        let (_, c, rb, lv) = q
            .iter(app.world())
            .find(|(e, ..)| *e == car)
            .expect("the knocked car despawned");
        (c.drive, *rb, lv.0)
    };
    assert_eq!(drive, AmbientDrive::Knocked);
    assert!(matches!(body, RigidBody::Dynamic));
    let wreck_v = kick.dot(travel);
    assert!(
        wreck_v > 1.0 && wreck_v < 11.0,
        "the wreck took more than one transfer's launch: {wreck_v}"
    );

    let mut speeds: Vec<f32> = strikers
        .iter()
        .map(|striker| {
            app.world()
                .get::<LinearVelocity>(*striker)
                .expect("a striker despawned")
                .0
                .dot(travel)
        })
        .collect();
    speeds.sort_by(|a, b| a.total_cmp(b));
    for (i, v) in speeds.iter().enumerate() {
        assert!(v.is_finite(), "striker {i} diverged: {speeds:?}");
    }
    // Sorted: one corrected striker in the exchange-share band, two
    // uncorrected on the faster wall shove.
    assert!(
        speeds[0] > 1.0 && speeds[0] < 11.0,
        "no striker holds the corrected exchange share: {speeds:?}"
    );
    assert!(
        speeds[1] - speeds[0] > 4.0 && speeds[2] - speeds[0] > 4.0,
        "the losing strikers did not keep the wall shove: {speeds:?}"
    );
}

/// One striker reaching two lane cars in the same drain pays both
/// transfers (F10-B.15): the first committed edge's correction is the
/// wall-returning velocity target and the second a pure impulse
/// debit — a second target write along the shared direction would
/// erase the first edge's payment while both struck cars launch,
/// injecting the missing transfer outright.
#[test]
fn one_striker_pays_both_cars_it_flips() {
    let install = two_lane_install(0, 0);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));

    // A parked follower on each of road 0's parallel lanes, rear
    // faces at the same z; a wide dynamic block sliding down the gap
    // between the lanes reaches both rears in one solver step.
    let lanes = [lane_at(0, Side::Right, 0), lane_at(0, Side::Right, 1)];
    let cars = [
        spawn_follower(&mut app, lanes[0], 14.0, 0.0),
        spawn_follower(&mut app, lanes[1], 14.0, 0.0),
    ];
    let (spot, travel) = {
        let t = app.world().resource::<AmbientTraffic>();
        let a = t
            .graph()
            .sample_lane(lanes[0], 11.5)
            .expect("a live lane samples");
        let b = t
            .graph()
            .sample_lane(lanes[1], 11.5)
            .expect("a live lane samples");
        (
            (Vec3::from(a.position) + Vec3::from(b.position)) * 0.5 + Vec3::Y * 0.55,
            Vec3::from(a.tangent).normalize_or(Vec3::NEG_Z),
        )
    };
    let striker = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(4.0, 1.1, 1.8),
            Mass(2600.0),
            CollisionEventsEnabled,
            Position(spot),
            LinearVelocity(travel * 20.0),
            Transform::from_translation(spot),
        ))
        .id();

    // Prove the staging really is same-tick: both pairs' contact
    // begins on the same update, and the flips land in its drain.
    let mut first_touch = [usize::MAX; 2];
    let mut knocked_at = usize::MAX;
    for update in 0..240 {
        app.update();
        for (i, car) in cars.iter().enumerate() {
            if first_touch[i] == usize::MAX {
                let car = *car;
                let touching = app
                    .world_mut()
                    .run_system_once(move |c: Collisions| c.contains(striker, car))
                    .expect("the collisions query runs");
                if touching {
                    first_touch[i] = update;
                }
            }
        }
        if app.world().resource::<AmbientTraffic>().knocked >= 2 {
            knocked_at = update;
            break;
        }
    }
    assert_eq!(
        first_touch[0], first_touch[1],
        "the cars were not contacted on the same tick: {first_touch:?}"
    );
    assert_ne!(
        knocked_at,
        usize::MAX,
        "the sliding striker never knocked both cars"
    );
    assert!(
        knocked_at >= first_touch[0] && knocked_at <= first_touch[0] + 1,
        "the flips did not land in the drain the edges arrived in: \
         touch {first_touch:?} knock {knocked_at}"
    );

    let traffic = app.world().resource::<AmbientTraffic>();
    assert_eq!(traffic.knocked, 2);
    assert_eq!(
        traffic.knocked_by_other, 2,
        "each struck car charges the striker's class"
    );
    assert_eq!(traffic.knocked_by_participant, 0);
    assert_eq!(traffic.knocked_by_ambient, 0);

    // Both cars flip and launch; the striker pays both transfers —
    // its first edge rewrote it to the exchange share (~13 m/s for
    // these masses) and the second debited it again (~7 m/s). A
    // per-edge target write would leave it at the share band having
    // paid only once while two cars fly.
    let striker_v = app
        .world()
        .get::<LinearVelocity>(striker)
        .expect("the striker despawned")
        .0
        .dot(travel);
    assert!(
        striker_v > 1.0 && striker_v < 11.0,
        "the striker paid only one transfer for two launches: {striker_v}"
    );
    for (i, car) in cars.iter().enumerate() {
        let (drive, body, kick) = {
            let mut q = app
                .world_mut()
                .query::<(Entity, &AmbientCar, &RigidBody, &LinearVelocity)>();
            let (_, c, rb, lv) = q
                .iter(app.world())
                .find(|(e, ..)| *e == *car)
                .expect("a struck car despawned");
            (c.drive, *rb, lv.0)
        };
        assert_eq!(
            drive,
            AmbientDrive::Knocked,
            "car {i} stayed lane-following"
        );
        assert!(
            matches!(body, RigidBody::Dynamic),
            "car {i} stayed kinematic"
        );
        let wreck_v = kick.dot(travel);
        assert!(
            wreck_v > 6.0,
            "car {i} was counted knocked but never launched: {wreck_v}"
        );
    }

    run(&mut app, 60);
    for car in cars {
        assert!(
            app.world()
                .get::<Position>(car)
                .expect("a wreck despawned")
                .0
                .is_finite(),
            "the solver diverged on the two-car hit"
        );
    }
}

/// A same-tick daisy chain carries the debt through (F10-B.15): a
/// lane car this pass itself flips starts from its post-flip launch —
/// a kinematic–kinematic edge charged it no wall — so its own striker
/// edge owes the pure impulse debit rather than being skipped (the
/// third car would launch free) or charged a second wall return.
///
/// Staging: a wide follower `B` drives down road 0's inner lane; its
/// front face reaches a parked striker block `X` (the flip edge) and
/// the rear corner of a light parked follower `C` (the chain edge) on
/// the same step. `C`'s 200 kg mass puts the mutual orientation
/// `B←C` under the impulse floor, so `B`'s only flip edge is `X`'s —
/// the pair that flipped it can never alias the pair it strikes.
#[test]
fn a_same_tick_chain_charges_the_middle_car() {
    let install = two_lane_install(0, 0);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));

    let lane_inner = lane_at(0, Side::Right, 0);
    let lane_outer = lane_at(0, Side::Right, 1);
    // B is 6 m wide — its front face spans both lanes' fronts.
    let middle = spawn_shaped_follower(&mut app, lane_inner, 10.0, 15.0, 6.0, 600.0);
    // C parked on the outer lane, wide enough that B's front meets
    // its rear nearly flat.
    let last = spawn_shaped_follower(&mut app, lane_outer, 14.0, 0.0, 4.0, 200.0);
    // The block: its rear face coplanar with C's rear face so B's
    // front reaches both in one step.
    let (spot, travel) = {
        let t = app.world().resource::<AmbientTraffic>();
        let s = t
            .graph()
            .sample_lane(lane_inner, 13.3)
            .expect("a live lane samples");
        let tangent = Vec3::from(s.tangent);
        let lateral = Vec3::new(-tangent.z, 0.0, tangent.x);
        (
            Vec3::from(s.position) + lateral * -1.5 + Vec3::Y * 0.55,
            Vec3::from(s.tangent).normalize_or(Vec3::NEG_Z),
        )
    };
    let block = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.2, 1.1, 1.8),
            Mass(1300.0),
            CollisionEventsEnabled,
            Position(spot),
            Transform::from_translation(spot),
        ))
        .id();

    // Prove the staging really is same-tick: both pairs' contact
    // begins on the same update.
    let mut first_touch = [usize::MAX; 2];
    let mut knocked_at = usize::MAX;
    for update in 0..240 {
        app.update();
        for (i, other) in [block, last].iter().enumerate() {
            if first_touch[i] == usize::MAX {
                let other = *other;
                let touching = app
                    .world_mut()
                    .run_system_once(move |c: Collisions| c.contains(middle, other))
                    .expect("the collisions query runs");
                if touching {
                    first_touch[i] = update;
                }
            }
        }
        if app.world().resource::<AmbientTraffic>().knocked >= 2 {
            knocked_at = update;
            break;
        }
    }
    assert_eq!(
        first_touch[0], first_touch[1],
        "the chain edges did not contact on the same tick: {first_touch:?}"
    );
    assert_ne!(knocked_at, usize::MAX, "the chain never knocked both cars");
    assert!(
        knocked_at >= first_touch[0] && knocked_at <= first_touch[0] + 1,
        "the flips did not land in the drain the edges arrived in: \
         touch {first_touch:?} knock {knocked_at}"
    );

    let traffic = app.world().resource::<AmbientTraffic>();
    assert_eq!(traffic.knocked, 2);
    // B's flip charges the block (`x`); C's flip charges B (`a`).
    assert_eq!(traffic.knocked_by_other, 1);
    assert_eq!(traffic.knocked_by_ambient, 1);
    assert_eq!(traffic.knocked_by_participant, 0);

    let along = |e: Entity| {
        app.world()
            .get::<LinearVelocity>(e)
            .expect("a body despawned")
            .0
            .dot(travel)
    };
    // C launched — the corner contact's normal tilts, so the launch
    // splits lateral/forward; the speed reads the committed transfer.
    let last_v = app
        .world()
        .get::<LinearVelocity>(last)
        .expect("the last car despawned")
        .0
        .length();
    assert!(
        last_v > 4.0,
        "the chain's last car never launched: {last_v}"
    );
    // B shed its own launch off the block edge (≈5–6 m/s at these
    // masses and the sensed approach speed) and then paid C's debit —
    // ~2 m/s more along the push direction. Under the old skip it
    // would sit at the post-launch ~2.7 m/s while C flew free.
    let middle_v = along(middle);
    assert!(
        middle_v > -4.0 && middle_v < 2.0,
        "the middle car never paid the chain debit: {middle_v}"
    );
    // The block is corrected to its exchange share, not the wall.
    let block_v = along(block);
    assert!(
        block_v > 1.0 && block_v < 10.0,
        "the flip-edge striker kept the wall response: {block_v}"
    );
    for car in [middle, last] {
        let (drive, body) = {
            let mut q = app.world_mut().query::<(Entity, &AmbientCar, &RigidBody)>();
            let (_, c, rb) = q
                .iter(app.world())
                .find(|(e, ..)| *e == car)
                .expect("a chain car despawned");
            (c.drive, *rb)
        };
        assert_eq!(drive, AmbientDrive::Knocked);
        assert!(matches!(body, RigidBody::Dynamic));
    }
}

/// The mirror of the chain: when the striker is the same pair's other
/// side — two lane cars edge each other — the striker's own
/// struck-side launch already is its share of the exchange, so the
/// correction owes it nothing more. A follower clipping a parked
/// neighbour flips both, charges each a single `a`, and keeps the
/// split rather than being debited a second time.
#[test]
fn a_mutual_follower_edge_charges_the_exchange_once() {
    let install = two_lane_install(0, 0);
    let mut app = test_app(city_config(), vfs_of(install.path()));
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));

    let lane_inner = lane_at(0, Side::Right, 0);
    let lane_outer = lane_at(0, Side::Right, 1);
    // The moving car is wide enough that its front corner clips the
    // parked outer-lane follower's rear — a follower–follower edge at
    // full approach speed.
    let mover = spawn_shaped_follower(&mut app, lane_inner, 10.0, 15.0, 6.0, 1200.0);
    let parked = spawn_shaped_follower(&mut app, lane_outer, 14.0, 0.0, 4.0, 1200.0);
    let travel = {
        let t = app.world().resource::<AmbientTraffic>();
        let s = t
            .graph()
            .sample_lane(lane_inner, 10.0)
            .expect("a live lane samples");
        Vec3::from(s.tangent).normalize_or(Vec3::NEG_Z)
    };

    let mut first_touch = usize::MAX;
    let mut knocked_at = usize::MAX;
    for update in 0..240 {
        app.update();
        if first_touch == usize::MAX {
            let touching = app
                .world_mut()
                .run_system_once(move |c: Collisions| c.contains(mover, parked))
                .expect("the collisions query runs");
            if touching {
                first_touch = update;
            }
        }
        if app.world().resource::<AmbientTraffic>().knocked >= 2 {
            knocked_at = update;
            break;
        }
    }
    assert_ne!(first_touch, usize::MAX, "the pair never touched");
    assert!(
        knocked_at >= first_touch && knocked_at <= first_touch + 1,
        "the flips did not land in the drain the edge arrived in: \
         touch {first_touch} knock {knocked_at}"
    );

    // Both orientations of the same pair committed — two `a` charges.
    let traffic = app.world().resource::<AmbientTraffic>();
    assert_eq!(traffic.knocked, 2);
    assert_eq!(traffic.knocked_by_ambient, 2);
    assert_eq!(traffic.knocked_by_participant, 0);
    assert_eq!(traffic.knocked_by_other, 0);

    let along = |e: Entity| {
        app.world()
            .get::<LinearVelocity>(e)
            .expect("a body despawned")
            .0
            .dot(travel)
    };
    // The exchange splits: the mover sheds its struck-side launch and
    // keeps the remainder — it must not also be debited the striker
    // share, which would park it at ~0.
    let mover_v = along(mover);
    assert!(
        mover_v > 3.0,
        "the mover was charged twice for one exchange: {mover_v}"
    );
    let parked_v = along(parked);
    assert!(
        parked_v > 3.0,
        "the parked car never took its launch: {parked_v}"
    );
    for car in [mover, parked] {
        let (drive, body) = {
            let mut q = app.world_mut().query::<(Entity, &AmbientCar, &RigidBody)>();
            let (_, c, rb) = q
                .iter(app.world())
                .find(|(e, ..)| *e == car)
                .expect("a mutual-pair car despawned");
            (c.drive, *rb)
        };
        assert_eq!(drive, AmbientDrive::Knocked);
        assert!(matches!(body, RigidBody::Dynamic));
    }
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
