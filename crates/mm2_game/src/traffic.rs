//! The ambient-traffic contract: the typed roster a city aimap wires
//! plus the deterministic seeded spawn-policy planner (F10-A.1).
//!
//! An [`AmbientRoster`] is the runtime-facing counterpart of a city's
//! `[Ambient Types/Density]` rows (`mm2_formats::aimap`): one
//! [`AmbientSpec`] per row, authored cumulative weights kept verbatim —
//! they are non-decreasing and close at `1.0` on every retail file, and
//! an id may legitimately appear in more than one weight band (london
//! authors `va_compact_s` twice). Each spec optionally carries the
//! decoded `aiVehicleData` tuning — `None` when the row's
//! `tune/vehicle/<id>.aivehicledata` did not resolve, which the planner
//! treats as unspawnable: the authored weight band is preserved, never
//! silently rebalanced onto another class.
//!
//! [`plan_ambient`] draws an initial spawn set: the authored density
//! fraction times a bounded vehicle budget, each draw picking a class
//! through the cumulative table and a lane-position inside the spawn
//! annulus (`min_player_distance`…`recycle_distance` of the bubble
//! centre) over the routable vehicle lanes that survive the overrides
//! (closed roads and pedestrian-only/disabled road sides are already
//! excluded from the graph's arcs). Everything is seeded through
//! [`NavRng`] — the same `(seed, content)` pair produces the same
//! plan on every platform.
//!
//! This is *not* original traffic: the plan is spawn/despawn policy
//! data for the ambient system F10-B/C fills in — no intersection
//! yielding, signals or collisions are implied here. The original's
//! ambient population bound and bubble distances are unverified, so
//! [`SpawnPolicy`]'s defaults are designed values, not recovered ones.

use std::collections::BTreeSet;
use std::fmt;

use mm2_formats::veh::AiVehicleData;

use crate::nav::{LaneId, LaneSample, NavGraph, NavOverrides, NavRng};

/// One authored ambient class — an `[Ambient Types/Density]` row plus
/// its decoded tuning.
#[derive(Debug, Clone)]
pub struct AmbientSpec {
    /// The authored vehicle id — the `tune/vehicle/<id>.aivehicledata`
    /// and `geometry/<id>.pkg` basename (`va_*` on retail).
    pub id: String,
    /// Cumulative selection weight: non-decreasing down the roster and
    /// closing at `1.0` on retail. A pick is the first row whose weight
    /// exceeds the draw.
    pub cumulative_weight: f32,
    /// Raw trailing flag column — `0` on every retail row, absent on
    /// one `roambak` row; semantics unverified, kept verbatim.
    pub flag: i64,
    /// Decoded `aiVehicleData` tuning. `None` when the record did not
    /// resolve — the row stays in the weight table (the authored
    /// denominator is preserved) but a draw landing on it spawns
    /// nothing.
    pub tuning: Option<AiVehicleData>,
}

/// A problem the roster or plan reports rather than repairs.
#[derive(Debug, Clone, PartialEq)]
pub enum TrafficIssue {
    /// A weight is lower than the previous row's — cumulative tables
    /// are non-decreasing on retail.
    WeightOrder {
        /// Roster index of the offending row.
        index: usize,
        /// This row's cumulative weight.
        weight: f32,
        /// The previous row's cumulative weight.
        previous: f32,
    },
    /// The last cumulative weight is below `1.0` — draws above it
    /// select nothing (unverified whether the original tolerates this;
    /// retail always closes at `1.0`).
    WeightsNotClosed {
        /// The last authored cumulative weight.
        last: f32,
    },
    /// A roster row's tuning did not resolve. Reported once per id at
    /// plan time — the row stays in the weight table.
    UnspawnableClass {
        /// The authored vehicle id.
        id: String,
    },
    /// The roster carries no rows — planning produces no traffic.
    EmptyRoster,
    /// No routable vehicle lane survived the overrides — planning
    /// produces no traffic.
    NoEligibleLanes,
}

impl fmt::Display for TrafficIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WeightOrder {
                index,
                weight,
                previous,
            } => write!(
                f,
                "ambient row {index}: weight {weight} below previous {previous}"
            ),
            Self::WeightsNotClosed { last } => {
                write!(f, "ambient weights close at {last}, not 1.0")
            }
            Self::UnspawnableClass { id } => {
                write!(f, "ambient class {id} has no resolved tuning")
            }
            Self::EmptyRoster => write!(f, "ambient roster is empty"),
            Self::NoEligibleLanes => write!(f, "no eligible ambient vehicle lanes"),
        }
    }
}

