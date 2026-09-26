//! F11-B race-runtime integration: countdown, swept triggers, progress
//! and once-only results through the production `advance_race` system —
//! the same trigger path gameplay drives, with deterministic `Position`
//! writes standing in for the physics-produced segments (AC02–AC04).

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::race::{
    COUNTDOWN_GO_TICKS, CountdownBanner, CountdownBannerText, LOW_TIME_BRIGHT, LOW_TIME_DIM,
    LOW_TIME_TICKS, LowTimeWarning, NAV_AHEAD, NAV_BEHIND, NavArrow, advance_race,
    nav_target_input, reanchor_teleported_participants, spawn_countdown_banner, spawn_nav_arrow,
    spawn_race_warning, update_countdown_banner, update_nav_arrow, update_race_warning,
};
use mm2_app::session::{self, SessionControl};
use mm2_game::{
    Checkpoint, CheckpointRule, EventRef, EventTableKind, ParticipantState, Player, PlayerControl,
    PlayerId, ProgressOutcome, RaceDefinition, RacePhase, RaceProgress, RaceStart, RaceStarted,
    RaceState, ResultLedger, Session, SessionAuthority, SessionConfig, SessionEntity, SessionMode,
    SessionOutcome, SessionPhase, TargetSelection, advance_session_tick, despawn_session_entities,
    live_order,
};
use mm2_vehicle::{ResetVehicle, Teleported, Vehicle, VehicleConfig, VehicleState};

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
        time_limit_ticks: None,
        params: mm2_game::EventParams::default(),
        countdown_ticks: countdown,
        start_slots: vec![RaceStart {
            position: Vec3::new(-50.0, 0.0, 0.0),
            yaw_deg: Some(0.0),
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
        .init_resource::<mm2_app::hud::HudVisible>()
        .init_resource::<mm2_app::contracts::ImpactFilter>()
        .init_resource::<mm2_app::damage::DamageReport>()
        .init_resource::<mm2_app::stuck::StuckReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<mm2_app::recovery::RecoveryReport>()
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<mm2_app::spark_fx::SparkFxReport>()
        .init_resource::<mm2_app::texel_fx::TexelDamageReport>()
        .init_resource::<ButtonInput<KeyCode>>()
        .add_message::<RaceStarted>()
        .add_message::<ResetVehicle>()
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (reanchor_teleported_participants, advance_race).chain(),
        )
        .add_systems(
            Update,
            (
                mm2_vehicle::systems::vehicle_reset,
                session::session_control_input,
                nav_target_input,
                update_nav_arrow,
                update_race_warning,
                update_countdown_banner,
                (
                    despawn_session_entities.run_if(session::unloading),
                    session::drive_session,
                )
                    .chain(),
            ),
        )
        // The production spawn path stamps the needle with the owning
        // session generation — do the same so teardown tests exercise it.
        .add_systems(Startup, |mut commands: Commands, session: Res<Session>| {
            let owner = SessionEntity(session.generation());
            spawn_nav_arrow(&mut commands, owner);
            spawn_race_warning(&mut commands, owner);
            spawn_countdown_banner(&mut commands, owner);
        });
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
/// session's ownership generation like every session spawn. It also
/// carries the vehicle components `vehicle_reset`'s query needs, so
/// the production `ResetVehicle` teleport path works on it (no
/// `RigidBody` — physics leaves its `Position` alone between the
/// segment writes the tests make).
fn spawn_participant(app: &mut App, def: &RaceDefinition, pos: Vec3) -> (Entity, PlayerId) {
    let generation = app.world().resource::<Session>().generation();
    let id = app.world_mut().resource_mut::<Session>().mint_player_id();
    let cfg = VehicleConfig::default();
    let entity = app
        .world_mut()
        .spawn((
            SessionEntity(generation),
            Player {
                id,
                control: PlayerControl::Local,
            },
            RaceProgress::new(def),
            TargetSelection::default(),
            Vehicle {
                config: cfg.clone(),
            },
            VehicleState::new(&cfg),
            Position(pos),
            Rotation::default(),
            LinearVelocity::ZERO,
            AngularVelocity::ZERO,
            Transform::from_translation(pos),
        ))
        .id();
    (entity, id)
}

fn set_position(app: &mut App, entity: Entity, pos: Vec3) {
    *app.world_mut().get_mut::<Position>(entity).unwrap() = Position(pos);
    // Keep `Transform` in step like every real mover does (`vehicle_reset`
    // writes both, the solver syncs both): participants now carry
    // `Rotation`, so avian's `transform_to_position` would otherwise copy
    // the stale `GlobalTransform` back over `Position` a step later.
    app.world_mut()
        .get_mut::<Transform>(entity)
        .unwrap()
        .translation = pos;
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
        time_limit_ticks: None,
        params: mm2_game::EventParams::default(),
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

/// AC02 reset leg, production path: a `ResetVehicle` teleport — the
/// R-key reset's real route through `vehicle_reset` — marks the entity
/// `Teleported`, and `reanchor_teleported_participants` breaks the
/// swept segment so the jump cannot consume checkpoints or mint a
/// finish it physically skipped.
#[test]
fn vehicle_reset_breaks_the_swept_segment() {
    let def = RaceDefinition {
        checkpoints: vec![cp(0.0, 0.0), cp(50.0, 0.0)],
        finish: None,
        rule: CheckpointRule::Ordered,
        laps: 1,
        time_limit_ticks: None,
        params: mm2_game::EventParams::default(),
        countdown_ticks: 0,
        start_slots: Vec::new(),
    };
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-100.0, 0.0, 0.0));
    run(&mut app, 2); // release + anchor at -100

    // The production teleport path: message → vehicle_reset → marker.
    // One update applies the teleport, the next consumes the marker and
    // re-anchors at the spawn point — the jump itself sweeps nothing.
    app.world_mut().write_message(ResetVehicle {
        entity: Some(car),
        position: Vec3::new(200.0, 0.0, 0.0),
        yaw: 0.0,
    });
    run(&mut app, 2);

    assert_eq!(
        app.world().get::<Position>(car).unwrap().0,
        Vec3::new(200.0, 0.0, 0.0),
        "vehicle_reset applied the teleport"
    );
    let p = progress(&app, car);
    assert_eq!(
        p.cleared_count(),
        0,
        "the reset jump must not consume the checkpoints it skipped"
    );
    assert!(
        matches!(p.state, ParticipantState::Racing),
        "no finish minted from the jump"
    );
    assert_eq!(
        app.world().resource::<ResultLedger>().len(),
        0,
        "no result recorded"
    );
    assert!(
        app.world().get::<Teleported>(car).is_none(),
        "the marker was consumed"
    );
    // And the anchor re-established: driving back over the checkpoints
    // sweeps them legitimately.
    set_position(&mut app, car, Vec3::new(-10.0, 0.0, 0.0));
    run(&mut app, 1);
    assert!(progress(&app, car).is_cleared(0));
}

