//! F11-B race-runtime integration: countdown, swept triggers, progress
//! and once-only results through the production `advance_race` system —
//! the same trigger path gameplay drives, with deterministic `Position`
//! writes standing in for the physics-produced segments (AC02–AC04).

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::race::advance_race;
use mm2_app::session::{self, SessionControl};
use mm2_game::{
    Checkpoint, CheckpointRule, EventRef, EventTableKind, ParticipantState, Player, PlayerControl,
    PlayerId, ProgressOutcome, RaceDefinition, RacePhase, RaceProgress, RaceStart, RaceStarted,
    RaceState, ResultLedger, Session, SessionAuthority, SessionConfig, SessionEntity, SessionMode,
    SessionPhase, advance_session_tick, despawn_session_entities,
};

fn cp(x: f32, z: f32) -> Checkpoint {
    Checkpoint {
        center: Vec3::new(x, 0.0, z),
        radius: 15.0,
        height: mm2_game::DEFAULT_CHECKPOINT_HEIGHT,
        heading_deg: 0.0,
        require_direction: false,
    }
}

fn event_config() -> SessionConfig {
    SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "london".into(),
            table: EventTableKind::Checkpoint,
            index: 0,
        }),
        ..SessionConfig::default()
    }
}

fn any_order_def(countdown: u32) -> RaceDefinition {
    RaceDefinition {
        checkpoints: vec![cp(0.0, 0.0), cp(100.0, 0.0)],
        finish: None,
        rule: CheckpointRule::AnyOrder,
        laps: 1,
        countdown_ticks: countdown,
        start_slots: vec![RaceStart {
            position: Vec3::new(-50.0, 0.0, 0.0),
            yaw_deg: 0.0,
        }],
    }
}

/// A headless app running the production race driver plus the session
/// teardown path, minus windowing/input plugins and world content —
/// tests supply `Position` segments directly. The session is driven
/// through the same `Loading → Ready → Countdown` transitions the
/// event producer will use (F11-B.2 lands that wiring).
fn race_app(config: SessionConfig, def: RaceDefinition) -> App {
    let mut session = Session::new();
    session.begin(config).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Countdown).unwrap();
    let generation = session.generation();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        // Asset/Mesh/Gizmo plugins match the session-test harness:
        // avian's collider cache reads AssetEvent<Mesh>.
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
        .insert_resource(RaceState::new(def, generation))
        .insert_resource(mm2_app::session::SpawnPoint {
            position: Vec3::new(0.0, 1.5, 0.0),
            yaw: 0.0,
            trailers: Vec::new(),
        })
        .init_resource::<ResultLedger>()
        .init_resource::<SessionControl>()
        .init_resource::<mm2_app::contracts::ImpactFilter>()
        .init_resource::<ButtonInput<KeyCode>>()
        .add_message::<RaceStarted>()
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(FixedLast, advance_race)
        .add_systems(
            Update,
            (
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

fn phase(app: &App) -> SessionPhase {
    app.world().resource::<Session>().phase().clone()
}

fn race(app: &App) -> &RaceState {
    app.world().resource::<RaceState>()
}

fn drain_started(app: &mut App) -> usize {
    app.world_mut()
        .resource_mut::<Messages<RaceStarted>>()
        .drain()
        .count()
}

/// Spawn a race participant — a `Player`-marked entity carrying
/// `RaceProgress` and the `Position` the driver reads, stamped with the
/// session's ownership generation like every session spawn.
fn spawn_participant(app: &mut App, def: &RaceDefinition, pos: Vec3) -> (Entity, PlayerId) {
    let generation = app.world().resource::<Session>().generation();
    let id = app.world_mut().resource_mut::<Session>().mint_player_id();
    let entity = app
        .world_mut()
        .spawn((
            SessionEntity(generation),
            Player {
                id,
                control: PlayerControl::Local,
            },
            RaceProgress::new(def),
            Position(pos),
            Transform::from_translation(pos),
        ))
        .id();
    (entity, id)
}

fn set_position(app: &mut App, entity: Entity, pos: Vec3) {
    *app.world_mut().get_mut::<Position>(entity).unwrap() = Position(pos);
}

fn progress(app: &App, entity: Entity) -> &RaceProgress {
    app.world().get::<RaceProgress>(entity).unwrap()
}

fn run(app: &mut App, updates: usize) {
    for _ in 0..updates {
        app.update();
    }
}

/// AC03: the countdown releases control exactly once — one
/// `RaceStarted` message, one `Countdown → Playing` transition,
/// participants flipped to `Racing` — and nothing re-locks afterwards.
#[test]
fn countdown_releases_control_exactly_once() {
    let def = any_order_def(4);
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));
    assert_eq!(phase(&app), SessionPhase::Countdown);
    assert!(race(&app).input_locked());

    let mut starts = 0;
    // 4 ticks at 2 fixed steps per update → released after 2 updates;
    // run well past to prove nothing re-emits.
    for _ in 0..6 {
        app.update();
        starts += drain_started(&mut app);
    }
    assert_eq!(starts, 1, "RaceStarted must be written exactly once");
    assert_eq!(phase(&app), SessionPhase::Playing);
    assert!(!race(&app).input_locked());
    assert_eq!(race(&app).phase, RacePhase::Running);
    assert_eq!(
        progress(&app, car).state,
        ParticipantState::Racing,
        "the countdown released the participant"
    );
    run(&mut app, 4);
    assert_eq!(drain_started(&mut app), 0, "no second unlock");
    assert!(race(&app).clock > 0, "the race clock is running");
}

