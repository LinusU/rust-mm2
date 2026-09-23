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
//! annulus (`min_player_distance`…`recycle_distance` of the union of
//! player interest areas — see [`in_spawn_band`]) over the routable
//! vehicle lanes that survive the overrides (closed roads and
//! pedestrian-only/disabled road sides are already excluded from the
//! graph's arcs). A draw is also rejected when its exclusion box
//! touches occupied space — a live car, a participant, or an earlier
//! placement — so a spawn never materialises inside another vehicle
//! (F10-AC04). Everything is seeded through [`NavRng`] — the same
//! `(seed, content)` pair produces the same plan on every platform.
//!
//! This is *not* original traffic: the plan is spawn/despawn policy
//! data for the ambient system F10-B/C fills in — no intersection
//! yielding, signals or collisions are implied here. The original's
//! ambient population bound and bubble distances are unverified, so
//! [`SpawnPolicy`]'s defaults are designed values, not recovered ones.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use bevy::prelude::Entity;
use mm2_formats::bai::VehicleRule;
use mm2_formats::veh::AiVehicleData;

use crate::nav::{ArcEnd, LaneId, LaneSample, NavGraph, NavOverrides, NavRng};

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
    /// No spawn is placed within this distance of *any* player
    /// interest area, in metres — ambient cars materialising inside
    /// somebody's view cone is the failure this guards.
    pub min_player_distance: f32,
    /// A vehicle beyond this distance from *every* player interest
    /// area is recycled into the spawn pool (the ambient bubble
    /// radius), in metres; a spawn must land inside at least one
    /// interest area's radius.
    pub recycle_distance: f32,
    /// Bound on placement attempts per directive before the draw is
    /// dropped — keeps the planner finite when the player sits in the
    /// only eligible pocket.
    pub placement_attempts: usize,
    /// Longitudinal exclusion along the sampled tangent (m): a draw
    /// landing within this distance of an occupied point's
    /// ahead/behind projection is rejected — F10-AC04's "reject
    /// occupied space" leg, so a materialising car never overlaps a
    /// live hull or lands on a moving car's nose. Designed — about a
    /// car length; the original's spawn-collision behaviour is
    /// unverified (UNK-12).
    pub spawn_clearance: f32,
    /// Lateral exclusion half-width (m) — about a lane half-width, so
    /// a car in the neighbouring lane never blocks a spawn it does not
    /// physically overlap.
    pub spawn_half_width: f32,
    /// Vertical exclusion (m) — a car on an overpass above the spawn
    /// point does not occupy it.
    pub spawn_max_rise: f32,
}

