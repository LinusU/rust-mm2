//! The shared police-roster contract (F20-A.1).
//!
//! One [`PoliceRoster`] is the difficulty-selected `[Police]` lineup an
//! event's (or Cruise's `roam`) `.aimap`/`.aimap_p` record wires: which
//! vehicle stands where. It is the runtime-facing counterpart of the
//! parser record (`mm2_formats::aimap::PoliceRecord`) — a future spawn
//! or pursuit system consumes this, never CSV text.
//!
//! Only what the data shows is interpreted. The first tail value is a
//! heading in degrees (ledger COP-5: finite on all 237 retail rows but
//! spanning −270…535 — an authored `535` is an unnormalised 175° — so
//! it is reduced modulo a turn, the raw value kept in `params`); the
//! remaining columns are kept raw
//! because the file's own header comment ("StartLink, Start Dist, Start
//! Mode, Start Lane, Patrol Route") does not match the observed column
//! counts (UNK-9). Nothing here says what a cop *does* — pursuit rules
//! stay unverified (COP-4). A problem that does not prevent building the
//! roster is a [`PoliceIssue`] on it: reported, never silently repaired.

use std::fmt;

use bevy::prelude::{Component, Resource, Vec3};

use crate::Difficulty;
use crate::nav::{NavGraph, RouteOptions};

/// One authored police spawn — a distilled `[Police]` row.
#[derive(Debug, Clone, PartialEq)]
pub struct PoliceSpec {
    /// Authored vehicle identity — a basename in the vehicle-catalog id
    /// space (`vpcop` on every retail row). Whether it resolves to a
    /// loadable vehicle is the audit's question, not the contract's.
    pub vehicle: String,
    /// Authored spawn position (MM2 world axes, unmirrored).
    pub position: Vec3,
    /// The first tail value as a vehicle-yaw heading in degrees,
    /// reduced to (−180, 180] (see [`normalize_heading`]). `None` when
    /// the row ships no tail at all, or its first value is not finite.
    pub heading_deg: Option<f32>,
    /// Every tail value after the position, raw and including the
    /// heading (five on most retail rows, two on `race/sf/evade0`).
    pub params: Vec<f32>,
    /// 1-based line in the source aimap, for diagnostics.
    pub line: u32,
}

/// Reduce a heading in degrees to (−180, 180]; `None` when not finite.
pub fn normalize_heading(deg: f32) -> Option<f32> {
    if !deg.is_finite() {
        return None;
    }
    let r = deg.rem_euclid(360.0);
    Some(if r > 180.0 { r - 360.0 } else { r })
}

impl PoliceSpec {
    /// Whether a spawner may place the row: finite coordinates, and a
    /// heading that is either absent (no tail) or a finite number.
    pub fn placeable(&self) -> bool {
        self.position.is_finite() && self.params.first().is_none_or(|h| h.is_finite())
    }
}

/// A non-fatal problem found while building a [`PoliceRoster`].
#[derive(Debug, Clone, PartialEq)]
pub enum PoliceIssue {
    /// The requested difficulty's aimap variant is absent; the other
    /// one's roster was used (the only authored lineup that exists).
    MissingVariant {
        /// Difficulty the caller asked for.
        wanted: Difficulty,
        /// Difficulty whose record was read instead.
        used: Difficulty,
    },
    /// The wired roster size disagrees with the table row's authored
    /// `Cops` count.
    CountMismatch {
        /// `[Police]` rows actually wired.
        wired: usize,
        /// The table row's authored count, verbatim.
        table: i64,
    },
    /// A row's position or heading is unusable (see
    /// [`PoliceSpec::placeable`]). The slot is kept so the lineup stays
    /// the authored one; a spawner must refuse it.
    Unplaceable {
        /// 1-based source line.
        line: u32,
    },
}

impl fmt::Display for PoliceIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingVariant { wanted, used } => {
                write!(f, "no {wanted:?} aimap variant — using the {used:?} roster")
            }
            Self::CountMismatch { wired, table } => write!(
                f,
                "aimap wires {wired} police but the table row authors {table}"
            ),
            Self::Unplaceable { line } => {
                write!(
                    f,
                    "police row at line {line} has an unusable position or heading"
                )
            }
        }
    }
}

