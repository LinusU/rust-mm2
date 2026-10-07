//! Pedestrian sidewalk movement (F19-B.1): where pedestrians may stand,
//! how a seeded density places them, and how a walker travels along the
//! authored sidewalk curves.
//!
//! The BAI road network authors one polyline per sidewalk
//! ([`LaneKind::Sidewalk`], `nSidewalks` per road side — 1080 on London,
//! 758 on San Francisco). Those curves are the only ground a walker is
//! ever placed on or moved along, so a pedestrian can never be planned
//! onto a vehicle lane (F19 req 3: "keep pedestrians off ordinary
//! vehicle-only lanes except verified crossings" — no crossing is
//! verified yet, so there are none).
//!
//! What the original does at the end of a sidewalk curve — how a walker
//! gets round a corner, or across a street — is **unrecovered**
//! (UNK-42). The designed policy here: two sidewalk curve ends within
//! [`WalkPolicy::join_radius`] (and [`WalkPolicy::join_rise`] vertically)
//! of each other are the same kerb corner, so a walker reaching one end
//! continues onto a seeded pick of the others (never straight back onto
//! the curve it just left when there is another choice); an end with no
//! neighbour is a dead end and the walker turns round. A join is also
//! refused when the straight line between the two ends crosses a
//! vehicle lane, so no radius can make a walker step across a
//! carriageway; crossing a street mouth at a crosswalk is not modelled
//! (no crossing is verified) and stays open under F19-AC04.
//!
//! Everything is deterministic from a seed: the planner and the
//! [`NavRng`] choices use no hash-ordered iteration, so one seed gives
//! one crowd on every platform.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::nav::{LaneId, LaneKind, LaneQuery, LaneSample, NavGraph, NavOverrides, NavRng};

/// Sidewalk curves shorter than this (m) carry no walker: too short to
/// hold a figure, and a zero-length curve would let an advance loop
/// without covering ground.
pub const MIN_WALK_LANE: f32 = 1.0;

/// Longest single advance honoured (m). A frame spike or a debugger
/// pause cannot teleport a walker across the district in one call.
pub const MAX_ADVANCE: f32 = 50.0;

/// Curve ends one [`SidewalkNet::advance`] call may meet. A step is at
/// most [`MAX_ADVANCE`] metres over curves of at least
/// [`MIN_WALK_LANE`] — every end met costs a metre or more — so the
/// bound is unreachable today; it is the defensive limit that keeps
/// recovery terminating if either constant is ever relaxed (F19-AC04).
pub const MAX_HOPS: u32 = 64;

/// Designed placement and traversal bounds. None of these values is an
/// original constant (UNK-42): the density → population mapping, the
/// spawn annulus and the corner join are choices made so the behaviour
/// is bounded and reproducible.
#[derive(Debug, Clone, PartialEq)]
pub struct WalkPolicy {
    /// Bound on simultaneously active walkers — the density fraction
    /// scales the target inside this cap. Must stay at or below the
    /// app's actor cap.
    pub max_active: usize,
    /// No spawn lands within this distance of any interest point (m):
    /// nobody should materialise at a player's feet.
    pub min_player_distance: f32,
    /// A walker beyond this distance from every interest point is
    /// recycled; a spawn must land inside at least one point's radius.
    pub recycle_distance: f32,
    /// Placement attempts per directive before the draw is dropped, so
    /// the planner stays finite when the interest area holds no
    /// eligible pocket.
    pub placement_attempts: usize,
    /// Two walkers are never planned closer than this (m).
    pub spacing: f32,
    /// Gap (m) below which two curve ends are one kerb corner. Measured
    /// on retail (`mm2-inspect nav`): corner gaps are spread over
    /// 1–12 m rather than clustered, so the value is a designed trade
    /// between walkable reach and staying on the kerb side of the
    /// street (UNK-42).
    pub join_radius: f32,
    /// Vertical gap (m) above which two nearby ends are different
    /// levels (a bridge over a street) and never join.
    pub join_rise: f32,
}

impl Default for WalkPolicy {
    fn default() -> Self {
        Self {
            max_active: 48,
            min_player_distance: 15.0,
            recycle_distance: 90.0,
            placement_attempts: 24,
            spacing: 2.0,
            join_radius: 8.0,
            join_rise: 1.0,
        }
    }
}

/// Which way a walker travels along its curve, relative to the stored
/// vertex order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkDir {
    /// Start → end.
    Forward,
    /// End → start.
    Backward,
}