/// A reset while paused still lands: the `Teleported` marker persists
/// through the freeze, so resuming re-anchors instead of sweeping the
/// jump across the checkpoints.
#[test]
fn reset_while_paused_cannot_sweep_checkpoints() {
    let def = any_order_def(0);
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    run(&mut app, 2); // released + anchored

    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    app.world_mut().write_message(ResetVehicle {
        entity: Some(car),
        position: Vec3::new(200.0, 0.0, 0.0),
        yaw: 0.0,
    });
    run(&mut app, 3);
    assert_eq!(progress(&app, car).cleared_count(), 0);

    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Playing)
        .unwrap();
    run(&mut app, 3);
    let p = progress(&app, car);
    assert_eq!(
        p.cleared_count(),
        0,
        "the paused reset's jump must not count after resume"
    );
    assert!(matches!(p.state, ParticipantState::Racing));
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

/// F13-AC03/F14-AC03: two participants on one Ordered course lap
/// independently through the production driver — each wraps its own
/// `next`/`lap`, each finishes once, and the ledger's standings order
/// them by the recorded race clock (the remote's earlier finish
/// outranks the local's). The remote's resolution does not end the
/// local driver's race; the local's does (UI-5).
#[test]
fn ordered_multi_lap_participants_stay_independent_and_order() {
    let def = RaceDefinition {
        checkpoints: vec![cp(0.0, 0.0), cp(100.0, 0.0)],
        finish: None,
        rule: CheckpointRule::Ordered,
        laps: 2,
        time_limit_ticks: None,
        params: mm2_game::EventParams::default(),
        countdown_ticks: 0,
        start_slots: Vec::new(),
    };
    let mut app = race_app(event_config(), def.clone());
    let (remote, pr) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, -2.0));
    let (local, pl) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 2.0));
    app.world_mut().get_mut::<Player>(remote).unwrap().control = PlayerControl::Remote;
    run(&mut app, 2); // release + anchor

    // The drive path, per gate: enter the trigger, leave into the
    // middle zone (x in 15..85), then enter the next. One segment
    // clears one gate — resting inside a radius would let the next
    // segment clear the wrapped sequence too (the swept contract's
    // intended multi-crossing, pinned by other tests).
    let drive = |app: &mut App, e: Entity, z: f32, xs: &[f32]| {
        for &x in xs {
            set_position(app, e, Vec3::new(x, 0.0, z));
            run(app, 1);
        }
    };

    // Lap 1 for both, interleaved so each participant's progress is
    // provably its own: gate 0, out, gate 1.
    drive(&mut app, remote, -2.0, &[10.0]);
    drive(&mut app, local, 2.0, &[10.0]);
    assert_eq!(progress(&app, remote).next, 1);
    assert_eq!(progress(&app, local).next, 1);
    drive(&mut app, remote, -2.0, &[40.0, 90.0]);
    assert_eq!(progress(&app, remote).lap, 1, "remote finished lap 1");
    assert_eq!(progress(&app, remote).next, 0);
    assert_eq!(progress(&app, local).next, 1, "local still owes gate 1");
    assert_eq!(progress(&app, local).lap, 0);
    drive(&mut app, local, 2.0, &[40.0, 90.0]);
    assert_eq!(progress(&app, local).lap, 1);

    // The remote's lap 2 finishes first — its resolution is recorded
    // but the local still races, so the session stays Playing.
    drive(&mut app, remote, -2.0, &[50.0, 10.0]);
    assert_eq!(progress(&app, remote).next, 1, "lap-2 gate 0 cleared");
    drive(&mut app, remote, -2.0, &[40.0, 90.0]);
    assert!(
        matches!(
            progress(&app, remote).state,
            ParticipantState::Finished { .. }
        ),
        "the remote finished its second lap"
    );
    assert_eq!(race(&app).phase, RacePhase::Running);
    assert_eq!(phase(&app), SessionPhase::Playing);

    // The local completes lap 2 a few ticks later — one result each,
    // standings ordered by the recorded clock.
    drive(&mut app, local, 2.0, &[50.0, 10.0, 40.0, 90.0]);
    assert!(matches!(
        progress(&app, local).state,
        ParticipantState::Finished { .. }
    ));
    assert_eq!(phase(&app), SessionPhase::Results);
    assert_eq!(race(&app).phase, RacePhase::Complete);

    let ledger = app.world().resource::<ResultLedger>();
    assert_eq!(ledger.len(), 2, "one result per participant");
    let order: Vec<PlayerId> = ledger
        .standings()
        .iter()
        .map(|r| r.id.participant)
        .collect();
    assert_eq!(order, vec![pr, pl], "earlier finish outranks");
    assert_eq!(ledger.place_of(pr), Some(1));
    assert_eq!(ledger.place_of(pl), Some(2));
}

/// A small closed Ordered course in the shape `gates_for` produces for
/// a retail circuit: course gates in authored order at x = 0 and
/// x = 100, then the lifted start-line copy closing each lap at
/// x = 200 (WPT-2).
fn ordered_def(countdown: u32, laps: u32) -> RaceDefinition {
    RaceDefinition {
        checkpoints: vec![cp(0.0, 0.0), cp(100.0, 0.0), cp(200.0, 0.0)],
        finish: None,
        rule: CheckpointRule::Ordered,
        laps,
        time_limit_ticks: None,
        params: mm2_game::EventParams::default(),
        countdown_ticks: countdown,
        start_slots: Vec::new(),
    }
}

/// F14-AC02 Ordered missed-gate leg (spec edge "skipped gate"): a
/// swept crossing only counts against the gate the sequence is
/// waiting on — driving through a later gate while an earlier one is
/// outstanding banks nothing, and the skipped gate still has to be
/// visited before the lap can complete.
#[test]
fn ordered_skipped_gate_clears_nothing_until_revisited_in_order() {
    let def = ordered_def(0, 1);
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));
    run(&mut app, 2); // release + anchor
    let drive = |app: &mut App, to: Vec3| {
        set_position(app, car, to);
        run(app, 1);
    };

    // Bypass gate 0 on the z=50 lane and sweep straight through
    // gate 1's cylinder — it is not `next`, so nothing banks.
    drive(&mut app, Vec3::new(-50.0, 0.0, 50.0));
    drive(&mut app, Vec3::new(45.0, 0.0, 50.0));
    drive(&mut app, Vec3::new(45.0, 0.0, 0.0));
    drive(&mut app, Vec3::new(95.0, 0.0, 0.0));
    let p = progress(&app, car);
    assert_eq!(p.next, 0, "gate 1 cannot clear while gate 0 is owed");
    assert_eq!(p.cleared_count(), 0, "the skipped crossing must not bank");
    assert_eq!(
        p.crossings, 0,
        "a crossing against a non-required gate is not even a crossing"
    );

    // Gate 0 clears in order; the earlier drive-through was not
    // banked, so gate 1 still has to be visited.
    drive(&mut app, Vec3::new(45.0, 0.0, 0.0));
    drive(&mut app, Vec3::new(-10.0, 0.0, 0.0));
    assert_eq!(progress(&app, car).next, 1);
    drive(&mut app, Vec3::new(45.0, 0.0, 0.0));
    drive(&mut app, Vec3::new(95.0, 0.0, 0.0));
    assert_eq!(progress(&app, car).next, 2, "gate 1 needed a second visit");

    // The closing line completes the lap — once, with exactly one
    // result.
    drive(&mut app, Vec3::new(215.0, 0.0, 0.0));
    assert!(matches!(
        progress(&app, car).state,
        ParticipantState::Finished { .. }
    ));
    assert_eq!(progress(&app, car).crossings, 3);
    assert_eq!(app.world().resource::<ResultLedger>().len(), 1);
}

