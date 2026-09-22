//! F17-B.2 results-screen integration: the local resolution's
//! `Playing → Results` lands on a real overlay — the outcome, the
//! field's standings and the earned/refused rewards — whose rows drive
//! the same `SessionControl` intents the pause menu uses (UI-5, the
//! F17-AC01 return leg). Deterministic `Position` writes stand in for
//! physics segments, the same convention `tests/race.rs` uses.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::progression::{self, EventRewards, SessionReport};
use mm2_app::race::{advance_race, reanchor_teleported_participants};
use mm2_app::results::{self, ResultsMenu, ResultsUi};
use mm2_app::session::{self, SessionControl, SessionNote};
use mm2_game::{
    Checkpoint, CheckpointRule, EventKey, EventRef, EventTableKind, ParticipantState, Player,
    PlayerControl, PlayerId, ProfileKind, ProfileStore, RaceDefinition, RaceProgress, RaceStart,
    RaceState, ResultLedger, RewardRequirement, RewardRule, RewardTable, Session, SessionConfig,
    SessionEntity, SessionMode, SessionPhase, Unlock, VehicleSelection, WorldMode,
    advance_session_tick, despawn_session_entities,
};
use mm2_vehicle::{Vehicle, VehicleConfig, VehicleState};

fn cp(x: f32, z: f32) -> Checkpoint {
    Checkpoint {
        center: Vec3::new(x, 0.0, z),
        radius: 15.0,
        height: mm2_game::DEFAULT_CHECKPOINT_HEIGHT,
        heading_deg: 0.0,
        require_direction: false,
    }
}

/// A record-eligible event config (city world + catalog vehicle) —
/// `record_eligibility` reads config fields only, so no VFS is needed.
fn event_config() -> SessionConfig {
    SessionConfig {
        world: WorldMode::City {
            psdl: "city/london.psdl".into(),
        },
        mode: SessionMode::Event(EventRef {
            city: "london".into(),
            table: EventTableKind::Checkpoint,
            index: 0,
        }),
        vehicle: VehicleSelection {
            id: Some("vpt".into()),
            paint: 0,
        },
        ..SessionConfig::default()
    }
}

fn race_def(countdown: u32) -> RaceDefinition {
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

fn timed_def() -> RaceDefinition {
    RaceDefinition {
        time_limit_ticks: Some(10),
        ..race_def(0)
    }
}

/// A headless app running the production race driver, the results
/// overlay and the result→profile consumer, minus windowing — tests
/// drive positions and keys directly, the same way `tests/race.rs`
/// does.
fn results_app(
    config: SessionConfig,
    def: RaceDefinition,
    profile: Option<mm2_app::profile::ActiveProfile>,
    rewards: Option<EventRewards>,
) -> App {
    let mut session = Session::new();
    session.begin(config).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Countdown).unwrap();
    let generation = session.generation();

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
        .insert_resource(RaceState::new(def, generation))
        .insert_resource(mm2_app::session::SpawnPoint {
            position: Vec3::new(0.0, 1.5, 0.0),
            yaw: 0.0,
            trailers: Vec::new(),
        })
        .init_resource::<ResultLedger>()
        .init_resource::<SessionControl>()
        .init_resource::<SessionNote>()
        .init_resource::<ResultsMenu>()
        .init_resource::<mm2_app::contracts::ImpactFilter>()
        .init_resource::<mm2_app::damage::DamageReport>()
        .init_resource::<ButtonInput<KeyCode>>()
        .add_message::<mm2_game::RaceStarted>()
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (reanchor_teleported_participants, advance_race).chain(),
        )
        .add_systems(
            Update,
            (
                session::session_control_input,
                // Same ordering contract as the binary: results owns
                // `Results`, running between the intent reader (which
                // ignores `Results`) and the driver.
                results::results_input
                    .after(session::session_control_input)
                    .before(session::drive_session),
                (
                    despawn_session_entities.run_if(session::unloading),
                    results::dev_finish_once,
                    session::drive_session,
                )
                    .chain(),
                results::results_present.after(session::drive_session),
                progression::record_session_results,
            ),
        );
    if let Some(profile) = profile {
        app.insert_resource(profile);
    }
    if let Some(rewards) = rewards {
        app.insert_resource(rewards);
    }
    app.finish();
    app.cleanup();
    app
}

fn phase(app: &App) -> SessionPhase {
    app.world().resource::<Session>().phase().clone()
}

/// Spawn a race participant — `Player` + `RaceProgress` + `Position`,
/// stamped with the session's ownership generation (same shape as
/// `tests/race.rs`: no `RigidBody`, so the tests' `Position` writes
/// are the segments `advance_race` sweeps).
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
            mm2_game::TargetSelection::default(),
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
    app.world_mut()
        .get_mut::<Transform>(entity)
        .unwrap()
        .translation = pos;
}

