//! Crash Course lesson catalog (F21-A.1): the typed per-lesson view
//! over the shared [`EventCatalog`].
//!
//! A Crash Course event is *not* a checkpoint race — its row in
//! `mmcrashdata.csv` names a lesson (`lesson1`, `midtrm2`, `final13`),
//! and the lesson's sub-events live in its `crash<N>data.csv` /
//! `crash<N>data_p.csv` tables: each row names a waypoint CSV to run
//! on and an `Event` code. This module distills the cataloged records
//! into the shape an evaluator or audit consumes:
//!
//! - [`LessonStage`] — the authored tag decoded (`lesson`/`midtrm`/
//!   `final`), the ordering CC-2 verifies.
//! - [`LessonObjective`] — the `Event` column's *inferred* decode: the
//!   code↔lesson-family correlation is measured on every retail row
//!   (see `docs/research/crashcourse.md`) but the enum's semantics are
//!   unrecovered — labels are reported as inferred names, and any code
//!   outside the observed set stays [`LessonObjective::Unknown`]
//!   rather than a guessed fit.
//! - [`LessonTable`] — one `data.csv`/`data_p.csv` row set. The `_p`
//!   variant carries tighter limits on retail, matching the Amateur/
//!   Professional split elsewhere — inferred, not documented.
//! - [`AimapWiring`] — the lesson's own-stem aimap per difficulty:
//!   cop-chase lessons wire `[Police]` spawns (plus the one authored
//!   `[CopChaseDistance]`), follow lessons wire `[Opponent]` lead cars.
//!   Wired `.opp` names resolve through the VFS (case-insensitive like
//!   every authored reference — `Follow-1.opp` → `follow-1.opp`).
//!
//! `<object>_crash<N>` override records (`london_bridge_crash3.pathset`,
//! `london_parkedcar_crash0.pathset`…) classify under their own stems,
//! so the catalog surfaces them as extras; the `_crash<N>` suffix is
//! the inferred per-lesson attribution the lesson view re-binds
//! (mirroring the `<object>_<event>` convention — `pathset.md`).

use mm2_assets::Vfs;
use mm2_formats::aimap::Aimap;
use mm2_formats::racefiles::RaceFileKind;
use mm2_formats::rewards::RewardRow;
use mm2_game::{Difficulty, EventRef, EventTableKind};

use crate::events::{CatalogEvent, EventCatalog, EventRecord, EventStatus, RecordContent};
use crate::opponents::{RosterBuildError, event_aimap};

/// The authored `Description` tag decoded (`lesson3`, `midtrm1`,
/// `final13`). Row order is the authored sequence CC-2 verifies —
/// the stage names do not reorder it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LessonStage {
    /// `lesson<N>` — a teaching lesson.
    Lesson(u32),
    /// `midtrm<N>` — the group exam after each third lesson.
    Midterm(u32),
    /// `final<N>` — the course final (`final13` on retail).
    Final(u32),
    /// Any other authored tag (`none`, mod labels), kept verbatim.
    Other(String),
}

impl LessonStage {
    /// Decode an authored description tag. Case-sensitive like the
    /// authored data; a numeric suffix parses when present (`final13`
    /// → `Final(13)`, a bare `final` → `Other`).
    pub fn parse(description: &str) -> Self {
        for (prefix, stage) in [
            ("lesson", Self::Lesson as fn(u32) -> Self),
            ("midtrm", Self::Midterm),
            ("final", Self::Final),
        ] {
            if let Some(rest) = description.strip_prefix(prefix)
                && let Ok(n) = rest.parse::<u32>()
            {
                return stage(n);
            }
        }
        Self::Other(description.to_string())
    }
}