/// The difficulty-selected police lineup for one event or for Cruise.
#[derive(Debug, Clone, Default)]
pub struct PoliceRoster {
    /// One entry per authored `[Police]` row, in file order.
    pub entries: Vec<PoliceSpec>,
    /// `[CopChaseDistance]` when the aimap authors it (retail:
    /// `race/sf/crash5.aimap{,_p}` only). Raw; what the distance means
    /// is unverified.
    pub chase_distance: Option<f32>,
    /// Non-fatal problems found while building (row order).
    pub issues: Vec<PoliceIssue>,
}

impl PoliceRoster {
    /// Entries a spawner may place.
    pub fn placeable(&self) -> impl Iterator<Item = &PoliceSpec> {
        self.entries.iter().filter(|e| e.placeable())
    }

    /// Distinct authored vehicle ids, sorted.
    pub fn vehicles(&self) -> Vec<String> {
        let mut v: Vec<String> = self.entries.iter().map(|e| e.vehicle.clone()).collect();
        v.sort();
        v.dedup();
        v
    }
}

/// The designed pursuit policy (F20-A.3). Every number here is an
/// **enhanced policy**, not an original rule: nothing in the shipped
/// docs or data states how the retail cops sense, escalate or give up
/// (ledger COP-4 / UNK-9). What the help does say (COP-1/COP-2) is the
/// shape — chase on sight, escape by leaving the cop's sight — and the
/// machine honours exactly that shape with bounded, disclosed numbers.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct PursuitPolicy {
    /// Longest range (m) at which a still-idle cop notices the target.
    pub detect_range: f32,
    /// Longest range (m) at which an engaged cop keeps the target in
    /// contact — wider than `detect_range` so the edge does not flicker.
    pub contact_range: f32,
    /// Continuous sighting (s) before an idle cop commits — a glance
    /// across a junction does not start a chase.
    pub reaction: f32,
    /// Seconds without contact before the cop gives up (the target
    /// "left its sight"). Until then it drives to the last place it
    /// saw the target — it is never told where the target is.
    pub lose_after: f32,
    /// Seconds a cop that lost its target stands down before it can
    /// notice anyone again.
    pub cooldown: f32,
    /// Most cops that may be in [`PursuitPhase::Pursuing`] at once; a
    /// further cop that has finished reacting waits (still engaged,
    /// watching) until a slot frees.
    pub max_pursuers: usize,
}

impl Default for PursuitPolicy {
    fn default() -> Self {
        Self {
            detect_range: 90.0,
            contact_range: 140.0,
            reaction: 0.75,
            lose_after: 8.0,
            cooldown: 6.0,
            max_pursuers: 4,
        }
    }
}

/// Where one cop is in the detect → engage → pursue → lost cycle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PursuitPhase {
    /// Standing at its post, watching.
    Idle,
    /// Has the target in sight and is reacting (or waiting for a free
    /// pursuer slot); the field is how long it has held the sighting.
    Engaged(f32),
    /// Chasing; the field is how long contact has been lost (0 while
    /// the target is in view).
    Pursuing(f32),
    /// Gave up; the field is the stand-down time left.
    Lost(f32),
}

/// What the world shows one cop this tick: the nearest eligible target
/// it could be looking at, or `None` when there is no eligible target
/// (nobody racing, a finished race, a networked session).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sighting {
    /// The target's position.
    pub position: Vec3,
    /// Straight-line distance (m) from the cop.
    pub distance: f32,
    /// Whether the line between them is clear of the static world.
    pub clear: bool,
}

/// A phase change worth reporting (smoke counters, siren hooks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PursuitEvent {
    /// Idle → Engaged: the target came into view.
    Noticed,
    /// Engaged → Idle: the sighting ended before the cop committed.
    Dismissed,
    /// Engaged → Pursuing.
    Committed,
    /// Pursuing → Lost.
    GaveUp,
    /// Lost → Idle: the stand-down ended.
    Rearmed,
    /// Any → Idle by [`Pursuit::stand_down`] (no eligible target any
    /// more, a restart, a finished race).
    Stood,
}

/// One cop's pursuit state: pure, deterministic, no clock of its own.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Pursuit {
    /// Current phase.
    pub phase: PursuitPhase,
    /// Where the target was last in contact — what a pursuing cop
    /// drives to. `None` until the first contact.
    pub last_seen: Option<Vec3>,
}

impl Default for Pursuit {
    fn default() -> Self {
        Self::new()
    }
}

impl Pursuit {
    /// A cop at its post, having seen nothing.
    pub const fn new() -> Self {
        Self {
            phase: PursuitPhase::Idle,
            last_seen: None,
        }
    }