/// F14-AC02's "repeated finish hits" and "backward" legs under
/// `Ordered`: the start line is each lap's *last* gate, so crossing it
/// — repeatedly, in either direction — while course gates are still
/// owed banks nothing.
#[test]
fn ordered_finish_line_is_inert_until_it_is_next() {
    let def = ordered_def(0, 2);
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));
    run(&mut app, 2); // release + anchor
    let drive = |app: &mut App, to: Vec3| {
        set_position(app, car, to);
        run(app, 1);
    };

    // Reach past the line on the bypass lane, then hammer it back and
    // forth: it is not `next`, so every sweep is inert.
    for to in [
        Vec3::new(-50.0, 0.0, 50.0),
        Vec3::new(230.0, 0.0, 50.0),
        Vec3::new(230.0, 0.0, 0.0),
        Vec3::new(170.0, 0.0, 0.0),
        Vec3::new(230.0, 0.0, 0.0),
        Vec3::new(170.0, 0.0, 0.0),
    ] {
        drive(&mut app, to);
    }
    let p = progress(&app, car);
    assert_eq!(p.lap, 0, "an early line crossing cannot bank a lap");
    assert_eq!(p.cleared_count(), 0);
    assert_eq!(p.crossings, 0);
    assert_eq!(p.next, 0);
    assert_eq!(p.state, ParticipantState::Racing);

    // Running the course properly banks the lap — once — and the lap
    // counter cannot wind backward through the line.
    let lap = |app: &mut App| {
        for to in [
            Vec3::new(230.0, 0.0, 50.0),
            Vec3::new(45.0, 0.0, 50.0),
            Vec3::new(45.0, 0.0, 0.0),
            Vec3::new(-10.0, 0.0, 0.0),
            Vec3::new(45.0, 0.0, 0.0),
            Vec3::new(95.0, 0.0, 0.0),
            Vec3::new(215.0, 0.0, 0.0),
        ] {
            drive(app, to);
        }
    };
    lap(&mut app);
    assert_eq!(progress(&app, car).lap, 1, "lap 1 banks on the line");
    assert_eq!(progress(&app, car).next, 0, "the sequence re-wrapped");
    lap(&mut app);
    assert!(matches!(
        progress(&app, car).state,
        ParticipantState::Finished { .. }
    ));
    assert_eq!(app.world().resource::<ResultLedger>().len(), 1);
}

/// Spec edge "finish-line spawn": a participant staged *inside* the
/// closing gate's cylinder — exactly where a circuit grid sits —
/// banks nothing at release or while moving within the volume, and
/// the lap still requires every course gate in order.
#[test]
fn ordered_spawn_inside_the_line_grants_nothing() {
    let def = ordered_def(0, 1);
    let mut app = race_app(event_config(), def.clone());
    // Staged dead centre on the start/finish line: inside the last
    // gate's trigger from the first tick.
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(200.0, 0.0, 0.0));
    run(&mut app, 2); // release + anchor inside the cylinder
    let drive = |app: &mut App, to: Vec3| {
        set_position(app, car, to);
        run(app, 1);
    };

    // Moving around inside the closing gate's volume while it is not
    // `next` banks nothing — no lap from the spawn.
    for to in [
        Vec3::new(190.0, 0.0, 0.0),
        Vec3::new(210.0, 0.0, 0.0),
        Vec3::new(200.0, 0.0, 0.0),
    ] {
        drive(&mut app, to);
    }
    let p = progress(&app, car);
    assert_eq!(p.lap, 0);
    assert_eq!(p.cleared_count(), 0);
    assert_eq!(p.crossings, 0, "dwelling in the line is not a crossing");

    // Leave, run the course gates, and the return trip across the line
    // banks the lap exactly once.
    for to in [
        Vec3::new(230.0, 0.0, 50.0),
        Vec3::new(45.0, 0.0, 50.0),
        Vec3::new(45.0, 0.0, 0.0),
        Vec3::new(-10.0, 0.0, 0.0),
        Vec3::new(45.0, 0.0, 0.0),
        Vec3::new(95.0, 0.0, 0.0),
        Vec3::new(205.0, 0.0, 0.0),
    ] {
        drive(&mut app, to);
    }
    assert!(matches!(
        progress(&app, car).state,
        ParticipantState::Finished { .. }
    ));
    assert_eq!(progress(&app, car).lap, 1);
    assert_eq!(app.world().resource::<ResultLedger>().len(), 1);
}

/// Spec edge "overlapping start/finish volumes": where the closing
/// line's cylinder overlaps the last course gate's, one segment
/// through the overlap banks the lap *once* — the wrap consumes both
/// gates in order and cannot count the same crossing twice.
#[test]
fn ordered_overlapping_closing_gate_banks_one_lap_once() {
    // The line's cylinder overlaps the last course gate's — 8 m apart
    // with 15 m radii.
    let mut def = ordered_def(0, 1);
    def.checkpoints[2] = cp(108.0, 0.0);
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));
    run(&mut app, 2); // release + anchor
    let drive = |app: &mut App, to: Vec3| {
        set_position(app, car, to);
        run(app, 1);
    };

    drive(&mut app, Vec3::new(10.0, 0.0, 0.0));
    assert_eq!(progress(&app, car).next, 1);
    // One segment sweeps gate 1 and the overlapping line in order —
    // physically crossing both is a genuine traversal, and the lap
    // banks exactly once off it.
    drive(&mut app, Vec3::new(140.0, 0.0, 0.0));
    let p = progress(&app, car);
    assert_eq!(p.lap, 1, "the overlap banks one lap, not two");
    assert_eq!(p.crossings, 3, "three gates cleared, three crossings");
    assert!(matches!(p.state, ParticipantState::Finished { .. }));
    // Re-sweeping the overlap after resolving cannot mint again.
    drive(&mut app, Vec3::new(10.0, 0.0, 0.0));
    drive(&mut app, Vec3::new(140.0, 0.0, 0.0));
    assert_eq!(app.world().resource::<ResultLedger>().len(), 1);
}

