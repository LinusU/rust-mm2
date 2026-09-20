//! F09-B nav-graph tests: directed arcs, lane sampling, elevation-aware
//! nearest-lane queries, turn connections and bounded routing over
//! synthetic BAI fixtures — straight, curved, one-way, intersection,
//! dead-end and multilevel cases (F09-AC01/AC03/AC05/AC06).

use mm2_formats::bai::{
    Bai, Culling, END_FILL, Intersection, Road, RoadEnd, RoadSection, RoadSide,
};
use mm2_game::*;

// ---------- synthetic BAI fixtures ----------

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

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > f32::EPSILON {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 0.0, 1.0]
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

/// Build one road side from `(edge-distance, vertices)` lane and
/// sidewalk curves; `ambient` is the raw ambientTypes code.
fn side(
    ambient: u16,
    lanes: &[(f32, Vec<[f32; 3]>)],
    sidewalks: &[(f32, Vec<[f32; 3]>)],
    nsec: usize,
) -> RoadSide {
    let curves: Vec<&(f32, Vec<[f32; 3]>)> = lanes.iter().chain(sidewalks.iter()).collect();
    RoadSide {
        lane_count: lanes.len() as u16,
        tram_count: 0,
        train_count: 0,
        sidewalk_count: sidewalks.len() as u16,
        ambient_types: ambient,
        lane_distances: curves.iter().map(|(_, v)| cum(v)).collect(),
        edge_distances: curves.iter().map(|(e, _)| *e).collect(),
        misc: [0xCD; 40],
        lane_vertices: curves.iter().map(|(_, v)| v.clone()).collect(),
        tram_vertices: Vec::new(),
        train_vertices: Vec::new(),
        sidewalk_inner: vec![[0.0; 3]; nsec],
        sidewalk_outer: vec![[0.0; 3]; nsec],
    }
}

fn dead_end() -> RoadEnd {
    RoadEnd {
        intersection: 0,
        fill0: 0xCDCD,
        vehicle_rule_code: 0,
        unknown1: 0,
        intersection_road_index: END_FILL,
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

/// A road along `pts` (the centre line) with `rooms`, lane geometry on
/// each side, and junction records for both extremities.
fn road(
    id: u16,
    pts: &[[f32; 3]],
    rooms: Vec<u16>,
    right: RoadSide,
    left: RoadSide,
    start: RoadEnd,
    end: RoadEnd,
) -> Road {
    let dists = cum(pts);
    let sections: Vec<RoadSection> = pts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let t = if i + 1 < pts.len() {
                norm([
                    pts[i + 1][0] - p[0],
                    pts[i + 1][1] - p[1],
                    pts[i + 1][2] - p[2],
                ])
            } else if i > 0 {
                norm([
                    p[0] - pts[i - 1][0],
                    p[1] - pts[i - 1][1],
                    p[2] - pts[i - 1][2],
                ])
            } else {
                [0.0, 0.0, 1.0]
            };
            section(dists[i], *p, t)
        })
        .collect();
    Road {
        id,
        flags: 0,
        rooms,
        half_width: 7.5,
        base_speed: 15.0,
        right,
        left,
        sections,
        end,
        start,
    }
}

fn offset(pts: &[[f32; 3]], dx: f32, dz: f32) -> Vec<[f32; 3]> {
    pts.iter().map(|p| [p[0] + dx, p[1], p[2] + dz]).collect()
}

fn intersection(id: u16, center: [f32; 3], roads: &[u32]) -> Intersection {
    Intersection {
        id,
        room: 1,
        center,
        roads: roads.to_vec(),
    }
}

fn bai(roads: Vec<Road>, intersections: Vec<Intersection>) -> Bai {
    Bai {
        roads,
        intersections,
        culling: Culling {
            large: vec![Vec::new()],
            small: vec![Vec::new()],
        },
    }
}

