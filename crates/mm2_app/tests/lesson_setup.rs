//! F21-B: a Crash Course row resolves through the production catalog
//! into a lesson's legs and sequencer — a synthetic `race/london/`
//! install mounted through the real VFS.

use std::path::Path;

use mm2_app::race::{LessonSetupError, event_race_setup, lesson_race_setup};
use mm2_assets::Vfs;
use mm2_game::{
    Difficulty, EventRef, EventTableKind, LegReport, LessonPhase, ParticipantState, PlayerId,
    ReportEffect, ResultId,
};

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
fn install() -> tempfile::TempDir {
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

fn vfs(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

fn crash(index: usize) -> EventRef {
    EventRef {
        city: "london".into(),
        table: EventTableKind::CrashCourse,
        index,
    }
}

#[test]
fn a_crash_row_resolves_into_its_legs_and_a_first_attempt_sequencer() {
    let d = install();
    let setup = lesson_race_setup(&vfs(d.path()), &crash(1), Difficulty::Amateur).unwrap();
    assert_eq!(setup.key.stem, "crash1");
    assert_eq!(setup.key.table, EventTableKind::CrashCourse);
    assert_eq!(setup.legs.len(), 2);
    assert_eq!(setup.legs[0].filename, "gates");
    assert_eq!(setup.legs[1].filename, "free");
    assert_eq!(setup.run.leg_count(), 2);
    assert_eq!(setup.run.attempt(), 1);
    assert_eq!(setup.run.phase(), LessonPhase::Running { leg: 0 });
}

#[test]
fn the_setup_sequencer_takes_the_gate_run_verdicts_of_its_legs() {
    let d = install();
    let mut setup = lesson_race_setup(&vfs(d.path()), &crash(1), Difficulty::Professional).unwrap();
    let finished = |race_ticks| ParticipantState::Finished {
        race_ticks,
        result: ResultId {
            generation: 1,
            participant: PlayerId(0),
            event: None,
            sequence: 0,
        },
    };
    let attempt = setup.run.attempt();
    let first = LegReport::from_gate_run(attempt, 0, &finished(300), false).unwrap();
    assert_eq!(
        setup.run.report(first),
        ReportEffect::Advanced { next_leg: 1 }
    );
    let last = LegReport::from_gate_run(attempt, 1, &finished(200), false).unwrap();
    assert_eq!(setup.run.report(last), ReportEffect::Passed);
    assert_eq!(setup.run.take_pass().unwrap().total_ticks(), 500);
}

#[test]
fn a_lesson_with_an_unbuildable_leg_is_refused_whole() {
    let d = install();
    let err = lesson_race_setup(&vfs(d.path()), &crash(2), Difficulty::Amateur)
        .err()
        .expect("crash2 links a missing waypoint file");
    // Resolution already reports the dangling waypoint link; the
    // leg-build refusals (negative limit, one-row file) are covered
    // against `lesson_legs` itself.
    assert!(
        matches!(
            err,
            LessonSetupError::Resolve(mm2_content::EventResolveError::Incomplete { .. })
        ),
        "{err:?}"
    );
}

#[test]
fn a_non_crash_row_is_not_a_lesson_and_a_crash_row_is_not_an_event_race() {
    let d = install();
    let v = vfs(d.path());
    let err = lesson_race_setup(
        &v,
        &EventRef {
            city: "london".into(),
            table: EventTableKind::Checkpoint,
            index: 0,
        },
        Difficulty::Amateur,
    )
    .err()
    .expect("no checkpoint table in the fixture");
    assert!(matches!(
        err,
        LessonSetupError::Resolve(_) | LessonSetupError::NotCrashCourse(..)
    ));
    // The gate-run event path still refuses a lesson row: a lesson is
    // not a single race.
    assert!(event_race_setup(&v, &crash(0), Difficulty::Amateur).is_err());
    assert!(matches!(
        lesson_race_setup(&v, &crash(9), Difficulty::Amateur),
        Err(LessonSetupError::Resolve(_))
    ));
}