/// Spec edge "last-lap tie" (F14-AC03): two participants crossing the
/// line on the same tick of their final lap both record — the
/// standings break the shared race clock by `PlayerId`, so the result
/// order is deterministic rather than query order.
#[test]
fn ordered_last_lap_tie_records_both_deterministically() {
    let def = ordered_def(0, 1);
    let mut app = race_app(event_config(), def.clone());
    let (a, pa) = spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, -2.0));
    let (b, pb) = spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 2.0));
    app.world_mut().get_mut::<Player>(b).unwrap().control = PlayerControl::Remote;
    run(&mut app, 2); // release + anchor
    let drive = |app: &mut App, e: Entity, z: f32, to: Vec3| {
        set_position(app, e, Vec3::new(to.x, to.y, z));
        run(app, 1);
    };

    // Both clear the course gates on parallel lanes, then cross the
    // line on the same update.
    drive(&mut app, a, -2.0, Vec3::new(10.0, 0.0, 0.0));
    drive(&mut app, a, -2.0, Vec3::new(90.0, 0.0, 0.0));
    drive(&mut app, b, 2.0, Vec3::new(10.0, 0.0, 0.0));
    drive(&mut app, b, 2.0, Vec3::new(90.0, 0.0, 0.0));
    drive(&mut app, a, -2.0, Vec3::new(150.0, 0.0, 0.0));
    drive(&mut app, b, 2.0, Vec3::new(150.0, 0.0, 0.0));
    set_position(&mut app, a, Vec3::new(215.0, 0.0, -2.0));
    set_position(&mut app, b, Vec3::new(215.0, 0.0, 2.0));
    run(&mut app, 1);

    let (ta, tb) = (
        match &progress(&app, a).state {
            ParticipantState::Finished { race_ticks, .. } => *race_ticks,
            _ => panic!("a not finished"),
        },
        match &progress(&app, b).state {
            ParticipantState::Finished { race_ticks, .. } => *race_ticks,
            _ => panic!("b not finished"),
        },
    );
    assert_eq!(ta, tb, "a genuine last-lap tie shares the race clock");
    assert_eq!(race(&app).phase, RacePhase::Complete);
    let ledger = app.world().resource::<ResultLedger>();
    assert_eq!(ledger.len(), 2, "both tied finishes record");
    let order: Vec<PlayerId> = ledger
        .standings()
        .iter()
        .map(|r| r.id.participant)
        .collect();
    let mut expected = vec![pa, pb];
    expected.sort();
    assert_eq!(order, expected, "the tie resolves by participant id");
}

/// Spec edge "reset on finish" (F14-AC02): clearing every course gate
/// and then resetting *past* the closing line sweeps its cylinder
/// through the production `ResetVehicle` jump — which banks nothing,
/// and the line still has to be physically re-crossed.
#[test]
fn ordered_reset_over_the_line_still_owes_the_crossing() {
    let def = ordered_def(0, 1);
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));
    run(&mut app, 2); // release + anchor
    let drive = |app: &mut App, to: Vec3| {
        set_position(app, car, to);
        run(app, 1);
    };

    drive(&mut app, Vec3::new(10.0, 0.0, 0.0));
    drive(&mut app, Vec3::new(90.0, 0.0, 0.0));
    assert_eq!(progress(&app, car).next, 2, "only the line is owed");

    // The reset's jump sweeps the closing gate's cylinder on the way
    // to 400 — the broken segment must not bank the lap.
    app.world_mut().write_message(ResetVehicle {
        entity: Some(car),
        position: Vec3::new(400.0, 0.0, 0.0),
        yaw: 0.0,
    });
    run(&mut app, 3);
    let p = progress(&app, car);
    assert_eq!(p.lap, 0, "the reset jump cannot bank the finish");
    assert_eq!(p.next, 2, "the line is still owed");
    assert!(matches!(p.state, ParticipantState::Racing));
    assert_eq!(app.world().resource::<ResultLedger>().len(), 0);

    // Driving back across it completes the lap — once.
    drive(&mut app, Vec3::new(300.0, 0.0, 0.0));
    drive(&mut app, Vec3::new(210.0, 0.0, 0.0));
    assert!(matches!(
        progress(&app, car).state,
        ParticipantState::Finished { .. }
    ));
    assert_eq!(app.world().resource::<ResultLedger>().len(), 1);
}

/// Spec edge "DNF participant" plus requirement 4's bounded result
/// handling: an opponent that never resolves does not hold the race
/// hostage — the local finish still ends the session with exactly the
/// local result banked and the drifter unplaced.
#[test]
fn an_unresolved_participant_does_not_block_the_local_result() {
    let def = ordered_def(0, 1);
    let mut app = race_app(event_config(), def.clone());
    // An opponent parked off the course — it never earns a gate.
    let (stuck, ps) = spawn_participant(&mut app, &def, Vec3::new(600.0, 0.0, 600.0));
    app.world_mut().get_mut::<Player>(stuck).unwrap().control = PlayerControl::Remote;
    let (car, pl) = spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));
    run(&mut app, 2); // release + anchor
    let drive = |app: &mut App, to: Vec3| {
        set_position(app, car, to);
        run(app, 1);
    };

    for to in [
        Vec3::new(10.0, 0.0, 0.0),
        Vec3::new(90.0, 0.0, 0.0),
        Vec3::new(215.0, 0.0, 0.0),
    ] {
        drive(&mut app, to);
    }
    assert!(matches!(
        progress(&app, car).state,
        ParticipantState::Finished { .. }
    ));
    assert_eq!(
        phase(&app),
        SessionPhase::Results,
        "the local finish resolves the session"
    );
    // The race itself stays Running for the unresolved field — but
    // the ledger already holds the bounded outcome.
    assert_eq!(race(&app).phase, RacePhase::Running);
    assert_eq!(progress(&app, stuck).state, ParticipantState::Racing);
    let ledger = app.world().resource::<ResultLedger>();
    assert_eq!(ledger.len(), 1, "only the resolved participant records");
    assert_eq!(ledger.place_of_in(1, pl), Some(1));
    assert_eq!(ledger.place_of_in(1, ps), None, "the drifter is unplaced");
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
    // The driver only advances `Racing` participants — the contract
    // itself is inert outside that state.
    p.state = ParticipantState::Racing;
    p.advance(&def, Vec3::new(-200.0, 0.0, 0.0));
    assert_eq!(
        p.advance(&def, Vec3::new(200.0, 0.0, 0.0)),
        ProgressOutcome::Finished
    );
}

/// A definition with a Blitz-style deadline (`time_limit_ticks`, F12-A).
fn timed_def(countdown: u32, limit: u32) -> RaceDefinition {
    let mut def = any_order_def(countdown);
    def.time_limit_ticks = Some(limit);
    def
}

/// F12-AC03/AC04: the deadline expires a race that never finished —
/// one `TimedOut` result on the authoritative race clock, recorded
/// once, then the race completes and nothing refires.
#[test]
fn timeout_records_one_timed_out_result_and_completes() {
    let def = timed_def(0, 40);
    let mut app = race_app(event_config(), def.clone());
    let (car, pid) = spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));
    assert_eq!(race(&app).time_remaining(), Some(40));
    run(&mut app, 30); // clock: 2 ticks per update → 40 hit at update 21

    let ParticipantState::TimedOut { race_ticks, result } = &progress(&app, car).state else {
        panic!("expected TimedOut, got {:?}", progress(&app, car).state)
    };
    assert_eq!(*race_ticks, 40, "expiry lands on the limit's tick");
    assert_eq!(result.participant, pid);
    let ledger = app.world().resource::<ResultLedger>();
    let rec = ledger.get(result).expect("the result is retained");
    assert!(
        matches!(rec.outcome, SessionOutcome::TimedOut { race_ticks: 40 }),
        "the ledger records the outcome, not just the id: {:?}",
        rec.outcome
    );
    assert_eq!(race(&app).phase, RacePhase::Complete);
    assert_eq!(race(&app).time_remaining(), Some(0));

    run(&mut app, 6);
    assert_eq!(
        app.world().resource::<ResultLedger>().len(),
        1,
        "expiry records once and does not refire"
    );
}

