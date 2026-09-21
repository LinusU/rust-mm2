//! `CatalogEvent → RaceDefinition` producer (F11-B.2).
//!
//! Turns a resolved catalog event's parsed waypoint rows into the
//! shared [`RaceDefinition`] contract so the app-side session loader
//! never touches CSV records itself. Mode conventions (original-rules
//! ledger `WPT-2`, inferred from retail row geometry):
//!
//! - Blitz/Checkpoint `AnyOrder`: row 0 is the start line (not a
//!   gate), the last row is the finish trigger, the middle rows are
//!   the any-order checkpoints.
//! - Circuit `Ordered`: the same rows form the closed course. Row 0
//!   is still the start line, but it participates as a gate — a
//!   lifted copy of it is appended last so each lap's final gate is
//!   the start line and `laps × rows` crossings complete the race.
//!
//! Trigger radius is the authored fifth-column value verbatim — the
//! [`Checkpoint`] contract already documents that mapping; the
//! `radius`/`poly count` label difference stays a provenance question
//! (`WPT-1`), not a runtime branch. Start slots come from a
//! `_strtpnts` record when the event ships one (SF circuits only on
//! retail); otherwise one designed player slot is derived from the
//! waypoint start line (`DSN-6`). Opponent `.opp` first-rows are AI
//! path anchors, not player grid slots (`UNK-17`).

use bevy::prelude::Vec3;
use mm2_assets::Vfs;
use mm2_formats::racedata::RaceParams;
use mm2_formats::racefiles::RaceFileKind;
use mm2_game::{
    Checkpoint, CheckpointRule, Densities, Difficulty, EventParams, EventRef, EventTableKind,
    RACE_TICK_HZ, RaceDefinition, RaceError, RaceStart, SessionConditions, TimeOfDay, Weather,
};
use thiserror::Error;

use crate::events::{CatalogEvent, EventCatalog, EventStatus, RecordContent};

/// Index of the player slot inside [`RaceDefinition::start_slots`].
/// With an authored `_strtpnts` grid the first row is the player's
/// (inferred, `WPT-3`); with no authored grid the producer emits a
/// single designed slot at index 0.
pub const PLAYER_SLOT: usize = 0;

/// Elevation gap (m) separating a circuit's start line from the gate
/// copy that closes each lap — the rows share XZ, so a small lift
/// keeps the two triggers distinct for marker consumers (designed).
const CIRCUIT_LINE_LIFT: f32 = 0.5;

