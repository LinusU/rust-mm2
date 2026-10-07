//! Crash Course lesson legs (F21-B.1): a synthetic `race/london/`
//! install mounted through the real VFS, scanned by the production
//! `EventCatalog` → `CourseCatalog` chain, then built into runnable
//! [`LessonLeg`]s.

use std::path::Path;

use mm2_assets::Vfs;
use mm2_content::{
    CourseCatalog, EventCatalog, LessonBuildError, LessonObjective, lesson_legs,
    race_def::RaceBuildError,
};
use mm2_game::{CheckpointRule, Difficulty, RACE_TICK_HZ, RaceError};

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WP_HEADER: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";
const CRASHDATA: &str =
    "Filename,Event,Checkpoints,TimeLimit,AmbDensity,extra,extra,extra,extra,etra,\n";

fn write(dir: &Path, rel: &str, contents: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

/// A lesson whose amateur and professional tables are `am`/`pro` data
/// rows (`Filename,Event,Checkpoints,TimeLimit,AmbDensity,tail...`).
fn lesson(dir: &Path, stem: &str, am: &str, pro: &str) {
    write(dir, &format!("race/london/{stem}.aimap"), "[Opponent]\n0\n");
    write(
        dir,
        &format!("race/london/{stem}data.csv"),
        &format!("{CRASHDATA}{am}"),
    );
    write(
        dir,
        &format!("race/london/{stem}data_p.csv"),
        &format!("{CRASHDATA}{pro}"),
    );
}

/// crash0 slalom (2 rows, timed, density differs per difficulty);
/// crash1 an exam of two legs (4-row timed `gates`, 3-row untimed
/// `free`); crash2 a one-row waypoint file; crash3 a negative limit;
/// crash4 a leg linking a waypoint CSV that does not exist.
fn install() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    write(
        dir,
        "race/london/mmcrashdata.csv",
        &format!(
            "{MM_HEADER}\n\
             lesson1,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1\n\
             midtrm1,0,1,2,0,0,0.5,0,0,0,1,0,3,1,0,0,0.5,0,0,0,1\n\
             lesson2,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1\n\
             lesson3,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1\n\
             lesson4,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1\n"
        ),
    );
    lesson(
        dir,
        "crash0",
        "slalom,7,1,12,0.05,0,0,0,0,0,0\n",
        "slalom,7,1,11,0.30,0,0,0,0,0,0\n",
    );
    write(
        dir,
        "race/london/slalom.csv",
        &format!("{WP_HEADER}10,1,20,90,15,0,0,0,\n10,1,60,-30,16,0,0,0,\n"),
    );
    lesson(
        dir,
        "crash1",
        "gates,4,1,35,0,40,0,0,0,0\nfree,7,1,0,0,0,0,0,0,0\n",
        "gates,4,1,30,0,40,0,0,0,0\nfree,7,1,0,0,0,0,0,0,0\n",
    );
    write(
        dir,
        "race/london/gates.csv",
        &format!(
            "{WP_HEADER}0,0,0,0,10,0,0,0,\n0,0,40,0,9,0,0,0,\n30,0,80,0,8,0,0,0,\n60,0,80,0,7,0,0,0,\n"
        ),
    );
    write(
        dir,
        "race/london/free.csv",
        &format!("{WP_HEADER}5,0,5,0,6,0,0,0,\n5,0,25,0,6,0,0,0,\n5,0,45,0,6,0,0,0,\n"),
    );
    lesson(
        dir,
        "crash2",
        "lonely,7,1,20,0,0,0,0,0,0\n",
        "lonely,7,1,20,0,0,0,0,0,0\n",
    );
    write(
        dir,
        "race/london/lonely.csv",
        &format!("{WP_HEADER}0,0,0,0,10,0,0,0,\n"),
    );
    lesson(
        dir,
        "crash3",
        "back,7,1,-5,0,0,0,0,0,0\n",
        "back,7,1,10,0,0,0,0,0,0\n",
    );
    write(
        dir,
        "race/london/back.csv",
        &format!("{WP_HEADER}0,0,0,0,10,0,0,0,\n0,0,40,0,9,0,0,0,\n"),
    );
    lesson(
        dir,
        "crash4",
        "nowhere,7,1,20,0,0,0,0,0,0\n",
        "nowhere,7,1,20,0,0,0,0,0,0\n",
    );
    d
}

fn scanned(dir: &Path) -> (EventCatalog, CourseCatalog) {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    let catalog = EventCatalog::scan(&vfs, "london");
    let course = CourseCatalog::scan(&vfs, &catalog);
    (catalog, course)
}

#[test]
fn a_timed_leg_is_a_one_pass_ordered_gate_run_ending_at_the_last_row() {
    let d = install();
    let (catalog, course) = scanned(d.path());
    let legs = lesson_legs(&catalog, &course.lessons[0], Difficulty::Amateur).unwrap();
    assert_eq!(legs.len(), 1);
    let leg = &legs[0];
    assert_eq!(leg.filename, "slalom");
    assert_eq!(leg.objective, LessonObjective::Maneuver);
    assert_eq!(leg.source, "crash0:2");

    let def = &leg.definition;
    // Row 0 is the start pose, row 1 the only gate (the finish).
    assert_eq!(def.rule, CheckpointRule::Ordered);
    assert_eq!(def.laps, 1);
    assert_eq!(def.checkpoints.len(), 1);
    assert_eq!(def.finish, None);
    assert_eq!(def.checkpoints[0].center.to_array(), [10.0, 1.0, 60.0]);
    assert_eq!(def.checkpoints[0].radius, 16.0);
    // The authored TimeLimit, seconds → fixed ticks.
    assert_eq!(def.time_limit_ticks, Some(12 * RACE_TICK_HZ));
    // The player starts on row 0, facing row 1 (+Z → yaw 180°).
    assert_eq!(def.start_slots.len(), 1);
    assert_eq!(def.start_slots[0].position.to_array(), [10.0, 1.0, 20.0]);
    let yaw = def.start_slots[0].yaw_deg.unwrap();
    assert!((yaw.abs() - 180.0).abs() < 1e-3, "{yaw}");
    // The leg's density is the sub-event's AmbDensity, not mmcrashdata's.
    assert_eq!(def.params.densities.traffic, 0.05);
    assert_eq!(def.params.opponents, 0);
    assert_eq!(def.params.cops, 0);
}