/// F12-AC03 boundary rule (DSN-7): the deadline is inclusive — a
/// crossing that lands on the tick the clock reaches the limit wins,
/// because segments evaluate before the timeout check.
#[test]
fn finish_on_the_expiry_tick_still_counts() {
    let def = timed_def(0, 20);
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    run(&mut app, 3); // released, racing and anchored
    assert_eq!(progress(&app, car).state, ParticipantState::Racing);
    // Stand the clock one tick short of the deadline, then drive the
    // finish segment on the tick the clock reaches it.
    app.world_mut().resource_mut::<RaceState>().clock = 19;
    set_position(&mut app, car, Vec3::new(200.0, 0.0, 0.0));
    run(&mut app, 1); // the next step is clock 20 — exactly the limit

    let ParticipantState::Finished { race_ticks, .. } = &progress(&app, car).state else {
        panic!(
            "a finish on the deadline tick must count, got {:?}",
            progress(&app, car).state
        )
    };
    assert_eq!(*race_ticks, 20);
    let ledger = app.world().resource::<ResultLedger>();
    assert_eq!(ledger.len(), 1);
    assert!(
        matches!(
            ledger.iter().next().unwrap().outcome,
            SessionOutcome::Finished { race_ticks: 20 }
        ),
        "the one result is the finish, not a timeout"
    );
}

/// A participant still `AwaitingStart` while the race runs (a joiner
/// that bypassed `join`) cannot hold the race open past the deadline —
/// it times out like everyone racing.
#[test]
fn timeout_resolves_an_unreleased_participant() {
    let def = timed_def(0, 10);
    let mut app = race_app(event_config(), def.clone());
    spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));
    run(&mut app, 2); // released; clock 3
    // A second entity joins after release with a fresh progress — it
    // sits AwaitingStart inside a Running race.
    let (late, _) = spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));
    assert_eq!(progress(&app, late).state, ParticipantState::AwaitingStart);
    run(&mut app, 5); // clock 3 → 13, past the limit

    assert!(
        matches!(
            progress(&app, late).state,
            ParticipantState::TimedOut { race_ticks: 10, .. }
        ),
        "the unreleased participant timed out on the deadline tick: {:?}",
        progress(&app, late).state
    );
    assert_eq!(app.world().resource::<ResultLedger>().len(), 2);
    assert_eq!(race(&app).phase, RacePhase::Complete);
}

/// UI-5's results flow through the production driver: the local
/// driver's finish moves the session `Playing → Results` on the same
/// step — the race freezes with the phase change and the result is
/// already recorded once (F13-A: the finish is terminal, not just a
/// checkpoint count).
#[test]
fn local_finish_moves_the_session_to_results() {
    let def = any_order_def(0);
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    run(&mut app, 2);
    set_position(&mut app, car, Vec3::new(200.0, 0.0, 0.0));
    run(&mut app, 1);
    assert!(matches!(
        progress(&app, car).state,
        ParticipantState::Finished { .. }
    ));
    assert_eq!(race(&app).phase, RacePhase::Complete);
    assert_eq!(
        phase(&app),
        SessionPhase::Results,
        "the local driver's finish ends the playing session (UI-5)"
    );
    run(&mut app, 4);
    assert_eq!(phase(&app), SessionPhase::Results);
    assert_eq!(
        app.world().resource::<ResultLedger>().len(),
        1,
        "the result stays recorded once — nothing re-fires"
    );
}

/// A remote/AI participant resolving while the local driver still
/// races must not end the local session — only a *local* terminal
/// resolution moves `Playing → Results` (AC03: one participant's
/// trigger cannot update everyone else's race).
#[test]
fn a_non_local_resolution_does_not_end_the_local_race() {
    let def = any_order_def(0);
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    let (remote, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 1.0));
    app.world_mut().get_mut::<Player>(remote).unwrap().control = PlayerControl::Remote;
    run(&mut app, 2);

    // The remote sweeps both gates and finishes while the local car
    // is still racing — the session and race stay open.
    set_position(&mut app, remote, Vec3::new(200.0, 0.0, 1.0));
    run(&mut app, 1);
    assert!(matches!(
        progress(&app, remote).state,
        ParticipantState::Finished { .. }
    ));
    assert_eq!(
        phase(&app),
        SessionPhase::Playing,
        "a remote finish must not end the local driver's race"
    );
    assert_eq!(race(&app).phase, RacePhase::Running);

    // The local driver's own finish still resolves the session — and
    // completes the race now that everyone has recorded a result.
    set_position(&mut app, car, Vec3::new(200.0, 0.0, 0.0));
    run(&mut app, 1);
    assert_eq!(phase(&app), SessionPhase::Results);
    assert_eq!(race(&app).phase, RacePhase::Complete);
    assert_eq!(app.world().resource::<ResultLedger>().len(), 2);
}

/// The deadline's `TimedOut` is terminal for the local session too —
/// an expired race lands in `Results`, not just `RacePhase::Complete`
/// (UI-5 covers failure as well as finishes).
#[test]
fn local_timeout_moves_the_session_to_results() {
    let def = timed_def(0, 10);
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));
    run(&mut app, 8); // clock ≈2 ticks/update → expiry inside update 6
    assert!(matches!(
        progress(&app, car).state,
        ParticipantState::TimedOut { .. }
    ));
    assert_eq!(race(&app).phase, RacePhase::Complete);
    assert_eq!(phase(&app), SessionPhase::Results);
}

/// `Results` is a live quittable phase, not a dead end: a restart
/// intent unloads and re-begins exactly like from `Playing`.
#[test]
fn restart_from_results_rebegins_the_session() {
    let def = any_order_def(0);
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    run(&mut app, 2);
    set_position(&mut app, car, Vec3::new(200.0, 0.0, 0.0));
    run(&mut app, 1);
    assert_eq!(phase(&app), SessionPhase::Results);

    app.world_mut().resource_mut::<SessionControl>().restart = true;
    let mut reached = false;
    for _ in 0..12 {
        app.update();
        if phase(&app) == SessionPhase::Loading {
            reached = true;
            break;
        }
    }
    assert!(reached, "restart from Results never re-began the session");
    assert_eq!(app.world().resource::<Session>().generation(), 2);
    assert!(
        app.world().get_resource::<RaceState>().is_none(),
        "the old race timer must not survive the restart"
    );
}

/// The one session-owned needle, read back as
/// `(rotation, visibility, color)`.
fn arrow(app: &mut App) -> (Rot2, Visibility, Color) {
    let entity = {
        let world = app.world_mut();
        world
            .query_filtered::<Entity, With<NavArrow>>()
            .iter(world)
            .next()
            .expect("the harness spawns the nav arrow")
    };
    (
        app.world().get::<UiTransform>(entity).unwrap().rotation,
        *app.world().get::<Visibility>(entity).unwrap(),
        app.world().get::<BackgroundColor>(entity).unwrap().0,
    )
}

/// Simulate one key press through `ButtonInput` the way winit would
/// deliver it; `clear` after the consuming update ends the press the
/// way the input plugin would (MinimalPlugins installs none).
fn press(app: &mut App, key: KeyCode) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(key);
}

