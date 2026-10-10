//! F22-C validation (F22-AC01): the instrument line across the
//! pause → resume → reset → finish journey, always as a faithful read
//! of the authoritative state — the production publisher's
//! `VehicleTelemetry` snapshot plus the live `RaceState`/`RaceProgress`
//! and the result ledger. The line recomputes nothing: every leg
//! asserts its text against the snapshot and the race state the
//! production systems hold at that moment, so a HUD that drifts from
//! the simulation (or survives a reset with stale numbers) fails here.
//!
//! The legs run the real chain: `advance_race` drives the countdown
//! release, the swept crossings, the clock and the `Playing →
//! Results` transition; `vehicle_reset` performs the `R`-reset
//! teleport; and `contracts::publish_vehicle_telemetry` — the binary's
//! own FixedLast publisher — copies `VehicleState` into the snapshot
//! the HUD reads. The per-instrument units (timer glyph composition,
//! speedometer dial, nav-arrow release) keep their own modules; this
//! file is the end-to-end agreement of the line with the state
//! machine.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::contracts;
use mm2_app::hud;
use mm2_app::race::{advance_race, reanchor_teleported_participants};
use mm2_app::session::{self, SpawnPoint};
use mm2_game::{
    Checkpoint, CheckpointRule, EventParams, EventRef, EventTableKind, ObjectId, ObjectIdentity,
    ParticipantState, Player, PlayerControl, PlayerId, RaceDefinition, RacePhase, RaceProgress,
    RaceStart, RaceState, ResultLedger, Session, SessionConfig, SessionEntity, SessionMode,
    SessionPhase, TargetSelection, advance_session_tick, live_order,
};
use mm2_vehicle::{ResetVehicle, TireConditions, Vehicle, VehicleConfig, VehicleState};

/// One fixed step at the binary's 120 Hz, presented as one 60 Hz frame.
const FRAME: f64 = 1.0 / 60.0;

fn cp(x: f32, z: f32) -> Checkpoint {
    Checkpoint {
        center: Vec3::new(x, 0.0, z),
        radius: 15.0,
        height: mm2_game::DEFAULT_CHECKPOINT_HEIGHT,
        heading_deg: 0.0,
        require_direction: false,
    }
}

/// A two-gate AnyOrder course: enough to show `cp` counting, the
/// clock and the finish without a lap wrap.
fn course_def() -> RaceDefinition {
    RaceDefinition {
        checkpoints: vec![cp(0.0, 0.0), cp(100.0, 0.0)],
        finish: None,
        rule: CheckpointRule::AnyOrder,
        laps: 1,
        time_limit_ticks: None,
        params: EventParams::default(),
        countdown_ticks: 120,
        start_slots: vec![RaceStart {
            position: Vec3::new(-50.0, 0.0, 0.0),
            yaw_deg: Some(0.0),
        }],
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

/// The production event path minus windowing and world content: the
/// same race driver, telemetry publisher and reset apply the binary's
/// schedules, with the HUD line spawned the way `load_session_world`
/// spawns it.
fn journey_app(def: &RaceDefinition) -> (App, Entity, PlayerId) {
    let mut session = Session::new();
    session.begin(event_config()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Countdown).unwrap();
    let generation = session.generation();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            FRAME,
        )))
        .insert_resource(session)
        .insert_resource(RaceState::new(def.clone(), generation))
        .insert_resource(SpawnPoint::new(Vec3::new(-50.0, 0.0, -20.0), 0.0))
        .init_resource::<ResultLedger>()
        .init_resource::<hud::HudVisible>()
        .init_resource::<TireConditions>()
        .add_message::<ResetVehicle>()
        .add_message::<mm2_game::RaceStarted>()
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (
                reanchor_teleported_participants,
                advance_race,
                contracts::publish_vehicle_telemetry,
            )
                .chain(),
        )
        .add_systems(
            Update,
            (mm2_vehicle::systems::vehicle_reset, hud::update_hud).chain(),
        );
    app.finish();
    app.cleanup();

    app.world_mut()
        .spawn((SessionEntity(generation), session::Hud, Text::new("")));

    let id = app.world_mut().resource_mut::<Session>().mint_player_id();
    let cfg = VehicleConfig::default();
    let start = Vec3::new(-50.0, 0.0, -10.0);
    let car = app
        .world_mut()
        .spawn((
            SessionEntity(generation),
            Player {
                id,
                control: PlayerControl::Local,
            },
            mm2_game::PlayerVehicle,
            ObjectIdentity(ObjectId {
                generation: 1,
                slot: 0,
            }),
            RaceProgress::new(def),
            TargetSelection::default(),
            Vehicle {
                config: cfg.clone(),
            },
            VehicleState::new(&cfg),
            Position(start),
            Rotation::default(),
            LinearVelocity::ZERO,
            AngularVelocity::ZERO,
            Transform::from_translation(start),
        ))
        .id();
    (app, car, id)
}

