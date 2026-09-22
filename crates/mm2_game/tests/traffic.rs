//! F10-A.1 ambient-traffic tests: cumulative-weight roster selection,
//! seeded spawn planning over synthetic BAI fixtures — closed roads,
//! pedestrian-only sides, the player bubble, unspawnable classes and
//! plan bounds (F10-AC01's data slice).

use bevy::prelude::Entity;
use mm2_formats::bai::{
    Bai, Culling, Intersection, Road, RoadEnd, RoadSection, RoadSide, Side, VehicleRule,
};
use mm2_formats::veh::AiVehicleData;
use mm2_game::*;

// ---------- synthetic BAI fixtures (same shape as tests/nav.rs) ----------

fn section(distance: f32, origin: [f32; 3], tangent: [f32; 3]) -> RoadSection {
    RoadSection {
        distance,
        origin,
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
        tangent,
    }
}

fn cum(pts: &[[f32; 3]]) -> Vec<f32> {
    let mut out = Vec::with_capacity(pts.len());
    let mut acc = 0.0;
    for (i, p) in pts.iter().enumerate() {
        if i > 0 {
            let d = [
                p[0] - pts[i - 1][0],
                p[1] - pts[i - 1][1],
                p[2] - pts[i - 1][2],
            ];
            acc += (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        }
        out.push(acc);
    }
    out
}

fn side(ambient: u16, lanes: &[(f32, Vec<[f32; 3]>)], nsec: usize) -> RoadSide {
    RoadSide {
        lane_count: lanes.len() as u16,
        tram_count: 0,
        train_count: 0,
        sidewalk_count: 0,
        ambient_types: ambient,
        lane_distances: lanes.iter().map(|(_, v)| cum(v)).collect(),
        edge_distances: lanes.iter().map(|(e, _)| *e).collect(),
        misc: [0xCD; 40],
        lane_vertices: lanes.iter().map(|(_, v)| v.clone()).collect(),
        tram_vertices: Vec::new(),
        train_vertices: Vec::new(),
        sidewalk_inner: vec![[0.0; 3]; nsec],
        sidewalk_outer: vec![[0.0; 3]; nsec],
    }
}

fn dead_end() -> mm2_formats::bai::RoadEnd {
    mm2_formats::bai::RoadEnd {
        intersection: 0,
        fill0: 0xCDCD,
        vehicle_rule_code: 0,
        unknown1: 0,
        intersection_road_index: mm2_formats::bai::END_FILL,
        traffic_light_origin: [0.0; 3],
        traffic_light_axis: [0.0; 3],
    }
}

fn connected(intersection: u32, road_index: u32) -> RoadEnd {
    RoadEnd {
        intersection,
        intersection_road_index: road_index,
        ..dead_end()
    }
}

fn offset(pts: &[[f32; 3]], dx: f32, dz: f32) -> Vec<[f32; 3]> {
    pts.iter().map(|p| [p[0] + dx, p[1], p[2] + dz]).collect()
}

/// One road along +z at x=`x0`, one lane per side; `ambient` is the raw
/// ambientTypes code on both sides.
fn road_x(id: u16, x0: f32, ambient: u16) -> Road {
    let centre = [[x0, 0.0, 0.0], [x0, 0.0, 100.0]];
    let right = side(ambient, &[(3.75, offset(&centre, 3.75, 0.0))], 2);
    let left = side(ambient, &[(-3.75, offset(&centre, -3.75, 0.0))], 2);
    let dists = cum(&centre);
    Road {
        id,
        flags: 0,
        rooms: vec![1],
        half_width: 7.5,
        base_speed: 15.0,
        right,
        left,
        sections: dists
            .iter()
            .enumerate()
            .map(|(i, d)| section(*d, centre[i], [0.0, 0.0, 1.0]))
            .collect(),
        start: dead_end(),
        end: dead_end(),
    }
}

fn bai(roads: Vec<Road>) -> Bai {
    Bai {
        roads,
        intersections: Vec::<Intersection>::new(),
        culling: Culling {
            large: vec![Vec::new()],
            small: vec![Vec::new()],
        },
    }
}

/// Two parallel vehicle roads (0 and 1), each 100 m long.
fn two_roads() -> NavGraph {
    NavGraph::build(&bai(vec![road_x(0, 0.0, 0), road_x(1, 20.0, 0)])).graph
}

/// A road along `centre` with `lanes` (edge distance, vertices) per
/// side and caller-wired junction records.
fn road_full(
    id: u16,
    centre: &[[f32; 3]],
    lanes_per_side: usize,
    start: RoadEnd,
    end: RoadEnd,
) -> Road {
    let mk = |sign: f32| -> Vec<(f32, Vec<[f32; 3]>)> {
        (0..lanes_per_side)
            .map(|k| {
                let o = sign * (1.5 + k as f32 * 2.25);
                (o.abs(), offset(centre, o, 0.0))
            })
            .collect()
    };
    let right = side(0, &mk(1.0), centre.len());
    let left = side(0, &mk(-1.0), centre.len());
    let dists = cum(centre);
    Road {
        id,
        flags: 0,
        rooms: vec![1],
        half_width: 7.5,
        base_speed: 15.0,
        right,
        left,
        sections: dists
            .iter()
            .enumerate()
            .map(|(i, d)| section(*d, centre[i], [0.0, 0.0, 1.0]))
            .collect(),
        start,
        end,
    }
}

/// Two 100 m roads chained end-to-start at an intersection at z=100 —
/// road 0 forward runs into road 1 forward; both other extremities are
/// dead ends.
fn chain(lanes_per_side: usize) -> (NavGraph, Vec<NavIssue>) {
    let r0 = road_full(
        0,
        &[[0.0, 0.0, 0.0], [0.0, 0.0, 100.0]],
        lanes_per_side,
        dead_end(),
        connected(0, 0),
    );
    let r1 = road_full(
        1,
        &[[0.0, 0.0, 100.0], [0.0, 0.0, 200.0]],
        lanes_per_side,
        connected(0, 1),
        dead_end(),
    );
    let build = NavGraph::build(&bai_full(
        vec![r0, r1],
        vec![Intersection {
            id: 0,
            room: 1,
            center: [0.0, 0.0, 100.0],
            roads: vec![0, 1],
        }],
    ));
    (build.graph, build.issues)
}

fn bai_full(roads: Vec<Road>, intersections: Vec<Intersection>) -> Bai {
    Bai {
        roads,
        intersections,
        culling: Culling {
            large: vec![Vec::new()],
            small: vec![Vec::new()],
        },
    }
}

/// Road 1's sides are pedestrian-only (ambientTypes 1) — no arcs.
fn road1_pedestrian_only() -> NavGraph {
    NavGraph::build(&bai(vec![road_x(0, 0.0, 0), road_x(1, 20.0, 1)])).graph
}

// ---------- roster fixtures ----------

fn tuning() -> AiVehicleData {
    AiVehicleData {
        mass: 1000.0,
        size: [2.0, 1.5, 5.0],
        max_ang: Some([0.0; 3]),
        elasticity: 0.5,
        friction: 0.2,
        max_damage: 70000.0,
        ptx_thresh: 70000.0,
        spring: 17000.0,
        damping: 1200.0,
        limit: 0.07,
        rubber_spring: 12000.0,
        rubber_damp: 600.0,
        cg: Some([0.0; 3]),
        warnings: Vec::new(),
    }
}

fn spec(id: &str, weight: f32, resolved: bool) -> AmbientSpec {
    AmbientSpec {
        id: id.to_string(),
        cumulative_weight: weight,
        flag: 0,
        tuning: resolved.then(tuning),
    }
}

fn roster(specs: &[(&str, f32)]) -> AmbientRoster {
    AmbientRoster::new(specs.iter().map(|(id, w)| spec(id, *w, true)).collect())
}

/// Past the recycle radius — every lane is out of the spawn annulus.
const FAR_AWAY: [f32; 3] = [0.0, -500.0, 0.0];
/// Inside the annulus but outside the 60 m bubble: every fixture lane
/// sample is 150–260 m away, so draws always reach the class pick.
const CLEAR: [f32; 3] = [10.0, 0.0, -150.0];

// ---------- roster selection ----------

#[test]
fn select_respects_cumulative_weight_boundaries() {
    let r = roster(&[("va_a", 0.5), ("va_b", 1.0)]);
    assert!(r.issues.is_empty());
    assert_eq!(r.select(0.0), Some(0));
    assert_eq!(r.select(0.4999), Some(0));
    // u == cumulative weight falls through to the next band.
    assert_eq!(r.select(0.5), Some(1));
    assert_eq!(r.select(0.9999), Some(1));

    // A table that never reaches 1.0 leaves the tail unselectable.
    let open = roster(&[("va_a", 0.8)]);
    assert_eq!(open.select(0.79), Some(0));
    assert_eq!(open.select(0.9), None);
    assert!(
        open.issues
            .iter()
            .any(|i| matches!(i, TrafficIssue::WeightsNotClosed { .. }))
    );

    assert!(AmbientRoster::default().select(0.1).is_none());
}

#[test]
fn roster_reports_non_monotone_weights_but_allows_repeat_ids() {
    let r = AmbientRoster::new(vec![
        spec("va_a", 0.5, true),
        spec("va_b", 0.4, true),
        spec("va_b", 1.0, true), // london authors va_compact_s twice — legal
    ]);
    assert!(
        r.issues
            .iter()
            .any(|i| matches!(i, TrafficIssue::WeightOrder { index: 1, .. })),
        "{:?}",
        r.issues
    );
    assert_eq!(r.issues.len(), 1, "{:?}", r.issues);
}

// ---------- plan ----------

#[test]
fn plan_is_deterministic_and_seed_sensitive() {
    let g = two_roads();
    let r = roster(&[("va_a", 0.4), ("va_b", 0.7), ("va_c", 1.0)]);
    let a = plan_ambient(
        &g,
        &NavOverrides::default(),
        &r,
        42,
        0.5,
        &[CLEAR],
        &SpawnPolicy::default(),
    );
    let b = plan_ambient(
        &g,
        &NavOverrides::default(),
        &r,
        42,
        0.5,
        &[CLEAR],
        &SpawnPolicy::default(),
    );
    assert_eq!(a.spawns, b.spawns, "same seed must replay identically");
    assert_eq!(a.spawns.len(), a.target);

    let c = plan_ambient(
        &g,
        &NavOverrides::default(),
        &r,
        7,
        0.5,
        &[CLEAR],
        &SpawnPolicy::default(),
    );
    assert_ne!(a.spawns, c.spawns, "different seed, different draw");
}

#[test]
fn plan_bounds_output_and_scales_with_density() {
    let g = two_roads();
    let r = roster(&[("va_a", 1.0)]);
    let policy = SpawnPolicy {
        max_active: 10,
        ..SpawnPolicy::default()
    };
    let full = plan_ambient(&g, &NavOverrides::default(), &r, 1, 1.0, &[CLEAR], &policy);
    assert_eq!(full.target, 10);
    assert_eq!(full.spawns.len(), 10);
    assert!(full.spawns.len() <= policy.max_active);

    let half = plan_ambient(&g, &NavOverrides::default(), &r, 1, 0.5, &[CLEAR], &policy);
    assert_eq!(half.target, 5);
    let off = plan_ambient(&g, &NavOverrides::default(), &r, 1, 0.0, &[CLEAR], &policy);
    assert_eq!(off.target, 0);
    assert!(off.spawns.is_empty());
}

#[test]
fn plan_honours_closed_roads_and_pedestrian_only_sides() {
    let r = roster(&[("va_a", 1.0)]);
    let policy = SpawnPolicy::default();

    // Road 0 closed by the event overrides: every spawn lands on road 1.
    let g = two_roads();
    let mut overrides = NavOverrides::default();
    overrides.closed_roads.insert(0);
    let plan = plan_ambient(&g, &overrides, &r, 9, 1.0, &[CLEAR], &policy);
    assert_eq!(plan.eligible_lanes, 2, "only road 1's two lanes");
    assert!(plan.spawns.iter().all(|s| s.lane.road == 1));

    // Pedestrian-only sides never become routable arcs — the graph
    // withholds them, so the planner cannot pick them.
    let g = road1_pedestrian_only();
    let plan = plan_ambient(&g, &NavOverrides::default(), &r, 9, 1.0, &[CLEAR], &policy);
    assert_eq!(plan.eligible_lanes, 2, "only road 0's two lanes");
    assert!(plan.spawns.iter().all(|s| s.lane.road == 0));

    // Closing every road leaves the plan empty but well-formed.
    let mut overrides = NavOverrides::default();
    overrides.closed_roads.extend([0u16, 1u16]);
    let plan = plan_ambient(&g, &overrides, &r, 9, 1.0, &[CLEAR], &policy);
    assert!(plan.spawns.is_empty());
    assert!(
        plan.issues
            .iter()
            .any(|i| matches!(i, TrafficIssue::NoEligibleLanes))
    );
}

#[test]
fn plan_flags_unspawnable_and_empty_rosters() {
    let g = two_roads();
    let policy = SpawnPolicy::default();

    // A class with no resolved tuning stays in the weight table; draws
    // landing on it spawn nothing and report once per id.
    let r = AmbientRoster::new(vec![spec("va_ghost", 1.0, false)]);
    let plan = plan_ambient(&g, &NavOverrides::default(), &r, 3, 1.0, &[CLEAR], &policy);
    assert!(plan.spawns.is_empty());
    assert_eq!(plan.unspawnable, plan.target);
    assert_eq!(
        plan.issues
            .iter()
            .filter(|i| matches!(i, TrafficIssue::UnspawnableClass { .. }))
            .count(),
        1,
        "one issue per unspawnable id, not per draw"
    );

    // Empty roster → EmptyRoster, no panic, no spawns.
    let empty = AmbientRoster::default();
    let plan = plan_ambient(
        &g,
        &NavOverrides::default(),
        &empty,
        3,
        1.0,
        &[CLEAR],
        &policy,
    );
    assert!(plan.spawns.is_empty());
    assert!(
        plan.issues
            .iter()
            .any(|i| matches!(i, TrafficIssue::EmptyRoster))
    );
}

#[test]
fn plan_never_spawns_inside_the_player_bubble() {
    let g = two_roads();
    let r = roster(&[("va_a", 1.0)]);
    // Player at the centre of road 0; bubble covers every lane —
    // every directive drops rather than materialising on the player.
    let policy = SpawnPolicy {
        min_player_distance: 200.0,
        ..SpawnPolicy::default()
    };
    let plan = plan_ambient(
        &g,
        &NavOverrides::default(),
        &r,
        5,
        1.0,
        &[[10.0, 0.0, 50.0]],
        &policy,
    );
    assert!(plan.spawns.is_empty());
    assert_eq!(plan.dropped, plan.target);

    // With a sane bubble every surviving spawn is outside it.
    let policy = SpawnPolicy::default();
    let player = [10.0, 0.0, 50.0];
    let plan = plan_ambient(&g, &NavOverrides::default(), &r, 5, 1.0, &[player], &policy);
    for s in &plan.spawns {
        let d = [
            s.sample.position[0] - player[0],
            s.sample.position[1] - player[1],
            s.sample.position[2] - player[2],
        ];
        assert!(
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() >= policy.min_player_distance,
            "spawn inside the bubble: {:?}",
            s.sample.position
        );
    }
}

/// A corrupt or modded `.bai` can author non-finite lane geometry —
/// lane vertices are raw `f32` bits. The graph drops such curves with
/// `NavIssue::NonFiniteLane` and the planner never sees them; before
/// the fix, a NaN-length lane reached `sample_storage`'s
/// `s.clamp(0.0, lane.length)`, which panics on a NaN bound, and an
/// inf-length lane produced NaN spawn positions that slipped past the
/// player-bubble check.
#[test]
fn plan_skips_non_finite_lane_geometry() {
    let mut b = bai(vec![road_x(0, 0.0, 0), road_x(1, 20.0, 0)]);
    // Road 1 right lane: NaN vertex with the authored distances
    // dropped, so the recomputed length is NaN — the old panic path.
    b.roads[1].right.lane_vertices[0][1][0] = f32::NAN;
    b.roads[1].right.lane_distances[0].clear();
    // Road 1 left lane: +inf vertex under still-valid authored
    // distances — finite length, non-finite samples (the sibling case).
    b.roads[1].left.lane_vertices[0][0][2] = f32::INFINITY;
    // Road 0 right lane: a non-finite authored distance under finite
    // vertices — recomputed, not dropped.
    b.roads[0].right.lane_distances[0][1] = f32::NAN;

    let build = NavGraph::build(&b);
    for side in [Side::Right, Side::Left] {
        assert!(
            build.issues.iter().any(|i| matches!(
                i,
                NavIssue::NonFiniteLane {
                    road: 1,
                    side: s,
                    kind: LaneKind::Vehicle,
                    ..
                } if *s == side
            )),
            "missing NonFiniteLane for road 1 {side}: {:?}",
            build.issues
        );
    }
    assert!(
        build
            .issues
            .iter()
            .any(|i| matches!(i, NavIssue::LaneDistancesRecomputed { road: 0, .. })),
        "{:?}",
        build.issues
    );

    let r = roster(&[("va_a", 1.0)]);
    // A bound under the two surviving lanes' occupancy capacity — the
    // check under test is geometry rejection, not spawn saturation.
    let plan = plan_ambient(
        &build.graph,
        &NavOverrides::default(),
        &r,
        5,
        1.0,
        &[CLEAR],
        &SpawnPolicy {
            max_active: 12,
            ..SpawnPolicy::default()
        },
    );
    assert_eq!(plan.eligible_lanes, 2, "only road 0's lanes survive");
    assert_eq!(plan.spawns.len(), plan.target);
    assert!(plan.spawns.iter().all(|s| s.lane.road == 0));
    assert!(
        plan.spawns
            .iter()
            .all(|s| s.sample.position.iter().all(|c| c.is_finite())),
        "non-finite spawn position"
    );
}

#[test]
fn nav_rng_next_f32_stays_in_unit_range() {
    let mut rng = NavRng::new(0);
    for _ in 0..1000 {
        let u = rng.next_f32();
        assert!((0.0..1.0).contains(&u), "{u}");
    }
}

// ---------- lane cursor (F10-A.2 runtime follow) ----------

fn lane_id(road: u16, side: Side, index: u16) -> LaneId {
    LaneId {
        road,
        side,
        kind: LaneKind::Vehicle,
        index,
    }
}

#[test]
fn cursor_advances_then_turns_then_dead_ends() {
    let (g, issues) = chain(1);
    assert!(issues.is_empty(), "{issues:?}");
    let overrides = NavOverrides::default();
    let mut rng = NavRng::new(42);
    let mut cur = LaneCursor {
        lane: lane_id(0, Side::Right, 0),
        along: 90.0,
    };

    // Mid-lane: stays put, `along` is travel distance.
    assert_eq!(
        advance_lane_cursor(&g, &overrides, &mut cur, 5.0, &mut rng),
        LaneAdvance::Along
    );
    assert_eq!(cur.lane, lane_id(0, Side::Right, 0));
    assert_eq!(cur.along, 95.0);

    // A step past the end turns onto road 1's forward lane and
    // consumes the remainder there.
    assert_eq!(
        advance_lane_cursor(&g, &overrides, &mut cur, 10.0, &mut rng),
        LaneAdvance::Turned
    );
    assert_eq!(cur.lane, lane_id(1, Side::Right, 0));
    assert_eq!(cur.along, 5.0);

    // Road 1's far end is a dead end — no legal exit at all.
    let step = advance_lane_cursor(&g, &overrides, &mut cur, 200.0, &mut rng);
    assert_eq!(step, LaneAdvance::DeadEnd);
}

#[test]
fn cursor_faces_the_authored_travel_direction() {
    let (g, _) = chain(1);
    let overrides = NavOverrides::default();
    let mut rng = NavRng::new(1);
    // Left side travels -z on these +z roads: the sampled pose must
    // face -z and advancing must move the car toward z=0.
    let left = lane_id(0, Side::Left, 0);
    let mut cur = LaneCursor {
        lane: left,
        along: 10.0,
    };
    let before = g.sample_lane(left, cur.along).unwrap();
    assert!(before.tangent[2] < 0.0, "left side drives -z: {before:?}");
    assert_eq!(
        advance_lane_cursor(&g, &overrides, &mut cur, 5.0, &mut rng),
        LaneAdvance::Along
    );
    let after = g.sample_lane(left, cur.along).unwrap();
    assert!(after.position[2] < before.position[2]);
    // The -z end of road 0 is a dead end.
    assert_eq!(
        advance_lane_cursor(&g, &overrides, &mut cur, 100.0, &mut rng),
        LaneAdvance::DeadEnd
    );
}

#[test]
fn cursor_never_enters_a_closed_road() {
    let (g, _) = chain(1);
    let mut overrides = NavOverrides::default();
    overrides.closed_roads.insert(1);
    let mut rng = NavRng::new(7);
    let mut cur = LaneCursor {
        lane: lane_id(0, Side::Right, 0),
        along: 95.0,
    };
    // The only exit leads onto closed road 1 — the car stops instead.
    assert_eq!(
        advance_lane_cursor(&g, &overrides, &mut cur, 10.0, &mut rng),
        LaneAdvance::DeadEnd
    );
}

#[test]
fn cursor_keeps_its_lane_rank_across_a_turn() {
    let (g, issues) = chain(2);
    assert!(issues.is_empty(), "{issues:?}");
    let overrides = NavOverrides::default();
    let mut rng = NavRng::new(3);
    // Inner lane (rank 1 of 2) on road 0 → rank 1 on road 1.
    let mut cur = LaneCursor {
        lane: lane_id(0, Side::Right, 1),
        along: 98.0,
    };
    assert_eq!(
        advance_lane_cursor(&g, &overrides, &mut cur, 10.0, &mut rng),
        LaneAdvance::Turned
    );
    assert_eq!(cur.lane, lane_id(1, Side::Right, 1));
}

// ---------- spawn annulus (F10-B.1: the recycler-radius bound) ----------

/// Placements past the recycler's own radius are never drawn — a car
/// that the next tick would collect is churn, not population. The
/// bubble test above covers the annulus's inner bound; this is the
/// outer one.
#[test]
fn plan_never_spawns_beyond_the_recycle_radius() {
    let g = two_roads();
    let r = roster(&[("va_a", 1.0)]);
    let policy = SpawnPolicy::default();
    // The whole network sits past 400 m — every attempt is out of
    // band, so the directives drop rather than spawning churn.
    let plan = plan_ambient(
        &g,
        &NavOverrides::default(),
        &r,
        5,
        1.0,
        &[FAR_AWAY],
        &policy,
    );
    assert!(plan.spawns.is_empty());
    assert_eq!(plan.dropped, plan.target);

    // With lanes inside the annulus every placement respects both
    // bounds: never inside the bubble, never past the recycle radius.
    let plan = plan_ambient(&g, &NavOverrides::default(), &r, 5, 1.0, &[CLEAR], &policy);
    assert_eq!(plan.spawns.len(), plan.target);
    for s in &plan.spawns {
        let d = [
            s.sample.position[0] - CLEAR[0],
            s.sample.position[1] - CLEAR[1],
            s.sample.position[2] - CLEAR[2],
        ];
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        assert!(
            dist >= policy.min_player_distance && dist <= policy.recycle_distance,
            "spawn out of annulus at {dist} m: {:?}",
            s.sample.position
        );
    }
}

// ---------- union of player interest areas (F10-B.8, spec req 2) ----------

/// The spawn band is the *union* of player interest areas: a position
/// admits inside any one area's `[min_player_distance,
/// recycle_distance]` annulus provided it also sits outside *every*
/// area's `min_player_distance` — a car can never materialise next to
/// anybody, no matter which area's bubble covered it. The recycler's
/// `within_interest` is the matching any-bubble survival test.
#[test]
fn spawn_band_is_the_union_of_player_interest_areas() {
    let policy = SpawnPolicy::default();
    let a = [0.0, 0.0, 0.0];
    let b = [350.0, 0.0, 0.0];

    // Each area alone covers part of the space; the union covers both.
    assert!(in_spawn_band([200.0, 0.0, 0.0], &[a], &policy));
    assert!(in_spawn_band([200.0, 0.0, 0.0], &[a, b], &policy));
    // 500 m out: past A's recycle radius, inside B's band — admitted
    // by the union, not by the nearest single area.
    assert!(!in_spawn_band([500.0, 0.0, 0.0], &[a], &policy));
    assert!(in_spawn_band([500.0, 0.0, 0.0], &[a, b], &policy));
    // Inside A's band yet 50 m from B: no area admits a spawn on
    // another player's nose.
    assert!(!in_spawn_band([300.0, 0.0, 0.0], &[a, b], &policy));
    // Past every area's recycle radius, and an empty interest set,
    // admit nothing.
    assert!(!in_spawn_band([5000.0, 0.0, 0.0], &[a, b], &policy));
    assert!(!in_spawn_band([200.0, 0.0, 0.0], &[], &policy));
    // Non-finite positions never admit (defensive — lane vertices are
    // raw f32 bits upstream of the sample).
    assert!(!in_spawn_band([f32::NAN, 0.0, 0.0], &[a], &policy));

    // The recycler collects only outside *every* bubble.
    assert!(within_interest([200.0, 0.0, 0.0], &[a, b], &policy));
    assert!(within_interest([500.0, 0.0, 0.0], &[a, b], &policy));
    assert!(!within_interest([500.0, 0.0, 0.0], &[a], &policy));
    assert!(!within_interest([0.0, 0.0, 0.0], &[], &policy));
}

/// Two players far apart: the first area's bubble covers no lane
/// (every sample past its recycle radius), so the union populates
/// entirely through the second area — the "two players far apart"
/// edge case at plan level.
#[test]
fn plan_populates_through_any_interest_area() {
    let g = two_roads();
    let r = roster(&[("va_a", 1.0)]);
    let policy = SpawnPolicy::default();
    let plan = plan_ambient(
        &g,
        &NavOverrides::default(),
        &r,
        5,
        1.0,
        &[FAR_AWAY, CLEAR],
        &policy,
    );
    assert_eq!(plan.spawns.len(), plan.target);
    for s in &plan.spawns {
        assert!(
            in_spawn_band(s.sample.position, &[FAR_AWAY, CLEAR], &policy),
            "spawn outside the union band: {:?}",
            s.sample.position
        );
    }

    // And with no players at all there is no bubble to populate.
    let plan = plan_ambient(&g, &NavOverrides::default(), &r, 5, 1.0, &[], &policy);
    assert!(plan.spawns.is_empty());
    assert_eq!(plan.dropped, plan.target);
}

// ---------- obstruction sense + follow law (F10-B.1) ----------

#[test]
fn corridor_reports_the_nearest_in_band_blocker() {
    let pos = [0.0, 0.0, 0.0];
    let fwd = [0.0, 0.0, 1.0];
    // Two blockers ahead in the corridor — the nearer wins.
    assert_eq!(
        corridor_gap(
            pos,
            fwd,
            2.4,
            3.0,
            40.0,
            [[0.5, 0.0, 20.0], [0.0, 0.0, 10.0]]
        ),
        Some(10.0)
    );
    // Behind, beside, above or past the reach: none of these count.
    for b in [
        [0.0, 0.0, -5.0], // behind
        [3.4, 0.0, 10.0], // neighbouring lane
        [0.0, 6.0, 10.0], // overpass
        [0.0, 0.0, 45.0], // beyond reach
    ] {
        assert_eq!(
            corridor_gap(pos, fwd, 2.4, 3.0, 40.0, [b]),
            None,
            "blocker {b:?} must not be sensed"
        );
    }
    // The corridor resolves against the heading, not a fixed axis.
    assert_eq!(
        corridor_gap(
            pos,
            [-1.0, 0.0, 0.0],
            2.4,
            3.0,
            40.0,
            [[-12.0, 0.0, 1.0], [12.0, 0.0, 0.0]]
        ),
        Some(12.0)
    );
    // A degenerate heading senses nothing rather than dividing by zero.
    assert_eq!(
        corridor_gap(pos, [0.0, 1.0, 0.0], 2.4, 3.0, 40.0, [[0.0, 0.0, 5.0]]),
        None
    );
}

#[test]
fn follow_speed_brakes_to_the_gap_and_resumes() {
    let p = FollowPolicy::default();
    let dt = 1.0 / 120.0;
    // A clear corridor accelerates to the road limit and holds it.
    let mut v = 0.0;
    for _ in 0..1200 {
        v = follow_speed(v, 15.0, None, dt, &p);
    }
    assert_eq!(v, 15.0);
    // Room ahead caps the desired speed on a sliding scale — 30 m is
    // still nearly a free run, 8 m is a crawl.
    let far = follow_speed(15.0, 15.0, Some(30.0), dt, &p);
    assert!(far > 14.0, "{far}");
    let near = follow_speed(15.0, 15.0, Some(8.0), dt, &p);
    assert!(near < 15.0, "{near}");
    // Rolling up on a standing blocker: the speed bleeds off and the
    // car settles at the follow gap — it never closes to contact.
    let mut v = 15.0;
    let mut gap = 40.0;
    for _ in 0..3600 {
        v = follow_speed(v, 15.0, Some(gap), dt, &p);
        gap -= v * dt;
    }
    assert!(
        v <= p.held_speed,
        "still rolling at the hold: v={v} gap={gap}"
    );
    assert!(
        (p.follow_gap - 1.0..=p.follow_gap + 1.0).contains(&gap),
        "held at gap {gap}, expected ~{}",
        p.follow_gap
    );
    // A blocker already inside panic range stops the car outright —
    // a kinematic hull that kept moving would shove it.
    assert_eq!(
        follow_speed(8.0, 15.0, Some(p.panic_gap - 0.1), dt, &p),
        0.0
    );
    // Corridor clear again: the car pulls away.
    assert!(follow_speed(0.0, 15.0, None, dt, &p) > 0.0);
}

// ---------- junction rules (F10-B.2) ----------

fn car(n: u32) -> Entity {
    Entity::from_raw_u32(n).expect("a test entity")
}

fn connected_rule(intersection: u32, road_index: u32, rule: u16) -> RoadEnd {
    RoadEnd {
        vehicle_rule_code: rule,
        ..connected(intersection, road_index)
    }
}

/// `chain` with caller-authored `vehicleRule` codes on the two
/// junction-connected ends — `r0_end` rules road 0's forward approach,
/// `r1_start` rules road 1's backward approach.
fn chain_with_rules(r0_end: u16, r1_start: u16) -> NavGraph {
    chain_with_center([0.0, 0.0, 100.0], r0_end, r1_start)
}

/// `chain_with_rules` with a caller-authored junction centre — the
/// occupancy zone's radius is measured against it.
fn chain_with_center(center: [f32; 3], r0_end: u16, r1_start: u16) -> NavGraph {
    let r0 = road_full(
        0,
        &[[0.0, 0.0, 0.0], [0.0, 0.0, 100.0]],
        1,
        dead_end(),
        connected_rule(0, 0, r0_end),
    );
    let r1 = road_full(
        1,
        &[[0.0, 0.0, 100.0], [0.0, 0.0, 200.0]],
        1,
        connected_rule(0, 1, r1_start),
        dead_end(),
    );
    NavGraph::build(&bai_full(
        vec![r0, r1],
        vec![Intersection {
            id: 0,
            room: 1,
            center,
            roads: vec![0, 1],
        }],
    ))
    .graph
}

#[test]
fn junction_gate_binds_the_authored_end_rule() {
    let r0 = lane_id(0, Side::Right, 0);
    let r1 = lane_id(1, Side::Left, 0);

    // NeverStop on both approaches: the gate is inert at any distance.
    let g = chain_with_rules(3, 3);
    let mut j = Junctions::default();
    assert_eq!(
        j.gate(&g, r0, car(1), false, false, false),
        JunctionGate::Open
    );
    assert_eq!(
        j.gate(&g, r1, car(2), true, true, false),
        JunctionGate::Open
    );

    // AlwaysStop never releases, however long the car stands.
    let g = chain_with_rules(2, 3);
    let mut j = Junctions::default();
    assert_eq!(
        j.gate(&g, r0, car(1), false, false, false),
        JunctionGate::Closed
    );
    for _ in 0..1000 {
        j.advance_tick();
    }
    assert_eq!(
        j.gate(&g, r0, car(1), true, true, false),
        JunctionGate::Closed
    );

    // An unconnected end carries no junction at all — the approach
    // lookup reports it rather than inventing a rule.
    assert_eq!(Junctions::approach(&g, lane_id(0, Side::Left, 0)), None);
}

#[test]
fn a_stop_sign_admits_the_first_arrival_after_its_dwell() {
    // StopSign on both approaches into the same junction.
    let g = chain_with_rules(0, 0);
    let mut j = Junctions::default();
    let dwell = j.policy.stop_dwell_ticks;
    let r0 = lane_id(0, Side::Right, 0);
    let r1 = lane_id(1, Side::Left, 0);

    // Not at the line: closed, and not registered — a car that brakes
    // short of the stop takes no place in the queue.
    assert_eq!(
        j.gate(&g, r0, car(1), false, false, false),
        JunctionGate::Closed
    );
    assert_eq!(j.waiting(), 0);

    // A stands at its line and registers; the dwell still holds it.
    j.advance_tick();
    assert_eq!(
        j.gate(&g, r0, car(1), true, true, false),
        JunctionGate::Closed
    );
    assert_eq!(j.waiting(), 1);

    // B arrives on the other approach after A and queues behind it.
    j.advance_tick();
    assert_eq!(
        j.gate(&g, r1, car(2), true, true, false),
        JunctionGate::Closed
    );
    assert_eq!(j.waiting(), 2);

    // A's dwell elapses → A alone may go; B stays closed — FCFS is
    // one-at-a-time, not simultaneous.
    for _ in 0..dwell {
        j.advance_tick();
    }
    assert_eq!(
        j.gate(&g, r0, car(1), true, true, false),
        JunctionGate::Open
    );
    assert_eq!(
        j.gate(&g, r1, car(2), true, true, false),
        JunctionGate::Closed
    );

    // A departs: B heads the queue and its own dwell has long passed.
    j.depart(car(1));
    assert_eq!(
        j.gate(&g, r1, car(2), true, true, false),
        JunctionGate::Open
    );

    // A stale entry never holds the queue — a recycled car's id is
    // dropped by `retain` against the live set.
    j.depart(car(2));
    assert_eq!(j.waiting(), 0);
    j.gate(&g, r0, car(9), true, true, false);
    assert_eq!(j.waiting(), 1);
    j.retain(&std::collections::BTreeSet::from([car(1), car(2)]));
    assert_eq!(j.waiting(), 0);
}

#[test]
fn a_traffic_light_cycles_one_member_road_at_a_time() {
    // TrafficLight on both approaches → a two-member signal cycle.
    let g = chain_with_rules(1, 1);
    assert_eq!(Junctions::signal_members(&g, 0), vec![0, 1]);
    let mut j = Junctions::default();
    j.policy.green_ticks = 10;
    j.policy.clear_ticks = 5;
    let r0 = lane_id(0, Side::Right, 0);
    let r1 = lane_id(1, Side::Left, 0);

    // Sweep a full 30-tick cycle: each member holds a contiguous
    // green, the all-red slice closes both approaches, and no tick
    // opens both.
    let mut seen_green = [false, false];
    let mut saw_all_red = false;
    for _ in 0..30 {
        match j.green_road(&g, 0) {
            Some(0) => {
                seen_green[0] = true;
                assert_eq!(
                    j.gate(&g, r0, car(1), true, true, false),
                    JunctionGate::Open
                );
                assert_eq!(
                    j.gate(&g, r1, car(2), true, true, false),
                    JunctionGate::Closed
                );
            }
            Some(1) => {
                seen_green[1] = true;
                assert_eq!(
                    j.gate(&g, r1, car(2), true, true, false),
                    JunctionGate::Open
                );
                assert_eq!(
                    j.gate(&g, r0, car(1), true, true, false),
                    JunctionGate::Closed
                );
            }
            other => {
                assert_eq!(other, None, "only member roads may hold green");
                saw_all_red = true;
                assert_eq!(
                    j.gate(&g, r0, car(1), true, true, false),
                    JunctionGate::Closed
                );
                assert_eq!(
                    j.gate(&g, r1, car(2), true, true, false),
                    JunctionGate::Closed
                );
            }
        }
        j.advance_tick();
    }
    assert_eq!(seen_green, [true, true], "both members must cycle");
    assert!(saw_all_red, "the clearance slice must close every road");

    // The retail-typical mixed junction: r1's approach authors
    // NeverStop — the signal cycles the lit member alone while the
    // free approach ignores the phase entirely.
    let g = chain_with_rules(1, 3);
    assert_eq!(Junctions::signal_members(&g, 0), vec![0]);
    let mut j = Junctions::default();
    j.policy.green_ticks = 10;
    j.policy.clear_ticks = 5;
    let mut lit_open = 0;
    let mut lit_closed = 0;
    for _ in 0..30 {
        match j.gate(&g, r0, car(1), true, true, false) {
            JunctionGate::Open => lit_open += 1,
            JunctionGate::Closed => lit_closed += 1,
        }
        assert_eq!(
            j.gate(&g, r1, car(2), true, true, false),
            JunctionGate::Open
        );
        j.advance_tick();
    }
    assert!(lit_open > 0 && lit_closed > 0, "{lit_open}/{lit_closed}");
}

#[test]
fn junction_speed_brakes_to_the_stop_line_and_never_accelerates() {
    let p = JunctionPolicy::default();
    let dt = 1.0 / 120.0;
    // An open gate is inert.
    assert_eq!(junction_speed(15.0, 1.0, JunctionGate::Open, dt, &p), 15.0);
    // Far out the ramp allows more than the car's speed — it keeps
    // it, never gains.
    assert_eq!(junction_speed(2.0, 30.0, JunctionGate::Closed, dt, &p), 2.0);
    // Faster than the ramp: bleeds off at `decel`, clamped at the ramp.
    let v = junction_speed(10.0, 5.0, JunctionGate::Closed, dt, &p);
    assert!((v - (10.0 - 9.0 / 120.0)).abs() < 1.0e-6, "{v}");
    // At and past the line the ramp is zero — the car stands.
    assert_eq!(junction_speed(0.05, 0.0, JunctionGate::Closed, dt, &p), 0.0);
    let v = junction_speed(3.0, -1.0, JunctionGate::Closed, dt, &p);
    assert!((v - (3.0 - 9.0 / 120.0)).abs() < 1.0e-6, "{v}");
}

// ---------- junction-box yield (F10-B.5) ----------

/// `junction_zone` spans the farthest member end plus `box_margin`
/// around the authored centre (endpoint-centroid fallback when the
/// centre is non-finite), and `inside_junction_zone` bands it
/// vertically — the occupancy region the yield consults.
#[test]
fn junction_zone_spans_member_ends_with_margin_and_rise() {
    let p = JunctionPolicy::default();
    // The standard chain authors every member end at the centre, so
    // the zone is the margin alone.
    let g = chain_with_rules(3, 3);
    let (c, r) = junction_zone(&g, 0, &p).expect("a wired junction zones");
    assert_eq!(c, [0.0, 0.0, 100.0]);
    assert!((r - p.box_margin).abs() < 1.0e-6, "{r}");
    assert!(inside_junction_zone([0.0, 0.0, 100.0], (c, r), &p));
    assert!(!inside_junction_zone([0.0, 0.0, 104.5], (c, r), &p));
    // Above the rise band the same XZ point does not occupy.
    assert!(!inside_junction_zone([0.0, 4.0, 100.0], (c, r), &p));

    // An offset centre measures the endpoint reach: member ends at
    // z = 100 against a z = 50 centre → radius 50 + margin.
    let g = chain_with_center([0.0, 0.0, 50.0], 3, 3);
    let (c, r) = junction_zone(&g, 0, &p).unwrap();
    assert!((r - (50.0 + p.box_margin)).abs() < 1.0e-6, "{r}");
    assert!(inside_junction_zone([0.0, 0.0, 99.0], (c, r), &p));
    assert!(!inside_junction_zone([0.0, 0.0, 104.0], (c, r), &p));

    // A non-finite authored centre falls back to the endpoint
    // centroid rather than zoning nothing.
    let g = chain_with_center([f32::NAN, 0.0, f32::NAN], 3, 3);
    let (c, _) = junction_zone(&g, 0, &p).unwrap();
    assert_eq!(c, [0.0, 0.0, 100.0]);

    // Out-of-range junctions zone nothing.
    assert!(junction_zone(&g, 9, &p).is_none());
}

/// A green member's gate closes while the box is occupied and reopens
/// when it clears — the authored cycle decides whose turn it is, not
/// whether the box is passable (F10-AC02's right-of-way leg).
#[test]
fn a_green_member_yields_while_the_box_is_occupied() {
    let g = chain_with_rules(1, 1);
    let mut j = Junctions::default();
    j.policy.green_ticks = 10;
    j.policy.clear_ticks = 5;
    let r0 = lane_id(0, Side::Right, 0);

    let mut yielded = false;
    let mut admitted = false;
    for _ in 0..30 {
        if j.green_road(&g, 0) == Some(0) {
            assert_eq!(
                j.gate(&g, r0, car(1), true, true, true),
                JunctionGate::Closed
            );
            yielded = true;
            assert_eq!(
                j.gate(&g, r0, car(1), true, true, false),
                JunctionGate::Open
            );
            admitted = true;
        }
        j.advance_tick();
    }
    assert!(yielded && admitted, "no green window for road 0 ran");
}

/// The FCFS head at a stop sign dwells, registers — and still yields
/// to an occupied box; a `NeverStop` approach keeps its documented
/// free flow through the same occupancy report.
#[test]
fn a_stop_sign_head_yields_but_a_never_stop_flows() {
    let g = chain_with_rules(0, 0);
    let mut j = Junctions::default();
    let dwell = j.policy.stop_dwell_ticks;
    let r0 = lane_id(0, Side::Right, 0);

    j.advance_tick();
    j.gate(&g, r0, car(1), true, true, false); // registers at the line
    for _ in 0..dwell {
        j.advance_tick();
    }
    assert_eq!(
        j.gate(&g, r0, car(1), true, true, true),
        JunctionGate::Closed,
        "the admitted head must yield to the occupied box"
    );
    assert_eq!(
        j.gate(&g, r0, car(1), true, true, false),
        JunctionGate::Open
    );

    // Occupancy never gates a `NeverStop` end.
    let g = chain_with_rules(3, 3);
    let mut j = Junctions::default();
    assert_eq!(
        j.gate(&g, r0, car(1), false, false, true),
        JunctionGate::Open
    );
    // Nor an `AlwaysStop` one — it was closed before and stays closed.
    let g = chain_with_rules(2, 3);
    let mut j = Junctions::default();
    assert_eq!(
        j.gate(&g, r0, car(1), true, true, false),
        JunctionGate::Closed
    );
}

/// The stuck window (F10-B.3): "cannot get anywhere", not "moved
/// slowly" — progress past `min_displacement` re-anchors, a full
/// stationary window expires on its bound tick and stays expired, and
/// a non-finite pose resets rather than counting as a stall.
#[test]
fn stuck_window_resets_on_progress_and_expires_stationary() {
    let policy = StuckPolicy {
        window_ticks: 10,
        min_displacement: 4.0,
    };
    let mut w = StuckWindow::new([0.0, 0.0, 0.0]);
    // Standing still fills the window without expiring early.
    for _ in 0..9 {
        assert!(!w.tick([0.0, 0.0, 0.0], &policy));
    }
    // Progress past the bound re-anchors and zeroes the window.
    assert!(!w.tick([5.0, 0.0, 0.0], &policy));
    assert_eq!((w.ticks, w.anchor), (0, [5.0, 0.0, 0.0]));
    // Exactly-under progress keeps counting — a penned car nudged a
    // hair is still penned.
    assert!(!w.tick([5.0 + 3.9, 0.0, 0.0], &policy));
    assert_eq!(w.ticks, 1);
    // A full stationary window expires on its bound tick — and stays
    // expired while the car is left in place, so a late despawn still
    // reads stuck.
    for _ in 0..8 {
        assert!(!w.tick([8.9, 0.0, 0.0], &policy));
    }
    assert!(w.tick([8.9, 0.0, 0.0], &policy));
    assert!(w.tick([8.9, 0.0, 0.0], &policy));
    // A non-finite pose resets rather than counting as "no progress".
    let mut nan = StuckWindow::new([0.0, 0.0, 0.0]);
    assert!(!nan.tick([f32::NAN; 3], &policy));
    assert_eq!(nan.ticks, 0);
}

// ---------- spawn occupancy (F10-B.4: reject occupied space) ----------

/// The exclusion box a draw carries: a point within `spawn_clearance`
/// of the sampled tangent — ahead *or* behind — and inside the
/// lateral/vertical bounds occupies the spot; a point in the
/// neighbouring lane or on an overpass does not.
#[test]
fn spawn_occupied_boxes_the_sample_tangent() {
    let p = SpawnPolicy::default(); // 5.0 along / 2.0 lateral / 3.0 rise
    let sample = LaneSample {
        position: [0.0, 0.0, 0.0],
        tangent: [0.0, 0.0, 1.0],
    };
    // Same lane, inside the bound — both directions occupy.
    assert!(spawn_occupied(&sample, &[[0.0, 0.0, 4.9]], &p));
    assert!(spawn_occupied(&sample, &[[0.0, 0.0, -4.9]], &p));
    // None of these touch the box.
    for o in [
        [0.0, 0.0, 5.1],   // past the longitudinal bound
        [2.1, 0.0, 0.0],   // neighbouring lane
        [0.0, 3.1, 0.0],   // overpass
        [0.0, 0.0, -60.0], // far behind
    ] {
        assert!(!spawn_occupied(&sample, &[o], &p), "{o:?} must not occupy");
    }
    // A degenerate tangent orients no box.
    let flat = LaneSample {
        position: [0.0, 0.0, 0.0],
        tangent: [0.0, 1.0, 0.0],
    };
    assert!(!spawn_occupied(&flat, &[[0.0, 0.0, 1.0]], &p));
}

/// A draw whose exclusion box touches an occupied point is rejected
/// (`Occupied`) before the class pick; the same draw stream under the
/// default box places — the fixture is not inherently unspawnable.
#[test]
fn draw_spawn_rejects_occupied_space() {
    let g = two_roads();
    let r = roster(&[("va_a", 1.0)]);
    let eligible = eligible_lanes(&g, &NavOverrides::default());
    // A box wider than the whole fixture: every sample lands occupied.
    let wide = SpawnPolicy {
        spawn_clearance: 500.0,
        spawn_half_width: 500.0,
        spawn_max_rise: 500.0,
        ..SpawnPolicy::default()
    };
    let mut rng = NavRng::new(7);
    for _ in 0..8 {
        assert!(matches!(
            draw_spawn(
                &g,
                &NavOverrides::default(),
                &eligible,
                &r,
                &mut rng,
                &[CLEAR],
                &[[0.0, 0.0, 0.0]],
                &wide,
            ),
            SpawnDraw::Occupied
        ));
    }
    // The default box never touches the blocker point — it sits on
    // the road centre, 3.75 m laterally from every lane sample.
    let mut rng = NavRng::new(7);
    assert!(matches!(
        draw_spawn(
            &g,
            &NavOverrides::default(),
            &eligible,
            &r,
            &mut rng,
            &[CLEAR],
            &[[0.0, 0.0, 0.0]],
            &SpawnPolicy::default(),
        ),
        SpawnDraw::Placed(_)
    ));
}

/// The plan's own placements join the occupied set: two directives
/// sharing a lane keep `spawn_clearance` between them — the
/// spawn-vs-spawn leg.
#[test]
fn plan_keeps_same_lane_spawns_clear_of_each_other() {
    let g = two_roads();
    let r = roster(&[("va_a", 1.0)]);
    let policy = SpawnPolicy {
        max_active: 24,
        ..SpawnPolicy::default()
    };
    let plan = plan_ambient(&g, &NavOverrides::default(), &r, 5, 1.0, &[CLEAR], &policy);
    assert!(
        plan.spawns.len() >= 4,
        "the fixture should place several cars: {}",
        plan.spawns.len()
    );
    for (i, a) in plan.spawns.iter().enumerate() {
        for b in &plan.spawns[i + 1..] {
            if a.lane == b.lane {
                let d = (a.along - b.along).abs();
                assert!(
                    d >= policy.spawn_clearance,
                    "same-lane spawns {d} m apart: {a:?} vs {b:?}"
                );
            }
        }
    }
}

/// A saturated lane accepts only what fits: with an exclusion box
/// covering a whole lane, at most one spawn survives per lane and the
/// overflow drops — the occupancy rejection lands in `dropped`, not
/// in stacked cars.
#[test]
fn plan_drops_directives_past_a_lane_s_capacity() {
    // One road, two independent lanes (7.5 m apart, lateral bound 2 m).
    let g = NavGraph::build(&bai(vec![road_x(0, 0.0, 0)])).graph;
    let r = roster(&[("va_a", 1.0)]);
    let policy = SpawnPolicy {
        max_active: 6,
        spawn_clearance: 500.0,
        ..SpawnPolicy::default()
    };
    let plan = plan_ambient(&g, &NavOverrides::default(), &r, 5, 1.0, &[CLEAR], &policy);
    assert_eq!(
        plan.spawns.len() + plan.dropped,
        plan.target,
        "every directive is accounted: {:?}",
        plan.spawns
    );
    // A 500 m box on a 100 m lane admits at most one car per lane —
    // two lanes, never three cars.
    assert!(plan.spawns.len() <= 2, "{:?}", plan.spawns);
    assert!(plan.dropped > 0, "the overflow must drop");
}

// ---------- authored signal markers + aspects (F10-B.7) ----------

/// A connected end carrying the authored signal marker pair.
fn signal_end(
    intersection: u32,
    road_index: u32,
    rule: u16,
    origin: [f32; 3],
    axis: [f32; 3],
) -> RoadEnd {
    RoadEnd {
        vehicle_rule_code: rule,
        traffic_light_origin: origin,
        traffic_light_axis: axis,
        ..connected(intersection, road_index)
    }
}

/// `chain_with_rules` with authored signal data on road 0's
/// junction-facing end.
fn chain_with_signal(r0_end: RoadEnd, r1_start: u16) -> NavGraph {
    let r0 = road_full(
        0,
        &[[0.0, 0.0, 0.0], [0.0, 0.0, 100.0]],
        1,
        dead_end(),
        r0_end,
    );
    let r1 = road_full(
        1,
        &[[0.0, 0.0, 100.0], [0.0, 0.0, 200.0]],
        1,
        connected_rule(0, 1, r1_start),
        dead_end(),
    );
    NavGraph::build(&bai_full(
        vec![r0, r1],
        vec![Intersection {
            id: 0,
            room: 1,
            center: [0.0, 0.0, 100.0],
            roads: vec![0, 1],
        }],
    ))
    .graph
}

/// A nonzero authored origin lands verbatim on the arc exiting that
/// end — forward arcs read `road.end`, backward arcs read
/// `road.start`. Zero or non-finite origins expose no signal, and a
/// non-finite axis is normalised to zero rather than propagated.
#[test]
fn exit_light_carries_the_authored_marker_verbatim() {
    let origin = [2.5, 6.8, 97.0];
    let axis = [0.0, 1.0, 0.0];
    let g = chain_with_signal(signal_end(0, 0, 1, origin, axis), 1);
    let fwd = g
        .arc_of(0, TravelDir::Forward)
        .expect("road 0 forward arcs");
    let light = g.arc(fwd).exit_light.expect("the end authored a light");
    assert_eq!(light.origin, origin);
    assert_eq!(light.axis, axis);
    // Road 1's backward arc exits at its start, which authored no
    // light — and road 0's backward exit is a dead end.
    let back = g.arc_of(1, TravelDir::Backward).expect("road 1 backward");
    assert_eq!(g.arc(back).exit_light, None);
    let dead = g.arc_of(0, TravelDir::Backward).expect("road 0 backward");
    assert_eq!(g.arc(dead).exit_light, None);

    // A zero origin means "no light" even when the axis is set (R3).
    let g = chain_with_signal(signal_end(0, 0, 1, [0.0; 3], axis), 1);
    let fwd = g.arc_of(0, TravelDir::Forward).unwrap();
    assert_eq!(g.arc(fwd).exit_light, None);
    // A non-finite origin is junk, not a signal at the world centre.
    let g = chain_with_signal(signal_end(0, 0, 1, [f32::NAN, 0.0, 0.0], axis), 1);
    let fwd = g.arc_of(0, TravelDir::Forward).unwrap();
    assert_eq!(g.arc(fwd).exit_light, None);
    // A non-finite axis cannot reach the consumer — it is zeroed.
    let g = chain_with_signal(signal_end(0, 0, 1, origin, [f32::NAN; 3]), 1);
    let fwd = g.arc_of(0, TravelDir::Forward).unwrap();
    assert_eq!(
        g.arc(fwd).exit_light,
        Some(NavSignal {
            origin,
            axis: [0.0; 3]
        })
    );
}

/// `NavGraph::signals` is the *full* authored head set, not just the
/// arc exits: a lit end on a one-way road's upstream end and a lit
/// end on a road with no vehicle arcs at all both list — the
/// original draws the head it authored. A lit end whose junction
/// reference does not resolve has nothing to govern and stays out.
#[test]
fn signals_lists_every_lit_resolved_end() {
    let lit = |ix: u32, ri: u32, rule: u16, origin: [f32; 3]| {
        signal_end(ix, ri, rule, origin, [0.0, 1.0, 0.0])
    };
    // Road 0: two-way, lit `end` → its head is a forward arc exit.
    let r0 = road_full(
        0,
        &[[0.0, 0.0, 0.0], [0.0, 0.0, -50.0]],
        1,
        RoadEnd {
            // Lit but unconnected — nothing to govern.
            traffic_light_origin: [9.0, 9.0, 9.0],
            traffic_light_axis: [0.0, 1.0, 0.0],
            ..dead_end()
        },
        lit(0, 0, 1, [1.0, 5.0, -1.0]),
    );
    // Road 1: right side carries vehicles, left is pedestrians-only —
    // only the forward arc exists, so a lit `start` is an entry-only
    // end no arc ever exits.
    let c1 = [[20.0, 0.0, 0.0], [20.0, 0.0, -50.0]];
    let r1 = Road {
        id: 1,
        flags: 0,
        rooms: vec![1],
        half_width: 7.5,
        base_speed: 15.0,
        right: side(0, &[(3.75, offset(&c1, 3.75, 0.0))], 2),
        left: side(1, &[(3.75, offset(&c1, -3.75, 0.0))], 2),
        sections: cum(&c1)
            .iter()
            .enumerate()
            .map(|(i, d)| section(*d, c1[i], [0.0, 0.0, 1.0]))
            .collect(),
        start: lit(0, 1, 3, [21.0, 5.0, -1.0]),
        end: dead_end(),
    };
    // Road 2: pedestrians-only on both sides — no arcs at all — yet
    // its lit `end` still faces the junction.
    let c2 = [[-20.0, 0.0, 0.0], [-20.0, 0.0, -50.0]];
    let r2 = Road {
        id: 2,
        flags: 0,
        rooms: vec![1],
        half_width: 7.5,
        base_speed: 15.0,
        right: side(1, &[(3.75, offset(&c2, 3.75, 0.0))], 2),
        left: side(1, &[(3.75, offset(&c2, -3.75, 0.0))], 2),
        sections: cum(&c2)
            .iter()
            .enumerate()
            .map(|(i, d)| section(*d, c2[i], [0.0, 0.0, 1.0]))
            .collect(),
        start: dead_end(),
        end: lit(0, 2, 1, [-19.0, 5.0, -1.0]),
    };
    let g = NavGraph::build(&bai_full(
        vec![r0, r1, r2],
        vec![Intersection {
            id: 0,
            room: 1,
            center: [0.0, 0.0, 0.0],
            roads: vec![0, 1, 2],
        }],
    ))
    .graph;

    // The fixture's arc shape: road 1 is one-way forward, road 2
    // carries no vehicle arcs.
    assert!(g.arc_of(1, TravelDir::Forward).is_some());
    assert!(g.arc_of(1, TravelDir::Backward).is_none());
    assert!(g.arc_of(2, TravelDir::Forward).is_none());
    assert!(g.arc_of(2, TravelDir::Backward).is_none());

    let sigs = g.signals();
    assert_eq!(sigs.len(), 3, "every resolved lit end lists: {sigs:?}");
    let at = |road: u16| sigs.iter().find(|s| s.road == road).expect("head");
    assert_eq!(
        at(0),
        &EndSignal {
            road: 0,
            junction: 0,
            rule_code: 1,
            signal: NavSignal {
                origin: [1.0, 5.0, -1.0],
                axis: [0.0, 1.0, 0.0],
            },
        }
    );
    assert_eq!(at(1).rule_code, 3, "road 1's entry-only head lists");
    assert_eq!(at(2).rule_code, 1, "road 2's arc-less head lists");
    // Only the real arc exit joins the signal cycle; the non-approach
    // heads resolve the green fallback like `gate`'s unruled ends.
    assert_eq!(Junctions::signal_members(&g, 0), vec![0]);
}

/// `signal_aspect` mirrors `gate`'s rule admission without the box
/// yield: a light member is green exactly while it holds the phase
/// (red through the all-red clearance), free-flow ends stay green,
/// stop-signed ends show the stop aspect, `AlwaysStop` stays red, and
/// a light-coded road outside the member set resolves green like
/// `gate`'s unruled fallback.
#[test]
fn signal_aspect_follows_the_authoritative_phase() {
    let g = chain_with_rules(1, 1);
    let members = Junctions::signal_members(&g, 0);
    assert_eq!(members, vec![0, 1]);
    let mut j = Junctions::default();
    j.policy.green_ticks = 10;
    j.policy.clear_ticks = 5;
    let light = Some(VehicleRule::TrafficLight);

    let mut saw_all_red = false;
    for _ in 0..30 {
        let green = j.green_road(&g, 0);
        let a0 = j.signal_aspect(0, &members, 0, light);
        let a1 = j.signal_aspect(0, &members, 1, light);
        match green {
            Some(0) => assert_eq!((a0, a1), (SignalAspect::Green, SignalAspect::Red)),
            Some(1) => assert_eq!((a0, a1), (SignalAspect::Red, SignalAspect::Green)),
            None => {
                saw_all_red = true;
                assert_eq!((a0, a1), (SignalAspect::Red, SignalAspect::Red));
            }
            other => panic!("a non-member held the phase: {other:?}"),
        }
        // The non-light aspects are phase-independent.
        assert_eq!(
            j.signal_aspect(0, &members, 0, Some(VehicleRule::NeverStop)),
            SignalAspect::Green
        );
        assert_eq!(
            j.signal_aspect(0, &members, 0, Some(VehicleRule::StopSign)),
            SignalAspect::Stop
        );
        assert_eq!(
            j.signal_aspect(0, &members, 0, Some(VehicleRule::AlwaysStop)),
            SignalAspect::Red
        );
        assert_eq!(j.signal_aspect(0, &members, 0, None), SignalAspect::Green);
        // A light-coded end outside the member set resolves green —
        // the same fallback `gate` applies.
        assert_eq!(j.signal_aspect(0, &members, 9, light), SignalAspect::Green);
        j.advance_tick();
    }
    assert!(saw_all_red, "the clearance slice must red every member");
}
