//! `event` single-event inspection (F11-AC06): resolve one authored
//! event row through the production [`EventCatalog`] and validate its
//! complete dependency closure without launching the game — every
//! attributed record's parse status, the aimap/pathset records the
//! catalog scan leaves unparsed (deep-parsed and validated here), the
//! production `CatalogEvent → RaceDefinition` and
//! `CatalogEvent → OpponentRoster` builds at both difficulties, and a
//! cross-check of wired vehicle ids against the vehicle catalog.
//!
//! `--all` sweeps the whole catalog through the same deep check
//! (F11-AC01/AC06): every authored row in every discovered race city,
//! one line each, `--city` restricting the sweep to one stem.
//!
//! The command reports rather than hides: an incomplete event still
//! prints its full record inventory so the missing piece is visible.
//! `--strict` exits nonzero on an incomplete event, a failed record or
//! reference, a record validation issue, a failed production build,
//! roster issues, or a wired vehicle id outside the catalog.

use std::collections::BTreeSet;
use std::path::Path;

use mm2_assets::Vfs;
use mm2_content::{
    CatalogEvent, EventStatus, RaceDefBuild, RecordContent, RosterBuild, VehicleCatalog,
    audit_build, audit_roster,
};
use mm2_formats::aimap::Aimap;
use mm2_formats::pathset::Pathset;
use mm2_formats::racedata::RaceParams;
use mm2_formats::racefiles::RaceFileKind;
use mm2_formats::rewards::RewardRow;
use mm2_game::{Difficulty, EventRef, EventTableKind};

use crate::{build_vfs, describe_build, describe_roster, parse_table_filter};

/// Outcome of checking one record attributed to the event.
#[derive(Debug)]
pub struct RecordCheck {
    /// Resolved logical path.
    pub logical: String,
    /// Record kind per the shared `race/<city>` grammar.
    pub kind: RaceFileKind,
    /// `.opp` difficulty tag, where the filename carries one.
    pub difficulty: Option<char>,
    /// Compact summary (`8 opponents, 45 exceptions`, `10 rows`).
    pub detail: String,
    /// Recoverable validation/diagnostic problems.
    pub issues: Vec<String>,
    /// The record failed to resolve or parse.
    pub failed: bool,
}

/// A selected event's full dependency report.
#[derive(Debug)]
pub struct EventReport {
    /// The resolved ref (city lowercased).
    pub event_ref: EventRef,
    /// File stem the row maps to.
    pub stem: String,
    /// Authored description tag.
    pub description: String,
    /// Completeness against the authored record set.
    pub status: EventStatus,
    /// Amateur parameter block, verbatim.
    pub amateur: RaceParams,
    /// Professional parameter block, verbatim.
    pub professional: RaceParams,
    /// Every attributed record with its check outcome.
    pub records: Vec<RecordCheck>,
    /// References that failed to resolve or parse at scan time.
    pub failed_refs: Vec<mm2_content::FailedRef>,
    /// Indexed rewards attached to this event.
    pub rewards: Vec<RewardRow>,
    /// `RaceDefinition` builds (amateur, professional).
    pub defs: [RaceDefBuild; 2],
    /// `OpponentRoster` builds (amateur, professional).
    pub rosters: [RosterBuild; 2],
    /// Wired vehicle ids absent from the vehicle catalog.
    pub unresolved_vehicles: Vec<String>,
}

