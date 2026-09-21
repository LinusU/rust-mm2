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
//! the staged-start heading, but the rest of the `.opp` columns remain
//! unverified (ledger UNK-11) and nothing here interprets them. The
//! ten-value `[Opponent]` parameter tail decodes through
//! [`OpponentSpec::drive_params`] into the driving-behavior vocabulary
//! mm2hook documents (`OpponentData`/`RegisterRoute`, R4) — a
//! documented-but-inferred mapping kept beside the raw values, not a
//! replacement for them. Problems that do not prevent building
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
    /// rows — the driving parameters [`OpponentSpec::drive_params`]
    /// decodes — one on the `race/sf/stunt0.aimap` extra.
    pub params: Vec<f32>,
    /// The wired driving line. `None` when the `.opp` reference failed
    /// to resolve — the authored slot is kept (the roster still knows
    /// the vehicle the original lineup fields) and the failure is
    /// recorded in [`OpponentRoster::issues`].
    pub route: Option<OpponentRoute>,
}

/// The `[Opponent]` parameter tail decoded into the driving-behavior
/// vocabulary mm2hook recovers for it (`OpponentData` +
/// `aiVehiclePhysics::RegisterRoute`, R4 — documented, not
/// original-verified; ledger UNK-11/RACE-14). The column-to-field
/// assignment is inferred from the recovered parameter names plus the
/// retail value distributions (536 rows measured): every column's
/// authored range matches the corresponding `RegisterRoute` default.
///
/// Columns the tail is too short to supply (`stunt0`'s single-value
/// row, malformed rows) stay `None` — consumers resolve their own
/// defaults; a missing column is authored absence, not a zero.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OpponentDriveParams {
    /// Column 0 — `maxThrottle` (RegisterRoute default 1.0): the
    /// throttle demand ceiling. Retail 0.57–1.00; amateur rows author
    /// the low end far more often than professional ones — the
    /// authored difficulty dial.
    pub max_throttle: Option<f32>,
    /// Column 1 — a 0/1 flag (set only on london `crash6`/`race12`
    /// rows on retail): `weirdPathfinding`/`unkFlag` candidate. Bound,
    /// unconsumed — no pathfinding variant exists to switch.
    pub weird_pathfinding: Option<bool>,
    /// Column 2 — a distance in metres (retail 50–150):
    /// `someDistancePadding` (default 75)/`TurnRadius` candidate.
    /// Bound, unconsumed — which distance it pads is unverified.
    pub distance_padding: Option<f32>,
    /// Column 3 — corner braking factor (retail 0.07–1.0, centred
    /// ~0.7): `cornerBrakingThreshold` (default 0.7)/
    /// `TurnSpeedMultiplier` candidate. Bound, unconsumed — its exact
    /// threshold semantics are unverified.
    pub corner_brake: Option<f32>,
    /// Column 4 — a 0/1 flag, purpose unknown. Bound, unconsumed.
    pub unused_flag: Option<bool>,
    /// Column 5 — `avoidTraffic`: sense ambient traffic. Bound but
    /// inert — no ambient-traffic class exists yet (F10 scope).
    pub avoid_traffic: Option<bool>,
    /// Column 6 — `avoidProps`: sense props. Bound but inert — the
    /// corridor senses participants only.
    pub avoid_props: Option<bool>,
    /// Column 7 — `avoidPlayers`: sense human participants (local and
    /// remote). `false` makes the player transparent to the corridor.
    pub avoid_players: Option<bool>,
    /// Column 8 — `avoidOpponents`: sense other AI opponents. Bound
    /// but **inert**: retail authors it ≈ universally 0, so consuming
    /// it under this inferred mapping would make every stock opponent
    /// blind to the rest of the field — either the original genuinely
    /// never avoids AI, or the flag order/polarity here is wrong. Held
    /// unverified until it can be measured; the corridor senses AI
    /// participants unconditionally meanwhile.
    pub avoid_opponents: Option<bool>,
    /// Column 9 — `cornerSpeedMultiplier` (RegisterRoute default 2.0):
    /// how much corner speed the driver carries. Retail 0.89–2.29;
    /// professional rows author the high end (>2.0).
    pub corner_speed_multiplier: Option<f32>,
}

impl OpponentSpec {
    /// The first authored parameter — the `maxThrottle` ceiling
    /// (0.57–1.00 on retail; inferred, UNK-11).
    pub fn skill(&self) -> Option<f32> {
        self.params.first().copied()
    }

    /// Decode the authored tail into the documented driving-parameter
    /// vocabulary (RACE-14). Columns are positional — a row short of
    /// ten values leaves the trailing fields `None`.
    pub fn drive_params(&self) -> OpponentDriveParams {
        let num = |i: usize| self.params.get(i).copied();
        let flag = |i: usize| num(i).map(|v| v != 0.0);
        OpponentDriveParams {
            max_throttle: num(0),
            weird_pathfinding: flag(1),
            distance_padding: num(2),
            corner_brake: num(3),
            unused_flag: flag(4),
            avoid_traffic: flag(5),
            avoid_props: flag(6),
            avoid_players: flag(7),
            avoid_opponents: flag(8),
            corner_speed_multiplier: num(9),
        }
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