    /// Whether the cop is chasing right now.
    pub fn is_pursuing(&self) -> bool {
        matches!(self.phase, PursuitPhase::Pursuing(_))
    }

    /// Whether the cop currently counts as having the target in
    /// contact: within range (the wider `contact_range` once engaged,
    /// `detect_range` while idle) with a clear line.
    fn in_contact(&self, policy: &PursuitPolicy, s: &Sighting) -> bool {
        let range = match self.phase {
            PursuitPhase::Idle | PursuitPhase::Lost(_) => policy.detect_range,
            _ => policy.contact_range,
        };
        s.clear && s.distance.is_finite() && s.distance <= range
    }

    /// Advance by `dt` seconds. `slot_free` says whether the fleet has
    /// room for one more pursuer (the caller counts the others).
    /// Returns the phase change, if any; at most one per call.
    pub fn step(
        &mut self,
        dt: f32,
        sighting: Option<&Sighting>,
        slot_free: bool,
        policy: &PursuitPolicy,
    ) -> Option<PursuitEvent> {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let contact = sighting.filter(|s| self.in_contact(policy, s));
        if let Some(s) = contact {
            self.last_seen = Some(s.position);
        }
        match self.phase {
            PursuitPhase::Idle => {
                contact?;
                self.phase = PursuitPhase::Engaged(0.0);
                Some(PursuitEvent::Noticed)
            }
            PursuitPhase::Engaged(held) => {
                if contact.is_none() {
                    self.phase = PursuitPhase::Idle;
                    return Some(PursuitEvent::Dismissed);
                }
                let held = held + dt;
                if held >= policy.reaction && slot_free {
                    self.phase = PursuitPhase::Pursuing(0.0);
                    return Some(PursuitEvent::Committed);
                }
                self.phase = PursuitPhase::Engaged(held);
                None
            }
            PursuitPhase::Pursuing(unseen) => {
                if contact.is_some() {
                    self.phase = PursuitPhase::Pursuing(0.0);
                    return None;
                }
                let unseen = unseen + dt;
                if unseen >= policy.lose_after {
                    self.phase = PursuitPhase::Lost(policy.cooldown);
                    return Some(PursuitEvent::GaveUp);
                }
                self.phase = PursuitPhase::Pursuing(unseen);
                None
            }
            PursuitPhase::Lost(left) => {
                let left = left - dt;
                if left <= 0.0 {
                    self.phase = PursuitPhase::Idle;
                    return Some(PursuitEvent::Rearmed);
                }
                self.phase = PursuitPhase::Lost(left);
                None
            }
        }
    }

    /// Return to the post: there is nothing eligible to chase. A cop
    /// already idle reports nothing.
    pub fn stand_down(&mut self) -> Option<PursuitEvent> {
        let was_idle = self.phase == PursuitPhase::Idle;
        *self = Self::new();
        (!was_idle).then_some(PursuitEvent::Stood)
    }
}

/// How long (s) one side of a flashing light bar stays lit before the
/// other takes over — a **designed** cadence (two flashes per second
/// per side). The retail rate is unrecovered (ledger COP-10); nothing
/// in the data states one.
pub const LIGHT_BAR_HALF_PERIOD: f32 = 0.25;

/// A cop car's emergency signals are on: its light bar flashes and its
/// siren program plays (F20-B.1). Present on a car exactly while it is
/// [`Pursuit::is_pursuing`] — the designed trigger (COP-10), since the
/// original's is unknown (UNK-9/UNK-25). The component is the single
/// request both consumers read: the audio side plays the opponent siren
/// program, the visual side lights the `SRNn` flares.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct EmergencyLights {
    /// Seconds the signals have been on — the flash clock. It advances
    /// only while the owner's clock does, so a pause holds the bar.
    pub elapsed: f32,
}

impl EmergencyLights {
    /// Advance the flash clock by `dt` seconds (non-finite or negative
    /// steps are ignored, never poisoning the clock).
    pub fn advance(&mut self, dt: f32) {
        if dt.is_finite() && dt > 0.0 {
            self.elapsed += dt;
        }
    }

    /// Which half of the bar is lit: `0` or `1`, alternating every
    /// [`LIGHT_BAR_HALF_PERIOD`] and starting on `0`.
    pub fn lit_side(&self) -> usize {
        ((self.elapsed / LIGHT_BAR_HALF_PERIOD).floor() as u64 % 2) as usize
    }
}