fn run(app: &mut App, updates: usize) {
    for _ in 0..updates {
        app.update();
    }
}

/// Drain `AppExit` messages written so far. `Messages` is
/// double-buffered — a write ages out two updates later — so callers
/// must drain while the write is fresh, not after a `run`.
fn drain_exits(app: &mut App) -> Vec<AppExit> {
    app.world_mut()
        .resource_mut::<Messages<AppExit>>()
        .drain()
        .collect()
}

/// Press a key for exactly one update (no InputPlugin runs here, so
/// the input state is managed by hand).
fn press(app: &mut App, key: KeyCode) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(key);
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .reset_all();
}

/// Every text line the results overlay drew — the root plus each
/// child carries `ResultsUi`.
fn overlay_texts(app: &mut App) -> Vec<String> {
    let world = app.world_mut();
    world
        .query_filtered::<&Text, With<ResultsUi>>()
        .iter(world)
        .map(|t| t.0.clone())
        .collect()
}

fn overlay_roots(app: &mut App) -> usize {
    let world = app.world_mut();
    world
        .query_filtered::<Entity, (With<ResultsUi>, Without<ChildOf>)>()
        .iter(world)
        .count()
}

/// Drive `entity` through the two gates to a finish, the same
/// `Position`-segment convention the race tests use.
fn drive_to_finish(app: &mut App, def: &RaceDefinition, entity: Entity, lane: f32) {
    for gate in &def.checkpoints {
        let mut p = gate.center;
        p.z += lane;
        set_position(app, entity, p);
        run(app, 1);
    }
}

/// A bound standard profile in a tempdir store — the reward leg's
/// persistence target.
fn bound_profile() -> (tempfile::TempDir, mm2_app::profile::ActiveProfile) {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(dir.path()).unwrap();
    let profile = store
        .create(
            "driver",
            mm2_game::Difficulty::Amateur,
            ProfileKind::Standard,
        )
        .unwrap();
    (
        dir,
        mm2_app::profile::ActiveProfile {
            store,
            profile,
            recovered_from_backup: false,
        },
    )
}

/// The event key the test config's `SessionMode::Event` resolves to.
fn event_key() -> EventKey {
    EventKey {
        city: "london".into(),
        table: EventTableKind::Checkpoint,
        stem: "race0".into(),
    }
}

/// UI-5: a local finish lands on the results overlay — headline
/// placing + time, the field in ledger order (resolved entries with
/// their outcome, unresolved ones honestly still racing), and the
/// continue/restart rows.
#[test]
fn results_screen_shows_the_outcome_and_the_field() {
    let def = race_def(0);
    let mut app = results_app(event_config(), def.clone(), None, None);
    let (remote, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, -2.0));
    let (local, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 2.0));
    app.world_mut().get_mut::<Player>(remote).unwrap().control = PlayerControl::Remote;
    run(&mut app, 2); // release + anchor

    // The remote sweeps both gates first; the local finishes second.
    for gate in &def.checkpoints {
        let mut p = gate.center;
        p.z -= 2.0;
        set_position(&mut app, remote, p);
        run(&mut app, 1);
    }
    assert_eq!(
        phase(&app),
        SessionPhase::Playing,
        "a remote finish ends nothing"
    );
    drive_to_finish(&mut app, &def, local, 2.0);
    assert_eq!(phase(&app), SessionPhase::Results);
    assert_eq!(overlay_roots(&mut app), 1, "the results overlay drew");

    let texts = overlay_texts(&mut app);
    assert!(texts.iter().any(|t| t == "Race results"), "{texts:?}");
    assert!(
        texts.iter().any(|t| t.starts_with("2nd of 2 - ")),
        "the headline is the local placing + total time: {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("1. driver") && t.ends_with('s')),
        "the remote's finish leads the standings: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.starts_with("2. You - ")),
        "the local entry reads You: {texts:?}"
    );
    // No MenuShell in this harness — the continue row is "Quit".
    assert!(texts.iter().any(|t| t == "> Quit"), "{texts:?}");
    assert!(texts.iter().any(|t| t == "  Restart race"), "{texts:?}");
}

/// The field entry for a still-racing participant is reported, not
/// invented — the session resolves at the local finish (DSN-11).
#[test]
fn results_screen_marks_unresolved_participants() {
    let def = race_def(0);
    let mut app = results_app(event_config(), def.clone(), None, None);
    let (remote, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, -2.0));
    let (local, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 2.0));
    app.world_mut().get_mut::<Player>(remote).unwrap().control = PlayerControl::Remote;
    run(&mut app, 2);
    drive_to_finish(&mut app, &def, local, 2.0);
    assert_eq!(phase(&app), SessionPhase::Results);

    let texts = overlay_texts(&mut app);
    assert!(
        texts
            .iter()
            .any(|t| t == "1. You - 0.0s" || t.starts_with("1. You")),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("still racing")),
        "the unfinished remote is marked, not placed: {texts:?}"
    );
}