impl WalkDir {
    fn flipped(self) -> Self {
        match self {
            Self::Forward => Self::Backward,
            Self::Backward => Self::Forward,
        }
    }
}

/// A pedestrian's place on the sidewalk network.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Walker {
    /// Sidewalk curve the walker is on.
    pub lane: LaneId,
    /// Distance along the curve from its stored start (m).
    pub s: f32,
    /// Travel direction.
    pub dir: WalkDir,
    /// Curve the walker arrived from at its last corner, so the next
    /// corner prefers somewhere new.
    pub from: Option<LaneId>,
}

/// What one [`SidewalkNet::advance`] call did besides moving.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WalkStep {
    /// Corner joins taken onto another curve.
    pub hops: u32,
    /// Dead ends turned round at.
    pub turned_around: u32,
}

/// One end of a sidewalk curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Start,
    Finish,
}

/// `(curve index into `SidewalkNet::lanes`, which end)`.
type EndRef = (usize, End);

#[derive(Debug, Clone)]
struct WalkLane {
    id: LaneId,
    length: f32,
    /// Ends of other curves that share this curve's `Start` / `Finish`
    /// corner, nearest first.
    joins: [Vec<EndRef>; 2],
}

/// Census of one network build.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WalkNetStats {
    /// Sidewalk curves the graph carries.
    pub curves: usize,
    /// Curves a walker may use.
    pub walkable: usize,
    /// Curves dropped: shorter than [`MIN_WALK_LANE`], non-finite
    /// geometry, or on a road the event closes.
    pub excluded: usize,
    /// Ends of walkable curves (two per curve).
    pub ends: usize,
    /// Ends sharing a corner with at least one other curve's end.
    pub joined_ends: usize,
    /// Candidate joins refused because the line between the two ends
    /// crosses a vehicle lane (each pair counted from both ends).
    pub severed: usize,
}

/// A problem found while planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalkIssue {
    /// The graph holds no walkable sidewalk curve — nothing can spawn.
    NoSidewalks,
    /// No interest point was supplied: with nobody to populate around,
    /// nothing spawns.
    NoInterest,
}

/// The sidewalk curves a walker may use, with their corner joins.
#[derive(Debug, Clone)]
pub struct SidewalkNet {
    lanes: Vec<WalkLane>,
    index: HashMap<u64, usize>,
    stats: WalkNetStats,
}

