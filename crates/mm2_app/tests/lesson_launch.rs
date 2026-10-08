//! F21-B.6: a Crash Course row launches through the real
//! `load_session_world` — leg 0 becomes the session's `RaceState`, the
//! `LessonDriver` rides with it, a restart rebuilds the driver on
//! leg 0, and a lesson that cannot build fails the session instead of
//! falling back to a plain race. Synthetic `race/london/` install; no
//! original data, except the opt-in `MM2_RETAIL` sweeps at the end
//! (launch, and retry-rebuilds-the-same-world on the real city worlds).

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::lesson::LessonDriver;
use mm2_app::race::{self, CheckpointMarker};
use mm2_app::session::{self, SessionControl};
use mm2_app::{camera, contracts};
use mm2_assets::Vfs;
use mm2_game::{
    EventRef, EventTableKind, ImpactEvent, LessonPhase, Mm2Vfs, ParticipantState, PlayerId,
    PlayerVehicle, RaceProgress, RaceStarted, RaceState, ResultId, ResultLedger, Session,
    SessionConfig, SessionEntity, SessionMode, SessionPhase, WorldMode, advance_session_tick,
    despawn_session_entities,
};
use mm2_vehicle::{VehicleConfig, VehiclePlugin};

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WP_HEADER: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";
const CRASHDATA: &str =
    "Filename,Event,Checkpoints,TimeLimit,AmbDensity,extra,extra,extra,extra,etra,\n";