/// The `crash<N>data.csv` `Event` column decoded into the lesson
/// family the retail rows correlate with. **Inferred** — the mapping
/// below is measured per-file on all 26 retail lessons (every row's
/// `Filename` names a waypoint CSV whose stem makes the family
/// unambiguous for most codes) but no recovered structure or document
/// names the enum; `docs/research/crashcourse.md` holds the measured
/// table. Unknown codes keep their raw value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LessonObjective {
    /// `0` — jump lessons (`longjump`, `precisionjump`).
    Jump,
    /// `2` — follow-the-lead-vehicle lessons (`follow`, london
    /// `exam2_2`/`final1`); every retail row wires `numopp`=1 and an
    /// `[Opponent]` lead car.
    Follow,
    /// `3` — cop-chase lessons (`copchase`, sf `exam2_2`); retail rows
    /// wire `[Police]` spawns in the lesson aimap.
    CopChase,
    /// `4` — cornering lessons (`corner`, `corner0waypoints`,
    /// `exam1_1`, sf `final`); the only objective family carrying a
    /// nonzero `cornerspeed`/`Misc` tail value.
    Corner,
    /// `5` — traffic-crossing lessons (`frogger0waypoints`, `safe`).
    CrossTraffic,
    /// `7` — maneuver lessons (`slalom`, `oneeighty`, `reverse180`,
    /// london `exam1_2`/`exam1_3`/`final2`).
    Maneuver,
    /// `8` — braking lessons (`stop`, sf `exam1_2`); both retail rows
    /// also carry `numopp`=1 and their lessons wire a `vpford`
    /// opponent, like the follow family.
    Stop,
    /// `9` — the map lesson (london `map`).
    Map,
    /// A code outside the retail-observed set (1 and 6 are authored
    /// nowhere on retail) or inside it with no measured meaning.
    Unknown(i64),
}

impl LessonObjective {
    /// Decode a raw `Event` column value (inferred — see type docs).
    pub fn from_code(code: i64) -> Self {
        match code {
            0 => Self::Jump,
            2 => Self::Follow,
            3 => Self::CopChase,
            4 => Self::Corner,
            5 => Self::CrossTraffic,
            7 => Self::Maneuver,
            8 => Self::Stop,
            9 => Self::Map,
            other => Self::Unknown(other),
        }
    }

    /// Short audit label (`"jump"`, `"cop-chase"`, `"unknown(6)"`).
    pub fn label(&self) -> String {
        match self {
            Self::Jump => "jump".into(),
            Self::Follow => "follow".into(),
            Self::CopChase => "cop-chase".into(),
            Self::Corner => "corner".into(),
            Self::CrossTraffic => "cross-traffic".into(),
            Self::Maneuver => "maneuver".into(),
            Self::Stop => "stop".into(),
            Self::Map => "map".into(),
            Self::Unknown(code) => format!("unknown({code})"),
        }
    }
}

/// How well a lesson's *distinctive* pass/fail rule is evidenced
/// (F21-AC06's four-way split; the gate-run baseline every leg shares
/// is not a family rule). Ordered weakest first, so a multi-leg exam
/// takes the `min` of its legs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuleEvidence {
    /// No family evaluator exists: the leg passes on the designed
    /// gate-run baseline (DSN-72/DSN-74) and the original rule is not
    /// recovered (UNK-35).
    Unresolved,
    /// An evaluator exists and its positive/negative tests use
    /// self-authored synthetic data only.
    SyntheticOnly,
    /// An evaluator exists and its rule is recovered from the
    /// supplied installation, binary or a documented observation.
    OriginalVerified,
}

impl RuleEvidence {
    /// Audit label (`"unresolved"`, `"synthetic-only"`,
    /// `"original-verified"`).
    pub fn label(&self) -> &'static str {
        match self {
            Self::Unresolved => "unresolved",
            Self::SyntheticOnly => "synthetic-only",
            Self::OriginalVerified => "original-verified",
        }
    }
}

impl LessonObjective {
    /// Evidence level of this family's pass/fail rule. **Every family
    /// is `Unresolved` today**: no `Event`-code evaluator has been
    /// written because no rule (cornering speed, follow distance, stop,
    /// jump landing, cop-chase outcome, ...) has been recovered
    /// (UNK-35). The match is exhaustive on purpose — landing the first
    /// evaluator must change a named arm here, which is what moves the
    /// coverage report.
    pub fn rule_evidence(&self) -> RuleEvidence {
        match self {
            Self::Jump
            | Self::Follow
            | Self::CopChase
            | Self::Corner
            | Self::CrossTraffic
            | Self::Maneuver
            | Self::Stop
            | Self::Map
            | Self::Unknown(_) => RuleEvidence::Unresolved,
        }
    }
}

