//! F21-B.4: a Crash Course lesson's legs run back to back through the
//! production `advance_race` + `drive_lesson` systems — the swap of
//! `RaceState`, the rebuilt progress, the respawned gate markers and the
//! `ResetVehicle` reseat — with `Position` writes standing in for
//! driving (no original data: the legs are self-authored fixtures).

use std::time::Duration;

use avian3d::prelude::*;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::lesson::{LessonDriver, drive_lesson};
use mm2_app::race::{
    CheckpointMarker, LessonSetup, advance_race, reanchor_teleported_participants,
    spawn_checkpoint_markers,
};
use mm2_app::results::{ResultsMenu, ResultsUi, results_present};
use mm2_content::{LessonLeg, LessonObjective};
use mm2_game::{
    Checkpoint, CheckpointRule, EventKey, EventTableKind, LegFailure, LessonPhase, LessonRun,
    ParticipantState, Player, PlayerControl, RaceDefinition, RacePhase, RaceProgress, RaceStart,
    RaceStarted, RaceState, ResultLedger, Session, SessionConfig, SessionEntity, SessionPhase,
    advance_session_tick,
};
use mm2_vehicle::{ResetVehicle, Vehicle, VehicleConfig, VehicleState};

fn gate(x: f32, z: f32) -> Checkpoint {
    Checkpoint {
        center: Vec3::new(x, 0.0, z),
        radius: 15.0,
        height: mm2_game::DEFAULT_CHECKPOINT_HEIGHT,
        heading_deg: -90.0,
        require_direction: false,
    }
}

fn leg(name: &str, at: Vec3, start: Vec3, yaw_deg: f32, limit: Option<u32>) -> LessonLeg {
    LessonLeg {
        filename: name.into(),
        objective: LessonObjective::Maneuver,
        source: format!("crash0:{name}"),
        definition: RaceDefinition {
            checkpoints: vec![gate(at.x, at.z)],
            finish: None,
            rule: CheckpointRule::AnyOrder,
            laps: 1,
            time_limit_ticks: limit,
            params: mm2_game::EventParams::default(),
            countdown_ticks: 2,
            start_slots: vec![RaceStart {
                position: start,
                yaw_deg: Some(yaw_deg),
            }],
        },
    }
}

/// Two legs: leg 0 gates at the origin (start −50 m on X), leg 1 gates
/// 300 m down +X with its own start slot and a time limit.
fn two_leg_setup(first_limit: Option<u32>) -> LessonSetup {
    let legs = vec![
        leg(
            "first",
            Vec3::ZERO,
            Vec3::new(-50.0, 0.0, 0.0),
            0.0,
            first_limit,
        ),
        leg(
            "second",
            Vec3::new(300.0, 0.0, 0.0),
            Vec3::new(250.0, 0.0, 0.0),
            90.0,
            Some(600),
        ),
    ];
    LessonSetup {
        key: EventKey {
            city: "london".into(),
            table: EventTableKind::CrashCourse,
            stem: "lesson1".into(),
        },
        run: LessonRun::new(legs.len()).unwrap(),
        legs,
        rewards: Default::default(),
        availability: Default::default(),
        aimap: None,
    }
}

fn lesson_app(setup: LessonSetup) -> (App, Entity) {
    lesson_app_with(setup, SessionConfig::default())
}

