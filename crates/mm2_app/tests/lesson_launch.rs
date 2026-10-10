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
    SessionConfig, SessionEntity, SessionMode, SessionPhase, VehicleBreaks, WorldMode,
    advance_session_tick, despawn_session_entities,
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
    event_app_with_car(
        config,
        vfs,
        session::SelectedCar {
            def: None,
            paint: 0,
        },
    )
}

/// [`event_app`] with the player's vehicle chosen — a loaded stock
/// `VehicleDef` brings its authored damage/breakaway rigs with it.
fn event_app_with_car(config: SessionConfig, vfs: Vfs, selected: session::SelectedCar) -> App {
    let tuned = selected
        .def
        .as_ref()
        .map_or_else(VehicleConfig::default, |d| d.config.clone());
    let mut session = Session::new();
    session.begin(config).unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(f64::from(mm2_game::RACE_TICK_HZ)))
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
        .insert_resource(session::TunedVehicle(tuned))
        .insert_resource(selected)
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
    assert_eq!(
        race.definition.time_limit_ticks,
        Some(35 * mm2_game::RACE_TICK_HZ)
    );
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
fn a_lesson_with_a_malformed_aimap_fails_the_session_not_a_bare_launch() {
    let tmp = lesson_install();
    // crash0's legs build, but its aimap record is truncated (count 2,
    // one row): the session fails rather than driving the lesson on the
    // city's ambient defaults with its police and lead cars dropped.
    write(
        tmp.path(),
        "race/london/crash0.aimap",
        "[Exceptions]\n2\n1 0.0 0\n",
    );
    let mut app = event_app(lesson_config(0), vfs_of(tmp.path()));
    app.update();
    assert!(
        matches!(phase(&app), SessionPhase::Failed(_)),
        "got {:?}",
        phase(&app)
    );
    assert!(app.world().get_resource::<LessonDriver>().is_none());
    assert!(app.world().get_resource::<RaceState>().is_none());
}

/// The route twin of the aimap refusal: a lead car whose `.opp` exists but
/// cannot be parsed fails the session, not a lesson whose lead car stands
/// still. (A route the catalog itself claims, one named for a leg, is
/// already refused as an incomplete event; this is the unclaimed name
/// resolved through the VFS.) A route that is merely absent stays a
/// reported issue — the slot keeps no line — so the same wiring with the
/// file missing still launches.
#[test]
fn a_lesson_whose_lead_route_is_malformed_fails_the_session_but_a_missing_one_launches() {
    let tmp = lesson_install();
    write(
        tmp.path(),
        "race/london/crash0.aimap",
        "[Opponent]\n1\nvpcab pursuit-0.opp 0.9 0 50.0 0.7 1 1 1 1 0 1.0\n",
    );
    write(
        tmp.path(),
        "race/london/pursuit-0.opp",
        "x,y\n1,2,not-a-number\n",
    );
    crate::support::tuned_car(tmp.path(), "vpcab", 1500.0);
    let mut app = event_app(lesson_config(0), vfs_of(tmp.path()));
    app.update();
    // The reason names the route, so the failure is this refusal and not
    // a missing car or world.
    assert!(
        matches!(phase(&app), SessionPhase::Failed(ref r) if r.contains("lead-car route")),
        "got {:?}",
        phase(&app)
    );
    assert!(app.world().get_resource::<LessonDriver>().is_none());
    assert!(app.world().get_resource::<RaceState>().is_none());

    // Remove the file: the reference dangles, which is reported, not refused.
    std::fs::remove_file(tmp.path().join("race/london/pursuit-0.opp")).unwrap();
    let (setup, _) = race::lesson_launch(
        &vfs_of(tmp.path()),
        &EventRef {
            city: "london".into(),
            table: EventTableKind::CrashCourse,
            index: 0,
        },
        mm2_game::Difficulty::Amateur,
    )
    .unwrap();
    assert_eq!(setup.lead_cars.entries.len(), 1);
    assert!(setup.lead_cars.entries[0].route.is_none());
    assert!(!setup.lead_cars.issues.is_empty());
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
/// resolvable record launches on the city's own aimap, and a malformed
/// one refuses the launch.
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
    // An unreadable one is refused rather than launched on the city's
    // own aimap with no police or lead cars (F29-AC04): the lesson is a
    // sibling-independent unit, so crash0 still launches beside it.
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
    let err = race::lesson_launch(&vfs, &other, Difficulty::Amateur)
        .err()
        .expect("a malformed lesson aimap refuses the launch");
    assert!(
        matches!(err, race::LessonSetupError::Aimap(_)),
        "unexpected error: {err}"
    );
    assert!(race::lesson_launch(&vfs, &event_ref, Difficulty::Amateur).is_ok());
}