/// One `crash<N>data{,_p}.csv` row: the sub-event a lesson runs.
#[derive(Debug, Clone)]
pub struct LessonSubEvent {
    /// `Filename` column verbatim — the waypoint CSV stem.
    pub filename: String,
    /// Resolved logical path (`race/<city>/<filename>.csv`), `None`
    /// when the link does not resolve — also reported as an issue.
    pub resolved: Option<String>,
    /// `Event` column verbatim.
    pub code: i64,
    /// Inferred objective decode of `code`.
    pub objective: LessonObjective,
    /// `Checkpoints` column verbatim (1 on every retail row).
    pub checkpoints: i64,
    /// `TimeLimit` column verbatim (seconds inferred).
    pub time_limit: f32,
    /// `AmbDensity` column verbatim (ambient traffic fraction).
    pub amb_density: f32,
    /// Tail columns beyond the named five, kept raw — positions are
    /// fixed but the header names drift; the file's [`columns`]
    /// (CrashDataFile) are the only authored names for them.
    pub extras: Vec<i64>,
    /// Source file the row came from (`crash<N>data.csv` or the `_p`
    /// variant) plus its 1-based line for diagnostics.
    pub source: String,
}

impl LessonSubEvent {
    /// Whether the sub-event carries an authored countdown — the two
    /// retail `corner` lessons author `TimeLimit 0` (untimed inferred;
    /// kept as data, not hidden).
    pub fn timed(&self) -> bool {
        self.time_limit > 0.0
    }
}

/// Which `data*.csv` variant a [`LessonTable`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LessonTableRole {
    /// `<stem>data.csv` — inferred Amateur set.
    Amateur,
    /// `<stem>data_p.csv` — inferred Professional set (tighter limits
    /// on retail, the `_p` convention `.aimap_p` shares).
    Professional,
}

/// One difficulty's sub-event table.
#[derive(Debug, Clone)]
pub struct LessonTable {
    /// Which authored variant produced these rows.
    pub role: LessonTableRole,
    /// Source record (`race/<city>/crash<N>data{,_p}.csv`).
    pub logical: String,
    /// The file's authored header cells (the tail names are the only
    /// in-file column-name evidence).
    pub columns: Vec<String>,
    /// Sub-event rows in authored order.
    pub sub_events: Vec<LessonSubEvent>,
    /// Non-fatal parse notes from `CrashDataFile::diagnostics` (header
    /// quirks, skipped malformed rows) — surfaced so a dropped row can
    /// never silently shrink the audit. Informational, not a strict
    /// failure.
    pub diagnostics: Vec<mm2_formats::racedata::TableDiagnostic>,
    /// The record was expected but failed to resolve/parse.
    pub error: Option<String>,
}

/// One `[Opponent]` row wired by a lesson aimap — the lead vehicle of
/// a follow lesson or a scripted actor in an exam.
#[derive(Debug, Clone)]
pub struct WiredOpponent {
    /// Vehicle geo id (`vpcab`, `vpbullet`, `vpford`).
    pub vehicle: String,
    /// `*.opp` route name verbatim from the aimap.
    pub route: String,
    /// Resolved logical path through the VFS (case-insensitive, like
    /// every authored reference); `None` = dead reference — reported.
    pub resolved: Option<String>,
}

/// A lesson's own-stem aimap at one difficulty (`event_aimap`'s
/// preferred/fallback pick, so Amateur on `crash<N>.aimap`,
/// Professional on `.aimap_p` when both ship).
#[derive(Debug, Clone)]
pub struct AimapWiring {
    /// The record's logical path.
    pub logical: String,
    /// Difficulty whose record was used — differs from the requested
    /// one when only the other variant ships.
    pub used: Difficulty,
    /// `[Police]` spawn rows (cop-chase lessons: 14 on retail).
    pub police: usize,
    /// Distinct police vehicle ids, sorted.
    pub police_vehicles: Vec<String>,
    /// `[Opponent]` rows (follow/exam lead cars).
    pub opponents: Vec<WiredOpponent>,
    /// `[CopChaseDistance]` pursuit radius when authored (sf `crash5`
    /// only on retail).
    pub cop_chase_distance: Option<f32>,
    /// `[Exceptions]` road overrides (sf `crash1`/`crash2`/`crash4`/
    /// `crash12` each wire the same ten-road block; london none).
    pub exceptions: usize,
    /// `[Ambient Types/Density]` roster rows.
    pub ambient_types: usize,
    /// `Aimap::validate` issues, prefixed for reporting.
    pub issues: Vec<String>,
}

