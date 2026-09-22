//! F12-C scripted-driver tests: the `--bot` course-follower drives the
//! real session systems (`load_session_world` → `advance_race`) to a
//! finish the throttle-hold driver cannot reach — an L-turn checkpoint
//! course and a two-lap ordered circuit — plus the pure control law.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::race;
use mm2_app::scripted::{self, ScriptedBot, ScriptedDrive};
use mm2_app::session::{self, SessionControl};
use mm2_app::{camera, contracts};
use mm2_assets::Vfs;
use mm2_game::{
    Checkpoint, CheckpointRule, EventRef, EventTableKind, ImpactEvent, Mm2Vfs, ParticipantState,
    PlayerVehicle, RaceDefinition, RacePhase, RaceProgress, RaceStarted, RaceState, ResultLedger,
    Session, SessionConfig, SessionMode, SessionOutcome, advance_session_tick,
    despawn_session_entities,
};
use mm2_vehicle::{VehicleConfig, VehicleInput, VehiclePlugin};

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";

fn write(dir: &Path, rel: &str, contents: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

fn waypoint_row(x: f32, z: f32) -> String {
    format!("{x},0,{z},0,15,0,0,0,\n")
}

/// A checkpoint event whose course demands a real turn: +X along
/// `z = 140`, then −Z down `x = 100` — both lanes are clear dev-world
/// ground, and no straight line touches every gate.
fn turn_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/testcity/mmracedata.csv",
        &format!("{MM_HEADER}\nnone,0,0,0,0,0,0.1,0.0,1,50,1,0,0,0,0,0,0.2,0.0,1,40,1\n"),
    );
    write(d, "race/testcity/race0.aimap", "#\n");
    write(
        d,
        "race/testcity/race0waypoints.csv",
        &format!(
            "{WAYPOINTS}{}{}{}{}{}{}",
            waypoint_row(-60.0, 140.0), // start line
            waypoint_row(20.0, 140.0),  // gates
            waypoint_row(70.0, 140.0),
            waypoint_row(100.0, 60.0),
            waypoint_row(100.0, -40.0),
            waypoint_row(100.0, -90.0), // finish
        ),
    );
    tmp
}

/// A square circuit around clear dev-world ground — `x ∈ [0, 80]`,
/// `z ∈ [60, 140]` — with `NumLaps = 2` authored.
fn circuit_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/testcity/mmcircuitdata.csv",
        &format!("{MM_HEADER}\nnone,0,0,0,0,0,0.1,0.0,2,50,1,0,0,0,0,0,0.2,0.0,2,40,1\n"),
    );
    write(d, "race/testcity/circuit0.aimap", "#\n");
    write(
        d,
        "race/testcity/circuit0waypoints.csv",
        &format!(
            "{WAYPOINTS}{}{}{}{}",
            waypoint_row(0.0, 140.0), // start line (gate copy closes the lap)
            waypoint_row(80.0, 140.0),
            waypoint_row(80.0, 60.0),
            waypoint_row(0.0, 60.0),
        ),
    );
    tmp
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

fn event_config(table: EventTableKind) -> SessionConfig {
    SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "testcity".into(),
            table,
            index: 0,
        }),
        ..SessionConfig::default()
    }
}

/// The `headless_smoke` system set plus the scripted driver — the same
/// production path `--event --headless --bot` runs.
fn bot_app(config: SessionConfig, vfs: Vfs) -> App {
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
        .insert_resource(ScriptedDrive)
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .add_message::<RaceStarted>()
        .init_resource::<contracts::ImpactFilter>()
        .init_resource::<mm2_app::damage::DamageReport>()
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
                scripted::scripted_drive.run_if(resource_exists::<ScriptedDrive>),
            ),
        );
    app.finish();
    app.cleanup();
    app
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

