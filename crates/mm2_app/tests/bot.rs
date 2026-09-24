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
use mm2_app::scripted::{self, ScriptedBot, ScriptedDrive, ScriptedRoute};
use mm2_app::session::{self, SessionControl};
use mm2_app::{camera, contracts};
use mm2_assets::Vfs;
use mm2_game::{
    Checkpoint, CheckpointRule, EventRef, EventTableKind, ImpactEvent, Mm2Vfs, OpponentRoster,
    OpponentRoute, OpponentRoutePoint, OpponentSpec, ParticipantState, PlayerVehicle,
    RaceDefinition, RacePhase, RaceProgress, RaceStarted, RaceState, ResultLedger, Session,
    SessionConfig, SessionMode, SessionOutcome, advance_session_tick, despawn_session_entities,
};
use mm2_vehicle::{VehicleConfig, VehicleInput, VehiclePlugin};

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";
const OPP_HEADER: &str =
    "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\n";

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
        .add_message::<mm2_game::RecoveryEvent>()
        .init_resource::<contracts::ImpactFilter>()
        .init_resource::<mm2_app::damage::DamageReport>()
        .init_resource::<mm2_app::stuck::StuckReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<mm2_app::recovery::RecoveryReport>()
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<mm2_app::spark_fx::SparkFxReport>()
        .init_resource::<mm2_app::texel_fx::TexelDamageReport>()
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
    assert_eq!(bot.escapes, 1, "the triggered escape is counted");
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

fn route_point(x: f32, y: f32, z: f32) -> OpponentRoutePoint {
    OpponentRoutePoint {
        position: Vec3::new(x, y, z),
        brake: 0.0,
        forward_offset: 0.0,
        side_offset: 0.0,
        target_speed: 0.0,
        speed_start: 0.0,
        side_start: 0.0,
    }
}

fn route_of(pts: &[(f32, f32, f32)]) -> OpponentRoute {
    OpponentRoute {
        points: pts.iter().map(|&(x, y, z)| route_point(x, y, z)).collect(),
    }
}

fn opp_row(x: f32, z: f32) -> String {
    format!("{x},0,{z},0,0,0,0,0,0\n")
}

/// A checkpoint event wired with an `[Opponent]` route that detours
/// far off the gate straight-line: start (−150,−140), gates at
/// (−100|0|100, −140), finish (170,−140), while the `.opp` swings out
/// to (−40,120) between gates 0 and 1 — 260 m off the gate line, a
/// place the gate-to-gate aim never goes. The detour legs are laid out
/// to clear the dev world's bumps and slalom barriers. The wired
/// vehicle id resolves to nothing (the authored slot is warned and
/// skipped); the route the bot borrows still resolves.
fn routed_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/testcity/mmracedata.csv",
        &format!("{MM_HEADER}\nnone,0,0,0,1,0,0.1,0.0,1,50,1,0,0,0,1,0,0.2,0.0,1,40,1\n"),
    );
    write(
        d,
        "race/testcity/race0.aimap",
        "[Opponent]\n1\nvpmissing race0-a-0.opp 0.9 0 50.0 0.7 1 1 1 1 0 1.0\n",
    );
    write(
        d,
        "race/testcity/race0-a-0.opp",
        &format!(
            "{OPP_HEADER}{}{}{}{}{}{}",
            opp_row(-160.0, -140.0), // staging
            opp_row(-100.0, -140.0), // gate 0
            opp_row(-40.0, 120.0),   // the detour the gate line never sees
            opp_row(0.0, -140.0),    // gate 1
            opp_row(100.0, -140.0),  // gate 2
            opp_row(170.0, -140.0),  // finish
        ),
    );
    write(
        d,
        "race/testcity/race0waypoints.csv",
        &format!(
            "{WAYPOINTS}{}{}{}{}{}",
            waypoint_row(-150.0, -140.0), // start line
            waypoint_row(-100.0, -140.0), // gates
            waypoint_row(0.0, -140.0),
            waypoint_row(100.0, -140.0),
            waypoint_row(170.0, -140.0), // finish
        ),
    );
    tmp
}

