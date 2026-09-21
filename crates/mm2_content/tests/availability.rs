//! F16-B availability production: `availability_table` maps the
//! catalog's authored rows into gates — checkpoint sets of three
//! (CHK-2/CHK-3), crash-course lesson→midterm→final tags (CC-2/CC-3),
//! everything else open.

use std::path::Path;

use mm2_assets::Vfs;
use mm2_content::{EventCatalog, availability_table};
use mm2_game::{EventGate, EventTableKind};

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const ROW: &str = "none,0,0,0,0,0,0.1,0.0,0,50,1,0,0,0,0,0,0.2,0.0,0,40,1";

fn write(dir: &Path, rel: &str, contents: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

/// Table rows only — the availability surface derives from the
/// authored tables, so record completeness does not matter here.
fn synthetic_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();

    // Four checkpoint rows: an open first set and a partial second.
    write(
        d,
        "race/testcity/mmracedata.csv",
        &format!("{MM_HEADER}\n{ROW}\n{ROW}\n{ROW}\n{ROW}\n"),
    );
    write(
        d,
        "race/testcity/mmblitzdata.csv",
        &format!("{MM_HEADER}\n{ROW}\n{ROW}\n"),
    );
    write(
        d,
        "race/testcity/mmcircuitdata.csv",
        &format!("{MM_HEADER}\n{ROW}\n"),
    );
    // lesson1-3, midtrm1, lesson4, midtrm2 (only one group-2 lesson
    // authored), an unrecognized row, final13.
    let crash_row = |desc: &str| format!("{desc},0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1");
    write(
        d,
        "race/testcity/mmcrashdata.csv",
        &format!(
            "{MM_HEADER}\n{}\n",
            [
                "lesson1", "lesson2", "lesson3", "midtrm1", "lesson4", "midtrm2", "bogus",
                "final13",
            ]
            .map(crash_row)
            .join("\n")
        ),
    );
    tmp
}

fn catalog() -> (tempfile::TempDir, EventCatalog) {
    let tmp = synthetic_install();
    let mut vfs = Vfs::new();
    vfs.mount_dir(tmp.path(), 0).unwrap();
    let cat = EventCatalog::scan(&vfs, "testcity");
    (tmp, cat)
}

fn gate<'a>(
    table: &'a mm2_game::AvailabilityTable,
    table_kind: EventTableKind,
    stem: &str,
) -> &'a EventGate {
    &table
        .rows
        .iter()
        .find(|r| r.key.table == table_kind && r.key.stem == stem)
        .unwrap_or_else(|| panic!("no availability row for {stem}"))
        .gate
}

fn gate_stems(gate: &EventGate) -> Vec<&str> {
    match gate {
        EventGate::Open => Vec::new(),
        EventGate::AfterAll(reqs) => reqs.iter().map(|k| k.stem.as_str()).collect(),
    }
}

/// CHK-2/CHK-3: the first set of three is open; each later set gates
/// on the whole previous set — a partial set included.
#[test]
fn checkpoint_rows_gate_in_sets_of_three() {
    let (_tmp, cat) = catalog();
    let table = availability_table(&cat);
    use EventTableKind::Checkpoint as C;
    for stem in ["race0", "race1", "race2"] {
        assert_eq!(gate(&table, C, stem), &EventGate::Open);
    }
    assert_eq!(
        gate_stems(gate(&table, C, "race3")),
        ["race0", "race1", "race2"]
    );
    assert_eq!(table.rows.len(), cat.events.len());
    assert_eq!(table.diagnostics.len(), 1, "only the `bogus` crash row");
}

/// Blitz and Circuit carry no authored gating.
#[test]
fn blitz_and_circuit_are_always_open() {
    let (_tmp, cat) = catalog();
    let table = availability_table(&cat);
    for (kind, stem) in [
        (EventTableKind::Blitz, "blitz0"),
        (EventTableKind::Blitz, "blitz1"),
        (EventTableKind::Circuit, "circuit0"),
    ] {
        assert_eq!(gate(&table, kind, stem), &EventGate::Open);
    }
}