/// One straight road along +z with one lane per side offset ±3.75 x.
fn straight() -> Bai {
    let centre = [[0.0, 0.0, 0.0], [0.0, 0.0, 100.0]];
    let right = side(0, &[(3.75, offset(&centre, 3.75, 0.0))], &[], 2);
    let left = side(0, &[(-3.75, offset(&centre, -3.75, 0.0))], &[], 2);
    bai(
        vec![road(
            0,
            &centre,
            vec![1],
            right,
            left,
            dead_end(),
            dead_end(),
        )],
        Vec::new(),
    )
}

/// A four-arm cross: roads 0..3 are the south/east/north/west arms of
/// an intersection at the origin, in that order in the road list.
/// Every road carries `lanes_per_side` lanes on both sides.
fn cross(lanes_per_side: usize) -> Bai {
    let arms: [&[[f32; 3]]; 4] = [
        &[[0.0, 0.0, -30.0], [0.0, 0.0, -4.0]], // 0 south → ends at int
        &[[4.0, 0.0, 0.0], [30.0, 0.0, 0.0]],   // 1 east  → starts at int
        &[[0.0, 0.0, 30.0], [0.0, 0.0, 4.0]],   // 2 north → ends at int
        &[[-30.0, 0.0, 0.0], [-4.0, 0.0, 0.0]], // 3 west  → ends at int
    ];
    // Per-arm lane offsets so lanes stay on their authored side.
    let lane_offsets: Vec<f32> = (0..lanes_per_side).map(|k| 1.5 + k as f32 * 2.25).collect();
    let mut roads = Vec::new();
    for (i, pts) in arms.iter().enumerate() {
        // x offsets for z-running arms, z offsets for x-running arms.
        let (axis, sign_r, sign_l) = if i % 2 == 0 {
            (0usize, 1.0f32, -1.0f32)
        } else {
            (2usize, -1.0f32, 1.0f32)
        };
        let mk = |sign: f32| -> Vec<(f32, Vec<[f32; 3]>)> {
            lane_offsets
                .iter()
                .map(|o| {
                    let v = if axis == 0 {
                        offset(pts, sign * o, 0.0)
                    } else {
                        offset(pts, 0.0, sign * o)
                    };
                    (*o, v)
                })
                .collect()
        };
        let right = side(0, &mk(sign_r), &[], 2);
        let left = side(0, &mk(sign_l), &[], 2);
        // Ends wired: arm ends at the intersection except road1 whose
        // start does (arm 3's start is a dead end — the road runs into
        // the junction on its end).
        let (start, end) = match i {
            1 => (connected(0, 1), dead_end()),
            _ => (dead_end(), connected(0, i as u32)),
        };
        roads.push(road(i as u16, pts, vec![1], right, left, start, end));
    }
    bai(roads, vec![intersection(0, [0.0; 3], &[0, 1, 2, 3])])
}

fn lane(road: u16, s: Side, index: u16) -> LaneId {
    LaneId {
        road,
        side: s,
        index,
        kind: LaneKind::Vehicle,
    }
}

use mm2_formats::bai::Side;

fn approx(a: [f32; 3], b: [f32; 3], eps: f32) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= eps)
}

// ---------- tests ----------

#[test]
fn straight_road_yields_two_directed_arcs() {
    let build = NavGraph::build(&straight());
    assert!(build.issues.is_empty(), "{:?}", build.issues);
    let g = &build.graph;
    assert_eq!(g.stats().vehicle_arcs, 2);
    assert!(!g.road(0).unwrap().one_way);

    // Right-side lane travels Forward (+z); left-side Backward (-z).
    let rf = g.arc_of(0, TravelDir::Forward).unwrap();
    let rb = g.arc_of(0, TravelDir::Backward).unwrap();
    assert_eq!(g.arc(rf).side, Side::Right);
    assert_eq!(g.arc(rb).side, Side::Left);

    let s = g.sample_lane(lane(0, Side::Right, 0), 50.0).unwrap();
    assert!(approx(s.position, [3.75, 0.0, 50.0], 0.01));
    assert!(approx(s.tangent, [0.0, 0.0, 1.0], 0.01));
    let s = g.sample_lane(lane(0, Side::Left, 0), 50.0).unwrap();
    assert!(approx(s.position, [-3.75, 0.0, 50.0], 0.01));
    assert!(approx(s.tangent, [0.0, 0.0, -1.0], 0.01));
}

