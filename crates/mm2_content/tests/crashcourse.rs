//! Crash Course lesson catalog tests (F21-A.1): a synthetic
//! `race/<city>/` install mounted through the real VFS, scanned by the
//! production `EventCatalog` → `CourseCatalog` chain.

use std::path::Path;

use mm2_assets::Vfs;
use mm2_content::{
    CourseCatalog, EventCatalog, EventStatus, LessonObjective, LessonStage, LessonTableRole,
};

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n1,2,3,4,15,0,0,0,\n5,6,7,8,10,0,0,0,\n";
const OPP: &str = "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\n1,2,3,175,0,0,0,0,0\n";
const CRASHDATA: &str =
    "Filename,Event,Checkpoints,TimeLimit,AmbDensity,extra,extra,extra,extra,etra,\n";

fn write(dir: &Path, rel: &str, contents: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

fn course(dir: &Path) -> (Vfs, CourseCatalog) {
    let vfs = vfs_of(dir);
    let catalog = EventCatalog::scan(&vfs, "london");
    let course = CourseCatalog::scan(&vfs, &catalog);
    (vfs, course)
}

/// Three lessons: a complete crash0 (aimap + both data tables + linked
/// waypoint + wired lead car + `_crash0` override pathset + reward), a
/// crash1 missing its professional table, and a crash2 whose sub-event
/// links a waypoint CSV that does not exist.
fn synthetic_install() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    write(
        dir,
        "race/london/mmcrashdata.csv",
        &format!(
            "{MM_HEADER}\n\
             lesson1,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1\n\
             midtrm1,0,1,0,0,0,0,0,0,0,1,0,1,0,0,0,0,0,0,0,1\n\
             final13,0,3,1,0,0,0,0,0,0,1,0,3,2,0,0,0,0,0,0,1\n"
        ),
    );
    // crash0: complete follow-style lesson.
    write(
        dir,
        "race/london/crash0.aimap",
        "[Police]\n1\nvpcab 1 2 3 4 5 6\n[Opponent]\n1\nvpcab Follow-1.opp 1.00 0 50.0 0.7 1 1 1 1 0 1.0\n[CopChaseDistance]\n150\n",
    );
    write(
        dir,
        "race/london/crash0.aimap_p",
        "[Opponent]\n1\nvpbullet Follow-1.opp 1.00 0 50.0 0.7 1 1 1 1 0 1.0\n",
    );
    write(
        dir,
        "race/london/crash0data.csv",
        &format!("{CRASHDATA}follow,2,1,36,0,0,0,1,0,0,0\n"),
    );
    write(
        dir,
        "race/london/crash0data_p.csv",
        &format!("{CRASHDATA}follow,2,1,30,0.2,0,0,1,0,0,0\n"),
    );
    write(dir, "race/london/follow.csv", WAYPOINTS);
    write(dir, "race/london/follow-1.opp", OPP);
    // `<object>_crash<N>` override — surfaces as an extra the lesson
    // attribution claims.
    write_bytes(
        dir,
        "race/london/london_parkedcar_crash0.pathset",
        b"PTH1\0\0\0\0\0\0\0\0",
    );
    write(
        dir,
        "race/london/london_rewards.csv",
        "RaceType,RaceNum,CarName,VariantNum (zero if it unlocks a car),\n\
         crash,3,vpvwcup,6,Congrats!  Mystery paint job,\n",
    );
    // crash1: incomplete — no data_p table, no waypoints.
    write(dir, "race/london/crash1.aimap", "[Opponent]\n0\n");
    write(
        dir,
        "race/london/crash1data.csv",
        &format!("{CRASHDATA}ghost,4,1,90,0,50,0,0,0,0,0\n"),
    );
    write(dir, "race/london/ghost.csv", WAYPOINTS);
    // crash2: complete on records but links a missing waypoint CSV.
    write(dir, "race/london/crash2.aimap", "[Opponent]\n0\n");
    write(
        dir,
        "race/london/crash2data.csv",
        &format!("{CRASHDATA}nowhere,0,1,26,0,0,0,0,0,0,0\n"),
    );
    write(
        dir,
        "race/london/crash2data_p.csv",
        &format!("{CRASHDATA}nowhere,0,1,20,0,0,0,0,0,0,0\n"),
    );
    d
}

fn write_bytes(dir: &Path, rel: &str, content: &[u8]) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

#[test]
fn lesson_stage_parses_authored_tags() {
    assert_eq!(LessonStage::parse("lesson3"), LessonStage::Lesson(3));
    assert_eq!(LessonStage::parse("midtrm1"), LessonStage::Midterm(1));
    assert_eq!(LessonStage::parse("final13"), LessonStage::Final(13));
    assert_eq!(
        LessonStage::parse("none"),
        LessonStage::Other("none".into())
    );
    assert_eq!(
        LessonStage::parse("lesson"),
        LessonStage::Other("lesson".into())
    );
}

