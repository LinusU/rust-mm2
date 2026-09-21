//! Shared race runtime (F11-B): countdown, swept checkpoint triggers,
//! per-participant progress and once-only results.
//!
//! This is the contract every mode implementation (Blitz F12, Checkpoint
//! F13, Circuit F14, Crash Course F21) drives — the modes choose a
//! [`CheckpointRule`] and a [`RaceDefinition`], the runtime here owns
//! the lifecycle:
//!
//! ```text
//!   Countdown { remaining } ──0──► Running ──all finished──► Complete
//!        │  input_locked()          │  clock ticks per fixed step
//!        └─ RaceStarted written     └─ crossings → SessionResult (ledger)
//!          exactly once
//! ```
//!
//! Classification notes (`docs/original-rules.md`):
//!
//! - `AnyOrder` clearing is the *documented* Blitz/Checkpoint rule
//!   (BLZ-1/CHK-1); `Ordered` is the *documented* Circuit rule (CIR-1 —
//!   missed checkpoints must be cleared). Neither ordering is imposed on
//!   the other modes: the definition carries it.
//! - A separate finish trigger gated on full clearance is *documented*
//!   (RACE-7: the finish line only appears after every checkpoint is
//!   cleared). `Ordered` definitions do not use it — finishing the last
//!   required lap is the finish.
//! - The trigger height band, the optional direction check and the
//!   3-second countdown are *designed* defaults: no authored value or
//!   documentation pins them down, so they are tunable on the
//!   definition/checkpoint, never baked-in claims.

use bevy::prelude::*;

use crate::config::{Densities, SessionConditions};
use crate::ids::PlayerId;
use crate::result::ResultId;

/// The fixed-step rate the shared race clock counts at — the
/// `Time::<Fixed>` the app and the headless smoke both install
/// (120 Hz). Authored time values (`TimeLimit`, seconds — BLZ-3)
/// convert through it.
pub const RACE_TICK_HZ: u32 = 120;

/// Designed default countdown: 3 s at the 120 Hz fixed step. No
/// authored value exists — the start countdown is not described in the
/// shipped documentation, so this is a tunable default, not an
/// original-rules claim.
pub const DEFAULT_COUNTDOWN_TICKS: u32 = 3 * RACE_TICK_HZ;

/// Designed default for a checkpoint's vertical half-extent. Authored
/// records carry a horizontal radius but no height; 8 m sits between
/// typical jump apexes and overpass grade separations, provisional
/// until real event data says otherwise (F12+).
pub const DEFAULT_CHECKPOINT_HEIGHT: f32 = 8.0;

/// One checkpoint trigger volume: a vertical cylinder of `radius`
/// around `center` (XZ) and `±height` around `center.y`. The authored
/// `radius`/`poly count` column supplies `radius`; `height` and
/// `require_direction` are contract fields the producer sets — no
/// authored value exists for either (designed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Checkpoint {
    /// Authored world position of the trigger.
    pub center: Vec3,
    /// Horizontal extent — the authored fifth-column value.
    pub radius: f32,
    /// Vertical half-extent — designed default
    /// [`DEFAULT_CHECKPOINT_HEIGHT`]; a car a bridge below or a jump
    /// above does not count.
    pub height: f32,
    /// Authored `a` column in degrees — a heading/orientation is
    /// inferred but unverified; kept verbatim for the direction check
    /// and for consumers that draw the gate.
    pub heading_deg: f32,
    /// Whether a crossing must travel the heading's forward direction.
    /// No verified rule requires it — default `false` so an unverified
    /// guess cannot silently discard legitimate crossings (designed).
    pub require_direction: bool,
}

impl Checkpoint {
    /// The trigger's forward axis in the ground plane, derived from
    /// [`heading_deg`](Self::heading_deg). The column's convention is
    /// inferred — `0°` is treated as `+Z`, increasing toward `+X` —
    /// and only matters when `require_direction` is set.
    pub fn forward(&self) -> Vec2 {
        let a = self.heading_deg.to_radians();
        Vec2::new(a.sin(), a.cos())
    }