/// `route_aim` chases route anchors until the anchor nearest the
/// objective gate, where the aim hands off to the gate itself.
#[test]
fn route_aim_chases_the_line_until_the_gate_anchor() {
    let route = route_of(&[(0.0, 0.0, 0.0), (100.0, 0.0, 0.0), (200.0, 0.0, 0.0)]);
    let gate = Vec3::new(200.0, 0.0, 0.0); // gate anchor = index 2

    // A car behind the first anchor chases the route, not the gate:
    // 30 m to anchor 0 leaves 10 m of lookahead down the next leg.
    let (next, aim) = scripted::route_aim(&route, 0, Vec3::new(0.0, 0.0, -30.0), gate);
    assert_eq!(next, 0);
    assert!(
        (aim - Vec3::new(10.0, 0.0, 0.0)).length() < 0.01,
        "aim {aim:?}"
    );

    // Past anchor 0's perpendicular plane the chase advances; the aim
    // rides the leg to anchor 1 the lookahead out — still not the gate.
    let (next, aim) = scripted::route_aim(&route, 0, Vec3::new(50.0, 0.0, 3.0), gate);
    assert_eq!(next, 1);
    assert!(
        (aim - Vec3::new(89.93, 0.0, 0.60)).length() < 0.05,
        "aim {aim:?}"
    );

    // Reaching the gate's own anchor hands the aim to the trigger.
    let (next, aim) = scripted::route_aim(&route, 1, Vec3::new(95.0, 0.0, 2.0), gate);
    assert_eq!(next, 2);
    assert_eq!(aim, gate);
}

/// The lookahead aim wraps a bend before the car reaches its apex —
/// the bearing opens while the turn is still ahead, which is what lets
/// the corner-brake band scrub speed in time — and a gate just outside
/// the window is approached down its leg, never passed.
#[test]
fn route_aim_lookahead_bends_around_the_apex() {
    let route = route_of(&[(0.0, 0.0, 0.0), (100.0, 0.0, 0.0), (100.0, 0.0, 100.0)]);
    let gate = Vec3::new(100.0, 0.0, 100.0); // gate anchor = index 2

    // 20 m before the apex the window wraps the corner: the aim is
    // 20 m down the post-apex leg, not the apex itself.
    let (next, aim) = scripted::route_aim(&route, 1, Vec3::new(80.0, 0.0, 0.0), gate);
    assert_eq!(next, 1);
    assert!(
        (aim - Vec3::new(100.0, 0.0, 20.0)).length() < 0.01,
        "aim {aim:?}"
    );

    let route = route_of(&[(0.0, 0.0, 0.0), (100.0, 0.0, 0.0), (300.0, 0.0, 0.0)]);
    let gate = Vec3::new(300.0, 0.0, 0.0); // gate anchor = index 2
    let (next, aim) = scripted::route_aim(&route, 0, Vec3::new(70.0, 0.0, 0.0), gate);
    assert_eq!(next, 1);
    assert!(
        (aim - Vec3::new(110.0, 0.0, 0.0)).length() < 0.01,
        "aim {aim:?}"
    );
}

/// Advancement is bounded at the objective's anchor: an off-height car
/// whose XZ already ran past it (the elevated-road fall the sf
/// circuit:0 evidence run took) cannot chase the course onward while
/// the gate stays un-cleared — the aim returns to the gate.
#[test]
fn route_aim_caps_advance_at_the_objective_anchor() {
    let route = route_of(&[(0.0, 0.0, 0.0), (100.0, 0.0, 0.0), (200.0, 0.0, 0.0)]);
    let gate = Vec3::new(100.0, 8.0, 0.0); // gate anchor = index 1

    // Displaced past the anchor in XZ, below the gate's height — the
    // chase stops at anchor 1 and aims at the gate, not onward at 2.
    let (next, aim) = scripted::route_aim(&route, 0, Vec3::new(150.0, -20.0, 0.0), gate);
    assert_eq!(next, 1);
    assert_eq!(aim, gate);

    // The chase index itself already past it is the same shape.
    let (next, aim) = scripted::route_aim(&route, 2, Vec3::new(150.0, -20.0, 0.0), gate);
    assert_eq!(next, 2);
    assert_eq!(aim, gate);
}

/// A gate whose nearest anchor sits *behind* the chase — the car
/// passed or fell past it without clearing — is aimed at directly
/// rather than chased around the rest of the course.
#[test]
fn route_aim_gate_behind_aims_direct() {
    let route = route_of(&[(0.0, 0.0, 0.0), (100.0, 0.0, 0.0), (200.0, 0.0, 0.0)]);
    let gate = Vec3::new(40.0, 0.0, 0.0); // nearest anchor: index 0

    let (next, aim) = scripted::route_aim(&route, 2, Vec3::new(150.0, 0.0, 0.0), gate);
    assert_eq!(next, 2, "a behind-gate does not advance the chase");
    assert_eq!(aim, gate);
}