/// A city's ambient-vehicle pick table.
#[derive(Debug, Clone, Default)]
pub struct AmbientRoster {
    /// One entry per authored row, in file order.
    pub entries: Vec<AmbientSpec>,
    /// Structural problems found while validating the table.
    pub issues: Vec<TrafficIssue>,
}

impl AmbientRoster {
    /// Validate the authored table — weight monotonicity and closure.
    /// Duplicate ids are *not* an issue: a class may legitimately
    /// occupy two weight bands.
    pub fn new(entries: Vec<AmbientSpec>) -> Self {
        let mut issues = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            if i > 0 && e.cumulative_weight < entries[i - 1].cumulative_weight {
                issues.push(TrafficIssue::WeightOrder {
                    index: i,
                    weight: e.cumulative_weight,
                    previous: entries[i - 1].cumulative_weight,
                });
            }
        }
        if let Some(last) = entries.last()
            && last.cumulative_weight < 1.0 - f32::EPSILON
        {
            issues.push(TrafficIssue::WeightsNotClosed {
                last: last.cumulative_weight,
            });
        }
        Self { entries, issues }
    }

    /// Select the class for a cumulative draw `u` in `[0, 1)`: the
    /// first row whose weight exceeds `u`. `None` on an empty roster
    /// or a table that does not close at `1.0`.
    pub fn select(&self, u: f32) -> Option<usize> {
        self.entries.iter().position(|e| u < e.cumulative_weight)
    }

    /// Seeded pick — [`Self::select`] driven by `rng`.
    pub fn pick(&self, rng: &mut NavRng) -> Option<usize> {
        self.select(rng.next_f32())
    }

    /// Whether `index` can spawn — resolved tuning present.
    pub fn spawnable(&self, index: usize) -> bool {
        self.entries.get(index).is_some_and(|e| e.tuning.is_some())
    }
}

/// Spawn/despawn policy constants the ambient system runs under.
/// All values are designed defaults: the original's ambient pool size
/// and bubble distances are unverified, so these are implementation
/// choices, not recovered rules.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnPolicy {
    /// Bound on simultaneously active ambient vehicles — the density
    /// fraction scales the target inside this cap.
    pub max_active: usize,
    /// No spawn is placed within this distance of the player, in
    /// metres — ambient cars materialising inside the view cone is the
    /// failure this guards.
    pub min_player_distance: f32,
    /// A vehicle beyond this distance from the player is recycled into
    /// the spawn pool (the ambient bubble radius), in metres.
    pub recycle_distance: f32,
    /// Bound on placement attempts per directive before the draw is
    /// dropped — keeps the planner finite when the player sits in the
    /// only eligible pocket.
    pub placement_attempts: usize,
}

impl Default for SpawnPolicy {
    fn default() -> Self {
        Self {
            max_active: 32,
            min_player_distance: 60.0,
            recycle_distance: 400.0,
            placement_attempts: 8,
        }
    }
}

/// One planned ambient spawn.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnDirective {
    /// Roster index of the class drawn.
    pub class: usize,
    /// Lane the vehicle spawns on.
    pub lane: LaneId,
    /// Distance along the lane's travel direction.
    pub along: f32,
    /// Spawned pose — sampled position and travel-direction tangent.
    pub sample: LaneSample,
    /// The road's effective speed (`NavOverrides::effective_speed` —
    /// authored BAI/aimap units, unverified as m/s).
    pub target_speed: f32,
}

/// The seeded spawn plan for one session: the target population, the
/// initial placement set and the policy the recycler runs under.
#[derive(Debug, Clone)]
pub struct AmbientPlan {
    /// Seed the draw ran under.
    pub seed: u64,
    /// Density fraction applied (0–1 authored, clamped defensively).
    pub density: f32,
    /// Target simultaneous population — `density × policy.max_active`,
    /// rounded. The plan never exceeds `policy.max_active`.
    pub target: usize,
    /// Routable vehicle lanes surviving the overrides.
    pub eligible_lanes: usize,
    /// Initial spawn set — at most `target` entries.
    pub spawns: Vec<SpawnDirective>,
    /// Directives dropped because every placement attempt landed inside
    /// `policy.min_player_distance` of the player.
    pub dropped: usize,
    /// Draws that selected a class with no resolved tuning or fell off
    /// a non-closed weight table — the authored weight band stood, so
    /// the slot spawns nothing rather than silently rebalancing.
    pub unspawnable: usize,
    /// The policy the plan was built under.
    pub policy: SpawnPolicy,
    /// Non-fatal problems found while planning.
    pub issues: Vec<TrafficIssue>,
}