fn end_press(app: &mut App, key: KeyCode) {
    let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
    keys.release(key);
    keys.clear();
}

/// RACE-6 through the production systems: the needle tracks the
/// nearest un-cleared gate, rotates to the signed bearing, and turns
/// yellow when the target is behind the car.
#[test]
fn arrow_tracks_the_live_objective() {
    // Gates on the Z axis: gate 0 dead ahead (−Z), gate 1 behind.
    let def = RaceDefinition {
        checkpoints: vec![cp(0.0, -100.0), cp(0.0, 100.0)],
        ..any_order_def(0)
    };
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(0.0, 0.0, 0.0));
    run(&mut app, 3);

    let (rot, vis, color) = arrow(&mut app);
    assert_eq!(vis, Visibility::Visible);
    assert!(
        rot.as_radians().abs() < 1e-3,
        "gate 0 is dead ahead — needle up: {}",
        rot.as_radians()
    );
    assert_eq!(color, NAV_AHEAD, "ahead target is green");

    // Sweep gate 0: the arrow retargets to gate 1, which now sits
    // dead behind — yellow needle pointing back (RACE-6).
    set_position(&mut app, car, Vec3::new(0.0, 0.0, -200.0));
    run(&mut app, 2);
    assert!(progress(&app, car).is_cleared(0));
    let (rot, vis, color) = arrow(&mut app);
    assert_eq!(vis, Visibility::Visible);
    assert_eq!(color, NAV_BEHIND, "the remaining gate is behind");
    assert!(
        rot.as_radians().abs() > std::f32::consts::FRAC_PI_2,
        "needle points back: {}",
        rot.as_radians()
    );
}

/// RACE-6 cycling through the real input path: `X` steps the pick
/// forward and `Z` backward through the remaining gates — edge
/// triggered, so a held key does not keep cycling.
#[test]
fn arrow_pick_cycles_through_input() {
    let def = RaceDefinition {
        checkpoints: vec![cp(-100.0, 0.0), cp(0.0, 0.0), cp(100.0, 0.0)],
        finish: None,
        ..any_order_def(0)
    };
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(0.0, 0.0, -50.0));
    run(&mut app, 3);
    let picked = |app: &App| app.world().get::<TargetSelection>(car).unwrap().picked;
    assert_eq!(picked(&app), None, "no pick until the player chooses");

    // Nearest to (0,-50) is gate 1 — the first press moves off it.
    press(&mut app, KeyCode::KeyX);
    run(&mut app, 1);
    assert_eq!(picked(&app), Some(2));
    end_press(&mut app, KeyCode::KeyX);
    run(&mut app, 2);
    assert_eq!(picked(&app), Some(2), "a held state does not re-cycle");

    press(&mut app, KeyCode::KeyX);
    run(&mut app, 1);
    end_press(&mut app, KeyCode::KeyX);
    assert_eq!(picked(&app), Some(0), "forward cycling wraps");
    press(&mut app, KeyCode::KeyZ);
    run(&mut app, 1);
    end_press(&mut app, KeyCode::KeyZ);
    assert_eq!(picked(&app), Some(2), "Z steps backward");

    // A complete race stops listening — the pick can no longer move.
    app.world_mut().resource_mut::<RaceState>().phase = RacePhase::Complete;
    press(&mut app, KeyCode::KeyX);
    run(&mut app, 1);
    end_press(&mut app, KeyCode::KeyX);
    assert_eq!(picked(&app), Some(2), "a dead race ignores cycling");
}

/// The needle is already live while the race counts down — the
/// original's arrow works before the start too (RACE-6 names no
/// phase gate).
#[test]
fn arrow_is_live_during_countdown() {
    let def = any_order_def(600);
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    run(&mut app, 2);
    assert!(matches!(race(&app).phase, RacePhase::Countdown { .. }));
    assert_eq!(arrow(&mut app).1, Visibility::Visible);
    press(&mut app, KeyCode::KeyX);
    run(&mut app, 1);
    end_press(&mut app, KeyCode::KeyX);
    assert_eq!(
        app.world().get::<TargetSelection>(car).unwrap().picked,
        Some(1),
        "cycling already works during the countdown"
    );
}

/// HUD-2 scopes the compass arrow to Blitz/Checkpoint — an `Ordered`
/// (Circuit) race never shows the needle.
#[test]
fn ordered_race_shows_no_arrow() {
    let def = RaceDefinition {
        rule: CheckpointRule::Ordered,
        ..any_order_def(0)
    };
    let mut app = race_app(event_config(), def.clone());
    spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));
    run(&mut app, 5);
    assert_eq!(arrow(&mut app).1, Visibility::Hidden);
}

/// Once every gate is cleared the needle points at the armed finish
/// trigger; a resolved participant and a complete race hide it, and
/// session teardown despawns it (AC05 — nothing stale survives).
#[test]
fn arrow_arms_the_finish_then_cleans_up() {
    let def = RaceDefinition {
        finish: Some(cp(500.0, 0.0)),
        ..any_order_def(0)
    };
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    run(&mut app, 3);
    assert_eq!(arrow(&mut app).1, Visibility::Visible);

    // One segment sweeps both gates — the finish is now the objective.
    set_position(&mut app, car, Vec3::new(200.0, 0.0, 0.0));
    run(&mut app, 1);
    assert_eq!(progress(&app, car).cleared_count(), 2);
    let (rot, vis, _) = arrow(&mut app);
    assert_eq!(vis, Visibility::Visible);
    assert!(
        (rot.as_radians() - std::f32::consts::FRAC_PI_2).abs() < 1e-3,
        "the finish at (500,0) is dead right of a −Z-facing car: {}",
        rot.as_radians()
    );

    // Cross the finish — the participant resolves and the needle hides.
    set_position(&mut app, car, Vec3::new(520.0, 0.0, 0.0));
    run(&mut app, 1);
    assert!(matches!(
        progress(&app, car).state,
        ParticipantState::Finished { .. }
    ));
    assert_eq!(arrow(&mut app).1, Visibility::Hidden);

    // Teardown removes the session-owned needle with the rest.
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
    let world = app.world_mut();
    assert_eq!(
        world
            .query_filtered::<Entity, With<NavArrow>>()
            .iter(world)
            .count(),
        0,
        "the session-owned arrow despawned"
    );
}

/// The session-owned `LOW TIME` banner, read back as
/// `(visibility, text color)`.
fn warning(app: &mut App) -> (Visibility, Color) {
    let entity = {
        let world = app.world_mut();
        world
            .query_filtered::<Entity, With<LowTimeWarning>>()
            .iter(world)
            .next()
            .expect("the harness spawns the race warning")
    };
    (
        *app.world().get::<Visibility>(entity).unwrap(),
        app.world().get::<TextColor>(entity).unwrap().0,
    )
}