#[test]
fn the_professional_table_builds_its_own_limit_and_density() {
    let d = install();
    let (catalog, course) = scanned(d.path());
    let legs = lesson_legs(&catalog, &course.lessons[0], Difficulty::Professional).unwrap();
    let def = &legs[0].definition;
    assert_eq!(def.time_limit_ticks, Some(11 * RACE_TICK_HZ));
    assert_eq!(def.params.densities.traffic, 0.30);
}

#[test]
fn an_exam_yields_every_leg_in_authored_order_with_untimed_legs_untimed() {
    let d = install();
    let (catalog, course) = scanned(d.path());
    let exam = &course.lessons[1];
    let legs = lesson_legs(&catalog, exam, Difficulty::Amateur).unwrap();
    let names: Vec<&str> = legs.iter().map(|l| l.filename.as_str()).collect();
    assert_eq!(names, ["gates", "free"]);
    assert_eq!(legs[0].objective, LessonObjective::Corner);
    // Rows 1..: three gates in the authored order, the last the finish.
    let zs: Vec<f32> = legs[0]
        .definition
        .checkpoints
        .iter()
        .map(|c| c.center.z)
        .collect();
    assert_eq!(zs, [40.0, 80.0, 80.0]);
    assert_eq!(legs[0].definition.checkpoints[2].center.x, 60.0);
    assert_eq!(legs[0].definition.time_limit_ticks, Some(35 * RACE_TICK_HZ));
    // `TimeLimit 0` is untimed, not a zero-tick race.
    assert_eq!(legs[1].definition.time_limit_ticks, None);
    assert_eq!(legs[1].definition.checkpoints.len(), 2);
    // The per-leg conditions come from the lesson's authored block of
    // the chosen difficulty.
    assert_eq!(legs[0].definition.params.conditions.weather.get(), 2);
    let pro = lesson_legs(&catalog, exam, Difficulty::Professional).unwrap();
    assert_eq!(pro[0].definition.params.conditions.weather.get(), 1);
    assert_eq!(pro[0].definition.time_limit_ticks, Some(30 * RACE_TICK_HZ));
}

#[test]
fn a_one_row_waypoint_file_is_a_named_error_not_an_empty_course() {
    let d = install();
    let (catalog, course) = scanned(d.path());
    let err = lesson_legs(&catalog, &course.lessons[2], Difficulty::Amateur).unwrap_err();
    match err {
        LessonBuildError::Leg {
            source_row,
            error: RaceBuildError::TooFewRows { needed, found },
        } => {
            assert_eq!(source_row, "crash2:2");
            assert_eq!((needed, found), (2, 1));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_negative_limit_fails_the_leg_but_the_other_difficulty_still_builds() {
    let d = install();
    let (catalog, course) = scanned(d.path());
    let bad = &course.lessons[3];
    match lesson_legs(&catalog, bad, Difficulty::Amateur).unwrap_err() {
        LessonBuildError::Leg {
            error: RaceBuildError::BadParam { field, value },
            ..
        } => {
            assert_eq!(field, "TimeLimit");
            assert_eq!(value, "-5");
        }
        other => panic!("{other:?}"),
    }
    // Each difficulty reads its own table.
    let pro = lesson_legs(&catalog, bad, Difficulty::Professional).unwrap();
    assert_eq!(pro[0].definition.time_limit_ticks, Some(10 * RACE_TICK_HZ));
}

#[test]
fn a_lesson_that_is_not_ready_builds_nothing() {
    let d = install();
    let (catalog, course) = scanned(d.path());
    // crash4 links a waypoint CSV that does not exist.
    assert!(matches!(
        lesson_legs(&catalog, &course.lessons[4], Difficulty::Amateur),
        Err(LessonBuildError::NotReady(_))
    ));
}

#[test]
fn a_lesson_outside_the_catalog_is_refused() {
    let d = install();
    let (_, course) = scanned(d.path());
    let other = tempfile::tempdir().unwrap();
    write(
        other.path(),
        "race/london/mmcrashdata.csv",
        &format!("{MM_HEADER}\n"),
    );
    let (empty, _) = scanned(other.path());
    assert!(matches!(
        lesson_legs(&empty, &course.lessons[0], Difficulty::Amateur),
        Err(LessonBuildError::NotInCatalog(_))
    ));
}

#[test]
fn a_zero_width_gate_is_rejected_by_the_shared_validation() {
    let d = install();
    write(
        d.path(),
        "race/london/back.csv",
        &format!("{WP_HEADER}0,0,0,0,10,0,0,0,\n0,0,0,0,0,0,0,0,\n"),
    );
    let (catalog, course) = scanned(d.path());
    // A zero gate half-width is the shared contract's BadExtent.
    match lesson_legs(&catalog, &course.lessons[3], Difficulty::Professional).unwrap_err() {
        LessonBuildError::Leg {
            error: RaceBuildError::Invalid(RaceError::BadExtent),
            ..
        } => {}
        other => panic!("{other:?}"),
    }
}