/// Why an event cannot be converted to a `RaceDefinition`.
#[derive(Debug, Error)]
pub enum RaceBuildError {
    /// `CatalogEvent::status` is not `Ready` — required records are
    /// missing or failed; the catalog's `resolve` reports the detail.
    #[error("event is not ready: {0:?}")]
    NotReady(EventStatus),
    /// Crash Course rows describe lesson sub-events, not a single
    /// checkpoint race — deferred to the crash-course slice (F21).
    #[error("crash course events are not loadable yet")]
    CrashCourseUnsupported,
    /// No usable waypoint record on the event.
    #[error("event has no waypoint record")]
    NoWaypoints,
    /// The waypoint record has fewer rows than the mode needs
    /// (start + at least one gate + finish, or a closed circuit).
    #[error("need at least {needed} waypoint rows, found {found}")]
    TooFewRows {
        /// Rows the mode requires.
        needed: usize,
        /// Rows the record actually has.
        found: usize,
    },
    /// An authored parameter-block value is outside its legal range —
    /// a selector above the authored 0-3, a density outside 0..=1, a
    /// negative actor count or an unusable `TimeLimit`. Rejected, never
    /// clamped silently.
    #[error("event parameter {field} is out of range: {value}")]
    BadParam {
        /// The `mm*data.csv` column.
        field: &'static str,
        /// The rejected authored value.
        value: String,
    },
    /// The assembled definition failed the shared contract's own
    /// validation (e.g. a non-positive authored extent).
    #[error("invalid race definition: {0}")]
    Invalid(#[from] RaceError),
}

/// Build the runtime race definition for a catalog event at a
/// difficulty. The event must be `Ready` — use
/// [`EventCatalog::resolve`](crate::EventCatalog::resolve) first for
/// the detailed reason when it is not.
///
/// The returned definition is player-only: the authored opponent/cop
/// counts ride along in [`RaceDefinition::params`] for the systems
/// that spawn them (F15/F20) — nobody consumes them yet. Blitz rows
/// bind their authored `TimeLimit` as `time_limit_ticks` (seconds →
/// fixed ticks, `BLZ-3`/`DSN-7`); the constant `50`/`40` on
/// Checkpoint/Circuit rows is a likely-unused template value
/// (`UNK-4`), so those definitions stay untimed.
pub fn race_definition(
    event: &CatalogEvent,
    difficulty: Difficulty,
) -> Result<RaceDefinition, RaceBuildError> {
    if !event.status.is_ready() {
        return Err(RaceBuildError::NotReady(event.status.clone()));
    }
    if event.event_ref.table == EventTableKind::CrashCourse {
        return Err(RaceBuildError::CrashCourseUnsupported);
    }

    let file = event
        .records
        .iter()
        .find_map(|r| {
            if r.kind == RaceFileKind::Waypoints
                && let RecordContent::Waypoints(f) = &r.content
            {
                return Some(f);
            }
            None
        })
        .ok_or(RaceBuildError::NoWaypoints)?;

    let rule = match event.event_ref.table {
        EventTableKind::Circuit => CheckpointRule::Ordered,
        EventTableKind::Blitz | EventTableKind::Checkpoint => CheckpointRule::AnyOrder,
        EventTableKind::CrashCourse => unreachable!("rejected above"),
    };
    let rows = &file.rows;
    let (checkpoints, finish) = gates_for(rule, rows)?;
    let params = event.race_params(difficulty);

    let definition = RaceDefinition {
        checkpoints,
        finish,
        rule,
        // `NumLaps` is meaningful only on Ordered (Circuit) definitions —
        // the constant 3/4 on Blitz/Checkpoint rows is template junk
        // (`UNK-5`). There it binds like every other authored value: a
        // lap race needs a positive count that fits the field, and an
        // out-of-range value is a named build error, never a silent
        // clamp (`BadParam`).
        laps: if rule == CheckpointRule::Ordered {
            u32::try_from(params.num_laps)
                .ok()
                .filter(|&n| n >= 1)
                .ok_or(RaceBuildError::BadParam {
                    field: "NumLaps",
                    value: params.num_laps.to_string(),
                })?
        } else {
            0
        },
        time_limit_ticks: match event.event_ref.table {
            EventTableKind::Blitz => Some(time_limit_ticks(params.time_limit)?),
            _ => None,
        },
        params: event_params(params)?,
        countdown_ticks: mm2_game::DEFAULT_COUNTDOWN_TICKS,
        start_slots: start_slots(event, rows),
    };
    definition.validate()?;
    Ok(definition)
}

/// Authored `TimeLimit` → fixed ticks. The unit is seconds by strong
/// inference (`BLZ-3`): London `blitz0` is ~450 m of gates against an
/// authored `25`/`18` — only seconds lands in MM2 driving speeds;
/// minutes and frames are absurd at every row. A non-positive,
/// non-finite or unrepresentable authored value is a build error,
/// never a clamp.
fn time_limit_ticks(seconds: f32) -> Result<u32, RaceBuildError> {
    let bad = || RaceBuildError::BadParam {
        field: "TimeLimit",
        value: seconds.to_string(),
    };
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(bad());
    }
    let ticks = f64::from(seconds) * f64::from(RACE_TICK_HZ);
    if ticks > f64::from(u32::MAX) {
        return Err(bad());
    }
    Ok(ticks.round() as u32)
}