fn write(dir: &Path, rel: &str, contents: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

fn lesson(dir: &Path, stem: &str, rows: &str) {
    write(dir, &format!("race/london/{stem}.aimap"), "[Opponent]\n0\n");
    for suffix in ["data", "data_p"] {
        write(
            dir,
            &format!("race/london/{stem}{suffix}.csv"),
            &format!("{CRASHDATA}{rows}"),
        );
    }
}

/// crash0 a one-leg slalom; crash1 an exam chaining two legs; crash2 a
/// lesson whose only leg links a waypoint file that does not exist.
fn lesson_install() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    write(
        dir,
        "race/london/mmcrashdata.csv",
        &format!(
            "{MM_HEADER}\n\
             lesson1,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1\n\
             midtrm1,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1\n\
             lesson2,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1\n"
        ),
    );
    lesson(dir, "crash0", "slalom,7,1,12,0.05,0,0,0,0,0,0\n");
    write(
        dir,
        "race/london/slalom.csv",
        &format!("{WP_HEADER}10,1,20,90,15,0,0,0,\n10,1,60,-30,16,0,0,0,\n"),
    );
    lesson(
        dir,
        "crash1",
        "gates,4,1,35,0,40,0,0,0,0\nfree,7,1,0,0,0,0,0,0,0\n",
    );
    write(
        dir,
        "race/london/gates.csv",
        &format!("{WP_HEADER}0,0,0,0,10,0,0,0,\n0,0,40,0,9,0,0,0,\n30,0,80,0,8,0,0,0,\n"),
    );
    write(
        dir,
        "race/london/free.csv",
        &format!("{WP_HEADER}5,0,5,0,6,0,0,0,\n5,0,25,0,6,0,0,0,\n"),
    );
    lesson(dir, "crash2", "nowhere,7,1,20,0,0,0,0,0,0\n");
    d
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

fn lesson_config(index: usize) -> SessionConfig {
    SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "london".into(),
            table: EventTableKind::CrashCourse,
            index,
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
        .insert_resource(session::SpawnPoint::new(Vec3::new(0.0, 1.5, 0.0), 0.0))
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
                mm2_app::lesson::drive_lesson,
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

fn markers(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<Entity, With<CheckpointMarker>>()
        .iter(app.world())
        .count()
}

#[test]
fn a_crash_row_loads_leg_zero_with_its_driver() {
    let tmp = lesson_install();
    let mut app = event_app(lesson_config(1), vfs_of(tmp.path()));
    app.update();

    assert_eq!(phase(&app), SessionPhase::Countdown);
    let driver = app.world().resource::<LessonDriver>();
    assert_eq!(driver.key().stem, "crash1");
    assert_eq!(driver.run().phase(), LessonPhase::Running { leg: 0 });
    assert_eq!(driver.current_leg().unwrap().filename, "gates");
    // Leg 0 ("gates": start row + two gates) is the session's race.
    let race = app.world().resource::<RaceState>();
    assert_eq!(race.generation, 1);
    assert_eq!(race.definition.checkpoints.len(), 2);
    assert_eq!(race.definition.time_limit_ticks, Some(35 * 120));
    // The local car is a participant of leg 0, with its gates marked.
    let car = car(&mut app);
    let progress = app.world().get::<RaceProgress>(car).unwrap();
    assert_eq!(progress.state, ParticipantState::AwaitingStart);
    assert_eq!(markers(&mut app), 2);
}

#[test]
fn a_restarted_lesson_re_enters_on_leg_zero_with_a_fresh_driver() {
    let tmp = lesson_install();
    let mut app = event_app(lesson_config(1), vfs_of(tmp.path()));
    app.update();
    // Clear leg 0 on the sequencer: the driver is now on leg 1.
    app.world_mut()
        .resource_mut::<LessonDriver>()
        .observe(&ParticipantState::Finished {
            race_ticks: 100,
            result: ResultId {
                generation: 1,
                participant: PlayerId(0),
                event: None,
                sequence: 0,
            },
        });
    assert_eq!(
        app.world().resource::<LessonDriver>().run().phase(),
        LessonPhase::Running { leg: 1 }
    );

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
    let driver = app.world().resource::<LessonDriver>();
    assert_eq!(driver.run().phase(), LessonPhase::Running { leg: 0 });
    assert_eq!(driver.run().attempt(), 1);
    assert_eq!(driver.current_leg().unwrap().filename, "gates");
    assert_eq!(
        app.world()
            .resource::<RaceState>()
            .definition
            .checkpoints
            .len(),
        2,
        "the race is leg 0 again"
    );
    assert_eq!(markers(&mut app), 2, "leg 0's gates only, none doubled");
}

#[test]
fn a_lesson_that_cannot_build_fails_the_session_with_no_driver() {
    let tmp = lesson_install();
    // crash2's only leg links a waypoint file that does not exist.
    let mut app = event_app(lesson_config(2), vfs_of(tmp.path()));
    app.update();
    assert!(
        matches!(phase(&app), SessionPhase::Failed(_)),
        "got {:?}",
        phase(&app)
    );
    assert!(app.world().get_resource::<LessonDriver>().is_none());
    assert!(app.world().get_resource::<RaceState>().is_none());
}

#[test]
fn a_non_crash_row_never_installs_a_lesson_driver() {
    let tmp = lesson_install();
    // The install authors no Checkpoint table, so this row fails the
    // plain event path (unchanged) and no driver appears.
    let mut config = lesson_config(0);
    config.mode = SessionMode::Event(EventRef {
        city: "london".into(),
        table: EventTableKind::Checkpoint,
        index: 0,
    });
    let mut app = event_app(config, vfs_of(tmp.path()));
    app.update();
    assert!(app.world().get_resource::<LessonDriver>().is_none());
}

/// The lesson's own `crash<N>.aimap{,_p}` rides on the launch setup so
/// its ambient-traffic overrides reach the session's traffic load
/// (CC-7; sf crash1/2/4/12 author ten per-road speed limits). The Professional record
/// wins at Professional, the Amateur one at Amateur; a lesson without a
/// resolvable record launches on the city's own aimap.
#[test]
fn a_lesson_launch_carries_its_difficulty_selected_aimap() {
    use mm2_game::Difficulty;

    let tmp = lesson_install();
    write(
        tmp.path(),
        "race/london/crash0.aimap",
        "[Exceptions]\n2\n10\t0.00\t0\n11\t0.00\t0\n",
    );
    write(
        tmp.path(),
        "race/london/crash0.aimap_p",
        "[Exceptions]\n1\n12\t0.00\t0\n",
    );
    let vfs = vfs_of(tmp.path());
    let event_ref = EventRef {
        city: "london".into(),
        table: EventTableKind::CrashCourse,
        index: 0,
    };
    let roads = |difficulty| {
        let (setup, _driver) = race::lesson_launch(&vfs, &event_ref, difficulty).unwrap();
        setup
            .aimap
            .expect("the lesson's aimap resolves")
            .exceptions
            .iter()
            .map(|e| e.road)
            .collect::<Vec<_>>()
    };
    assert_eq!(roads(Difficulty::Amateur), vec![10, 11]);
    assert_eq!(roads(Difficulty::Professional), vec![12]);

    // crash1's record authors no exceptions: an aimap, but an empty one.
    let other = EventRef {
        index: 1,
        ..event_ref.clone()
    };
    let (setup, _driver) = race::lesson_launch(&vfs, &other, Difficulty::Amateur).unwrap();
    assert!(setup.aimap.unwrap().exceptions.is_empty());
    // And an unreadable one degrades to the city's own aimap, the
    // lesson still launching.
    write(
        tmp.path(),
        "race/london/crash1.aimap",
        "[Exceptions]\n2\n1 0.0 0\n",
    );
    write(
        tmp.path(),
        "race/london/crash1.aimap_p",
        "[Exceptions]\n2\n1 0.0 0\n",
    );
    let (setup, _driver) = race::lesson_launch(&vfs, &other, Difficulty::Amateur).unwrap();
    assert!(setup.aimap.is_none());
}

/// Original-content validation (opt-in: `MM2_RETAIL` names an install).
/// Every Crash Course row of both cities, at both difficulties, loads
/// through the real `load_session_world` into `Countdown` on leg 0 with
/// its driver — the denominator is the catalog's own `CrashCourse`
/// rows, never a hard-coded or parse-filtered list. This proves launch
/// only: no gate is driven and the family rules stay unrecovered
/// (UNK-35).
#[test]
fn every_retail_lesson_launches_at_both_difficulties() {
    use mm2_assets::{InstallMount, mount_install};
    use mm2_game::Difficulty;

    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("MM2_RETAIL unset: retail lesson launch sweep NOT run");
        return;
    };
    let mut expected = 0;
    let mut launched = 0;
    let mut failures = Vec::new();
    for city in ["london", "sf"] {
        let mut vfs = Vfs::new();
        mount_install(&mut vfs, &retail, &InstallMount::default()).unwrap();
        let catalog = mm2_content::EventCatalog::scan(&vfs, city);
        let rows: Vec<EventRef> = catalog
            .events
            .iter()
            .filter(|e| e.event_ref.table == EventTableKind::CrashCourse)
            .map(|e| e.event_ref.clone())
            .collect();
        assert!(!rows.is_empty(), "{city}: no Crash Course rows enumerated");
        for event_ref in rows {
            for difficulty in [Difficulty::Amateur, Difficulty::Professional] {
                expected += 1;
                let mut vfs = Vfs::new();
                mount_install(&mut vfs, &retail, &InstallMount::default()).unwrap();
                let config = SessionConfig {
                    world: WorldMode::City {
                        psdl: format!("city/{city}.psdl"),
                    },
                    mode: SessionMode::Event(event_ref.clone()),
                    difficulty,
                    ..SessionConfig::default()
                };
                let mut app = event_app(config, vfs);
                app.update();
                let label = format!("{city} crash:{} {difficulty:?}", event_ref.index);
                let driver_ok = app
                    .world()
                    .get_resource::<LessonDriver>()
                    .is_some_and(|d| d.run().phase() == LessonPhase::Running { leg: 0 });
                if phase(&app) == SessionPhase::Countdown
                    && driver_ok
                    && app.world().get_resource::<RaceState>().is_some()
                {
                    launched += 1;
                } else {
                    failures.push(format!("{label}: phase {:?}", phase(&app)));
                }
            }
        }
    }
    eprintln!("retail lessons: expected {expected}, launched {launched}");
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(expected, launched);
}

/// Every session-owned entity, bucketed by its `Name` (unnamed ones
/// share one bucket) with the generation stamp read off the marker.
fn session_census(app: &mut App) -> (std::collections::BTreeMap<String, usize>, Vec<u64>) {
    let mut by_name = std::collections::BTreeMap::new();
    let mut generations = std::collections::BTreeSet::new();
    let mut query = app.world_mut().query::<(&SessionEntity, Option<&Name>)>();
    for (owner, name) in query.iter(app.world()) {
        let key = name.map_or("<unnamed>".to_owned(), |n| {
            // Strip a trailing index so `prop 12` and `prop 13` share a bucket.
            n.as_str()
                .trim_end_matches(|c: char| c.is_ascii_digit() || c == ' ' || c == '#' || c == '_')
                .to_owned()
        });
        *by_name.entry(key).or_insert(0) += 1;
        generations.insert(owner.0);
    }
    (by_name, generations.into_iter().collect())
}

/// F21-AC05 on original data: retrying a lesson (the session restart)
/// rebuilds the same world — the session-owned entity census after the
/// retry equals the first launch's, nothing from the previous
/// generation survives, and the sequencer is back on leg 0 with a
/// fresh attempt. Every Crash Course row of both cities is checked.
#[test]
fn a_retried_retail_lesson_rebuilds_the_same_world() {
    use mm2_assets::{InstallMount, mount_install};
    use mm2_game::Difficulty;

    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("MM2_RETAIL unset: retail lesson retry sweep NOT run");
        return;
    };
    let mut expected = 0;
    let mut checked = 0;
    let mut total_owned = 0;
    let mut failures = Vec::new();
    for city in ["london", "sf"] {
        let mut vfs = Vfs::new();
        mount_install(&mut vfs, &retail, &InstallMount::default()).unwrap();
        let catalog = mm2_content::EventCatalog::scan(&vfs, city);
        let rows: Vec<EventRef> = catalog
            .events
            .iter()
            .filter(|e| e.event_ref.table == EventTableKind::CrashCourse)
            .map(|e| e.event_ref.clone())
            .collect();
        assert!(!rows.is_empty(), "{city}: no Crash Course rows enumerated");
        for event_ref in rows {
            for difficulty in [Difficulty::Amateur, Difficulty::Professional] {
                expected += 1;
                let label = format!("{city} crash:{} {difficulty:?}", event_ref.index);
                let mut vfs = Vfs::new();
                mount_install(&mut vfs, &retail, &InstallMount::default()).unwrap();
                let config = SessionConfig {
                    world: WorldMode::City {
                        psdl: format!("city/{city}.psdl"),
                    },
                    mode: SessionMode::Event(event_ref.clone()),
                    difficulty,
                    ..SessionConfig::default()
                };
                let mut app = event_app(config, vfs);
                app.update();
                if phase(&app) != SessionPhase::Countdown {
                    failures.push(format!("{label}: first launch phase {:?}", phase(&app)));
                    continue;
                }
                let (first, first_gens) = session_census(&mut app);
                total_owned += first.values().sum::<usize>();
                // Clear leg 0 so the retry has a counter to reset.
                app.world_mut().resource_mut::<LessonDriver>().observe(
                    &ParticipantState::Finished {
                        race_ticks: 100,
                        result: ResultId {
                            generation: 1,
                            participant: PlayerId(0),
                            event: None,
                            sequence: 0,
                        },
                    },
                );
                app.world_mut().resource_mut::<SessionControl>().restart = true;
                let mut reached = false;
                for _ in 0..40 {
                    app.update();
                    if phase(&app) == SessionPhase::Countdown
                        && app.world().resource::<Session>().generation() == 2
                    {
                        reached = true;
                        break;
                    }
                }
                if !reached {
                    failures.push(format!("{label}: restart never returned to Countdown"));
                    continue;
                }
                let (second, second_gens) = session_census(&mut app);
                let driver = app.world().resource::<LessonDriver>();
                if driver.run().phase() != (LessonPhase::Running { leg: 0 })
                    || driver.run().attempt() != 1
                {
                    failures.push(format!("{label}: sequencer not fresh on leg 0"));
                }
                if first_gens.len() != 1 || second_gens.len() != 1 || first_gens == second_gens {
                    failures.push(format!(
                        "{label}: generations {first_gens:?} -> {second_gens:?}"
                    ));
                }
                if first != second {
                    failures.push(format!("{label}: census {first:?} -> {second:?}"));
                }
                checked += 1;
            }
        }
    }
    eprintln!(
        "retail lesson retries: expected {expected}, checked {checked}, \
         {total_owned} session-owned entities compared"
    );
    assert!(failures.is_empty(), "{failures:#?}");
    assert_eq!(expected, checked);
}
