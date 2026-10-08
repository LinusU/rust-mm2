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
//! F29-C.3 (the last section) covers the `.aimap` `[Opponent]` roster and
//! the `.opp` routes it names: a mod's vehicle id or driving line reaches
//! the fielded lineup, a dead route reference is reported rather than
//! backfilled from the stock file, and a malformed aimap is refused
//! instead of racing the event without opponents.
//!
//! F29-C.5 (the final section) covers a Crash Course lesson through
//! `lesson_race_setup`: a mod's sub-event table, `.aimap` lead car and
//! `.opp` route each reach the built lesson, a dead route is reported, and
//! a present-but-unusable route is refused.
//!
//! Self-authored synthetic data; no original install is read. This is
//! synthetic evidence for the Checkpoint, Blitz and Circuit tables, their
//! waypoint/grid files, the checkpoint event's `.aimap`/`.opp` roster and
//! one Crash Course lesson's table/aimap/route only.

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

// ---- F29-C.3: the `.aimap` roster and its `.opp` routes -----------------
//
// `event_race_setup` reads the difficulty-selected aimap through the VFS
// and distils its `[Opponent]` rows into the fielded lineup, resolving
// each named `.opp` route through the same catalog. A mod can therefore
// retarget a lineup by replacing the aimap (who drives) or one `.opp`
// (where they drive) independently.

const OPP_HEADER: &str =
    "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\n";
const TAIL: &str = "0.9 0 50.0 0.7 1 1 1 1 0 1.0";
const STOCK_ROUTE: [f32; 3] = [60.0, 110.0, 180.0];

fn opp(xs: &[f32]) -> String {
    let mut s = OPP_HEADER.to_string();
    for x in xs {
        s.push_str(&format!("{x},0,140,0,0,0,0,0,0\n"));
    }
    s
}

fn opponent_aimap(vehicle: &str, route: &str) -> String {
    format!("[Opponent]\n1\n{vehicle} {route} {TAIL}\n")
}

/// Event 0 authors one opponent (`vpstock` on `race0-a-0.opp`); event 1
/// authors none.
fn roster_base(d: &Path) {
    write(
        d,
        "race/testcity/mmracedata.csv",
        format!("{HEADER}\n{}{}", row(1, 0.1), row(0, 0.1)),
    );
    for i in 0..2 {
        write(
            d,
            &format!("race/testcity/race{i}waypoints.csv"),
            course(&[60.0, 110.0, 140.0, 165.0, 180.0]),
        );
    }
    write(
        d,
        "race/testcity/race0.aimap",
        opponent_aimap("vpstock", "race0-a-0.opp"),
    );
    write(d, "race/testcity/race1.aimap", "#\n");
    write(d, "race/testcity/race0-a-0.opp", opp(&STOCK_ROUTE));
}

/// The fielded lineup reduced to what the mods move: vehicle id, the
/// route's x anchors (`None` for a dead reference) and how many issues
/// the build reported.
#[derive(Debug, PartialEq)]
struct Lineup {
    entries: Vec<(String, Option<Vec<f32>>)>,
    issues: usize,
}

fn lineup(vfs: &Vfs, index: usize) -> Lineup {
    let setup = event_race_setup(vfs, &event(index), Difficulty::Amateur).unwrap();
    Lineup {
        entries: setup
            .roster
            .entries
            .iter()
            .map(|e| {
                (
                    e.vehicle.clone(),
                    e.route
                        .as_ref()
                        .map(|r| r.points.iter().map(|p| p.position.x).collect()),
                )
            })
            .collect(),
        issues: setup.roster.issues.len(),
    }
}

fn stock_lineup() -> Lineup {
    Lineup {
        entries: vec![("vpstock".into(), Some(STOCK_ROUTE.to_vec()))],
        issues: 0,
    }
}

struct Rosters {
    tmp: tempfile::TempDir,
    base: std::path::PathBuf,
}

fn rosters() -> Rosters {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    std::fs::create_dir_all(&base).unwrap();
    roster_base(&base);
    Rosters { tmp, base }
}