#[test]
fn curved_lane_samples_along_the_curve() {
    // Quarter-circle centre line in the XZ plane, radius 20.
    let n = 9;
    let centre: Vec<[f32; 3]> = (0..n)
        .map(|i| {
            let a = std::f32::consts::FRAC_PI_2 * i as f32 / (n - 1) as f32;
            [20.0 * a.cos(), 0.0, 20.0 * a.sin()]
        })
        .collect();
    let lane_pts: Vec<[f32; 3]> = (0..n)
        .map(|i| {
            let a = std::f32::consts::FRAC_PI_2 * i as f32 / (n - 1) as f32;
            [17.0 * a.cos(), 0.0, 17.0 * a.sin()]
        })
        .collect();
    let right = side(0, &[(3.0, lane_pts)], &[], n);
    let left = side(0, &[], &[], n);
    let b = bai(
        vec![road(
            0,
            &centre,
            vec![1],
            right,
            left,
            dead_end(),
            dead_end(),
        )],
        Vec::new(),
    );
    let g = NavGraph::build(&b).graph;
    let l = g.lane(lane(0, Side::Right, 0)).unwrap();
    // Mid-arc sample sits near 45° on the curve, tangent diagonal.
    let s = g.sample_lane(l.id, l.length / 2.0).unwrap();
    let r = (s.position[0].powi(2) + s.position[2].powi(2)).sqrt();
    assert!((r - 17.0).abs() < 0.2, "radius {r}");
    assert!(s.tangent[0] < -0.5 && s.tangent[2] > 0.5, "{:?}", s.tangent);
}

#[test]
fn one_way_road_yields_a_single_arc() {
    let centre = [[0.0, 0.0, 0.0], [0.0, 0.0, 100.0]];
    let right = side(0, &[(3.75, offset(&centre, 3.75, 0.0))], &[], 2);
    let left = side(0, &[], &[], 2); // no lanes on the left side
    let b = bai(
        vec![road(
            0,
            &centre,
            vec![1],
            right,
            left,
            dead_end(),
            dead_end(),
        )],
        Vec::new(),
    );
    let build = NavGraph::build(&b);
    let g = &build.graph;
    assert!(g.arc_of(0, TravelDir::Forward).is_some());
    assert!(g.arc_of(0, TravelDir::Backward).is_none());
    assert!(g.road(0).unwrap().one_way);
}

#[test]
fn ambient_types_forbid_vehicle_arcs() {
    let centre = [[0.0, 0.0, 0.0], [0.0, 0.0, 100.0]];
    let mk = |amb_r: u16, amb_l: u16| {
        let right = side(amb_r, &[(3.75, offset(&centre, 3.75, 0.0))], &[], 2);
        let left = side(amb_l, &[(-3.75, offset(&centre, -3.75, 0.0))], &[], 2);
        bai(
            vec![road(
                0,
                &centre,
                vec![1],
                right,
                left,
                dead_end(),
                dead_end(),
            )],
            Vec::new(),
        )
    };
    // PedestriansOnly left side → forward-only one-way.
    let g = NavGraph::build(&mk(0, 1)).graph;
    assert!(g.arc_of(0, TravelDir::Forward).is_some());
    assert!(g.arc_of(0, TravelDir::Backward).is_none());
    // Disabled both sides → no arcs, reported.
    let build = NavGraph::build(&mk(3, 3));
    assert_eq!(build.graph.stats().vehicle_arcs, 0);
    assert!(
        build
            .issues
            .iter()
            .any(|i| matches!(i, NavIssue::NoVehicleLanes { .. }))
    );
}