/// The designed low-time cue (DSN-9): the banner arms when
/// `time_remaining` reaches `LOW_TIME_TICKS` and pulses bright/dim on
/// a 0.5 s cadence derived from the remaining ticks — the same
/// authoritative race clock the deadline is judged on (F12-AC04), so
/// it freezes with a pause and clears when the deadline resolves.
#[test]
fn low_time_warning_pulses_on_the_race_clock() {
    let def = timed_def(0, LOW_TIME_TICKS + 40);
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));

    // The first `update` runs no fixed step, so clock = 2(n−1)−1
    // after release: 39 at update 21, and the remaining time crosses
    // the threshold inside update 22.
    run(&mut app, 21);
    assert!(race(&app).time_remaining() > Some(LOW_TIME_TICKS));
    assert_eq!(
        warning(&mut app).0,
        Visibility::Hidden,
        "above the threshold"
    );

    run(&mut app, 1); // clock 41 — remaining is one tick under
    assert_eq!(
        warning(&mut app),
        (Visibility::Visible, LOW_TIME_BRIGHT),
        "the warning arms at the threshold, bright first"
    );

    // The pulse reads the race clock, not a wall clock: each
    // `LOW_TIME_FLASH_TICKS` of remaining time flips the phase.
    run(&mut app, 29); // 60 ticks of warning elapsed — still bright
    assert_eq!(warning(&mut app).1, LOW_TIME_BRIGHT);
    run(&mut app, 1); // 61 elapsed — dim half
    assert_eq!(warning(&mut app).1, LOW_TIME_DIM);
    run(&mut app, 29); // 119 elapsed — still dim
    assert_eq!(warning(&mut app).1, LOW_TIME_DIM);
    run(&mut app, 1); // 121 elapsed — bright again
    assert_eq!(warning(&mut app).1, LOW_TIME_BRIGHT);

    // Paused, the race clock holds — so does the pulse phase.
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    run(&mut app, 10);
    assert_eq!(
        warning(&mut app),
        (Visibility::Visible, LOW_TIME_BRIGHT),
        "the frozen clock holds the pulse phase"
    );
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Playing)
        .unwrap();

    // Stand the clock one step short of the deadline: expiry resolves
    // the participant and the race — the banner hides with it.
    app.world_mut().resource_mut::<RaceState>().clock = u64::from(LOW_TIME_TICKS) + 38;
    run(&mut app, 1);
    assert!(matches!(
        progress(&app, car).state,
        ParticipantState::TimedOut { .. }
    ));
    assert_eq!(race(&app).phase, RacePhase::Complete);
    assert_eq!(warning(&mut app).0, Visibility::Hidden);
}

/// The cue waits for `Running`: a race whose whole limit sits under
/// the threshold still shows nothing while the countdown holds the
/// clock at zero.
#[test]
fn low_time_warning_waits_for_the_running_phase() {
    let def = timed_def(600, 400);
    let mut app = race_app(event_config(), def.clone());
    spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));
    run(&mut app, 20);
    assert!(matches!(race(&app).phase, RacePhase::Countdown { .. }));
    assert_eq!(race(&app).time_remaining(), Some(400));
    assert_eq!(
        warning(&mut app).0,
        Visibility::Hidden,
        "a counting-down race never warns"
    );

    run(&mut app, 302); // 600 countdown ticks release at update 300
    assert_eq!(race(&app).phase, RacePhase::Running);
    assert_eq!(
        warning(&mut app).0,
        Visibility::Visible,
        "a sub-threshold limit warns from the first running tick"
    );
}

/// The cue belongs to the local participant: once it resolves, the
/// banner hides even while a remote participant keeps the race
/// running — the remaining deadline is theirs, not the finished
/// driver's.
#[test]
fn low_time_warning_ignores_other_participants_deadlines() {
    let def = timed_def(0, LOW_TIME_TICKS + 40);
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    let (remote, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    app.world_mut().get_mut::<Player>(remote).unwrap().control = PlayerControl::Remote;
    run(&mut app, 22); // remaining under the threshold
    assert_eq!(warning(&mut app).0, Visibility::Visible);

    // The local car sweeps both gates and finishes — the remote is
    // still racing, so the race stays Running, but the local driver's
    // warning is done.
    set_position(&mut app, car, Vec3::new(200.0, 0.0, 0.0));
    run(&mut app, 1);
    assert!(matches!(
        progress(&app, car).state,
        ParticipantState::Finished { .. }
    ));
    assert_eq!(
        race(&app).phase,
        RacePhase::Running,
        "the remote keeps it open"
    );
    assert_eq!(warning(&mut app).0, Visibility::Hidden);
}

/// Untimed races never arm the banner (`time_remaining` is `None`),
/// and session teardown despawns the session-owned node (AC05).
#[test]
fn untimed_race_never_warns_and_teardown_cleans_up() {
    let def = any_order_def(0);
    let mut app = race_app(event_config(), def.clone());
    spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    run(&mut app, 10);
    assert_eq!(race(&app).phase, RacePhase::Running);
    assert_eq!(race(&app).time_remaining(), None);
    assert_eq!(warning(&mut app).0, Visibility::Hidden);

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
    let world = app.world_mut();
    assert_eq!(
        world
            .query_filtered::<Entity, With<LowTimeWarning>>()
            .iter(world)
            .count(),
        0,
        "the session-owned banner despawned"
    );
}

/// F14-B/F13-B live order (DSN-13) through the production driver: each
/// participant's own progress moves it in the order — more gates, then
/// proximity to the next one — a finish locks the lead while the other
/// still races, and once everyone resolves the order equals the
/// ledger's standings.
#[test]
fn live_order_tracks_progress_and_locks_finished_places() {
    let def = RaceDefinition {
        checkpoints: vec![cp(0.0, 0.0), cp(100.0, 0.0)],
        finish: None,
        rule: CheckpointRule::Ordered,
        laps: 2,
        time_limit_ticks: None,
        params: mm2_game::EventParams::default(),
        countdown_ticks: 0,
        start_slots: Vec::new(),
    };
    let mut app = race_app(event_config(), def.clone());
    let (remote, pr) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, -2.0));
    let (local, pl) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 2.0));
    app.world_mut().get_mut::<Player>(remote).unwrap().control = PlayerControl::Remote;
    run(&mut app, 2); // release + anchor

    let order = |app: &App| -> Vec<PlayerId> {
        let world = app.world();
        let definition = &world.resource::<RaceState>().definition;
        live_order(
            definition,
            world.iter_entities().filter_map(|e| {
                match (
                    e.get::<Player>(),
                    e.get::<RaceProgress>(),
                    e.get::<Position>(),
                ) {
                    (Some(p), Some(prog), Some(pos)) => Some((p.id, prog, pos.0)),
                    _ => None,
                }
            }),
        )
    };
    // Waypoints never park inside an un-cleared trigger (the 15 m
    // radius covers 85..115): each `set_position` also produces ghost
    // segments back toward the previous `GlobalTransform` via avian's
    // `transform_to_position`, so every park-to-park path must sweep
    // only the gates it means to.
    let drive = |app: &mut App, e: Entity, z: f32, xs: &[f32]| {
        for &x in xs {
            set_position(app, e, Vec3::new(x, 0.0, z));
            run(app, 1);
        }
    };

    // The remote's extra gate outranks the local's empty progress.
    drive(&mut app, remote, -2.0, &[10.0, 30.0]);
    assert_eq!(order(&app), vec![pr, pl]);

    // Same progress: proximity to the next gate decides the lead —
    // the local parks nearer gate 1 (x=80, just outside its radius).
    drive(&mut app, local, 2.0, &[10.0, 80.0]);
    assert_eq!(order(&app), vec![pl, pr], "nearer gate 1 leads");

    // A completed lap outranks proximity: the remote's lap-1 wrap beats
    // the local parked outside gate 1's door.
    drive(&mut app, remote, -2.0, &[90.0]);
    assert_eq!(progress(&app, remote).lap, 1);
    assert_eq!(order(&app), vec![pr, pl]);

    // The remote finishes lap 2 — its lead is now a locked place, not
    // progress; the local still races so the session stays Playing.
    drive(&mut app, remote, -2.0, &[40.0, 10.0, 40.0, 90.0]);
    assert!(matches!(
        progress(&app, remote).state,
        ParticipantState::Finished { .. }
    ));
    assert_eq!(phase(&app), SessionPhase::Playing);
    assert_eq!(order(&app), vec![pr, pl], "a finished place is locked");

    // The local finishes later; the live order resolves into the same
    // ordering the ledger's standings record.
    drive(&mut app, local, 2.0, &[90.0, 40.0, 10.0, 40.0, 90.0]);
    assert!(matches!(
        progress(&app, local).state,
        ParticipantState::Finished { .. }
    ));
    assert_eq!(race(&app).phase, RacePhase::Complete);
    assert_eq!(order(&app), vec![pr, pl]);
    let standings: Vec<PlayerId> = app
        .world()
        .resource::<ResultLedger>()
        .standings()
        .iter()
        .map(|r| r.id.participant)
        .collect();
    assert_eq!(standings, order(&app), "live order converges to standings");
}