impl SidewalkNet {
    /// Gather the walkable sidewalk curves of `graph` and join their
    /// ends into corners under `policy`. Curves on roads the event
    /// closes are left out.
    pub fn build(graph: &NavGraph, overrides: &NavOverrides, policy: &WalkPolicy) -> Self {
        let mut stats = WalkNetStats::default();
        let mut lanes: Vec<WalkLane> = Vec::new();
        let mut ends: Vec<[f32; 3]> = Vec::new();
        for lane in graph.lanes() {
            if lane.id.kind != LaneKind::Sidewalk {
                continue;
            }
            stats.curves += 1;
            let pts = lane.vertices();
            let finite =
                lane.length.is_finite() && pts.iter().all(|p| p.iter().all(|c| c.is_finite()));
            if !finite
                || lane.length < MIN_WALK_LANE
                || pts.len() < 2
                || overrides.is_closed(lane.id.road)
            {
                stats.excluded += 1;
                continue;
            }
            ends.push(pts[0]);
            ends.push(pts[pts.len() - 1]);
            lanes.push(WalkLane {
                id: lane.id,
                length: lane.length,
                joins: [Vec::new(), Vec::new()],
            });
        }
        stats.walkable = lanes.len();
        stats.ends = ends.len();

        // Bucket the ends on a join-radius grid so the pairing stays
        // near-linear on a 2 000-curve city; BTreeMap keeps it ordered.
        let cell = policy.join_radius.max(0.01);
        let key = |p: [f32; 3]| ((p[0] / cell).floor() as i64, (p[2] / cell).floor() as i64);
        let mut grid: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
        for (i, p) in ends.iter().enumerate() {
            grid.entry(key(*p)).or_default().push(i);
        }
        let r2 = policy.join_radius * policy.join_radius;
        for (i, p) in ends.iter().enumerate() {
            let (cx, cz) = key(*p);
            let mut near: Vec<(f32, usize)> = Vec::new();
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let Some(bucket) = grid.get(&(cx + dx, cz + dz)) else {
                        continue;
                    };
                    for &j in bucket {
                        if j / 2 == i / 2 {
                            continue; // a curve's own other end is not a neighbour
                        }
                        let q = ends[j];
                        let (ex, ey, ez) = (q[0] - p[0], q[1] - p[1], q[2] - p[2]);
                        let d2 = ex * ex + ey * ey + ez * ez;
                        if d2 > r2 || ey.abs() > policy.join_rise {
                            continue;
                        }
                        if crosses_vehicle_lane(graph, *p, q, d2.sqrt()) {
                            stats.severed += 1;
                            continue;
                        }
                        near.push((d2, j));
                    }
                }
            }
            near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            if !near.is_empty() {
                stats.joined_ends += 1;
            }
            lanes[i / 2].joins[i % 2] = near
                .into_iter()
                .map(|(_, j)| (j / 2, if j % 2 == 0 { End::Start } else { End::Finish }))
                .collect();
        }

        let index = lanes
            .iter()
            .enumerate()
            .map(|(i, l)| (l.id.key(), i))
            .collect();
        Self {
            lanes,
            index,
            stats,
        }
    }

    /// Build census.
    pub fn stats(&self) -> WalkNetStats {
        self.stats
    }

    /// Whether `lane` is a curve walkers use.
    pub fn is_walkable(&self, lane: LaneId) -> bool {
        self.index.contains_key(&lane.key())
    }

    /// The walkable curves, in graph order.
    pub fn lane_ids(&self) -> impl Iterator<Item = LaneId> + '_ {
        self.lanes.iter().map(|l| l.id)
    }

    /// Length of a walkable curve (m).
    pub fn length(&self, lane: LaneId) -> Option<f32> {
        self.index.get(&lane.key()).map(|&i| self.lanes[i].length)
    }

    /// The curves a walker at `lane`'s `Start` (`finish == false`) or
    /// `Finish` corner can continue onto, nearest first.
    pub fn corner(&self, lane: LaneId, finish: bool) -> Vec<LaneId> {
        let Some(&i) = self.index.get(&lane.key()) else {
            return Vec::new();
        };
        self.lanes[i].joins[finish as usize]
            .iter()
            .map(|(j, _)| self.lanes[*j].id)
            .collect()
    }

    /// World pose of `walker`: the position on its curve and the
    /// direction it is facing (the curve tangent, flipped for a
    /// backward walker). `None` for a curve the net does not carry.
    pub fn sample(&self, graph: &NavGraph, walker: &Walker) -> Option<LaneSample> {
        let i = *self.index.get(&walker.lane.key())?;
        let mut sample =
            graph.sample_lane(walker.lane, walker.s.clamp(0.0, self.lanes[i].length))?;
        if walker.dir == WalkDir::Backward {
            for c in &mut sample.tangent {
                *c = -*c;
            }
        }
        Some(sample)
    }

    /// Move `walker` `ds` metres along its curve, continuing round kerb
    /// corners and turning at dead ends. `ds` is clamped to
    /// `0..=`[`MAX_ADVANCE`] (a non-finite step moves nothing), and at
    /// most [`MAX_HOPS`] curve ends are met per call, so this always
    /// terminates. A walker whose curve is not in the net is left
    /// untouched. Returns what happened beyond the move.
    pub fn advance(&self, walker: &mut Walker, ds: f32, rng: &mut NavRng) -> WalkStep {
        let mut step = WalkStep::default();
        let Some(mut cur) = self.index.get(&walker.lane.key()).copied() else {
            return step;
        };
        let mut left = if ds.is_finite() {
            ds.clamp(0.0, MAX_ADVANCE)
        } else {
            0.0
        };
        let mut guard = 0;
        loop {
            let len = self.lanes[cur].length;
            walker.s = walker.s.clamp(0.0, len);
            let room = match walker.dir {
                WalkDir::Forward => len - walker.s,
                WalkDir::Backward => walker.s,
            };
            if left <= room {
                walker.s += match walker.dir {
                    WalkDir::Forward => left,
                    WalkDir::Backward => -left,
                };
                walker.s = walker.s.clamp(0.0, len);
                return step;
            }
            left -= room;
            let end = match walker.dir {
                WalkDir::Forward => End::Finish,
                WalkDir::Backward => End::Start,
            };
            walker.s = match end {
                End::Start => 0.0,
                End::Finish => len,
            };
            guard += 1;
            if guard > MAX_HOPS {
                return step;
            }
            let all = &self.lanes[cur].joins[(end == End::Finish) as usize];
            // Prefer a curve other than the one just left, when the
            // corner offers any other.
            let fresh: Vec<EndRef> = all
                .iter()
                .copied()
                .filter(|(j, _)| Some(self.lanes[*j].id) != walker.from)
                .collect();
            let options = if fresh.is_empty() { all } else { &fresh };
            match rng.pick(options).copied() {
                Some((next, next_end)) => {
                    walker.from = Some(self.lanes[cur].id);
                    cur = next;
                    walker.lane = self.lanes[next].id;
                    match next_end {
                        End::Start => {
                            walker.s = 0.0;
                            walker.dir = WalkDir::Forward;
                        }
                        End::Finish => {
                            walker.s = self.lanes[next].length;
                            walker.dir = WalkDir::Backward;
                        }
                    }
                    step.hops += 1;
                }
                None => {
                    walker.dir = walker.dir.flipped();
                    step.turned_around += 1;
                }
            }
        }
    }
}

