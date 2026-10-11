//! `CrashLesson → [LessonLeg]` producer (F21-B.1): the runnable course
//! of each lesson's sub-events.
//!
//! A Crash Course lesson is one `mmcrashdata.csv` row whose
//! `crash<N>data{,_p}.csv` table lists one or more sub-events (legs) —
//! the exams chain two or three. Each leg names a waypoint CSV, an
//! authored `TimeLimit` and an `AmbDensity`. This module turns each leg
//! into the shared [`RaceDefinition`] the race runtime already drives,
//! so a lesson is a *sequence of definitions* rather than the single
//! checkpoint race `race_definition` refuses ([`RaceBuildError::
//! CrashCourseUnsupported`]).
//!
//! What a leg's definition is — and is not:
//!
//! - **Gate sequence** (`CC-8`, verified in the executable): row 0 is the
//!   start pose — the course's passed counter starts at 1 for it — and
//!   rows `1..` are the gates, the last being the finish. Every family
//!   but jump runs them in authored order under
//!   [`CheckpointRule::Ordered`]; a jump lesson takes its middle gates
//!   in any order and the last row only once they are all hit
//!   ([`CheckpointRule::AnyOrder`] with the last row as the separate
//!   finish). The retail files are course paths (`corner.csv` 24 rows,
//!   `london/exam1_1.csv` 15) and the two-row ones (`slalom`, `stop`,
//!   `follow`) reduce to "reach the far pose".
//! - **Timer** (inferred, `CC-7`): the sub-event's `TimeLimit` in seconds
//!   (the Blitz unit, `BLZ-3`); `0` is untimed — the two retail `corner`
//!   rows author it. The executable never starts the timer for a follow
//!   leg, so [`LegRule::ignores_time_limit`] families carry a limit the
//!   rule does not read.
//! - **The family rule** is [`LessonLeg::rule`] (`CC-9`): the gate run
//!   plus whatever the family's handler adds — the cornering speed in
//!   `tail[0]`, a follow lesson's distance to its lead car, a stop's
//!   wrecked lead, a cop chase's pursuer check. The definition here is
//!   only the course and the clock; `mm2_game::LegRule::evaluate` decides.
//! - **No actors.** Lead cars and police come from the lesson's aimap
//!   wiring ([`crate::AimapWiring`]); the definition's `opponents`/`cops`
//!   stay at the authored `mmcrashdata` values (0 on every retail row).

use mm2_formats::racefiles::RaceFileKind;
use mm2_game::{CheckpointRule, Difficulty, LegRule, RaceDefinition, RaceError};
use thiserror::Error;

use crate::crashcourse::{CrashLesson, LessonObjective, LessonSubEvent, LessonTableRole};
use crate::events::{CatalogEvent, EventCatalog, EventStatus, RecordContent};
use crate::race_def::{RaceBuildError, checkpoint, event_params, start_slots, time_limit_ticks};

/// One sub-event of a lesson, ready to run.
#[derive(Debug, Clone)]
pub struct LessonLeg {
    /// The authored `Filename` stem (`slalom`, `exam1_2`).
    pub filename: String,
    /// The inferred `Event`-code family — `rule` is its recovered
    /// pass/fail rule.
    pub objective: LessonObjective,
    /// `crash<N>:<line>` of the authored row, for diagnostics.
    pub source: String,
    /// The gate-sequence + timer definition (see the module docs).
    pub definition: RaceDefinition,
    /// The family's pass/fail rule recovered from the executable
    /// (`CC-9`); `None` for an `Event` code the executable handles but
    /// no retail row uses (`1`, `6`) or does not handle at all — the
    /// leg then runs the gate-run baseline alone.
    pub rule: Option<LegRule>,
}

/// Why a lesson's legs cannot be built.
#[derive(Debug, Error)]
pub enum LessonBuildError {
    /// The lesson's catalog event is not in the catalog it is built from.
    #[error("lesson {0} is not in the catalog")]
    NotInCatalog(String),
    /// The cataloged event's required records are missing or failed.
    #[error("lesson is not ready: {0:?}")]
    NotReady(EventStatus),
    /// No `data{,_p}.csv` table for the requested difficulty.
    #[error("no {0:?} sub-event table")]
    NoTable(Difficulty),
    /// The difficulty's table failed to parse.
    #[error("sub-event table failed: {0}")]
    TableFailed(String),
    /// The difficulty's table parsed to no sub-events — a lesson with
    /// nothing to run, rejected rather than passed vacuously.
    #[error("sub-event table has no rows")]
    NoSubEvents,
    /// A sub-event's `Filename` link did not resolve.
    #[error("{source_row}: waypoint link {filename}.csv is unresolved")]
    UnresolvedWaypoint {
        /// `crash<N>:<line>`.
        source_row: String,
        /// The authored link.
        filename: String,
    },
    /// The linked waypoint CSV is not among the event's parsed records.
    #[error("{source_row}: {logical} is not a parsed waypoint record of the event")]
    MissingWaypoints {
        /// `crash<N>:<line>`.
        source_row: String,
        /// The resolved logical path.
        logical: String,
    },
    /// A leg's rows or parameters do not make a runnable definition.
    #[error("{source_row}: {error}")]
    Leg {
        /// `crash<N>:<line>`.
        source_row: String,
        /// The named reason.
        #[source]
        error: RaceBuildError,
    },
}