/// Closed (circuit) routes: the same cap and hand-off in lap
/// arithmetic — the chase wraps the loop, and a gate anchor more than
/// half the loop behind the chase counts as missed.
#[test]
fn route_aim_closed_route_wraps_and_detects_behind() {
    // A square loop that closes on its start (last anchor within 40 m
    // of the first — the retail circuit pattern).
    let route = route_of(&[
        (0.0, 0.0, 0.0),
        (100.0, 0.0, 0.0),
        (100.0, 0.0, 100.0),
        (0.0, 0.0, 100.0),
        (5.0, 0.0, 0.0),
    ]);

    // Gate at anchor 3: chasing anchor 4 puts the gate 4 steps behind
    // (> n/2) — missed; aim at it directly.
    let gate = Vec3::new(0.0, 0.0, 100.0);
    let (next, aim) = scripted::route_aim(&route, 4, Vec3::new(10.0, 0.0, 10.0), gate);
    assert_eq!(next, 4);
    assert_eq!(aim, gate);

    // Chasing anchor 2 toward the same gate reaches it and hands off.
    let (next, aim) = scripted::route_aim(&route, 2, Vec3::new(95.0, 0.0, 95.0), gate);
    assert_eq!(next, 3);
    assert_eq!(aim, gate);

    // The wrap itself: chasing anchor 4 toward a gate at anchor 1 walks
    // the loop's end (4 → 0 → 1) rather than turning back.
    let gate = Vec3::new(60.0, 0.0, 0.0); // nearest anchor: index 1
    let (next, aim) = scripted::route_aim(&route, 4, Vec3::new(3.0, 0.0, 2.0), gate);
    assert_eq!(next, 1);
    assert_eq!(aim, gate);
}

/// An empty route is degenerate authored data, not a crash: the aim is
/// the gate itself.
#[test]
fn route_aim_empty_route_aims_at_the_gate() {
    let route = OpponentRoute::default();
    let gate = Vec3::new(5.0, 0.0, 5.0);
    assert_eq!(scripted::route_aim(&route, 0, Vec3::ZERO, gate), (0, gate));
}

/// `route_distance` is a 3-D clearance to the polyline: parked under
/// an elevated leg on its own XZ footprint still reads off-route (the
/// retail descent fall loop), while a lane-width offset at the line's
/// height does not.
#[test]
fn route_distance_sees_height_not_just_footprint() {
    let elevated = route_of(&[(0.0, 20.0, 0.0), (100.0, 20.0, 0.0)]);
    assert!(scripted::route_distance(&elevated, Vec3::new(50.0, 20.0, 0.0)) < 0.01);
    // 15 m under the line's own footprint — the sf descent fall loop.
    assert!(scripted::route_distance(&elevated, Vec3::new(50.0, 5.0, 0.0)) > 12.0);
    // The neighbouring lane at the line's height is still "on route".
    assert!(scripted::route_distance(&elevated, Vec3::new(50.0, 20.0, 6.0)) < 12.0);
    // Empty route: no line to measure — never inside the bound.
    assert_eq!(
        scripted::route_distance(&OpponentRoute::default(), Vec3::ZERO),
        f32::MAX
    );
}

/// `pick_bot_route` borrows the wired route staged nearest the player
/// spawn; unresolved or empty routes are skipped and a route-less
/// roster yields `None` (the gate-to-gate fallback stays).
#[test]
fn pick_bot_route_prefers_the_nearest_staging() {
    let spec = |route: Option<OpponentRoute>| OpponentSpec {
        vehicle: "vpx".into(),
        params: Vec::new(),
        route,
    };
    let roster = OpponentRoster {
        entries: vec![
            spec(Some(route_of(&[(500.0, 0.0, 0.0), (600.0, 0.0, 0.0)]))),
            spec(None),
            spec(Some(route_of(&[(20.0, 0.0, 0.0), (30.0, 0.0, 0.0)]))),
        ],
        issues: Vec::new(),
    };
    let picked = scripted::pick_bot_route(&roster, Vec3::ZERO).expect("a route resolves");
    assert_eq!(picked.points[0].position, Vec3::new(20.0, 0.0, 0.0));

    assert!(scripted::pick_bot_route(&OpponentRoster::default(), Vec3::ZERO).is_none());
}

