//! Directed navigation over a city's BAI road network.
//!
//! [`NavGraph`] turns a parsed [`Bai`] into an immutable query
//! structure shared by consumers with different goals — ambient
//! traffic, race opponents, police and pedestrians. The graph answers
//! *where* an actor may legally travel: lane curves, turn connections
//! and reachable routes. How the actor drives there stays with the
//! consumer, so two consumers can hold independent route state over
//! the same graph.
//!
//! Direction convention: lane curves are stored in road section order
//! (start → end). Right-side curves carry traffic `Forward` (with the
//! sections) and left-side curves carry it `Backward` — the London
//! left-hand-driving swap is baked into the authored lane data by the
//! original toolchain, so the graph needs no per-city handedness flag
//! (see `docs/research/bai.md` and the original-rules ledger).
//!
//! Turn legality follows the documented ambient rule: on a two-way
//! road the innermost lane may turn toward the road centre or go
//! straight, the outermost lane may turn kerb-side or go straight and
//! middle lanes must go straight; one-way roads may take any exit;
//! U-turns are never legal. Turn *classification* is geometric —
//! [`ArcExit::heading_change`] — because the original's counter-
//! clockwise index arithmetic is only documented for a 4-way. When no
//! exit satisfies the lane's set (e.g. a middle lane reaching a
//! T-junction) the straightest exit is returned as a documented
//! fallback rather than inventing connectivity.

use mm2_formats::aimap::Aimap;
use mm2_formats::bai::{AmbientType, Bai, End, Side, VehicleRule};
use std::collections::{BTreeSet, BinaryHeap, HashMap};
use std::fmt;

/// XZ bucket size of the `nearest_lane` candidate grid, in metres.
const GRID_CELL: f32 = 32.0;
/// Half-angle of the cone classifying an exit as [`TurnKind::Straight`].
const STRAIGHT_CONE: f32 = std::f32::consts::FRAC_PI_4;

/// Direction an [`NavArc`] travels along its road, relative to the
/// authored section order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TravelDir {
    /// Start → end, with the stored vertex order.
    Forward,
    /// End → start, against the stored vertex order.
    Backward,
}

/// What an authored curve carries. Vehicle lanes are the only routable
/// kind; the rest remain queryable and sampleable for their own
/// consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LaneKind {
    /// Ambient-vehicle lane.
    Vehicle,
    /// Pedestrian path down a sidewalk.
    Sidewalk,
    /// Tram rail.
    Tram,
    /// Train rail.
    Train,
}

/// Stable identity of one lane curve within a graph built from one
/// file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneId {
    /// BAI road index.
    pub road: u16,
    /// Authored side holding the curve.
    pub side: Side,
    /// Index among curves of the same kind on that side.
    pub index: u16,
    /// Curve kind.
    pub kind: LaneKind,
}

impl LaneId {
    fn key(&self) -> u64 {
        let side = match self.side {
            Side::Right => 0u64,
            Side::Left => 1,
        };
        let kind = match self.kind {
            LaneKind::Vehicle => 0u64,
            LaneKind::Sidewalk => 1,
            LaneKind::Tram => 2,
            LaneKind::Train => 3,
        };
        (self.road as u64) << 20 | side << 19 | kind << 16 | self.index as u64
    }
}

/// Identity of one directed travel arc — a road traversed in one
/// direction. Indexes the graph's arc table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArcId(pub u32);

/// What lies at an extremity of a travel arc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArcEnd {
    /// A wired intersection (index into [`NavGraph::intersections`]).
    Intersection(u16),
    /// No usable connection: authored stub, cul-de-sac or an end whose
    /// back-reference could not be resolved.
    DeadEnd,
}

/// A turn off one arc onto another through an intersection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArcExit {
    /// The arc entered by taking this turn.
    pub to: ArcId,
    /// Intersection the turn passes through.
    pub intersection: u16,
    /// Geometric turn classification.
    pub turn: TurnKind,
    /// Signed heading change in radians on the ground plane:
    /// positive turns toward the driver's left (`up × forward`).
    pub heading_change: f32,
    /// Difference between the two ends' positions in the
    /// intersection's counterclockwise road list — the original's
    /// documented turn-selection mechanism, carried for consumers and
    /// research.
    pub ccw_delta: u8,
}

/// Geometric classification of a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnKind {
    /// More than 45° toward the driver's left.
    Left,
    /// Within ±45° of the entry heading.
    Straight,
    /// More than 45° toward the driver's right.
    Right,
}

/// Turn kinds a lane position permits, as a small bit set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TurnSet(u8);

impl TurnSet {
    const LEFT: Self = Self(1);
    const STRAIGHT: Self = Self(2);
    const RIGHT: Self = Self(4);
    const ANY: Self = Self(7);

    fn contains(self, kind: TurnKind) -> bool {
        let bit = match kind {
            TurnKind::Left => Self::LEFT,
            TurnKind::Straight => Self::STRAIGHT,
            TurnKind::Right => Self::RIGHT,
        };
        self.0 & bit.0 != 0
    }
}

/// One lane curve, in authored (section) order.
#[derive(Debug, Clone)]
pub struct NavLane {
    /// Curve identity.
    pub id: LaneId,
    /// Arc this lane carries, when it is a routable vehicle lane.
    pub arc: Option<ArcId>,
    /// Signed lateral offset of the curve from the road centre line in
    /// metres, computed per-section against the authored frame: positive
    /// is `+x_axis` (the driver's right when travelling [`TravelDir::Forward`]).
    pub lateral_offset: f32,
    /// Authored outer-edge distance of the curve from the road centre.
    /// `None` for rail curves, which carry no authored edge distance.
    /// The field's exact meaning is unverified — lane ranking uses
    /// [`NavLane::lateral_offset`], which is measured, instead.
    pub edge_distance: Option<f32>,
    /// Curve length in metres.
    pub length: f32,
    /// Curve vertices in storage order.
    points: Vec<[f32; 3]>,
    /// Cumulative distance at each vertex in storage order.
    cum: Vec<f32>,
}

impl NavLane {
    /// Curve vertices in authored section order.
    pub fn vertices(&self) -> &[[f32; 3]] {
        &self.points
    }

    /// Cumulative distance at each vertex in authored section order.
    pub fn distances(&self) -> &[f32] {
        &self.cum
    }
}

/// A road as the graph sees it.
#[derive(Debug, Clone)]
pub struct NavRoad {
    /// BAI road index (equal to the authored id on retail data).
    pub id: u16,
    /// Raw flag bits (`FLAG_*` in `mm2_formats::bai`).
    pub flags: u16,
    /// PSDL rooms the road passes through (index + 1, as authored).
    pub rooms: Vec<u16>,
    /// Authored half width.
    pub half_width: f32,
    /// Authored base speed (aimap files may override it).
    pub base_speed: f32,
    /// Travel arc per direction, when that direction carries vehicles.
    /// Index 0 = [`TravelDir::Forward`], 1 = [`TravelDir::Backward`].
    pub arcs: [Option<ArcId>; 2],
    /// Only one direction carries vehicles.
    pub one_way: bool,
}