/// Draw a seeded ambient spawn plan over `graph` under `overrides`.
///
/// Eligible lanes are the graph's routable vehicle lanes on roads the
/// overrides leave open — `NavGraph::build` already withholds arcs from
/// pedestrian-only/disabled road sides, so `arc.is_some()` is the BAI
/// ambient-classification test. Each of `target` draws picks a lane
/// uniformly, a position along it, and a class through the roster's
/// cumulative weights; placements outside the `[min_player_distance,
/// recycle_distance]` annulus around `player_at` retry up to
/// `policy.placement_attempts` times before the directive is dropped —
/// the outer bound keeps the plan from populating road the recycler
/// would collect on its first tick. `player_at` may be a spawn pose,
/// not a tracked position — the planner only needs the bubble centre.
pub fn plan_ambient(
    graph: &NavGraph,
    overrides: &NavOverrides,
    roster: &AmbientRoster,
    seed: u64,
    density: f32,
    player_at: [f32; 3],
    policy: &SpawnPolicy,
) -> AmbientPlan {
    let mut issues = Vec::new();
    let density = if density.is_finite() {
        density.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let target = (density * policy.max_active as f32).round() as usize;

    let eligible = eligible_lanes(graph, overrides);

    if roster.entries.is_empty() {
        issues.push(TrafficIssue::EmptyRoster);
    }
    if eligible.is_empty() {
        issues.push(TrafficIssue::NoEligibleLanes);
    }

    let mut rng = NavRng::new(seed);
    let mut spawns = Vec::with_capacity(target.min(eligible.len().max(1) * 4));
    let mut dropped = 0usize;
    let mut unspawnable = 0usize;
    let mut flagged: BTreeSet<&str> = BTreeSet::new();

    'draws: for _ in 0..target {
        if eligible.is_empty() {
            break;
        }
        for _ in 0..policy.placement_attempts {
            match draw_spawn(
                graph, overrides, &eligible, roster, &mut rng, player_at, policy,
            ) {
                SpawnDraw::OutOfBand => continue,
                SpawnDraw::Placed(directive) => spawns.push(directive),
                SpawnDraw::Unspawnable(class) => {
                    unspawnable += 1;
                    if let Some(class) = class {
                        let id = roster.entries[class].id.as_str();
                        if flagged.insert(id) {
                            issues.push(TrafficIssue::UnspawnableClass { id: id.to_string() });
                        }
                    }
                }
            }
            continue 'draws;
        }
        dropped += 1;
    }

    AmbientPlan {
        seed,
        density,
        target,
        eligible_lanes: eligible.len(),
        spawns,
        dropped,
        unspawnable,
        policy: policy.clone(),
        issues,
    }
}

/// Lanes ambient traffic may spawn on: routed through an arc
/// (`arc.is_some()` is the routable-vehicle test — `NavGraph::build`
/// withholds arcs from pedestrian-only/disabled road sides), finite
/// geometry, and not on a road the event overrides close. Dead ends
/// stay eligible — a car that runs out of road despawns at runtime
/// rather than being pre-filtered here.
///
/// The finiteness guard is belt-and-braces — `NavGraph::build` already
/// drops non-finite curves with `NavIssue::NonFiniteLane`, and a NaN
/// length here would panic `sample_storage`'s clamp while an inf
/// length would spawn at NaN positions past the bubble check.
pub fn eligible_lanes(graph: &NavGraph, overrides: &NavOverrides) -> Vec<LaneId> {
    graph
        .lanes()
        .iter()
        .filter(|l| {
            l.arc.is_some()
                && l.length.is_finite()
                && l.vertices().iter().all(|p| p.iter().all(|c| c.is_finite()))
                && !overrides.is_closed(l.id.road)
        })
        .map(|l| l.id)
        .collect()
}

/// The result of one spawn-placement attempt.
#[derive(Debug)]
pub enum SpawnDraw {
    /// A directive was placed.
    Placed(SpawnDirective),
    /// The sampled position fell outside the spawn annulus — inside
    /// `min_player_distance` of the player (a car must not materialise
    /// next to them) or beyond `max_player_distance` (a placement past
    /// the recycler's own radius would be despawned on the next tick,
    /// so drawing it is pure churn). The caller retries on a fresh
    /// lane sample.
    OutOfBand,
    /// The class draw selected an unspawnable row (`Some`) or fell off
    /// a non-closed weight table (`None`) — this slot produces nothing;
    /// the authored band is never rebalanced.
    Unspawnable(Option<usize>),
}