/// Distill one authored parameter block into the runtime's typed
/// [`EventParams`]. Selectors are validated against their authored
/// ranges (0-3, WLD-4), densities against 0..=1 (WLD-1) and actor
/// counts must be non-negative; an out-of-range value fails the event
/// build explicitly rather than being clamped (`RaceBuildError::BadParam`).
fn event_params(p: &RaceParams) -> Result<EventParams, RaceBuildError> {
    let bad = |field: &'static str, value: String| RaceBuildError::BadParam { field, value };
    let selector = |field: &'static str, value: i64| -> Result<u8, RaceBuildError> {
        u8::try_from(value).map_err(|_| bad(field, value.to_string()))
    };
    let conditions = SessionConditions {
        time_of_day: TimeOfDay::new(selector("TimeofDay", p.time_of_day)?)
            .map_err(|e| bad("TimeofDay", e.to_string()))?,
        weather: Weather::new(selector("Weather", p.weather)?)
            .map_err(|e| bad("Weather", e.to_string()))?,
    };
    let densities = Densities {
        traffic: p.ambient,
        pedestrians: p.peds,
    };
    densities
        .validate()
        .map_err(|e| bad("Ambient/Peds", e.to_string()))?;
    Ok(EventParams {
        conditions,
        densities,
        opponents: u32::try_from(p.opponents)
            .map_err(|_| bad("Opponents", p.opponents.to_string()))?,
        cops: u32::try_from(p.cops).map_err(|_| bad("Cops", p.cops.to_string()))?,
        car_type: p.car_type,
    })
}

fn checkpoint(w: &mm2_formats::waypoints::Waypoint) -> Checkpoint {
    Checkpoint {
        center: Vec3::new(w.position[0], w.position[1], w.position[2]),
        radius: w.width,
        height: mm2_game::DEFAULT_CHECKPOINT_HEIGHT,
        heading_deg: w.angle_deg,
        require_direction: false, // DSN-5: no verified direction rule.
    }
}

/// Split authored rows into gate triggers plus an optional finish
/// trigger. Row 0 is the start line everywhere (RACE-11).
fn gates_for(
    rule: CheckpointRule,
    rows: &[mm2_formats::waypoints::Waypoint],
) -> Result<(Vec<Checkpoint>, Option<Checkpoint>), RaceBuildError> {
    match rule {
        // Rows 1..n-1 are the any-order gates; the last row is the
        // finish the runtime arms once all gates are cleared (RACE-7).
        CheckpointRule::AnyOrder => {
            if rows.len() < 3 {
                return Err(RaceBuildError::TooFewRows {
                    needed: 3,
                    found: rows.len(),
                });
            }
            Ok((
                rows[1..rows.len() - 1].iter().map(checkpoint).collect(),
                Some(checkpoint(&rows[rows.len() - 1])),
            ))
        }
        // The start line participates as a gate, so a lifted copy of
        // it becomes the final gate of each lap — a lap completes
        // crossing the line. `rows.len()` gates per lap.
        CheckpointRule::Ordered => {
            if rows.len() < 4 {
                return Err(RaceBuildError::TooFewRows {
                    needed: 4,
                    found: rows.len(),
                });
            }
            let mut gates: Vec<Checkpoint> = rows[1..].iter().map(checkpoint).collect();
            let mut line = checkpoint(&rows[0]);
            line.center.y += CIRCUIT_LINE_LIFT;
            gates.push(line);
            Ok((gates, None))
        }
    }
}

