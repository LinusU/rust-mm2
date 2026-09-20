//! F11-B.2 event-session integration: an authored `CatalogEvent` driving
//! the real `load_session_world` → `advance_race` path — countdown,
//! checkpoint markers, swept progress and results — over the dev world,
//! with the event's records resolved through the production VFS.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::race::{self, CheckpointMarker};
use mm2_app::session::{self, SessionControl};
use mm2_app::{camera, contracts};
use mm2_assets::Vfs;
use mm2_game::{
    EventRef, EventTableKind, ImpactEvent, Mm2Vfs, ParticipantState, PlayerVehicle, RacePhase,
    RaceProgress, RaceStarted, RaceState, ResultLedger, Session, SessionConfig, SessionMode,
    SessionPhase, advance_session_tick, despawn_session_entities,
};
use mm2_vehicle::{VehicleConfig, VehicleInput, VehiclePlugin, VehicleState};

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";
const ROW: &str = "none,0,0,0,0,0,0.1,0.0,1,50,1,0,0,0,0,0,0.2,0.0,1,40,1";

/// The synthetic course: a straight +X lane on clear dev-world ground
/// (z=140 has no obstacles between x≈40 and the east wall).
const COURSE: &[f32] = &[60.0, 110.0, 140.0, 165.0, 180.0];
const COURSE_Z: f32 = 140.0;

fn write(dir: &Path, rel: &str, contents: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

fn waypoint_row(x: f32, z: f32) -> String {
    format!("{x},0,{z},0,15,0,0,0,\n")
}

/// A `race/testcity/` install: one checkpoint row plus records.
fn event_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/testcity/mmracedata.csv",
        &format!("{MM_HEADER}\n{ROW}\n"),
    );
    write(d, "race/testcity/race0.aimap", "#\n");
    write(
        d,
        "race/testcity/race0waypoints.csv",
        &format!(
            "{WAYPOINTS}{}{}{}{}{}",
            waypoint_row(COURSE[0], COURSE_Z), // start line
            waypoint_row(COURSE[1], COURSE_Z), // gates
            waypoint_row(COURSE[2], COURSE_Z),
            waypoint_row(COURSE[3], COURSE_Z),
            waypoint_row(COURSE[4], COURSE_Z), // finish
        ),
    );
    tmp
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

fn event_config() -> SessionConfig {
    SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "testcity".into(),
            table: EventTableKind::Checkpoint,
            index: 0,
        }),
        ..SessionConfig::default()
    }
}

/// The same system set `headless_smoke` runs — the real session driver,
/// race driver and marker updater on a minimal headless app.
fn event_app(config: SessionConfig, vfs: Vfs) -> App {
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
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .add_message::<RaceStarted>()
        .init_resource::<contracts::ImpactFilter>()
        .init_resource::<ResultLedger>()
        .init_resource::<SessionControl>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(camera::CameraMode::Chase)
        .insert_resource(session::SpawnPoint {
            position: Vec3::new(0.0, 1.5, 0.0),
            yaw: 0.0,
            trailers: Vec::new(),
        })
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(session::TunedVehicle(VehicleConfig::default()))
        .insert_resource(session::SelectedCar {
            def: None,
            paint: 0,
        })
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (
                contracts::collect_impacts,
                contracts::publish_vehicle_telemetry,
                race::reanchor_teleported_participants,
                race::advance_race,
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
                race::update_checkpoint_markers,
            ),
        );
    app.finish();
    app.cleanup();
    app
}

fn phase(app: &App) -> SessionPhase {
    app.world().resource::<Session>().phase().clone()
}

fn car(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<PlayerVehicle>>()
        .iter(app.world())
        .next()
        .expect("player vehicle spawned")
}

fn run(app: &mut App, updates: usize) {
    for _ in 0..updates {
        app.update();
    }
}

/// The authored event loads through `Loading → Ready → Countdown`:
/// the race resource, the participant progress and the marker
/// entities all land in the same update the world spawns.
#[test]
fn authored_event_loads_into_countdown() {
    let tmp = event_install();
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();

    assert_eq!(phase(&app), SessionPhase::Countdown);
    let race = app.world().resource::<RaceState>();
    assert_eq!(race.generation, 1, "race belongs to this session");
    assert!(race.input_locked(), "countdown still holds input");
    assert_eq!(race.definition.checkpoints.len(), 3);
    assert!(race.definition.finish.is_some());

    // The player spawned at the authored slot — 10 m behind the start
    // line (the designed fallback, no `_strtpnts` authored) — facing
    // the course, and it is a race participant.
    let car = car(&mut app);
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!(
        (pos.x - 50.0).abs() < 3.0 && (pos.z - COURSE_Z).abs() < 3.0,
        "player at the authored start slot, got {pos:?}"
    );
    let progress = app.world().get::<RaceProgress>(car).unwrap();
    assert_eq!(progress.state, ParticipantState::AwaitingStart);

    // One marker per gate plus the finish.
    let markers = app
        .world_mut()
        .query_filtered::<Entity, With<CheckpointMarker>>()
        .iter(app.world())
        .count();
    assert_eq!(markers, 4);
}

/// The countdown releases through `advance_race` exactly once, into
/// `Playing` — the authored event drives the shared lifecycle, not a
/// test-side stand-in.
#[test]
fn event_countdown_releases_into_playing() {
    let tmp = event_install();
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();

    let mut starts = 0;
    for _ in 0..240 {
        app.update();
        starts += app
            .world_mut()
            .resource_mut::<Messages<RaceStarted>>()
            .drain()
            .count();
    }
    assert_eq!(starts, 1, "one RaceStarted for the whole release");
    assert_eq!(phase(&app), SessionPhase::Playing);
    let race = app.world().resource::<RaceState>();
    assert_eq!(race.phase, RacePhase::Running);
    assert!(race.clock > 0);
}