/// A race resource still counting down while the session already runs
/// (a producer that skipped the session gate) still releases exactly
/// once — the lock comes from the race, not only the session phase.
#[test]
fn countdown_completes_while_session_already_playing() {
    let def = any_order_def(3);
    let mut app = race_app(event_config(), def.clone());
    // Drive the session to Playing early — the race's own countdown
    // must still gate the participants.
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Playing)
        .unwrap();
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));
    run(&mut app, 4);
    assert_eq!(drain_started(&mut app), 1);
    assert_eq!(progress(&app, car).state, ParticipantState::Racing);
}

/// AC02 high-speed case: a 400 m step through a 15 m trigger counts —
/// the same `Position`-driven swept segment the sim produces.
#[test]
fn high_speed_crossing_cannot_be_skipped() {
    let def = any_order_def(0);
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    run(&mut app, 2); // release + anchor
    set_position(&mut app, car, Vec3::new(200.0, 0.0, 0.0));
    run(&mut app, 1);
    let p = progress(&app, car);
    assert!(p.is_cleared(0), "checkpoint 0 was swept, not sampled");
    assert!(p.is_cleared(1), "the same segment swept checkpoint 1");
    // One checkpoint left to clear… actually both cleared in one
    // segment — the AnyOrder race with no finish trigger is done.
    assert!(matches!(p.state, ParticipantState::Finished { .. }));
    assert_eq!(race(&app).phase, RacePhase::Complete);
}

/// AC02 wrong-height case: flying over the trigger's vertical band does
/// not count.
#[test]
fn crossing_at_the_wrong_height_does_not_count() {
    let def = any_order_def(0);
    let mut app = race_app(event_config(), def.clone());
    let y = mm2_game::DEFAULT_CHECKPOINT_HEIGHT + 10.0;
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, y, 0.0));
    run(&mut app, 2);
    set_position(&mut app, car, Vec3::new(200.0, y, 0.0));
    run(&mut app, 2);
    assert_eq!(progress(&app, car).cleared_count(), 0);
    // Back at road height, the same path clears it.
    set_position(&mut app, car, Vec3::new(-200.0, 0.0, 0.0));
    run(&mut app, 1); // anchor
    set_position(&mut app, car, Vec3::new(0.0, 0.0, 0.0));
    run(&mut app, 1);
    assert!(progress(&app, car).is_cleared(0));
}