/// A cop-chase lesson's own `[Police]` lineup (F21-B.18; retail london
/// crash10/11, sf crash5/7) rides on the launch setup, difficulty-
/// selected, with the lesson's `[CopChaseDistance]`. A lesson that
/// authors none launches copless, and the lesson's table `Cops` column
/// (0 on every retail row) is not a count to disagree with.
#[test]
fn a_lesson_launch_carries_its_difficulty_selected_police() {
    use mm2_game::Difficulty;

    let tmp = lesson_install();
    write(
        tmp.path(),
        "race/london/crash0.aimap",
        "[Police]\n2\nvpcop 10 0 40 90 0 15 0.5 50\nvpcop -20 0 60 0 0 15 0.5 50\n[CopChaseDistance]\n150\n",
    );
    write(
        tmp.path(),
        "race/london/crash0.aimap_p",
        "[Police]\n3\nvpcop 10 0 40 90 0 15 0.5 50\nvpcop -20 0 60 0 0 15 0.5 50\nvpcop 0 0 80 0 0 15 0.5 50\n",
    );
    let vfs = vfs_of(tmp.path());
    let event_ref = EventRef {
        city: "london".into(),
        table: EventTableKind::CrashCourse,
        index: 0,
    };
    let (setup, _) = race::lesson_launch(&vfs, &event_ref, Difficulty::Amateur).unwrap();
    assert_eq!(setup.police.entries.len(), 2);
    assert_eq!(setup.police.chase_distance, Some(150.0));
    assert_eq!(setup.police.entries[0].position, Vec3::new(10.0, 0.0, 40.0));
    assert!(
        setup.police.issues.is_empty(),
        "no table-count disagreement is reported: {:?}",
        setup.police.issues
    );
    let (setup, _) = race::lesson_launch(&vfs, &event_ref, Difficulty::Professional).unwrap();
    assert_eq!(
        setup.police.entries.len(),
        3,
        "the Professional record wins"
    );
    assert_eq!(setup.police.chase_distance, None);

    // crash1 authors only an `[Opponent]` section: no police.
    let other = EventRef {
        index: 1,
        ..event_ref
    };
    let (setup, _) = race::lesson_launch(&vfs, &other, Difficulty::Amateur).unwrap();
    assert!(setup.police.entries.is_empty());
}

/// The lesson's police are fielded by the production loader as
/// session-owned `PoliceCar`s standing at their authored posts, a
/// retry refields them under the new generation without doubling, and a
/// copless lesson fields none.
#[test]
fn a_cop_chase_lessons_police_is_fielded_and_refielded_on_retry() {
    use mm2_app::police::{PoliceCar, PoliceFleet};

    let tmp = lesson_install();
    write(
        tmp.path(),
        "race/london/crash0.aimap",
        "[Police]\n2\nvpcop 10 0 40 90 0 15 0.5 50\nvpcop -20 0 60 0 0 15 0.5 50\n",
    );
    crate::support::tuned_car(tmp.path(), "vpcop", 1500.0);
    let cops = |app: &mut App| -> Vec<(Entity, u64)> {
        app.world_mut()
            .query::<(Entity, &PoliceCar, &SessionEntity)>()
            .iter(app.world())
            .map(|(e, _, owner)| (e, owner.0))
            .collect()
    };

    let mut app = event_app(lesson_config(0), vfs_of(tmp.path()));
    app.update();
    assert_eq!(phase(&app), SessionPhase::Countdown);
    let first = cops(&mut app);
    assert_eq!(first.len(), 2, "both authored rows became cars");
    assert!(first.iter().all(|&(_, owner)| owner == 1));
    let fleet = app.world().resource::<PoliceFleet>();
    assert_eq!(
        (fleet.authored, fleet.spawned, fleet.load_failed),
        (2, 2, 0)
    );
    // Cops are not participants of the lesson's race.
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<RaceProgress>>()
            .iter(app.world())
            .count(),
        1,
        "only the player carries progress"
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
    let second = cops(&mut app);
    assert_eq!(second.len(), 2, "refielded, none doubled or left over");
    assert!(second.iter().all(|&(_, owner)| owner == 2));
    assert!(
        second
            .iter()
            .all(|(e, _)| !first.iter().any(|(f, _)| f == e)),
        "new entities, not the first attempt's"
    );

    // crash1 authors no `[Police]`: a copless lesson.
    let mut app = event_app(lesson_config(1), vfs_of(tmp.path()));
    app.update();
    assert_eq!(phase(&app), SessionPhase::Countdown);
    assert!(cops(&mut app).is_empty());
}