/// One directed travel arc.
#[derive(Debug, Clone)]
pub struct NavArc {
    /// Road travelled.
    pub road: u16,
    /// Direction relative to authored section order.
    pub dir: TravelDir,
    /// Authored side whose curves carry this arc.
    pub side: Side,
    /// Vehicle lanes in inner→outer order from the road centre.
    pub lanes: Vec<LaneId>,
    /// Centre-line length in metres.
    pub length: f32,
    /// Junction at the upstream extremity.
    pub entry: ArcEnd,
    /// Junction at the downstream extremity.
    pub exit: ArcEnd,
    /// Raw `vehicleRule` code at the downstream end.
    pub exit_rule_code: u16,
    /// Raw `vehicleRule` code at the upstream end.
    pub entry_rule_code: u16,
    /// World position of the upstream extremity.
    pub entry_point: [f32; 3],
    /// World position of the downstream extremity.
    pub exit_point: [f32; 3],
}

impl NavArc {
    /// Interpreted `vehicleRule` at the downstream end.
    pub fn exit_rule(&self) -> Option<VehicleRule> {
        decode_vehicle_rule(self.exit_rule_code)
    }

    /// Interpreted `vehicleRule` at the upstream end.
    pub fn entry_rule(&self) -> Option<VehicleRule> {
        decode_vehicle_rule(self.entry_rule_code)
    }
}

fn decode_vehicle_rule(code: u16) -> Option<VehicleRule> {
    match code {
        0 => Some(VehicleRule::StopSign),
        1 => Some(VehicleRule::TrafficLight),
        2 => Some(VehicleRule::AlwaysStop),
        3 => Some(VehicleRule::NeverStop),
        _ => None,
    }
}

/// An intersection, mirroring the authored record.
#[derive(Debug, Clone)]
pub struct NavIntersection {
    /// BAI intersection index.
    pub id: u16,
    /// Hosting PSDL room (index + 1).
    pub room: u16,
    /// Centre point.
    pub center: [f32; 3],
    /// Connected roads in authored counterclockwise order.
    pub roads: Vec<u32>,
}

/// A problem found while building the graph. The graph stays usable:
/// bad ends degrade to dead ends and bad lanes drop out of their arc —
/// nothing is invented to repair them.
#[derive(Debug, Clone, PartialEq)]
pub enum NavIssue {
    /// A road end claims a connection that does not resolve (dangling
    /// intersection reference, out-of-range road index, or a missing
    /// back-reference). The end is treated as a dead end.
    UnresolvedEnd {
        /// BAI road index.
        road: usize,
        /// Which extremity.
        end: End,
    },
    /// A curve has fewer than two vertices or no positive length.
    /// Vehicle curves drop out of their arc; other kinds stay
    /// queryable but unsampleable.
    DegenerateLane {
        /// BAI road index.
        road: usize,
        /// Authored side.
        side: Side,
        /// Curve kind.
        kind: LaneKind,
        /// Index among curves of the same kind on that side.
        index: usize,
    },
    /// Authored per-vertex distances were absent or not monotone;
    /// cumulative distances were recomputed from vertex positions.
    LaneDistancesRecomputed {
        /// BAI road index.
        road: usize,
        /// Authored side.
        side: Side,
        /// Curve kind.
        kind: LaneKind,
        /// Index among curves of the same kind on that side.
        index: usize,
    },
    /// Neither side of a road carries routable vehicle lanes.
    NoVehicleLanes {
        /// BAI road index.
        road: usize,
    },
}

impl fmt::Display for NavIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NavIssue::UnresolvedEnd { road, end } => {
                write!(
                    f,
                    "road {road} {end}: connection does not resolve (dead end)"
                )
            }
            NavIssue::DegenerateLane {
                road,
                side,
                kind,
                index,
            } => write!(
                f,
                "road {road} {side} {kind:?} lane {index}: degenerate curve"
            ),
            NavIssue::LaneDistancesRecomputed {
                road,
                side,
                kind,
                index,
            } => write!(
                f,
                "road {road} {side} {kind:?} lane {index}: distances recomputed from vertices"
            ),
            NavIssue::NoVehicleLanes { road } => {
                write!(f, "road {road}: no routable vehicle lanes")
            }
        }
    }
}

/// Result of [`NavGraph::build`].
#[derive(Debug)]
pub struct NavBuild {
    /// The built graph.
    pub graph: NavGraph,
    /// Structural problems found along the way.
    pub issues: Vec<NavIssue>,
}

/// Counts describing a built graph.
#[derive(Debug, Clone, Default)]
pub struct NavStats {
    /// Roads in the source file.
    pub roads: usize,
    /// Directed vehicle arcs.
    pub vehicle_arcs: usize,
    /// Roads routable in only one direction.
    pub one_way_roads: usize,
    /// Routable vehicle lanes.
    pub vehicle_lanes: usize,
    /// Sidewalk curves.
    pub sidewalk_lanes: usize,
    /// Tram rail curves.
    pub tram_lanes: usize,
    /// Train rail curves.
    pub train_lanes: usize,
    /// Intersections.
    pub intersections: usize,
    /// Arc extremities with no usable intersection connection.
    pub dead_ends: usize,
    /// Weakly connected road components — maximal sets of roads joined
    /// through shared intersections, ignoring direction.
    pub components: usize,
}

/// A point sampled along a lane curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaneSample {
    /// World position.
    pub position: [f32; 3],
    /// Unit tangent. For vehicle lanes this points along travel; for
    /// sidewalks and rails it points along storage order.
    pub tangent: [f32; 3],
}

/// Filter for [`NavGraph::nearest_lane`].
#[derive(Debug, Clone)]
pub struct LaneQuery {
    /// Restrict to one curve kind (`None` = any kind).
    pub kind: Option<LaneKind>,
    /// Maximum 3D snap distance in metres.
    pub max_distance: f32,
    /// Restrict to lanes on roads touching any of these PSDL rooms
    /// (index + 1, as authored). `None` accepts any room — use a room
    /// hint to keep stacked bridges and tunnels apart when proximity
    /// alone would be ambiguous.
    pub rooms: Option<BTreeSet<u16>>,
    /// Only lanes that belong to a routable vehicle arc.
    pub routable_only: bool,
}

impl LaneQuery {
    /// Any curve kind within `max_distance`.
    pub fn any(max_distance: f32) -> Self {
        Self {
            kind: None,
            max_distance,
            rooms: None,
            routable_only: false,
        }
    }

    /// Routable vehicle lanes within `max_distance`.
    pub fn vehicles(max_distance: f32) -> Self {
        Self {
            kind: Some(LaneKind::Vehicle),
            max_distance,
            rooms: None,
            routable_only: true,
        }
    }