fn line(app: &mut App) -> String {
    app.world_mut()
        .query_filtered::<&Text, With<session::Hud>>()
        .single(app.world())
        .expect("the instrument line")
        .0
        .clone()
}

fn hud_visibility(app: &mut App) -> Visibility {
    let entity = app
        .world_mut()
        .query_filtered::<Entity, With<session::Hud>>()
        .single(app.world())
        .expect("the instrument line");
    *app.world().get::<Visibility>(entity).unwrap()
}

fn run(app: &mut App, frames: usize) {
    for _ in 0..frames {
        app.update();
    }
}

fn set_position(app: &mut App, entity: Entity, pos: Vec3) {
    *app.world_mut().get_mut::<Position>(entity).unwrap() = Position(pos);
    app.world_mut()
        .get_mut::<Transform>(entity)
        .unwrap()
        .translation = pos;
}

fn race_clock(app: &App) -> u64 {
    app.world().resource::<RaceState>().clock
}

/// The clock string `update_hud` composes for an untimed race — the
/// same authoritative `RaceState::clock` the deadline and timer use.
fn clock_text(app: &App) -> String {
    format!(
        "  {:.1}s",
        race_clock(app) as f32 / mm2_game::RACE_TICK_HZ as f32
    )
}

/// Cross one gate through its swept trigger the way the physics-driven
/// segment would: in, then out the far side.
fn cross_gate(app: &mut App, car: Entity, x: f32) {
    set_position(app, car, Vec3::new(x, 0.0, -10.0));
    run(app, 1);
    set_position(app, car, Vec3::new(x, 0.0, 10.0));
    run(app, 1);
}