/// One cataloged Crash Course event viewed as a lesson.
#[derive(Debug, Clone)]
pub struct CrashLesson {
    /// Stable identity `(city, CrashCourse, index)` — the index is the
    /// authored order (the prerequisite sequence CC-2 verifies).
    pub event_ref: EventRef,
    /// File stem (`crash0` … `crash12`).
    pub stem: String,
    /// Authored `Description` tag verbatim.
    pub description: String,
    /// The tag decoded.
    pub stage: LessonStage,
    /// Catalog completeness (`Incomplete` events still report fully).
    pub status: EventStatus,
    /// Amateur / Professional `mmcrashdata.csv` parameter blocks,
    /// verbatim (CarType/TimeofDay/Weather on retail; limits live in
    /// the lesson tables).
    pub amateur: mm2_formats::racedata::RaceParams,
    /// Professional parameter block.
    pub professional: mm2_formats::racedata::RaceParams,
    /// The difficulty tables found (`data.csv`/`data_p.csv`), in
    /// authored-variant order.
    pub tables: Vec<LessonTable>,
    /// Own-stem aimap wiring per difficulty (Amateur/Professional
    /// slots; `Err` becomes `None` plus a `failures` entry).
    pub wiring: [Option<AimapWiring>; 2],
    /// Errors resolving the own-stem aimap per difficulty slot.
    pub wiring_errors: [Option<String>; 2],
    /// `<object>_crash<N>` records re-attributed from the catalog's
    /// unclaimed extras (labels + kinds; inferred suffix convention).
    pub override_records: Vec<crate::events::ExtraRecord>,
    /// Indexed `crash,N` reward rows attached to this event.
    pub rewards: Vec<RewardRow>,
    /// Per-lesson problems the audit reports (`failures` under
    /// `--strict`): unresolved filename links, missing tables, wired
    /// `.opp` names that do not resolve.
    pub issues: Vec<String>,
}

/// The Crash Course half of one city's catalog: every `mmcrashdata.csv`
/// row as a lesson, plus the extras the lesson attribution did not
/// claim (kept visible — the denominator is never filtered).
#[derive(Debug)]
pub struct CourseCatalog {
    /// City stem scanned.
    pub city: String,
    /// Lessons in authored table order.
    pub lessons: Vec<CrashLesson>,
    /// Catalog extras no `_crash<N>` suffix claims (`stunt0`,
    /// `ramp`, `evade0waypoints`…), sorted by label.
    pub remaining_extras: Vec<crate::events::ExtraRecord>,
}

/// The vehicle a school's lessons require (CC-4, help: "Crash Course"):
/// the Ford Mustang Fastback (`vpbullet`) at the San Francisco stunt
/// school, the London Cab (`vpcab`) at the London cabbie school.
/// `None` for any other city — a mod's course names no required car.
///
/// The help text names the cars; the ids are measured: the Fastback is
/// the `vpbullet` catalog row and the Cab `vpcab`, the same two ids the
/// midterm-3 / midterm-2 paint rewards bind (CC-6). The *per-lesson*
/// binding is not data-driven anywhere recovered — the same car serves
/// every lesson of a school here (designed reading; UNK-35).
pub fn required_vehicle(city: &str) -> Option<&'static str> {
    match city {
        "sf" => Some("vpbullet"),
        "london" => Some("vpcab"),
        _ => None,
    }
}