/// Whether the straight line `a`→`b` (`gap` metres long) crosses a
/// vehicle lane, judged in the ground plane among lanes at the same
/// level (within 1 m of the line's height).
fn crosses_vehicle_lane(graph: &NavGraph, a: [f32; 3], b: [f32; 3], gap: f32) -> bool {
    let mid = [
        (a[0] + b[0]) * 0.5,
        (a[1] + b[1]) * 0.5,
        (a[2] + b[2]) * 0.5,
    ];
    // Any crossing point lies on the segment, so within half the gap of
    // its midpoint.
    let query = LaneQuery::of_kind(LaneKind::Vehicle, gap * 0.5 + 0.05);
    for hit in graph.lane_hits(mid, &query) {
        let Some(lane) = graph.lane(hit.lane) else {
            continue;
        };
        let pts = lane.vertices();
        for w in pts.windows(2) {
            let level = (w[0][1] + w[1][1]) * 0.5 - mid[1];
            if level.abs() <= 1.0 && segments_cross(a, b, w[0], w[1]) {
                return true;
            }
        }
    }
    false
}

/// Whether segments `a`–`b` and `c`–`d` properly cross in the XZ plane.
fn segments_cross(a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) -> bool {
    let orient = |p: [f32; 3], q: [f32; 3], r: [f32; 3]| {
        (q[0] - p[0]) * (r[2] - p[2]) - (q[2] - p[2]) * (r[0] - p[0])
    };
    let (d1, d2) = (orient(a, b, c), orient(a, b, d));
    let (d3, d4) = (orient(c, d, a), orient(c, d, b));
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

/// One planned pedestrian.
#[derive(Debug, Clone, PartialEq)]
pub struct PedSpawn {
    /// Where it stands and which way it walks.
    pub walker: Walker,
    /// World pose at the spawn (position + facing tangent).
    pub sample: LaneSample,
    /// A seeded draw the caller maps onto archetype / paint choices, so
    /// the same seed dresses the same crowd.
    pub variant: u64,
}

/// A seeded initial population.
#[derive(Debug, Clone)]
pub struct PedPlan {
    /// Seed the draw ran under.
    pub seed: u64,
    /// Density fraction applied (0–1, clamped).
    pub density: f32,
    /// Target population — `density × policy.max_active`, rounded.
    pub target: usize,
    /// Sidewalk curves within some interest point's radius.
    pub candidate_curves: usize,
    /// The spawn set — at most `target` entries.
    pub spawns: Vec<PedSpawn>,
    /// Directives dropped after every attempt landed outside the spawn
    /// annulus or too close to a placed walker.
    pub dropped: usize,
    /// The policy the plan was built under.
    pub policy: WalkPolicy,
    /// Non-fatal problems.
    pub issues: Vec<WalkIssue>,
}

/// Target population for `density` under `policy` (non-finite density
/// reads as 0).
pub fn target_population(density: f32, policy: &WalkPolicy) -> usize {
    let density = if density.is_finite() {
        density.clamp(0.0, 1.0)
    } else {
        0.0
    };
    (density * policy.max_active as f32).round() as usize
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (x, y, z) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    x * x + y * y + z * z
}

/// Whether `position` is inside the spawn annulus: within
/// `recycle_distance` of at least one interest point and at least
/// `min_player_distance` from every one. An empty set admits nothing.
pub fn in_walk_band(position: [f32; 3], interest: &[[f32; 3]], policy: &WalkPolicy) -> bool {
    let min2 = policy.min_player_distance * policy.min_player_distance;
    let max2 = policy.recycle_distance * policy.recycle_distance;
    let mut inside = false;
    for p in interest {
        let d2 = dist2(position, *p);
        if d2 < min2 {
            return false;
        }
        inside |= d2 <= max2;
    }
    inside
}

/// Whether `position` is still inside somebody's bubble (within
/// `recycle_distance` of at least one interest point). A walker for
/// whom this is false is due to be recycled.
pub fn within_bubble(position: [f32; 3], interest: &[[f32; 3]], policy: &WalkPolicy) -> bool {
    let max2 = policy.recycle_distance * policy.recycle_distance;
    interest.iter().any(|p| dist2(position, *p) <= max2)
}

/// Indices of the walkable sidewalk curves within `recycle_distance` of
/// some interest point, in index order (so a draw over them is
/// deterministic). Pass the result to [`draw_pedestrian`].
pub fn candidate_curves(
    net: &SidewalkNet,
    graph: &NavGraph,
    interest: &[[f32; 3]],
    policy: &WalkPolicy,
) -> Vec<usize> {
    let mut candidates: BTreeSet<usize> = BTreeSet::new();
    for p in interest {
        let query = LaneQuery::of_kind(LaneKind::Sidewalk, policy.recycle_distance);
        for hit in graph.lane_hits(*p, &query) {
            if let Some(&i) = net.index.get(&hit.lane.key()) {
                candidates.insert(i);
            }
        }
    }
    candidates.into_iter().collect()
}

/// One placement directive: up to `policy.placement_attempts` random
/// points on `candidates` curves, the first of which lands inside the
/// spawn annulus ([`in_walk_band`]) and `policy.spacing` clear of every
/// `occupied` position wins. `None` when none does — the draw is
/// dropped, never forced. Shared by the initial plan and the runtime
/// refill, so both obey the same bounds.
pub fn draw_pedestrian(
    net: &SidewalkNet,
    graph: &NavGraph,
    candidates: &[usize],
    rng: &mut NavRng,
    interest: &[[f32; 3]],
    occupied: &[[f32; 3]],
    policy: &WalkPolicy,
) -> Option<PedSpawn> {
    let spacing2 = policy.spacing * policy.spacing;
    for _ in 0..policy.placement_attempts {
        let &i = rng.pick(candidates)?;
        let lane = &net.lanes[i];
        let s = rng.next_f32() * lane.length;
        let dir = if rng.next_u64() & 1 == 0 {
            WalkDir::Forward
        } else {
            WalkDir::Backward
        };
        let variant = rng.next_u64();
        let walker = Walker {
            lane: lane.id,
            s,
            dir,
            from: None,
        };
        let Some(sample) = net.sample(graph, &walker) else {
            continue;
        };
        if !in_walk_band(sample.position, interest, policy)
            || occupied
                .iter()
                .any(|o| dist2(*o, sample.position) < spacing2)
        {
            continue;
        }
        return Some(PedSpawn {
            walker,
            sample,
            variant,
        });
    }
    None
}

/// Draw the initial pedestrian population around `interest` (each
/// player's position). Candidate curves are the walkable sidewalks
/// within `recycle_distance` of an interest point; each directive
/// retries up to `placement_attempts` times to land on a point inside
/// the spawn annulus and `spacing` clear of earlier walkers, and is
/// dropped (counted, never forced) when none does. Deterministic in
/// `(seed, density, interest, policy)`.
pub fn plan_pedestrians(
    net: &SidewalkNet,
    graph: &NavGraph,
    seed: u64,
    density: f32,
    interest: &[[f32; 3]],
    policy: &WalkPolicy,
) -> PedPlan {
    let density = if density.is_finite() {
        density.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let target = target_population(density, policy);
    let mut issues = Vec::new();
    if net.lanes.is_empty() {
        issues.push(WalkIssue::NoSidewalks);
    }
    if interest.is_empty() {
        issues.push(WalkIssue::NoInterest);
    }

    let candidates = candidate_curves(net, graph, interest, policy);

    let mut rng = NavRng::new(seed);
    let mut spawns: Vec<PedSpawn> = Vec::new();
    let mut dropped = 0usize;
    if !candidates.is_empty() && !interest.is_empty() {
        for _ in 0..target {
            let occupied: Vec<[f32; 3]> = spawns.iter().map(|s| s.sample.position).collect();
            match draw_pedestrian(
                net,
                graph,
                &candidates,
                &mut rng,
                interest,
                &occupied,
                policy,
            ) {
                Some(spawn) => spawns.push(spawn),
                None => dropped += 1,
            }
        }
    }

    PedPlan {
        seed,
        density,
        target,
        candidate_curves: candidates.len(),
        spawns,
        dropped,
        policy: policy.clone(),
        issues,
    }
}