const OPP_HEADER: &str =
    "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\n";

/// A one-vehicle `.opp` route on the +Z axis from `z0` in 40 m steps.
fn lead_route(z0: f32, steps: usize) -> String {
    let mut s = OPP_HEADER.to_string();
    for i in 0..steps {
        s.push_str(&format!("20,0,{},0,0,0,0,0,0\n", z0 + 40.0 * i as f32));
    }
    s
}

/// A follow lesson's `[Opponent]` lead car (F21-B.19; retail london
/// crash3, sf crash6/7/10/11 among them) rides on the launch setup as
/// `lead_cars`, never as a race opponent, difficulty-selected, with its
/// `.opp` route resolved. The lesson table's `Opponents` column (0 on
/// every retail row) is not a count to disagree with.
#[test]
fn a_lesson_launch_carries_its_difficulty_selected_lead_cars() {
    use mm2_game::Difficulty;

    let tmp = lesson_install();
    write(
        tmp.path(),
        "race/london/crash0.aimap",
        "[Opponent]\n1\nvpcab slalom-0.opp 0.9 0 50.0 0.7 1 1 1 1 0 1.0\n",
    );
    write(
        tmp.path(),
        "race/london/crash0.aimap_p",
        "[Opponent]\n3\nvpcab slalom-0.opp 0.9 0 50.0 0.7 1 1 1 1 0 1.0\nvpford slalom-0.opp 0.9 0 50.0 0.7 1 1 1 1 0 1.0\nvpbullet Elsewhere-0.opp 0.9 0 50.0 0.7 1 1 1 1 0 1.0\n",
    );
    write(tmp.path(), "race/london/slalom-0.opp", &lead_route(30.0, 4));
    // A route no record of this lesson claims — resolved through the
    // VFS by its authored (mixed-case) name, not left dead.
    write(
        tmp.path(),
        "race/london/elsewhere-0.opp",
        &lead_route(90.0, 3),
    );
    let vfs = vfs_of(tmp.path());
    let event_ref = EventRef {
        city: "london".into(),
        table: EventTableKind::CrashCourse,
        index: 0,
    };
    let (setup, _) = race::lesson_launch(&vfs, &event_ref, Difficulty::Amateur).unwrap();
    assert_eq!(setup.lead_cars.entries.len(), 1);
    assert_eq!(setup.lead_cars.entries[0].vehicle, "vpcab");
    assert!(
        setup.lead_cars.entries[0]
            .route
            .as_ref()
            .is_some_and(|r| r.points.len() == 4),
        "the wired .opp resolved to its route"
    );
    assert!(
        setup.lead_cars.issues.is_empty(),
        "no table-count disagreement is reported: {:?}",
        setup.lead_cars.issues
    );
    assert!(
        setup.roster.entries.is_empty(),
        "a lead car is not a race opponent"
    );
    let (setup, _) = race::lesson_launch(&vfs, &event_ref, Difficulty::Professional).unwrap();
    assert_eq!(setup.lead_cars.entries.len(), 3, "the _p record wins");
    assert!(
        setup.lead_cars.entries[2]
            .route
            .as_ref()
            .is_some_and(|r| r.points.len() == 3),
        "a route outside the lesson's records resolves through the VFS"
    );
    assert!(
        setup.lead_cars.issues.is_empty(),
        "{:?}",
        setup.lead_cars.issues
    );

    // crash1 wires no opponent: no lead car.
    let other = EventRef {
        index: 1,
        ..event_ref
    };
    let (setup, _) = race::lesson_launch(&vfs, &other, Difficulty::Amateur).unwrap();
    assert!(setup.lead_cars.entries.is_empty());
}

