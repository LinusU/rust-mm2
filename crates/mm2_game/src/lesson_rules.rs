//! Crash Course per-family leg rules (F21-C.2, `CC-8`/`CC-9`).
//!
//! The retail `Midtown2.exe` runs each lesson sub-event through one of
//! eight handlers picked by the row's `Event` code (dispatch at
//! `0x4169a0`; see `docs/research/crashcourse.md`). Every handler
//! shares one skeleton — the course (waypoint file) completing clears
//! the sub-event, the sub-event timer reaching zero fails it — and adds
//! its own conditions on top. [`LegRule`] is that per-handler decision,
//! pure: a family's rule reads one [`LegObservation`] per tick and
//! answers a [`Verdict`]; [`LegReport::from_verdict`] turns the answer
//! into the sequencer's [`LegReport`].
//!
//! | `Event` | handler | rule here |
//! | --- | --- | --- |
//! | 0 jump, 5 cross-traffic, 7 maneuver, 9 map | `0x4174d0`, `0x418100`, `0x419020` | [`LegRule::GateRun`] — the skeleton alone |
//! | 2 follow | `0x417b20` | [`LegRule::Follow`] |
//! | 3 cop chase | `0x416a60` | [`LegRule::CopChase`] |
//! | 4 corner | `0x4187b0` | [`LegRule::Corner`] |
//! | 8 stop | `0x419600` | [`LegRule::Stop`] |
//!
//! What this module does not own: where the observation comes from (the
//! session feeds it), which gates the course counts (`mm2_content`'s
//! leg definitions), and what a wreck costs the *run* — a rule only
//! reports the original's wreck policy ([`WreckPolicy`]).
//!
//! Every constant below was read from the executable (addresses in the
//! item docs), not tuned.

use std::fmt;

use crate::{LegFailure, LegReport, LegResult};

/// Miles per hour per metre per second — the executable's own
/// `SpeedMPH = Speed · 2.236025` (vehicle field `0x24c`, see
/// `docs/research/vehicle-physics/04-chassis-and-assists.md`).
pub const MPH_PER_MS: f32 = 2.236_025;

/// Follow: the player clears the lesson once the lead car has finished
/// its route (or the course completes) with the two cars closer than
/// this many metres (squared distance `< 100`, constant at `0x5af430`).
pub const FOLLOW_PASS_RANGE_M: f32 = 10.0;

/// Follow: the lead "escapes" — the lesson fails — once the cars are
/// further apart than this many metres (squared distance `> 10000`,
/// constant at `0x5af628`).
pub const FOLLOW_ESCAPE_RANGE_M: f32 = 100.0;

/// Cop chase and corner: a cop counts as "on you" at the finish when it
/// is closer than this many metres with a clear line of sight
/// (`0x41a1c0` over the executable's cop array; the distance constant
/// at `0x5af4ac` is `200.0`).
pub const PURSUER_SIGHT_RANGE_M: f32 = 200.0;

/// Corner: the minimum speed, in mph, when the row's cornering-speed
/// tail value is below `1.0` (`0x415e1f`–`0x415e2c`: `50.0`).
pub const CORNER_DEFAULT_MIN_MPH: f32 = 50.0;

/// Corner: how long, in seconds, the car may stay under the minimum
/// speed once it has reached it (`0x7960` of the lesson object, set to
/// `1.0` by the constructor at `0x415ba9` and never rewritten).
pub const CORNER_SPEED_GRACE_SECS: f32 = 1.0;

/// Corner: a real gate (past the start gate) passed before the minimum
/// speed was first reached fails the leg. The executable tests the
/// course's passed counter `>= 2`; that counter starts at 1 (the start
/// gate counts as passed, `0x4358d2`), so one cleared gate trips it.
pub const CORNER_GATES_BEFORE_SPEED: u32 = 1;

/// Stop: the lead car's `MaxDamage` while the lesson runs (`150000.0`,
/// set on the lead at `0x4198a3`, with `MedDamage` at half). The lead
/// is wrecked — the lesson passed — once its damage reaches this.
pub const STOP_LEAD_MAX_DAMAGE: f32 = 150_000.0;