/// A time-out lands on the same screen with the honest outcome.
#[test]
fn results_screen_reports_a_timeout() {
    let def = timed_def();
    let mut app = results_app(event_config(), def.clone(), None, None);
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    run(&mut app, 8);
    assert!(matches!(
        app.world().get::<RaceProgress>(car).unwrap().state,
        ParticipantState::TimedOut { .. }
    ));
    assert_eq!(phase(&app), SessionPhase::Results);
    let texts = overlay_texts(&mut app);
    assert!(texts.iter().any(|t| t == "Out of time"), "{texts:?}");
    // The expiry also resolves the local result — it stands in the
    // field list as out of time.
    assert!(texts.iter().any(|t| t.contains("out of time")), "{texts:?}");
}

/// The return leg: Enter on the focused row is quit-to-menu — the
/// session tears down through `Unloading` (overlay, race state and
/// report all die with it) and, with no `MenuShell` running, `Menu`
/// exits.
#[test]
fn results_continue_quits_through_teardown() {
    let def = race_def(0);
    let mut app = results_app(event_config(), def.clone(), None, None);
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    run(&mut app, 2);
    drive_to_finish(&mut app, &def, car, 0.0);
    assert_eq!(phase(&app), SessionPhase::Results);
    assert_eq!(overlay_roots(&mut app), 1);

    press(&mut app, KeyCode::Enter);
    assert!(
        app.world().resource::<SessionControl>().quit,
        "the focused row issues the quit intent"
    );
    let mut exits = Vec::new();
    for _ in 0..6 {
        app.update();
        exits.extend(drain_exits(&mut app));
    }
    assert_eq!(phase(&app), SessionPhase::Menu);
    assert_eq!(
        overlay_roots(&mut app),
        0,
        "the overlay dies with the session"
    );
    assert!(
        app.world().get_resource::<RaceState>().is_none(),
        "the race state is session-scoped"
    );
    assert!(
        app.world().get_resource::<SessionReport>().is_none(),
        "the report is session-scoped"
    );
    assert!(
        exits.iter().any(|e| matches!(e, AppExit::Success)),
        "quit with no menu shell exits: {exits:?}"
    );

    // Esc is the same intent (Back → continue).
    let mut app = results_app(event_config(), race_def(0), None, None);
    let def = race_def(0);
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    run(&mut app, 2);
    drive_to_finish(&mut app, &def, car, 0.0);
    press(&mut app, KeyCode::Escape);
    for _ in 0..6 {
        app.update();
    }
    assert_eq!(phase(&app), SessionPhase::Menu, "Esc continues to menu");
}

/// The restart row replays the event through the same teardown →
/// `begin` path the pause menu's Restart uses — generation bumps, the
/// overlay and report die with the old session.
#[test]
fn results_restart_replays_the_event() {
    let def = race_def(0);
    let mut app = results_app(event_config(), def.clone(), None, None);
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    run(&mut app, 2);
    drive_to_finish(&mut app, &def, car, 0.0);
    assert_eq!(phase(&app), SessionPhase::Results);

    // Down moves focus onto "Restart race"; Enter issues the intent.
    press(&mut app, KeyCode::ArrowDown);
    press(&mut app, KeyCode::Enter);
    assert!(app.world().resource::<SessionControl>().restart);
    let mut reloaded = false;
    for _ in 0..12 {
        app.update();
        if phase(&app) == SessionPhase::Loading {
            reloaded = true;
            break;
        }
    }
    assert!(reloaded, "restart from results never re-began");
    assert_eq!(app.world().resource::<Session>().generation(), 2);
    assert_eq!(overlay_roots(&mut app), 0);
    assert!(app.world().get_resource::<SessionReport>().is_none());
}

