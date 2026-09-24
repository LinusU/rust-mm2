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
//! the community format reference and mm2hook document (R3/R4 — the
//! flag-column order is documented, the float semantics corroborated
//! by `RegisterRoute` defaults) — kept beside the raw values, not a
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
/// vocabulary documented for it (ledger RACE-14). The column order is
/// **documented**, not just inferred: the community format reference
/// (angel-file-formats `AIMAP.md`, R3) publishes the same ten columns
/// and mm2hook's recovered `aiVehiclePhysics::RegisterRoute` signature
/// (R4) lists the flag arguments in exactly the tail's flag-column
/// order (`unkFlag, avoidTraffic, avoidProps, avoidPlayers,
/// avoidOpponents, weirdPathfinding` ↔ columns 1, 4–8). The float
/// columns corroborate by their `RegisterRoute` defaults: `maxThrottle`
/// 1.0 sits in column 0's 0.57–1.00, `cornerBrakingThreshold` 0.7 at
/// column 3's ~0.7 centre, `someDistancePadding` 75 inside column 2's
/// 50–150, `cornerSpeedMultiplier` 2.0 inside column 9's 0.89–2.29 —
/// 537 retail rows measured. An earlier inference had the flag block
/// shifted one column later; that assignment is superseded (it put a
/// 43%-set "unused" flag on ordinary rows and made `avoidOpponents`
/// ~never authored — both anomalies resolve under this order).
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
    /// Column 1 — a 0/1 flag the format reference calls
    /// unknown/unused (RegisterRoute's `unkFlag`). Retail authors it on
    /// only 3 rows — the same rows that set `weirdPathfinding`
    /// (london `crash6`/`race12`). Bound, unconsumed.
    pub unused_flag: Option<bool>,
    /// Column 2 — the obstacle look-ahead distance in metres (retail
    /// 50–150; the format reference's "Look Ahead Distance", feeding
    /// RegisterRoute's `someDistancePadding` = 75 default). Consumed as
    /// the avoidance corridor's reach — a designed reading of the
    /// documented name; the original's exact use is unrecovered.
    pub distance_padding: Option<f32>,
    /// Column 3 — `cornerBrakingThreshold` (RegisterRoute default 0.7;
    /// retail 0.07–1.0 centred ~0.7). The documented semantics are a
    /// brake-demand floor: the original skips braking when the corner's
    /// required brake power falls below it. Bound, unconsumed — our
    /// control law's corner brake is a binary engage, not a continuous
    /// demand, so there is no faithful quantity to compare yet.
    pub corner_brake: Option<f32>,
    /// Column 4 — `avoidTraffic`: sense ambient traffic. Bound but
    /// inert — ambient cars are not `Player` participants, so the
    /// corridor never sees the class (F10 scope).
    pub avoid_traffic: Option<bool>,
    /// Column 5 — `avoidProps`: sense props. Bound but inert — the
    /// corridor senses participants only.
    pub avoid_props: Option<bool>,
    /// Column 6 — `avoidPlayers`: sense human participants (local and
    /// remote). `false` makes the player transparent to the corridor.
    /// Retail authors both values (25% set) — real per-driver variance.
    pub avoid_players: Option<bool>,
    /// Column 7 — `avoidOpponents`: sense other AI opponents. `false`
    /// makes fellow AI transparent to the corridor — 59% of retail
    /// rows author exactly that (documented polarity: 1 = avoid), so
    /// stock opponents genuinely do not dodge each other. Consumed.
    pub avoid_opponents: Option<bool>,
    /// Column 8 — `weirdPathfinding`/`BadPathfinding` (RegisterRoute
    /// default false): the format reference reports "unusual and
    /// sometimes broken" pathfinding. Retail sets it on only 3 rows —
    /// london `crash6`'s follow car and `race12`. Bound, unconsumed —
    /// no alternate pathfinding mode exists to switch to.
    pub weird_pathfinding: Option<bool>,
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
            unused_flag: flag(1),
            distance_padding: num(2),
            corner_brake: num(3),
            avoid_traffic: flag(4),
            avoid_props: flag(5),
            avoid_players: flag(6),
            avoid_opponents: flag(7),
            weird_pathfinding: flag(8),
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