impl EventReport {
    /// Everything `--strict` fails on — collected, not just printed, so
    /// tests can assert the same conditions the exit code reports.
    pub fn failures(&self) -> Vec<String> {
        let mut failures = Vec::new();
        if let EventStatus::Incomplete { missing } = &self.status {
            failures.push(format!("incomplete: {}", missing.join(", ")));
        }
        for r in &self.records {
            if r.failed {
                failures.push(format!("{} — {}", r.logical, r.detail));
            }
            for issue in &r.issues {
                failures.push(format!("{} — {issue}", r.logical));
            }
        }
        for f in &self.failed_refs {
            failures.push(format!("failed ref: {} — {}", f.reference, f.reason));
        }
        for (label, build) in ["amateur", "professional"].iter().zip(&self.defs) {
            if let RaceDefBuild::Failed(e) = build {
                failures.push(format!("{label} race definition — {e}"));
            }
        }
        for (label, build) in ["amateur", "professional"].iter().zip(&self.rosters) {
            match build {
                RosterBuild::Failed(e) => {
                    failures.push(format!("{label} opponent roster — {e}"));
                }
                RosterBuild::Built(s) => {
                    for issue in &s.issues {
                        failures.push(format!("{label} opponent roster — {issue}"));
                    }
                }
                RosterBuild::Unsupported => {}
            }
        }
        for v in &self.unresolved_vehicles {
            failures.push(format!("wired vehicle {v} is not in the vehicle catalog"));
        }
        failures
    }
}

/// Deep-check one attributed record. Kinds the catalog scan leaves
/// `Unparsed` are parsed here so a selected event's dependencies are
/// fully validated, not just present.
fn check_record(vfs: &Vfs, rec: &mm2_content::EventRecord) -> RecordCheck {
    let mut check = RecordCheck {
        logical: rec.logical.clone(),
        kind: rec.kind,
        difficulty: rec.difficulty,
        detail: String::new(),
        issues: Vec::new(),
        failed: false,
    };
    let diagnostics = |diags: &[mm2_formats::racedata::TableDiagnostic]| -> Vec<String> {
        diags
            .iter()
            .map(|d| format!("line {}: {}", d.line, d.message))
            .collect()
    };
    match &rec.content {
        RecordContent::Waypoints(f) => {
            check.detail = format!("{} rows ({})", f.rows.len(), f.width_label);
            check.issues.extend(diagnostics(&f.diagnostics));
        }
        RecordContent::StartPoints(f) => {
            check.detail = format!("{} slots", f.rows.len());
            check.issues.extend(diagnostics(&f.diagnostics));
        }
        RecordContent::Opp(f) => {
            check.detail = format!("{} points", f.rows.len());
            check.issues.extend(diagnostics(&f.diagnostics));
        }
        RecordContent::CrashData(f) => {
            check.detail = format!("{} lesson rows", f.rows.len());
            check.issues.extend(diagnostics(&f.diagnostics));
        }
        RecordContent::Failed(e) => {
            check.detail = format!("parse failed: {e}");
            check.failed = true;
        }
        RecordContent::Unparsed => match rec.kind {
            RaceFileKind::Aimap | RaceFileKind::AimapP => {
                match vfs
                    .read_logical(&rec.logical)
                    .map_err(|e| e.to_string())
                    .and_then(|b| {
                        Aimap::parse(&String::from_utf8_lossy(&b)).map_err(|e| e.to_string())
                    }) {
                    Ok(a) => {
                        check.detail = format!(
                            "{} opponents, {} police, {} exceptions, {} ambient types",
                            a.opponents.len(),
                            a.police.len(),
                            a.exceptions.len(),
                            a.ambient_types.len(),
                        );
                        check
                            .issues
                            .extend(a.validate().iter().map(|i| i.to_string()));
                    }
                    Err(e) => {
                        check.detail = format!("parse failed: {e}");
                        check.failed = true;
                    }
                }
            }
            RaceFileKind::Pathset => {
                match vfs
                    .read_logical(&rec.logical)
                    .map_err(|e| e.to_string())
                    .and_then(|b| Pathset::parse(&b).map_err(|e| e.to_string()))
                {
                    Ok(p) => {
                        let points: usize = p.paths.iter().map(|p| p.points.len()).sum();
                        check.detail = format!("{} paths, {} points", p.paths.len(), points);
                        check
                            .issues
                            .extend(p.validate().iter().map(|i| i.to_string()));
                    }
                    Err(e) => {
                        check.detail = format!("parse failed: {e}");
                        check.failed = true;
                    }
                }
            }
            _ => {
                check.detail = "resolved (no interpreter for this kind)".into();
            }
        },
    }
    check
}