/// Build the lesson view of one cataloged Crash Course event.
pub fn crash_lesson(vfs: &Vfs, catalog: &EventCatalog, event: &CatalogEvent) -> CrashLesson {
    debug_assert_eq!(event.event_ref.table, EventTableKind::CrashCourse);
    let mut issues: Vec<String> = Vec::new();
    if let EventStatus::Incomplete { missing } = &event.status {
        issues.extend(missing.iter().map(|m| format!("incomplete: {m}")));
    }

    // Difficulty tables: own-stem DataCsv records — `data_p.csv` is
    // the inferred Professional set, plain `data.csv` Amateur.
    let mut tables = Vec::new();
    for rec in event.records.iter().filter(|r| {
        r.kind == RaceFileKind::DataCsv
            && [
                format!("{}data.csv", event.stem),
                format!("{}data_p.csv", event.stem),
            ]
            .iter()
            .any(|n| record_basename(r) == n)
    }) {
        let role = if rec.logical.ends_with("data_p.csv") {
            LessonTableRole::Professional
        } else {
            LessonTableRole::Amateur
        };
        let mut table = LessonTable {
            role,
            logical: rec.logical.clone(),
            columns: Vec::new(),
            sub_events: Vec::new(),
            diagnostics: Vec::new(),
            error: None,
        };
        match &rec.content {
            RecordContent::CrashData(file) => {
                table.columns = file.columns.clone();
                table.diagnostics = file.diagnostics.clone();
                table.sub_events = file
                    .rows
                    .iter()
                    .map(|row| sub_event(vfs, catalog, event, row, &mut issues))
                    .collect();
                if table.sub_events.is_empty() {
                    issues.push(format!("{} parses to 0 sub-events", rec.logical));
                }
            }
            RecordContent::Failed(e) => table.error = Some(e.clone()),
            _ => {}
        }
        tables.push(table);
    }
    for role in [LessonTableRole::Amateur, LessonTableRole::Professional] {
        if !tables.iter().any(|t| t.role == role) {
            issues.push(match role {
                LessonTableRole::Amateur => format!("no {}data.csv table", event.stem),
                LessonTableRole::Professional => format!("no {}data_p.csv table", event.stem),
            });
        }
    }

    // Own-stem aimap per difficulty — police/opponent wiring is what
    // turns a lesson's rows into its actors.
    let mut wiring: [Option<AimapWiring>; 2] = [None, None];
    let mut wiring_errors: [Option<String>; 2] = [None, None];
    for (slot, difficulty) in [Difficulty::Amateur, Difficulty::Professional]
        .into_iter()
        .enumerate()
    {
        match event_aimap(vfs, event, difficulty) {
            Ok((aimap, picked)) => {
                let basename = match picked.tag {
                    'a' => format!("{}.aimap", event.stem),
                    _ => format!("{}.aimap_p", event.stem),
                };
                let rec = event
                    .records
                    .iter()
                    .find(|r| record_basename(r) == basename)
                    .map(|r| r.logical.clone())
                    .unwrap_or(basename);
                wiring[slot] = Some(aimap_wiring(
                    vfs,
                    catalog,
                    &rec,
                    &aimap,
                    picked.used,
                    &mut issues,
                ));
            }
            Err(RosterBuildError::NoAimapRecord) => {
                // The required-record check already reports the
                // missing aimap via the Incomplete status.
            }
            Err(e) => {
                wiring_errors[slot] = Some(e.to_string());
                issues.push(format!("aimap ({difficulty:?}): {e}"));
            }
        }
    }

    // `<object>_crash<N>` extras — the per-lesson animated/parked-car
    // overrides (inferred suffix attribution, `pathset.md`).
    let suffix = format!("_{}", event.stem);
    let override_records: Vec<crate::events::ExtraRecord> = catalog
        .extras
        .iter()
        .filter(|x| x.label.ends_with(&suffix))
        .cloned()
        .collect();

    CrashLesson {
        event_ref: event.event_ref.clone(),
        stem: event.stem.clone(),
        description: event.description.clone(),
        stage: LessonStage::parse(&event.description),
        status: event.status.clone(),
        amateur: event.amateur.clone(),
        professional: event.professional.clone(),
        tables,
        wiring,
        wiring_errors,
        override_records,
        rewards: event.rewards.clone(),
        issues,
    }
}

