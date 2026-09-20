//! F01-A session contract tests: lifecycle transitions, ownership
//! teardown, config validation and the fixed-step session clock.

use std::time::Duration;

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_formats::racedata::{EventRow, EventTable, RaceParams};
use mm2_game::*;

fn playing_session() -> Session {
    let mut s = Session::new();
    s.begin(SessionConfig::default()).unwrap();
    s.transition(SessionPhase::Ready).unwrap();
    s.transition(SessionPhase::Playing).unwrap();
    s
}

#[test]
fn begin_validates_and_enters_loading() {
    let mut s = Session::new();
    assert_eq!(*s.phase(), SessionPhase::Menu);
    assert!(s.config().is_none());
    s.begin(SessionConfig::default()).unwrap();
    assert_eq!(*s.phase(), SessionPhase::Loading);
    assert_eq!(s.generation(), 1);
    assert!(matches!(s.config(), Some(c) if c.mode == SessionMode::Cruise));
}

#[test]
fn full_lifecycle_menu_to_menu() {
    let mut s = Session::new();
    s.begin(SessionConfig::default()).unwrap();
    s.transition(SessionPhase::Ready).unwrap();
    s.transition(SessionPhase::Countdown).unwrap();
    s.transition(SessionPhase::Playing).unwrap();
    s.transition(SessionPhase::Paused).unwrap();
    s.transition(SessionPhase::Playing).unwrap();
    s.transition(SessionPhase::Results).unwrap();
    s.transition(SessionPhase::Unloading).unwrap();
    s.transition(SessionPhase::Menu).unwrap();
    // Restart: a new begin bumps the generation and rewinds the clock.
    s.begin(SessionConfig::default()).unwrap();
    assert_eq!(s.generation(), 2);
    assert_eq!(s.tick(), 0);
    assert_eq!(*s.phase(), SessionPhase::Loading);
}

#[test]
fn quit_mid_session_unloads_to_menu() {
    let mut s = playing_session();
    s.transition(SessionPhase::Unloading).unwrap();
    s.transition(SessionPhase::Menu).unwrap();
    s.begin(SessionConfig::default()).unwrap();
    assert_eq!(s.generation(), 2);
}

#[test]
fn illegal_transitions_are_rejected_without_moving() {
    let mut s = Session::new();
    for to in [
        SessionPhase::Playing,
        SessionPhase::Ready,
        SessionPhase::Paused,
        SessionPhase::Unloading,
        SessionPhase::Results,
    ] {
        let err = s.transition(to.clone()).unwrap_err();
        assert!(matches!(err, SessionError::IllegalTransition { .. }));
        assert_eq!(*s.phase(), SessionPhase::Menu, "rejected {to:?}");
    }

    let mut s = playing_session();
    assert!(s.transition(SessionPhase::Loading).is_err());
    assert!(s.transition(SessionPhase::Menu).is_err());
    assert!(s.transition(SessionPhase::Countdown).is_err());
    assert_eq!(*s.phase(), SessionPhase::Playing);
    // Pause must not skip to menu or back to loading either.
    s.transition(SessionPhase::Paused).unwrap();
    assert!(s.transition(SessionPhase::Menu).is_err());
    assert!(s.transition(SessionPhase::Results).is_err());
    assert_eq!(*s.phase(), SessionPhase::Paused);
}

#[test]
fn failed_load_unloads_before_restart() {
    let mut s = Session::new();
    s.begin(SessionConfig::default()).unwrap();
    s.fail("city/bogus.psdl: not found").unwrap();
    assert_eq!(
        *s.phase(),
        SessionPhase::Failed("city/bogus.psdl: not found".into())
    );
    // A failed session cannot keep playing or skip cleanup.
    assert!(s.transition(SessionPhase::Playing).is_err());
    assert!(s.begin(SessionConfig::default()).is_err());
    s.transition(SessionPhase::Unloading).unwrap();
    s.transition(SessionPhase::Menu).unwrap();
    s.begin(SessionConfig::default()).unwrap();
    assert_eq!(s.generation(), 2);
}

#[test]
fn networked_authority_cannot_pause() {
    // MP-6: no pausing in multiplayer — the lifecycle enforces it from
    // the session authority rather than trusting callers.
    for authority in [SessionAuthority::Host, SessionAuthority::Remote] {
        let mut s = Session::new();
        s.begin(SessionConfig {
            authority,
            ..SessionConfig::default()
        })
        .unwrap();
        s.transition(SessionPhase::Ready).unwrap();
        s.transition(SessionPhase::Playing).unwrap();
        let err = s.transition(SessionPhase::Paused).unwrap_err();
        assert!(matches!(err, SessionError::IllegalTransition { .. }));
        assert_eq!(*s.phase(), SessionPhase::Playing);
    }
}

#[test]
fn config_validation_rejects_bad_fields() {
    let mut s = Session::new();
    let bad_density = SessionConfig {
        densities: Densities {
            traffic: 1.5,
            pedestrians: 0.0,
        },
        ..SessionConfig::default()
    };
    let err = s.begin(bad_density).unwrap_err();
    assert!(matches!(
        err,
        SessionError::Config(ConfigError::Density {
            field: "traffic",
            ..
        })
    ));
    assert_eq!(*s.phase(), SessionPhase::Menu, "rejected begin keeps Menu");

    let blank_car = SessionConfig {
        vehicle: VehicleSelection {
            id: Some("   ".into()),
            paint: 0,
        },
        ..SessionConfig::default()
    };
    assert!(matches!(
        s.begin(blank_car).unwrap_err(),
        SessionError::Config(ConfigError::EmptyVehicleId)
    ));

    let nan_density = SessionConfig {
        densities: Densities {
            traffic: f32::NAN,
            pedestrians: 0.0,
        },
        ..SessionConfig::default()
    };
    assert!(s.begin(nan_density).is_err());
}

