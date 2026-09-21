//! The shared opponent-roster contract (F15-A.1).
//!
//! One [`OpponentRoster`] is the difficulty-selected `[Opponent]`
//! lineup a race event wires in its `.aimap`/`.aimap_p` record: which
//! vehicles, which authored driving line each one follows, and the
//! authored tuning tail. It is the runtime-facing counterpart of the
//! parser records (`mm2_formats::aimap`, `mm2_formats::opp`) — spawn
//! and driving systems consume this, never CSV text.
//!
//! Authored scalars are kept verbatim: row 0's `brake` is measured as
//! the staged-start heading, but the rest of the `.opp` columns and the
//! ten-value parameter tail remain unverified (ledger UNK-11), so
//! nothing here interprets them. Problems that do not prevent building
//! the roster (a dead `.opp` reference, a wired count that disagrees
//! with the table row) are [`OpponentIssue`]s on the roster — they are
//! reported, never silently repaired.

use std::fmt;

use bevy::prelude::Vec3;

use crate::Difficulty;

/// One waypoint of an authored opponent driving line — a distilled
/// `.opp` row.
#[derive(Debug, Clone, PartialEq)]
pub struct OpponentRoutePoint {
    /// Authored position (MM2 world axes, unmirrored).
    pub position: Vec3,
    /// `brake` column — mislabelled in the authored header: nonzero
    /// values mark a *staging record*, and the value is a heading in
    /// vehicle-yaw degrees (measured on all 612 retail files: row 0
    /// carries it on 592, agreeing with the route's course direction
    /// within ~25° on 542 — deviations are scattered checkpoint
    /// stagings; `race/sf/race5-a-{5,6,7}` carry a second staging row
    /// mid-file, semantics open). Plain route rows author 0. Kept raw.
    pub brake: f32,
    /// `forward offset` column, raw.
    pub forward_offset: f32,
    /// `side offset` column, raw.
    pub side_offset: f32,
    /// `target speed` column — reads like a per-point speed profile;
    /// inferred, not verified.
    pub target_speed: f32,
    /// `speed start` column (0 on retail data), raw.
    pub speed_start: f32,
    /// `side start` column (0 on retail data), raw.
    pub side_start: f32,
}

/// An authored driving line: the `.opp` rows in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OpponentRoute {
    /// Waypoints in authored order.
    pub points: Vec<OpponentRoutePoint>,
}

impl OpponentRoute {
    /// Total polyline length in metres (3-D segment lengths).
    pub fn length(&self) -> f32 {
        self.points
            .windows(2)
            .map(|w| w[0].position.distance(w[1].position))
            .sum()
    }

    /// The authored start heading in vehicle-yaw degrees — row 0's
    /// nonzero `brake` value (measured: the `.opp` first row is a
    /// staging record carrying the opponent's staged facing; `to_radians`
    /// turns it into a spawn yaw). `None` when row 0 authors 0 —
    /// 20/612 retail files, where no facing is authored.
    pub fn start_heading_deg(&self) -> Option<f32> {
        self.points
            .first()
            .and_then(|p| (p.brake != 0.0).then_some(p.brake))
    }
}

/// One authored opponent: vehicle identity, tuning tail, driving line.
#[derive(Debug, Clone, PartialEq)]
pub struct OpponentSpec {
    /// Authored vehicle identity — a basename in the vehicle-catalog
    /// id space (`vpcoop`, `vpcoop2k`, `vpbug`). Whether the id resolves
    /// to a loadable vehicle is the audit's question, not the
    /// contract's.
    pub vehicle: String,
    /// Authored numeric tail, preserved raw: ten values on retail race
    /// rows — the first behaves like a 0-1 skill (inferred, UNK-11) —
    /// one on the `race/sf/stunt0.aimap` extra. Interpretation belongs
    /// to the difficulty model (F15-B), not this contract.
    pub params: Vec<f32>,
    /// The wired driving line. `None` when the `.opp` reference failed
    /// to resolve — the authored slot is kept (the roster still knows
    /// the vehicle the original lineup fields) and the failure is
    /// recorded in [`OpponentRoster::issues`].
    pub route: Option<OpponentRoute>,
}

impl OpponentSpec {
    /// The first authored parameter — behaves like a 0-1 skill level on
    /// retail data (0.57-1.00; inferred, UNK-11).
    pub fn skill(&self) -> Option<f32> {
        self.params.first().copied()
    }
}

/// A problem the roster reports rather than repairs.
#[derive(Debug, Clone, PartialEq)]
pub enum OpponentIssue {
    /// The difficulty's `.aimap` variant is absent; the other
    /// difficulty's roster was used — the only authored lineup the
    /// event ships (designed fallback policy).
    MissingVariant {
        /// The difficulty whose variant was wanted.
        wanted: Difficulty,
        /// The difficulty whose variant was used instead.
        used: Difficulty,
    },
    /// An `[Opponent]` row names a `.opp` file that resolves to no
    /// record on this event (e.g. `race/sf/stunt0.aimap`'s `opp-c0.2`).
    UnresolvedRoute {
        /// The referenced basename.
        name: String,
    },
    /// The referenced `.opp` record exists but failed to parse. Not
    /// reachable through a `Ready` catalog event today (a failed
    /// record already makes the event `Incomplete`), kept for
    /// completeness of the classification.
    RouteFailed {
        /// The referenced basename.
        name: String,
        /// The recorded parse failure.
        reason: String,
    },
    /// The referenced `.opp` carries the other difficulty's `-a-`/`-p-`
    /// tag than the selecting aimap variant's.
    WrongDifficultyTag {
        /// The referenced basename.
        name: String,
        /// The tag this variant's references should carry.
        expected: char,
    },
    /// The wired roster size disagrees with the table row's authored
    /// `Opponents` count (the `sf/race0` anomaly class, RACE-11).
    CountMismatch {
        /// `[Opponent]` rows actually wired.
        wired: usize,
        /// The table row's authored count, verbatim.
        table: i64,
    },
    /// An `.opp` record attributed to this event is referenced by no
    /// `[Opponent]` row in the selecting variant (e.g. the orphaned
    /// `race0-a-6.opp`). Only records whose difficulty tag matches the
    /// selected variant — or carry no tag — are counted.
    UnreferencedRoute {
        /// The record's basename.
        name: String,
    },
}

impl fmt::Display for OpponentIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingVariant { wanted, used } => {
                write!(f, "no {wanted:?} aimap variant — using the {used:?} roster")
            }
            Self::UnresolvedRoute { name } => {
                write!(f, "opponent route {name} resolves to no record")
            }
            Self::RouteFailed { name, reason } => {
                write!(f, "opponent route {name} failed to parse: {reason}")
            }
            Self::WrongDifficultyTag { name, expected } => write!(
                f,
                "opponent route {name} carries the wrong difficulty tag (expected -{expected}-)"
            ),
            Self::CountMismatch { wired, table } => write!(
                f,
                "aimap wires {wired} opponents but the table row authors {table}"
            ),
            Self::UnreferencedRoute { name } => {
                write!(f, "route record {name} is wired to no opponent")
            }
        }
    }
}

/// The difficulty-selected opponent lineup for one event.
#[derive(Debug, Clone, Default)]
pub struct OpponentRoster {
    /// One entry per authored `[Opponent]` row, in file order.
    pub entries: Vec<OpponentSpec>,
    /// Non-fatal problems found while building (row order).
    pub issues: Vec<OpponentIssue>,
}

impl OpponentRoster {
    /// Entries whose driving line resolved.
    pub fn resolved_routes(&self) -> usize {
        self.entries.iter().filter(|e| e.route.is_some()).count()
    }
}