/// Seconds a wrecked car waits before [`WreckPolicy::ResetCar`] resets
/// it in place (`CC-9`: the 3 s wreck delay of the follow, cop-chase and
/// stop handlers).
pub const WRECK_RESET_SECONDS: f32 = 3.0;

/// What the original does with a wrecked player car.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WreckPolicy {
    /// The leg fails ("Game over!") — jump, cross-traffic, corner,
    /// maneuver and map.
    FailLeg,
    /// The leg goes on: after 3 s the car is reset in place
    /// (`0x406180`) and the lesson state returns to running — follow,
    /// cop chase and stop.
    ResetCar,
}

/// A leg family's pass/fail rule, decoded from the sub-event row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LegRule {
    /// The shared skeleton and nothing else: course complete clears,
    /// timer expiry fails, a wreck fails. Retail jump, cross-traffic,
    /// maneuver and map handlers have no further condition.
    GateRun,
    /// Stay with the lead car (`Event 2`). Untimed: the row's
    /// `TimeLimit` is not used.
    Follow,
    /// Reach the finish and lose the pursuers (`Event 3`).
    CopChase,
    /// Keep the car above a minimum speed (`Event 4`).
    Corner {
        /// The minimum speed in mph — the row's cornering-speed tail
        /// value, or [`CORNER_DEFAULT_MIN_MPH`].
        min_speed_mph: f32,
    },
    /// Wreck the lead car before it reaches its destination
    /// (`Event 8`).
    Stop,
}

impl LegRule {
    /// The corner rule for a row whose cornering-speed tail value is
    /// `tail`: a value below `1.0` falls back to
    /// [`CORNER_DEFAULT_MIN_MPH`] (`0x415e1f`).
    pub fn corner(tail: f32) -> Self {
        let min_speed_mph = if tail >= 1.0 {
            tail
        } else {
            CORNER_DEFAULT_MIN_MPH
        };
        Self::Corner { min_speed_mph }
    }

    /// What a wrecked player car does to a leg of this family.
    pub fn wreck_policy(&self) -> WreckPolicy {
        match self {
            Self::GateRun | Self::Corner { .. } => WreckPolicy::FailLeg,
            Self::Follow | Self::CopChase | Self::Stop => WreckPolicy::ResetCar,
        }
    }

    /// Whether the family needs a [`LeadObservation`].
    pub fn needs_lead(&self) -> bool {
        matches!(self, Self::Follow | Self::Stop)
    }

    /// Whether the sub-event's `TimeLimit` is ignored. The follow
    /// handler never starts the timer (`0x415fe3`: only `Event 8`
    /// turns it on among the lead-car families).
    pub fn ignores_time_limit(&self) -> bool {
        matches!(self, Self::Follow)
    }

    /// Audit label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::GateRun => "gate-run",
            Self::Follow => "follow",
            Self::CopChase => "cop-chase",
            Self::Corner { .. } => "corner",
            Self::Stop => "stop",
        }
    }
}

/// The lead car, as a follow or stop leg sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeadObservation {
    /// Squared distance between the player and the lead car, m².
    pub distance_sq: f32,
    /// The lead has driven its route to the end (`0x53d6c0`: on its
    /// last route node, within 30 m of the goal point and — inside
    /// 20 m — past the goal plane).
    pub route_finished: bool,
    /// The lead's damage has reached its (lesson-set) maximum.
    pub wrecked: bool,
}

/// One tick of what a leg's rule can see.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LegObservation {
    /// The leg's course is complete (the last gate reached).
    pub course_complete: bool,
    /// Gates past the start gate that the player has cleared.
    pub gates_passed: u32,
    /// The sub-event timer ran out. Always `false` for an untimed
    /// leg (`TimeLimit 0`, or a family that ignores it).
    pub time_expired: bool,
    /// The player car is wrecked (damage above its maximum).
    pub wrecked: bool,
    /// The player's speed in mph ([`MPH_PER_MS`] of `|forward speed|`).
    pub speed_mph: f32,
    /// The lead car, for the families that have one.
    pub lead: Option<LeadObservation>,
    /// A cop within [`PURSUER_SIGHT_RANGE_M`] has a clear line of sight
    /// to the player.
    pub pursuer_in_sight: bool,
}