    /// Whether the movement segment `from → to` crosses this trigger.
    ///
    /// Swept, not sampled: the test is against the whole segment, so a
    /// car covering more than a trigger's width in one fixed step still
    /// registers — it cannot skip a checkpoint by going fast (AC02).
    /// The check is cylindrical: closest XZ approach within `radius`
    /// *and* the segment's height there within `±height`.
    pub fn crossed(&self, from: Vec3, to: Vec3) -> bool {
        let a = Vec2::new(from.x - self.center.x, from.z - self.center.z);
        let ab = Vec2::new(to.x - from.x, to.z - from.z);
        // Parameter of the XZ segment's closest approach to the axis.
        let len2 = ab.length_squared();
        let t = if len2 > 0.0 {
            (-a.dot(ab) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let p = from.lerp(to, t);
        let dx = p.x - self.center.x;
        let dz = p.z - self.center.z;
        if dx.mul_add(dx, dz * dz) > self.radius * self.radius {
            return false;
        }
        if (p.y - self.center.y).abs() > self.height {
            return false;
        }
        if self.require_direction && len2 > 0.0 && (ab / len2.sqrt()).dot(self.forward()) <= 0.0 {
            return false;
        }
        true
    }
}

/// A starting slot: a pose the producer places a participant at.
/// `yaw_deg` is a heading in the *vehicle-yaw* convention — forward is
/// `(−sin a, −cos a)` in XZ, so the value converts to a spawn yaw with
/// `to_radians()` directly. That is measured, not inferred: the
/// `_strtpnts` `a` column and the `.opp` row-0 heading both author it
/// this way (retail `cir1_strtpnts` ≈ +92° faces the grid's −X course),
/// while the waypoint `a` column
/// ([`Checkpoint::heading_deg`]) is a course *bearing* — the same
/// column name, exactly 180° apart (the UNK-16 split).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RaceStart {
    /// Authored slot position.
    pub position: Vec3,
    /// Heading in vehicle-yaw degrees.
    pub yaw_deg: f32,
}

/// How a participant's checkpoint crossings accumulate — carried on
/// the definition, never imposed across modes (the spec forbids one
/// ordering rule for every mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointRule {
    /// Every checkpoint clears independently, in any order (documented
    /// Blitz/Checkpoint rule, BLZ-1/CHK-1). With a `finish` trigger the
    /// race ends when it is crossed after full clearance (RACE-7).
    AnyOrder,
    /// Checkpoints clear in authored order only — crossing a later one
    /// while an earlier is outstanding clears nothing (documented
    /// Circuit rule, CIR-1). The sequence wraps for `laps`; clearing
    /// the last checkpoint of the final lap finishes the race.
    ///
    /// Start-lap handling (implementation choice): lap 1 begins at the
    /// countdown release with `next` at the first gate — the producer
    /// puts participants on or behind the start line and makes the
    /// line itself each lap's *last* gate, so crossing the line is
    /// what closes a lap. Repeated crossings of the line — or of any
    /// gate — while an earlier gate is outstanding clear nothing, and
    /// a lap counts exactly once per completed sequence.
    Ordered,
}

/// The authored per-event settings a resolved event binds into the
/// runtime (F12-A): the `mm*data.csv` parameter block for the selected
/// difficulty, distilled into typed session-legal values. Their world
/// effects land with their own systems — weather/time-of-day F18,
/// traffic/pedestrians F10, opponents F15, cops F20 — and consumers
/// read them here rather than re-reading the table; a session's own
/// `SessionConfig.conditions`/`densities` stays the cruise/dev
/// fallback, the event-authored values take precedence while an event
/// runs.
///
/// Column accounting: `CarType` is carried verbatim (`UNK-1` — its
/// enum map is unverified), `NumLaps` binds as [`RaceDefinition::laps`]
/// where it is meaningful (its constant `3`/`4` on Blitz/Checkpoint
/// rows is a template artifact, unbound), `TimeLimit` binds as
/// [`RaceDefinition::time_limit_ticks`] where it is meaningful, and
/// the `Difficulty` column (constant `1` on every retail row) stays on
/// `CatalogEvent`'s raw `RaceParams` — it has no verified meaning.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EventParams {
    /// Authored weather + time-of-day selectors (WLD-4; index→name
    /// unverified, UNK-1).
    pub conditions: SessionConditions,
    /// Authored ambient-traffic / pedestrian densities (WLD-1).
    pub densities: Densities,
    /// Authored computer-opponent count (RACE-8); Blitz rows ship 0
    /// (BLZ-2).
    pub opponents: u32,
    /// Authored police count; Blitz and Circuit rows ship 0 (BLZ-2).
    pub cops: u32,
    /// Authored `CarType` column verbatim — carried, not interpreted
    /// (UNK-1).
    pub car_type: i64,
}