/// One spawn-placement attempt — [`plan_ambient`] retries it
/// `policy.placement_attempts` times per directive and the runtime
/// recycler reuses it to top the population back up. Placements are
/// drawn inside the `[min_player_distance, recycle_distance]` annulus
/// `policy` declares around `player_at` — the population lives in the
/// player's bubble, never on the far side of the city where the
/// recycler would collect it immediately.
pub fn draw_spawn(
    graph: &NavGraph,
    overrides: &NavOverrides,
    eligible: &[LaneId],
    roster: &AmbientRoster,
    rng: &mut NavRng,
    player_at: [f32; 3],
    policy: &SpawnPolicy,
) -> SpawnDraw {
    let lane = *rng.pick(eligible).expect("eligible is non-empty");
    let l = graph.lane(lane).expect("eligible lanes exist");
    let along = rng.next_f32() * l.length;
    let sample = graph.sample_lane(lane, along).expect("a live lane samples");
    let d = [
        sample.position[0] - player_at[0],
        sample.position[1] - player_at[1],
        sample.position[2] - player_at[2],
    ];
    let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if !(policy.min_player_distance..=policy.recycle_distance).contains(&dist) {
        return SpawnDraw::OutOfBand;
    }
    match roster.pick(rng) {
        Some(class) if roster.spawnable(class) => {
            let road = graph.road(lane.road).expect("a live lane's road exists");
            SpawnDraw::Placed(SpawnDirective {
                class,
                lane,
                along,
                sample,
                target_speed: overrides.effective_speed(road),
            })
        }
        // Some(unspawnable) or a non-closed weight table's None: the
        // draw selects nothing.
        other => SpawnDraw::Unspawnable(other),
    }
}

/// A car's position on the authored network — lane plus
/// travel-direction distance, the same convention
/// [`crate::nav::RouteCursor::distance`] uses.
/// [`NavGraph::sample_lane`] converts it to a world pose facing the
/// authored travel direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaneCursor {
    /// Lane the car is travelling on.
    pub lane: LaneId,
    /// Distance travelled along the lane.
    pub along: f32,
}

/// What [`advance_lane_cursor`] did with the step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneAdvance {
    /// Still on the same lane.
    Along,
    /// Crossed into a new lane through an intersection exit.
    Turned,
    /// No legal open exit — the car has run out of road.
    DeadEnd,
}

/// Advance `cursor` `ds` metres along the authored network, choosing a
/// seeded legal exit at each lane end. Exits onto roads the overrides
/// close are excluded, so closed roads are never entered; a car
/// already on one runs to its end and stops. Lane rank is preserved
/// across the crossing ([`NavGraph::transfer_lane`]), matching the
/// router's lane choice. `DeadEnd` means the car has run out of road —
/// the caller despawns/recycles it; the intersection controller
/// (F10-B) will later queue instead.
pub fn advance_lane_cursor(
    graph: &NavGraph,
    overrides: &NavOverrides,
    cursor: &mut LaneCursor,
    ds: f32,
    rng: &mut NavRng,
) -> LaneAdvance {
    let mut rest = ds.max(0.0);
    let mut turned = false;
    // Bound the crossing loop — a degenerate graph must terminate.
    for _ in 0..64 {
        let Some(lane) = graph.lane(cursor.lane) else {
            return LaneAdvance::DeadEnd;
        };
        let remaining = (lane.length - cursor.along).max(0.0);
        if rest < remaining {
            cursor.along += rest;
            return if turned {
                LaneAdvance::Turned
            } else {
                LaneAdvance::Along
            };
        }
        rest -= remaining;
        let exits: Vec<crate::nav::ArcExit> = graph
            .legal_exits(cursor.lane)
            .into_iter()
            .filter(|e| !overrides.is_closed(graph.arc(e.to).road))
            .collect();
        let Some(exit) = rng.pick(&exits).copied() else {
            return LaneAdvance::DeadEnd;
        };
        let Some(next) = graph.transfer_lane(cursor.lane, exit.to) else {
            return LaneAdvance::DeadEnd;
        };
        cursor.lane = next;
        cursor.along = 0.0;
        turned = true;
    }
    LaneAdvance::DeadEnd
}