/// The session-owned countdown banner, read back as
/// `(visibility, label)` from its root and text child.
fn countdown_banner(app: &mut App) -> (Visibility, String) {
    let (root, text) = {
        let world = app.world_mut();
        let root = world
            .query_filtered::<Entity, (With<CountdownBanner>, Without<CountdownBannerText>)>()
            .iter(world)
            .next()
            .expect("the harness spawns the countdown banner");
        let text = world
            .query_filtered::<Entity, With<CountdownBannerText>>()
            .iter(world)
            .next()
            .expect("the banner carries a text child");
        (root, text)
    };
    (
        *app.world().get::<Visibility>(root).unwrap(),
        app.world().get::<Text>(text).unwrap().0.clone(),
    )
}

/// F17-B countdown presentation (DSN-19): the banner counts one second
/// per digit off the same `remaining` ticks the release is judged on,
/// then flashes `GO!` for `COUNTDOWN_GO_TICKS` of the race clock and
/// goes dark.
#[test]
fn countdown_banner_counts_digits_then_flashes_go() {
    let hz = mm2_game::RACE_TICK_HZ;
    let def = any_order_def(3 * hz);
    let mut app = race_app(event_config(), def.clone());
    spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));

    run(&mut app, 1);
    assert!(matches!(race(&app).phase, RacePhase::Countdown { .. }));
    assert_eq!(
        countdown_banner(&mut app),
        (Visibility::Visible, "3".to_string()),
        "the top digit is up from the first update"
    );

    // Step to each second boundary and read the digit the remaining
    // ticks imply — `ceil` keeps each digit up exactly one second.
    let reach = |app: &mut App, bound: u32| {
        for _ in 0..400 {
            match race(app).phase {
                RacePhase::Countdown { remaining } if remaining > bound => app.update(),
                _ => break,
            }
        }
    };
    reach(&mut app, 2 * hz);
    assert_eq!(countdown_banner(&mut app).1, "2");
    reach(&mut app, hz);
    assert_eq!(countdown_banner(&mut app).1, "1");

    // Release: the digit becomes `GO!` on the race clock.
    for _ in 0..400 {
        if race(&app).phase == RacePhase::Running {
            break;
        }
        app.update();
    }
    assert_eq!(phase(&app), SessionPhase::Playing);
    assert_eq!(
        countdown_banner(&mut app),
        (Visibility::Visible, "GO!".to_string())
    );

    // The flash is bounded by the race clock, not a wall clock.
    while race(&app).clock < COUNTDOWN_GO_TICKS {
        app.update();
    }
    assert_eq!(
        countdown_banner(&mut app).0,
        Visibility::Hidden,
        "GO! goes dark once its race-clock window ends"
    );
}

/// The `GO!` flash only belongs to a live `Playing` session: pausing
/// hides it (the pause overlay owns the screen) and resuming inside
/// the frozen clock's window brings it back — deterministic, since the
/// window is measured in race ticks. A `Results` session can never sit
/// under it either: even a finish landed inside the window takes the
/// banner down with the phase.
#[test]
fn countdown_banner_go_belongs_to_a_live_playing_session() {
    let def = any_order_def(0); // no digits — release shows only GO!
    let mut app = race_app(event_config(), def.clone());
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    for _ in 0..10 {
        if race(&app).phase == RacePhase::Running {
            break;
        }
        app.update();
    }
    assert!(race(&app).clock < COUNTDOWN_GO_TICKS);
    assert_eq!(countdown_banner(&mut app).1, "GO!");

    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    run(&mut app, 1);
    assert_eq!(
        countdown_banner(&mut app).0,
        Visibility::Hidden,
        "paused: the overlay owns the screen and the clock holds"
    );
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Playing)
        .unwrap();
    run(&mut app, 1);
    assert_eq!(
        countdown_banner(&mut app).1,
        "GO!",
        "the frozen race clock keeps the window open across a pause"
    );

    // A finish inside the window ends the session at Results — the
    // frozen race clock still sits inside it, but the cue must not
    // linger under the results overlay.
    set_position(&mut app, car, Vec3::new(200.0, 0.0, 0.0));
    run(&mut app, 1);
    assert_eq!(phase(&app), SessionPhase::Results);
    assert!(race(&app).clock < COUNTDOWN_GO_TICKS);
    assert_eq!(
        countdown_banner(&mut app).0,
        Visibility::Hidden,
        "Results owns the screen — no GO! behind it"
    );
}

/// The banner is presentation over the live race only: a stale
/// `RaceState` (generation mismatch — the restart seam) shows nothing,
/// and teardown despawns the session-owned tree (AC05/AC06 — nothing
/// survives into the next generation).
#[test]
fn countdown_banner_ignores_stale_race_and_despawns() {
    let def = any_order_def(600);
    let mut app = race_app(event_config(), def.clone());
    spawn_participant(&mut app, &def, Vec3::new(-50.0, 0.0, 0.0));
    run(&mut app, 2);
    assert_eq!(countdown_banner(&mut app).0, Visibility::Visible);

    // A race stamped for another generation is never the cue's input.
    app.world_mut().resource_mut::<RaceState>().generation += 1;
    run(&mut app, 1);
    assert_eq!(
        countdown_banner(&mut app).0,
        Visibility::Hidden,
        "a stale race cannot drive the countdown"
    );
    app.world_mut().resource_mut::<RaceState>().generation -= 1;
    run(&mut app, 1);
    assert_eq!(countdown_banner(&mut app).0, Visibility::Visible);

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
    let world = app.world_mut();
    assert_eq!(
        world
            .query_filtered::<Entity, With<CountdownBanner>>()
            .iter(world)
            .count(),
        0,
        "the session-owned banner despawned"
    );
}