/// Distill one crash-data row into a sub-event, resolving its
/// `Filename` waypoint link through the VFS (and the records the
/// catalog already attributed — the link claims the whole linked
/// stem, `follow.csv` → `follow-0.opp` included).
fn sub_event(
    vfs: &Vfs,
    catalog: &EventCatalog,
    event: &CatalogEvent,
    row: &mm2_formats::crashdata::CrashDataRow,
    issues: &mut Vec<String>,
) -> LessonSubEvent {
    let expected = format!("race/{}/{}.csv", catalog.city, row.filename);
    let resolved = if event.records.iter().any(|r| r.logical == expected)
        || vfs.resolve(&expected).is_some()
    {
        Some(expected.clone())
    } else {
        issues.push(format!(
            "waypoint link {}.csv unresolved ({}:{})",
            row.filename, event.stem, row.line
        ));
        None
    };
    LessonSubEvent {
        filename: row.filename.clone(),
        resolved,
        code: row.event,
        objective: LessonObjective::from_code(row.event),
        checkpoints: row.checkpoints,
        time_limit: row.time_limit,
        amb_density: row.amb_density,
        extras: row.extra.clone(),
        source: format!("{}:{}", event.stem, row.line),
    }
}

/// Distill a parsed aimap into a lesson's wiring summary, resolving
/// every wired `.opp` route through the VFS — the wire's basename is
/// matched case-insensitively (`Follow-1.opp` → `follow-1.opp`), the
/// same normalization `resolve` applies.
fn aimap_wiring(
    vfs: &Vfs,
    catalog: &EventCatalog,
    logical: &str,
    aimap: &Aimap,
    used: Difficulty,
    issues: &mut Vec<String>,
) -> AimapWiring {
    let opponents: Vec<WiredOpponent> = aimap
        .opponents
        .iter()
        .map(|o| {
            let logical = format!("race/{}/{}", catalog.city, o.waypoints);
            let resolved = vfs.resolve(&logical).map(|r| r.logical);
            if resolved.is_none() {
                issues.push(format!("wired route {} unresolved", o.waypoints));
            }
            WiredOpponent {
                vehicle: o.geo.clone(),
                route: o.waypoints.clone(),
                resolved,
            }
        })
        .collect();
    let mut police_vehicles: Vec<String> = aimap
        .police
        .iter()
        .map(|p| p.geo.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    police_vehicles.sort();
    AimapWiring {
        logical: logical.to_string(),
        used,
        police: aimap.police.len(),
        police_vehicles,
        opponents,
        cop_chase_distance: aimap.cop_chase_distance,
        exceptions: aimap.exceptions.len(),
        ambient_types: aimap.ambient_types.len(),
        issues: aimap.validate().iter().map(|i| i.to_string()).collect(),
    }
}

impl CourseCatalog {
    /// Scan the Crash Course slice of one city's event catalog.
    /// `catalog` must come from `EventCatalog::scan(vfs, city)` — the
    /// lessons are views over its parsed records, not a second scan.
    pub fn scan(vfs: &Vfs, catalog: &EventCatalog) -> Self {
        let lessons: Vec<CrashLesson> = catalog
            .events
            .iter()
            .filter(|e| e.event_ref.table == EventTableKind::CrashCourse)
            .map(|e| crash_lesson(vfs, catalog, e))
            .collect();
        let claimed: std::collections::BTreeSet<String> = lessons
            .iter()
            .flat_map(|l| l.override_records.iter().map(|r| r.label.clone()))
            .collect();
        Self {
            city: catalog.city.clone(),
            lessons,
            remaining_extras: catalog
                .extras
                .iter()
                .filter(|x| !claimed.contains(x.label.as_str()))
                .cloned()
                .collect(),
        }
    }

    /// Every issue across every lesson, prefixed `crash<N>` — the
    /// `--strict` denominator.
    pub fn failures(&self) -> Vec<String> {
        let mut failures = Vec::new();
        if self.lessons.is_empty() {
            failures.push("no crash-course lessons cataloged".to_string());
        }
        for l in &self.lessons {
            failures.extend(l.issues.iter().map(|i| format!("{} — {i}", l.stem)));
        }
        failures
    }
}

/// One lesson's row in the coverage report (F21-AC06).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LessonCoverage {
    /// File stem (`crash<N>`).
    pub stem: String,
    /// Authored tag decoded.
    pub stage: LessonStage,
    /// Sub-events (legs) in the Amateur / Professional table.
    pub legs: [usize; 2],
    /// Distinct objective families across both tables, first-seen order.
    pub objectives: Vec<LessonObjective>,
    /// Weakest evidence of any leg in either table; a lesson with no
    /// legs at all is `Unresolved`, never vacuously verified.
    pub evidence: RuleEvidence,
}

