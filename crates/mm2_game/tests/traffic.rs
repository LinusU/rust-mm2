//! F10-A.1 ambient-traffic tests: cumulative-weight roster selection,
//! seeded spawn planning over synthetic BAI fixtures — closed roads,
//! pedestrian-only sides, the player bubble, unspawnable classes and
//! plan bounds (F10-AC01's data slice).

use mm2_formats::bai::{Bai, Culling, Intersection, Road, RoadSection, RoadSide};
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

const FAR_AWAY: [f32; 3] = [0.0, -500.0, 0.0];

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
        FAR_AWAY,
        &SpawnPolicy::default(),
    );
    let b = plan_ambient(
        &g,
        &NavOverrides::default(),
        &r,
        42,
        0.5,
        FAR_AWAY,
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
        FAR_AWAY,
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
    let full = plan_ambient(&g, &NavOverrides::default(), &r, 1, 1.0, FAR_AWAY, &policy);
    assert_eq!(full.target, 10);
    assert_eq!(full.spawns.len(), 10);
    assert!(full.spawns.len() <= policy.max_active);

    let half = plan_ambient(&g, &NavOverrides::default(), &r, 1, 0.5, FAR_AWAY, &policy);
    assert_eq!(half.target, 5);
    let off = plan_ambient(&g, &NavOverrides::default(), &r, 1, 0.0, FAR_AWAY, &policy);
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
    let plan = plan_ambient(&g, &overrides, &r, 9, 1.0, FAR_AWAY, &policy);
    assert_eq!(plan.eligible_lanes, 2, "only road 1's two lanes");
    assert!(plan.spawns.iter().all(|s| s.lane.road == 1));

    // Pedestrian-only sides never become routable arcs — the graph
    // withholds them, so the planner cannot pick them.
    let g = road1_pedestrian_only();
    let plan = plan_ambient(&g, &NavOverrides::default(), &r, 9, 1.0, FAR_AWAY, &policy);
    assert_eq!(plan.eligible_lanes, 2, "only road 0's two lanes");
    assert!(plan.spawns.iter().all(|s| s.lane.road == 0));

    // Closing every road leaves the plan empty but well-formed.
    let mut overrides = NavOverrides::default();
    overrides.closed_roads.extend([0u16, 1u16]);
    let plan = plan_ambient(&g, &overrides, &r, 9, 1.0, FAR_AWAY, &policy);
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
    let plan = plan_ambient(&g, &NavOverrides::default(), &r, 3, 1.0, FAR_AWAY, &policy);
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
        FAR_AWAY,
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
        [10.0, 0.0, 50.0],
        &policy,
    );
    assert!(plan.spawns.is_empty());
    assert_eq!(plan.dropped, plan.target);

    // With a sane bubble every surviving spawn is outside it.
    let policy = SpawnPolicy::default();
    let player = [10.0, 0.0, 50.0];
    let plan = plan_ambient(&g, &NavOverrides::default(), &r, 5, 1.0, player, &policy);
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

#[test]
fn nav_rng_next_f32_stays_in_unit_range() {
    let mut rng = NavRng::new(0);
    for _ in 0..1000 {
        let u = rng.next_f32();
        assert!((0.0..1.0).contains(&u), "{u}");
    }
}