/// AC02 repeated-crossing case: an already-cleared checkpoint does not
/// accumulate crossings.
#[test]
fn repeated_crossing_counts_once() {
    let def = any_order_def(0);
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-10.0, 0.0, 0.0));
    run(&mut app, 2);
    set_position(&mut app, car, Vec3::new(10.0, 0.0, 0.0));
    run(&mut app, 1);
    assert_eq!(progress(&app, car).crossings, 1);
    // Drive back and forth through it again — nothing more counts.
    set_position(&mut app, car, Vec3::new(-10.0, 0.0, 0.0));
    run(&mut app, 1);
    set_position(&mut app, car, Vec3::new(10.0, 0.0, 0.0));
    run(&mut app, 1);
    assert_eq!(progress(&app, car).crossings, 1);
}

/// AC02 teleport case: `break_segment` on a jump means the skipped
/// checkpoints are not consumed — the explicit teleport path.
#[test]
fn teleport_breaks_the_swept_segment() {
    let def = RaceDefinition {
        checkpoints: vec![cp(0.0, 0.0), cp(50.0, 0.0)],
        finish: None,
        rule: CheckpointRule::Ordered,
        laps: 1,
        countdown_ticks: 0,
        start_slots: Vec::new(),
    };
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-100.0, 0.0, 0.0));
    run(&mut app, 2);
    // Teleport past both triggers with the segment broken.
    app.world_mut()
        .get_mut::<RaceProgress>(car)
        .unwrap()
        .break_segment();
    set_position(&mut app, car, Vec3::new(200.0, 0.0, 0.0));
    run(&mut app, 2);
    assert_eq!(
        progress(&app, car).cleared_count(),
        0,
        "the teleport must not consume the checkpoints it skipped"
    );
    // Driving back over checkpoint 0 clears it normally.
    set_position(&mut app, car, Vec3::new(-10.0, 0.0, 0.0));
    run(&mut app, 1);
    assert!(progress(&app, car).is_cleared(0));
}

/// AC04: finishing emits exactly one result per participant per
/// generation, carrying event + participant + outcome provenance.
#[test]
fn finish_records_one_result_with_provenance() {
    let def = any_order_def(0);
    let mut app = race_app(event_config(), def.clone());
    let (car, pid) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    run(&mut app, 2);
    set_position(&mut app, car, Vec3::new(200.0, 0.0, 0.0));
    run(&mut app, 1);
    run(&mut app, 6); // well past the finish — nothing re-records
    let ledger = app.world().resource::<ResultLedger>();
    assert_eq!(ledger.len(), 1, "one result, once");
    let ParticipantState::Finished { race_ticks, result } = &progress(&app, car).state else {
        panic!("participant should be finished")
    };
    assert!(*race_ticks > 0);
    assert_eq!(result.participant, pid);
    assert_eq!(result.generation, 1);
    assert_eq!(
        result.event.as_ref().map(|e| e.index),
        Some(0),
        "result identity carries the event"
    );
    assert_eq!(race(&app).phase, RacePhase::Complete);
}

/// Two participants finishing on the same step both record — ties are
/// distinguished by participant + sequence, not collapsed (AC04).
#[test]
fn tied_finishes_record_separately() {
    let def = any_order_def(0);
    let mut app = race_app(event_config(), def.clone());
    let (a, pa) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, -1.0));
    let (b, pb) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 1.0));
    run(&mut app, 2);
    set_position(&mut app, a, Vec3::new(200.0, 0.0, -1.0));
    set_position(&mut app, b, Vec3::new(200.0, 0.0, 1.0));
    run(&mut app, 2);
    assert_eq!(app.world().resource::<ResultLedger>().len(), 2);
    let (ra, rb) = (
        match &progress(&app, a).state {
            ParticipantState::Finished { result, .. } => result.clone(),
            _ => panic!("a not finished"),
        },
        match &progress(&app, b).state {
            ParticipantState::Finished { result, .. } => result.clone(),
            _ => panic!("b not finished"),
        },
    );
    assert_ne!(ra, rb);
    assert_eq!(ra.participant, pa);
    assert_eq!(rb.participant, pb);
}