    /// One curve kind within `max_distance`.
    pub fn of_kind(kind: LaneKind, max_distance: f32) -> Self {
        Self {
            kind: Some(kind),
            max_distance,
            rooms: None,
            routable_only: false,
        }
    }

    /// Restrict to roads touching at least one of `rooms`.
    pub fn in_rooms(mut self, rooms: impl IntoIterator<Item = u16>) -> Self {
        self.rooms = Some(rooms.into_iter().collect());
        self
    }
}

/// Result of a [`NavGraph::nearest_lane`] query.
#[derive(Debug, Clone, PartialEq)]
pub struct LaneHit {
    /// The closest eligible lane.
    pub lane: LaneId,
    /// 3D distance from the query point to the curve.
    pub distance: f32,
    /// Closest point on the curve.
    pub point: [f32; 3],
    /// Curve tangent at `point` in storage order.
    pub tangent: [f32; 3],
    /// Distance along the curve in storage order.
    pub along: f32,
}

/// Ambient-navigation overrides distilled from a parsed `.aimap`
/// file — the contract the content producer fills for routing and
/// diagnostics consumers. Interpretation (`docs/research/aimap.md`):
///
/// - `[Exceptions]` rows carry `density 0.00` on every retail row:
///   a zero density is treated as "ambient traffic forbidden here",
///   closing the road to ambient routing (inferred — the original's
///   runtime semantics are unverified). Rows with a nonzero density
///   are kept for consumers but do not close the road.
/// - `[Speed Limit]` overrides [`NavRoad::base_speed`] on every road;
///   a positive per-road exception speed beats the file default.
///   Units are the BAI base speed's (unverified).
/// - `[Ambients Drive On The Left]` is informational only: authored
///   lane direction already encodes handedness, so the flag never
///   changes the graph.
#[derive(Debug, Clone, Default)]
pub struct NavOverrides {
    /// BAI road ids closed to ambient traffic (zero-density
    /// `[Exceptions]` rows). Ids outside the city's road space — seen
    /// on several retail London race files — are kept verbatim: they
    /// simply never match an arc.
    pub closed_roads: BTreeSet<u16>,
    /// `[Speed Limit]` file default; overrides `base_speed` when set.
    pub default_speed_limit: Option<f32>,
    /// Raw `[Exceptions]` rows, for consumers that need density or
    /// per-road speed detail.
    pub exceptions: Vec<mm2_formats::aimap::RoadException>,
    /// `[Ambients Drive On The Left]` raw flag (0/1 on retail).
    pub drive_on_left: Option<i64>,
}

impl NavOverrides {
    /// Distill a parsed aimap into navigation overrides.
    pub fn from_aimap(aimap: &Aimap) -> Self {
        Self {
            closed_roads: aimap
                .exceptions
                .iter()
                .filter(|e| e.density <= 0.0)
                .filter_map(|e| u16::try_from(e.road).ok())
                .collect(),
            default_speed_limit: aimap.speed_limit,
            exceptions: aimap.exceptions.clone(),
            drive_on_left: aimap.drive_on_left,
        }
    }

    /// Whether `road` (BAI id, equal to the road index on retail) is
    /// closed to ambient traffic.
    pub fn is_closed(&self, road: u16) -> bool {
        self.closed_roads.contains(&road)
    }

    /// Speed limit for `road`: a positive per-road exception wins,
    /// then the file default. `None` leaves the authored base speed.
    pub fn speed_limit(&self, road: u16) -> Option<f32> {
        self.exceptions
            .iter()
            .find(|e| e.road == road as u32 && e.speed_limit > 0.0)
            .map(|e| e.speed_limit)
            .or(self.default_speed_limit)
    }

    /// The effective limit for a graph road — the aimap value or the
    /// authored `base_speed`.
    pub fn effective_speed(&self, road: &NavRoad) -> f32 {
        self.speed_limit(road.id).unwrap_or(road.base_speed)
    }

    /// Route options honouring the closures: a closed road is never
    /// entered through a turn (start/goal endpoints may still snap
    /// onto one, matching [`RouteOptions::closed_roads`] semantics).
    pub fn route_options(&self) -> RouteOptions {
        RouteOptions {
            closed_roads: self.closed_roads.clone(),
            ..RouteOptions::default()
        }
    }
}

/// Options for [`NavGraph::route`].
#[derive(Debug, Clone)]
pub struct RouteOptions {
    /// Snap radius for the start and goal points, in metres.
    pub max_snap: f32,
    /// Bound on A* expansions; exceeding it fails with
    /// [`RouteError::ExpansionLimit`] instead of searching forever.
    pub max_expansions: usize,
    /// Road indices closed to routing — e.g. an event aimap's
    /// `[Exceptions]` list. Closed roads are never entered through a
    /// turn; a start or goal snapped onto one is still honoured.
    pub closed_roads: BTreeSet<u16>,
}

impl Default for RouteOptions {
    fn default() -> Self {
        Self {
            max_snap: 64.0,
            max_expansions: 4096,
            closed_roads: BTreeSet::new(),
        }
    }
}

/// Why a route query failed. Every failure is a specific variant —
/// routing never hangs and never guesses at missing connectivity.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteError {
    /// No routable lane within `max_snap` of the start point.
    NoStartLane,
    /// No routable lane within `max_snap` of the goal point.
    NoGoalLane,
    /// The goal arc is not reachable from the start arc: the search
    /// frontier was exhausted.
    Unreachable {
        /// Arcs expanded before the frontier emptied.
        expanded: usize,
    },
    /// The expansion bound was hit before reaching the goal — the goal
    /// may or may not be reachable.
    ExpansionLimit {
        /// Arcs expanded when the bound was hit.
        expanded: usize,
    },
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RouteError::NoStartLane => write!(f, "no routable lane near the start point"),
            RouteError::NoGoalLane => write!(f, "no routable lane near the goal point"),
            RouteError::Unreachable { expanded } => {
                write!(f, "goal unreachable ({expanded} arcs expanded)")
            }
            RouteError::ExpansionLimit { expanded } => {
                write!(f, "route search hit the expansion bound ({expanded})")
            }
        }
    }
}

impl std::error::Error for RouteError {}

/// A found route: the snapped endpoints plus the arc sequence to
/// travel.
#[derive(Debug, Clone)]
pub struct Route {
    /// Snap result for the query's start point.
    pub start: LaneHit,
    /// Snap result for the query's goal point.
    pub goal: LaneHit,
    /// Arcs to travel, start arc first and goal arc last.
    pub steps: Vec<ArcId>,
    /// Sum of the step arcs' centre-line lengths — an approximation
    /// that ignores the partial first and last arcs.
    pub length: f32,
}

