//! F09-B nav-graph tests: directed arcs, lane sampling, elevation-aware
//! nearest-lane queries, turn connections and bounded routing over
//! synthetic BAI fixtures — straight, curved, one-way, intersection,
//! dead-end and multilevel cases (F09-AC01/AC03/AC05/AC06).

use bevy::prelude::Vec3;
use mm2_formats::aimap::Aimap;
use mm2_formats::bai::{
    Bai, Culling, END_FILL, Intersection, Road, RoadEnd, RoadSection, RoadSide,
};
use mm2_game::*;
use std::collections::BTreeSet;

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
fn reachable_arcs_follow_only_authored_turns() {
    let g = NavGraph::build(&cross(1)).graph;
    let open = BTreeSet::new();
    // Road 0 forward ends at the junction and may enter any other
    // arm's departing arc; every departure dead-ends, so the walk
    // stops there instead of wandering back.
    let entry = g.arc_of(0, TravelDir::Forward).unwrap();
    let reach = g.reachable_arcs(entry, &open);
    let mut roads: Vec<u16> = reach.iter().map(|a| g.arc(*a).road).collect();
    roads.sort_unstable();
    assert_eq!(roads, vec![0, 1, 2, 3]);
    assert_eq!(reach.len(), 4);
    // The U-turn exclusion holds in reachability: road 0's backward
    // arc departs the junction but is never re-entered from the start.
    let back = g.arc_of(0, TravelDir::Backward).unwrap();
    assert!(!reach.contains(&back));
    // A departing arc exits to a dead end: it reaches only itself.
    let dep = g.arc_of(1, TravelDir::Forward).unwrap();
    assert_eq!(g.reachable_arcs(dep, &open), BTreeSet::from([dep]));
}

/// The r0 → I0 → r1 → I1 → r2 chain both `closed_roads` tests build.
fn chain() -> Bai {
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
    bai(
        vec![
            mk(0, &c0, dead_end(), connected(0, 0)),
            mk(1, &c1, connected(0, 1), connected(1, 0)),
            mk(2, &c2, connected(1, 1), dead_end()),
        ],
        vec![
            intersection(0, [0.0; 3], &[0, 1]),
            intersection(1, [0.0, 0.0, 32.0], &[1, 2]),
        ],
    )
}