#[test]
fn a_mod_replaces_who_drives_and_where_through_the_roster_consumer() {
    let r = rosters();
    let stock = mounted(&r.base, &[]);
    assert_eq!(lineup(&stock, 0), stock_lineup());
    assert_eq!(lineup(&stock, 1).entries, []);

    // The aimap swaps the car; the stock `.opp` still serves the route.
    let who = kinds_mod(
        r.tmp.path(),
        "roster-who",
        &[(
            "race/testcity/race0.aimap",
            opponent_aimap("vpmod", "race0-a-0.opp"),
        )],
    );
    let v = mounted(&r.base, &[&who]);
    assert_eq!(
        lineup(&v, 0),
        Lineup {
            entries: vec![("vpmod".into(), Some(STOCK_ROUTE.to_vec()))],
            issues: 0
        }
    );
    assert_eq!(lineup(&v, 1).entries, []);

    // The `.opp` swaps the line; the stock aimap still names the car.
    let wher = kinds_mod(
        r.tmp.path(),
        "roster-where",
        &[(
            "race/testcity/race0-a-0.opp",
            opp(&[70.0, 120.0, 175.0, 185.0]),
        )],
    );
    let v = mounted(&r.base, &[&wher]);
    assert_eq!(
        lineup(&v, 0),
        Lineup {
            entries: vec![("vpstock".into(), Some(vec![70.0, 120.0, 175.0, 185.0]))],
            issues: 0
        }
    );

    // Both together compose, and unmounting restores the stock lineup.
    let v = mounted(&r.base, &[&who, &wher]);
    assert_eq!(
        lineup(&v, 0).entries,
        [("vpmod".to_string(), Some(vec![70.0, 120.0, 175.0, 185.0]))]
    );
    assert_eq!(lineup(&mounted(&r.base, &[]), 0), stock_lineup());
}

#[test]
fn roster_mods_are_gameplay_and_move_the_fingerprint() {
    let r = rosters();
    let base = fingerprint::gameplay(&mounted(&r.base, &[])).unwrap().hash;
    let mods = [
        kinds_mod(
            r.tmp.path(),
            "roster-who",
            &[(
                "race/testcity/race0.aimap",
                opponent_aimap("vpmod", "race0-a-0.opp"),
            )],
        ),
        kinds_mod(
            r.tmp.path(),
            "roster-where",
            &[("race/testcity/race0-a-0.opp", opp(&[70.0, 120.0, 175.0]))],
        ),
    ];
    for m in &mods {
        let vfs = mounted(&r.base, &[m]);
        let reports = fingerprint::mod_reports(&vfs);
        assert_eq!(reports.len(), 1);
        assert!(!reports[0].is_cosmetic_only(), "{reports:?}");
        assert!(reports[0].contradiction().is_none(), "{reports:?}");
        assert_ne!(fingerprint::gameplay(&vfs).unwrap().hash, base, "{m:?}");
    }
}

#[test]
fn a_roster_override_naming_a_dead_route_is_reported_not_backfilled_from_the_stock_line() {
    let r = rosters();
    let dead = kinds_mod(
        r.tmp.path(),
        "roster-dead",
        &[(
            "race/testcity/race0.aimap",
            opponent_aimap("vpmod", "nowhere-a-0.opp"),
        )],
    );
    let got = lineup(&mounted(&r.base, &[&dead]), 0);
    // The slot is kept with no line (never the stock one), and the
    // dangling reference and the orphaned stock `.opp` are both reported.
    assert_eq!(got.entries, [("vpmod".to_string(), None)]);
    assert!(got.issues >= 1, "{got:?}");
    let setup =
        event_race_setup(&mounted(&r.base, &[&dead]), &event(0), Difficulty::Amateur).unwrap();
    assert!(
        setup.roster.issues.iter().any(|i| matches!(
            i,
            mm2_game::OpponentIssue::UnresolvedRoute { name } if name == "nowhere-a-0.opp"
        )),
        "{:?}",
        setup.roster.issues
    );
}