fn start_slots(event: &CatalogEvent, rows: &[mm2_formats::waypoints::Waypoint]) -> Vec<RaceStart> {
    if let Some(slots) = authored_start_slots(event) {
        return slots;
    }
    // Designed fallback (DSN-6): a single player slot on the start
    // line, facing the course tangent row0→row1. The line is the one
    // point the authored data guarantees is on the course — backing
    // off along the tangent can leave the drivable surface (london
    // `blitz6`'s line sits on an elevated deck; 10 m behind it is
    // past the edge, over a void). The waypoint `a` column is a
    // bearing, not a facing (UNK-16 split), so the tangent supplies it.
    let line = Vec3::new(
        rows[0].position[0],
        rows[0].position[1],
        rows[0].position[2],
    );
    let next = Vec3::new(
        rows[1].position[0],
        rows[1].position[1],
        rows[1].position[2],
    );
    let d = (next - line).normalize_or_zero();
    vec![RaceStart {
        position: line,
        // `yaw_deg` is the vehicle-yaw convention: forward =
        // (−sin a, −cos a) in XZ — the tangent's bearing + 180°.
        yaw_deg: Some((-d.x).atan2(-d.z).to_degrees()),
    }]
}

/// `_strtpnts` rows as start slots — only SF circuit records ship
/// these on retail (F11-A); row 0 is the player slot by convention
/// (inferred, `WPT-3`/`UNK-17`). Nonzero angles stay verbatim — the
/// column is measured as a vehicle-yaw heading (the grid's ~+92° faces
/// the −X course on `cir1_strtpnts`), *not* the waypoint `a` bearing —
/// the two `a` columns sit exactly 180° apart (UNK-16 split). An
/// authored `a = 0` means no heading — the same zero-means-unset rule
/// the `.opp` staging field uses: retail `cir6_strtpnts` is the lone
/// all-zero grid and its routes stage ~180°, so a verbatim 0 would
/// face the whole grid backward. `None` lets each consumer derive a
/// course facing instead.
fn authored_start_slots(event: &CatalogEvent) -> Option<Vec<RaceStart>> {
    let file = event.records.iter().find_map(|r| {
        if let RecordContent::StartPoints(f) = &r.content {
            Some(f)
        } else {
            None
        }
    })?;
    let slots: Vec<RaceStart> = file
        .rows
        .iter()
        .map(|p| RaceStart {
            position: Vec3::new(p.position[0], p.position[1], p.position[2]),
            yaw_deg: (p.angle_deg != 0.0).then_some(p.angle_deg),
        })
        .collect();
    if slots.is_empty() { None } else { Some(slots) }
}

/// Outcome of building one event's [`RaceDefinition`] at one difficulty
/// inside a [`RaceDefReport`].
#[derive(Debug)]
pub enum RaceDefBuild {
    /// The definition built and passed `RaceDefinition::validate`.
    Built(RaceDefSummary),
    /// The event kind is deliberately not loadable yet (Crash Course —
    /// F21 scope). Reported and counted, never treated as a failure.
    Unsupported,
    /// The producer rejected the event — a `NotReady` catalog event, an
    /// out-of-range authored parameter, too few waypoint rows, or a
    /// definition that failed validation.
    Failed(RaceBuildError),
}

/// The parts of a built definition the catalog audit reports — enough
/// to show each event binds its own authored objectives and settings
/// rather than a shared template (F12-AC01).
#[derive(Debug)]
pub struct RaceDefSummary {
    /// Checkpoint triggers (`checkpoints.len()`).
    pub gates: usize,
    /// A separate finish trigger exists (any-order modes).
    pub finish: bool,
    /// Authored lap count (ordered modes; 0 elsewhere).
    pub laps: u32,
    /// Deadline in race ticks — timed modes only.
    pub time_limit_ticks: Option<u32>,
    /// Start slots the definition carries.
    pub start_slots: usize,
    /// Authored opponent count.
    pub opponents: u32,
    /// Authored cop count.
    pub cops: u32,
    /// Authored `CarType` column verbatim (UNK-1).
    pub car_type: i64,
    /// Authored time-of-day selector (WLD-4).
    pub time_of_day: u8,
    /// Authored weather selector (WLD-4).
    pub weather: u8,
}

/// One catalog event audited at both authored difficulties.
#[derive(Debug)]
pub struct RaceDefEntry {
    /// Stable identity — `(city, table, row)`.
    pub event_ref: EventRef,
    /// File stem (`blitz3`).
    pub stem: String,
    /// Amateur parameter-block build.
    pub amateur: RaceDefBuild,
    /// Professional parameter-block build.
    pub professional: RaceDefBuild,
}