/// Build every leg of a lesson at one difficulty, in authored order.
///
/// Fails on the first leg that cannot run: a lesson with a broken leg
/// is not playable, and silently dropping the leg would turn an exam
/// into a shorter one.
pub fn lesson_legs(
    catalog: &EventCatalog,
    lesson: &CrashLesson,
    difficulty: Difficulty,
) -> Result<Vec<LessonLeg>, LessonBuildError> {
    let event = catalog
        .get(&lesson.event_ref)
        .ok_or_else(|| LessonBuildError::NotInCatalog(lesson.stem.clone()))?;
    if !event.status.is_ready() {
        return Err(LessonBuildError::NotReady(event.status.clone()));
    }
    let role = match difficulty {
        Difficulty::Amateur => LessonTableRole::Amateur,
        Difficulty::Professional => LessonTableRole::Professional,
    };
    let table = lesson
        .tables
        .iter()
        .find(|t| t.role == role)
        .ok_or(LessonBuildError::NoTable(difficulty))?;
    if let Some(e) = &table.error {
        return Err(LessonBuildError::TableFailed(e.clone()));
    }
    if table.sub_events.is_empty() {
        return Err(LessonBuildError::NoSubEvents);
    }
    table
        .sub_events
        .iter()
        .map(|sub| build_leg(event, sub, difficulty))
        .collect()
}

fn build_leg(
    event: &CatalogEvent,
    sub: &LessonSubEvent,
    difficulty: Difficulty,
) -> Result<LessonLeg, LessonBuildError> {
    let leg_err = |error: RaceBuildError| LessonBuildError::Leg {
        source_row: sub.source.clone(),
        error,
    };
    let logical = sub
        .resolved
        .as_deref()
        .ok_or_else(|| LessonBuildError::UnresolvedWaypoint {
            source_row: sub.source.clone(),
            filename: sub.filename.clone(),
        })?;
    let rows = event
        .records
        .iter()
        .find_map(|r| match &r.content {
            RecordContent::Waypoints(f)
                if r.logical == logical
                    && matches!(r.kind, RaceFileKind::Waypoints | RaceFileKind::Csv) =>
            {
                Some(&f.rows)
            }
            _ => None,
        })
        .ok_or_else(|| LessonBuildError::MissingWaypoints {
            source_row: sub.source.clone(),
            logical: logical.to_string(),
        })?;
    // Start pose + at least one gate.
    if rows.len() < 2 {
        return Err(leg_err(RaceBuildError::TooFewRows {
            needed: 2,
            found: rows.len(),
        }));
    }

    // The leg's density is the sub-event's own `AmbDensity`; the
    // `mmcrashdata` block's `Ambient` is 0 on every retail row.
    let mut params = event.race_params(difficulty).clone();
    params.ambient = sub.amb_density;
    let (checkpoints, finish, rule) = gates(sub.objective, rows).map_err(leg_err)?;
    let definition = RaceDefinition {
        checkpoints,
        finish,
        rule,
        laps: 1,
        time_limit_ticks: if sub.timed() {
            Some(time_limit_ticks(sub.time_limit).map_err(leg_err)?)
        } else if sub.time_limit == 0.0 {
            None
        } else {
            // Negative / non-finite: a named error, never "untimed".
            return Err(leg_err(RaceBuildError::BadParam {
                field: "TimeLimit",
                value: sub.time_limit.to_string(),
            }));
        },
        params: event_params(&params).map_err(leg_err)?,
        countdown_ticks: mm2_game::DEFAULT_COUNTDOWN_TICKS,
        start_slots: start_slots(event, rows),
    };
    definition
        .validate()
        .map_err(|e: RaceError| leg_err(RaceBuildError::Invalid(e)))?;
    Ok(LessonLeg {
        filename: sub.filename.clone(),
        objective: sub.objective,
        source: sub.source.clone(),
        definition,
        rule: leg_rule(sub),
    })
}

/// The gate layout of a leg's waypoint rows (row 0 is the start pose).
///
/// Every family but jump runs the gates in authored order (`CC-8`: the
/// executable's checkpoint mode 5 tests the *current* gate only, its
/// passed counter starts at 1 for the start gate and the course
/// completes at the row count). A jump lesson uses mode 4: gates may
/// be taken in any order and the last row only completes the course
/// once every other gate has been hit — [`CheckpointRule::AnyOrder`]
/// with the last row as the separate finish.
fn gates(
    objective: LessonObjective,
    rows: &[mm2_formats::waypoints::Waypoint],
) -> Result<
    (
        Vec<mm2_game::Checkpoint>,
        Option<mm2_game::Checkpoint>,
        CheckpointRule,
    ),
    RaceBuildError,
> {
    if objective == LessonObjective::Jump {
        // Start pose, at least one gate, the finish.
        if rows.len() < 3 {
            return Err(RaceBuildError::TooFewRows {
                needed: 3,
                found: rows.len(),
            });
        }
        let (finish, gates) = rows[1..].split_last().expect("three rows checked above");
        return Ok((
            gates.iter().map(checkpoint).collect(),
            Some(checkpoint(finish)),
            CheckpointRule::AnyOrder,
        ));
    }
    Ok((
        rows[1..].iter().map(checkpoint).collect(),
        None,
        CheckpointRule::Ordered,
    ))
}

/// The recovered rule of a sub-event's family. The corner family reads
/// its minimum speed from `tail[0]` (`cornerspeed`/`Misc`); the other
/// families take nothing from the row but their `Event` code.
fn leg_rule(sub: &LessonSubEvent) -> Option<LegRule> {
    match sub.objective {
        LessonObjective::Jump
        | LessonObjective::CrossTraffic
        | LessonObjective::Maneuver
        | LessonObjective::Map => Some(LegRule::GateRun),
        LessonObjective::Follow => Some(LegRule::Follow),
        LessonObjective::CopChase => Some(LegRule::CopChase),
        LessonObjective::Corner => {
            let tail = sub.extras.first().copied().unwrap_or(0);
            Some(LegRule::corner(tail as f32))
        }
        LessonObjective::Stop => Some(LegRule::Stop),
        LessonObjective::Unknown(_) => None,
    }
}