impl Default for EventParams {
    /// Neutral parameters for synthetic definitions: default
    /// conditions, designed-default densities, no authored actors.
    fn default() -> Self {
        Self {
            conditions: SessionConditions::default(),
            densities: Densities::DEFAULT,
            opponents: 0,
            cops: 0,
            car_type: 0,
        }
    }
}

/// Everything the shared runtime needs to run one authored event.
/// Produced by `mm2_content` from a catalog event's parsed records
/// (F11-B.2); the fields here are the runtime contract, not the file
/// layout.
#[derive(Debug, Clone)]
pub struct RaceDefinition {
    /// Checkpoint triggers in authored order.
    pub checkpoints: Vec<Checkpoint>,
    /// Optional separate finish trigger — only consulted under
    /// [`CheckpointRule::AnyOrder`], where it stays inert until every
    /// checkpoint is cleared (RACE-7). `Ordered` definitions ignore it.
    pub finish: Option<Checkpoint>,
    /// The clearing rule for this event.
    pub rule: CheckpointRule,
    /// Laps under [`CheckpointRule::Ordered`]; ignored otherwise.
    pub laps: u32,
    /// Race budget in fixed steps for timed events — the authored
    /// `TimeLimit` converted at [`RACE_TICK_HZ`] (BLZ-3; the seconds
    /// unit is inferred). `None` = untimed: Checkpoint/Circuit rows
    /// carry a constant `50`/`40` template value (UNK-4), so the
    /// producer leaves them untimed rather than enforce an unverified
    /// rule. The deadline is inclusive — a finish landing on the tick
    /// the clock reaches the limit still counts (DSN-7); everyone
    /// still unresolved after it records [`SessionOutcome::TimedOut`].
    pub time_limit_ticks: Option<u32>,
    /// The authored per-event settings this event binds (F12-A).
    pub params: EventParams,
    /// Countdown length in fixed steps before control releases
    /// ([`DEFAULT_COUNTDOWN_TICKS`] when nothing else asks).
    pub countdown_ticks: u32,
    /// Authored start slots — the producer assigns participants.
    pub start_slots: Vec<RaceStart>,
}

/// A [`RaceDefinition`] that cannot run.
#[derive(Debug, Clone, PartialEq)]
pub enum RaceError {
    /// No checkpoints at all — there is nothing to clear.
    NoCheckpoints,
    /// A non-positive or non-finite extent on a trigger.
    BadExtent,
    /// [`CheckpointRule::Ordered`] with `laps == 0`.
    NoLaps,
    /// `time_limit_ticks` of `Some(0)` — a race no one could ever run;
    /// a zero authored limit is rejected at the producer, not clamped.
    BadTimeLimit,
}

impl std::fmt::Display for RaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCheckpoints => write!(f, "race definition has no checkpoints"),
            Self::BadExtent => write!(f, "checkpoint with non-positive or non-finite extent"),
            Self::NoLaps => write!(f, "ordered race with zero laps"),
            Self::BadTimeLimit => write!(f, "time limit of zero fixed steps"),
        }
    }
}

impl std::error::Error for RaceError {}

impl RaceDefinition {
    /// Reject a definition that would behave oddly at runtime; the
    /// producer validates before inserting [`RaceState`].
    pub fn validate(&self) -> Result<(), RaceError> {
        if self.checkpoints.is_empty() {
            return Err(RaceError::NoCheckpoints);
        }
        if self.rule == CheckpointRule::Ordered && self.laps == 0 {
            return Err(RaceError::NoLaps);
        }
        if self.time_limit_ticks == Some(0) {
            return Err(RaceError::BadTimeLimit);
        }
        let extent_ok = |c: &Checkpoint| {
            c.radius.is_finite() && c.height.is_finite() && c.radius > 0.0 && c.height > 0.0
        };
        if self.checkpoints.iter().any(|c| !extent_ok(c))
            || self.finish.is_some_and(|c| !extent_ok(&c))
        {
            return Err(RaceError::BadExtent);
        }
        Ok(())
    }
}

/// Phase of the shared race lifecycle — inside the session's
/// `Countdown`/`Playing` phases; a race does not own session phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RacePhase {
    /// Counting down to control release; `remaining` fixed steps left.
    Countdown {
        /// Steps until [`RacePhase::Running`].
        remaining: u32,
    },
    /// Live: the clock advances one tick per fixed step.
    Running,
    /// Every participant has finished.
    Complete,
}