/// The control law: right of the nose is positive steering, the
/// throttle bands step down with bearing, and the corner brake
/// overrides throttle only above its speed floor.
#[test]
fn input_law_steers_and_modulates_throttle() {
    let mut bot = ScriptedBot::default();

    let straight = scripted::scripted_input(&mut bot, 0.05, 10.0, true);
    assert_eq!(straight.throttle, 1.0);
    assert!(straight.steering.abs() < 0.1);

    let right = scripted::scripted_input(&mut bot, 0.5, 10.0, true);
    assert!(right.steering > 0.5, "target right steers right");
    assert_eq!(right.throttle, 0.45, "moderate bearing lifts the throttle");

    let left = scripted::scripted_input(&mut bot, -1.5, 10.0, true);
    assert_eq!(left.steering, -1.0, "steering saturates at full lock");
    assert_eq!(left.throttle, 0.25, "sharp bearing drops to the crawl band");

    // The corner brake engages only with speed: sharp bearing at 20 m/s
    // brakes, at 10 m/s it keeps the crawl throttle.
    let fast = scripted::scripted_input(&mut bot, 1.5, 20.0, true);
    assert_eq!(fast.brake, 0.6);
    assert_eq!(fast.throttle, 0.0);
    let slow = scripted::scripted_input(&mut bot, 1.5, 10.0, true);
    assert_eq!(slow.brake, 0.0);
    assert!(slow.throttle > 0.0);

    // The speed cap coasts: no throttle above it even when straight.
    let capped = scripted::scripted_input(&mut bot, 0.0, 35.0, true);
    assert_eq!(capped.throttle, 0.0);
    assert_eq!(capped.brake, 0.0);
}

/// The opponent tuning overlay (F15-B.2): the authored `maxThrottle`
/// caps every band including the speed-capped coast, and the authored
/// corner-speed multiplier raises the corner-brake floor — at 2.0 a
/// 20 m/s sharp bearing is under the raised floor and keeps the crawl
/// throttle instead of braking. `ScriptedTuning::DEFAULT` reproduces
/// the untuned law exactly.
#[test]
fn tuned_input_law_scales_throttle_and_corner_floor() {
    use mm2_app::scripted::ScriptedTuning;

    let mut bot = ScriptedBot::default();
    let tame = ScriptedTuning {
        throttle_cap: 0.4,
        corner_speed: ScriptedTuning::DEFAULT.corner_speed,
    };
    let straight = scripted::scripted_input_tuned(&mut bot, 0.05, 10.0, true, &tame);
    assert!(
        (straight.throttle - 0.4).abs() < 1e-6,
        "the authored cap tops the full band: {}",
        straight.throttle
    );
    let mid = scripted::scripted_input_tuned(&mut bot, 0.5, 10.0, true, &tame);
    assert_eq!(mid.throttle, 0.4, "0.45 is over the cap — ceiling binds");
    let sharp = scripted::scripted_input_tuned(&mut bot, -1.5, 10.0, true, &tame);
    assert_eq!(
        sharp.throttle, 0.25,
        "0.25 is under the cap — the band is a ceiling, not a scale"
    );

    let fast_corner = ScriptedTuning {
        throttle_cap: 1.0,
        corner_speed: ScriptedTuning::DEFAULT.corner_speed * 2.0,
    };
    let tuned = scripted::scripted_input_tuned(&mut bot, 1.5, 20.0, true, &fast_corner);
    assert_eq!(
        tuned.brake, 0.0,
        "20 m/s is under the doubled corner floor — no brake"
    );
    assert_eq!(tuned.throttle, 0.25, "the sharp band still applies");
    let untuned = scripted::scripted_input(&mut ScriptedBot::default(), 1.5, 20.0, true);
    assert_eq!(
        untuned.brake, 0.6,
        "the default floor still brakes at 20 m/s"
    );

    // The default overlay is the untuned law, bit-for-bit.
    let mut a = ScriptedBot::default();
    let mut b = ScriptedBot::default();
    for bearing in [0.0f32, 0.3, -0.8, 1.5] {
        let d = scripted::scripted_input(&mut a, bearing, 20.0, true);
        let t =
            scripted::scripted_input_tuned(&mut b, bearing, 20.0, true, &ScriptedTuning::DEFAULT);
        assert_eq!(d.throttle, t.throttle);
        assert_eq!(d.brake, t.brake);
        assert_eq!(d.steering, t.steering);
    }
}