#[test]
fn objective_decode_covers_the_retail_enum() {
    assert_eq!(LessonObjective::from_code(0), LessonObjective::Jump);
    assert_eq!(LessonObjective::from_code(2), LessonObjective::Follow);
    assert_eq!(LessonObjective::from_code(3), LessonObjective::CopChase);
    assert_eq!(LessonObjective::from_code(4), LessonObjective::Corner);
    assert_eq!(LessonObjective::from_code(5), LessonObjective::CrossTraffic);
    assert_eq!(LessonObjective::from_code(7), LessonObjective::Maneuver);
    assert_eq!(LessonObjective::from_code(8), LessonObjective::Stop);
    assert_eq!(LessonObjective::from_code(9), LessonObjective::Map);
    // 1 and 6 ship nowhere on retail — kept raw, never guessed.
    assert_eq!(LessonObjective::from_code(1), LessonObjective::Unknown(1));
    assert_eq!(LessonObjective::from_code(6), LessonObjective::Unknown(6));
    assert_eq!(LessonObjective::from_code(42), LessonObjective::Unknown(42));
    assert_eq!(LessonObjective::CopChase.label(), "cop-chase");
    assert_eq!(LessonObjective::Unknown(6).label(), "unknown(6)");
}

#[test]
fn complete_lesson_builds_the_full_view() {
    let d = synthetic_install();
    let (_vfs, course) = course(d.path());
    assert_eq!(course.lessons.len(), 3);
    let lesson = &course.lessons[0];
    assert_eq!(lesson.stem, "crash0");
    assert_eq!(lesson.stage, LessonStage::Lesson(1));
    assert!(matches!(lesson.status, EventStatus::Ready));

    // Both difficulty tables, amateur first.
    assert_eq!(lesson.tables.len(), 2);
    assert_eq!(lesson.tables[0].role, LessonTableRole::Amateur);
    assert_eq!(lesson.tables[1].role, LessonTableRole::Professional);
    assert!(lesson.issues.is_empty(), "{:?}", lesson.issues);

    let sub = &lesson.tables[0].sub_events[0];
    assert_eq!(sub.filename, "follow");
    assert_eq!(sub.objective, LessonObjective::Follow);
    assert_eq!(sub.resolved.as_deref(), Some("race/london/follow.csv"));
    assert_eq!(sub.time_limit, 36.0);
    assert!(sub.timed());
    // The professional row carries its own limits.
    assert_eq!(lesson.tables[1].sub_events[0].time_limit, 30.0);

    // Aimap wiring: amateur picks crash0.aimap (police + opponent +
    // chase distance), professional falls back to crash0.aimap_p's
    // own lineup.
    let am = lesson.wiring[0].as_ref().unwrap();
    assert_eq!(am.logical, "race/london/crash0.aimap");
    assert_eq!(am.police, 1);
    assert_eq!(am.police_vehicles, vec!["vpcab".to_string()]);
    assert_eq!(am.cop_chase_distance, Some(150.0));
    assert_eq!(am.opponents.len(), 1);
    assert_eq!(am.opponents[0].vehicle, "vpcab");
    // The wire resolves case-insensitively to the real record.
    assert_eq!(
        am.opponents[0].resolved.as_deref(),
        Some("race/london/follow-1.opp")
    );
    let pro = lesson.wiring[1].as_ref().unwrap();
    assert_eq!(pro.logical, "race/london/crash0.aimap_p");
    assert_eq!(pro.opponents[0].vehicle, "vpbullet");

    // The `<object>_crash0` extra is attributed to the lesson and
    // leaves the unclaimed denominator.
    assert_eq!(lesson.override_records.len(), 1);
    assert_eq!(lesson.override_records[0].label, "london_parkedcar_crash0");
    assert!(
        course
            .remaining_extras
            .iter()
            .all(|x| x.label != "london_parkedcar_crash0")
    );
}

#[test]
fn incomplete_and_unresolved_lessons_report_not_fail() {
    let d = synthetic_install();
    let (_vfs, course) = course(d.path());

    // crash1: missing its professional table and its sub-event's
    // required waypoint claim — both surfaces honestly.
    let l1 = &course.lessons[1];
    assert_eq!(l1.stage, LessonStage::Midterm(1));
    assert!(matches!(l1.status, EventStatus::Incomplete { .. }));
    assert!(l1.issues.iter().any(|i| i.contains("data_p")));

    // crash2: complete record set, but the Filename link points at a
    // CSV that does not exist — a runtime issue the catalog's own
    // completeness check also reports.
    let l2 = &course.lessons[2];
    assert_eq!(l2.stage, LessonStage::Final(13));
    assert!(l2.issues.iter().any(|i| i.contains("nowhere.csv")));
    assert!(l2.tables[0].sub_events[0].resolved.is_none());

    let failures = course.failures();
    assert!(failures.iter().any(|f| f.contains("crash1")));
    assert!(failures.iter().any(|f| f.contains("nowhere.csv")));
}

#[test]
fn a_missing_course_table_yields_no_lessons() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "race/london/mmblitzdata.csv",
        &format!("{MM_HEADER}\nnone,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1\n"),
    );
    let (_vfs, course) = course(d.path());
    assert!(course.lessons.is_empty());
    assert!(
        course
            .failures()
            .iter()
            .any(|f| f.contains("no crash-course lessons"))
    );
}