/// Bounds on how a lane-following ambient car may change speed, plus
/// the corridor it senses blockers through. All values are designed:
/// the original's ambient braking/follow model is unverified (UNK-12
/// covers the policy constants generally), so these are chosen for
/// believable, collision-safe motion — a blocked car sheds speed,
/// holds a visible gap, and never shoves what it queues behind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FollowPolicy {
    /// Rate a clear car regains its road speed (m/s²).
    pub accel: f32,
    /// Rate a blocked car sheds speed (m/s²).
    pub decel: f32,
    /// Centre-to-centre gap the follower holds behind a blocker (m).
    pub follow_gap: f32,
    /// Gap below which the car stops outright — contact range, where
    /// rate-limiting would still roll it into the blocker (m).
    pub panic_gap: f32,
    /// Seconds of travel per metre of excess gap — the slope that
    /// converts room ahead into a desired speed.
    pub follow_time: f32,
    /// A blocked car at or below this speed counts as queued (m/s).
    pub held_speed: f32,
    /// Corridor reach at a standstill (m); grows with own speed.
    pub near: f32,
    /// Added corridor reach per m/s of own speed.
    pub lead: f32,
    /// Corridor half-width (m) — about a car width, so a blocker in
    /// the neighbouring lane does not count.
    pub half_width: f32,
    /// A blocker offset vertically by more than this rides another
    /// road level (overpass/underpass) and is not sensed (m).
    pub max_rise: f32,
    /// Speed cap applied on an intersection turn (m/s) — the cheap
    /// stand-in for corner braking until real curvature-aware braking
    /// lands (F10-B remainder).
    pub turn_speed: f32,
}

impl Default for FollowPolicy {
    fn default() -> Self {
        Self {
            accel: 4.0,
            decel: 9.0,
            follow_gap: 7.0,
            panic_gap: 4.5,
            follow_time: 1.2,
            held_speed: 1.0,
            near: 14.0,
            lead: 1.4,
            half_width: 2.4,
            max_rise: 3.0,
            turn_speed: 8.0,
        }
    }
}

/// Distance (m) to the nearest blocker inside a forward corridor —
/// `None` when the corridor ahead is clear out to `reach`.
///
/// `pos`/`fwd` are the follower's centre and heading; only the XZ
/// projection of `fwd` is used so a pitched lane still senses level
/// traffic. A blocker counts when it sits ahead (`0 < along <=
/// reach`), within `half_width` laterally, and within `max_rise`
/// vertically — the last test keeps a car on an underpass from
/// braking for the overpass above it.
pub fn corridor_gap(
    pos: [f32; 3],
    fwd: [f32; 3],
    half_width: f32,
    max_rise: f32,
    reach: f32,
    blockers: impl IntoIterator<Item = [f32; 3]>,
) -> Option<f32> {
    let n = (fwd[0] * fwd[0] + fwd[2] * fwd[2]).sqrt();
    if n < 1.0e-6 {
        return None;
    }
    let (fx, fz) = (fwd[0] / n, fwd[2] / n);
    let mut best: Option<f32> = None;
    for b in blockers {
        let dx = b[0] - pos[0];
        let dz = b[2] - pos[2];
        let along = dx * fx + dz * fz;
        if along <= 0.0 || along > reach || best.is_some_and(|c| along >= c) {
            continue;
        }
        let lateral = (dx * -fz + dz * fx).abs();
        if lateral > half_width || (b[1] - pos[1]).abs() > max_rise {
            continue;
        }
        best = Some(along);
    }
    best
}

/// One tick of the lane follower's speed law. `target` is the road's
/// speed limit; `gap` is [`corridor_gap`]'s distance to the nearest
/// blocker ahead. A clear corridor drives at `target` (rate-limited
/// by `accel`); a sensed blocker caps the desired speed so the car
/// rolls up to `follow_gap` behind it and holds, and a blocker inside
/// `panic_gap` stops it outright — a kinematic body that keeps moving
/// would shove whatever it touches. The response is bounded and
/// symmetric: the car waits rather than passing, and resumes the
/// moment the corridor clears; the only lasting recovery is the
/// distance recycler.
pub fn follow_speed(
    speed: f32,
    target: f32,
    gap: Option<f32>,
    dt: f32,
    policy: &FollowPolicy,
) -> f32 {
    let desired = match gap {
        Some(g) if g <= policy.panic_gap => return 0.0,
        Some(g) => target.min(((g - policy.follow_gap) / policy.follow_time).max(0.0)),
        None => target,
    };
    let dv = (desired - speed).clamp(-policy.decel * dt, policy.accel * dt);
    (speed + dv).max(0.0)
}