#[test]
fn a_malformed_roster_override_is_refused_not_raced_without_opponents() {
    let r = rosters();
    // Declares two `[Opponent]` rows and authors one.
    let broken = kinds_mod(
        r.tmp.path(),
        "roster-broken",
        &[(
            "race/testcity/race0.aimap",
            format!("[Opponent]\n2\nvpmod race0-a-0.opp {TAIL}\n"),
        )],
    );
    let v = mounted(&r.base, &[&broken]);
    let err = event_race_setup(&v, &event(0), Difficulty::Amateur)
        .err()
        .expect("the malformed aimap must be refused, not raced as an empty lineup");
    assert!(
        matches!(
            err,
            mm2_app::race::EventSetupError::Aimap(mm2_content::RosterBuildError::AimapParse {
                ref logical,
                ..
            }) if logical == "race/testcity/race0.aimap"
        ),
        "{err:?}"
    );
    // The bystander event still loads, and dropping the mod restores event 0.
    assert_eq!(lineup(&v, 1).entries, []);
    assert_eq!(lineup(&mounted(&r.base, &[]), 0), stock_lineup());
}

// ---- F29-C.5: a Crash Course lesson's sequence table, aimap and `.opp` ---
//
// `lesson_race_setup` is the lesson consumer: it reads the lesson's
// `crash<N>data{,_p}.csv` sub-event table for the legs, its `.aimap`
// `[Opponent]` rows for the lead cars and the `.opp` routes those name.
// A mod can replace each independently; none falls back to the stock
// record once the mod's file is the one the VFS serves.

const CRASHDATA: &str =
    "Filename,Event,Checkpoints,TimeLimit,AmbDensity,extra,extra,extra,extra,etra,\n";
const LEAD_ROUTE: [f32; 3] = [30.0, 70.0, 110.0];

fn lesson_event() -> EventRef {
    kind_event(EventTableKind::CrashCourse, 0)
}

/// One lesson (`crash0`) with one slalom leg and a lead car on
/// `lead-0.opp`.
fn lesson_base(d: &Path) {
    write(
        d,
        "race/testcity/mmcrashdata.csv",
        format!("{HEADER}\nlesson1,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1\n"),
    );
    write(
        d,
        "race/testcity/crash0.aimap",
        opponent_aimap("vpstock", "lead-0.opp"),
    );
    for suffix in ["data", "data_p"] {
        write(
            d,
            &format!("race/testcity/crash0{suffix}.csv"),
            format!("{CRASHDATA}slalom,7,1,12,0.05,0,0,0,0,0,0\n"),
        );
    }
    write(
        d,
        "race/testcity/slalom.csv",
        format!("{WAYPOINTS}10,1,20,90,15,0,0,0,\n10,1,60,-30,16,0,0,0,\n"),
    );
    write(d, "race/testcity/lead-0.opp", opp(&LEAD_ROUTE));
}

struct Lessons {
    tmp: tempfile::TempDir,
    base: std::path::PathBuf,
}

fn lessons() -> Lessons {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    std::fs::create_dir_all(&base).unwrap();
    lesson_base(&base);
    Lessons { tmp, base }
}

/// The lesson as the consumer built it: leg time limits and gate counts,
/// and the lead-car lineup reduced like [`Lineup`].
#[derive(Debug, PartialEq)]
struct Lesson {
    legs: Vec<(String, Option<u32>, usize)>,
    lead: Lineup,
}

fn lesson_of(vfs: &Vfs) -> Lesson {
    lesson_at(vfs, Difficulty::Amateur)
}

fn lesson_at(vfs: &Vfs, difficulty: Difficulty) -> Lesson {
    let setup = mm2_app::race::lesson_race_setup(vfs, &lesson_event(), difficulty).unwrap();
    Lesson {
        legs: setup
            .legs
            .iter()
            .map(|l| {
                (
                    l.filename.clone(),
                    l.definition.time_limit_ticks,
                    l.definition.checkpoints.len(),
                )
            })
            .collect(),
        lead: Lineup {
            entries: setup
                .lead_cars
                .entries
                .iter()
                .map(|e| {
                    (
                        e.vehicle.clone(),
                        e.route
                            .as_ref()
                            .map(|r| r.points.iter().map(|p| p.position.x).collect()),
                    )
                })
                .collect(),
            issues: setup.lead_cars.issues.len(),
        },
    }
}