/// Full production path: throttle through physics drives the car down
/// the authored lane — every gate is swept, the finish records exactly
/// one result and the race completes.
#[test]
fn driving_the_authored_course_finishes_the_race() {
    let tmp = event_install();
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    let car = car(&mut app);

    // Countdown runs ~180 updates; hold throttle after release and the
    // dev car covers the ~130 m lane well inside the remaining frames.
    for f in 0..900 {
        if let Some(mut input) = app.world_mut().get_mut::<VehicleInput>(car) {
            *input = VehicleInput {
                throttle: if f > 200 { 1.0 } else { 0.0 },
                ..default()
            };
        }
        app.update();
    }

    let progress = app.world().get::<RaceProgress>(car).unwrap();
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!(
        matches!(progress.state, ParticipantState::Finished { .. }),
        "expected a finish — cleared {}/{}, car at {pos:?}",
        progress.cleared_count(),
        3
    );
    assert_eq!(app.world().resource::<ResultLedger>().len(), 1);
    assert_eq!(
        app.world().resource::<RaceState>().phase,
        RacePhase::Complete
    );
    let _ = app.world().get::<VehicleState>(car);
}

/// Marker visibility tracks progress through the real
/// `update_checkpoint_markers` system: cleared gates hide and the
/// finish marker appears only once every gate is cleared (RACE-7).
#[test]
fn markers_hide_cleared_gates_and_reveal_the_finish() {
    let tmp = event_install();
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    let car = car(&mut app);
    run(&mut app, 1); // let update_checkpoint_markers settle

    // Finish marker hidden while gates are open.
    let vis_of = |app: &mut App, gate: Option<usize>| {
        app.world_mut()
            .query_filtered::<(&CheckpointMarker, &Visibility), ()>()
            .iter(app.world())
            .find(|(m, _)| m.gate == gate)
            .map(|(_, v)| *v)
    };
    assert_eq!(vis_of(&mut app, None), Some(Visibility::Hidden));

    // Clear every gate through the contract's own `advance` — the
    // swept-segment math is exercised elsewhere; this test only needs
    // the cleared flags the marker system reads.
    let def = app.world().resource::<RaceState>().definition.clone();
    for x in [COURSE[1], COURSE[2], COURSE[3]] {
        let mut p = app.world_mut().get_mut::<RaceProgress>(car).unwrap();
        p.advance(&def, Vec3::new(x - 25.0, 0.0, COURSE_Z));
        p.advance(&def, Vec3::new(x + 25.0, 0.0, COURSE_Z));
        p.advance(&def, Vec3::new(50.0, 0.0, COURSE_Z));
    }
    app.update();

    for gate in [Some(0), Some(1), Some(2)] {
        assert_eq!(
            vis_of(&mut app, gate),
            Some(Visibility::Hidden),
            "cleared gate {gate:?} should hide"
        );
    }
    assert_eq!(
        vis_of(&mut app, None),
        Some(Visibility::Visible),
        "finish appears once every gate is cleared (RACE-7)"
    );
}

/// An event whose table row doesn't exist fails the session — never a
/// silent roam.
#[test]
fn unknown_event_row_fails_the_session() {
    let tmp = event_install();
    let mut config = event_config();
    config.mode = SessionMode::Event(EventRef {
        city: "testcity".into(),
        table: EventTableKind::Checkpoint,
        index: 9,
    });
    let mut app = event_app(config, vfs_of(tmp.path()));
    app.update();
    assert!(
        matches!(phase(&app), SessionPhase::Failed(_)),
        "unknown row must fail, got {:?}",
        phase(&app)
    );
}

/// An event missing required records is Incomplete → Failed, with the
/// reason preserved.
#[test]
fn incomplete_event_fails_the_session() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/testcity/mmracedata.csv",
        &format!("{MM_HEADER}\n{ROW}\n"),
    );
    write(d, "race/testcity/race0.aimap", "#\n");
    // No waypoints.
    let mut app = event_app(event_config(), vfs_of(d));
    app.update();
    match phase(&app) {
        SessionPhase::Failed(reason) => {
            assert!(
                reason.contains("waypoints"),
                "failure should name the missing record: {reason}"
            );
        }
        other => panic!("incomplete event must fail, got {other:?}"),
    }
}

/// Restart tears the race down with the session and rebuilds it: a
/// fresh `RaceState` for generation 2, fresh markers, back in
/// `Countdown`.
#[test]
fn restart_rebuilds_the_event_session() {
    let tmp = event_install();
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    run(&mut app, 5);

    app.world_mut().resource_mut::<SessionControl>().restart = true;
    let mut reached = false;
    for _ in 0..20 {
        app.update();
        if phase(&app) == SessionPhase::Countdown
            && app.world().resource::<Session>().generation() == 2
        {
            reached = true;
            break;
        }
    }
    assert!(reached, "restart never returned to Countdown");
    let race = app.world().resource::<RaceState>();
    assert_eq!(race.generation, 2, "race belongs to the new session");
    assert!(matches!(race.phase, RacePhase::Countdown { .. }));
    let markers = app
        .world_mut()
        .query_filtered::<Entity, With<CheckpointMarker>>()
        .iter(app.world())
        .count();
    assert_eq!(markers, 4, "markers respawned with the new session");
}