/// The reward leg lands on screen: an eligible finish records the
/// event, grants the authored per-event unlock once, and the report
/// the overlay reads carries both facts (F17-AC01).
#[test]
fn results_reports_recorded_rewards() {
    let def = race_def(0);
    let (_dir, profile) = bound_profile();
    let rewards = EventRewards {
        key: event_key(),
        table: RewardTable {
            per_event: vec![(
                event_key(),
                RewardRule {
                    family: EventTableKind::Checkpoint,
                    requirement: RewardRequirement::Event(0),
                    unlock: Unlock::Vehicle("vpreward".into()),
                    message: "A new vehicle is yours".into(),
                    line: 1,
                },
            )],
            ..RewardTable::default()
        },
        availability: mm2_game::AvailabilityTable::default(),
    };
    let mut app = results_app(event_config(), def.clone(), Some(profile), Some(rewards));
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    run(&mut app, 2);
    drive_to_finish(&mut app, &def, car, 0.0);
    assert_eq!(phase(&app), SessionPhase::Results);
    run(&mut app, 2); // the report lands a frame after the result

    let report = app
        .world()
        .get_resource::<SessionReport>()
        .expect("the consumer wrote a report");
    assert!(report.recorded, "the finish recorded");
    assert_eq!(report.granted, vec!["A new vehicle is yours".to_string()]);
    let texts = overlay_texts(&mut app);
    assert!(
        texts
            .iter()
            .any(|t| t == "Unlocked: A new vehicle is yours"),
        "the authored unlock message shows: {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.contains("saved to the driver profile")),
        "{texts:?}"
    );

    // And it was persisted — the unlock id is in the saved progress.
    let store = &app
        .world()
        .resource::<mm2_app::profile::ActiveProfile>()
        .store;
    let saved = store
        .load(
            &app.world()
                .resource::<mm2_app::profile::ActiveProfile>()
                .profile
                .id,
        )
        .unwrap()
        .profile;
    assert!(saved.progress.unlocks.contains("vehicle:vpreward"));
}

/// A finish with no bound driver keeps nothing — and the screen says
/// so instead of implying a save.
#[test]
fn results_reports_a_profileless_run() {
    let def = race_def(0);
    let mut app = results_app(event_config(), def.clone(), None, None);
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    run(&mut app, 2);
    drive_to_finish(&mut app, &def, car, 0.0);
    run(&mut app, 2);
    let report = app.world().get_resource::<SessionReport>().unwrap();
    assert!(!report.recorded);
    assert_eq!(
        report.note.as_deref(),
        Some("no driver profile - progress is not saved")
    );
    let texts = overlay_texts(&mut app);
    assert!(
        texts.iter().any(|t| t.contains("progress is not saved")),
        "{texts:?}"
    );
}

/// `--finish` drives the real path: each update the local participant
/// is swept into its next open trigger, the swept segment clears it,
/// and the session resolves to `Results` on its own — no test reaches
/// into the ledger or the phase machine.
#[test]
fn dev_finish_sweeps_to_the_results_screen() {
    let def = race_def(0);
    let mut config = event_config();
    config.dev.finish = true;
    // A bound driver, so the gate that refuses the record is the
    // override itself rather than profile absence.
    let (_dir, profile) = bound_profile();
    let mut app = results_app(config, def.clone(), Some(profile), None);
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    let mut reached = false;
    for _ in 0..30 {
        app.update();
        if phase(&app) == SessionPhase::Results {
            reached = true;
            break;
        }
    }
    assert!(reached, "--finish never resolved the race");
    assert!(matches!(
        app.world().get::<RaceProgress>(car).unwrap().state,
        ParticipantState::Finished { .. }
    ));
    assert_eq!(app.world().resource::<ResultLedger>().len(), 1);
    assert_eq!(overlay_roots(&mut app), 1, "the results screen is up");
    // The sweep is gameplay-affecting — the report must refuse the
    // record, and the screen shows why.
    run(&mut app, 2);
    let report = app.world().get_resource::<SessionReport>().unwrap();
    assert!(!report.recorded);
    assert!(
        report
            .note
            .as_deref()
            .is_some_and(|n| n.contains("dev override finish")),
        "the finish override is record-ineligible: {report:?}"
    );
    // And the screen shows the refusal, not a fake save.
    let texts = overlay_texts(&mut app);
    assert!(
        texts.iter().any(|t| t.contains("records not kept")),
        "{texts:?}"
    );
}

/// `Results` input ownership: the keys `session_control_input` used to
/// own there now belong to the overlay — Esc no longer bypasses it
/// (it *is* the continue gesture), and stray keys do nothing.
#[test]
fn results_phase_ignores_stray_keys() {
    let def = race_def(0);
    let mut app = results_app(event_config(), def.clone(), None, None);
    let (car, _) = spawn_participant(&mut app, &def, Vec3::new(-200.0, 0.0, 0.0));
    run(&mut app, 2);
    drive_to_finish(&mut app, &def, car, 0.0);
    assert_eq!(phase(&app), SessionPhase::Results);
    // Left/Right/Delete have no rows to touch; the session stays put.
    for key in [KeyCode::ArrowLeft, KeyCode::ArrowRight, KeyCode::Delete] {
        press(&mut app, key);
        assert_eq!(phase(&app), SessionPhase::Results);
    }
    assert_eq!(
        overlay_roots(&mut app),
        1,
        "stray keys do not drop the screen"
    );
}