#[test]
fn cross_intersection_turn_kinds_and_no_uturn() {
    let g = NavGraph::build(&cross(1)).graph;
    let entry = g.arc_of(0, TravelDir::Forward).unwrap();
    let exits = g.exits(entry);
    // Road 0 forward arrives heading +z. Departures: road1 +x (left in
    // the coordinate convention), road2 +z (straight), road3 −x (right).
    assert_eq!(exits.len(), 3, "{exits:?}");
    let kinds: Vec<(u16, TurnKind)> = exits.iter().map(|e| (g.arc(e.to).road, e.turn)).collect();
    assert!(kinds.contains(&(1, TurnKind::Left)), "{kinds:?}");
    assert!(kinds.contains(&(2, TurnKind::Straight)), "{kinds:?}");
    assert!(kinds.contains(&(3, TurnKind::Right)), "{kinds:?}");
    // No U-turn back onto road 0.
    assert!(!exits.iter().any(|e| g.arc(e.to).road == 0));
    // Counterclockwise deltas are the authored index differences.
    let delta: Vec<u8> = exits.iter().map(|e| e.ccw_delta).collect();
    assert_eq!(delta, vec![1, 2, 3]);
}

#[test]
fn lane_position_governs_legal_turns() {
    let g = NavGraph::build(&cross(3)).graph;
    let roads_of = |lane: LaneId| -> Vec<u16> {
        g.legal_exits(lane)
            .iter()
            .map(|e| g.arc(e.to).road)
            .collect()
    };
    // Right-side travel: innermost (index 0) → left or straight;
    // middle → straight only; outermost → right or straight.
    let mut inner = roads_of(lane(0, Side::Right, 0));
    inner.sort();
    assert_eq!(inner, vec![1, 2]);
    assert_eq!(roads_of(lane(0, Side::Right, 1)), vec![2]);
    let mut outer = roads_of(lane(0, Side::Right, 2));
    outer.sort();
    assert_eq!(outer, vec![2, 3]);
}

#[test]
fn t_junction_falls_back_to_straightest_exit() {
    // Entry road heading +z into a T: exits go ±x only.
    let arms: [&[[f32; 3]]; 3] = [
        &[[0.0, 0.0, -30.0], [0.0, 0.0, -4.0]],
        &[[4.0, 0.0, 0.0], [30.0, 0.0, 0.0]],
        &[[-30.0, 0.0, 0.0], [-4.0, 0.0, 0.0]],
    ];
    let mut roads = Vec::new();
    for (i, pts) in arms.iter().enumerate() {
        let lanes: Vec<(f32, Vec<[f32; 3]>)> = [1.5, 3.75, 6.0]
            .iter()
            .map(|o| {
                let v = if i == 0 {
                    offset(pts, *o, 0.0)
                } else {
                    offset(pts, 0.0, -*o)
                };
                (*o, v)
            })
            .collect();
        let right = side(0, &lanes, &[], 2);
        let left = side(0, &[], &[], 2);
        let (start, end) = match i {
            1 => (connected(0, 1), dead_end()),
            _ => (dead_end(), connected(0, i as u32)),
        };
        roads.push(road(i as u16, pts, vec![1], right, left, start, end));
    }
    let g = NavGraph::build(&bai(roads, vec![intersection(0, [0.0; 3], &[0, 1, 2])])).graph;
    // The middle lane may only go straight, but no straight exit exists
    // — the fallback returns the single straightest turn instead of
    // stranding the vehicle.
    let exits = g.legal_exits(lane(0, Side::Right, 1));
    assert_eq!(exits.len(), 1);
}

#[test]
fn dead_ends_are_reported_not_invented() {
    let g = NavGraph::build(&straight()).graph;
    assert_eq!(g.stats().dead_ends, 2);
    let f = g.arc_of(0, TravelDir::Forward).unwrap();
    assert_eq!(g.arc(f).exit, ArcEnd::DeadEnd);
    assert!(g.exits(f).is_empty());
}