/// One consumer's progress along a [`Route`]. Cursors are owned
/// per-consumer; the shared graph is immutable, so two consumers
/// cannot disturb each other's route state.
#[derive(Debug, Clone)]
pub struct RouteCursor {
    /// Index into `route.steps` of the arc being travelled.
    pub step: usize,
    /// Lane currently occupied.
    pub lane: LaneId,
    /// Metres along `lane` in travel direction.
    pub distance: f32,
}

/// Deterministic generator for seeded route/exit choice — an xorshift,
/// not a cryptographic RNG. The same seed always produces the same
/// sequence, on every platform.
#[derive(Debug, Clone)]
pub struct NavRng(u64);

impl NavRng {
    /// Seed the generator. Every `u64` is a usable seed.
    pub fn new(seed: u64) -> Self {
        // SplitMix64 finalizer so seed 0 (and clustered seeds) still
        // produce a well-mixed state.
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        Self(z ^ (z >> 31))
    }

    /// Next raw value.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform-ish pick from a slice (`None` when empty).
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            Some(&items[(self.next_u64() % items.len() as u64) as usize])
        }
    }
}

/// Uniform XZ bucket grid accelerating [`NavGraph::nearest_lane`].
#[derive(Debug, Default)]
struct LaneGrid {
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl LaneGrid {
    fn insert(&mut self, lane_idx: u32, aabb_min: [f32; 3], aabb_max: [f32; 3]) {
        let x0 = (aabb_min[0] / GRID_CELL).floor() as i32;
        let x1 = (aabb_max[0] / GRID_CELL).floor() as i32;
        let z0 = (aabb_min[2] / GRID_CELL).floor() as i32;
        let z1 = (aabb_max[2] / GRID_CELL).floor() as i32;
        for x in x0..=x1 {
            for z in z0..=z1 {
                self.cells.entry((x, z)).or_default().push(lane_idx);
            }
        }
    }

    fn candidates(&self, point: [f32; 3], radius: f32) -> Vec<u32> {
        let mut out = BTreeSet::new();
        let x0 = ((point[0] - radius) / GRID_CELL).floor() as i32;
        let x1 = ((point[0] + radius) / GRID_CELL).floor() as i32;
        let z0 = ((point[2] - radius) / GRID_CELL).floor() as i32;
        let z1 = ((point[2] + radius) / GRID_CELL).floor() as i32;
        for x in x0..=x1 {
            for z in z0..=z1 {
                if let Some(lanes) = self.cells.get(&(x, z)) {
                    out.extend(lanes.iter().copied());
                }
            }
        }
        out.into_iter().collect()
    }
}

/// Immutable navigation graph over one city's BAI data.
#[derive(Debug)]
pub struct NavGraph {
    roads: Vec<NavRoad>,
    arcs: Vec<NavArc>,
    lanes: Vec<NavLane>,
    lane_lookup: HashMap<u64, u32>,
    exits: Vec<Vec<ArcExit>>,
    intersections: Vec<NavIntersection>,
    grid: LaneGrid,
    stats: NavStats,
}

impl NavGraph {
    /// Build the graph from a parsed BAI. Structural problems are
    /// reported in [`NavBuild::issues`]; the graph itself is always
    /// produced, degrading bad ends to dead ends and bad lanes out of
    /// their arcs rather than inventing connectivity.
    pub fn build(bai: &Bai) -> NavBuild {
        let mut issues = Vec::new();

        // Resolve every road end first: an end is an intersection
        // connection only when the whole back-reference chain checks
        // out — otherwise it degrades to a dead end with an issue.
        let mut resolved: Vec<(Option<u16>, Option<u16>)> = Vec::with_capacity(bai.roads.len());
        for (ri, road) in bai.roads.iter().enumerate() {
            let mut pair = (None, None);
            for (slot, end, e) in [(0, End::Start, &road.start), (1, End::End, &road.end)] {
                if !e.is_connected() {
                    continue;
                }
                let ok = bai
                    .intersections
                    .get(e.intersection as usize)
                    .and_then(|int| int.roads.get(e.intersection_road_index as usize))
                    .is_some_and(|&back| back as usize == ri);
                if ok {
                    if slot == 0 {
                        pair.0 = Some(e.intersection as u16);
                    } else {
                        pair.1 = Some(e.intersection as u16);
                    }
                } else {
                    issues.push(NavIssue::UnresolvedEnd { road: ri, end });
                }
            }
            resolved.push(pair);
        }

        let mut roads = Vec::with_capacity(bai.roads.len());
        let mut arcs: Vec<NavArc> = Vec::new();
        let mut lanes: Vec<NavLane> = Vec::new();
        let mut lane_lookup = HashMap::new();
        let mut stats = NavStats {
            roads: bai.roads.len(),
            intersections: bai.intersections.len(),
            ..NavStats::default()
        };

        for (ri, road) in bai.roads.iter().enumerate() {
            let centre_len = road.sections.last().map(|s| s.distance).unwrap_or_default();
            let (start_int, end_int) = resolved[ri];
            let first = road.sections.first();
            let last = road.sections.last();

            let mut road_arcs = [None, None];
            // Right-side curves travel with the sections; left-side
            // curves travel against them. London's left-hand driving is
            // baked into the authored data, not a runtime flag.
            for (side, bside, dir) in [
                (Side::Right, &road.right, TravelDir::Forward),
                (Side::Left, &road.left, TravelDir::Backward),
            ] {
                // Build every curve on this side, arc or not.
                let mut vehicle_lanes: Vec<LaneId> = Vec::new();
                for i in 0..bside.lane_count as usize {
                    let id = LaneId {
                        road: ri as u16,
                        side,
                        index: i as u16,
                        kind: LaneKind::Vehicle,
                    };
                    let points = bside.lane_vertices.get(i).cloned().unwrap_or_default();
                    let authored = bside.lane_distances.get(i).map(Vec::as_slice);
                    let edge = bside.edge_distances.get(i).copied();
                    let offset = lateral_offset(&points, road);
                    match push_lane(
                        &mut lanes,
                        &mut lane_lookup,
                        id,
                        points,
                        authored,
                        edge,
                        offset,
                        ri,
                        &mut issues,
                    ) {
                        Some(_) => vehicle_lanes.push(id),
                        None => continue,
                    }
                }
                for i in 0..bside.sidewalk_count as usize {
                    let curve = bside.lane_count as usize + i;
                    let id = LaneId {
                        road: ri as u16,
                        side,
                        index: i as u16,
                        kind: LaneKind::Sidewalk,
                    };
                    let points = bside.lane_vertices.get(curve).cloned().unwrap_or_default();
                    let authored = bside.lane_distances.get(curve).map(Vec::as_slice);
                    let edge = bside.edge_distances.get(curve).copied();
                    let offset = lateral_offset(&points, road);
                    if push_lane(
                        &mut lanes,
                        &mut lane_lookup,
                        id,
                        points,
                        authored,
                        edge,
                        offset,
                        ri,
                        &mut issues,
                    )
                    .is_some()
                    {
                        stats.sidewalk_lanes += 1;
                    }
                }
                for (kind, curves) in [
                    (LaneKind::Tram, &bside.tram_vertices),
                    (LaneKind::Train, &bside.train_vertices),
                ] {
                    for (i, points) in curves.iter().enumerate() {
                        let id = LaneId {
                            road: ri as u16,
                            side,
                            index: i as u16,
                            kind,
                        };
                        let offset = lateral_offset(points, road);
                        if push_lane(
                            &mut lanes,
                            &mut lane_lookup,
                            id,
                            points.clone(),
                            None,
                            None,
                            offset,
                            ri,
                            &mut issues,
                        )
                        .is_some()
                        {
                            match kind {
                                LaneKind::Tram => stats.tram_lanes += 1,
                                LaneKind::Train => stats.train_lanes += 1,
                                _ => {}
                            }
                        }
                    }
                }

                // Vehicles are routable only where the authored ambient
                // types allow them; an undocumented code is left
                // routable — `Bai::validate` already reports it.
                let vehicles_allowed = !matches!(
                    bside.ambient_type(),
                    Some(AmbientType::PedestriansOnly) | Some(AmbientType::Disabled)
                );
                if vehicles_allowed && !vehicle_lanes.is_empty() {
                    // Inner→outer order by measured lateral offset:
                    // right side ascending (+x is driver's right), left
                    // side descending (its lanes sit at −x).
                    vehicle_lanes.sort_by(|a, b| {
                        let oa = lane_offset(&lanes, &lane_lookup, *a);
                        let ob = lane_offset(&lanes, &lane_lookup, *b);
                        let ord = oa.partial_cmp(&ob).unwrap_or(std::cmp::Ordering::Equal);
                        match side {
                            Side::Right => ord,
                            Side::Left => ord.reverse(),
                        }
                    });
                    let arc = ArcId(arcs.len() as u32);
                    for id in &vehicle_lanes {
                        if let Some(&idx) = lane_lookup.get(&id.key()) {
                            lanes[idx as usize].arc = Some(arc);
                        }
                    }
                    stats.vehicle_lanes += vehicle_lanes.len();
                    let (entry, exit, entry_rule_code, exit_rule_code) = match dir {
                        TravelDir::Forward => (
                            start_int,
                            end_int,
                            road.start.vehicle_rule_code,
                            road.end.vehicle_rule_code,
                        ),
                        TravelDir::Backward => (
                            end_int,
                            start_int,
                            road.end.vehicle_rule_code,
                            road.start.vehicle_rule_code,
                        ),
                    };
                    let (entry_point, exit_point) = match dir {
                        TravelDir::Forward => (
                            first.map(|s| s.origin).unwrap_or_default(),
                            last.map(|s| s.origin).unwrap_or_default(),
                        ),
                        TravelDir::Backward => (
                            last.map(|s| s.origin).unwrap_or_default(),
                            first.map(|s| s.origin).unwrap_or_default(),
                        ),
                    };
                    if exit.is_none() {
                        stats.dead_ends += 1;
                    }
                    arcs.push(NavArc {
                        road: ri as u16,
                        dir,
                        side,
                        lanes: vehicle_lanes,
                        length: centre_len,
                        entry: entry.map_or(ArcEnd::DeadEnd, ArcEnd::Intersection),
                        exit: exit.map_or(ArcEnd::DeadEnd, ArcEnd::Intersection),
                        exit_rule_code,
                        entry_rule_code,
                        entry_point,
                        exit_point,
                    });
                    road_arcs[dir_index(dir)] = Some(arc);
                    stats.vehicle_arcs += 1;
                }
            }

            let one_way = road_arcs[0].is_some() != road_arcs[1].is_some();
            if one_way {
                stats.one_way_roads += 1;
            }
            if road_arcs[0].is_none() && road_arcs[1].is_none() {
                issues.push(NavIssue::NoVehicleLanes { road: ri });
            }
            roads.push(NavRoad {
                id: road.id,
                flags: road.flags,
                rooms: road.rooms.clone(),
                half_width: road.half_width,
                base_speed: road.base_speed,
                arcs: road_arcs,
                one_way,
            });
        }

        // Turn connections: an arc's downstream end decides which arcs
        // it may enter next. U-turns back onto the same road are never
        // legal (documented).
        let mut exits: Vec<Vec<ArcExit>> = vec![Vec::new(); arcs.len()];
        for (ai, arc) in arcs.iter().enumerate() {
            let ArcEnd::Intersection(int_idx) = arc.exit else {
                continue;
            };
            let int = &bai.intersections[int_idx as usize];
            let road = &bai.roads[arc.road as usize];
            let from_end = match arc.dir {
                TravelDir::Forward => &road.end,
                TravelDir::Backward => &road.start,
            };
            let entry_heading = travel_tangent(road, arc.dir, true);
            let n = int.roads.len().max(1) as u32;
            let mut seen = BTreeSet::new();
            for &s_idx in &int.roads {
                if s_idx as usize == arc.road as usize || !seen.insert(s_idx) {
                    continue; // no U-turns; each road once
                }
                let Some(s) = bai.roads.get(s_idx as usize) else {
                    continue;
                };
                for (end, dep_dir) in [
                    (&s.start, TravelDir::Forward),
                    (&s.end, TravelDir::Backward),
                ] {
                    if !end.is_connected() || end.intersection as usize != int_idx as usize {
                        continue;
                    }
                    let Some(to) = roads[s_idx as usize].arcs[dir_index(dep_dir)] else {
                        continue;
                    };
                    let exit_heading = travel_tangent(s, dep_dir, false);
                    let change = signed_xz_angle(entry_heading, exit_heading);
                    exits[ai].push(ArcExit {
                        to,
                        intersection: int_idx,
                        turn: classify_turn(change),
                        heading_change: change,
                        ccw_delta: (end
                            .intersection_road_index
                            .wrapping_sub(from_end.intersection_road_index)
                            % n) as u8,
                    });
                }
            }
        }

        // Weakly connected components over roads sharing intersections.
        let mut parent: Vec<usize> = (0..bai.roads.len()).collect();
        for int in &bai.intersections {
            for w in int.roads.windows(2) {
                union(&mut parent, w[0] as usize, w[1] as usize);
            }
        }
        stats.components = (0..bai.roads.len())
            .filter(|&i| find(&mut parent, i) == i)
            .count();

        // nearest_lane grid.
        let mut grid = LaneGrid::default();
        for (i, lane) in lanes.iter().enumerate() {
            if lane.points.is_empty() {
                continue;
            }
            let mut lo = lane.points[0];
            let mut hi = lane.points[0];
            for p in &lane.points {
                for a in 0..3 {
                    lo[a] = lo[a].min(p[a]);
                    hi[a] = hi[a].max(p[a]);
                }
            }
            grid.insert(i as u32, lo, hi);
        }

        let intersections = bai
            .intersections
            .iter()
            .map(|i| NavIntersection {
                id: i.id,
                room: i.room,
                center: i.center,
                roads: i.roads.clone(),
            })
            .collect();

        NavBuild {
            graph: NavGraph {
                roads,
                arcs,
                lanes,
                lane_lookup,
                exits,
                intersections,
                grid,
                stats,
            },
            issues,
        }
    }