/// Whole-city audit: every cataloged event run through the production
/// `CatalogEvent → RaceDefinition` producer at both difficulties
/// (F12-C). This is the complete-catalog structural check — the same
/// builder the session loader uses proves each event's authored
/// objectives convert into a validated runtime definition, and events
/// that cannot are named in [`RaceDefBuild::Failed`] rather than
/// dropped from the denominator.
#[derive(Debug)]
pub struct RaceDefReport {
    /// City stem audited.
    pub city: String,
    /// Table-level scan problems (missing or malformed `mm*data.csv`).
    /// Kept separate because a missing table means its rows never
    /// became events — the entries list alone would under-report.
    pub table_errors: Vec<String>,
    /// One entry per authored table row, in catalog order.
    pub entries: Vec<RaceDefEntry>,
}

impl RaceDefReport {
    /// Scan `race/<city>/` through the VFS and audit every cataloged
    /// event. Never fails as a whole — like the catalog itself, a
    /// partial install reports honestly instead of erroring out.
    pub fn scan(vfs: &Vfs, city: &str) -> Self {
        let catalog = EventCatalog::scan(vfs, city);
        let table_errors = catalog
            .tables
            .iter()
            .filter_map(|t| t.error.as_ref().map(|e| format!("{}: {e}", t.logical)))
            .collect();
        let entries = catalog
            .events
            .iter()
            .map(|event| RaceDefEntry {
                event_ref: event.event_ref.clone(),
                stem: event.stem.clone(),
                amateur: audit_build(event, Difficulty::Amateur),
                professional: audit_build(event, Difficulty::Professional),
            })
            .collect();
        Self {
            city: catalog.city,
            table_errors,
            entries,
        }
    }

    /// Builds that produced a validated definition (at most
    /// `2 × entries.len()`).
    pub fn built(&self) -> usize {
        self.entries
            .iter()
            .flat_map(|e| [&e.amateur, &e.professional])
            .filter(|b| matches!(b, RaceDefBuild::Built(_)))
            .count()
    }

    /// Builds on deliberately-deferred kinds (Crash Course).
    pub fn unsupported(&self) -> usize {
        self.entries
            .iter()
            .flat_map(|e| [&e.amateur, &e.professional])
            .filter(|b| matches!(b, RaceDefBuild::Unsupported))
            .count()
    }

    /// Builds the producer rejected.
    pub fn failed(&self) -> usize {
        self.entries
            .iter()
            .flat_map(|e| [&e.amateur, &e.professional])
            .filter(|b| matches!(b, RaceDefBuild::Failed(_)))
            .count()
    }

    /// Events carrying at least one `Failed` build — a per-difficulty
    /// failure (e.g. only the Professional block is out of range) still
    /// flags the event.
    pub fn failed_events(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| {
                [&e.amateur, &e.professional]
                    .iter()
                    .any(|b| matches!(b, RaceDefBuild::Failed(_)))
            })
            .count()
    }
}

fn audit_build(event: &CatalogEvent, difficulty: Difficulty) -> RaceDefBuild {
    match race_definition(event, difficulty) {
        Ok(def) => RaceDefBuild::Built(RaceDefSummary {
            gates: def.checkpoints.len(),
            finish: def.finish.is_some(),
            laps: def.laps,
            time_limit_ticks: def.time_limit_ticks,
            start_slots: def.start_slots.len(),
            opponents: def.params.opponents,
            cops: def.params.cops,
            car_type: def.params.car_type,
            time_of_day: def.params.conditions.time_of_day.get(),
            weather: def.params.conditions.weather.get(),
        }),
        Err(RaceBuildError::CrashCourseUnsupported) => RaceDefBuild::Unsupported,
        Err(e) => RaceDefBuild::Failed(e),
    }
}