impl LegObservation {
    /// A tick with nothing happening: the course open, the timer
    /// running, a healthy car at rest and no pursuer.
    pub const IDLE: Self = Self {
        course_complete: false,
        gates_passed: 0,
        time_expired: false,
        wrecked: false,
        speed_mph: 0.0,
        lead: None,
        pursuer_in_sight: false,
    };
}

/// Memory a rule keeps across a leg's ticks (the corner rule's speed
/// bookkeeping). A new leg, or a retry, starts from
/// [`RuleState::default`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RuleState {
    reached_speed: bool,
    below_secs: f32,
}

impl RuleState {
    /// Whether the corner rule has seen the minimum speed yet.
    pub fn reached_speed(&self) -> bool {
        self.reached_speed
    }

    /// Seconds spent under the minimum speed since it was last above.
    pub fn below_secs(&self) -> f32 {
        self.below_secs
    }
}

/// A rule's answer for one tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Undecided; keep playing.
    Pending,
    /// The leg is cleared.
    Cleared,
    /// The leg ends the attempt.
    Failed(LegFailure),
    /// The player car is wrecked and the family's policy is
    /// [`WreckPolicy::ResetCar`]: the leg goes on, the car is reset
    /// after 3 s.
    ResetCar,
}

/// Why a rule could not be evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleError {
    /// A follow or stop leg was observed without a lead car — a
    /// lesson setup defect, never a pass.
    MissingLead,
    /// An observed number was NaN or infinite.
    NonFinite(&'static str),
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingLead => f.write_str("the leg's rule needs a lead car and none was seen"),
            Self::NonFinite(what) => write!(f, "observed {what} is not finite"),
        }
    }
}

impl std::error::Error for RuleError {}

impl LegRule {
    /// Decide one tick of `dt_secs` seconds.
    ///
    /// Order within a tick follows each handler's own: the checks that
    /// end the leg come first, the wreck last.
    pub fn evaluate(
        &self,
        state: &mut RuleState,
        obs: &LegObservation,
        dt_secs: f32,
    ) -> Result<Verdict, RuleError> {
        if !dt_secs.is_finite() || dt_secs < 0.0 {
            return Err(RuleError::NonFinite("tick length"));
        }
        if !obs.speed_mph.is_finite() {
            return Err(RuleError::NonFinite("speed"));
        }
        match self {
            Self::GateRun => Ok(skeleton(obs)),
            Self::Follow => follow(obs),
            Self::CopChase => Ok(cop_chase(obs)),
            Self::Corner { min_speed_mph } => corner(*min_speed_mph, state, obs, dt_secs),
            Self::Stop => stop(obs),
        }
    }
}

/// Complete, then timer, then wreck — the jump (`0x417747`),
/// maneuver/map (`0x4192d0`) and cross-traffic (`0x418518`) handlers.
fn skeleton(obs: &LegObservation) -> Verdict {
    if obs.course_complete {
        Verdict::Cleared
    } else if obs.time_expired {
        Verdict::Failed(LegFailure::TimedOut)
    } else if obs.wrecked {
        Verdict::Failed(LegFailure::Disabled)
    } else {
        Verdict::Pending
    }
}

fn lead_of(obs: &LegObservation) -> Result<LeadObservation, RuleError> {
    let lead = obs.lead.ok_or(RuleError::MissingLead)?;
    if !lead.distance_sq.is_finite() || lead.distance_sq < 0.0 {
        return Err(RuleError::NonFinite("lead distance"));
    }
    Ok(lead)
}