    /// Build counts.
    pub fn stats(&self) -> &NavStats {
        &self.stats
    }

    /// Every road.
    pub fn roads(&self) -> &[NavRoad] {
        &self.roads
    }

    /// Every intersection.
    pub fn intersections(&self) -> &[NavIntersection] {
        &self.intersections
    }

    /// Every lane curve.
    pub fn lanes(&self) -> &[NavLane] {
        &self.lanes
    }

    /// Look up a road by BAI index.
    pub fn road(&self, index: u16) -> Option<&NavRoad> {
        self.roads.get(index as usize)
    }

    /// Look up an arc.
    pub fn arc(&self, id: ArcId) -> &NavArc {
        &self.arcs[id.0 as usize]
    }

    /// The arc for `road` travelled in `dir`, when routable.
    pub fn arc_of(&self, road: u16, dir: TravelDir) -> Option<ArcId> {
        self.roads.get(road as usize)?.arcs[dir_index(dir)]
    }

    /// Look up a lane.
    pub fn lane(&self, id: LaneId) -> Option<&NavLane> {
        self.lanes.get(*self.lane_lookup.get(&id.key())? as usize)
    }

    /// Every arc reachable after travelling `arc` to its downstream
    /// end — connectivity only, no lane-position rules. U-turns are
    /// already excluded.
    pub fn exits(&self, arc: ArcId) -> &[ArcExit] {
        &self.exits[arc.0 as usize]
    }

