//! F29-C.1: a mod replaces race rules — an event table row and a
//! waypoint file — and the production race consumer
//! (`event_race_setup`: catalog scan → record resolution →
//! `race_definition`) reads the replacement, while the other event on
//! the same table, and the stock install once the mod is unmounted,
//! stay as they were. The gameplay fingerprint moves, the mods classify
//! as gameplay, and `--expect`-style provenance names the mod serving
//! each record.
//!
//! F29-C.2 (the second half of the file) repeats the exercise for the
//! Blitz and Circuit tables: a `TimeLimit`, a `NumLaps` and a
//! `_strtpnts` grid each move only their own field, and out-of-range
//! values are refused rather than papered over by the stock row.
//!
//! Self-authored synthetic data; no original install is read. This is
//! synthetic evidence for the Checkpoint, Blitz and Circuit tables and
//! their waypoint/grid files only — `.aimap` rosters and Crash Course
//! sequences are not exercised here.

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

// ---- F29-C.2: the Blitz and Circuit tables ----------------------------
//
// The Checkpoint legs above never touch the two record kinds that bind
// differently: a Blitz row's `TimeLimit` becomes the runtime deadline, a
// Circuit row's `NumLaps` becomes the lap count, and a circuit's
// `_strtpnts` file supplies the start grid. Each mod below replaces one
// of those records through the same `event_race_setup` consumer.

fn kind_row(laps: i64, limit: f32) -> String {
    format!("none,0,0,0,0,0,0.1,0.0,{laps},{limit},1,0,0,0,0,0,0.1,0.0,{laps},{limit},1\n")
}

fn kind_event(table: EventTableKind, index: usize) -> EventRef {
    EventRef {
        city: "testcity".into(),
        table,
        index,
    }
}

const BLITZ_TABLE: &str = "race/testcity/mmblitzdata.csv";
const CIRCUIT_TABLE: &str = "race/testcity/mmcircuitdata.csv";

/// Two Blitz and two Circuit events, each with an aimap and a course;
/// circuit 0 also ships a one-slot grid.
fn kinds_base(d: &Path) {
    write(
        d,
        BLITZ_TABLE,
        format!("{HEADER}\n{}{}", kind_row(3, 50.0), kind_row(3, 50.0)),
    );
    write(
        d,
        CIRCUIT_TABLE,
        format!("{HEADER}\n{}{}", kind_row(2, 0.0), kind_row(2, 0.0)),
    );
    for stem in ["blitz0", "blitz1", "circuit0", "circuit1"] {
        write(d, &format!("race/testcity/{stem}.aimap"), "#\n");
        write(
            d,
            &format!("race/testcity/{stem}waypoints.csv"),
            course(&[60.0, 110.0, 140.0, 165.0, 180.0]),
        );
    }
    write(d, "race/testcity/cir0_strtpnts", "60,0,140,90,0,0,0,0,0,\n");
}

fn kinds_mod(root: &Path, id: &str, files: &[(&str, String)]) -> std::path::PathBuf {
    let d = root.join(id);
    std::fs::create_dir_all(&d).unwrap();
    manifest(&d, id);
    for (rel, body) in files {
        write(&d, rel, body);
    }
    d
}

#[derive(Debug, PartialEq)]
struct KindObserved {
    laps: u32,
    time_limit_ticks: Option<u32>,
    gates: usize,
    slots: Vec<(f32, Option<f32>)>,
}

fn observe_kind(vfs: &Vfs, table: EventTableKind, index: usize) -> KindObserved {
    let setup = event_race_setup(vfs, &kind_event(table, index), Difficulty::Amateur).unwrap();
    let def = setup.definition;
    KindObserved {
        laps: def.laps,
        time_limit_ticks: def.time_limit_ticks,
        gates: def.checkpoints.len(),
        slots: def
            .start_slots
            .iter()
            .map(|s| (s.position.x, s.yaw_deg))
            .collect(),
    }
}

struct Kinds {
    tmp: tempfile::TempDir,
    base: std::path::PathBuf,
}

fn kinds() -> Kinds {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    std::fs::create_dir_all(&base).unwrap();
    kinds_base(&base);
    Kinds { tmp, base }
}