/// Grounded and barely moving with the throttle demanded accumulates
/// the stuck timer into a two-phase escape — reverse-and-turn, then a
/// forward full-lock — then releases back to normal drive. Airborne
/// frames do not count as stuck.
#[test]
fn input_law_recovers_from_stuck() {
    let mut bot = ScriptedBot::default();

    // A few airborne frames with zero speed must not build the timer.
    for _ in 0..200 {
        scripted::scripted_input(&mut bot, 0.0, 0.0, false);
    }
    assert_eq!(bot.stuck_frames, 0);
    assert_eq!(bot.reverse_frames, 0);
    assert_eq!(bot.turn_frames, 0);

    // Grounded, throttling, stopped: after the stuck window the bot
    // reverses — brake (which doubles as reverse at a standstill) with
    // the steering inverted so the nose swings back at the target.
    for _ in 0..90 {
        scripted::scripted_input(&mut bot, 0.5, 0.2, true);
    }
    let reversing = scripted::scripted_input(&mut bot, 0.5, 0.0, true);
    assert_eq!(reversing.brake, 1.0);
    assert_eq!(reversing.throttle, 0.0);
    assert!(
        reversing.steering < 0.0,
        "reverse steers opposite the bearing"
    );

    // The reverse runs its frames, then the escape's second half turns
    // the car at full lock — the side it flipped to when it triggered.
    for _ in 0..71 {
        scripted::scripted_input(&mut bot, 0.5, -1.0, true);
    }
    let turning = scripted::scripted_input(&mut bot, 0.5, 0.0, true);
    assert_eq!(turning.brake, 0.0);
    assert_eq!(turning.throttle, 0.5);
    assert_eq!(turning.steering, -1.0, "the escape's forward full-lock");

    // Once the turn runs out, normal drive resumes.
    for _ in 0..44 {
        scripted::scripted_input(&mut bot, 0.5, 0.0, true);
    }
    let resumed = scripted::scripted_input(&mut bot, 0.5, 0.0, true);
    assert!(
        resumed.throttle > 0.0,
        "recovery hands control back to drive"
    );
}

/// `drive_target` exposes the objective the route owns: under
/// `AnyOrder` the earliest un-cleared gate in authored order (then the
/// armed finish); under `Ordered` the participant's `next` required
/// gate.
#[test]
fn drive_target_tracks_the_live_objective() {
    let gate = |x: f32, z: f32| Checkpoint {
        center: Vec3::new(x, 0.0, z),
        radius: 10.0,
        height: 8.0,
        heading_deg: 0.0,
        require_direction: false,
    };
    let any = RaceDefinition {
        checkpoints: vec![gate(60.0, 0.0), gate(0.0, 60.0)],
        finish: Some(gate(-20.0, -20.0)),
        rule: CheckpointRule::AnyOrder,
        laps: 0,
        time_limit_ticks: None,
        params: mm2_game::EventParams::default(),
        countdown_ticks: 0,
        start_slots: Vec::new(),
    };
    let mut progress = RaceProgress::new(&any);
    progress.state = ParticipantState::Racing;
    // The route is authored order, not proximity: clearing gate 1 out
    // of order first still leaves gate 0 as the objective.
    progress.advance(&any, Vec3::new(0.0, 0.0, 70.0));
    progress.advance(&any, Vec3::new(0.0, 0.0, 52.0));
    assert_eq!(
        scripted::drive_target(&any, &progress),
        Some(Vec3::new(60.0, 0.0, 0.0)),
        "gate 1 cleared out of order — gate 0 is still the objective"
    );
    progress.advance(&any, Vec3::new(55.0, 0.0, 4.0));
    assert_eq!(
        scripted::drive_target(&any, &progress),
        Some(Vec3::new(-20.0, 0.0, -20.0)),
        "all gates cleared aims the bot at the finish trigger"
    );

    let ordered = RaceDefinition {
        checkpoints: vec![gate(60.0, 0.0), gate(0.0, 60.0)],
        finish: None,
        rule: CheckpointRule::Ordered,
        laps: 2,
        ..any.clone()
    };
    let mut progress = RaceProgress::new(&ordered);
    progress.state = ParticipantState::Racing;
    let t = scripted::drive_target(&ordered, &progress);
    assert_eq!(t, Some(Vec3::new(60.0, 0.0, 0.0)));
}