/// `0x417dba`–`0x417f4c`.
fn follow(obs: &LegObservation) -> Result<Verdict, RuleError> {
    let lead = lead_of(obs)?;
    let pass_sq = FOLLOW_PASS_RANGE_M * FOLLOW_PASS_RANGE_M;
    let escape_sq = FOLLOW_ESCAPE_RANGE_M * FOLLOW_ESCAPE_RANGE_M;
    if (lead.route_finished || obs.course_complete) && lead.distance_sq < pass_sq {
        Ok(Verdict::Cleared)
    } else if lead.distance_sq > escape_sq {
        Ok(Verdict::Failed(LegFailure::Objective))
    } else if obs.wrecked {
        Ok(Verdict::ResetCar)
    } else {
        Ok(Verdict::Pending)
    }
}

/// `0x416cf4`–`0x416e67`.
fn cop_chase(obs: &LegObservation) -> Verdict {
    if obs.course_complete {
        if obs.pursuer_in_sight {
            Verdict::Failed(LegFailure::Objective)
        } else {
            Verdict::Cleared
        }
    } else if obs.time_expired {
        Verdict::Failed(LegFailure::TimedOut)
    } else if obs.wrecked {
        Verdict::ResetCar
    } else {
        Verdict::Pending
    }
}

/// `0x418a9f`–`0x418fe8`.
fn corner(
    min_speed_mph: f32,
    state: &mut RuleState,
    obs: &LegObservation,
    dt_secs: f32,
) -> Result<Verdict, RuleError> {
    if !min_speed_mph.is_finite() || min_speed_mph <= 0.0 {
        return Err(RuleError::NonFinite("minimum speed"));
    }
    if obs.course_complete {
        return Ok(if obs.pursuer_in_sight {
            Verdict::Failed(LegFailure::Objective)
        } else {
            Verdict::Cleared
        });
    }
    if obs.time_expired {
        return Ok(Verdict::Failed(LegFailure::TimedOut));
    }
    if !state.reached_speed {
        if obs.gates_passed >= CORNER_GATES_BEFORE_SPEED {
            return Ok(Verdict::Failed(LegFailure::Objective));
        }
        if obs.speed_mph > min_speed_mph {
            state.reached_speed = true;
        }
    }
    if state.below_secs > 0.0 && obs.speed_mph > min_speed_mph {
        state.below_secs = 0.0;
    }
    if state.reached_speed && obs.speed_mph < min_speed_mph {
        state.below_secs += dt_secs;
        if state.below_secs > CORNER_SPEED_GRACE_SECS {
            state.reached_speed = false;
            return Ok(Verdict::Failed(LegFailure::Objective));
        }
    }
    Ok(if obs.wrecked {
        Verdict::Failed(LegFailure::Disabled)
    } else {
        Verdict::Pending
    })
}

/// `0x419600` state 3: lead wrecked, lead arrived, timer, own wreck.
fn stop(obs: &LegObservation) -> Result<Verdict, RuleError> {
    let lead = lead_of(obs)?;
    if lead.wrecked {
        Ok(Verdict::Cleared)
    } else if lead.route_finished {
        Ok(Verdict::Failed(LegFailure::Objective))
    } else if obs.time_expired {
        Ok(Verdict::Failed(LegFailure::TimedOut))
    } else if obs.wrecked {
        Ok(Verdict::ResetCar)
    } else {
        Ok(Verdict::Pending)
    }
}

impl LegReport {
    /// A rule's [`Verdict`] as the sequencer's report. `ticks` is the
    /// leg's own clock when it clears. `None` while the leg is
    /// undecided — and for [`Verdict::ResetCar`], which does not end
    /// the leg.
    pub fn from_verdict(attempt: u32, leg: u32, ticks: u64, verdict: Verdict) -> Option<Self> {
        let result = match verdict {
            Verdict::Cleared => LegResult::Cleared { ticks },
            Verdict::Failed(failure) => LegResult::Failed(failure),
            Verdict::Pending | Verdict::ResetCar => return None,
        };
        Some(Self {
            attempt,
            leg,
            result,
        })
    }
}