/// Full production path (F15-B.5): the event's roster binds the
/// nearest-staged `.opp` route onto the player as `ScriptedRoute`, the
/// bot chases its anchors through a 180 m detour no gate-to-gate line
/// touches, and progress still banks only through the swept triggers —
/// a real `Finished` result through `advance_race`.
#[test]
fn bot_follows_the_authored_route_off_the_gate_line() {
    let tmp = routed_install();
    let mut app = bot_app(event_config(EventTableKind::Checkpoint), vfs_of(tmp.path()));
    app.update();
    let car = car(&mut app);

    let bound = app
        .world()
        .get::<ScriptedRoute>(car)
        .expect("the roster's route bound onto the player");
    assert_eq!(bound.route.points.len(), 6);
    let detour = bound.route.points[2].position;
    assert_eq!(detour, Vec3::new(-40.0, 0.0, 120.0));

    let mut closest = f32::MAX;
    let mut finished = false;
    for _ in 0..9000 {
        app.update();
        let p = app.world().get::<Position>(car).unwrap().0;
        closest = closest.min((p.x - detour.x).hypot(p.z - detour.z));
        if matches!(
            app.world().get::<RaceProgress>(car).unwrap().state,
            ParticipantState::Finished { .. }
        ) {
            finished = true;
            break;
        }
    }
    assert!(
        closest < 25.0,
        "the bot drove the authored detour — closest approach {closest:.1} m"
    );
    assert!(finished, "the routed bot still finished the event");
}