fn lesson_app_with(setup: LessonSetup, config: SessionConfig) -> (App, Entity) {
    let driver = LessonDriver::new(setup);
    let mut session = Session::new();
    session.begin(config).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Countdown).unwrap();
    let generation = session.generation();
    let race = driver.race_state(generation).unwrap();
    let first = race.definition.clone();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .init_asset::<StandardMaterial>()
        .init_asset::<Image>()
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .insert_resource(session)
        .insert_resource(race)
        .insert_resource(driver)
        .insert_resource(mm2_app::session::SpawnPoint::new(Vec3::ZERO, 0.0))
        .init_resource::<ResultLedger>()
        .init_resource::<ResultsMenu>()
        .add_message::<RaceStarted>()
        .add_message::<ResetVehicle>()
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (reanchor_teleported_participants, advance_race, drive_lesson).chain(),
        )
        .add_systems(
            Update,
            (mm2_vehicle::systems::vehicle_reset, results_present),
        );
    app.finish();
    app.cleanup();

    let generation = app.world().resource::<Session>().generation();
    spawn_markers(&mut app, &first);
    let id = app.world_mut().resource_mut::<Session>().mint_player_id();
    let cfg = VehicleConfig::default();
    let start = Vec3::new(-50.0, 0.0, 0.0);
    let car = app
        .world_mut()
        .spawn((
            SessionEntity(generation),
            Player {
                id,
                control: PlayerControl::Local,
            },
            RaceProgress::new(&first),
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
    (app, car)
}

fn spawn_markers(app: &mut App, def: &RaceDefinition) {
    let generation = app.world().resource::<Session>().generation();
    let def = def.clone();
    app.world_mut()
        .run_system_once(
            move |mut commands: Commands,
                  mut meshes: ResMut<Assets<Mesh>>,
                  mut images: ResMut<Assets<Image>>,
                  mut materials: ResMut<Assets<StandardMaterial>>| {
                spawn_checkpoint_markers(
                    &mut commands,
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    &def,
                    SessionEntity(generation),
                );
            },
        )
        .unwrap();
}

fn run(app: &mut App, updates: usize) {
    for _ in 0..updates {
        app.update();
    }
}

fn set_position(app: &mut App, car: Entity, pos: Vec3) {
    *app.world_mut().get_mut::<Position>(car).unwrap() = Position(pos);
    app.world_mut()
        .get_mut::<Transform>(car)
        .unwrap()
        .translation = pos;
}

fn phase(app: &App) -> SessionPhase {
    app.world().resource::<Session>().phase().clone()
}

fn driver(app: &App) -> &LessonDriver {
    app.world().resource::<LessonDriver>()
}

fn race(app: &App) -> &RaceState {
    app.world().resource::<RaceState>()
}

fn marker_centers(app: &mut App) -> Vec<Vec3> {
    let mut q = app
        .world_mut()
        .query_filtered::<&Transform, With<CheckpointMarker>>();
    q.iter(app.world()).map(|t| t.translation).collect()
}

/// Release the countdown and sweep the car through the current leg's
/// only gate: the step that clears it.
fn clear_leg(app: &mut App, car: Entity, from: Vec3, to: Vec3) {
    // Countdown of 2 ticks → released within a couple of updates.
    run(app, 4);
    assert_eq!(race(app).phase, RacePhase::Running);
    set_position(app, car, from);
    run(app, 1);
    set_position(app, car, to);
    run(app, 2);
}

#[test]
fn clearing_a_non_final_leg_swaps_the_race_and_reseats_the_car() {
    let (mut app, car) = lesson_app(two_leg_setup(None));
    clear_leg(
        &mut app,
        car,
        Vec3::new(-50.0, 0.0, 0.0),
        Vec3::new(50.0, 0.0, 0.0),
    );

    // The session carries on — a non-final clear never reaches Results.
    assert_eq!(phase(&app), SessionPhase::Playing);
    assert_eq!(driver(&app).run().current_leg(), Some(1));
    assert_eq!(driver(&app).current_leg().unwrap().filename, "second");
    // The race runtime now runs leg 1: its gate, its limit, a fresh
    // countdown and clock.
    let r = race(&app);
    assert_eq!(r.definition.time_limit_ticks, Some(600));
    assert_eq!(
        r.definition.checkpoints[0].center,
        Vec3::new(300.0, 0.0, 0.0)
    );
    assert!(
        matches!(r.phase, RacePhase::Countdown { .. }) || r.clock < 10,
        "leg 1 counts down afresh, got {:?} clock {}",
        r.phase,
        r.clock
    );
    // Progress was rebuilt for the new gate list, not carried over.
    let progress = app.world().get::<RaceProgress>(car).unwrap();
    assert_eq!(progress.cleared_count(), 0);
    // The car stands on leg 1's start slot.
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!(
        (pos - Vec3::new(250.0, 0.0, 0.0)).length() < 1e-3,
        "reseated at the leg's slot, got {pos}"
    );
    // The gate markers followed: only leg 1's gantry remains.
    let centers = marker_centers(&mut app);
    assert!(!centers.is_empty());
    assert!(
        centers
            .iter()
            .all(|c| (*c - Vec3::new(300.0, 0.0, 0.0)).length() < 1e-3),
        "stale markers survived: {centers:?}"
    );
    assert!(driver(&app).pass().is_none());
}

#[test]
fn the_swap_does_not_count_the_reseat_as_a_crossing() {
    // Leg 1's gate sits between where leg 0 ended (x = 50) and leg 1's
    // start slot (x = 250): a teleport that counted as motion would
    // sweep it.
    let mut setup = two_leg_setup(None);
    setup.legs[1].definition.checkpoints[0] = gate(150.0, 0.0);
    let (mut app, car) = lesson_app(setup);
    clear_leg(
        &mut app,
        car,
        Vec3::new(-50.0, 0.0, 0.0),
        Vec3::new(50.0, 0.0, 0.0),
    );
    run(&mut app, 8);
    assert_eq!(driver(&app).run().current_leg(), Some(1));
    let progress = app.world().get::<RaceProgress>(car).unwrap();
    assert_eq!(progress.cleared_count(), 0, "the reseat swept the gate");
    assert_eq!(progress.state, ParticipantState::Racing);
    assert_eq!(phase(&app), SessionPhase::Playing);
}

#[test]
fn the_last_clear_passes_once_and_ends_the_session() {
    let (mut app, car) = lesson_app(two_leg_setup(None));
    clear_leg(
        &mut app,
        car,
        Vec3::new(-50.0, 0.0, 0.0),
        Vec3::new(50.0, 0.0, 0.0),
    );
    clear_leg(
        &mut app,
        car,
        Vec3::new(250.0, 0.0, 0.0),
        Vec3::new(350.0, 0.0, 0.0),
    );
    assert_eq!(phase(&app), SessionPhase::Results);
    assert_eq!(driver(&app).run().phase(), LessonPhase::Passed);
    let pass = driver(&app).pass().expect("the lesson passed");
    assert_eq!(pass.leg_ticks.len(), 2);
    assert_eq!(pass.attempt, 1);
    // Idle afterwards — no stale-report churn from later ticks.
    run(&mut app, 10);
    assert_eq!(driver(&app).run().stale_reports(), 0);
    assert_eq!(driver(&app).run().phase(), LessonPhase::Passed);
}

#[test]
fn a_timed_out_leg_fails_the_lesson_and_ends_the_session() {
    let (mut app, _car) = lesson_app(two_leg_setup(Some(20)));
    // Never cross the gate: let the 20-tick limit expire.
    run(&mut app, 40);
    assert_eq!(phase(&app), SessionPhase::Results);
    assert_eq!(
        driver(&app).run().phase(),
        LessonPhase::Failed {
            leg: 0,
            failure: LegFailure::TimedOut
        }
    );
    assert!(driver(&app).pass().is_none());
    // The failed lesson does not move on to leg 1.
    assert_eq!(race(&app).definition.time_limit_ticks, Some(20));
    assert_eq!(driver(&app).run().stale_reports(), 0);
}

/// F21-B.9 end to end: the lesson's legs are cleared by the production
/// `advance_race` + `drive_lesson` systems and the profile credit is made
/// by the production `record_session_results` — no hand-fed `observe`
/// and no injected ledger entry. Both legs' own finishes reach the
/// ledger and must not become records; the pass is one record on the
/// lesson key with the sum of the legs' clear times, and the authored
/// reward grants once.
#[test]
fn clearing_every_leg_credits_the_profile_once_through_the_production_systems() {
    use mm2_app::profile::{ActiveProfile, ProfileRequest};
    use mm2_app::progression::{EventRewards, SessionReport, record_session_results};
    use mm2_game::{
        AvailabilityTable, Difficulty, ProfileKind, ProfileStore, RewardRequirement, RewardRule,
        RewardTable, Unlock, VehicleSelection, WorldMode,
    };

    let setup = two_leg_setup(None);
    let key = setup.key.clone();
    let config = SessionConfig {
        world: WorldMode::City {
            psdl: "city/testcity.psdl".to_string(),
        },
        vehicle: VehicleSelection {
            id: Some("vpt".to_string()),
            paint: 0,
        },
        ..SessionConfig::default()
    };
    let (mut app, car) = lesson_app_with(setup, config);

    let store_dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(store_dir.path()).unwrap();
    let profile = mm2_app::profile::resolve(
        &store,
        &ProfileRequest::Create {
            name: "driver".to_string(),
            rank: Difficulty::Amateur,
            kind: ProfileKind::Standard,
        },
    )
    .unwrap()
    .expect("a created profile binds");
    let profile_id = profile.profile.id.clone();
    app.insert_resource(profile);
    app.insert_resource(EventRewards {
        key: key.clone(),
        table: RewardTable {
            per_event: vec![(
                key.clone(),
                RewardRule {
                    family: EventTableKind::CrashCourse,
                    requirement: RewardRequirement::Event(3),
                    unlock: Unlock::Vehicle("vpreward".into()),
                    message: "lesson reward".into(),
                    line: 1,
                },
            )],
            ..RewardTable::default()
        },
        availability: AvailabilityTable::default(),
    });
    app.add_systems(
        FixedLast,
        record_session_results
            .after(drive_lesson)
            .run_if(resource_exists::<ActiveProfile>),
    );

    clear_leg(
        &mut app,
        car,
        Vec3::new(-50.0, 0.0, 0.0),
        Vec3::new(50.0, 0.0, 0.0),
    );
    // Mid-lesson the first leg's finish is in the ledger, yet nothing
    // reached the profile.
    assert_eq!(driver(&app).run().current_leg(), Some(1));
    run(&mut app, 4);
    let saved = store.load(&profile_id).unwrap().profile.progress;
    assert!(saved.events.is_empty() && saved.unlocks.is_empty());

    clear_leg(
        &mut app,
        car,
        Vec3::new(250.0, 0.0, 0.0),
        Vec3::new(350.0, 0.0, 0.0),
    );
    run(&mut app, 10);
    assert_eq!(phase(&app), SessionPhase::Results);
    let total = driver(&app)
        .pass()
        .expect("the lesson passed")
        .total_ticks();
    assert!(total > 0);

    let saved = store.load(&profile_id).unwrap().profile.progress;
    assert_eq!(
        saved.events.len(),
        1,
        "only the lesson key: {:?}",
        saved.events
    );
    let record = &saved.events[0];
    assert_eq!(record.key, key);
    assert!(record.is_beaten());
    assert_eq!(record.finishes, 1, "one pass, one count");
    assert_eq!(record.best_race_ticks, Some(total));
    assert!(saved.unlocks.contains("vehicle:vpreward"));
    assert_eq!(saved.unlocks.len(), 1);
    let report = app.world().resource::<SessionReport>();
    assert!(report.recorded);
    assert_eq!(report.granted, vec!["lesson reward".to_string()]);
}

/// Every line the results overlay drew.
fn overlay_texts(app: &mut App) -> Vec<String> {
    let world = app.world_mut();
    world
        .query_filtered::<&Text, With<ResultsUi>>()
        .iter(world)
        .map(|t| t.0.clone())
        .collect()
}

/// A passed lesson is not a race: the results screen says so, lists
/// each leg's clear time and offers the lesson's own retry.
#[test]
fn a_passed_lesson_reports_a_pass_and_its_legs() {
    let (mut app, car) = lesson_app(two_leg_setup(None));
    clear_leg(
        &mut app,
        car,
        Vec3::new(-50.0, 0.0, 0.0),
        Vec3::new(50.0, 0.0, 0.0),
    );
    clear_leg(
        &mut app,
        car,
        Vec3::new(250.0, 0.0, 0.0),
        Vec3::new(350.0, 0.0, 0.0),
    );
    run(&mut app, 2);
    assert_eq!(phase(&app), SessionPhase::Results);
    let texts = overlay_texts(&mut app);
    let total = driver(&app).pass().unwrap().total_ticks() as f32 / mm2_game::RACE_TICK_HZ as f32;
    assert!(texts.iter().any(|t| t == "Crash Course"), "{texts:?}");
    assert!(
        texts.contains(&format!("Lesson passed - {total:.1}s")),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.starts_with("1. first - ")),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.starts_with("2. second - ")),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("Retry lesson")),
        "{texts:?}"
    );
    // None of the race screen's vocabulary: no field, no placing.
    assert!(
        !texts.iter().any(|t| t.contains("Race results")
            || t.contains("Restart race")
            || t.contains(" of ")),
        "{texts:?}"
    );
}

/// A failed lesson names the leg and the reason, keeps the legs it did
/// clear, and marks the failed one — it never reads as a finish.
#[test]
fn a_failed_lesson_names_the_leg_and_the_reason() {
    let (mut app, car) = lesson_app(two_leg_setup(None));
    clear_leg(
        &mut app,
        car,
        Vec3::new(-50.0, 0.0, 0.0),
        Vec3::new(50.0, 0.0, 0.0),
    );
    // Leg 1 (limit 600 ticks) is left to expire.
    run(&mut app, 1500);
    assert_eq!(phase(&app), SessionPhase::Results);
    assert!(matches!(
        driver(&app).run().phase(),
        LessonPhase::Failed { leg: 1, .. }
    ));
    let texts = overlay_texts(&mut app);
    assert!(
        texts
            .iter()
            .any(|t| t == "Lesson failed - out of time on leg 2 of 2"),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.starts_with("1. first - ")),
        "{texts:?}"
    );
    assert!(texts.iter().any(|t| t == "2. second - failed"), "{texts:?}");
    assert!(
        texts.iter().any(|t| t.contains("Retry lesson")),
        "{texts:?}"
    );
    assert!(!texts.iter().any(|t| t.contains("passed")), "{texts:?}");
}