impl Default for SpawnPolicy {
    fn default() -> Self {
        Self {
            max_active: 32,
            min_player_distance: 60.0,
            recycle_distance: 400.0,
            placement_attempts: 8,
            spawn_clearance: 5.0,
            spawn_half_width: 2.0,
            spawn_max_rise: 3.0,
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
    /// Directives dropped because every placement attempt landed
    /// outside the spawn annulus or inside `spawn_clearance` of space
    /// an earlier directive already claimed.
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
/// cumulative weights; placements outside the union of player interest
/// areas ([`in_spawn_band`] — inside at least one area's
/// `recycle_distance`, outside every area's `min_player_distance`)
/// retry up to `policy.placement_attempts` times before the directive
/// is dropped — the outer bound keeps the plan from populating road
/// the recycler would collect on its first tick. Each placed directive
/// also joins the occupied set later draws must keep `spawn_clearance`
/// from, so two planned cars can never stack on the same spot. Entries
/// of `interest` may be spawn poses, not tracked positions — the
/// planner only needs the bubble centres.
pub fn plan_ambient(
    graph: &NavGraph,
    overrides: &NavOverrides,
    roster: &AmbientRoster,
    seed: u64,
    density: f32,
    interest: &[[f32; 3]],
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
    // Space already claimed by this plan — each placed directive
    // joins it so later draws keep `spawn_clearance` away rather than
    // stacking two cars on one spot.
    let mut taken: Vec<[f32; 3]> = Vec::with_capacity(target);

    'draws: for _ in 0..target {
        if eligible.is_empty() {
            break;
        }
        for _ in 0..policy.placement_attempts {
            match draw_spawn(
                graph, overrides, &eligible, roster, &mut rng, interest, &taken, policy,
            ) {
                SpawnDraw::OutOfBand | SpawnDraw::Occupied => continue,
                SpawnDraw::Placed(directive) => {
                    taken.push(directive.sample.position);
                    spawns.push(directive);
                }
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
    /// The sampled position fell outside the union spawn band —
    /// inside `min_player_distance` of some interest area (a car must
    /// not materialise next to anybody) or beyond every area's
    /// `recycle_distance` (a placement past the recycler's own radius
    /// would be despawned on the next tick, so drawing it is pure
    /// churn). The caller retries on a fresh lane sample.
    OutOfBand,
    /// The sampled position's exclusion box touched an occupied point
    /// — a live ambient car, a participant, or an earlier placement —
    /// so the draw is rejected rather than materialising a car inside
    /// it. The caller retries on a fresh lane sample.
    Occupied,
    /// The class draw selected an unspawnable row (`Some`) or fell off
    /// a non-closed weight table (`None`) — this slot produces nothing;
    /// the authored band is never rebalanced.
    Unspawnable(Option<usize>),
}

/// Whether `sample`'s exclusion box touches an occupied point — the
/// "reject occupied space" test F10-AC04 names. The box is
/// `±spawn_clearance` along the sampled tangent, `±spawn_half_width`
/// lateral and `±spawn_max_rise` vertical: a same-lane neighbour
/// within about a car length occupies, while a car in the adjacent
/// lane or on an overpass does not. A degenerate tangent cannot
/// orient a box, so it occupies nothing (consistent with
/// [`corridor_gap`] sensing nothing).
pub fn spawn_occupied(sample: &LaneSample, occupied: &[[f32; 3]], policy: &SpawnPolicy) -> bool {
    let t = sample.tangent;
    let n = (t[0] * t[0] + t[2] * t[2]).sqrt();
    if n < 1.0e-6 {
        return false;
    }
    let (fx, fz) = (t[0] / n, t[2] / n);
    let along = policy.spawn_clearance.max(0.0);
    let lateral = policy.spawn_half_width.max(0.0);
    let rise = policy.spawn_max_rise.max(0.0);
    occupied.iter().any(|o| {
        let dx = o[0] - sample.position[0];
        let dz = o[2] - sample.position[2];
        (dx * fx + dz * fz).abs() <= along
            && (dx * -fz + dz * fx).abs() <= lateral
            && (o[1] - sample.position[1]).abs() <= rise
    })
}

/// Whether `position` lies inside the union of player interest areas
/// the spawn band declares (F10 spec req 2): within
/// `policy.recycle_distance` of *at least one* interest point — inside
/// somebody's bubble — and outside `policy.min_player_distance` of
/// *every* one, so a car can never materialise next to anybody no
/// matter which area admitted it. An empty `interest` set admits
/// nothing: with no player there is no bubble to populate.
pub fn in_spawn_band(position: [f32; 3], interest: &[[f32; 3]], policy: &SpawnPolicy) -> bool {
    let min2 = policy.min_player_distance * policy.min_player_distance;
    let max2 = policy.recycle_distance * policy.recycle_distance;
    let mut inside_any = false;
    for p in interest {
        let d = [position[0] - p[0], position[1] - p[1], position[2] - p[2]];
        let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        if d2 < min2 {
            return false;
        }
        inside_any |= d2 <= max2;
    }
    inside_any
}

/// Whether `position` is still inside somebody's bubble — within
/// `policy.recycle_distance` of at least one interest point. The
/// recycler collects a live car only when this returns `false` for
/// the whole set, so a car parked beside a far-away player survives
/// the local bubble leaving it (F10-AC04's "near any player" leg).
pub fn within_interest(position: [f32; 3], interest: &[[f32; 3]], policy: &SpawnPolicy) -> bool {
    let max2 = policy.recycle_distance * policy.recycle_distance;
    interest.iter().any(|p| {
        let d = [position[0] - p[0], position[1] - p[1], position[2] - p[2]];
        d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= max2
    })
}

/// One spawn-placement attempt — [`plan_ambient`] retries it
/// `policy.placement_attempts` times per directive and the runtime
/// recycler reuses it to top the population back up. Placements are
/// drawn inside the union of player interest areas ([`in_spawn_band`])
/// — the population lives in *somebody's* bubble, never on the far
/// side of the city where the recycler would collect it immediately —
/// and rejected when the sampled exclusion box touches a point
/// `occupied` reports (live cars, participants, earlier placements).
#[allow(clippy::too_many_arguments)] // the draw threads the same plan state the planner/recycler share
pub fn draw_spawn(
    graph: &NavGraph,
    overrides: &NavOverrides,
    eligible: &[LaneId],
    roster: &AmbientRoster,
    rng: &mut NavRng,
    interest: &[[f32; 3]],
    occupied: &[[f32; 3]],
    policy: &SpawnPolicy,
) -> SpawnDraw {
    let lane = *rng.pick(eligible).expect("eligible is non-empty");
    let l = graph.lane(lane).expect("eligible lanes exist");
    let along = rng.next_f32() * l.length;
    let sample = graph.sample_lane(lane, along).expect("a live lane samples");
    if !in_spawn_band(sample.position, interest, policy) {
        return SpawnDraw::OutOfBand;
    }
    // Rejected before the class pick so an occupied spot never burns
    // a roster draw.
    if spawn_occupied(&sample, occupied, policy) {
        return SpawnDraw::Occupied;
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

/// A committed junction crossing (F10-B.9): the generated interior
/// path plus progress along it. While a crossing is active the
/// cursor's `lane` still names the *approach* lane — the transfer
/// commits only when the path runs out, so a mid-box car reports the
/// road it left, never a lane it has not reached.
#[derive(Debug, Clone, PartialEq)]
pub struct Crossing {
    /// Lane the crossing lands on — the seeded exit's rank-preserved
    /// lane on the next arc.
    pub landing: LaneId,
    /// The generated interior path.
    pub path: crate::nav::CrossingPath,
    /// Distance travelled along the path (m).
    pub along: f32,
}

/// A car's position on the authored network — lane plus
/// travel-direction distance, the same convention
/// [`crate::nav::RouteCursor::distance`] uses, plus an in-flight
/// junction [`Crossing`]. [`LaneCursor::sample`] converts it to a
/// world pose facing the authored travel direction — continuous
/// across the junction interior, where sampling the landing lane
/// alone would teleport the car between the lane extremities.
#[derive(Debug, Clone, PartialEq)]
pub struct LaneCursor {
    /// Lane the car is travelling on — the approach lane while a
    /// `crossing` is active.
    pub lane: LaneId,
    /// Distance travelled along the lane.
    pub along: f32,
    /// The junction interior being traversed, when the car has
    /// committed to a transfer but not yet landed.
    pub crossing: Option<Crossing>,
}

impl LaneCursor {
    /// A cursor at `along` metres on `lane`, not crossing.
    pub fn new(lane: LaneId, along: f32) -> Self {
        Self {
            lane,
            along,
            crossing: None,
        }
    }

    /// The cursor's world pose: the crossing path while a junction
    /// interior is being traversed, the lane curve otherwise.
    pub fn sample(&self, graph: &NavGraph) -> Option<LaneSample> {
        match &self.crossing {
            Some(c) => Some(c.path.sample(c.along)),
            None => graph.sample_lane(self.lane, self.along),
        }
    }
}

/// What [`advance_lane_cursor`] did with the step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneAdvance {
    /// Still on the same lane — no junction involvement this step.
    Along,
    /// Traversed a junction interior — the car is inside the box.
    Crossing,
    /// Committed to a junction crossing this step: the car left the
    /// approach lane and `cursor.crossing` holds the path and the
    /// landing lane — or already landed when one step covered the
    /// whole interior (`cursor.crossing` is then `None`). The caller
    /// should check the landing for occupancy before letting the
    /// transfer stand. `Entered` takes precedence over `Landed` so a
    /// commit never goes unreported.
    Entered,
    /// Completed a transfer this step without committing a new
    /// crossing — an earlier crossing ran out, or a coincident
    /// endpoint needed no interior path — and the cursor now sits on
    /// the landing lane.
    Landed,
    /// No legal open exit — the car has run out of road.
    DeadEnd,
}

/// Advance `cursor` `ds` metres along the authored network, choosing a
/// seeded legal exit at each lane end. Exits onto roads the overrides
/// close are excluded, so closed roads are never entered; a car
/// already on one runs to its end and stops. Lane rank is preserved
/// across the crossing ([`NavGraph::transfer_lane`]), matching the
/// router's lane choice.
///
/// A lane end that opens into a junction interior commits a
/// [`Crossing`] instead of teleporting the cursor to the landing
/// lane's start (F10-B.9): `ds` metres past the lane end flow into
/// the generated interior path, the path's own length is consumed
/// before the landing lane's, and the transfer only commits when the
/// car physically reaches the far side. `DeadEnd` means the car has
/// run out of road — the caller despawns/recycles it.
pub fn advance_lane_cursor(
    graph: &NavGraph,
    overrides: &NavOverrides,
    cursor: &mut LaneCursor,
    ds: f32,
    rng: &mut NavRng,
) -> LaneAdvance {
    let mut rest = ds.max(0.0);
    let mut result = LaneAdvance::Along;
    // A crossing committed this call reports `Entered` even when the
    // same step lands it — the commit bookkeeping (queue release,
    // landing clearance) must not be swallowed by the landing's.
    let mut committed = false;
    // Bound the transfer loop — a degenerate graph must terminate.
    for _ in 0..64 {
        if let Some(crossing) = &mut cursor.crossing {
            // Inside the junction: the interior path consumes the
            // step before the landing lane does.
            let left = (crossing.path.length - crossing.along).max(0.0);
            if rest < left {
                crossing.along += rest;
                return if committed {
                    LaneAdvance::Entered
                } else {
                    LaneAdvance::Crossing
                };
            }
            rest -= left;
            cursor.lane = crossing.landing;
            cursor.along = 0.0;
            cursor.crossing = None;
            result = LaneAdvance::Landed;
            continue;
        }
        let Some(lane) = graph.lane(cursor.lane) else {
            return LaneAdvance::DeadEnd;
        };
        let remaining = (lane.length - cursor.along).max(0.0);
        if rest < remaining {
            cursor.along += rest;
            return if committed {
                LaneAdvance::Entered
            } else {
                result
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
        match graph.crossing_path(cursor.lane, next) {
            Some(path) => {
                cursor.crossing = Some(Crossing {
                    landing: next,
                    path,
                    along: 0.0,
                });
                committed = true;
            }
            // Coincident endpoints carry no interior — the transfer
            // lands directly.
            None => {
                cursor.lane = next;
                cursor.along = 0.0;
                result = LaneAdvance::Landed;
            }
        }
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

// ---------- junction rules (F10-B.2) ----------

/// Signal and right-of-way policy constants for the junction
/// controller. The rule *kinds* are authored — the BAI `vehicleRule`
/// code on every road end (`mm2_formats::bai::RoadEnd::vehicle_rule`)
/// documents `StopSign` ("longest waiting vehicle drives first"),
/// `TrafficLight` ("one road at a time"), `AlwaysStop` and
/// `NeverStop`. The timing and distance values are designed: the
/// original's signal period, stop dwell, stop-line placement and
/// entry clearance are unverified (UNK-12).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JunctionPolicy {
    /// Ticks each light-controlled member road holds green — at the
    /// 120 Hz fixed step the default is a six-second phase.
    pub green_ticks: u64,
    /// All-red clearance ticks between member greens, so a crossing
    /// car clears the junction before the next road is admitted.
    pub clear_ticks: u64,
    /// Ticks a car must stand at a stop sign before it may take the
    /// junction — the "stop" half of the documented stop-sign rule.
    pub stop_dwell_ticks: u64,
    /// Distance before the lane end marking the stop line (m) — about
    /// a car half-length, so a held car's nose stays out of the box.
    pub stop_inset: f32,
    /// A car this close to the stop line counts as standing on it
    /// (m). The brake ramp decays `dist_to_stop` geometrically — an
    /// f32 lane cursor asymptotes a hair short of an exact zero and
    /// stalls — so "at the line" is this tolerance, never
    /// `dist_to_stop <= 0`, or a queued car would never register.
    pub stop_line_tolerance: f32,
    /// Seconds per metre converting distance-to-stop into the desired
    /// approach speed — the ramp a closed gate brakes toward.
    pub approach_time: f32,
    /// Decel bound while braking to a closed gate (m/s²).
    pub decel: f32,
    /// A lane transfer landing within this distance of a live car or
    /// participant would materialise inside it — the car holds at the
    /// lane end and retries instead (m).
    pub enter_clearance: f32,
    /// Padding added to the farthest member end's distance from the
    /// junction centre to size the occupancy zone [`junction_zone`]
    /// reports (m) — the box a yielded approach waits on. Sized so a
    /// car that just turned keeps occupying until its hull has left
    /// the junction, not merely crossed the endpoint ring.
    pub box_margin: f32,
    /// Vertical band around the junction zone (m) — a car on an
    /// overpass crossing above the junction does not occupy it.
    pub box_max_rise: f32,
}

impl Default for JunctionPolicy {
    fn default() -> Self {
        Self {
            green_ticks: 720,
            clear_ticks: 120,
            stop_dwell_ticks: 90,
            stop_inset: 2.5,
            stop_line_tolerance: 0.1,
            approach_time: 1.0,
            decel: 9.0,
            enter_clearance: 6.0,
            box_margin: 3.0,
            box_max_rise: 3.0,
        }
    }
}

/// Whether a car may pass its lane end this tick, per [`Junctions::gate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JunctionGate {
    /// Proceed — no rule, `NeverStop`, the car's road green, an
    /// admitted stop-sign head, or a lane that does not end at a
    /// junction.
    Open,
    /// Brake to the stop line and wait.
    Closed,
}

/// What an authored traffic signal shows, per
/// [`Junctions::signal_aspect`] — a designed presentation mapping the
/// approach's *rule* admission, not a recovered original display
/// (UNK-12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalAspect {
    /// The approach is admitted by its rule — green phase or free
    /// flow.
    Green,
    /// The approach is held by its rule — a light member out of
    /// phase, or an `AlwaysStop` end.
    Red,
    /// The approach must stop before proceeding — a stop-signed end.
    Stop,
}

/// Per-intersection controller state the ambient driver consults each
/// tick — the "one authoritative controller" the F10 spec asks for,
/// scoped to junction admission only (ambient routing stays separate
/// from opponents/pursuit per the spec).
///
/// Two documented mechanisms live here:
///
/// - **Signals**: a junction whose member roads author `TrafficLight`
///   approaches cycles a green through those members — the roads (in
///   authored counterclockwise order) that have at least one arc
///   exiting into the junction under that rule — with an all-red
///   clearance slice inside each period. Mixed junctions are the
///   retail norm (SF: 82 of 212 vehicle-approached junctions
///   uniformly lit, 92 mixed, none uniformly stop-signed; London:
///   76/76/102 of 254) — each approach gates by its own end's rule:
///   `NeverStop` approaches flow regardless of the phase.
/// - **Stop signs**: a first-come-first-served queue per junction —
///   the documented "longest waiting vehicle drives first". A car
///   registers when it stands at its stop line, takes the junction
///   once it reaches the head and its dwell has elapsed, and is
///   dropped on departure.
///
/// `AlwaysStop` ends are never admitted (unused on retail data).
/// Queue keys are `Entity`s: junction admission is ephemeral
/// per-world state, never a result or network identity.
///
/// F10-B.5 layers the box-yield on top of the rule gates: the caller
/// reports whether the junction's occupancy zone ([`junction_zone`])
/// holds a vehicle not bound for it, and [`Junctions::gate`] keeps an
/// otherwise-admitted approach closed while it does. That is the
/// F10-AC02 right-of-way leg the rules alone cannot express — the
/// authored cycle decides *whose turn* it is, not whether the box is
/// physically passable.
#[derive(Default)]
pub struct Junctions {
    /// Fixed-step clock — `advance_tick` runs once per driver tick, so
    /// phases and dwells are deterministic under the session seed.
    tick: u64,
    /// The constants this controller runs under — `pub` so sessions
    /// and tests can bind a different cadence.
    pub policy: JunctionPolicy,
    /// Junction index → FIFO of `(car, arrival tick)` standing at its
    /// stop-signed approaches.
    waiting: BTreeMap<u16, VecDeque<(Entity, u64)>>,
}

/// Per-junction signal-phase desynchronisation, in ticks — a fixed
/// spread so neighbouring junctions never share a phase edge.
const PHASE_SPREAD: u64 = 137;

impl Junctions {
    /// Advance the controller clock — call once per drive tick, under
    /// the same phase gate the driver runs.
    pub fn advance_tick(&mut self) {
        self.tick += 1;
    }

    /// The controller's current tick (diagnostics/tests).
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Cars standing at stop-signed approaches across all junctions.
    pub fn waiting(&self) -> usize {
        self.waiting.values().map(VecDeque::len).sum()
    }

    /// The junction `lane`'s travel arc exits into, the arc's road,
    /// and the authored rule at that end — `None` when the lane ends
    /// at a dead end or carries no routable arc.
    pub fn approach(graph: &NavGraph, lane: LaneId) -> Option<(u16, u16, Option<VehicleRule>)> {
        let l = graph.lane(lane)?;
        let arc = graph.arc(l.arc?);
        match arc.exit {
            ArcEnd::Intersection(ix) => Some((ix, arc.road, arc.exit_rule())),
            ArcEnd::DeadEnd => None,
        }
    }

    /// Member roads the signal at `ix` cycles through: every road (in
    /// the intersection's authored counterclockwise order, deduped)
    /// with at least one arc exiting into the junction under a
    /// `TrafficLight` end. Empty when nothing at the junction is
    /// light-controlled.
    pub fn signal_members(graph: &NavGraph, ix: u16) -> Vec<u16> {
        let Some(intersection) = graph.intersections().get(ix as usize) else {
            return Vec::new();
        };
        let mut members = Vec::new();
        for &idx in &intersection.roads {
            let Ok(road_idx) = u16::try_from(idx) else {
                continue;
            };
            let Some(road) = graph.road(road_idx) else {
                continue;
            };
            let lit = road.arcs.iter().flatten().any(|a| {
                let arc = graph.arc(*a);
                arc.exit == ArcEnd::Intersection(ix)
                    && arc.exit_rule() == Some(VehicleRule::TrafficLight)
            });
            if lit && !members.contains(&road_idx) {
                members.push(road_idx);
            }
        }
        members
    }

    /// The member road currently green at `ix`, or `None` during the
    /// all-red clearance slice. Deterministic: the member index is
    /// `(tick + ix·PHASE_SPREAD) / period mod members`, green for the
    /// first `green_ticks` of each `green+clear` period.
    pub fn green_road(&self, graph: &NavGraph, ix: u16) -> Option<u16> {
        let members = Self::signal_members(graph, ix);
        self.green_member(ix, &members)
    }

    fn green_member(&self, ix: u16, members: &[u16]) -> Option<u16> {
        let n = members.len() as u64;
        if n == 0 {
            return None;
        }
        let period = self.policy.green_ticks + self.policy.clear_ticks;
        if period == 0 {
            return members.first().copied();
        }
        let t = (self.tick + ix as u64 * PHASE_SPREAD) % (period * n);
        let member = (t / period) as usize;
        (t % period < self.policy.green_ticks).then(|| members[member])
    }

    /// May `car` pass the end of `lane` this tick? The authored rule
    /// at the arc's downstream end decides: `NeverStop`/unruled/dead
    /// ends open, `AlwaysStop` never opens, `TrafficLight` opens while
    /// the car's road holds the phase green, and `StopSign` opens for
    /// the FCFS head once its dwell elapsed. Reaching the stop line
    /// (`at_line` — the caller judges it against
    /// [`JunctionPolicy::stop_line_tolerance`], since the f32 cursor
    /// never lands on an exact zero) at a standstill (`stopped`)
    /// registers a stop-sign car in the junction queue — registration
    /// is what makes the wait ordering first-come-first-served.
    ///
    /// `box_occupied` is the caller's junction-box occupancy report
    /// (see [`junction_zone`]): an *admitted* approach — green member
    /// or FCFS head — still closes while the box holds a vehicle the
    /// rule cannot see (F10-AC02's right-of-way leg: a green light
    /// does not make a blocked box passable). It is consulted only on
    /// the two paths where a gated rule can open; `NeverStop`/
    /// unruled ends keep their documented free flow regardless.
    pub fn gate(
        &mut self,
        graph: &NavGraph,
        lane: LaneId,
        car: Entity,
        at_line: bool,
        stopped: bool,
        box_occupied: bool,
    ) -> JunctionGate {
        let Some((ix, road, rule)) = Self::approach(graph, lane) else {
            return JunctionGate::Open;
        };
        match rule {
            None | Some(VehicleRule::NeverStop) => JunctionGate::Open,
            Some(VehicleRule::AlwaysStop) => JunctionGate::Closed,
            Some(VehicleRule::TrafficLight) => {
                let members = Self::signal_members(graph, ix);
                if !members.contains(&road) {
                    // A light-coded end on a junction whose member set
                    // somehow excludes its road (inconsistent authored
                    // data) degrades to free flow, not a permanent red.
                    return JunctionGate::Open;
                }
                match self.green_member(ix, &members) {
                    Some(green) if green == road && !box_occupied => JunctionGate::Open,
                    _ => JunctionGate::Closed,
                }
            }
            Some(VehicleRule::StopSign) => {
                if at_line && stopped {
                    let q = self.waiting.entry(ix).or_default();
                    if !q.iter().any(|(c, _)| *c == car) {
                        q.push_back((car, self.tick));
                    }
                }
                let admitted = self.waiting.get(&ix).and_then(|q| q.front()).is_some_and(
                    |(front, arrived)| {
                        *front == car
                            && self.tick.saturating_sub(*arrived) >= self.policy.stop_dwell_ticks
                    },
                );
                if admitted && !box_occupied {
                    JunctionGate::Open
                } else {
                    JunctionGate::Closed
                }
            }
        }
    }

    /// The aspect a signal on `road`'s approach into `ix` displays
    /// this tick — the *rule* admission only (F10-B.7). This mirrors
    /// [`Junctions::gate`] minus the box-yield: a light member shows
    /// green while it holds the phase, red otherwise (including the
    /// all-red clearance); a `NeverStop`/unruled end shows green
    /// constantly; a `StopSign` end shows the stop-controlled aspect
    /// (the per-car FCFS admission cannot be read off a signal); an
    /// `AlwaysStop` end shows red since its gate never opens. A
    /// light-coded end on a road outside the member set shows green,
    /// the same `Open` fallback `gate` resolves to. `members` is the
    /// caller's [`Junctions::signal_members`] result — the caller
    /// caches it per junction when evaluating many signals a tick.
    ///
    /// This is a designed presentation mapping: the authored data
    /// places the signal heads, the authored rules pick the aspect;
    /// the original's signal visuals and exact state semantics are
    /// unverified (UNK-12).
    pub fn signal_aspect(
        &self,
        ix: u16,
        members: &[u16],
        road: u16,
        rule: Option<VehicleRule>,
    ) -> SignalAspect {
        match rule {
            None | Some(VehicleRule::NeverStop) => SignalAspect::Green,
            Some(VehicleRule::AlwaysStop) => SignalAspect::Red,
            Some(VehicleRule::StopSign) => SignalAspect::Stop,
            Some(VehicleRule::TrafficLight) => {
                if !members.contains(&road) {
                    return SignalAspect::Green;
                }
                match self.green_member(ix, members) {
                    Some(green) if green == road => SignalAspect::Green,
                    _ => SignalAspect::Red,
                }
            }
        }
    }

    /// Forget `car` everywhere — on transfer out of a junction and on
    /// despawn, so a stale entry can never hold the queue.
    pub fn depart(&mut self, car: Entity) {
        self.waiting.retain(|_, q| {
            q.retain(|(c, _)| *c != car);
            !q.is_empty()
        });
    }

    /// Drop queued cars absent from `live` — the recycler can collect
    /// a registered car, and the queue must not wait on a ghost.
    pub fn retain(&mut self, live: &BTreeSet<Entity>) {
        self.waiting.retain(|_, q| {
            q.retain(|(c, _)| live.contains(c));
            !q.is_empty()
        });
    }
}

/// The junction's occupancy zone for the box-yield (F10-B.5): its
/// centre (the authored `center`, falling back to the member-endpoint
/// centroid when that is non-finite) and the XZ radius reaching the
/// farthest member road's end position at the junction plus
/// [`JunctionPolicy::box_margin`]. `None` only when neither a finite
/// centre nor any member end exists — the caller then cannot form a
/// box and yields nothing.
///
/// The member ends are the box boundary in the authored data: a car
/// turning off an approach lands just past one and keeps occupying
/// until it has driven the margin clear. A car still *bound* for the
/// junction — waiting at or behind its own stop line — must never be
/// counted as an occupant by the caller, or two competing approaches
/// would hold each other forever; that exclusion is the caller's job
/// because only it knows each blocker's destination.
pub fn junction_zone(
    graph: &NavGraph,
    ix: u16,
    policy: &JunctionPolicy,
) -> Option<([f32; 3], f32)> {
    let intersection = graph.intersections().get(ix as usize)?;
    let mut ends: Vec<[f32; 3]> = Vec::new();
    for &idx in &intersection.roads {
        let Ok(road_idx) = u16::try_from(idx) else {
            continue;
        };
        let Some(road) = graph.road(road_idx) else {
            continue;
        };
        for a in road.arcs.iter().flatten() {
            let arc = graph.arc(*a);
            if arc.exit == ArcEnd::Intersection(ix) {
                ends.push(arc.exit_point);
            }
            if arc.entry == ArcEnd::Intersection(ix) {
                ends.push(arc.entry_point);
            }
        }
    }
    let center = if intersection.center.iter().all(|c| c.is_finite()) {
        intersection.center
    } else if !ends.is_empty() {
        let mut c = [0.0f32; 3];
        for p in &ends {
            for a in 0..3 {
                c[a] += p[a];
            }
        }
        for v in &mut c {
            *v /= ends.len() as f32;
        }
        c
    } else {
        return None;
    };
    let radius = ends
        .iter()
        .map(|p| {
            let dx = p[0] - center[0];
            let dz = p[2] - center[2];
            dx * dx + dz * dz
        })
        .fold(0.0f32, f32::max)
        .sqrt()
        + policy.box_margin.max(0.0);
    Some((center, radius))
}

/// Whether `pos` lies inside `zone` as [`junction_zone`] reports it:
/// XZ within the radius and within [`JunctionPolicy::box_max_rise`]
/// vertically, so a car crossing above or below on another road level
/// does not occupy the junction.
pub fn inside_junction_zone(pos: [f32; 3], zone: ([f32; 3], f32), policy: &JunctionPolicy) -> bool {
    let (c, r) = zone;
    let dx = pos[0] - c[0];
    let dz = pos[2] - c[2];
    dx * dx + dz * dz <= r * r && (pos[1] - c[1]).abs() <= policy.box_max_rise.max(0.0)
}

// ---------- stuck recovery (F10-B.3) ----------

/// Bounds on how long an ambient car may go without progress before
/// the runtime recycles it. All values are designed: the original's
/// stuck handling is unverified (UNK-12 covers the ambient policy
/// constants generally), so the window is sized against the worst
/// *legitimate* wait this controller can impose — a junction's
/// longest red is `(members − 1) × (green + clear)` under
/// [`JunctionPolicy`], ~21 s at four members and ~35 s at six — not
/// tuned to observed retail behaviour.
///
/// The recovery the runtime applies on expiry is despawn into the
/// pool the maintainer refills from — bounded, never a teleport
/// through whatever pens the car (the F10 spec's explicit bar on
/// aggressive unconditional teleports). A multi-cycle queue tail
/// *can* outwait the window at a heavily loaded signal: that car is
/// sacrificed to the recycler, which is the designed trade — the
/// freed slot repopulates elsewhere and the queue drains instead of
/// freezing the population.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StuckPolicy {
    /// Drive ticks a car may go without `min_displacement` of
    /// progress before it is declared stuck — 4800 is 40 s at the
    /// 120 Hz fixed step, roughly double a four-member signal's worst
    /// red and beyond a six-member one's.
    pub window_ticks: u64,
    /// Progress that resets the window (m). Large enough that
    /// stop-line creep and solver jitter never read as progress;
    /// small enough that a queue genuinely advancing a car-length at
    /// a time keeps its cars.
    pub min_displacement: f32,
}

impl Default for StuckPolicy {
    fn default() -> Self {
        Self {
            window_ticks: 4800,
            min_displacement: 4.0,
        }
    }
}

/// A car's displacement window for stuck detection: the anchor the
/// window measures from plus the consecutive drive ticks gone without
/// `min_displacement` of progress. The test is "cannot get anywhere",
/// not "moved slowly" — a penned car and a parked blocker look the
/// same to a speed check, but only progress resets the window, so a
/// car held at a legitimate red whose light turns green re-anchors on
/// its first metres and never recovers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StuckWindow {
    /// World position the current window measures progress from.
    pub anchor: [f32; 3],
    /// Consecutive ticks the car has stayed within
    /// [`StuckPolicy::min_displacement`] of `anchor`.
    pub ticks: u64,
}

impl StuckWindow {
    /// Start a window at `pos` — call once at spawn.
    pub fn new(pos: [f32; 3]) -> Self {
        Self {
            anchor: pos,
            ticks: 0,
        }
    }

    /// One drive tick; `pos` is the car's position *after* this
    /// tick's move. Returns `true` on the tick the window expires —
    /// and keeps returning `true` while the caller leaves the car
    /// alive, so a late despawn still reads stuck. A non-finite
    /// position resets the window rather than counting as "no
    /// progress": the driver already despawns non-finite poses
    /// upstream, and a NaN gap must not masquerade as a stall.
    pub fn tick(&mut self, pos: [f32; 3], policy: &StuckPolicy) -> bool {
        if !pos.iter().all(|c| c.is_finite()) {
            self.ticks = 0;
            return false;
        }
        let d = [
            pos[0] - self.anchor[0],
            pos[1] - self.anchor[1],
            pos[2] - self.anchor[2],
        ];
        let reach = policy.min_displacement.max(0.0);
        if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] >= reach * reach {
            self.anchor = pos;
            self.ticks = 0;
            false
        } else {
            self.ticks = self.ticks.saturating_add(1);
            self.ticks >= policy.window_ticks
        }
    }
}

// ---------- collision handover (F10-B.6) ----------

/// Bounds on the kinematic→dynamic handover a lane-following ambient
/// car takes when a contact hits it hard enough (F10-B.6). All values
/// are designed — the original's ambient collision behaviour is
/// unverified (UNK-12 covers the ambient policy constants generally).
///
/// The gate is a linear impulse *estimate* — approach speed × striker
/// mass — so a real vehicle hit hands the car to the solver
/// while a light brush, or a lane path scraping world geometry (a
/// massless striker reads 1 kg), never does. Banger activation reads
/// the same contact differently: since F04-C.5 it compares striker
/// kinetic energy `½·m·v²` to the authored `ImpulseLimit2`
/// (DSN-10/UNK-22); only the deepest-contact severity and the
/// striker-mass resolution are shared. The handover itself adds
/// at most the striker's approach speed of velocity along the contact
/// normal — the energy the impact actually carried, never a scaled-up
/// kick — so "transition to dynamic behaviour without injecting
/// extreme energy" (F10 req 4) holds by construction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KnockPolicy {
    /// Impulse estimate (kg·m/s) at or above which contact with a
    /// lane-following car flips it to a dynamic body. 4000 reads
    /// ~3 m/s off a stock-mass striker and ~8 m/s off a light one —
    /// a real hit, not a parking-lot nudge.
    pub min_impulse: f32,
}

impl Default for KnockPolicy {
    fn default() -> Self {
        Self {
            min_impulse: 4000.0,
        }
    }
}

/// The speed a closed gate allows at `dist_to_stop` metres before the
/// stop line — a `decel`-limited ramp to a standstill at the line. An
/// open gate imposes nothing (returns `speed`); the law never
/// accelerates a car — the follow law owns that.
pub fn junction_speed(
    speed: f32,
    dist_to_stop: f32,
    gate: JunctionGate,
    dt: f32,
    policy: &JunctionPolicy,
) -> f32 {
    let speed = speed.max(0.0);
    if gate == JunctionGate::Open {
        return speed;
    }
    let ramp = (dist_to_stop / policy.approach_time.max(1.0e-3)).max(0.0);
    if speed > ramp {
        (speed - policy.decel.max(0.0) * dt).max(ramp)
    } else {
        speed
    }
}
