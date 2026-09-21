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
    CityEntity, EventRef, EventTableKind, ImpactEvent, Mm2Vfs, ParticipantState, PlayerVehicle,
    RacePhase, RaceProgress, RaceStarted, RaceState, ResultLedger, Session, SessionConfig,
    SessionEntity, SessionMode, SessionPhase, advance_session_tick, despawn_session_entities,
};
use mm2_vehicle::{VehicleConfig, VehicleInput, VehiclePlugin, VehicleState};

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";
const ROW: &str = "none,0,0,0,0,0,0.1,0.0,1,50,1,0,0,0,0,0,0.2,0.0,1,40,1";

/// The synthetic course: a straight +X lane on clear dev-world ground
/// (z=140 has no obstacles between x≈40 and the east wall).
const COURSE: &[f32] = &[60.0, 110.0, 140.0, 165.0, 180.0];
const COURSE_Z: f32 = 140.0;

fn write(dir: &Path, rel: &str, contents: impl AsRef<[u8]>) {
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
        format!("{MM_HEADER}\n{ROW}\n"),
    );
    write(d, "race/testcity/race0.aimap", "#\n");
    write(
        d,
        "race/testcity/race0waypoints.csv",
        format!(
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

    // The player spawned at the authored slot — on the start line
    // (the designed fallback, no `_strtpnts` authored) — facing
    // the course, and it is a race participant.
    let car = car(&mut app);
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!(
        (pos.x - COURSE[0]).abs() < 3.0 && (pos.z - COURSE_Z).abs() < 3.0,
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

/// Authored `_strtpnts` yaw is vehicle yaw (measured: the `a` column's
/// ~92° faces the retail `cir1` −X course). The player slot's heading
/// must reach the spawn verbatim — reading it as the waypoint bearing
/// turned the car +180° and spawned it backward on SF's authored grids.
#[test]
fn authored_strtpnts_yaw_faces_the_player_spawn() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/testcity/mmcircuitdata.csv",
        format!("{MM_HEADER}\n{ROW}\n"),
    );
    write(d, "race/testcity/circuit0.aimap", "#\n");
    write(
        d,
        "race/testcity/circuit0waypoints.csv",
        format!(
            "{WAYPOINTS}{}{}{}{}",
            waypoint_row(60.0, COURSE_Z),
            waypoint_row(110.0, COURSE_Z),
            waypoint_row(140.0, COURSE_Z),
            waypoint_row(165.0, COURSE_Z),
        ),
    );
    // Retail-style grid facing −X (vehicle yaw +90°) on a course whose
    // waypoint tangent runs +X — the measured convention split.
    write(
        d,
        "race/testcity/cir0_strtpnts",
        "60,0,140,90,0,0,0,0,0,\n56,0,144,90,0,0,0,0,0,\n",
    );
    let config = SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "testcity".into(),
            table: EventTableKind::Circuit,
            index: 0,
        }),
        ..SessionConfig::default()
    };
    let mut app = event_app(config, vfs_of(tmp.path()));
    app.update();
    assert_eq!(phase(&app), SessionPhase::Countdown);

    let car = car(&mut app);
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!(
        (pos.x - 60.0).abs() < 3.0 && (pos.z - COURSE_Z).abs() < 3.0,
        "player on the authored grid slot, got {pos:?}"
    );
    let fwd = app.world().get::<Transform>(car).unwrap().rotation * Vec3::NEG_Z;
    assert!(
        fwd.x < -0.98,
        "authored 90° grid yaw must face −X verbatim, fwd={fwd:?}"
    );
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
    // the cleared flags the marker system reads. Flags only move while
    // `Racing`, so the participant is put there the way the driver's
    // countdown release would.
    let def = app.world().resource::<RaceState>().definition.clone();
    for x in [COURSE[1], COURSE[2], COURSE[3]] {
        let mut p = app.world_mut().get_mut::<RaceProgress>(car).unwrap();
        p.state = ParticipantState::Racing;
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
        format!("{MM_HEADER}\n{ROW}\n"),
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

/// F12-A end to end: an authored Blitz `TimeLimit` becomes the runtime
/// deadline — the session loads `Ready → Countdown → Playing`, the
/// bound params ride the definition, and a race that never finishes
/// expires into a `TimedOut` result through `advance_race`.
#[test]
fn authored_blitz_limit_times_out_the_race() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    // 0.5 s amateur limit → 60 fixed ticks = 30 updates of racing.
    write(
        d,
        "race/testcity/mmblitzdata.csv",
        format!("{MM_HEADER}\nnone,0,2,1,0,0,0.3,0.2,3,0.5,1,0,0,0,0,0,0.4,0.1,4,0.5,1\n"),
    );
    write(d, "race/testcity/blitz0.aimap", "#\n");
    write(
        d,
        "race/testcity/blitz0waypoints.csv",
        format!(
            "{WAYPOINTS}{}{}{}{}",
            waypoint_row(COURSE[0], COURSE_Z),
            waypoint_row(COURSE[1], COURSE_Z),
            waypoint_row(COURSE[2], COURSE_Z),
            waypoint_row(COURSE[4], COURSE_Z),
        ),
    );
    let mut config = event_config();
    config.mode = SessionMode::Event(EventRef {
        city: "testcity".into(),
        table: EventTableKind::Blitz,
        index: 0,
    });
    let mut app = event_app(config, vfs_of(d));
    app.update();

    let race = app.world().resource::<RaceState>();
    assert_eq!(
        race.definition.time_limit_ticks,
        Some(60),
        "the authored 0.5 s limit bound to fixed ticks"
    );
    assert_eq!(race.definition.params.densities.traffic, 0.3);
    assert_eq!(race.definition.params.densities.pedestrians, 0.2);
    assert_eq!(race.definition.params.conditions.time_of_day.get(), 2);
    assert_eq!(race.definition.params.conditions.weather.get(), 1);
    assert_eq!(race.definition.params.opponents, 0);
    assert_eq!(race.time_remaining(), Some(60));

    let car = car(&mut app);
    run(&mut app, 260); // countdown (≈180) + 60-tick limit (30) + slack

    let progress = app.world().get::<RaceProgress>(car).unwrap();
    assert!(
        matches!(
            progress.state,
            ParticipantState::TimedOut { race_ticks: 60, .. }
        ),
        "the authored deadline expired the race: {:?}",
        progress.state
    );
    assert_eq!(
        app.world().resource::<RaceState>().phase,
        RacePhase::Complete
    );
    let ledger = app.world().resource::<ResultLedger>();
    assert_eq!(ledger.len(), 1);
    assert!(
        matches!(
            ledger.iter().next().unwrap().outcome,
            mm2_game::SessionOutcome::TimedOut { race_ticks: 60 }
        ),
        "the one retained result is the timeout"
    );
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

// ---------------------------------------------------------------------------
// Event pathset overlays (F03-AC04)
// ---------------------------------------------------------------------------

/// One `PTH1` path record: `name`, `points`, raw `kind`/`spacing` bytes.
fn pth1_path(name: &str, points: &[[f32; 3]], kind: u8, spacing: u8) -> Vec<u8> {
    let mut d = vec![0u8; 32];
    d[..name.len()].copy_from_slice(name.as_bytes());
    d.extend_from_slice(&(points.len() as u32).to_le_bytes());
    d.extend_from_slice(&0u32.to_le_bytes()); // selection
    for p in points {
        d.extend_from_slice(&0u32.to_le_bytes()); // attributes
        for c in p {
            d.extend_from_slice(&c.to_le_bytes());
        }
    }
    d.push(kind);
    d.push(spacing);
    d.extend_from_slice(&[0, 0]);
    d
}

/// A `PTH1` file from path records.
fn pth1(paths: &[Vec<u8>]) -> Vec<u8> {
    let mut d = b"PTH1".to_vec();
    d.extend_from_slice(&(paths.len() as u32).to_le_bytes());
    d.extend_from_slice(&0u32.to_le_bytes()); // current_path
    for p in paths {
        d.extend_from_slice(p);
    }
    d
}

/// PKG3 with one tetrahedron geometry chunk (`testprop_h`), no shaders —
/// the same minimal prop `import_pipeline` stamps through INST.
fn testprop_pkg() -> Vec<u8> {
    let mut geo = Vec::new();
    geo.extend_from_slice(&1u32.to_le_bytes()); // nSections
    geo.extend_from_slice(&4u32.to_le_bytes()); // total vertices
    geo.extend_from_slice(&12u32.to_le_bytes()); // total indices
    geo.extend_from_slice(&1u32.to_le_bytes()); // sections duplicate
    geo.extend_from_slice(&0x112u32.to_le_bytes()); // fvf: XYZ|NORMAL|1 tex
    geo.extend_from_slice(&1u16.to_le_bytes()); // nStrips
    geo.extend_from_slice(&0u16.to_le_bytes()); // section flags
    geo.extend_from_slice(&(-1i32).to_le_bytes()); // shader offset → fallback
    geo.extend_from_slice(&3i32.to_le_bytes()); // prim type: triangles
    geo.extend_from_slice(&4u32.to_le_bytes()); // strip vertices
    let verts: &[([f32; 3], [f32; 3], [f32; 2])] = &[
        ([0., 0., 0.], [0., 1., 0.], [0., 0.]),
        ([1., 0., 0.], [0., 1., 0.], [1., 0.]),
        ([0., 0., 1.], [0., 1., 0.], [0., 1.]),
        ([0., 1., 0.], [0., 1., 0.], [0.5, 0.5]),
    ];
    for &(p, n, uv) in verts {
        for c in p {
            geo.extend_from_slice(&c.to_le_bytes());
        }
        for c in n {
            geo.extend_from_slice(&c.to_le_bytes());
        }
        for c in uv {
            geo.extend_from_slice(&c.to_le_bytes());
        }
    }
    geo.extend_from_slice(&12u32.to_le_bytes()); // strip indices
    for i in [0u16, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3] {
        geo.extend_from_slice(&i.to_le_bytes());
    }

    let mut d = Vec::new();
    d.extend_from_slice(b"PKG3");
    d.extend_from_slice(b"FILE");
    d.push(b"testprop_h".len() as u8 + 1);
    d.extend_from_slice(b"testprop_h");
    d.push(0);
    d.extend_from_slice(&(geo.len() as u32).to_le_bytes());
    d.extend_from_slice(&geo);
    d
}

/// `event_install` plus a `race0.pathset` overlay: one `testprop`
/// line strip — a 12 m segment stamped at 5 m intervals gives 4
/// placements (t = 0, 5, 10 + the end cap).
fn overlay_install() -> tempfile::TempDir {
    let tmp = event_install();
    write(tmp.path(), "geometry/testprop.pkg", testprop_pkg());
    write(
        tmp.path(),
        "race/testcity/race0.pathset",
        pth1(&[pth1_path(
            "testprop",
            &[[200.0, 0.0, 140.0], [212.0, 0.0, 140.0]],
            2,
            20,
        )]),
    );
    tmp
}

/// Session-owned entities an `event-pathset-*` name marks.
fn overlay_entities(app: &mut App) -> Vec<(Entity, SessionEntity)> {
    app.world_mut()
        .query_filtered::<(Entity, &Name, &SessionEntity), With<CityEntity>>()
        .iter(app.world())
        .filter(|(_, name, _)| name.as_str().starts_with("event-pathset-"))
        .map(|(e, _, owner)| (e, *owner))
        .collect()
}

/// The event's `.pathset` record stamps through the real
/// `load_session_world` path: `spawn_event_pathsets` places each
/// authored stamp as a session-owned prop (render part + static
/// collider), exactly like the ambient city set.
#[test]
fn event_pathset_overlay_spawns_session_owned_props() {
    let tmp = overlay_install();
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    assert_eq!(phase(&app), SessionPhase::Countdown);

    let props = overlay_entities(&mut app);
    assert_eq!(
        props.len(),
        8,
        "4 stamps × (1 render part + 1 collider), got {props:?}"
    );
    assert!(
        props.iter().all(|(_, owner)| *owner == SessionEntity(1)),
        "every overlay entity is owned by the session that stamped it"
    );
}

/// F03-AC04: restarting the event removes exactly its overlay and
/// restamps it once — generation 2 carries the same prop count with
/// no leftovers from generation 1.
#[test]
fn restarting_the_event_respawns_its_overlay_once() {
    let tmp = overlay_install();
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    let first = overlay_entities(&mut app);
    assert_eq!(first.len(), 8);

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

    let second = overlay_entities(&mut app);
    assert_eq!(
        second.len(),
        first.len(),
        "the overlay restamps exactly once — no duplicates, nothing lost"
    );
    assert!(
        second.iter().all(|(_, owner)| *owner == SessionEntity(2)),
        "generation-1 overlay entities must not survive teardown"
    );
}

/// Every path class is counted, not dropped: prop strips stamp,
/// `PATHnn` labels, `giz_*` animated objects, decal textures and dead
/// refs each land in their own report field, and an undocumented kind
/// is a validation issue that stamps nothing.
#[test]
fn event_pathset_classification_counts_every_path() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "geometry/testprop.pkg", testprop_pkg());
    std::fs::create_dir_all(d.join("texture")).unwrap();
    std::fs::write(
        d.join("texture/testdecal.png"),
        include_bytes!("../../../assets/texture/dev_road.png"),
    )
    .unwrap();
    write(
        d,
        "race/testcity/race0.pathset",
        pth1(&[
            pth1_path("testprop", &[[0.0, 0.0, 0.0], [12.0, 0.0, 0.0]], 2, 20),
            pth1_path("PATH03", &[[0.0, 0.0, 0.0], [5.0, 0.0, 0.0]], 2, 20),
            pth1_path("giz_pcar01_l", &[[0.0, 0.0, 0.0], [5.0, 0.0, 0.0]], 2, 20),
            pth1_path("testdecal", &[[0.0, 0.0, 0.0], [5.0, 0.0, 0.0]], 2, 20),
            pth1_path("nosuchprop", &[[0.0, 0.0, 0.0], [5.0, 0.0, 0.0]], 2, 20),
            pth1_path("testprop", &[[0.0, 0.0, 0.0], [5.0, 0.0, 0.0]], 9, 20),
        ]),
    );
    let vfs = vfs_of(d);

    let mut world = World::new();
    let mut queue = bevy::ecs::world::CommandQueue::default();
    let mut meshes: Assets<Mesh> = Assets::default();
    let mut images: Assets<Image> = Assets::default();
    let mut materials: Assets<StandardMaterial> = Assets::default();
    let report = {
        let mut commands = Commands::new(&mut queue, &world);
        let mut session = mm2_game::Session::new();
        mm2_app::city::spawn_event_pathsets(
            &mut commands,
            &vfs,
            &["race/testcity/race0.pathset".to_string()],
            &mut meshes,
            &mut images,
            &mut materials,
            SessionEntity(1),
            &mut session,
        )
    };
    queue.apply(&mut world);

    assert_eq!(report.files, 1);
    assert!(report.failed_files.is_empty());
    assert_eq!(report.stats.spawned, 4, "the one prop strip stamps");
    assert_eq!(report.stats.label_paths, 1, "PATH03 is a route label");
    assert_eq!(report.stats.animated_paths, 1, "giz_pcar01_l is animated");
    assert_eq!(
        report.stats.decal_paths, 1,
        "testdecal resolves to a texture"
    );
    assert_eq!(report.stats.unresolved_paths, 1, "nosuchprop is a dead ref");
    assert_eq!(report.stats.issues, 1, "kind 9 is an undocumented type");
    // The kind-9 path named `testprop` stamps nothing despite resolving.
    let stamped = world
        .query_filtered::<&Name, With<CityEntity>>()
        .iter(&world)
        .filter(|n| n.as_str().starts_with("event-pathset-"))
        .count();
    assert_eq!(stamped, 8, "4 stamps × (part + collider)");

    // A record that does not parse is reported, never silently dropped.
    write(d, "race/testcity/race1.pathset", b"PTH1\x01bad");
    let report = {
        let mut commands = Commands::new(&mut queue, &world);
        let mut session = mm2_game::Session::new();
        mm2_app::city::spawn_event_pathsets(
            &mut commands,
            &vfs,
            &["race/testcity/race1.pathset".to_string()],
            &mut meshes,
            &mut images,
            &mut materials,
            SessionEntity(1),
            &mut session,
        )
    };
    assert_eq!(report.files, 1);
    assert_eq!(report.failed_files.len(), 1);
}