/// Every lesson of one city bucketed by [`RuleEvidence`]: the four-way
/// split F21-AC06 asks for. Structural catalog success is not rule
/// coverage — a lesson that merely builds and runs on the gate-run
/// baseline is `Unresolved` here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CourseCoverage {
    /// City stem.
    pub city: String,
    /// One row per lesson, authored order — nothing filtered.
    pub lessons: Vec<LessonCoverage>,
}

impl CourseCoverage {
    /// Lessons at exactly `evidence`.
    pub fn count(&self, evidence: RuleEvidence) -> usize {
        self.lessons
            .iter()
            .filter(|l| l.evidence == evidence)
            .count()
    }

    /// One audit line per lesson plus a summary line.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for l in &self.lessons {
            let families: Vec<String> = l.objectives.iter().map(|o| o.label()).collect();
            out.push_str(&format!(
                "{} {:?} legs {}/{} [{}] {}\n",
                l.stem,
                l.stage,
                l.legs[0],
                l.legs[1],
                families.join(","),
                l.evidence.label()
            ));
        }
        out.push_str(&format!(
            "{}: {} lessons — {} original-verified, {} synthetic-only, {} unresolved\n",
            self.city,
            self.lessons.len(),
            self.count(RuleEvidence::OriginalVerified),
            self.count(RuleEvidence::SyntheticOnly),
            self.count(RuleEvidence::Unresolved),
        ));
        out
    }
}

impl CrashLesson {
    /// This lesson's coverage row.
    pub fn coverage(&self) -> LessonCoverage {
        let mut legs = [0usize; 2];
        let mut objectives: Vec<LessonObjective> = Vec::new();
        let mut evidence: Option<RuleEvidence> = None;
        for table in &self.tables {
            let slot = match table.role {
                LessonTableRole::Amateur => 0,
                LessonTableRole::Professional => 1,
            };
            legs[slot] += table.sub_events.len();
            for sub in &table.sub_events {
                if !objectives.contains(&sub.objective) {
                    objectives.push(sub.objective);
                }
                let e = sub.objective.rule_evidence();
                evidence = Some(evidence.map_or(e, |w| w.min(e)));
            }
        }
        LessonCoverage {
            stem: self.stem.clone(),
            stage: self.stage.clone(),
            legs,
            objectives,
            evidence: evidence.unwrap_or(RuleEvidence::Unresolved),
        }
    }
}

impl CourseCatalog {
    /// The F21-AC06 coverage report over every lesson.
    pub fn coverage(&self) -> CourseCoverage {
        CourseCoverage {
            city: self.city.clone(),
            lessons: self.lessons.iter().map(CrashLesson::coverage).collect(),
        }
    }
}

/// The basename of a record's logical path.
fn record_basename(record: &EventRecord) -> &str {
    record.logical.rsplit('/').next().unwrap_or(&record.logical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_objective_family_claims_a_recovered_rule_yet() {
        // UNK-35: when the first evaluator lands, this must change with
        // it — the coverage report's original-verified count follows.
        for code in [0, 2, 3, 4, 5, 7, 8, 9, 1, 6, 42] {
            assert_eq!(
                LessonObjective::from_code(code).rule_evidence(),
                RuleEvidence::Unresolved,
                "code {code}"
            );
        }
        assert!(RuleEvidence::Unresolved < RuleEvidence::SyntheticOnly);
        assert!(RuleEvidence::SyntheticOnly < RuleEvidence::OriginalVerified);
    }
}