#[test]
fn a_mod_replaces_a_blitz_time_limit_a_circuit_lap_count_and_a_start_grid() {
    use EventTableKind::{Blitz, Circuit};
    let k = kinds();
    let stock = mounted(&k.base, &[]);
    let blitz = observe_kind(&stock, Blitz, 0);
    let circuit = observe_kind(&stock, Circuit, 0);
    assert_eq!(blitz.time_limit_ticks, Some(50 * mm2_game::RACE_TICK_HZ));
    assert_eq!((blitz.laps, circuit.time_limit_ticks), (0, None));
    assert_eq!(circuit.laps, 2);
    assert_eq!(circuit.slots, [(60.0, Some(90.0))]);
    let blitz_bystander = observe_kind(&stock, Blitz, 1);
    let circuit_bystander = observe_kind(&stock, Circuit, 1);

    let limit = kinds_mod(
        k.tmp.path(),
        "blitz-limit",
        &[(
            BLITZ_TABLE,
            format!("{HEADER}\n{}{}", kind_row(3, 20.0), kind_row(3, 50.0)),
        )],
    );
    let laps = kinds_mod(
        k.tmp.path(),
        "circuit-laps",
        &[(
            CIRCUIT_TABLE,
            format!("{HEADER}\n{}{}", kind_row(5, 0.0), kind_row(2, 0.0)),
        )],
    );
    let grid = kinds_mod(
        k.tmp.path(),
        "circuit-grid",
        &[(
            "race/testcity/cir0_strtpnts",
            "70,0,140,-90,0,0,0,0,0,\n66,0,144,-90,0,0,0,0,0,\n".to_string(),
        )],
    );

    // Each mod moves only the one field it replaces, on its own table.
    let v = mounted(&k.base, &[&limit]);
    let moved = observe_kind(&v, Blitz, 0);
    assert_eq!(moved.time_limit_ticks, Some(20 * mm2_game::RACE_TICK_HZ));
    assert_eq!((moved.laps, moved.gates), (blitz.laps, blitz.gates));
    assert_eq!(observe_kind(&v, Blitz, 1), blitz_bystander);
    assert_eq!(observe_kind(&v, Circuit, 0), circuit, "other table intact");

    let v = mounted(&k.base, &[&laps]);
    let moved = observe_kind(&v, Circuit, 0);
    assert_eq!(moved.laps, 5);
    assert_eq!(moved.slots, circuit.slots, "the lap mod leaves the grid");
    assert_eq!(observe_kind(&v, Circuit, 1), circuit_bystander);
    assert_eq!(observe_kind(&v, Blitz, 0), blitz, "other table intact");

    let v = mounted(&k.base, &[&grid]);
    let moved = observe_kind(&v, Circuit, 0);
    assert_eq!(moved.slots, [(70.0, Some(-90.0)), (66.0, Some(-90.0))]);
    assert_eq!(moved.laps, circuit.laps, "the grid mod leaves the table");

    // All three together move all three; a fresh stock mount restores it.
    let v = mounted(&k.base, &[&limit, &laps, &grid]);
    assert_eq!(
        observe_kind(&v, Blitz, 0).time_limit_ticks,
        Some(20 * mm2_game::RACE_TICK_HZ)
    );
    let c = observe_kind(&v, Circuit, 0);
    assert_eq!((c.laps, c.slots.len()), (5, 2));
    assert_eq!(observe_kind(&mounted(&k.base, &[]), Circuit, 0), circuit);
    assert_eq!(observe_kind(&mounted(&k.base, &[]), Blitz, 0), blitz);
}

#[test]
fn blitz_and_circuit_record_mods_are_gameplay_and_move_the_fingerprint() {
    let k = kinds();
    let base = fingerprint::gameplay(&mounted(&k.base, &[])).unwrap().hash;
    let mods = [
        kinds_mod(
            k.tmp.path(),
            "blitz-limit",
            &[(
                BLITZ_TABLE,
                format!("{HEADER}\n{}{}", kind_row(3, 20.0), kind_row(3, 50.0)),
            )],
        ),
        kinds_mod(
            k.tmp.path(),
            "circuit-grid",
            &[(
                "race/testcity/cir0_strtpnts",
                "70,0,140,-90,0,0,0,0,0,\n".to_string(),
            )],
        ),
    ];
    for m in &mods {
        let vfs = mounted(&k.base, &[m]);
        let reports = fingerprint::mod_reports(&vfs);
        assert_eq!(reports.len(), 1);
        assert!(!reports[0].is_cosmetic_only(), "{reports:?}");
        assert!(reports[0].contradiction().is_none(), "{reports:?}");
        assert_ne!(fingerprint::gameplay(&vfs).unwrap().hash, base, "{m:?}");
    }
}

#[test]
fn an_out_of_range_blitz_or_circuit_value_is_refused_not_replaced_by_the_stock_row() {
    use EventTableKind::{Blitz, Circuit};
    use mm2_app::race::EventSetupError::Build;
    use mm2_content::RaceBuildError::{BadParam, TooFewRows};
    let k = kinds();
    let bad_limit = kinds_mod(
        k.tmp.path(),
        "bad-limit",
        &[(
            BLITZ_TABLE,
            format!("{HEADER}\n{}{}", kind_row(3, 0.0), kind_row(3, 50.0)),
        )],
    );
    let bad_laps = kinds_mod(
        k.tmp.path(),
        "bad-laps",
        &[(
            CIRCUIT_TABLE,
            format!("{HEADER}\n{}{}", kind_row(0, 0.0), kind_row(2, 0.0)),
        )],
    );
    let short = kinds_mod(
        k.tmp.path(),
        "short-circuit",
        &[(
            "race/testcity/circuit0waypoints.csv",
            course(&[60.0, 110.0, 140.0]),
        )],
    );
    let setup = |vfs: &Vfs, table, index| {
        event_race_setup(vfs, &kind_event(table, index), Difficulty::Amateur)
            .err()
            .expect("the malformed override must be refused")
    };

    let v = mounted(&k.base, &[&bad_limit]);
    assert!(
        matches!(
            setup(&v, Blitz, 0),
            Build(BadParam {
                field: "TimeLimit",
                ..
            })
        ),
        "a zero Blitz TimeLimit"
    );
    // The row below the broken one, and the other table, still load.
    assert!(observe_kind(&v, Blitz, 1).time_limit_ticks.is_some());
    assert_eq!(observe_kind(&v, Circuit, 0).laps, 2);

    let v = mounted(&k.base, &[&bad_laps]);
    assert!(
        matches!(
            setup(&v, Circuit, 0),
            Build(BadParam {
                field: "NumLaps",
                ..
            })
        ),
        "a zero Circuit NumLaps"
    );
    assert_eq!(observe_kind(&v, Circuit, 1).laps, 2);

    let v = mounted(&k.base, &[&short]);
    assert!(
        matches!(setup(&v, Circuit, 0), Build(TooFewRows { needed: 4, .. })),
        "a three-row closed course"
    );
    assert_eq!(observe_kind(&v, Circuit, 1).gates, 5);
}