fn stock_lesson() -> Lesson {
    let stock = lesson_of(&mounted(&lessons().base, &[]));
    assert_eq!(stock.legs.len(), 1);
    assert_eq!(
        stock.lead,
        Lineup {
            entries: vec![("vpstock".into(), Some(LEAD_ROUTE.to_vec()))],
            issues: 0
        }
    );
    stock
}

#[test]
fn a_mod_replaces_a_lessons_legs_lead_car_and_route_through_the_lesson_consumer() {
    let l = lessons();
    let stock = lesson_of(&mounted(&l.base, &[]));
    assert_eq!(stock, stock_lesson());

    // The sequence table swaps the leg (name and budget); the lead car is
    // the stock one.
    let legs = kinds_mod(
        l.tmp.path(),
        "lesson-legs",
        &[
            (
                "race/testcity/crash0data.csv",
                format!("{CRASHDATA}slalom,7,1,30,0.05,0,0,0,0,0,0\n"),
            ),
            (
                "race/testcity/crash0data_p.csv",
                format!("{CRASHDATA}slalom,7,1,30,0.05,0,0,0,0,0,0\n"),
            ),
        ],
    );
    let v = mounted(&l.base, &[&legs]);
    let got = lesson_of(&v);
    assert_ne!(got.legs, stock.legs, "the mod's table drives the leg");
    assert_eq!(got.lead, stock.lead);

    // The aimap swaps who leads; the stock `.opp` still serves the line.
    let who = kinds_mod(
        l.tmp.path(),
        "lesson-who",
        &[(
            "race/testcity/crash0.aimap",
            opponent_aimap("vpmod", "lead-0.opp"),
        )],
    );
    let got = lesson_of(&mounted(&l.base, &[&who]));
    assert_eq!(got.legs, stock.legs);
    assert_eq!(
        got.lead.entries,
        [("vpmod".to_string(), Some(LEAD_ROUTE.to_vec()))]
    );

    // The `.opp` swaps the line; the stock aimap still names the car.
    let wher = kinds_mod(
        l.tmp.path(),
        "lesson-where",
        &[("race/testcity/lead-0.opp", opp(&[40.0, 90.0, 140.0, 190.0]))],
    );
    let got = lesson_of(&mounted(&l.base, &[&wher]));
    assert_eq!(got.legs, stock.legs);
    assert_eq!(
        got.lead,
        Lineup {
            entries: vec![("vpstock".into(), Some(vec![40.0, 90.0, 140.0, 190.0]))],
            issues: 0
        }
    );

    // All three compose; unmounting them restores the stock lesson.
    let all = lesson_of(&mounted(&l.base, &[&legs, &who, &wher]));
    assert_eq!(all.legs, lesson_of(&v).legs);
    assert_eq!(
        all.lead.entries,
        [("vpmod".to_string(), Some(vec![40.0, 90.0, 140.0, 190.0]))]
    );
    assert_eq!(lesson_of(&mounted(&l.base, &[])), stock);
}

#[test]
fn lesson_mods_are_gameplay_and_move_the_fingerprint() {
    let l = lessons();
    let base = fingerprint::gameplay(&mounted(&l.base, &[])).unwrap().hash;
    let mods = [
        kinds_mod(
            l.tmp.path(),
            "lesson-legs",
            &[(
                "race/testcity/crash0data.csv",
                format!("{CRASHDATA}slalom,7,1,30,0.05,0,0,0,0,0,0\n"),
            )],
        ),
        kinds_mod(
            l.tmp.path(),
            "lesson-who",
            &[(
                "race/testcity/crash0.aimap",
                opponent_aimap("vpmod", "lead-0.opp"),
            )],
        ),
        kinds_mod(
            l.tmp.path(),
            "lesson-where",
            &[("race/testcity/lead-0.opp", opp(&[40.0, 90.0, 140.0]))],
        ),
    ];
    for m in &mods {
        let vfs = mounted(&l.base, &[m]);
        let reports = fingerprint::mod_reports(&vfs);
        assert_eq!(reports.len(), 1);
        assert!(!reports[0].is_cosmetic_only(), "{reports:?}");
        assert!(reports[0].contradiction().is_none(), "{reports:?}");
        assert_ne!(fingerprint::gameplay(&vfs).unwrap().hash, base, "{m:?}");
    }
}