#[test]
fn the_instrument_line_follows_the_authoritative_state_across_pause_reset_and_finish() {
    let def = course_def();
    let (mut app, car, id) = journey_app(&def);

    // --- Countdown: the banner comes from the race phase. ----------
    assert!(matches!(
        app.world().resource::<RaceState>().phase,
        RacePhase::Countdown { .. }
    ));
    run(&mut app, 2);
    let text = line(&mut app);
    assert!(
        text.contains("GET READY"),
        "countdown shows the race phase: {text}"
    );

    // The release is the production driver's: session `Playing`,
    // participant `Racing`, one `RaceStarted` — bounded, never a sleep.
    assert!(
        (0..200).any(|_| {
            app.update();
            app.world().resource::<Session>().phase() == &SessionPhase::Playing
        }),
        "the countdown released through advance_race"
    );
    assert!(matches!(
        app.world().resource::<RaceState>().phase,
        RacePhase::Running
    ));

    // --- Running: the line reads the publisher's snapshot. ---------
    *app.world_mut().get_mut::<LinearVelocity>(car).unwrap() =
        LinearVelocity(Vec3::new(10.0, 0.0, 0.0));
    {
        let mut state = app.world_mut().get_mut::<VehicleState>(car).unwrap();
        state.rpm = 3600.0;
        state.gear = 3;
    }
    run(&mut app, 2);
    let text = line(&mut app);
    let (speed_kmh, rpm) = {
        let veh = app
            .world()
            .get::<mm2_game::VehicleTelemetry>(car)
            .expect("the publisher wrote the snapshot");
        (veh.linear_velocity.length() * 3.6, veh.rpm)
    };
    assert!(
        text.contains(&format!("{speed_kmh:5.1} km/h")),
        "speed is the snapshot's, not a recompute: {text}"
    );
    assert!(text.contains("D4"), "gear is snapshot gear+1: {text}");
    assert!(
        text.contains(&format!("{:4.0} rpm", rpm)),
        "rpm is the snapshot's: {text}"
    );
    assert!(
        text.contains(&format!("cp 0/{}", def.checkpoints.len())),
        "cleared count starts at zero: {text}"
    );
    assert!(
        text.contains(&clock_text(&app)),
        "the clock is the race's own: {text}"
    );

    // Clear gate 0 through the swept trigger: the count follows.
    cross_gate(&mut app, car, 0.0);
    assert_eq!(
        app.world()
            .get::<RaceProgress>(car)
            .unwrap()
            .cleared_count(),
        1,
        "the sweep cleared gate 0"
    );
    assert!(
        line(&mut app).contains("cp 1/2"),
        "the line follows the progress: {}",
        line(&mut app)
    );

    // --- Pause: the world freezes, the line freezes with it. ------
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    let frozen = line(&mut app);
    let clock_at_pause = race_clock(&app);
    run(&mut app, 10);
    assert_eq!(race_clock(&app), clock_at_pause, "the paused clock froze");
    assert_eq!(
        line(&mut app),
        frozen,
        "a frozen race keeps a byte-identical line"
    );
    assert_eq!(
        hud_visibility(&mut app),
        Visibility::Visible,
        "pause does not hide the driving HUD (the pause menu is its own surface)"
    );

    // --- Resume: the clock runs on from where it paused. -----------
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Playing)
        .unwrap();
    run(&mut app, 5);
    assert!(race_clock(&app) > clock_at_pause, "the clock resumed");
    assert!(
        line(&mut app).contains(&clock_text(&app)),
        "the resumed line tracks the live clock: {}",
        line(&mut app)
    );

    // --- Reset: the teleport zeroes the state the snapshot reads. --
    let spawn = app.world().resource::<SpawnPoint>().position;
    app.world_mut()
        .resource_mut::<Messages<ResetVehicle>>()
        .write(ResetVehicle {
            entity: Some(car),
            position: spawn,
            yaw: 0.0,
        });
    run(&mut app, 2);
    // `vehicle_reset` marks the jump `Teleported`; the FixedLast
    // re-anchor consumes the marker in the same pass (the swept
    // segment breaks at the spawn, as in `race.rs`'s reset leg), so
    // the observable outcome is the pose itself.
    assert_eq!(
        app.world().get::<Transform>(car).unwrap().translation,
        spawn,
        "the production reset path teleported the car back to the spawn point"
    );
    assert_eq!(
        app.world()
            .get::<mm2_game::VehicleTelemetry>(car)
            .expect("post-reset snapshot")
            .linear_velocity,
        Vec3::ZERO,
        "reset rests the car"
    );
    let text = line(&mut app);
    assert!(
        text.contains("  0.0 km/h"),
        "the line shows the post-reset snapshot, not the pre-reset speed: {text}"
    );
    assert!(
        text.contains("cp 1/2"),
        "a reset does not clear the race progress the line reports: {text}"
    );

    // --- Finish: the outcome comes from the ledger, once. ----------
    cross_gate(&mut app, car, 100.0);
    run(&mut app, 1);
    assert_eq!(
        app.world().resource::<Session>().phase(),
        &SessionPhase::Results,
        "a local finish moves the session to Results (UI-5)"
    );
    assert_eq!(
        app.world().resource::<RaceState>().phase,
        RacePhase::Complete
    );
    let progress = app.world().get::<RaceProgress>(car).unwrap();
    let ParticipantState::Finished { race_ticks, .. } = progress.state else {
        panic!("the local driver finished: {:?}", progress.state);
    };
    let text = line(&mut app);
    assert!(
        text.contains("FINISHED"),
        "the outcome line replaces the live readout: {text}"
    );
    // The displayed seconds are the recorded race-clock time, not a
    // wall clock: `race_ticks / RACE_TICK_HZ`, to the shown precision.
    let want = format!(
        "  {:.1}s",
        race_ticks as f32 / mm2_game::RACE_TICK_HZ as f32
    );
    assert!(
        text.contains(&want),
        "finish time is the recorded race time: {text} vs {want}"
    );

    // The place is the ledger's, and the ledger has one row.
    let ledger = app.world().resource::<ResultLedger>();
    assert_eq!(ledger.len(), 1, "exactly one recorded result");
    assert_eq!(ledger.place_of(id), Some(1));
    assert!(
        text.contains("1st"),
        "the outcome carries the ledger's place: {text}"
    );

    // --- Results stays put: further frames never re-record. --------
    let results_text = line(&mut app);
    run(&mut app, 6);
    assert_eq!(line(&mut app), results_text, "the outcome line is stable");
    assert_eq!(app.world().resource::<ResultLedger>().len(), 1);

    // Live-order agreement (HUD-2's place-indicator source): the order
    // the HUD would read over the same participants is this session's
    // single driver.
    let definition = app.world().resource::<RaceState>().definition.clone();
    let order: Vec<PlayerId> = live_order(
        &definition,
        app.world_mut()
            .query::<(&Player, &RaceProgress, &Position)>()
            .iter(app.world())
            .map(|(p, prog, pos)| (p.id, prog, pos.0)),
    );
    assert_eq!(order, vec![id]);
}