#[test]
fn unresolved_end_degrades_to_dead_end() {
    // The end claims a connection but its index points at a different
    // road inside the intersection.
    let centre = [[0.0, 0.0, 0.0], [0.0, 0.0, 100.0]];
    let right = side(0, &[(3.75, offset(&centre, 3.75, 0.0))], &[], 2);
    let left = side(0, &[], &[], 2);
    let r = road(
        0,
        &centre,
        vec![1],
        right,
        left,
        dead_end(),
        connected(0, 1), // claims slot 1 — slot 0 holds a different road
    );
    let centre2 = [[100.0, 0.0, 0.0], [100.0, 0.0, 100.0]];
    let right2 = side(0, &[(3.75, offset(&centre2, 3.75, 0.0))], &[], 2);
    let left2 = side(0, &[], &[], 2);
    let r2 = road(1, &centre2, vec![1], right2, left2, dead_end(), dead_end());
    let b = bai(vec![r, r2], vec![intersection(0, [0.0; 3], &[1])]);
    let build = NavGraph::build(&b);
    assert!(build.issues.iter().any(|i| matches!(
        i,
        NavIssue::UnresolvedEnd {
            road: 0,
            end: mm2_formats::bai::End::End
        }
    )));
    let f = build.graph.arc_of(0, TravelDir::Forward).unwrap();
    assert_eq!(build.graph.arc(f).exit, ArcEnd::DeadEnd);
}

#[test]
fn routes_cross_intersections_and_respect_direction() {
    // road0 south arm → int → road1 east arm (one-way, forward only).
    let centre0 = [[0.0, 0.0, -30.0], [0.0, 0.0, -4.0]];
    let centre1 = [[4.0, 0.0, 0.0], [30.0, 0.0, 0.0]];
    let r0 = road(
        0,
        &centre0,
        vec![1],
        side(0, &[(3.75, offset(&centre0, 3.75, 0.0))], &[], 2),
        side(0, &[(-3.75, offset(&centre0, -3.75, 0.0))], &[], 2),
        dead_end(),
        connected(0, 0),
    );
    let r1 = road(
        1,
        &centre1,
        vec![1],
        side(0, &[(3.75, offset(&centre1, 0.0, -3.75))], &[], 2),
        side(0, &[], &[], 2), // one-way eastbound
        connected(0, 1),
        dead_end(),
    );
    let b = bai(vec![r0, r1], vec![intersection(0, [0.0; 3], &[0, 1])]);
    let g = NavGraph::build(&b).graph;

    let route = g
        .route(
            [3.0, 0.0, -20.0],
            [20.0, 0.0, -2.0],
            &RouteOptions::default(),
        )
        .unwrap();
    let steps: Vec<u16> = route.steps.iter().map(|a| g.arc(*a).road).collect();
    assert_eq!(steps, vec![0, 1]);
    assert_eq!(g.arc(route.steps[0]).dir, TravelDir::Forward);
    assert_eq!(g.arc(route.steps[1]).dir, TravelDir::Forward);

    // The reverse trip is impossible: road 1 has no inbound arc, so the
    // search frontier empties into a specific failure.
    let err = g
        .route(
            [20.0, 0.0, -2.0],
            [3.0, 0.0, -20.0],
            &RouteOptions::default(),
        )
        .unwrap_err();
    assert!(matches!(err, RouteError::Unreachable { .. }), "{err}");
}

#[test]
fn disconnected_islands_fail_specifically() {
    // Two parallel strips with no shared intersection.
    let a = [[0.0, 0.0, 0.0], [0.0, 0.0, 50.0]];
    let bpts = [[100.0, 0.0, 0.0], [100.0, 0.0, 50.0]];
    let mk = |id: u16, c: &[[f32; 3]]| {
        road(
            id,
            c,
            vec![1],
            side(0, &[(3.75, offset(c, 3.75, 0.0))], &[], 2),
            side(0, &[(-3.75, offset(c, -3.75, 0.0))], &[], 2),
            dead_end(),
            dead_end(),
        )
    };
    let b = bai(vec![mk(0, &a), mk(1, &bpts)], Vec::new());
    let g = NavGraph::build(&b).graph;
    assert_eq!(g.stats().components, 2);
    let err = g
        .route(
            [0.0, 0.0, 25.0],
            [100.0, 0.0, 25.0],
            &RouteOptions::default(),
        )
        .unwrap_err();
    assert!(matches!(err, RouteError::Unreachable { .. }));
    // And the snap radius itself bounds failures.
    let err = g
        .route(
            [0.0, 0.0, 25.0],
            [1000.0, 0.0, 0.0],
            &RouteOptions::default(),
        )
        .unwrap_err();
    assert_eq!(err, RouteError::NoGoalLane);
}