/// The session-scoped race resource: one active event's definition,
/// lifecycle phase and clock. Inserted by the producer when an event
/// session loads; removed by teardown (`drive_session`), so no old
/// timer survives a restart. [`generation`](Self::generation) matches
/// the session that created it — a stale resource is never stepped.
#[derive(Resource, Debug)]
pub struct RaceState {
    /// The event being run.
    pub definition: RaceDefinition,
    /// Lifecycle phase.
    pub phase: RacePhase,
    /// Ticks elapsed since the race went `Running` — the race clock
    /// results timestamp against. Frozen while the session is paused,
    /// so pause/resume is deterministic.
    pub clock: u64,
    /// Session generation this race belongs to.
    pub generation: u64,
}

impl RaceState {
    /// A race for `generation`, starting in countdown.
    pub fn new(definition: RaceDefinition, generation: u64) -> Self {
        Self {
            phase: RacePhase::Countdown {
                remaining: definition.countdown_ticks,
            },
            definition,
            clock: 0,
            generation,
        }
    }

    /// Whether gameplay input should be held from the simulation —
    /// the contract consumers (vehicle input) gate on this together
    /// with the session phase.
    pub fn input_locked(&self) -> bool {
        matches!(self.phase, RacePhase::Countdown { .. })
    }

    /// Whether this resource belongs to an older session generation —
    /// teardown removes it, but frames between `begin` and the
    /// producer's re-insert must never step a dead race's clock.
    pub fn is_stale(&self, generation: u64) -> bool {
        self.generation != generation
    }

    /// Fixed steps left on the time limit — `None` for untimed
    /// definitions, `Some(0)` once the deadline has been reached (the
    /// clock does not stop counting on its own).
    pub fn time_remaining(&self) -> Option<u32> {
        self.definition
            .time_limit_ticks
            .map(|limit| limit.saturating_sub(self.clock.min(u64::from(limit)) as u32))
    }
}

/// Written exactly once, when the countdown releases control (AC03).
/// Consumers (HUD, input, future network) observe the unlock rather
/// than re-deriving it.
#[derive(Message, Debug, Clone, Copy)]
pub struct RaceStarted;

/// Where one participant stands in the race.
#[derive(Debug, Clone, PartialEq)]
pub enum ParticipantState {
    /// Waiting for the countdown — crossings do not count yet.
    AwaitingStart,
    /// Racing.
    Racing,
    /// Finished — carries the race-clock time and the minted result's
    /// identity, so the finish is recorded once and stays attributable
    /// (AC04).
    Finished {
        /// [`RaceState::clock`] value at the finishing step.
        race_ticks: u64,
        /// The recorded [`SessionResult`](crate::SessionResult)'s id.
        result: ResultId,
    },
    /// The time limit expired with the event's objectives still open
    /// (BLZ-1: the finish must come before time runs out). Recorded
    /// once like a finish — the deadline is inclusive, so a finish
    /// landing on the expiry tick itself wins (DSN-7).
    TimedOut {
        /// [`RaceState::clock`] value at the expiring step — always
        /// the limit's tick: expiry flips every unresolved participant
        /// and completes the race on that step.
        race_ticks: u64,
        /// The recorded [`SessionResult`](crate::SessionResult)'s id.
        result: ResultId,
    },
}

/// Per-participant race progress — a component on the participant's
/// entity, so it despawns with the session like everything else it
/// owns. Only the rule authority mutates it; a predicted client
/// receives progress from the network (F25+).
#[derive(Component, Debug, Clone)]
pub struct RaceProgress {
    /// Lifecycle state.
    pub state: ParticipantState,
    /// Per-checkpoint cleared flags. Under `AnyOrder` they are
    /// independent; under `Ordered` they are a prefix of authored order
    /// and reset each lap.
    cleared: Vec<bool>,
    /// `Ordered` rule: index of the next checkpoint this participant
    /// must clear.
    pub next: usize,
    /// `Ordered` rule: completed laps.
    pub lap: u32,
    /// Total trigger crossings that cleared something — evidence for
    /// HUD/debugging.
    pub crossings: u32,
    /// Previous step's position; `None` breaks the swept segment so a
    /// spawn, teleport or reset cannot count the jump as a crossing.
    last_position: Option<Vec3>,
}