/// The scripted driver obeys the countdown lock: while `input_locked`
/// holds, `VehicleInput` stays zeroed — the bot cannot creep off the
/// line (AC03).
#[test]
fn scripted_drive_honors_the_countdown_lock() {
    let tmp = turn_install();
    let mut app = bot_app(event_config(EventTableKind::Checkpoint), vfs_of(tmp.path()));
    app.update();
    let car = car(&mut app);
    assert!(app.world().resource::<RaceState>().input_locked());

    run(&mut app, 30);
    let input = app.world().get::<VehicleInput>(car).unwrap();
    assert_eq!(input.throttle, 0.0);
    assert_eq!(input.steering, 0.0);
    assert_eq!(input.brake, 0.0);
}

/// Full production path: the scripted driver steers the L-course —
/// gates the throttle-hold driver could never all reach — sweeps every
/// checkpoint, crosses the finish and records exactly one `Finished`
/// result through `advance_race`.
#[test]
fn bot_drives_a_turning_course_to_the_finish() {
    let tmp = turn_install();
    let mut app = bot_app(event_config(EventTableKind::Checkpoint), vfs_of(tmp.path()));
    app.update();
    let car = car(&mut app);

    run(&mut app, 2400);

    let progress = app.world().get::<RaceProgress>(car).unwrap();
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!(
        matches!(progress.state, ParticipantState::Finished { .. }),
        "expected a finish — cleared {}/{}, car at {pos:?}",
        progress.cleared_count(),
        4
    );
    let ledger = app.world().resource::<ResultLedger>();
    assert_eq!(ledger.len(), 1);
    assert!(
        matches!(
            ledger.iter().next().unwrap().outcome,
            SessionOutcome::Finished { .. }
        ),
        "the one retained result is the finish"
    );
    assert_eq!(
        app.world().resource::<RaceState>().phase,
        RacePhase::Complete
    );
    // Resolved participants yield: the bot leaves the car parked.
    let input = app.world().get::<VehicleInput>(car).unwrap();
    assert_eq!(input.throttle, 0.0);
}

/// Ordered rules: the bot follows `progress.next` around a square
/// circuit for both authored laps — the lap-wrap (`next` → 0, gates
/// re-armed) is driven, not simulated — and the second crossing of the
/// lifted start line finishes the race.
#[test]
fn bot_laps_an_ordered_circuit_twice() {
    let tmp = circuit_install();
    let mut app = bot_app(event_config(EventTableKind::Circuit), vfs_of(tmp.path()));
    app.update();
    let car = car(&mut app);

    assert_eq!(
        app.world().resource::<RaceState>().definition.laps,
        2,
        "the authored NumLaps bound"
    );
    run(&mut app, 4200);

    let progress = app.world().get::<RaceProgress>(car).unwrap();
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!(
        matches!(progress.state, ParticipantState::Finished { .. }),
        "expected a two-lap finish — lap {}, cleared {}/{}, car at {pos:?}",
        progress.lap,
        progress.cleared_count(),
        4
    );
    let ledger = app.world().resource::<ResultLedger>();
    assert_eq!(ledger.len(), 1);
    assert!(matches!(
        ledger.iter().next().unwrap().outcome,
        SessionOutcome::Finished { .. }
    ));
    assert_eq!(
        app.world().resource::<RaceState>().phase,
        RacePhase::Complete
    );
}