// ---------- road-aware chase (F20-B.2) ----------

/// Metres ahead of the car along the road line a chasing cop aims — a
/// **designed** lookahead (enhanced policy, ledger COP-11): far enough
/// to round a junction smoothly, near enough not to cut a corner.
pub const CHASE_LOOKAHEAD: f32 = 20.0;
/// How far (m, on the ground plane) the chase goal may move from the one
/// a route was planned for before the cop re-plans — a target that keeps
/// driving does not re-run the router every frame.
pub const CHASE_GOAL_DRIFT: f32 = 25.0;
/// How far (m) the cop may be from the road line it follows before the
/// line is judged useless (a spin-out, a recovery, a crash) and re-planned.
pub const CHASE_OFF_ROUTE: f32 = 30.0;
/// Shortest wait (s) between two router queries by one cop; a failed
/// query waits [`CHASE_FAILED_WAIT`] instead.
pub const CHASE_REPLAN_WAIT: f32 = 1.5;
/// Wait (s) after a query that found no road line before asking again —
/// the cop has moved by then, so its endpoints may connect.
pub const CHASE_FAILED_WAIT: f32 = 4.0;
/// Within this ground distance (m) of a target it can see, a cop drives
/// straight at it: the road line only matters while the way is not
/// obvious.
pub const CHASE_DIRECT_RANGE: f32 = 30.0;
/// Arc-length step (m) between the samples of a planned road line.
pub const CHASE_STEP: f32 = 6.0;
/// Segments ahead of its current one the follower searches for the
/// nearest point; the cursor only moves forward, so a hairpin whose far
/// leg runs close to the near one cannot make the cop jump ahead.
const FOLLOW_WINDOW: usize = 12;

/// A road line a cop follows to a chase goal: a polyline over the nav
/// graph's lanes plus a forward-only cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct ChaseRoute {
    line: Vec<Vec3>,
    seg: usize,
}

fn ground_distance(a: Vec3, b: Vec3) -> f32 {
    (a.x - b.x).hypot(a.z - b.z)
}

impl ChaseRoute {
    /// A route over `line` (at least two finite points); `None`
    /// otherwise.
    pub fn new(line: Vec<Vec3>) -> Option<Self> {
        (line.len() >= 2 && line.iter().all(|p| p.is_finite())).then_some(Self { line, seg: 0 })
    }

    /// The point the route ends at — the goal it was planned for.
    pub fn goal(&self) -> Vec3 {
        *self.line.last().expect("a route has two points")
    }

    /// Nearest point of segment `i` to `pos` on the ground plane: the
    /// point and its ground distance.
    fn project(&self, i: usize, pos: Vec3) -> (Vec3, f32) {
        let (a, b) = (self.line[i], self.line[i + 1]);
        let (dx, dz) = (b.x - a.x, b.z - a.z);
        let len2 = dx.mul_add(dx, dz * dz);
        let t = if len2 > 1e-9 {
            (((pos.x - a.x) * dx + (pos.z - a.z) * dz) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let p = a.lerp(b, t);
        (p, ground_distance(p, pos))
    }

    /// Ground distance from `pos` to the part of the line still ahead
    /// of the cursor.
    pub fn lateral(&self, pos: Vec3) -> f32 {
        (self.seg..self.line.len() - 1)
            .take(FOLLOW_WINDOW)
            .map(|i| self.project(i, pos).1)
            .fold(f32::INFINITY, f32::min)
    }

    /// Advance the cursor to the line's nearest point to `pos` (never
    /// backwards) and return the point `lookahead` metres further along;
    /// the goal once the line runs out.
    pub fn follow(&mut self, pos: Vec3, lookahead: f32) -> Vec3 {
        let last = self.line.len() - 2;
        let mut best = (self.project(self.seg, pos), self.seg);
        for i in self.seg + 1..=last.min(self.seg + FOLLOW_WINDOW) {
            let hit = self.project(i, pos);
            if hit.1 < (best.0).1 {
                best = (hit, i);
            }
        }
        let ((mut at, _), seg) = best;
        self.seg = seg;
        let mut left = lookahead.max(0.0);
        for i in seg..=last {
            let end = self.line[i + 1];
            let d = ground_distance(at, end);
            if d >= left && d > 1e-6 {
                return at.lerp(end, left / d);
            }
            left -= d;
            at = end;
        }
        self.goal()
    }
}

/// Which law last chose a chasing cop's aim point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChaseMode {
    /// Nothing aimed yet.
    #[default]
    Idle,
    /// Straight at the goal: it is in view and close.
    Direct,
    /// Along a planned road line.
    Road,
    /// Straight at the goal because no road line exists (no graph, a
    /// query that failed, or one still waiting to be asked).
    Unrouted,
}

/// What one [`ChaseNav::aim`] call did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChaseAim {
    /// Where to steer.
    pub point: Vec3,
    /// The law that chose it.
    pub mode: ChaseMode,
    /// The router was asked this call and found a line.
    pub planned: bool,
    /// The router was asked this call and found none.
    pub failed: bool,
}