/// The bounded last resort end to end (F15-B.5): the route-guided
/// player penned off its line — where the retail `sf circuit:0`
/// descent left the car on a lower street the gate aim could never
/// climb back to — spends a bounded window (off-route distance or the
/// displacement bubble) and re-anchors onto the chased leg through
/// the disclosed `ResetVehicle` path. The jump banks nothing
/// (`Teleported` broke the swept segment and the walk-back lands
/// outside every pending trigger), then the car resumes the route to
/// a real finish.
#[test]
fn penned_bot_reanchors_onto_the_route_and_resumes() {
    let tmp = routed_install();
    let mut app = bot_app(event_config(EventTableKind::Checkpoint), vfs_of(tmp.path()));
    app.update();
    let car = car(&mut app);
    assert!(
        app.world().get::<ScriptedRoute>(car).is_some(),
        "the roster route bound onto the player"
    );

    // During the countdown, wall off a pocket at (150,·,150) — far
    // from every gate and every route leg — and move the car in
    // through the production teleport contract.
    let y = app.world().get::<Position>(car).unwrap().0.y;
    for (center, size) in [
        (Vec3::new(148.0, y + 1.0, 150.0), Vec3::new(0.4, 3.0, 8.0)),
        (Vec3::new(152.0, y + 1.0, 150.0), Vec3::new(0.4, 3.0, 8.0)),
        (Vec3::new(150.0, y + 1.0, 148.0), Vec3::new(8.0, 3.0, 0.4)),
        (Vec3::new(150.0, y + 1.0, 152.0), Vec3::new(8.0, 3.0, 0.4)),
    ] {
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(size.x, size.y, size.z),
            Transform::from_translation(center),
        ));
    }
    app.world_mut().get_mut::<Position>(car).unwrap().0 = Vec3::new(150.0, y, 150.0);
    app.world_mut()
        .get_mut::<Transform>(car)
        .unwrap()
        .translation = Vec3::new(150.0, y, 150.0);
    app.world_mut()
        .entity_mut(car)
        .insert(mm2_vehicle::Teleported);

    // The car sits penned through release and every failed escape;
    // nothing banks until the re-anchor fires.
    let mut fired_at = None;
    for u in 0..1400 {
        app.update();
        let rs = app.world().get::<ScriptedRoute>(car).unwrap();
        if rs.reanchors > 0 {
            fired_at = Some(u);
            break;
        }
        if u > 200 {
            assert_eq!(
                app.world()
                    .get::<RaceProgress>(car)
                    .unwrap()
                    .cleared_count(),
                0,
                "the penned car banked a gate at update {u}"
            );
        }
    }
    assert!(
        fired_at.is_some(),
        "the bounded re-anchor never fired for a penned player"
    );

    // The teleport may land an update after the counter flips — give
    // the pipeline its two frames, then check the contract: back on
    // the route polyline (whichever leg was being chased), upright,
    // marker consumed, nothing banked by the jump itself.
    run(&mut app, 3);
    let pose = app.world().get::<Position>(car).unwrap().0;
    let rs = app.world().get::<ScriptedRoute>(car).unwrap();
    let on_line = rs.route.points.windows(2).any(|w| {
        let (a, b) = (w[0].position, w[1].position);
        let ab = b - a;
        let t = ((pose - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
        let p = a + ab * t;
        (p.x - pose.x).hypot(p.z - pose.z) < 2.0
    });
    assert!(on_line, "re-anchored back onto the route line: {pose:?}");
    let rot = app.world().get::<Rotation>(car).unwrap().0;
    assert!((rot * Vec3::Y).y > 0.99, "re-anchor lands upright: {rot:?}");
    assert!(
        app.world().get::<mm2_vehicle::Teleported>(car).is_none(),
        "reanchor_teleported_participants consumed the marker"
    );
    assert_eq!(
        app.world()
            .get::<RaceProgress>(car)
            .unwrap()
            .cleared_count(),
        0,
        "the teleport itself banked nothing"
    );

    // And it resumes: the re-anchored car re-drives the authored line
    // — detour included — and finishes through the swept triggers.
    run(&mut app, 9000);
    let progress = app.world().get::<RaceProgress>(car).unwrap();
    assert!(
        matches!(progress.state, ParticipantState::Finished { .. }),
        "the re-anchored player finishes for real: {:?} cleared {}",
        progress.state,
        progress.cleared_count()
    );
}

/// The third re-anchor arm (F15-B.5): a route-guided car whose generic
/// recovery keeps firing — the retail descent's fall loop, where the
/// respawn lip is itself off the line — is re-anchored after three
/// recoveries without a banked gate. Neither the displacement bubble
/// (the car moves freely) nor the 300-frame off-route window can have
/// fired first, so `reanchors` proving this fast is the arm itself.
/// A banked gate clears the count, so honest falls never arm it.
#[test]
fn falling_bot_reanchors_after_bounded_recoveries() {
    use mm2_game::{ObjectIdentity, RecoveryCause, RecoveryEvent};

    let tmp = routed_install();
    let mut app = bot_app(event_config(EventTableKind::Checkpoint), vfs_of(tmp.path()));
    app.update();
    let car = car(&mut app);
    let id = app.world().get::<ObjectIdentity>(car).unwrap().0;
    let generation = app.world().resource::<Session>().generation();

    // The countdown lock skips the route block entirely — wait for
    // Racing so the events below land on a live arm.
    let mut racing = false;
    for _ in 0..600 {
        app.update();
        if matches!(
            app.world().get::<RaceProgress>(car).unwrap().state,
            ParticipantState::Racing
        ) {
            racing = true;
            break;
        }
    }
    assert!(racing, "the countdown never released");

    // Park the car on open ground far off the line — it can drive
    // (displacement resets) and the off-route window needs 300 frames.
    let y = app.world().get::<Position>(car).unwrap().0.y;
    let spot = Vec3::new(150.0, y, 150.0);
    app.world_mut().get_mut::<Position>(car).unwrap().0 = spot;
    app.world_mut()
        .get_mut::<Transform>(car)
        .unwrap()
        .translation = spot;
    app.world_mut()
        .entity_mut(car)
        .insert(mm2_vehicle::Teleported);
    let banked = app
        .world()
        .get::<RaceProgress>(car)
        .unwrap()
        .cleared_count();

    // Two falls are not yet the arm's bound.
    for _ in 0..2 {
        app.world_mut()
            .resource_mut::<Messages<RecoveryEvent>>()
            .write(RecoveryEvent {
                object: id,
                generation,
                tick: 0,
                cause: RecoveryCause::OutOfBounds,
                landing: None,
            });
        app.update();
    }
    for _ in 0..20 {
        app.update();
    }
    assert_eq!(
        app.world().get::<ScriptedRoute>(car).unwrap().reanchors,
        0,
        "two falls must not arm the recovery re-anchor"
    );

    // The third fall without a banked gate fires it immediately —
    // still far under the off-route window.
    app.world_mut()
        .resource_mut::<Messages<RecoveryEvent>>()
        .write(RecoveryEvent {
            object: id,
            generation,
            tick: 0,
            cause: RecoveryCause::OutOfBounds,
            landing: None,
        });
    let mut fired = false;
    for _ in 0..60 {
        app.update();
        if app.world().get::<ScriptedRoute>(car).unwrap().reanchors > 0 {
            fired = true;
            break;
        }
    }
    assert!(fired, "three recoveries off the line never re-anchored");

    // Same landing contract as the displacement arm: back on the
    // polyline, marker consumed, nothing banked by the jump.
    run(&mut app, 3);
    let pose = app.world().get::<Position>(car).unwrap().0;
    let rs = app.world().get::<ScriptedRoute>(car).unwrap();
    assert_eq!(rs.recoveries, 0, "the count resets with the re-anchor");
    let on_line = rs.route.points.windows(2).any(|w| {
        let (a, b) = (w[0].position, w[1].position);
        let ab = b - a;
        let t = ((pose - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
        let p = a + ab * t;
        (p.x - pose.x).hypot(p.z - pose.z) < 2.0
    });
    assert!(on_line, "re-anchored back onto the route line: {pose:?}");
    assert_eq!(
        app.world()
            .get::<RaceProgress>(car)
            .unwrap()
            .cleared_count(),
        banked,
        "the teleport itself banked nothing"
    );
}