#[test]
fn closed_roads_are_never_entered() {
    // Chain r0 → I0 → r1 → I1 → r2.
    let c0 = [[0.0, 0.0, -30.0], [0.0, 0.0, -4.0]];
    let c1 = [[0.0, 0.0, 4.0], [0.0, 0.0, 30.0]];
    let c2 = [[0.0, 0.0, 34.0], [0.0, 0.0, 60.0]];
    let mk = |id: u16, c: &[[f32; 3]], start: RoadEnd, end: RoadEnd| {
        road(
            id,
            c,
            vec![1],
            side(0, &[(3.75, offset(c, 3.75, 0.0))], &[], 2),
            side(0, &[(-3.75, offset(c, -3.75, 0.0))], &[], 2),
            start,
            end,
        )
    };
    let b = bai(
        vec![
            mk(0, &c0, dead_end(), connected(0, 0)),
            mk(1, &c1, connected(0, 1), connected(1, 0)),
            mk(2, &c2, connected(1, 1), dead_end()),
        ],
        vec![
            intersection(0, [0.0; 3], &[0, 1]),
            intersection(1, [0.0, 0.0, 32.0], &[1, 2]),
        ],
    );
    let g = NavGraph::build(&b).graph;
    let open = g.route(
        [3.0, 0.0, -20.0],
        [3.0, 0.0, 50.0],
        &RouteOptions::default(),
    );
    assert_eq!(open.unwrap().steps.len(), 3);
    let mut opts = RouteOptions::default();
    opts.closed_roads.insert(1);
    let err = g
        .route([3.0, 0.0, -20.0], [3.0, 0.0, 50.0], &opts)
        .unwrap_err();
    assert!(matches!(err, RouteError::Unreachable { .. }));
}

#[test]
fn route_search_is_bounded() {
    // Chain r0 → I0 → r1: a zero-expansion bound permits only
    // same-arc routes, so the two-arc route must fail.
    let c0 = [[0.0, 0.0, -30.0], [0.0, 0.0, -4.0]];
    let c1 = [[0.0, 0.0, 4.0], [0.0, 0.0, 30.0]];
    let mk = |id: u16, c: &[[f32; 3]], start: RoadEnd, end: RoadEnd| {
        road(
            id,
            c,
            vec![1],
            side(0, &[(3.75, offset(c, 3.75, 0.0))], &[], 2),
            side(0, &[], &[], 2),
            start,
            end,
        )
    };
    let b = bai(
        vec![
            mk(0, &c0, dead_end(), connected(0, 0)),
            mk(1, &c1, connected(0, 1), dead_end()),
        ],
        vec![intersection(0, [0.0; 3], &[0, 1])],
    );
    let g = NavGraph::build(&b).graph;
    let err = g
        .route(
            [3.0, 0.0, -20.0],
            [3.0, 0.0, 20.0],
            &RouteOptions {
                max_expansions: 0,
                ..RouteOptions::default()
            },
        )
        .unwrap_err();
    assert!(matches!(err, RouteError::ExpansionLimit { .. }));
}