/// CC-3: a midterm gates on its lesson group (tag arithmetic), the
/// final on every midterm, lessons stay open; an unrecognized tag is
/// open plus a diagnostic.
#[test]
fn crash_course_gates_follow_the_authored_tags() {
    let (_tmp, cat) = catalog();
    let table = availability_table(&cat);
    use EventTableKind::CrashCourse as CC;
    // lesson1/lesson4 stay open; midtrm1 needs crash0-2, midtrm2 the
    // authored group-2 lessons (only lesson4 exists), the final needs
    // both midterms.
    assert_eq!(gate(&table, CC, "crash0"), &EventGate::Open);
    assert_eq!(gate(&table, CC, "crash4"), &EventGate::Open);
    assert_eq!(
        gate_stems(gate(&table, CC, "crash3")),
        ["crash0", "crash1", "crash2"]
    );
    assert_eq!(gate_stems(gate(&table, CC, "crash5")), ["crash4"]);
    assert_eq!(gate_stems(gate(&table, CC, "crash7")), ["crash3", "crash5"]);
    assert_eq!(gate(&table, CC, "crash6"), &EventGate::Open);
    assert_eq!(table.diagnostics.len(), 1);
    assert!(table.diagnostics[0].contains("crash6"));
}

/// A `midtrm<N>` tag number is authored data — `3N` can exceed the
/// u32 lesson numbers without naming any real group. The group
/// arithmetic must not overflow (a debug-build panic) nor wrap onto
/// a real lesson group (a silent wrong gate in release): the row
/// fails open and is diagnosed. `midtrm2863311533` wraps to group 5
/// under u32 arithmetic — with lesson5-7 authored, a wrapping build
/// would lock the midterm on them with no diagnostic.
#[test]
fn an_out_of_range_midterm_tag_is_open_and_diagnosed() {
    let tmp = tempfile::tempdir().unwrap();
    let crash_row = |desc: &str| format!("{desc},0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1");
    write(
        tmp.path(),
        "race/testcity/mmcrashdata.csv",
        &format!(
            "{MM_HEADER}\n{}\n",
            ["lesson5", "lesson6", "lesson7", "midtrm2863311533"]
                .map(crash_row)
                .join("\n")
        ),
    );
    let mut vfs = Vfs::new();
    vfs.mount_dir(tmp.path(), 0).unwrap();
    let cat = EventCatalog::scan(&vfs, "testcity");
    let table = availability_table(&cat);
    assert_eq!(
        gate(&table, EventTableKind::CrashCourse, "crash3"),
        &EventGate::Open
    );
    assert_eq!(table.diagnostics.len(), 1);
    assert!(table.diagnostics[0].contains("crash3"));
}

/// A midterm whose whole lesson group is absent fails open and is
/// reported — the diagnostic, not silence, is the finding.
#[test]
fn a_midterm_without_lessons_is_open_and_diagnosed() {
    let tmp = tempfile::tempdir().unwrap();
    let crash_row = |desc: &str| format!("{desc},0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1");
    write(
        tmp.path(),
        "race/testcity/mmcrashdata.csv",
        &format!(
            "{MM_HEADER}\n{}\n",
            ["midtrm1", "final13"].map(crash_row).join("\n")
        ),
    );
    let mut vfs = Vfs::new();
    vfs.mount_dir(tmp.path(), 0).unwrap();
    let cat = EventCatalog::scan(&vfs, "testcity");
    let table = availability_table(&cat);
    use EventTableKind::CrashCourse as CC;
    assert_eq!(gate(&table, CC, "crash0"), &EventGate::Open);
    // The midterm exists, so the final gates on it — and inherits the
    // missing lesson group's openness.
    assert_eq!(gate_stems(gate(&table, CC, "crash1")), ["crash0"]);
    assert_eq!(table.diagnostics.len(), 1);
}