/// The production loader fields the lead car as a session-owned AI car
/// standing at its route's staging pose, not a participant of the
/// lesson's race; once the countdown releases it drives its route while
/// the player stands still; a retry refields it under the new generation
/// without doubling, and a lesson with none fields none.
#[test]
fn a_lessons_lead_car_is_fielded_drives_its_route_and_refields_on_retry() {
    use mm2_app::opponents::OpponentDriver;

    let tmp = lesson_install();
    write(
        tmp.path(),
        "race/london/crash0.aimap",
        "[Opponent]\n1\nvpcab slalom-0.opp 0.9 0 50.0 0.7 1 1 1 1 0 1.0\n",
    );
    write(tmp.path(), "race/london/slalom-0.opp", &lead_route(30.0, 6));
    crate::support::tuned_car(tmp.path(), "vpcab", 1500.0);
    let leads = |app: &mut App| -> Vec<(Entity, u64, Vec3)> {
        app.world_mut()
            .query_filtered::<(Entity, &SessionEntity, &Position), With<OpponentDriver>>()
            .iter(app.world())
            .map(|(e, owner, pos)| (e, owner.0, pos.0))
            .collect()
    };

    let mut app = event_app(lesson_config(0), vfs_of(tmp.path()));
    app.add_systems(Update, mm2_app::opponents::opponent_drive);
    app.update();
    assert_eq!(phase(&app), SessionPhase::Countdown);
    let first = leads(&mut app);
    assert_eq!(first.len(), 1, "the authored row became a car");
    assert_eq!(first[0].1, 1);
    // Staged at the route's row 0 (x=20, z=30), never a grid slot.
    assert!(
        (first[0].2.x - 20.0).abs() < 1.0 && (first[0].2.z - 30.0).abs() < 1.0,
        "lead car stands at its staging pose, got {:?}",
        first[0].2
    );
    // Not a participant: only the player carries progress.
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<RaceProgress>>()
            .iter(app.world())
            .count(),
        1,
        "only the player carries progress"
    );

    // Held through the countdown, driving once the race is released.
    let mut moved = 0.0_f32;
    for _ in 0..400 {
        app.update();
        if phase(&app) == SessionPhase::Playing {
            let now = leads(&mut app)[0].2;
            moved = moved.max(now.distance(first[0].2));
        }
    }
    assert_eq!(phase(&app), SessionPhase::Playing);
    assert!(
        moved > 3.0,
        "the lead car drove its route once released (moved {moved})"
    );
    let driver_stats = app
        .world_mut()
        .query::<&OpponentDriver>()
        .single(app.world())
        .unwrap()
        .next;
    assert!(driver_stats > 0, "the route cursor advanced");

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
    assert!(reached, "restart never returned to Countdown");
    let second = leads(&mut app);
    assert_eq!(second.len(), 1, "refielded, none doubled or left over");
    assert_eq!(second[0].1, 2);
    assert_ne!(second[0].0, first[0].0, "a new entity, not the first's");
    assert!(
        second[0].2.distance(first[0].2) < 1.0,
        "back at its staging pose, got {:?}",
        second[0].2
    );

    // crash1 wires no opponent.
    let mut app = event_app(lesson_config(1), vfs_of(tmp.path()));
    app.update();
    assert_eq!(phase(&app), SessionPhase::Countdown);
    assert!(leads(&mut app).is_empty());
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