#[test]
fn stacked_levels_snap_by_height_not_proximity() {
    // Ground road at y=0, bridge road at y=10 — same XZ, different
    // rooms, no shared intersection.
    let cg = [[0.0, 0.0, -20.0], [0.0, 0.0, 20.0]];
    let cb = [[0.0, 10.0, -20.0], [0.0, 10.0, 20.0]];
    let mk = |id: u16, c: &[[f32; 3]], room: u16| {
        road(
            id,
            c,
            vec![room],
            side(0, &[(3.75, offset(c, 3.75, 0.0))], &[], 2),
            side(0, &[(-3.75, offset(c, -3.75, 0.0))], &[], 2),
            dead_end(),
            dead_end(),
        )
    };
    let b = bai(vec![mk(0, &cg, 1), mk(1, &cb, 2)], Vec::new());
    let g = NavGraph::build(&b).graph;

    let up = g
        .nearest_lane([1.0, 10.0, 0.0], &LaneQuery::vehicles(8.0))
        .unwrap();
    assert_eq!(up.lane.road, 1);
    let down = g
        .nearest_lane([1.0, 0.0, 0.0], &LaneQuery::vehicles(8.0))
        .unwrap();
    assert_eq!(down.lane.road, 0);
    // A room hint resolves the genuinely ambiguous mid-height case.
    let hinted = g
        .nearest_lane([1.0, 5.0, 0.0], &LaneQuery::vehicles(8.0).in_rooms([1]))
        .unwrap();
    assert_eq!(hinted.lane.road, 0);
    // And the two levels do not route into each other.
    let err = g
        .route([0.0, 0.0, 0.0], [0.0, 10.0, 0.0], &RouteOptions::default())
        .unwrap_err();
    assert!(matches!(err, RouteError::Unreachable { .. }));
}

#[test]
fn two_cursors_share_one_graph_independently() {
    // Route down the cross's south → east arms; two consumers advance
    // different distances and keep their own lane/step state.
    let g = NavGraph::build(&cross(2)).graph;
    let route = g
        .route(
            [3.0, 0.0, -20.0],
            [20.0, 0.0, -2.0],
            &RouteOptions::default(),
        )
        .unwrap();
    let mut c1 = g.cursor(&route);
    let mut c2 = g.cursor(&route);
    g.advance_cursor(&mut c1, &route, 10.0);
    g.advance_cursor(&mut c2, &route, 80.0);
    assert_eq!(c1.step, 0);
    assert!(c2.step >= 1, "cursor 2 should be on the second arc");
    let p1 = g.cursor_sample(&c1).unwrap().position;
    let p2 = g.cursor_sample(&c2).unwrap().position;
    assert!(!approx(p1, p2, 0.5));
    // Advancing one cursor never touched the other.
    assert_eq!(c1.step, 0);
    assert_ne!(c1.distance, c2.distance);
    // A second consumer's route query is unaffected by the cursors.
    let other = g
        .route(
            [3.0, 0.0, -20.0],
            [-3.0, 0.0, 20.0],
            &RouteOptions::default(),
        )
        .unwrap();
    assert!(!other.steps.is_empty());
}

#[test]
fn seeded_exit_choice_is_deterministic() {
    let g = NavGraph::build(&cross(1)).graph;
    let l = lane(0, Side::Right, 0);
    let seq = |seed: u64| -> Vec<u16> {
        let mut rng = NavRng::new(seed);
        (0..12)
            .map(|_| {
                g.choose_exit(l, &mut rng)
                    .map(|e| g.arc(e.to).road)
                    .unwrap_or(u16::MAX)
            })
            .collect()
    };
    assert_eq!(seq(42), seq(42));
    // Single-lane road → all three exits are legal, so distinct seeds
    // walk different pick sequences.
    assert_ne!(seq(1), seq(2));
}

