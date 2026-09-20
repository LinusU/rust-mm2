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
use mm2_formats::racefiles::RaceFileKind;
use mm2_game::{
    Checkpoint, CheckpointRule, Difficulty, EventTableKind, RaceDefinition, RaceError, RaceStart,
};
use thiserror::Error;

use crate::events::{CatalogEvent, EventStatus, RecordContent};

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
/// The returned definition is player-only: authored opponent/cop
/// counts live on [`CatalogEvent::race_params`] for the opponent slice
/// (F15/F20), and the authored time limit stays on the params until
/// its unit is verified (`UNK-4`, F12).
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
        laps: if rule == CheckpointRule::Ordered {
            params.num_laps.max(1) as u32
        } else {
            0
        },
        countdown_ticks: mm2_game::DEFAULT_COUNTDOWN_TICKS,
        start_slots: start_slots(event, rows),
    };
    definition.validate()?;
    Ok(definition)
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
        // finish the runtime arms once all gates are cleared (RACE-12).
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
    // Designed fallback (DSN-6): a single player slot behind the
    // start line, facing the course tangent row0→row1. The authored
    // `a` convention is unverified for this purpose, so the tangent
    // — which every event's data supports — supplies facing.
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
        position: line - d * 10.0,
        // Store in the authored `a` convention: forward = (sin a, cos a)
        // in XZ (see `Checkpoint::forward`).
        yaw_deg: d.x.atan2(d.z).to_degrees(),
    }]
}

/// `_strtpnts` rows as start slots — only SF circuit records ship
/// these on retail (F11-A); row 0 is the player slot by convention
/// (inferred, `WPT-3`/`UNK-17`). Angles stay verbatim — their
/// convention is the same unverified `a` column (`UNK-16`).
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
            yaw_deg: p.angle_deg,
        })
        .collect();
    if slots.is_empty() { None } else { Some(slots) }
}