/// What [`RaceProgress::advance`] decided for one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressOutcome {
    /// Still racing.
    Racing,
    /// This segment completed the race — the caller mints and records
    /// the [`SessionResult`](crate::SessionResult).
    Finished,
}

impl RaceProgress {
    /// A participant for `definition`, awaiting the countdown.
    pub fn new(definition: &RaceDefinition) -> Self {
        Self {
            state: ParticipantState::AwaitingStart,
            cleared: vec![false; definition.checkpoints.len()],
            next: 0,
            lap: 0,
            crossings: 0,
            last_position: None,
        }
    }

    /// A participant joining mid-race: `AwaitingStart` while the race
    /// still counts down, `Racing` immediately once it is running
    /// (late join does not hold anyone to a countdown that already
    /// happened).
    pub fn join(definition: &RaceDefinition, race: &RaceState) -> Self {
        let mut p = Self::new(definition);
        if matches!(race.phase, RacePhase::Running) {
            p.state = ParticipantState::Racing;
        }
        p
    }

    /// Break the swept segment — call on teleport/reset so the jump
    /// cannot clear checkpoints it physically skipped. The next step
    /// re-anchors at the new position.
    pub fn break_segment(&mut self) {
        self.last_position = None;
    }

    /// Number of checkpoints cleared this lap (`Ordered`) or in total
    /// (`AnyOrder`).
    pub fn cleared_count(&self) -> usize {
        self.cleared.iter().filter(|c| **c).count()
    }

    /// Whether checkpoint `index` is cleared.
    pub fn is_cleared(&self, index: usize) -> bool {
        self.cleared.get(index).copied().unwrap_or(false)
    }

    /// Un-cleared checkpoint indices in authored order — the
    /// "remaining checkpoints" the navigation arrow can target
    /// (RACE-6). Under `Ordered` the flags are a prefix of authored
    /// order, so this is `next..len` while the lap runs.
    pub fn remaining(&self) -> impl Iterator<Item = usize> + '_ {
        self.cleared
            .iter()
            .enumerate()
            .filter_map(|(i, c)| (!c).then_some(i))
    }

    /// Feed one fixed-step segment to the participant: `to` is the
    /// entity's position this step, `last_position` the previous
    /// step's. Every checkpoint the segment crosses is consumed in one
    /// pass — a fast car cannot skip *through* a trigger, nor past two
    /// of them (AC02).
    ///
    /// Only a `Racing` participant accumulates progress — the rule
    /// authority gates on the lifecycle too, but the guard lives here
    /// so a resolved participant can never re-finish or keep clearing
    /// gates if positions are still fed in, and an `AwaitingStart`
    /// participant's steps only re-anchor the segment (AC04's
    /// once-only-result rule is a contract property, not a caller
    /// discipline).
    pub fn advance(&mut self, definition: &RaceDefinition, to: Vec3) -> ProgressOutcome {
        if self.state != ParticipantState::Racing {
            self.last_position = Some(to);
            return ProgressOutcome::Racing;
        }
        let Some(from) = self.last_position.replace(to) else {
            // Anchoring step after spawn/teleport: no segment, no
            // crossings.
            return ProgressOutcome::Racing;
        };
        match definition.rule {
            CheckpointRule::AnyOrder => {
                for (i, cp) in definition.checkpoints.iter().enumerate() {
                    if !self.cleared[i] && cp.crossed(from, to) {
                        self.cleared[i] = true;
                        self.crossings += 1;
                    }
                }
                if self.cleared.iter().all(|c| *c) {
                    let finished = match &definition.finish {
                        // RACE-7: the finish trigger is inert until
                        // every checkpoint is cleared — including the
                        // segment that clears the last one.
                        Some(f) => f.crossed(from, to),
                        None => true,
                    };
                    if finished {
                        return ProgressOutcome::Finished;
                    }
                }
            }
            CheckpointRule::Ordered => {
                while let Some(cp) = definition.checkpoints.get(self.next) {
                    if !cp.crossed(from, to) {
                        break;
                    }
                    self.cleared[self.next] = true;
                    self.crossings += 1;
                    self.next += 1;
                    if self.next == definition.checkpoints.len() {
                        self.lap += 1;
                        if self.lap >= definition.laps {
                            return ProgressOutcome::Finished;
                        }
                        self.next = 0;
                        self.cleared.fill(false);
                    }
                }
            }
        }
        ProgressOutcome::Racing
    }
}