/// A cop's road-aware chase state: the line it follows and the wait
/// before it may ask the router again. Pure and deterministic, with
/// no clock of its own; the route is only ever a query over the shared
/// nav graph, so the cop never learns more than the goal it is given
/// (the last place it saw the target).
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct ChaseNav {
    route: Option<ChaseRoute>,
    wait: f32,
    /// The law that chose the latest aim.
    pub mode: ChaseMode,
}

impl ChaseNav {
    /// The line currently followed, if any.
    pub fn route(&self) -> Option<&ChaseRoute> {
        self.route.as_ref()
    }

    /// Forget the line — the cop stopped chasing, so the next chase
    /// plans afresh. The wait is kept so a cop that stops and restarts
    /// cannot ask the router every frame.
    pub fn clear(&mut self) {
        self.route = None;
        self.mode = ChaseMode::Idle;
    }

    /// The point a cop at `pos` should steer for to reach `goal`.
    /// `in_view` is whether the cop currently sees the target (not just
    /// the last place it saw it). `graph` is the city's routing graph,
    /// `None` for a world with none.
    ///
    /// A target in view and within [`CHASE_DIRECT_RANGE`] is driven at
    /// directly; otherwise the cop follows a road line, planned when it
    /// has none, when the goal has moved [`CHASE_GOAL_DRIFT`] from the
    /// planned one, or when the cop is [`CHASE_OFF_ROUTE`] from the line
    /// — and never more often than the wait allows. No line means a
    /// straight aim at the goal, reported as [`ChaseMode::Unrouted`],
    /// never an invented connection.
    pub fn aim(
        &mut self,
        graph: Option<&NavGraph>,
        dt: f32,
        pos: Vec3,
        goal: Vec3,
        in_view: bool,
    ) -> ChaseAim {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.wait = (self.wait - dt).max(0.0);
        let mut aim = ChaseAim {
            point: goal,
            mode: ChaseMode::Direct,
            planned: false,
            failed: false,
        };
        if in_view && ground_distance(pos, goal) <= CHASE_DIRECT_RANGE {
            self.mode = ChaseMode::Direct;
            return aim;
        }
        let stale = self.route.as_ref().is_none_or(|r| {
            ground_distance(r.goal(), goal) > CHASE_GOAL_DRIFT || r.lateral(pos) > CHASE_OFF_ROUTE
        });
        if stale
            && self.wait <= 0.0
            && let Some(graph) = graph
        {
            match graph.drive_line(pos, goal, &RouteOptions::default(), CHASE_STEP) {
                Ok(line) => {
                    self.route = ChaseRoute::new(line);
                    self.wait = CHASE_REPLAN_WAIT;
                    aim.planned = self.route.is_some();
                    aim.failed = self.route.is_none();
                }
                Err(_) => {
                    // A line for a goal that has since moved on is worse
                    // than none.
                    self.route = None;
                    self.wait = CHASE_FAILED_WAIT;
                    aim.failed = true;
                }
            }
        }
        match self.route.as_mut() {
            Some(route) => {
                aim.point = route.follow(pos, CHASE_LOOKAHEAD);
                aim.mode = ChaseMode::Road;
            }
            None => aim.mode = ChaseMode::Unrouted,
        }
        self.mode = aim.mode;
        aim
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(tail: &[f32], pos: Vec3) -> PoliceSpec {
        PoliceSpec {
            vehicle: "vpcop".into(),
            position: pos,
            heading_deg: tail.first().copied().and_then(normalize_heading),
            params: tail.to_vec(),
            line: 1,
        }
    }

    #[test]
    fn headings_reduce_modulo_a_turn_and_keep_retail_values() {
        // Retail authors −270…535 (e.g. -110, 169, 535); 535 is a 175° yaw.
        assert_eq!(normalize_heading(-110.0), Some(-110.0));
        assert_eq!(normalize_heading(169.0), Some(169.0));
        assert_eq!(normalize_heading(535.0), Some(175.0));
        assert_eq!(normalize_heading(-270.0), Some(90.0));
        assert_eq!(normalize_heading(-180.0), Some(180.0));
        assert_eq!(normalize_heading(360.0), Some(0.0));
        assert_eq!(normalize_heading(f32::NAN), None);
        assert_eq!(normalize_heading(f32::INFINITY), None);
    }

    #[test]
    fn placeable_needs_a_finite_position_and_a_finite_tail_heading() {
        assert!(spec(&[-110.0, 0.0], Vec3::new(1.0, 2.0, 3.0)).placeable());
        assert!(spec(&[535.0], Vec3::ZERO).placeable());
        assert!(spec(&[], Vec3::ZERO).placeable());
        assert!(!spec(&[f32::NAN], Vec3::ZERO).placeable());
        assert!(!spec(&[0.0], Vec3::new(f32::INFINITY, 0.0, 0.0)).placeable());
        assert_eq!(spec(&[], Vec3::ZERO).heading_deg, None);
    }

    #[test]
    fn vehicles_are_distinct_and_sorted() {
        let mut roster = PoliceRoster::default();
        for v in ["vpcop", "vpbus", "vpcop"] {
            let mut s = spec(&[0.0], Vec3::ZERO);
            s.vehicle = v.into();
            roster.entries.push(s);
        }
        assert_eq!(roster.vehicles(), ["vpbus", "vpcop"]);
        assert_eq!(roster.placeable().count(), 3);
    }

    fn see(distance: f32, clear: bool) -> Sighting {
        Sighting {
            position: Vec3::new(distance, 0.0, 0.0),
            distance,
            clear,
        }
    }

    #[test]
    fn a_cop_notices_reacts_and_commits() {
        let policy = PursuitPolicy::default();
        let mut p = Pursuit::new();
        assert_eq!(
            p.step(0.1, Some(&see(50.0, true)), true, &policy),
            Some(PursuitEvent::Noticed)
        );
        // Still reacting.
        assert_eq!(p.step(0.5, Some(&see(50.0, true)), true, &policy), None);
        assert!(!p.is_pursuing());
        assert_eq!(
            p.step(0.5, Some(&see(50.0, true)), true, &policy),
            Some(PursuitEvent::Committed)
        );
        assert!(p.is_pursuing());
        assert_eq!(p.last_seen, Some(Vec3::new(50.0, 0.0, 0.0)));
    }

    #[test]
    fn the_control_scenario_never_triggers() {
        let policy = PursuitPolicy::default();
        // Out of range, behind a wall, and with no target at all.
        for s in [
            Some(see(policy.detect_range + 1.0, true)),
            Some(see(20.0, false)),
            None,
        ] {
            let mut p = Pursuit::new();
            for _ in 0..600 {
                assert_eq!(p.step(0.1, s.as_ref(), true, &policy), None);
            }
            assert_eq!(p, Pursuit::new());
        }
    }

    #[test]
    fn a_glance_is_dismissed_before_the_cop_commits() {
        let policy = PursuitPolicy::default();
        let mut p = Pursuit::new();
        p.step(0.1, Some(&see(30.0, true)), true, &policy);
        assert_eq!(
            p.step(0.1, Some(&see(30.0, false)), true, &policy),
            Some(PursuitEvent::Dismissed)
        );
        assert_eq!(p.phase, PursuitPhase::Idle);
    }

    #[test]
    fn contact_hysteresis_keeps_a_chase_past_the_detect_range() {
        let policy = PursuitPolicy::default();
        let mut p = Pursuit::new();
        p.step(0.0, Some(&see(50.0, true)), true, &policy);
        p.step(1.0, Some(&see(50.0, true)), true, &policy);
        assert!(p.is_pursuing());
        let far = see(policy.detect_range + 20.0, true);
        for _ in 0..100 {
            p.step(0.1, Some(&far), true, &policy);
        }
        assert_eq!(
            p.phase,
            PursuitPhase::Pursuing(0.0),
            "wider range holds contact"
        );
    }

    #[test]
    fn losing_sight_gives_up_after_the_bound_then_rearms() {
        let policy = PursuitPolicy::default();
        let mut p = Pursuit::new();
        p.step(0.0, Some(&see(40.0, true)), true, &policy);
        p.step(1.0, Some(&see(40.0, true)), true, &policy);
        let mut events = Vec::new();
        let mut t = 0.0;
        while t < policy.lose_after + 1.0 {
            if let Some(e) = p.step(0.5, None, true, &policy) {
                events.push((t, e));
            }
            t += 0.5;
        }
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].1, PursuitEvent::GaveUp);
        assert!(events[0].0 <= policy.lose_after, "bounded: {events:?}");
        // The last place it saw the target is kept for the chase's tail.
        assert_eq!(p.last_seen, Some(Vec3::new(40.0, 0.0, 0.0)));
        // Stood down: it does not re-notice during the cooldown…
        assert_eq!(p.step(0.5, Some(&see(10.0, true)), true, &policy), None);
        assert!(matches!(p.phase, PursuitPhase::Lost(_)));
        // …and is armed again afterwards.
        assert_eq!(
            p.step(policy.cooldown, None, true, &policy),
            Some(PursuitEvent::Rearmed)
        );
        assert_eq!(
            p.step(0.1, Some(&see(10.0, true)), true, &policy),
            Some(PursuitEvent::Noticed)
        );
    }

    #[test]
    fn a_brief_break_in_sight_does_not_end_the_chase() {
        let policy = PursuitPolicy::default();
        let mut p = Pursuit::new();
        p.step(0.0, Some(&see(40.0, true)), true, &policy);
        p.step(1.0, Some(&see(40.0, true)), true, &policy);
        p.step(policy.lose_after - 0.5, None, true, &policy);
        assert!(p.is_pursuing());
        p.step(0.1, Some(&see(40.0, true)), true, &policy);
        assert_eq!(
            p.phase,
            PursuitPhase::Pursuing(0.0),
            "contact resets the clock"
        );
    }

    #[test]
    fn a_full_fleet_keeps_the_next_cop_watching() {
        let policy = PursuitPolicy::default();
        let mut p = Pursuit::new();
        p.step(0.0, Some(&see(40.0, true)), false, &policy);
        for _ in 0..50 {
            assert_eq!(p.step(0.1, Some(&see(40.0, true)), false, &policy), None);
        }
        assert!(matches!(p.phase, PursuitPhase::Engaged(_)));
        assert_eq!(
            p.step(0.1, Some(&see(40.0, true)), true, &policy),
            Some(PursuitEvent::Committed)
        );
    }

    #[test]
    fn standing_down_resets_and_reports_once() {
        let policy = PursuitPolicy::default();
        let mut p = Pursuit::new();
        assert_eq!(p.stand_down(), None);
        p.step(0.0, Some(&see(40.0, true)), true, &policy);
        p.step(1.0, Some(&see(40.0, true)), true, &policy);
        assert_eq!(p.stand_down(), Some(PursuitEvent::Stood));
        assert_eq!(p, Pursuit::new());
        assert_eq!(p.stand_down(), None);
    }

    #[test]
    fn the_light_bar_alternates_sides_on_its_half_period() {
        let mut lights = EmergencyLights::default();
        assert_eq!(lights.lit_side(), 0, "a fresh bar starts on side 0");
        lights.advance(LIGHT_BAR_HALF_PERIOD * 0.99);
        assert_eq!(lights.lit_side(), 0);
        lights.advance(LIGHT_BAR_HALF_PERIOD * 0.02);
        assert_eq!(lights.lit_side(), 1, "the half period flips it");
        lights.advance(LIGHT_BAR_HALF_PERIOD);
        assert_eq!(lights.lit_side(), 0, "and flips back");
    }

    #[test]
    fn the_flash_clock_ignores_bad_steps() {
        let mut lights = EmergencyLights::default();
        lights.advance(f32::NAN);
        lights.advance(f32::INFINITY);
        lights.advance(-1.0);
        assert_eq!(lights, EmergencyLights::default());
        lights.advance(0.1);
        assert_eq!(lights.elapsed, 0.1);
    }

    #[test]
    fn nonfinite_inputs_neither_trigger_nor_panic() {
        let policy = PursuitPolicy::default();
        let mut p = Pursuit::new();
        let bad = Sighting {
            position: Vec3::NAN,
            distance: f32::NAN,
            clear: true,
        };
        assert_eq!(p.step(f32::NAN, Some(&bad), true, &policy), None);
        assert_eq!(p, Pursuit::new());
    }

    fn l_line() -> ChaseRoute {
        // East 40 m along z=0, then north (−z) 40 m: an L.
        ChaseRoute::new(vec![
            Vec3::ZERO,
            Vec3::new(20.0, 0.0, 0.0),
            Vec3::new(40.0, 0.0, 0.0),
            Vec3::new(40.0, 0.0, -20.0),
            Vec3::new(40.0, 0.0, -40.0),
        ])
        .unwrap()
    }

    #[test]
    fn a_route_needs_two_finite_points() {
        assert!(ChaseRoute::new(vec![Vec3::ZERO]).is_none());
        assert!(ChaseRoute::new(vec![Vec3::ZERO, Vec3::NAN]).is_none());
        assert!(ChaseRoute::new(vec![Vec3::ZERO, Vec3::X]).is_some());
        assert_eq!(l_line().goal(), Vec3::new(40.0, 0.0, -40.0));
    }

    #[test]
    fn following_aims_down_the_line_not_at_the_goal() {
        let mut r = l_line();
        // From the start the aim is 20 m along the first leg, though the
        // goal is diagonal across the block.
        let a = r.follow(Vec3::ZERO, 20.0);
        assert!((a - Vec3::new(20.0, 0.0, 0.0)).length() < 1e-3, "{a}");
        // Near the corner the lookahead bends around it.
        let a = r.follow(Vec3::new(35.0, 0.0, 0.0), 20.0);
        assert!((a - Vec3::new(40.0, 0.0, -15.0)).length() < 1e-3, "{a}");
        // Past the end it is the goal.
        let a = r.follow(Vec3::new(40.0, 0.0, -38.0), 20.0);
        assert_eq!(a, r.goal());
    }

    #[test]
    fn the_cursor_never_moves_backwards() {
        let mut r = l_line();
        r.follow(Vec3::new(40.0, 0.0, -30.0), 5.0);
        // The car is flung back near the start: the cursor stays on the
        // late leg instead of re-aiming along the first one.
        let a = r.follow(Vec3::new(2.0, 0.0, 0.0), 5.0);
        assert!(a.z < -20.0, "{a}");
        assert!(r.lateral(Vec3::new(2.0, 0.0, 0.0)) > 30.0);
    }

    #[test]
    fn the_lookahead_is_clamped_and_a_zero_one_aims_at_the_nearest_point() {
        let mut r = l_line();
        let a = r.follow(Vec3::new(5.0, 3.0, 0.0), -4.0);
        assert!((a - Vec3::new(5.0, 0.0, 0.0)).length() < 1e-3, "{a}");
    }

    #[test]
    fn without_a_graph_the_cop_aims_straight_and_says_so() {
        let mut nav = ChaseNav::default();
        let goal = Vec3::new(100.0, 0.0, 100.0);
        let a = nav.aim(None, 0.016, Vec3::ZERO, goal, false);
        assert_eq!((a.point, a.mode), (goal, ChaseMode::Unrouted));
        assert!(!a.planned && !a.failed);
        assert!(nav.route().is_none());
    }

    #[test]
    fn a_close_target_in_view_is_driven_at_directly() {
        let mut nav = ChaseNav::default();
        let goal = Vec3::new(CHASE_DIRECT_RANGE - 1.0, 0.0, 0.0);
        let a = nav.aim(None, 0.016, Vec3::ZERO, goal, true);
        assert_eq!((a.point, a.mode), (goal, ChaseMode::Direct));
        // Out of view, the same distance is not "direct".
        let a = nav.aim(None, 0.016, Vec3::ZERO, goal, false);
        assert_eq!(a.mode, ChaseMode::Unrouted);
        // And in view but far is not either.
        let a = nav.aim(None, 0.016, Vec3::ZERO, goal * 3.0, true);
        assert_eq!(a.mode, ChaseMode::Unrouted);
    }

    #[test]
    fn garbage_steps_do_not_poison_the_wait() {
        let mut nav = ChaseNav {
            wait: 1.0,
            ..ChaseNav::default()
        };
        nav.aim(None, f32::NAN, Vec3::ZERO, Vec3::X * 90.0, false);
        nav.aim(None, -5.0, Vec3::ZERO, Vec3::X * 90.0, false);
        assert_eq!(nav.wait, 1.0);
        nav.clear();
        assert_eq!(nav.mode, ChaseMode::Idle);
    }
}