/// The vehicle catalog's id set — scanned once and shared across a
/// sweep so a 90-event audit does not rescan the tune roster per row.
pub(crate) fn vehicle_ids(vfs: &Vfs) -> BTreeSet<String> {
    VehicleCatalog::scan(vfs)
        .entries
        .iter()
        .map(|e| e.id.clone())
        .collect()
}

/// Inspect one event: catalog-scan the city, look up the row, and
/// validate its dependency closure. `Err` when no authored row exists
/// for the ref — a lookup failure, distinct from a found-but-incomplete
/// event which returns a report carrying `status: Incomplete`.
pub fn inspect(
    vfs: &Vfs,
    city: &str,
    table: EventTableKind,
    index: usize,
) -> Result<EventReport, String> {
    let city = city.to_ascii_lowercase();
    let catalog = mm2_content::EventCatalog::scan(vfs, &city);
    let event_ref = EventRef {
        city: city.clone(),
        table,
        index,
    };
    let Some(event) = catalog.get(&event_ref) else {
        return Err(format!(
            "no authored event row for {table:?}:{index} in {city}"
        ));
    };
    Ok(inspect_event(vfs, event, &vehicle_ids(vfs)))
}

/// One city's catalog-wide deep audit: every authored row through the
/// same per-event check [`inspect`] performs.
#[derive(Debug)]
pub struct CitySweep {
    /// City stem scanned.
    pub city: String,
    /// Per-table parse status — a failed table claims no rows, so its
    /// events simply never appear; the error is reported here.
    pub tables: Vec<mm2_content::EventTableStatus>,
    /// Every cataloged event's report, in catalog order.
    pub events: Vec<EventReport>,
    /// Discovered record stems no table row claims (counted for the
    /// denominator; `events`/`aimap` audits them in detail).
    pub extras: usize,
    /// Catalog-level problems that are not one event's fault.
    pub diagnostics: Vec<String>,
}

impl CitySweep {
    /// Everything `--strict` fails on: a missing/malformed table, an
    /// empty catalog, and every per-event failure [`EventReport`]
    /// reports. Diagnostics stay notes, matching `events`.
    pub fn failures(&self) -> Vec<String> {
        let mut failures = Vec::new();
        if self.events.is_empty() {
            failures.push("event catalog is empty".to_string());
        }
        for t in &self.tables {
            if let Some(e) = &t.error {
                failures.push(format!("{} — {e}", t.logical));
            }
        }
        for ev in &self.events {
            let table = format!("{:?}", ev.event_ref.table).to_lowercase();
            for f in ev.failures() {
                failures.push(format!(
                    "{}:{} ({}) — {f}",
                    table, ev.event_ref.index, ev.stem
                ));
            }
        }
        failures
    }
}

/// Deep-check every cataloged event in one city. The vehicle catalog
/// is supplied by the caller so a multi-city sweep scans it once.
pub fn sweep(vfs: &Vfs, city: &str, vehicle_ids: &BTreeSet<String>) -> CitySweep {
    let catalog = mm2_content::EventCatalog::scan(vfs, &city.to_ascii_lowercase());
    CitySweep {
        city: catalog.city.clone(),
        tables: catalog.tables.clone(),
        events: catalog
            .events
            .iter()
            .map(|e| inspect_event(vfs, e, vehicle_ids))
            .collect(),
        extras: catalog.extras.len(),
        diagnostics: catalog.diagnostics.clone(),
    }
}