/// What the navigation arrow points at (RACE-6): an un-cleared
/// checkpoint gate, or the finish trigger once every gate is cleared
/// (the finish is what remains — RACE-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavTarget {
    /// Index into [`RaceDefinition::checkpoints`].
    Gate(usize),
    /// [`RaceDefinition::finish`].
    Finish,
}

impl NavTarget {
    /// World position of the target on `definition`.
    pub fn position(self, definition: &RaceDefinition) -> Option<Vec3> {
        match self {
            Self::Gate(i) => definition.checkpoints.get(i).map(|c| c.center),
            Self::Finish => definition.finish.as_ref().map(|c| c.center),
        }
    }
}

/// Which checkpoint the player's navigation arrow tracks when it is
/// not following the nearest gate (RACE-6: the documented keys cycle
/// the arrow through the remaining checkpoints). A component on the
/// participant entity — session-owned like [`RaceProgress`], so a
/// restart cannot keep a stale pick (AC05).
#[derive(Component, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TargetSelection {
    /// Picked checkpoint index, or `None` for nearest-remaining.
    pub picked: Option<usize>,
}

/// The objective the navigation arrow tracks for a participant at
/// `position` (RACE-6).
///
/// The compass arrow is a Blitz/Checkpoint instrument — HUD-2's
/// instrument list scopes it to those modes and gives Circuit the lap
/// record instead — so `Ordered` definitions return `None`. A `picked`
/// gate that is out of range or already cleared falls back to the
/// nearest un-cleared gate (XZ distance: the triggers are vertical
/// cylinders, so height is not part of "nearest"). Once every gate is
/// cleared the arrow tracks the finish trigger while the definition
/// has one (inferred — RACE-6 names only checkpoints; the armed finish
/// is the remaining objective).
pub fn navigation_target(
    definition: &RaceDefinition,
    progress: &RaceProgress,
    picked: Option<usize>,
    position: Vec3,
) -> Option<NavTarget> {
    if definition.rule != CheckpointRule::AnyOrder {
        return None;
    }
    let mut nearest: Option<(usize, f32)> = None;
    for i in progress.remaining() {
        let c = definition.checkpoints[i].center;
        let (dx, dz) = (c.x - position.x, c.z - position.z);
        let d2 = dx.mul_add(dx, dz * dz);
        if nearest.is_none_or(|(_, d)| d2 < d) {
            nearest = Some((i, d2));
        }
    }
    match nearest {
        // Every gate cleared: the armed finish is the remaining
        // objective (RACE-7).
        None => definition.finish.as_ref().map(|_| NavTarget::Finish),
        Some((nearest, _)) => {
            let gate = match picked {
                Some(p) if progress.remaining().any(|r| r == p) => p,
                _ => nearest,
            };
            Some(NavTarget::Gate(gate))
        }
    }
}