#[test]
fn degenerate_lanes_drop_out_but_do_not_break_the_arc() {
    let centre = [[0.0, 0.0, 0.0], [0.0, 0.0, 100.0]];
    // A zero-length lane next to a good one.
    let right = side(
        0,
        &[
            (3.75, offset(&centre, 3.75, 0.0)),
            (1.5, vec![[0.0, 0.0, 0.0], [0.0, 0.0, 0.0]]),
        ],
        &[],
        2,
    );
    let left = side(0, &[(-3.75, offset(&centre, -3.75, 0.0))], &[], 2);
    let b = bai(
        vec![road(
            0,
            &centre,
            vec![1],
            right,
            left,
            dead_end(),
            dead_end(),
        )],
        Vec::new(),
    );
    let build = NavGraph::build(&b);
    assert!(
        build
            .issues
            .iter()
            .any(|i| matches!(i, NavIssue::DegenerateLane { index: 1, .. }))
    );
    let f = build.graph.arc_of(0, TravelDir::Forward).unwrap();
    assert_eq!(build.graph.arc(f).lanes.len(), 1);
}

#[test]
fn non_monotone_authored_distances_are_recomputed() {
    let centre = [[0.0, 0.0, 0.0], [0.0, 0.0, 100.0]];
    let mut right = side(0, &[(3.75, offset(&centre, 3.75, 0.0))], &[], 2);
    right.lane_distances[0] = vec![100.0, 0.0]; // reversed, non-monotone
    let left = side(0, &[], &[], 2);
    let b = bai(
        vec![road(
            0,
            &centre,
            vec![1],
            right,
            left,
            dead_end(),
            dead_end(),
        )],
        Vec::new(),
    );
    let build = NavGraph::build(&b);
    assert!(
        build
            .issues
            .iter()
            .any(|i| matches!(i, NavIssue::LaneDistancesRecomputed { .. }))
    );
    let s = build
        .graph
        .sample_lane(lane(0, Side::Right, 0), 50.0)
        .unwrap();
    assert!(approx(s.position, [3.75, 0.0, 50.0], 0.01));
}

#[test]
fn elevation_is_preserved_in_sampling() {
    let centre = [[0.0, 0.0, 0.0], [0.0, 10.0, 100.0]];
    let right = side(0, &[(3.75, offset(&centre, 3.75, 0.0))], &[], 2);
    let left = side(0, &[], &[], 2);
    let b = bai(
        vec![road(
            0,
            &centre,
            vec![1],
            right,
            left,
            dead_end(),
            dead_end(),
        )],
        Vec::new(),
    );
    let g = NavGraph::build(&b).graph;
    let s = g.sample_lane(lane(0, Side::Right, 0), 50.0).unwrap();
    assert!((s.position[1] - 5.0).abs() < 0.3, "y={}", s.position[1]);
    assert!(s.tangent[1] > 0.05, "{:?}", s.tangent);
}

#[test]
fn sidewalks_and_rails_are_queryable_but_not_routable() {
    let centre = [[0.0, 0.0, 0.0], [0.0, 0.0, 100.0]];
    let mut right = side(
        0,
        &[(3.75, offset(&centre, 3.75, 0.0))],
        &[(8.0, offset(&centre, 8.0, 0.0))],
        2,
    );
    right.tram_count = 1;
    right.tram_vertices = vec![offset(&centre, 5.0, 0.0)];
    let left = side(0, &[], &[], 2);
    let b = bai(
        vec![road(
            0,
            &centre,
            vec![1],
            right,
            left,
            dead_end(),
            dead_end(),
        )],
        Vec::new(),
    );
    let g = NavGraph::build(&b).graph;
    assert_eq!(g.stats().sidewalk_lanes, 1);
    assert_eq!(g.stats().tram_lanes, 1);
    // Kind-filtered queries find them; routable vehicle queries do not.
    let sw = LaneId {
        road: 0,
        side: Side::Right,
        index: 0,
        kind: LaneKind::Sidewalk,
    };
    assert!(g.lane(sw).is_some());
    let hit = g
        .nearest_lane(
            [8.0, 0.0, 50.0],
            &LaneQuery::of_kind(LaneKind::Sidewalk, 4.0),
        )
        .unwrap();
    assert_eq!(hit.lane.kind, LaneKind::Sidewalk);
    let hit = g
        .nearest_lane([8.0, 0.0, 50.0], &LaneQuery::vehicles(20.0))
        .unwrap();
    assert_eq!(hit.lane.kind, LaneKind::Vehicle);
}