/// The report body — separated from ref lookup so tests can drive it
/// with a catalog they built themselves.
fn inspect_event(vfs: &Vfs, event: &CatalogEvent, vehicle_ids: &BTreeSet<String>) -> EventReport {
    let records = event.records.iter().map(|r| check_record(vfs, r)).collect();

    let mut wired: BTreeSet<String> = BTreeSet::new();
    let defs = [
        audit_build(event, Difficulty::Amateur),
        audit_build(event, Difficulty::Professional),
    ];
    let rosters = [
        audit_roster(vfs, event, Difficulty::Amateur, &mut wired),
        audit_roster(vfs, event, Difficulty::Professional, &mut wired),
    ];

    let unresolved_vehicles = wired
        .iter()
        .filter(|id| !vehicle_ids.contains(id.as_str()))
        .cloned()
        .collect();

    EventReport {
        event_ref: event.event_ref.clone(),
        stem: event.stem.clone(),
        description: event.description.clone(),
        status: event.status.clone(),
        amateur: event.amateur.clone(),
        professional: event.professional.clone(),
        records,
        failed_refs: event.failed.clone(),
        rewards: event.rewards.clone(),
        defs,
        rosters,
        unresolved_vehicles,
    }
}

/// One-line summary of a parameter block.
fn describe_params(p: &RaceParams) -> String {
    format!(
        "opp {}, cops {}, laps {}, limit {}s, tod {}, weather {}, ambient {:.2}, peds {:.2}, cartype {}, difficulty {}",
        p.opponents,
        p.cops,
        p.num_laps,
        p.time_limit,
        p.time_of_day,
        p.weather,
        p.ambient,
        p.peds,
        p.car_type,
        p.difficulty,
    )
}

/// Parse the `--event` value: `<table>:<row>`, same vocabulary as
/// `mm2 --event` (`checkpoint|race`, `blitz`, `circuit`,
/// `crash|crashcourse`).
pub fn parse_event_spec(s: &str) -> Result<(EventTableKind, usize), String> {
    let Some((table, row)) = s.split_once(':') else {
        return Err(format!(
            "invalid event spec {s:?}: expected <table>:<row> (e.g. circuit:0)"
        ));
    };
    let table = parse_table_filter(table)?;
    let index: usize = row
        .parse()
        .map_err(|_| format!("invalid event row {row:?}: expected a row index"))?;
    Ok((table, index))
}

/// The authored extension label for a record kind (`AimapP` is
/// `.aimap_p` on disk, not `aimapp`).
fn kind_label(kind: RaceFileKind) -> &'static str {
    match kind {
        RaceFileKind::Aimap => "aimap",
        RaceFileKind::AimapP => "aimap_p",
        RaceFileKind::Pathset => "pathset",
        RaceFileKind::Waypoints => "waypoints",
        RaceFileKind::DataCsv => "data",
        RaceFileKind::Csv => "csv",
        RaceFileKind::Opp => "opp",
        RaceFileKind::StartPoints => "strtpnts",
        RaceFileKind::Meta => "meta",
        RaceFileKind::Junk => "junk",
        RaceFileKind::Other => "other",
    }
}