#[test]
fn reachable_arcs_shrink_under_road_closures() {
    let g = NavGraph::build(&chain()).graph;
    let open = BTreeSet::new();
    let f0 = g.arc_of(0, TravelDir::Forward).unwrap();
    let f1 = g.arc_of(1, TravelDir::Forward).unwrap();
    let f2 = g.arc_of(2, TravelDir::Forward).unwrap();
    // Open: r0 forward reaches straight down the chain.
    assert_eq!(g.reachable_arcs(f0, &open), BTreeSet::from([f0, f1, f2]));
    let closed = BTreeSet::from([1u16]);
    // A turn onto the closed road is never taken — the same rule
    // `route` applies — while a start already on it still expands.
    assert_eq!(g.reachable_arcs(f0, &closed), BTreeSet::from([f0]));
    assert_eq!(g.reachable_arcs(f1, &closed), BTreeSet::from([f1, f2]));
    // Closures bind both directions: r2 backward cannot turn onto
    // road 1 either.
    let b2 = g.arc_of(2, TravelDir::Backward).unwrap();
    assert_eq!(g.reachable_arcs(b2, &closed), BTreeSet::from([b2]));
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

// ---------- aimap overrides ----------

fn exception(road: u32, density: f32, speed_limit: f32) -> mm2_formats::aimap::RoadException {
    mm2_formats::aimap::RoadException {
        road,
        density,
        speed_limit,
        line: 1,
    }
}

#[test]
fn zero_density_exceptions_close_roads_to_routing() {
    // The chain from `closed_roads_are_never_entered`, with the closure
    // arriving through `NavOverrides` instead of a hand-built set.
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
    let aimap = Aimap {
        exceptions: vec![exception(1, 0.0, 0.0), exception(2, 0.5, 0.0)],
        ..Aimap::default()
    };
    let overrides = NavOverrides::from_aimap(&aimap);
    // Zero density closes road 1; a nonzero density on road 2 does not.
    assert!(overrides.is_closed(1));
    assert!(!overrides.is_closed(2));
    let err = g
        .route(
            [3.0, 0.0, -20.0],
            [3.0, 0.0, 50.0],
            &overrides.route_options(),
        )
        .unwrap_err();
    assert!(matches!(err, RouteError::Unreachable { .. }));
}

/// Retail quirk (measured on `city/sf.bai` 2026-09-24): several roads
/// author individual lane curves vertex-reversed — the curve's last
/// authored vertex sits at the road's *start* while the lane still
/// belongs to its side's carriageway (sf roads 111-116, mixed orders
/// inside one side). Storage order is authoring noise, so the graph
/// normalizes it: a reversed curve samples and ranks like any other.
#[test]
fn authored_reversed_lane_vertices_normalize_to_section_order() {
    let centre = [[0.0, 0.0, 0.0], [0.0, 0.0, 50.0], [0.0, 0.0, 100.0]];
    let mut reversed = offset(&centre, 6.0, 0.0);
    reversed.reverse();
    let right = side(
        0,
        &[(3.75, offset(&centre, 3.75, 0.0)), (6.0, reversed)],
        &[],
        3,
    );
    let left = side(0, &[(-3.75, offset(&centre, -3.75, 0.0))], &[], 3);
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
    assert!(build.issues.is_empty(), "{:?}", build.issues);
    let g = build.graph;
    let l = lane(0, Side::Right, 1);
    let nav = g.lane(l).unwrap();
    // Vertices run in section order after the flip: first ≈ start.
    assert!(approx(nav.vertices()[0], [6.0, 0.0, 0.0], 0.01));
    assert!(approx(
        *nav.vertices().last().unwrap(),
        [6.0, 0.0, 100.0],
        0.01
    ));
    // The offset is measured against matching sections — +6, not the
    // drifted garbage an unflipped zip produces.
    assert!(
        (nav.lateral_offset - 6.0).abs() < 0.01,
        "{}",
        nav.lateral_offset
    );
    // Travel-direction sampling is uniform: s=0 is the road's start
    // junction side and s=length its end — the pre-fix behaviour put
    // the travel end a full road length from the exit junction.
    let s0 = g.sample_lane(l, 0.0).unwrap();
    let s1 = g.sample_lane(l, nav.length).unwrap();
    assert!(approx(s0.position, [6.0, 0.0, 0.0], 0.01));
    assert!(approx(s1.position, [6.0, 0.0, 100.0], 0.01));
    assert!(s0.tangent[2] > 0.0, "forward arc drives +z: {s0:?}");
    // Ranking sees it as the outer of the two right-side lanes.
    let arc = g.arc(nav.arc.unwrap());
    assert_eq!(arc.lanes.last().copied(), Some(l));
}

#[test]
fn speed_limit_resolution_prefers_exception_then_default_then_base() {
    let aimap = Aimap {
        exceptions: vec![exception(9, 0.0, 30.0)],
        speed_limit: Some(20.0),
        drive_on_left: Some(1),
        ..Aimap::default()
    };
    let o = NavOverrides::from_aimap(&aimap);
    assert_eq!(o.speed_limit(9), Some(30.0));
    assert_eq!(o.speed_limit(4), Some(20.0));
    assert_eq!(o.drive_on_left, Some(1));
    let bare = NavOverrides::from_aimap(&Aimap::default());
    assert_eq!(bare.speed_limit(9), None);
    // `effective_speed` falls back to the authored base speed.
    let g = NavGraph::build(&straight()).graph;
    let road = &g.roads()[0];
    assert_eq!(bare.effective_speed(road), road.base_speed);
    assert_eq!(o.effective_speed(road), 20.0);
}

// ---------- F15-B.6: route densification ----------

fn waypoint(x: f32, y: f32, z: f32) -> OpponentRoutePoint {
    OpponentRoutePoint {
        position: Vec3::new(x, y, z),
        brake: 0.0,
        forward_offset: 0.0,
        side_offset: 0.0,
        target_speed: 0.0,
        speed_start: 0.0,
        side_start: 0.0,
    }
}

/// A U-shaped network: road0 climbs +z at x=0, road1 crosses the top
/// +x, road2 descends −z at x=60. The straight line between the two
/// road bottoms crosses ~50 m of lane-less block interior — the same
/// shape as the sf circuit:0 hillside leg.
fn u_shape() -> Bai {
    let c0 = [[0.0, 0.0, 0.0], [0.0, 0.0, 100.0]];
    let c1 = [[0.0, 0.0, 100.0], [60.0, 0.0, 100.0]];
    let c2 = [[60.0, 0.0, 100.0], [60.0, 0.0, 0.0]];
    let zroad = |c: &[[f32; 3]]| {
        (
            side(0, &[(3.75, offset(c, 3.75, 0.0))], &[], 2),
            side(0, &[(-3.75, offset(c, -3.75, 0.0))], &[], 2),
        )
    };
    let (r0r, r0l) = zroad(&c0);
    let (r2r, r2l) = zroad(&c2);
    bai(
        vec![
            road(0, &c0, vec![1], r0r, r0l, dead_end(), connected(0, 0)),
            road(
                1,
                &c1,
                vec![1],
                side(0, &[(3.75, offset(&c1, 0.0, -3.75))], &[], 2),
                side(0, &[(-3.75, offset(&c1, 0.0, 3.75))], &[], 2),
                connected(0, 1),
                connected(1, 0),
            ),
            road(2, &c2, vec![1], r2r, r2l, connected(1, 1), dead_end()),
        ],
        vec![
            intersection(0, [0.0, 0.0, 100.0], &[0, 1]),
            intersection(1, [60.0, 0.0, 100.0], &[1, 2]),
        ],
    )
}

#[test]
fn route_candidates_enters_the_goalward_direction() {
    // From the south arm hugging its *left* lane — the nearer snap —
    // to the east arm. The single-entry `route` seeds only the
    // backward arc, which dead-ends; the candidate search seeds both
    // directions and reaches the goal going forward.
    let g = NavGraph::build(&cross(1)).graph;
    let opts = RouteOptions::default();
    let err = g
        .route([-4.5, 0.0, -20.0], [20.0, 0.0, -2.0], &opts)
        .unwrap_err();
    assert!(matches!(err, RouteError::Unreachable { .. }), "{err}");
    let r = g
        .route_candidates([-4.5, 0.0, -20.0], [20.0, 0.0, -2.0], &opts)
        .unwrap();
    let roads: Vec<u16> = r.steps.iter().map(|a| g.arc(*a).road).collect();
    assert_eq!(roads, vec![0, 1]);
    assert_eq!(g.arc(r.steps[0]).dir, TravelDir::Forward);
    // The reported start hit is on the forward (right-side) lane even
    // though the backward lane snapped nearer.
    assert_eq!(r.start.lane.side, Side::Right);
}

#[test]
fn route_candidates_prefers_the_near_level() {
    // The stacked fixture: the ground point still enters the ground
    // road and the bridge point the bridge — a farther lane's arc only
    // wins when its path is shorter by more than its extra snap cost.
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
    let g = NavGraph::build(&bai(vec![mk(0, &cg, 1), mk(1, &cb, 2)], Vec::new())).graph;
    // Ground point → ground goal: the bridge lane's trivial one-arc
    // "route" must not win just because it exists — the seed snap cost
    // keeps the start on the level the query point actually sits.
    let r = g
        .route_candidates(
            [0.0, 0.0, -10.0],
            [0.0, 0.0, 10.0],
            &RouteOptions::default(),
        )
        .unwrap();
    assert_eq!(g.arc(r.steps[0]).road, 0);
    assert!(r.start.distance < 8.0);
}

#[test]
fn route_path_samples_the_lane_walk() {
    let g = NavGraph::build(&u_shape()).graph;
    let r = g
        .route_candidates(
            [0.0, 0.0, 10.0],
            [60.0, 0.0, 10.0],
            &RouteOptions::default(),
        )
        .unwrap();
    let pts = g.route_path(&r, 8.0);
    assert!(pts.len() > 3, "the U walk samples many lane points");
    // The path starts near the query's snapped start and ends at its
    // snapped goal — the anchors themselves are the caller's.
    assert!(pts.first().unwrap().distance(Vec3::new(0.0, 0.0, 10.0)) < 12.0);
    assert!(pts.last().unwrap().distance(Vec3::new(60.0, 0.0, 10.0)) < 12.0);
    // No teleports: consecutive samples stay within a lane step plus a
    // junction chord.
    for w in pts.windows(2) {
        assert!(w[0].distance(w[1]) < 30.0, "{:?} → {:?}", w[0], w[1]);
    }
    // And it travels the top street rather than the straight leg.
    assert!(pts.iter().any(|p| p.z > 60.0));
}

#[test]
fn densify_route_keeps_corridor_legs_verbatim() {
    let g = NavGraph::build(&straight()).graph;
    let route = OpponentRoute {
        points: vec![waypoint(0.0, 0.0, 10.0), waypoint(0.0, 0.0, 90.0)],
    };
    let d = g.densify_route(&route, false, &[], &RouteOptions::default());
    assert_eq!(d, route);
}

#[test]
fn densify_route_repaths_legs_off_the_road() {
    let g = NavGraph::build(&u_shape()).graph;
    let a = waypoint(0.0, 0.0, 10.0);
    let b = waypoint(60.0, 0.0, 10.0);
    let route = OpponentRoute {
        points: vec![a.clone(), b.clone()],
    };
    let d = g.densify_route(&route, false, &[], &RouteOptions::default());
    // The authored anchors stay verbatim at the ends — gate binding is
    // unchanged — while the interior is lane geometry.
    assert_eq!(d.points.first().unwrap(), &a);
    assert_eq!(d.points.last().unwrap(), &b);
    assert!(d.points.len() > 2, "lane samples fill the leg");
    let on_lane = LaneQuery::vehicles(4.0);
    for p in &d.points[1..d.points.len() - 1] {
        assert!(
            g.nearest_lane(p.position.into(), &on_lane).is_some(),
            "inserted point off the network: {:?}",
            p.position
        );
        // Inserted points are never staging records.
        assert_eq!(p.brake, 0.0);
    }
    // The path takes the top street, not the straight line.
    assert!(d.points.iter().any(|p| p.position.z > 60.0));
}

#[test]
fn densify_route_wrap_leg_keeps_the_loop_closed() {
    // A closed out-and-back: both legs leave the corridor, and the
    // loop still ends exactly on the first anchor so the circuit
    // convention (last ≈ first) survives.
    let g = NavGraph::build(&u_shape()).graph;
    let a = waypoint(0.0, 0.0, 10.0);
    let b = waypoint(60.0, 0.0, 10.0);
    let route = OpponentRoute {
        points: vec![a.clone(), b.clone(), a.clone()],
    };
    let d = g.densify_route(&route, true, &[], &RouteOptions::default());
    assert_eq!(d.points.first().unwrap().position, a.position);
    assert_eq!(d.points.last().unwrap().position, a.position);
    assert!(d.points.len() > 3);
}

#[test]
fn densify_route_keeps_unroutable_legs_authored() {
    // Two parallel strips with no shared intersection — the anchors
    // sit on roads that cannot connect, so the authored leg stands.
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
    let g = NavGraph::build(&bai(vec![mk(0, &a), mk(1, &bpts)], Vec::new())).graph;
    let route = OpponentRoute {
        points: vec![waypoint(0.0, 0.0, 25.0), waypoint(100.0, 0.0, 25.0)],
    };
    let d = g.densify_route(&route, false, &[], &RouteOptions::default());
    assert_eq!(d, route);
}

#[test]
fn densify_route_rejects_repaths_that_drop_a_gate_the_leg_crossed() {
    // The authored leg cuts the U's interior — off-corridor, so the
    // router would normally send it over the top street. A gate on the
    // authored line keeps the leg verbatim: a drivable detour that no
    // longer crosses the trigger stalls every Ordered participant on
    // it (the `london circuit:6` flyover case).
    let g = NavGraph::build(&u_shape()).graph;
    let a = waypoint(0.0, 0.0, 10.0);
    let b = waypoint(60.0, 0.0, 10.0);
    let route = OpponentRoute {
        points: vec![a.clone(), b.clone()],
    };
    let gate = Checkpoint {
        center: Vec3::new(30.0, 0.0, 10.0),
        radius: 5.0,
        height: 4.0,
        heading_deg: 0.0,
        require_direction: false,
    };
    let d = g.densify_route(&route, false, &[gate], &RouteOptions::default());
    assert_eq!(d, route, "the authored leg crosses the gate — keep it");
}

#[test]
fn densify_route_keeps_repaths_that_still_cross_the_gate() {
    // A gate hugging the leg's end anchor: the authored segment
    // crosses it and so does the re-path's final approach, so the
    // detour stands — coverage is preserved rather than merely
    // asserted of the authored line.
    let g = NavGraph::build(&u_shape()).graph;
    let a = waypoint(0.0, 0.0, 10.0);
    let b = waypoint(60.0, 0.0, 10.0);
    let route = OpponentRoute {
        points: vec![a.clone(), b.clone()],
    };
    let gate = Checkpoint {
        center: Vec3::new(58.0, 0.0, 12.0),
        radius: 8.0,
        height: 4.0,
        heading_deg: 0.0,
        require_direction: false,
    };
    let d = g.densify_route(&route, false, &[gate], &RouteOptions::default());
    assert!(d.points.len() > 2, "the covered re-path still densifies");
    // And the published line still crosses the trigger.
    let pts: Vec<Vec3> = d.points.iter().map(|p| p.position).collect();
    assert!(
        pts.windows(2)
            .any(|w| gate.crossed(w[0], w[1]) || gate.crossed(w[1], w[0]))
    );
}