/// Original-content validation (opt-in: `MM2_RETAIL` names an install):
/// every `[Opponent]` row a retail lesson's own aimap wires, at both
/// difficulties of both cities, becomes a lead car on the launch setup
/// with its `.opp` route resolved and drivable — including the routes
/// the lesson's catalog records do not carry (london crash11's reused
/// `crash3-0.opp`, sf `race0-a-6.opp`, `Follow-1.opp`). The denominator
/// is the catalog's own wiring audit (`crash_lesson`), not the roster
/// builder's output.
#[test]
fn every_retail_lesson_wired_opponent_becomes_a_lead_car_with_a_route() {
    use mm2_assets::{InstallMount, mount_install};
    use mm2_game::Difficulty;

    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("MM2_RETAIL unset: retail lead-car sweep NOT run");
        return;
    };
    let mut vfs = Vfs::new();
    mount_install(&mut vfs, &retail, &InstallMount::default()).unwrap();
    let (mut wired, mut fielded, mut routed) = (0, 0, 0);
    let mut failures = Vec::new();
    for city in ["london", "sf"] {
        let catalog = mm2_content::EventCatalog::scan(&vfs, city);
        for event in catalog
            .events
            .iter()
            .filter(|e| e.event_ref.table == EventTableKind::CrashCourse)
        {
            let lesson = mm2_content::crash_lesson(&vfs, &catalog, event);
            for (slot, difficulty) in [Difficulty::Amateur, Difficulty::Professional]
                .into_iter()
                .enumerate()
            {
                let label = format!("{city} crash:{} {difficulty:?}", event.event_ref.index);
                let authored = lesson.wiring[slot]
                    .as_ref()
                    .map_or(0, |w| w.opponents.len());
                let setup = race::lesson_race_setup(&vfs, &event.event_ref, difficulty).unwrap();
                wired += authored;
                fielded += setup.lead_cars.entries.len();
                routed += setup
                    .lead_cars
                    .entries
                    .iter()
                    .filter(|e| e.route.is_some())
                    .count();
                if setup.lead_cars.entries.len() != authored {
                    failures.push(format!(
                        "{label}: {authored} wired, {} fielded",
                        setup.lead_cars.entries.len()
                    ));
                }
                for issue in &setup.lead_cars.issues {
                    eprintln!("{label}: roster issue: {issue}");
                }
            }
        }
    }
    eprintln!("retail lead cars: wired {wired}, fielded {fielded}, routed {routed}");
    assert!(wired > 0, "retail wires lesson opponents");
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(wired, routed, "every wired lead car has a drivable route");
}