/// Print the report in the same style as the catalog-wide audits.
fn print_report(report: &EventReport) {
    let table = format!("{:?}", report.event_ref.table).to_lowercase();
    println!(
        "== event: {} {} row {} ({}) ==",
        report.event_ref.city, table, report.event_ref.index, report.stem
    );
    println!("  description: {}", report.description);
    match &report.status {
        EventStatus::Ready => println!("  status: ready"),
        EventStatus::Incomplete { missing } => {
            println!("  status: incomplete ({})", missing.join(", "));
        }
    }
    println!("  amateur:      {}", describe_params(&report.amateur));
    println!("  professional: {}", describe_params(&report.professional));

    println!("  records ({}):", report.records.len());
    for r in &report.records {
        let diff = r.difficulty.map(|d| format!("[{d}]")).unwrap_or_default();
        let mark = if r.failed { "FAILED" } else { "ok" };
        println!(
            "    {:<44} {:<10} {mark}: {}",
            r.logical,
            format!("{}{diff}", kind_label(r.kind)),
            r.detail,
        );
        for issue in &r.issues {
            println!("       issue: {issue}");
        }
    }

    if report.failed_refs.is_empty() {
        println!("  failed references: none");
    } else {
        println!("  failed references:");
        for f in &report.failed_refs {
            println!("    {} — {}", f.reference, f.reason);
        }
    }
    if report.rewards.is_empty() {
        println!("  rewards: none");
    } else {
        println!("  rewards:");
        for r in &report.rewards {
            println!(
                "    {} {:?} {} variant {} — {}",
                r.race_type, r.race_num, r.car, r.variant, r.message
            );
        }
    }

    println!(
        "  race definition:  am: {}",
        describe_build(&report.defs[0])
    );
    println!(
        "                    pro: {}",
        describe_build(&report.defs[1])
    );
    println!(
        "  opponent roster:  am: {}",
        describe_roster(&report.rosters[0])
    );
    println!(
        "                    pro: {}",
        describe_roster(&report.rosters[1])
    );
    for (label, build) in ["amateur", "professional"].iter().zip(&report.rosters) {
        if let RosterBuild::Built(s) = build {
            for issue in &s.issues {
                println!("       {label} issue: {issue}");
            }
        }
    }
    if report.unresolved_vehicles.is_empty() {
        println!("  wired vehicle ids: all resolved");
    } else {
        println!(
            "  unresolved vehicle ids: {}",
            report.unresolved_vehicles.join(", ")
        );
    }
}

/// Compact per-build mark for the sweep table (`ok`/`unsup`/`FAILED`).
fn build_mark(build: &RaceDefBuild) -> &'static str {
    match build {
        RaceDefBuild::Built(_) => "ok",
        RaceDefBuild::Unsupported => "unsup",
        RaceDefBuild::Failed(_) => "FAILED",
    }
}

/// Compact per-roster mark: `ok`, `ok+Ni` (built with issues),
/// `unsup` or `FAILED`.
fn roster_mark(build: &RosterBuild) -> String {
    match build {
        RosterBuild::Built(s) if s.issues.is_empty() => "ok".to_string(),
        RosterBuild::Built(s) => format!("ok+{}i", s.issues.len()),
        RosterBuild::Unsupported => "unsup".to_string(),
        RosterBuild::Failed(_) => "FAILED".to_string(),
    }
}

/// Print a sweep in the same style as the other catalog-wide audits:
/// one line per event with its deep-check outcome, failure detail
/// lines indented under it, then a per-city summary.
fn print_sweep(sweep: &CitySweep) {
    println!("== event sweep: {} ==", sweep.city);
    for t in &sweep.tables {
        match &t.error {
            Some(e) => println!("  {:<30} ERROR: {e}", t.logical),
            None => println!(
                "  {:<30} {:>2} rows ({} row diagnostics)",
                t.logical, t.rows, t.diagnostics
            ),
        }
    }
    for ev in &sweep.events {
        let table = format!("{:?}", ev.event_ref.table).to_lowercase();
        let status = match &ev.status {
            EventStatus::Ready => "ready".to_string(),
            EventStatus::Incomplete { missing } => {
                format!("incomplete ({})", missing.join(", "))
            }
        };
        println!(
            "  {:<10} {:>2} {:<12} {:<24} {:>2} records   defs {}/{}   rosters {}/{}",
            table,
            ev.event_ref.index,
            ev.stem,
            status,
            ev.records.len(),
            build_mark(&ev.defs[0]),
            build_mark(&ev.defs[1]),
            roster_mark(&ev.rosters[0]),
            roster_mark(&ev.rosters[1]),
        );
        for f in ev.failures() {
            println!("       failure: {f}");
        }
    }
    let ready = sweep.events.iter().filter(|e| e.status.is_ready()).count();
    println!(
        "  {}: {} events — {} ready, {} incomplete, {} strict failures ({} extra stems not cataloged)",
        sweep.city,
        sweep.events.len(),
        ready,
        sweep.events.len() - ready,
        sweep.failures().len(),
        sweep.extras,
    );
    for d in &sweep.diagnostics {
        println!("  note: {d}");
    }
    println!();
}