#[test]
fn a_lesson_override_naming_a_dead_route_is_reported_not_backfilled_from_the_stock_line() {
    let l = lessons();
    let dead = kinds_mod(
        l.tmp.path(),
        "lesson-dead",
        &[(
            "race/testcity/crash0.aimap",
            opponent_aimap("vpmod", "nowhere-0.opp"),
        )],
    );
    let setup = mm2_app::race::lesson_race_setup(
        &mounted(&l.base, &[&dead]),
        &lesson_event(),
        Difficulty::Amateur,
    )
    .unwrap();
    assert_eq!(setup.lead_cars.entries.len(), 1);
    assert!(
        setup.lead_cars.entries[0].route.is_none(),
        "the slot keeps no line rather than the stock one"
    );
    assert!(
        setup.lead_cars.issues.iter().any(|i| matches!(
            i,
            mm2_game::OpponentIssue::UnresolvedRoute { name } if name == "nowhere-0.opp"
        )),
        "{:?}",
        setup.lead_cars.issues
    );
}

#[test]
fn a_malformed_or_degenerate_lesson_route_is_refused_not_fielded_as_a_lead_car_that_never_moves() {
    use mm2_app::race::LessonSetupError;
    let l = lessons();
    let stock = lesson_of(&mounted(&l.base, &[]));
    // A header from the wrong table, and a line that never leaves its
    // first point: both exist, neither can be driven.
    for (id, body) in [
        ("lesson-broken-opp", "x,y\n1,2,not-a-number\n".to_string()),
        ("lesson-still-opp", opp(&[40.0, 40.0, 40.0])),
    ] {
        let m = kinds_mod(l.tmp.path(), id, &[("race/testcity/lead-0.opp", body)]);
        let v = mounted(&l.base, &[&m]);
        let err = mm2_app::race::lesson_race_setup(&v, &lesson_event(), Difficulty::Amateur)
            .err()
            .unwrap_or_else(|| panic!("{id}: must be refused, not launched"));
        assert!(
            matches!(
                err,
                LessonSetupError::LeadRoute(
                    mm2_game::OpponentIssue::RouteFailed { ref name, .. }
                    | mm2_game::OpponentIssue::DegenerateRoute { ref name }
                ) if name == "lead-0.opp"
            ),
            "{id}: {err:?}"
        );
        // Dropping the mod restores the stock lesson.
        assert_eq!(lesson_of(&mounted(&l.base, &[])), stock);
    }
}

// ---- F29-C.6: the Professional side of a lesson --------------------------
//
// A lesson authors `crash<N>data.csv` (Amateur) beside `crash<N>data_p.csv`
// (Professional) and `<stem>.aimap` beside `<stem>.aimap_p`. A mod for one
// difficulty must not leak into the other; and when a lesson ships no
// `.aimap_p`, Professional falls back to the `.aimap` (RACE-11) — so a mod
// replacing only that `.aimap` reaches both difficulties until a mod (or the
// install) supplies the `_p`.