/// F21-AC05, vehicle half: a retry hands back a fresh car, not the
/// wrecked one. After the player drives off, spins and shifts up, the
/// restart despawns that body and the lesson relaunches a new one on
/// leg 0's start pose — at rest, in first gear, idling.
#[test]
fn a_retried_lesson_restores_the_vehicle_to_its_start_state() {
    let tmp = lesson_install();
    let mut app = event_app(lesson_config(1), vfs_of(tmp.path()));
    app.update();
    let first_car = car(&mut app);
    let start = app.world().get::<Transform>(first_car).unwrap().translation;
    let start_rot = app.world().get::<Transform>(first_car).unwrap().rotation;

    // Wreck the first attempt: far away, fast, tumbling, geared up.
    {
        let mut entity = app.world_mut().entity_mut(first_car);
        entity.get_mut::<Transform>().unwrap().translation = Vec3::new(300.0, 4.0, -120.0);
        *entity.get_mut::<LinearVelocity>().unwrap() = LinearVelocity(Vec3::new(30.0, 5.0, 8.0));
        *entity.get_mut::<AngularVelocity>().unwrap() = AngularVelocity(Vec3::new(2.0, 3.0, 1.0));
        let mut state = entity.get_mut::<mm2_vehicle::VehicleState>().unwrap();
        state.gear = 3;
        state.rpm = 6500.0;
        state.forward_speed = 30.0;
        state.upended_for = 4.0;
    }
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

    let second_car = car(&mut app);
    assert_ne!(first_car, second_car, "the wrecked body is gone");
    let world = app.world();
    let at = world.get::<Transform>(second_car).unwrap();
    assert!(
        at.translation.distance(start) < 0.05,
        "retry pose {:?} vs first launch {start:?}",
        at.translation
    );
    assert!(at.rotation.angle_between(start_rot) < 0.01);
    assert!(world.get::<LinearVelocity>(second_car).unwrap().0.length() < 0.1);
    assert!(world.get::<AngularVelocity>(second_car).unwrap().0.length() < 0.1);
    let state = world.get::<mm2_vehicle::VehicleState>(second_car).unwrap();
    assert_eq!(state.gear, 0);
    assert_eq!(state.upended_for, 0.0);
    assert!(state.forward_speed.abs() < 0.1);
    assert!(state.rpm < 2500.0, "idling, not {} rpm", state.rpm);
    assert_eq!(
        world.get::<RaceProgress>(second_car).unwrap().state,
        ParticipantState::AwaitingStart,
        "the new body has no carried-over race progress"
    );
    // Exactly one player car survives the turnover.
    let mut players = app
        .world_mut()
        .query_filtered::<Entity, With<PlayerVehicle>>();
    assert_eq!(players.iter(app.world()).count(), 1);
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

/// F21-AC05, vehicle half, on original data: the lesson's required
/// stock vehicle (`vpcab` / `vpbullet`, loaded through the production
/// `load_vehicle`) comes back from a retry with its authored damage
/// accumulator empty and every breakaway part attached — a wrecked,
/// part-shedding first attempt carries nothing over. Opt-in
/// (`MM2_RETAIL`); reports "not run" otherwise.
#[test]
fn a_retried_retail_lesson_hands_back_a_repaired_stock_vehicle() {
    use mm2_assets::{InstallMount, mount_install};
    use mm2_game::{DamageTier, ImpactId, VehicleDamage};

    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("MM2_RETAIL unset: retail lesson vehicle-repair retry NOT run");
        return;
    };
    let mut failures = Vec::new();
    let mut checked = 0;
    let mut parts_shed = 0;
    for city in ["london", "sf"] {
        let mut vfs = Vfs::new();
        mount_install(&mut vfs, &retail, &InstallMount::default()).unwrap();
        let id = mm2_content::required_vehicle(city).expect("both schools name a vehicle");
        let def = mm2_content::load_vehicle(&vfs, id, 0).unwrap();
        let label = format!("{city} ({id})");
        if def.damage.is_none() {
            failures.push(format!("{label}: no authored vehcardamage to reset"));
            continue;
        }
        let config = SessionConfig {
            world: WorldMode::City {
                psdl: format!("city/{city}.psdl"),
            },
            mode: SessionMode::Event(EventRef {
                city: city.into(),
                table: EventTableKind::CrashCourse,
                index: 0,
            }),
            ..SessionConfig::default()
        };
        let selected = session::SelectedCar {
            def: Some(def),
            paint: 0,
        };
        let mut app = event_app_with_car(config, vfs, selected);
        app.update();
        if phase(&app) != SessionPhase::Countdown {
            failures.push(format!("{label}: first launch phase {:?}", phase(&app)));
            continue;
        }
        let first_car = car(&mut app);

        // Wreck it: drive the real accumulator past its destruction
        // bound and shed every breakaway part the vehicle authors.
        {
            let mut entity = app.world_mut().entity_mut(first_car);
            let Some(mut damage) = entity.get_mut::<VehicleDamage>() else {
                failures.push(format!("{label}: spawned without a VehicleDamage"));
                continue;
            };
            let max = damage.spec.max_damage;
            damage.apply(ImpactId(1), max * 2.0);
            if damage.condition() != DamageTier::Disabled || damage.total() <= 0.0 {
                failures.push(format!("{label}: wrecking did not disable the car"));
                continue;
            }
            if let Some(mut breaks) = entity.get_mut::<VehicleBreaks>() {
                for index in 0..breaks.parts.len() {
                    breaks.detach(index, None);
                }
                parts_shed += breaks.detached_count();
            }
        }

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
        let second_car = car(&mut app);
        if second_car == first_car {
            failures.push(format!("{label}: the wrecked body survived the retry"));
        }
        let world = app.world();
        match world.get::<VehicleDamage>(second_car) {
            Some(d) => {
                if d.total() != 0.0
                    || d.condition() != DamageTier::Intact
                    || d.health_fraction() != 1.0
                {
                    failures.push(format!(
                        "{label}: damage carried over (total {}, {:?})",
                        d.total(),
                        d.condition()
                    ));
                }
            }
            None => failures.push(format!("{label}: retry car has no VehicleDamage")),
        }
        if let Some(breaks) = world.get::<VehicleBreaks>(second_car)
            && breaks.detached_count() != 0
        {
            failures.push(format!(
                "{label}: {} parts still detached",
                breaks.detached_count()
            ));
        }
        checked += 1;
    }
    eprintln!(
        "retail lesson vehicle repair: checked {checked}/2, {parts_shed} parts shed on first attempts"
    );
    assert!(failures.is_empty(), "{failures:#?}");
    assert_eq!(checked, 2);
}