/// `mm2-inspect event <dir> --city <stem> --event <table>:<row>`
/// inspects one row; `--all` sweeps every cataloged event (`--city`
/// restricts the sweep to one stem).
pub fn run(
    dir: &Path,
    mods: Option<&Path>,
    city: Option<&str>,
    event_spec: Option<&str>,
    all: bool,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    if all {
        let ids = vehicle_ids(&vfs);
        let mut failures = Vec::new();
        for city in crate::race_cities(&vfs, city) {
            let report = sweep(&vfs, &city, &ids);
            print_sweep(&report);
            failures.extend(
                report
                    .failures()
                    .into_iter()
                    .map(|f| format!("{city}: {f}")),
            );
        }
        if strict && !failures.is_empty() {
            return Err(format!("strict event sweep: {} failures", failures.len()).into());
        }
        return Ok(());
    }
    let (city, spec) = match (city, event_spec) {
        (Some(c), Some(s)) => (c, s),
        // clap enforces the pair; this is the defensive leg.
        _ => {
            return Err(
                "--event <table>:<row> and --city <stem> are required without --all".into(),
            );
        }
    };
    let (table, index) = parse_event_spec(spec)?;
    let report = inspect(&vfs, city, table, index)?;
    print_report(&report);
    let failures = report.failures();
    if strict && !failures.is_empty() {
        return Err(format!("strict event audit: {} failures", failures.len()).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn write_bytes(dir: &Path, rel: &str, content: &[u8]) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    /// Ordered (circuit) races need a start line plus at least three
    /// legs — four authored rows.
    const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\r\n\
        0,0,0,0,10,0,0,0,\r\n\
        50,0,0,0,10,0,0,0,\r\n\
        100,0,0,0,10,0,0,0,\r\n\
        150,0,0,0,10,0,0,0,\r\n";

    const STARTS: &str = "0,0,0,90,0,0,0,0,0,\r\n5,0,0,90,0,0,0,0,0,\r\n";

    const OPP: &str = "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\r\n\
        0,0,0,175,0,0,0,0,0\r\n\
        50,0,0,0,0,0,0,0,0\r\n\
        100,0,0,0,0,0,0,0,0\r\n";

    /// `mm*data.csv` header — the authored column names verbatim.
    const TABLE_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, \
        Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, \
        Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";

    /// A 3-row circuit table: amateur wires 1 opponent, professional 2.
    fn circuit_table() -> String {
        let mut s = format!("{TABLE_HEADER}\r\n");
        for _ in 0..3 {
            s.push_str(
                "none, 0, 0, 0, 1, 0, 0.5, 0.5, 3, 50.0, 1, \
                 0, 0, 0, 2, 0, 0.5, 0.5, 3, 60.0, 2\r\n",
            );
        }
        s
    }

    /// `.aimap` wiring one amateur opponent onto `circuit0-a-0.opp`.
    const AIMAP: &str =
        "[Opponent]\r\n1\r\nvpbug circuit0-a-0.opp 1.00 0 50.0 0.7 0 0 0 0 0 1.0\r\n";

    /// `.aimap_p` wiring two professional opponents onto `-p-` routes.
    const AIMAP_P: &str = "[Opponent]\r\n2\r\n\
        vpbug circuit0-p-0.opp 1.00 0 50.0 0.7 0 0 0 0 0 1.0\r\n\
        vpford circuit0-p-1.opp 1.00 0 50.0 0.7 0 0 0 0 0 1.0\r\n";

    /// A synthetic install with one complete 3-row circuit table; row 0
    /// gets the full authored record set at both difficulties.
    fn synthetic_install() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "race/london/mmcircuitdata.csv",
            &circuit_table(),
        );
        write(dir.path(), "race/london/circuit0waypoints.csv", WAYPOINTS);
        write(dir.path(), "race/london/circuit0_strtpnts", STARTS);
        write(dir.path(), "race/london/circuit0.aimap", AIMAP);
        write(dir.path(), "race/london/circuit0.aimap_p", AIMAP_P);
        write(dir.path(), "race/london/circuit0-a-0.opp", OPP);
        write(dir.path(), "race/london/circuit0-p-0.opp", OPP);
        write(dir.path(), "race/london/circuit0-p-1.opp", OPP);
        // Empty-but-valid PTH1: magic, 0 paths, cursor 0.
        write_bytes(
            dir.path(),
            "race/london/circuit0.pathset",
            b"PTH1\0\0\0\0\0\0\0\0",
        );
        write(dir.path(), "tune/vpbug.info", "vpbug\n");
        write(dir.path(), "tune/vpford.info", "vpford\n");
        dir
    }

    fn vfs_of(dir: &Path) -> Vfs {
        crate::build_vfs(dir, None).unwrap()
    }

    #[test]
    fn parses_event_specs() {
        assert_eq!(
            parse_event_spec("circuit:0").unwrap(),
            (EventTableKind::Circuit, 0)
        );
        assert_eq!(
            parse_event_spec("race:2").unwrap(),
            (EventTableKind::Checkpoint, 2)
        );
        assert!(parse_event_spec("circuit").is_err());
        assert!(parse_event_spec("circuit:x").is_err());
        assert!(parse_event_spec("bogus:0").is_err());
    }

    #[test]
    fn inspects_a_complete_event() {
        let dir = synthetic_install();
        let vfs = vfs_of(dir.path());
        let report = inspect(&vfs, "london", EventTableKind::Circuit, 0).unwrap();
        assert_eq!(report.stem, "circuit0");
        assert!(report.status.is_ready());
        assert!(report.failures().is_empty());
        // Every attributed record is reported, aimap deep-parsed.
        let aimap = report
            .records
            .iter()
            .find(|r| r.kind == RaceFileKind::Aimap)
            .unwrap();
        assert!(aimap.detail.contains("1 opponents"));
        let pathset = report
            .records
            .iter()
            .find(|r| r.kind == RaceFileKind::Pathset)
            .unwrap();
        assert!(pathset.detail.contains("0 paths"));
        assert!(matches!(report.defs[0], RaceDefBuild::Built(_)));
        assert!(matches!(report.rosters[0], RosterBuild::Built(_)));
        assert!(report.unresolved_vehicles.is_empty());
    }

    #[test]
    fn unknown_event_is_a_lookup_error() {
        let dir = synthetic_install();
        let vfs = vfs_of(dir.path());
        assert!(inspect(&vfs, "london", EventTableKind::Circuit, 9).is_err());
        assert!(inspect(&vfs, "london", EventTableKind::Blitz, 0).is_err());
        assert!(inspect(&vfs, "sf", EventTableKind::Circuit, 0).is_err());
    }

    #[test]
    fn incomplete_event_reports_but_fails_strict() {
        let dir = synthetic_install();
        // Remove a required record: no aimap.
        fs::remove_file(dir.path().join("race/london/circuit0.aimap")).unwrap();
        let vfs = vfs_of(dir.path());
        let report = inspect(&vfs, "london", EventTableKind::Circuit, 0).unwrap();
        assert!(!report.status.is_ready());
        assert!(!report.failures().is_empty());
    }

    #[test]
    fn malformed_aimap_is_a_record_failure() {
        let dir = synthetic_install();
        write(dir.path(), "race/london/circuit0.aimap", "[[[not a section");
        let vfs = vfs_of(dir.path());
        let report = inspect(&vfs, "london", EventTableKind::Circuit, 0).unwrap();
        let aimap = report
            .records
            .iter()
            .find(|r| r.kind == RaceFileKind::Aimap)
            .unwrap();
        assert!(aimap.failed);
        assert!(!report.failures().is_empty());
    }

    #[test]
    fn unresolved_wired_vehicle_is_reported() {
        let dir = synthetic_install();
        fs::remove_file(dir.path().join("tune/vpbug.info")).unwrap();
        let vfs = vfs_of(dir.path());
        let report = inspect(&vfs, "london", EventTableKind::Circuit, 0).unwrap();
        assert_eq!(report.unresolved_vehicles, vec!["vpbug".to_string()]);
        assert!(!report.failures().is_empty());
    }

    #[test]
    fn crash_course_reports_unsupported_not_failed() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "race/london/mmcrashdata.csv",
            &format!(
                "{TABLE_HEADER}\r\n\
                lesson1, 0, 0, 0, 0, 0, 0.0, 0.0, 0, 0.0, 0, \
                0, 0, 0, 0, 0, 0.0, 0.0, 0, 0.0, 0\r\n"
            ),
        );
        write(
            dir.path(),
            "race/london/crash0.aimap",
            "[Opponent]\r\n0\r\n",
        );
        // Both lesson sub-tables are required; each Filename names a
        // waypoint CSV the catalog must resolve and attach.
        let crash_data = "Filename,Event,Checkpoints,TimeLimit,AmbDensity,extra,etra,\r\n\
            crash0a,0,1,26,0,0,0\r\n";
        write(dir.path(), "race/london/crash0data.csv", crash_data);
        write(dir.path(), "race/london/crash0data_p.csv", crash_data);
        write(dir.path(), "race/london/crash0a.csv", WAYPOINTS);
        let vfs = vfs_of(dir.path());
        let report = inspect(&vfs, "london", EventTableKind::CrashCourse, 0).unwrap();
        assert!(report.status.is_ready());
        assert!(matches!(report.defs[0], RaceDefBuild::Unsupported));
        assert!(matches!(report.defs[1], RaceDefBuild::Unsupported));
        assert!(matches!(report.rosters[0], RosterBuild::Unsupported));
        // The referenced lesson CSV is attached and reported.
        assert!(
            report
                .records
                .iter()
                .any(|r| r.logical == "race/london/crash0a.csv")
        );
        assert!(report.failures().is_empty());
    }

    #[test]
    fn sweep_reports_every_cataloged_event() {
        let dir = synthetic_install();
        let vfs = vfs_of(dir.path());
        let ids = vehicle_ids(&vfs);
        let sweep = sweep(&vfs, "london", &ids);
        // All three authored rows appear, in row order — complete or
        // not, none are filtered out of the denominator.
        assert_eq!(sweep.events.len(), 3);
        assert_eq!(
            sweep
                .events
                .iter()
                .map(|e| e.event_ref.index)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        // Row 0 is fully wired; rows 1–2 author no records on disk.
        assert!(sweep.events[0].failures().is_empty());
        assert!(!sweep.events[1].status.is_ready());
        assert!(!sweep.events[2].status.is_ready());
        // The sweep's strict surface names the incomplete rows.
        let failures = sweep.failures();
        assert!(failures.iter().any(|f| f.contains("circuit:1")));
        assert!(failures.iter().any(|f| f.contains("circuit:2")));
    }

    #[test]
    fn sweep_surfaces_a_record_failure() {
        let dir = synthetic_install();
        write(dir.path(), "race/london/circuit0.aimap", "[[[not a section");
        let vfs = vfs_of(dir.path());
        let ids = vehicle_ids(&vfs);
        let sweep = sweep(&vfs, "london", &ids);
        assert!(
            sweep
                .failures()
                .iter()
                .any(|f| f.contains("circuit:0") && f.contains("circuit0.aimap"))
        );
    }

    #[test]
    fn sweep_empty_catalog_is_a_failure() {
        let dir = synthetic_install();
        let vfs = vfs_of(dir.path());
        let ids = vehicle_ids(&vfs);
        // The install carries no `race/sf/` data at all.
        let sweep = sweep(&vfs, "sf", &ids);
        assert!(sweep.events.is_empty());
        assert!(
            sweep
                .failures()
                .iter()
                .any(|f| f.contains("catalog is empty"))
        );
    }
}