    /// Exits legal for a vehicle travelling `lane` to its end, applying
    /// the documented lane-position rules. When the rules permit
    /// nothing (e.g. a middle lane at a T-junction) the straightest
    /// exit is returned — an implementation choice, documented in the
    /// module docs.
    pub fn legal_exits(&self, lane: LaneId) -> Vec<ArcExit> {
        let Some(l) = self.lane(lane) else {
            return Vec::new();
        };
        let Some(arc_id) = l.arc else {
            return Vec::new();
        };
        let arc = self.arc(arc_id);
        let exits = self.exits(arc_id);
        if exits.is_empty() {
            return Vec::new();
        }
        // One-way roads take any exit (documented); a single lane is
        // both far-left and far-right at once.
        if self.roads[arc.road as usize].one_way || arc.lanes.len() < 2 {
            return exits.to_vec();
        }
        let Some(rank) = arc.lanes.iter().position(|id| *id == lane) else {
            return Vec::new();
        };
        let last = arc.lanes.len() - 1;
        // Innermost lanes turn toward the centre, outermost kerb-side.
        // On the left side travel faces the other way, so the driver's
        // left/right swap relative to centre distance.
        let set = match (arc.side, rank, rank == last) {
            (_, r, _) if r > 0 && r < last => TurnSet::STRAIGHT,
            (Side::Right, 0, _) => TurnSet(TurnSet::LEFT.0 | TurnSet::STRAIGHT.0),
            (Side::Right, _, true) => TurnSet(TurnSet::RIGHT.0 | TurnSet::STRAIGHT.0),
            (Side::Left, 0, _) => TurnSet(TurnSet::RIGHT.0 | TurnSet::STRAIGHT.0),
            (Side::Left, _, true) => TurnSet(TurnSet::LEFT.0 | TurnSet::STRAIGHT.0),
            _ => TurnSet::ANY,
        };
        let filtered: Vec<ArcExit> = exits
            .iter()
            .copied()
            .filter(|e| set.contains(e.turn))
            .collect();
        if !filtered.is_empty() {
            return filtered;
        }
        // Fallback: no geometrically legal exit for this lane — take the
        // straightest available rather than stranding the vehicle.
        exits
            .iter()
            .copied()
            .min_by(|a, b| {
                a.heading_change
                    .abs()
                    .partial_cmp(&b.heading_change.abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .into_iter()
            .collect()
    }

    /// Deterministic ambient exit choice: legal exits for `lane`,
    /// picked by `rng`. The same seed picks the same exit everywhere.
    pub fn choose_exit(&self, lane: LaneId, rng: &mut NavRng) -> Option<ArcExit> {
        let exits = self.legal_exits(lane);
        rng.pick(&exits).copied()
    }

    /// Sample `lane` at `s` metres along its travel direction
    /// (clamped). Vehicle lanes measure `s` along travel and return a
    /// travel-direction tangent; sidewalks and rails measure `s` in
    /// storage order.
    pub fn sample_lane(&self, lane: LaneId, s: f32) -> Option<LaneSample> {
        let l = self.lane(lane)?;
        let (s_storage, sign) = match l.arc {
            Some(arc) => match self.arc(arc).dir {
                TravelDir::Forward => (s, 1.0),
                TravelDir::Backward => (l.length - s, -1.0),
            },
            None => (s, 1.0),
        };
        let mut sample = sample_storage(l, s_storage);
        sample.tangent = scale3(sample.tangent, sign);
        Some(sample)
    }

    /// Closest eligible lane to `point`, in full 3D — a lane on a
    /// bridge above the query never wins over one at the query's
    /// height, and [`LaneQuery::rooms`] can hard-filter by PSDL room
    /// when stacked lanes are genuinely ambiguous.
    pub fn nearest_lane(&self, point: [f32; 3], query: &LaneQuery) -> Option<LaneHit> {
        let mut best: Option<LaneHit> = None;
        let mut best_d2 = query.max_distance * query.max_distance;
        for idx in self.grid.candidates(point, query.max_distance) {
            let lane = &self.lanes[idx as usize];
            if let Some(kind) = query.kind
                && lane.id.kind != kind
            {
                continue;
            }
            if query.routable_only && lane.arc.is_none() {
                continue;
            }
            if let Some(rooms) = &query.rooms {
                let road = &self.roads[lane.id.road as usize];
                if !road.rooms.iter().any(|r| rooms.contains(r)) {
                    continue;
                }
            }
            let Some(hit) = project_to_lane(lane, point) else {
                continue;
            };
            let d2 = dist2(hit.point, point);
            if d2 <= best_d2 {
                best_d2 = d2;
                best = Some(LaneHit {
                    lane: lane.id,
                    distance: d2.sqrt(),
                    ..hit
                });
            }
        }
        best
    }

    /// Bounded A* route between two world points. Both ends snap to
    /// the nearest routable vehicle lane within
    /// [`RouteOptions::max_snap`]; the search expands turn connections
    /// until it reaches the goal arc, the frontier empties
    /// ([`RouteError::Unreachable`]) or the expansion bound trips
    /// ([`RouteError::ExpansionLimit`]). Deterministic: costs are arc
    /// lengths and ties break on [`ArcId`].
    pub fn route(
        &self,
        from: [f32; 3],
        to: [f32; 3],
        options: &RouteOptions,
    ) -> Result<Route, RouteError> {
        let query = LaneQuery::vehicles(options.max_snap);
        let start = self
            .nearest_lane(from, &query)
            .ok_or(RouteError::NoStartLane)?;
        let goal = self
            .nearest_lane(to, &query)
            .ok_or(RouteError::NoGoalLane)?;
        let start_arc = self.lane(start.lane).and_then(|l| l.arc);
        let goal_arc = self.lane(goal.lane).and_then(|l| l.arc);
        let (Some(start_arc), Some(goal_arc)) = (start_arc, goal_arc) else {
            return Err(RouteError::NoStartLane);
        };

        let mut came: HashMap<ArcId, ArcId> = HashMap::new();
        let mut g: HashMap<ArcId, f32> = HashMap::new();
        let mut heap: BinaryHeap<QueueEntry> = BinaryHeap::new();
        g.insert(start_arc, 0.0);
        heap.push(QueueEntry {
            cost: heuristic(self.arc(start_arc).exit_point, to),
            arc: start_arc,
        });
        let mut expanded = 0usize;
        let mut found = false;
        while let Some(QueueEntry { arc, .. }) = heap.pop() {
            if arc == goal_arc {
                found = true;
                break;
            }
            expanded += 1;
            if expanded > options.max_expansions {
                return Err(RouteError::ExpansionLimit { expanded });
            }
            let base = g[&arc];
            for exit in &self.exits[arc.0 as usize] {
                let next = self.arc(exit.to);
                if options.closed_roads.contains(&next.road) {
                    continue;
                }
                let cost = base + next.length.max(0.0);
                if cost < *g.get(&exit.to).unwrap_or(&f32::INFINITY) {
                    g.insert(exit.to, cost);
                    came.insert(exit.to, arc);
                    heap.push(QueueEntry {
                        cost: cost + heuristic(next.exit_point, to),
                        arc: exit.to,
                    });
                }
            }
        }
        if !found {
            return Err(RouteError::Unreachable { expanded });
        }

        let mut steps = vec![goal_arc];
        while steps.last() != Some(&start_arc) {
            steps.push(came[steps.last().unwrap()]);
        }
        steps.reverse();
        let length = steps.iter().map(|a| self.arc(*a).length.max(0.0)).sum();
        Ok(Route {
            start,
            goal,
            steps,
            length,
        })
    }

    /// Route probe between two BAI road indices — the shared helper
    /// `mm2-inspect nav --route` and the `--nav-route` overlay use.
    /// Each end anchors at the midpoint of the road's first arc's
    /// first lane: a point on the lane curve resolves unambiguously
    /// to that lane, where the arc's centreline would sit equidistant
    /// between both travel directions. A road with no arc maps to
    /// [`RouteError::NoStartLane`]/[`RouteError::NoGoalLane`].
    pub fn route_roads(
        &self,
        from: u16,
        to: u16,
        options: &RouteOptions,
    ) -> Result<Route, RouteError> {
        let anchor = |road: u16| -> Option<[f32; 3]> {
            let arc = self
                .road(road)?
                .arcs
                .iter()
                .flatten()
                .next()
                .map(|a| self.arc(*a))?;
            let lane = self.lane(*arc.lanes.first()?)?;
            self.sample_lane(lane.id, lane.length * 0.5)
                .map(|s| s.position)
        };
        let a = anchor(from).ok_or(RouteError::NoStartLane)?;
        let b = anchor(to).ok_or(RouteError::NoGoalLane)?;
        self.route(a, b, options)
    }

    /// Start a cursor at a route's snapped start position.
    pub fn cursor(&self, route: &Route) -> RouteCursor {
        RouteCursor {
            step: 0,
            lane: route.start.lane,
            distance: self.travel_distance(route.start.lane, route.start.along),
        }
    }

    /// Distance along a lane in travel direction for a storage-order
    /// `along` value.
    fn travel_distance(&self, lane: LaneId, along_storage: f32) -> f32 {
        let Some(l) = self.lane(lane) else {
            return 0.0;
        };
        match l.arc.map(|a| self.arc(a).dir) {
            Some(TravelDir::Backward) => l.length - along_storage,
            _ => along_storage,
        }
    }

    /// Sample a cursor's world position/tangent.
    pub fn cursor_sample(&self, cursor: &RouteCursor) -> Option<LaneSample> {
        self.sample_lane(cursor.lane, cursor.distance)
    }

    /// Advance `cursor` `ds` metres along `route`. Crossing an arc
    /// boundary keeps the same inner→outer lane rank on the next arc.
    /// Returns the distance left unconsumed at the route's end.
    pub fn advance_cursor(&self, cursor: &mut RouteCursor, route: &Route, ds: f32) -> f32 {
        let mut rest = ds.max(0.0);
        loop {
            let Some(lane) = self.lane(cursor.lane) else {
                return rest;
            };
            let remaining = lane.length - cursor.distance;
            if rest < remaining {
                cursor.distance += rest;
                return 0.0;
            }
            rest -= remaining.max(0.0);
            cursor.step += 1;
            let Some(&arc_id) = route.steps.get(cursor.step) else {
                return rest;
            };
            let arc = self.arc(arc_id);
            // Keep the same normalized inner→outer rank on the new arc.
            let prev_arc = self.arc(route.steps[cursor.step - 1]);
            let rank = prev_arc
                .lanes
                .iter()
                .position(|id| *id == cursor.lane)
                .map(|i| {
                    if prev_arc.lanes.len() > 1 {
                        i as f32 / (prev_arc.lanes.len() - 1) as f32
                    } else {
                        0.0
                    }
                })
                .unwrap_or(0.0);
            let pick = ((arc.lanes.len() - 1) as f32 * rank).round() as usize;
            cursor.lane = arc.lanes[pick.min(arc.lanes.len() - 1)];
            cursor.distance = 0.0;
        }
    }
}

fn dir_index(dir: TravelDir) -> usize {
    match dir {
        TravelDir::Forward => 0,
        TravelDir::Backward => 1,
    }
}

fn lane_offset(lanes: &[NavLane], lookup: &HashMap<u64, u32>, id: LaneId) -> f32 {
    lookup
        .get(&id.key())
        .and_then(|&i| lanes.get(i as usize))
        .map(|l| l.lateral_offset)
        .unwrap_or(0.0)
}

/// Mean signed lateral offset of a curve from the road centre line,
/// measured per section against the authored frame's `x_axis`.
fn lateral_offset(points: &[[f32; 3]], road: &mm2_formats::bai::Road) -> f32 {
    let n = points.len().min(road.sections.len());
    if n == 0 {
        return 0.0;
    }
    let mut acc = 0.0;
    for (p, s) in points.iter().zip(road.sections.iter()) {
        let x = normalize(s.x_axis);
        acc += dot3(sub3(*p, s.origin), x);
    }
    acc / n as f32
}

/// Register a lane curve; returns its index, or `None` when the curve
/// is degenerate. Recomputes cumulative distances from the vertices
/// when the authored row is absent or not monotone.
#[allow(clippy::too_many_arguments)]
fn push_lane(
    lanes: &mut Vec<NavLane>,
    lookup: &mut HashMap<u64, u32>,
    id: LaneId,
    points: Vec<[f32; 3]>,
    authored: Option<&[f32]>,
    edge: Option<f32>,
    offset: f32,
    road: usize,
    issues: &mut Vec<NavIssue>,
) -> Option<usize> {
    let valid_cum =
        authored.is_some_and(|c| c.len() == points.len() && c.windows(2).all(|w| w[1] >= w[0]));
    let cum: Vec<f32> = if valid_cum {
        authored.unwrap().to_vec()
    } else {
        if authored.is_some() {
            issues.push(NavIssue::LaneDistancesRecomputed {
                road,
                side: id.side,
                kind: id.kind,
                index: id.index as usize,
            });
        }
        let mut c = Vec::with_capacity(points.len());
        let mut acc = 0.0;
        for (i, p) in points.iter().enumerate() {
            if i > 0 {
                acc += dist(points[i - 1], *p);
            }
            c.push(acc);
        }
        c
    };
    let length = cum.last().copied().unwrap_or(0.0);
    if points.len() < 2 || length <= f32::EPSILON {
        issues.push(NavIssue::DegenerateLane {
            road,
            side: id.side,
            kind: id.kind,
            index: id.index as usize,
        });
        return None;
    }
    let idx = lanes.len();
    lanes.push(NavLane {
        id,
        arc: None,
        lateral_offset: offset,
        edge_distance: edge,
        length,
        points,
        cum,
    });
    lookup.insert(id.key(), idx as u32);
    Some(idx)
}

/// Signed heading change from `a` to `b` on the ground plane, in
/// radians: positive rotates toward `up × forward` (the driver's
/// left).
fn signed_xz_angle(a: [f32; 3], b: [f32; 3]) -> f32 {
    let cross_y = a[2] * b[0] - a[0] * b[2];
    let dot = a[0] * b[0] + a[2] * b[2];
    cross_y.atan2(dot)
}

fn classify_turn(change: f32) -> TurnKind {
    if change > STRAIGHT_CONE {
        TurnKind::Left
    } else if change < -STRAIGHT_CONE {
        TurnKind::Right
    } else {
        TurnKind::Straight
    }
}

/// Travel heading of an arc at one extremity. `downstream` selects the
/// exit extremity (`true`) or the entry extremity (`false`).
fn travel_tangent(road: &mm2_formats::bai::Road, dir: TravelDir, downstream: bool) -> [f32; 3] {
    let end = match (dir, downstream) {
        (TravelDir::Forward, true) | (TravelDir::Backward, false) => End::End,
        _ => End::Start,
    };
    let section = match end {
        End::Start => road.sections.first(),
        End::End => road.sections.last(),
    };
    let t = section.map(|s| s.tangent).unwrap_or([0.0, 0.0, 1.0]);
    match dir {
        TravelDir::Forward => t,
        TravelDir::Backward => scale3(t, -1.0),
    }
}

/// Project `p` onto a lane's polyline: closest point, its tangent in
/// storage order and its distance along the curve.
fn project_to_lane(lane: &NavLane, p: [f32; 3]) -> Option<LaneHit> {
    if lane.points.len() < 2 {
        return None;
    }
    let mut best: Option<([f32; 3], f32, f32)> = None; // point, d2, along
    let mut best_seg = 0usize;
    for i in 0..lane.points.len() - 1 {
        let a = lane.points[i];
        let b = lane.points[i + 1];
        let ab = sub3(b, a);
        let len2 = dot3(ab, ab);
        let t = if len2 > f32::EPSILON {
            (dot3(sub3(p, a), ab) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q = add3(a, scale3(ab, t));
        let d2 = dist2(q, p);
        let span = lane.cum[i + 1] - lane.cum[i];
        let along = lane.cum[i] + span * t;
        if best.is_none_or(|(_, bd2, _)| d2 < bd2) {
            best = Some((q, d2, along));
            best_seg = i;
        }
    }
    let (point, _, along) = best?;
    let tangent = segment_tangent(&lane.points, best_seg);
    Some(LaneHit {
        lane: lane.id,
        distance: 0.0,
        point,
        tangent,
        along,
    })
}

/// Unit tangent of segment `i`, falling back to the nearest non-zero
/// segment when this one is degenerate.
fn segment_tangent(points: &[[f32; 3]], i: usize) -> [f32; 3] {
    let n = points.len();
    for off in 0..n {
        for j in [i.saturating_sub(off), (i + off).min(n.saturating_sub(2))] {
            if j + 1 < n {
                let d = sub3(points[j + 1], points[j]);
                if dot3(d, d) > f32::EPSILON {
                    return normalize(d);
                }
            }
        }
    }
    [0.0, 0.0, 0.0]
}

/// Piecewise-linear sample at storage-order distance `s` (clamped).
fn sample_storage(lane: &NavLane, s: f32) -> LaneSample {
    let s = s.clamp(0.0, lane.length);
    let n = lane.points.len();
    // First index with cum[i+1] >= s, clamped to a real segment.
    let i = lane
        .cum
        .partition_point(|&c| c < s)
        .saturating_sub(1)
        .min(n - 2);
    let span = lane.cum[i + 1] - lane.cum[i];
    let t = if span > f32::EPSILON {
        ((s - lane.cum[i]) / span).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let a = lane.points[i];
    let b = lane.points[i + 1];
    LaneSample {
        position: add3(a, scale3(sub3(b, a), t)),
        tangent: segment_tangent(&lane.points, i),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct QueueEntry {
    cost: f32,
    arc: ArcId,
}

impl Eq for QueueEntry {}

impl Ord for QueueEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Min-heap: smaller cost pops first; ties break on ArcId so the
        // search is deterministic.
        other
            .cost
            .partial_cmp(&self.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| other.arc.cmp(&self.arc))
    }
}

impl PartialOrd for QueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn heuristic(from: [f32; 3], to: [f32; 3]) -> f32 {
    dist(from, to)
}

fn union(parent: &mut Vec<usize>, a: usize, b: usize) {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if ra != rb {
        parent[rb] = ra;
    }
}

fn find(parent: &mut Vec<usize>, i: usize) -> usize {
    if parent[i] != i {
        parent[i] = find(parent, parent[i]);
    }
    parent[i]
}

fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale3(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    dist2(a, b).sqrt()
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    dot3(sub3(a, b), sub3(a, b))
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = dot3(a, a).sqrt();
    if l > f32::EPSILON {
        scale3(a, 1.0 / l)
    } else {
        [0.0, 0.0, 0.0]
    }
}
