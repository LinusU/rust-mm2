//! F29-C.1: a mod replaces race rules — an event table row and a
//! waypoint file — and the production race consumer
//! (`event_race_setup`: catalog scan → record resolution →
//! `race_definition`) reads the replacement, while the other event on
//! the same table, and the stock install once the mod is unmounted,
//! stay as they were. The gameplay fingerprint moves, the mods classify
//! as gameplay, and `--expect`-style provenance names the mod serving
//! each record.
//!
//! Self-authored synthetic data; no original install is read. This is
//! synthetic evidence for the Checkpoint table's two record kinds only —
//! `.aimap` rosters, Blitz/Circuit tables and Crash Course sequences
//! are not exercised here.

use std::path::Path;

use mm2_app::race::event_race_setup;
use mm2_assets::Vfs;
use mm2_content::fingerprint;
use mm2_game::{Difficulty, EventRef, EventTableKind};

use crate::support::write;

const HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";

fn row(opponents: u32, ambient: f32) -> String {
    format!(
        "none,0,0,0,{opponents},0,{ambient},0.0,1,50,1,0,0,0,{opponents},0,{ambient},0.0,1,40,1\n"
    )
}

fn course(xs: &[f32]) -> String {
    let mut s = WAYPOINTS.to_string();
    for x in xs {
        s.push_str(&format!("{x},0,140,-90,15,0,0,0,\n"));
    }
    s
}

/// Two checkpoint events: `race0` is the one mods replace, `race1` the
/// bystander. Each has an aimap so the records resolve complete.
fn base_install(d: &Path) {
    write(
        d,
        "race/testcity/mmracedata.csv",
        format!("{HEADER}\n{}{}", row(0, 0.1), row(0, 0.1)),
    );
    for i in 0..2 {
        write(d, &format!("race/testcity/race{i}.aimap"), "#\n");
        write(
            d,
            &format!("race/testcity/race{i}waypoints.csv"),
            course(&[60.0, 110.0, 140.0, 165.0, 180.0]),
        );
    }
}

fn manifest(d: &Path, id: &str) {
    write(
        d,
        "mod.toml",
        format!("[mod]\nid = \"{id}\"\neffect = \"gameplay\"\n"),
    );
}

/// Retunes event 0's authored opponents and ambient density.
fn table_mod(d: &Path) {
    manifest(d, "rules-table");
    write(
        d,
        "race/testcity/mmracedata.csv",
        format!("{HEADER}\n{}{}", row(3, 0.9), row(0, 0.1)),
    );
}

/// Replaces event 0's course with a shorter, differently placed one.
fn course_mod(d: &Path) {
    manifest(d, "rules-course");
    write(
        d,
        "race/testcity/race0waypoints.csv",
        course(&[70.0, 120.0, 175.0]),
    );
}

struct Fixture {
    _tmp: tempfile::TempDir,
    base: std::path::PathBuf,
    table: std::path::PathBuf,
    course: std::path::PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let dir = |n: &str| {
        let p = tmp.path().join(n);
        std::fs::create_dir_all(&p).unwrap();
        p
    };
    let f = Fixture {
        base: dir("base"),
        table: dir("table"),
        course: dir("course"),
        _tmp: tmp,
    };
    base_install(&f.base);
    table_mod(&f.table);
    course_mod(&f.course);
    f
}

fn event(index: usize) -> EventRef {
    EventRef {
        city: "testcity".into(),
        table: EventTableKind::Checkpoint,
        index,
    }
}

fn mounted(base: &Path, mods: &[&Path]) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(base, 0).unwrap();
    for (i, m) in mods.iter().enumerate() {
        vfs.mount_mod(m, 300 + i as i32).unwrap();
    }
    vfs
}

/// What the race consumer produced for one event, reduced to the fields
/// the two mods are expected to move.
#[derive(Debug, PartialEq)]
struct Observed {
    gates: Vec<f32>,
    opponents: u32,
    traffic: f32,
}

fn observe(vfs: &Vfs, index: usize) -> Observed {
    let setup = event_race_setup(vfs, &event(index), Difficulty::Amateur).unwrap();
    Observed {
        gates: setup
            .definition
            .checkpoints
            .iter()
            .map(|c| c.center.x)
            .collect(),
        opponents: setup.definition.params.opponents,
        traffic: setup.definition.params.densities.traffic,
    }
}