#[test]
fn a_mod_for_one_lesson_difficulty_moves_only_that_difficulty() {
    let l = lessons();
    let amateur = lesson_at(&mounted(&l.base, &[]), Difficulty::Amateur);
    let pro = lesson_at(&mounted(&l.base, &[]), Difficulty::Professional);
    assert_eq!(amateur.legs, pro.legs, "the fixture authors the same legs");

    // `crash0data_p.csv` alone: Professional's leg budget moves, Amateur's
    // does not.
    let table_p = kinds_mod(
        l.tmp.path(),
        "lesson-table-p",
        &[(
            "race/testcity/crash0data_p.csv",
            format!("{CRASHDATA}slalom,7,1,25,0.05,0,0,0,0,0,0\n"),
        )],
    );
    let v = mounted(&l.base, &[&table_p]);
    assert_eq!(lesson_at(&v, Difficulty::Amateur), amateur);
    let got = lesson_at(&v, Difficulty::Professional);
    assert_ne!(
        got.legs, pro.legs,
        "the _p table drives the Professional leg"
    );
    assert_eq!(got.lead, pro.lead);

    // A mod's `.aimap_p` is read at Professional only; Amateur keeps the
    // stock `.aimap`'s car.
    let who_p = kinds_mod(
        l.tmp.path(),
        "lesson-who-p",
        &[(
            "race/testcity/crash0.aimap_p",
            opponent_aimap("vpproonly", "lead-0.opp"),
        )],
    );
    let v = mounted(&l.base, &[&who_p]);
    assert_eq!(lesson_at(&v, Difficulty::Amateur), amateur);
    let got = lesson_at(&v, Difficulty::Professional);
    assert_eq!(got.legs, pro.legs);
    assert_eq!(
        got.lead.entries,
        [("vpproonly".to_string(), Some(LEAD_ROUTE.to_vec()))]
    );

    // A mod's `.aimap` alone reaches Professional through the fallback...
    let who = kinds_mod(
        l.tmp.path(),
        "lesson-who",
        &[(
            "race/testcity/crash0.aimap",
            opponent_aimap("vpmod", "lead-0.opp"),
        )],
    );
    let v = mounted(&l.base, &[&who]);
    for difficulty in [Difficulty::Amateur, Difficulty::Professional] {
        assert_eq!(
            lesson_at(&v, difficulty).lead.entries,
            [("vpmod".to_string(), Some(LEAD_ROUTE.to_vec()))],
            "{difficulty:?}"
        );
    }
    // ...until a `.aimap_p` exists, which Professional prefers; unmounting
    // everything restores the stock lesson at both difficulties.
    let v = mounted(&l.base, &[&who, &who_p]);
    assert_eq!(
        lesson_at(&v, Difficulty::Amateur).lead.entries,
        [("vpmod".to_string(), Some(LEAD_ROUTE.to_vec()))]
    );
    assert_eq!(
        lesson_at(&v, Difficulty::Professional).lead.entries,
        [("vpproonly".to_string(), Some(LEAD_ROUTE.to_vec()))]
    );
    let v = mounted(&l.base, &[]);
    assert_eq!(lesson_at(&v, Difficulty::Amateur), amateur);
    assert_eq!(lesson_at(&v, Difficulty::Professional), pro);
}

#[test]
fn professional_lesson_mods_are_gameplay_and_move_the_fingerprint() {
    let l = lessons();
    let base = fingerprint::gameplay(&mounted(&l.base, &[])).unwrap().hash;
    let mods = [
        kinds_mod(
            l.tmp.path(),
            "lesson-table-p",
            &[(
                "race/testcity/crash0data_p.csv",
                format!("{CRASHDATA}slalom,7,1,25,0.05,0,0,0,0,0,0\n"),
            )],
        ),
        kinds_mod(
            l.tmp.path(),
            "lesson-who-p",
            &[(
                "race/testcity/crash0.aimap_p",
                opponent_aimap("vpproonly", "lead-0.opp"),
            )],
        ),
    ];
    for m in &mods {
        let vfs = mounted(&l.base, &[m]);
        let reports = fingerprint::mod_reports(&vfs);
        assert_eq!(reports.len(), 1);
        assert!(!reports[0].is_cosmetic_only(), "{reports:?}");
        assert!(reports[0].contradiction().is_none(), "{reports:?}");
        assert_ne!(fingerprint::gameplay(&vfs).unwrap().hash, base, "{m:?}");
    }
}

#[test]
fn a_malformed_professional_lesson_aimap_is_refused_while_amateur_still_launches() {
    use mm2_app::race::LessonSetupError;
    let l = lessons();
    let bad = kinds_mod(
        l.tmp.path(),
        "lesson-bad-p",
        &[(
            "race/testcity/crash0.aimap_p",
            "[Opponent]\n2\nvpmod\n".into(),
        )],
    );
    let v = mounted(&l.base, &[&bad]);
    assert!(
        mm2_app::race::lesson_race_setup(&v, &lesson_event(), Difficulty::Amateur).is_ok(),
        "the Amateur lesson does not read the _p record"
    );
    let Err(err) = mm2_app::race::lesson_race_setup(&v, &lesson_event(), Difficulty::Professional)
    else {
        panic!("a malformed .aimap_p must not fall back to the stock .aimap");
    };
    assert!(matches!(err, LessonSetupError::Aimap(_)), "{err:?}");
}