/// AC03 pause: the race clock and progress freeze with the session and
/// resume deterministically.
#[test]
fn pause_freezes_the_race() {
    let def = any_order_def(0);
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-100.0, 0.0, 0.0));
    run(&mut app, 3);
    let clock_at_pause = race(&app).clock;
    assert!(clock_at_pause > 0);
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    // Moving while paused produces no crossings and no clock.
    set_position(&mut app, car, Vec3::new(200.0, 0.0, 0.0));
    run(&mut app, 4);
    assert_eq!(race(&app).clock, clock_at_pause, "paused clock froze");
    assert_eq!(progress(&app, car).cleared_count(), 0);
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Playing)
        .unwrap();
    run(&mut app, 1);
    assert!(race(&app).clock > clock_at_pause, "the clock resumed");
}

/// AC03 restart: teardown removes the `RaceState` — no old timer
/// survives into the next session.
#[test]
fn restart_removes_the_race_resource() {
    let def = any_order_def(0);
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));
    run(&mut app, 3);
    assert_eq!(race(&app).phase, RacePhase::Running);

    app.world_mut().resource_mut::<SessionControl>().restart = true;
    let mut reached = false;
    for _ in 0..12 {
        app.update();
        if phase(&app) == SessionPhase::Loading {
            reached = true;
            break;
        }
    }
    assert!(reached, "restart never re-began the session");
    assert_eq!(app.world().resource::<Session>().generation(), 2);
    assert!(
        app.world().get_resource::<RaceState>().is_none(),
        "the old race timer must not survive the restart"
    );
    assert!(
        app.world().get_entity(car).is_err(),
        "session-owned participants despawned"
    );
    // A stale race resource is never stepped: plant one stamped with
    // the dead generation and confirm it does not tick.
    let stale = RaceState::new(def, 1);
    app.world_mut().insert_resource(stale);
    run(&mut app, 3);
    assert_eq!(app.world().resource::<RaceState>().clock, 0);
}

/// Quit during the countdown tears the session down like any other —
/// `Countdown → Unloading → Menu` and an exit request.
#[test]
fn quit_during_countdown_unloads_to_menu() {
    let def = any_order_def(600);
    let mut app = race_app(event_config(), def.clone());
    spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));
    assert_eq!(phase(&app), SessionPhase::Countdown);
    app.world_mut().resource_mut::<SessionControl>().quit = true;
    let mut reached = false;
    for _ in 0..12 {
        app.update();
        if phase(&app) == SessionPhase::Menu {
            reached = true;
            break;
        }
    }
    assert!(reached, "countdown quit never reached Menu");
    assert!(
        app.world().get_resource::<RaceState>().is_none(),
        "teardown removed the race"
    );
}

/// Authority boundary: a `Remote` session's race state is replicated,
/// not simulated — the driver never steps it (F25+ fills replication).
#[test]
fn remote_authority_does_not_simulate() {
    let mut config = event_config();
    config.authority = SessionAuthority::Remote;
    let def = any_order_def(2);
    let mut app = race_app(config, def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));
    run(&mut app, 6);
    assert_eq!(
        race(&app).phase,
        RacePhase::Countdown { remaining: 2 },
        "a predicted session never ticks its own race"
    );
    assert_eq!(phase(&app), SessionPhase::Countdown);
    assert_eq!(drain_started(&mut app), 0);
    assert_eq!(progress(&app, car).state, ParticipantState::AwaitingStart);
}

/// `advance` is the pure core the driver calls — pinned here so the
/// system-level tests above provably exercise the same code path.
#[test]
fn progress_advance_is_the_shared_step() {
    let def = any_order_def(0);
    let mut p = RaceProgress::new(&def);
    p.advance(&def, Vec3::new(-200.0, 0.0, 0.0));
    assert_eq!(
        p.advance(&def, Vec3::new(200.0, 0.0, 0.0)),
        ProgressOutcome::Finished
    );
}