/// The first and last waypoint rows are the start line and the finish;
/// the rows between are the gates the definition clears.
const STOCK_GATES: [f32; 3] = [110.0, 140.0, 165.0];

#[test]
fn a_mod_replaces_a_races_table_row_and_course_through_the_race_consumer() {
    let f = fixture();
    let stock = observe(&mounted(&f.base, &[]), 0);
    assert_eq!(stock.gates, STOCK_GATES);
    assert_eq!((stock.opponents, stock.traffic), (0, 0.1));
    let bystander = observe(&mounted(&f.base, &[]), 1);

    // Each mod moves only the record it replaces.
    let table = observe(&mounted(&f.base, &[&f.table]), 0);
    assert_eq!(table.gates, stock.gates, "the table mod leaves the course");
    assert_eq!((table.opponents, table.traffic), (3, 0.9));
    let course = observe(&mounted(&f.base, &[&f.course]), 0);
    assert_ne!(course.gates, stock.gates);
    assert!(course.gates.len() < stock.gates.len(), "{course:?}");
    assert_eq!(
        (course.opponents, course.traffic),
        (stock.opponents, stock.traffic),
        "the course mod leaves the table row"
    );

    // Both together move both; the unreplaced event is untouched.
    let both = mounted(&f.base, &[&f.table, &f.course]);
    let moved = observe(&both, 0);
    assert_eq!(moved.gates, course.gates);
    assert_eq!((moved.opponents, moved.traffic), (3, 0.9));
    assert_eq!(observe(&both, 1), bystander, "event 1 is unreplaced");

    // AC02: a fresh mount without the mods is the stock race again.
    assert_eq!(observe(&mounted(&f.base, &[]), 0), stock);
}

#[test]
fn race_rule_mods_are_gameplay_and_move_the_fingerprint() {
    let f = fixture();
    let base = fingerprint::gameplay(&mounted(&f.base, &[])).unwrap().hash;
    for m in [&f.table, &f.course] {
        let vfs = mounted(&f.base, &[m]);
        let reports = fingerprint::mod_reports(&vfs);
        assert_eq!(reports.len(), 1);
        assert!(!reports[0].is_cosmetic_only(), "{reports:?}");
        assert!(reports[0].contradiction().is_none(), "{reports:?}");
        assert!(!fingerprint::mods_cosmetic_only(&vfs));
        assert_ne!(fingerprint::gameplay(&vfs).unwrap().hash, base);
    }
}

#[test]
fn a_conflicting_race_record_names_the_mod_the_consumer_loaded() {
    let f = fixture();
    // A second mod replaces the same waypoint file; the later mount wins.
    let rival = f._tmp.path().join("rival");
    std::fs::create_dir_all(&rival).unwrap();
    manifest(&rival, "rules-rival");
    write(
        &rival,
        "race/testcity/race0waypoints.csv",
        course(&[80.0, 130.0, 150.0]),
    );
    let vfs = mounted(&f.base, &[&f.course, &rival]);
    assert_eq!(observe(&vfs, 0).gates, [130.0]);
    let explained = vfs
        .explain("race/testcity/race0waypoints.csv")
        .expect("the record resolves")
        .render();
    assert!(explained.contains("rules-rival"), "{explained}");
    assert!(explained.contains("rules-course"), "{explained}");
}

#[test]
fn a_malformed_race_override_is_refused_not_replaced_by_the_stock_course() {
    let f = fixture();
    let broken = f._tmp.path().join("broken");
    std::fs::create_dir_all(&broken).unwrap();
    manifest(&broken, "rules-broken");
    // A two-row course has no gate between start and finish; the stock five-gate file under it
    // must not silently stand in.
    write(
        &broken,
        "race/testcity/race0waypoints.csv",
        course(&[70.0, 90.0]),
    );
    let vfs = mounted(&f.base, &[&broken]);
    let err = event_race_setup(&vfs, &event(0), Difficulty::Amateur)
        .err()
        .expect("the malformed override must be refused, not swapped for the stock course");
    assert!(
        matches!(
            err,
            mm2_app::race::EventSetupError::Build(mm2_content::RaceBuildError::TooFewRows { .. })
        ),
        "{err:?}"
    );
    // The bystander event still loads.
    assert_eq!(observe(&vfs, 1).gates, STOCK_GATES);
}