#[test]
fn selectors_are_bounded_to_authored_range() {
    // WLD-4/UNK-1: 0-3 are valid authored selectors; the meaning of each
    // index is unverified, so the type bounds the range without naming.
    assert_eq!(TimeOfDay::new(3).unwrap().get(), 3);
    assert!(TimeOfDay::new(4).is_err());
    assert_eq!(Weather::new(0).unwrap().get(), 0);
    assert!(Weather::new(9).is_err());
}

#[test]
fn difficulty_selects_the_authored_param_block() {
    let params = |limit: f32| RaceParams {
        car_type: 0,
        time_of_day: 0,
        weather: 0,
        opponents: 0,
        cops: 0,
        ambient: 0.0,
        peds: 0.0,
        num_laps: 0,
        time_limit: limit,
        difficulty: 0,
    };
    let row = EventRow {
        description: "none".into(),
        amateur: params(50.0),
        professional: params(40.0),
        line: 2,
    };
    assert_eq!(Difficulty::Amateur.params(&row).time_limit, 50.0);
    assert_eq!(Difficulty::Professional.params(&row).time_limit, 40.0);
}

#[test]
fn event_ref_points_at_a_table_row() {
    let event = EventRef {
        city: "london".into(),
        table: EventTableKind::Blitz,
        index: 1,
    };
    assert_eq!(event.table_path(), "race/london/mmblitzdata.csv");
    let table = EventTable {
        rows: vec![EventRow {
            description: "row0".into(),
            amateur: RaceParams {
                car_type: 0,
                time_of_day: 0,
                weather: 0,
                opponents: 0,
                cops: 0,
                ambient: 0.0,
                peds: 0.0,
                num_laps: 0,
                time_limit: 0.0,
                difficulty: 0,
            },
            professional: RaceParams {
                car_type: 0,
                time_of_day: 0,
                weather: 0,
                opponents: 0,
                cops: 0,
                ambient: 0.0,
                peds: 0.0,
                num_laps: 0,
                time_limit: 0.0,
                difficulty: 0,
            },
            line: 2,
        }],
        diagnostics: Vec::new(),
    };
    assert!(event.row(&table).is_none(), "index 1 is out of bounds");
    let in_bounds = EventRef {
        index: 0,
        ..event.clone()
    };
    assert_eq!(in_bounds.row(&table).unwrap().description, "row0");
}

#[test]
fn session_entities_despawn_but_persistent_entities_survive() {
    /// Stand-in for persistent UI/profile state — never session-owned.
    #[derive(Component)]
    struct PersistentUi;
    #[derive(Component)]
    struct ChildPart;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_systems(Update, despawn_session_entities);

    // Session-owned root + child, a second root, and a persistent node.
    let root = app
        .world_mut()
        .spawn((SessionEntity(1), Name::new("car")))
        .id();
    app.world_mut()
        .spawn((ChildPart, ChildOf(root), Name::new("wheel")));
    app.world_mut().spawn((SessionEntity(1), Name::new("prop")));
    let persistent = app
        .world_mut()
        .spawn((PersistentUi, Name::new("hud-root")))
        .id();

    app.update();

    let world = app.world_mut();
    assert_eq!(
        world
            .query_filtered::<Entity, With<SessionEntity>>()
            .iter(world)
            .count(),
        0,
        "session entities are gone"
    );
    assert_eq!(
        world
            .query_filtered::<Entity, With<ChildPart>>()
            .iter(world)
            .count(),
        0,
        "children cascade with their root"
    );
    assert!(
        world.get_entity(persistent).is_ok(),
        "persistent state is not touched"
    );
}

#[test]
fn older_generation_entities_are_cleaned_too() {
    // A straggler stamped with an old generation is still session-owned —
    // teardown is not limited to the current generation.
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_systems(Update, despawn_session_entities);
    app.world_mut().spawn(SessionEntity(1));
    app.world_mut().spawn(SessionEntity(2));
    app.update();
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<SessionEntity>>()
            .iter(app.world())
            .count(),
        0
    );
}

#[test]
fn session_tick_counts_fixed_steps_not_updates() {
    // AC03: the gameplay clock runs on the fixed timestep. At 60 Hz
    // virtual updates and a 120 Hz fixed step, each update after the
    // clock priming frame produces exactly 2 ticks — regardless of how
    // a renderer would batch frames.
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(playing_session())
        .add_systems(FixedUpdate, advance_session_tick);
    let tick = |app: &App| app.world().resource::<Session>().tick();

    // The first update primes the time clock; measure deltas over a
    // known window rather than absolute counts.
    app.update();
    let primed = tick(&app);
    for _ in 0..10 {
        app.update();
    }
    assert_eq!(tick(&app) - primed, 20);

    // Paused stops the clock; resuming continues it.
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    let paused = tick(&app);
    for _ in 0..10 {
        app.update();
    }
    assert_eq!(tick(&app), paused, "paused session clock must not move");
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Playing)
        .unwrap();
    for _ in 0..10 {
        app.update();
    }
    assert_eq!(tick(&app) - paused, 20);
}