/// The live running order — best to worst — while a race runs: the
/// place indicator's ordering (HUD-2 names the instrument) computed
/// from authoritative progress state, not from who crossed a line most
/// recently.
///
/// Ordering contract (**designed** — no verified original live-place
/// rule exists, so this is an explicit policy, DSN-13):
///
/// - `Finished` participants lead: a completed race's place is locked.
///   They order among themselves by the recorded `race_ticks`, the same
///   key [`ResultLedger::standings`](crate::ResultLedger::standings)
///   uses, so the live order converges to the standings as everyone
///   resolves.
/// - Then active participants (`Racing`, `AwaitingStart`) by progress:
///   `Ordered` counts `(lap, next)` — a later lap, then a later gate in
///   the lap; `AnyOrder` counts cleared gates. A progress tie breaks
///   toward the participant closer (XZ) to their own current objective
///   — `checkpoints[next]` under `Ordered`, the [`navigation_target`]
///   objective under `AnyOrder` (nearest remaining gate, or the armed
///   finish once every gate is cleared). `AwaitingStart` participants
///   carry zero progress, so a countdown grid orders by proximity to
///   the first objective.
/// - `TimedOut` participants trail: they can no longer improve. Among
///   themselves they order like the standings — recorded `race_ticks`,
///   then `PlayerId` — so a shared-deadline expiry is a pure `PlayerId`
///   tie there too.
/// - Every remaining tie orders by `PlayerId`: deterministic and
///   independent of query order.
///
/// The distance tie-break is straight-line, not course distance — two
/// participants on different routes to the same objective can rank
/// "wrong" for a few steps. It is a presentation aid only: results and
/// progression read the ledger, never this order.
pub fn live_order<'a>(
    definition: &RaceDefinition,
    participants: impl IntoIterator<Item = (PlayerId, &'a RaceProgress, Vec3)>,
) -> Vec<PlayerId> {
    fn tier(state: &ParticipantState) -> u8 {
        match state {
            ParticipantState::Finished { .. } => 0,
            ParticipantState::AwaitingStart | ParticipantState::Racing => 1,
            ParticipantState::TimedOut { .. } => 2,
        }
    }
    fn resolved_ticks(state: &ParticipantState) -> u64 {
        match state {
            ParticipantState::Finished { race_ticks, .. }
            | ParticipantState::TimedOut { race_ticks, .. } => *race_ticks,
            _ => 0,
        }
    }
    let score = |p: &RaceProgress| -> (u32, u32) {
        match definition.rule {
            CheckpointRule::Ordered => (p.lap, p.next as u32),
            CheckpointRule::AnyOrder => (p.cleared_count() as u32, 0),
        }
    };
    let objective_distance = |p: &RaceProgress, pos: Vec3| -> f32 {
        let target = match definition.rule {
            CheckpointRule::Ordered => definition.checkpoints.get(p.next).map(|c| c.center),
            CheckpointRule::AnyOrder => {
                navigation_target(definition, p, None, pos).and_then(|t| t.position(definition))
            }
        };
        target.map_or(0.0, |c| {
            let (dx, dz) = (c.x - pos.x, c.z - pos.z);
            dx.mul_add(dx, dz * dz).sqrt()
        })
    };
    let mut rows: Vec<(PlayerId, &RaceProgress, Vec3)> = participants.into_iter().collect();
    rows.sort_by(|(a_id, a_p, a_pos), (b_id, b_p, b_pos)| {
        tier(&a_p.state)
            .cmp(&tier(&b_p.state))
            .then_with(|| match tier(&a_p.state) {
                1 => score(b_p).cmp(&score(a_p)).then_with(|| {
                    objective_distance(a_p, *a_pos).total_cmp(&objective_distance(b_p, *b_pos))
                }),
                _ => resolved_ticks(&a_p.state).cmp(&resolved_ticks(&b_p.state)),
            })
            .then_with(|| a_id.cmp(b_id))
    });
    rows.into_iter().map(|(id, _, _)| id).collect()
}

/// Cycle the arrow pick through the remaining gates (RACE-6: the
/// documented keys move the arrow through the remaining checkpoints).
/// `dir` `+1`/`-1` steps forward/backward in authored order, wrapping;
/// a stale or cleared pick starts from the current effective target.
/// Returns the new `picked` value — `None` when no gate remains, which
/// also clears the pick.
pub fn cycle_target(
    definition: &RaceDefinition,
    progress: &RaceProgress,
    picked: Option<usize>,
    position: Vec3,
    dir: i32,
) -> Option<usize> {
    let remaining: Vec<usize> = progress.remaining().collect();
    if remaining.is_empty() {
        return None;
    }
    let current = match navigation_target(definition, progress, picked, position) {
        Some(NavTarget::Gate(i)) => remaining.iter().position(|&r| r == i).unwrap_or(0),
        _ => 0,
    };
    let len = remaining.len() as i32;
    let next = (current as i32 + dir).rem_euclid(len) as usize;
    Some(remaining[next])
}

/// Signed ground-plane angle from a heading to `target`: `0` = dead
/// ahead, positive = to the driver's right (screen-clockwise), `±π` =
/// dead behind. `yaw` uses the `Quat::from_rotation_y` convention the
/// vehicle transform uses — forward is `(−sin yaw, −cos yaw)` in XZ.
/// A coincident target reports `0` rather than NaN.
pub fn relative_bearing(yaw: f32, from: Vec3, to: Vec3) -> f32 {
    let dx = to.x - from.x;
    let dz = to.z - from.z;
    if dx.mul_add(dx, dz * dz) < 1e-9 {
        return 0.0;
    }
    let (sin, cos) = yaw.sin_cos();
    // forward = (−sin, −cos), right = (cos, −sin) in XZ — projecting
    // the offset onto them gives the ahead/right components.
    let ahead = -(dx * sin + dz * cos);
    let right = dx * cos - dz * sin;
    right.atan2(ahead)
}
