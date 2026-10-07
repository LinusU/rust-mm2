//! F19-B.1 pedestrian sidewalk tests: the walkable net, kerb-corner
//! joins, seeded density placement and bounded walker advance over
//! synthetic BAI fixtures (F19-AC04's lane/sidewalk-eligibility and
//! termination legs, F19-AC06's seed leg). Synthetic geometry only — the
//! retail census lives behind `mm2-inspect nav`.

use std::collections::BTreeSet;

use mm2_formats::bai::{
    Bai, Culling, END_FILL, Intersection, Road, RoadEnd, RoadSection, RoadSide,
};
use mm2_game::pedwalk::{
    MAX_ADVANCE, MIN_WALK_LANE, PedPlan, SidewalkNet, WalkDir, WalkIssue, WalkPolicy, Walker,
    in_walk_band, plan_pedestrians, target_population, within_bubble,
};
use mm2_game::*;

// ---------- synthetic BAI fixtures (same shape as tests/nav.rs) ----------

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

fn side(lanes: &[Vec<[f32; 3]>], sidewalks: &[Vec<[f32; 3]>]) -> RoadSide {
    let curves: Vec<&Vec<[f32; 3]>> = lanes.iter().chain(sidewalks.iter()).collect();
    RoadSide {
        lane_count: lanes.len() as u16,
        tram_count: 0,
        train_count: 0,
        sidewalk_count: sidewalks.len() as u16,
        ambient_types: 0,
        lane_distances: curves.iter().map(|v| cum(v)).collect(),
        edge_distances: curves.iter().map(|_| 3.75).collect(),
        misc: [0xCD; 40],
        lane_vertices: curves.iter().map(|v| (*v).clone()).collect(),
        tram_vertices: Vec::new(),
        train_vertices: Vec::new(),
        sidewalk_inner: vec![[0.0; 3]; 2],
        sidewalk_outer: vec![[0.0; 3]; 2],
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

/// A road along +z at `x0` from `z0`, 100 m long: one vehicle lane on
/// the right, and `sidewalks` on the right side.
fn road_with(id: u16, x0: f32, z0: f32, sidewalks: Vec<Vec<[f32; 3]>>) -> Road {
    let lane = vec![[x0 + 3.75, 0.0, z0], [x0 + 3.75, 0.0, z0 + 100.0]];
    road_lane(id, x0, z0, lane, sidewalks)
}

/// As [`road_with`], with the vehicle lane's vertices given.
fn road_lane(
    id: u16,
    x0: f32,
    z0: f32,
    lane: Vec<[f32; 3]>,
    sidewalks: Vec<Vec<[f32; 3]>>,
) -> Road {
    let centre = [[x0, 0.0, z0], [x0, 0.0, z0 + 100.0]];
    let dists = cum(&centre);
    Road {
        id,
        flags: 0,
        rooms: vec![1],
        half_width: 7.5,
        base_speed: 15.0,
        right: side(&[lane], &sidewalks),
        left: side(&[], &[]),
        sections: dists
            .iter()
            .enumerate()
            .map(|(i, d)| RoadSection {
                distance: *d,
                origin: centre[i],
                x_axis: [1.0, 0.0, 0.0],
                y_axis: [0.0, 1.0, 0.0],
                z_axis: [0.0, 0.0, 1.0],
                tangent: [0.0, 0.0, 1.0],
            })
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

fn graph(roads: Vec<Road>) -> NavGraph {
    NavGraph::build(&bai(roads)).graph
}

fn sw(road: u16) -> LaneId {
    LaneId {
        road,
        side: Side::Right,
        index: 0,
        kind: LaneKind::Sidewalk,
    }
}

use mm2_formats::bai::Side;

/// Road 0's sidewalk runs z 0..40 along x = 8; road 1's runs from the
/// corner (8, 0, 41) out along +x for 40 m — the two ends are 1 m
/// apart, one kerb corner. Road 2's sidewalk sits 30 m away, alone.
fn corner_graph() -> NavGraph {
    graph(vec![
        road_with(0, 0.0, 0.0, vec![vec![[8.0, 0.0, 0.0], [8.0, 0.0, 40.0]]]),
        road_with(
            1,
            0.0,
            41.0,
            vec![vec![[8.0, 0.0, 41.0], [48.0, 0.0, 41.0]]],
        ),
        road_with(
            2,
            200.0,
            0.0,
            vec![vec![[208.0, 0.0, 0.0], [208.0, 0.0, 30.0]]],
        ),
    ])
}

fn net_of(g: &NavGraph) -> SidewalkNet {
    SidewalkNet::build(g, &NavOverrides::default(), &WalkPolicy::default())
}

// ---------- the net ----------

#[test]
fn the_net_carries_sidewalk_curves_only_and_joins_a_kerb_corner() {
    let g = corner_graph();
    assert_eq!(g.stats().vehicle_lanes, 3);
    let net = net_of(&g);
    let st = net.stats();
    assert_eq!((st.curves, st.walkable, st.excluded), (3, 3, 0));
    assert_eq!(st.ends, 6);
    // Road 0's finish meets road 1's start and vice versa; the rest are free.
    assert_eq!(st.joined_ends, 2);
    assert_eq!(net.corner(sw(0), true), vec![sw(1)]);
    assert_eq!(net.corner(sw(1), false), vec![sw(0)]);
    assert!(net.corner(sw(0), false).is_empty());
    assert!(net.corner(sw(2), true).is_empty());
    // No vehicle lane is ever walkable.
    let vehicle = LaneId {
        road: 0,
        side: Side::Right,
        index: 0,
        kind: LaneKind::Vehicle,
    };
    assert!(g.lane(vehicle).is_some());
    assert!(!net.is_walkable(vehicle));
    assert!(net.lane_ids().all(|l| l.kind == LaneKind::Sidewalk));
}

#[test]
fn closed_short_and_stacked_curves_do_not_join_or_walk() {
    // Closed road: its sidewalk drops out, and with it the corner.
    let g = corner_graph();
    let mut closed = NavOverrides::default();
    closed.closed_roads.insert(1);
    let net = SidewalkNet::build(&g, &closed, &WalkPolicy::default());
    assert_eq!(net.stats().excluded, 1);
    assert!(!net.is_walkable(sw(1)));
    assert!(net.corner(sw(0), true).is_empty());

    // A curve shorter than the minimum carries nobody.
    let short = graph(vec![road_with(
        0,
        0.0,
        0.0,
        vec![vec![[8.0, 0.0, 0.0], [8.0, 0.0, MIN_WALK_LANE * 0.5]]],
    )]);
    let net = net_of(&short);
    assert_eq!((net.stats().walkable, net.stats().excluded), (0, 1));

    // Two ends over each other but on different levels (a bridge over a
    // street) are not one corner.
    let stacked = graph(vec![
        road_with(0, 0.0, 0.0, vec![vec![[8.0, 0.0, 0.0], [8.0, 0.0, 40.0]]]),
        road_with(
            1,
            0.0,
            41.0,
            vec![vec![[8.0, 6.0, 40.0], [48.0, 6.0, 40.0]]],
        ),
    ]);
    let net = net_of(&stacked);
    assert!(net.corner(sw(0), true).is_empty());
    assert_eq!(net.stats().joined_ends, 0);
}

#[test]
fn a_join_is_refused_when_the_line_between_the_ends_crosses_a_vehicle_lane() {
    // Two sidewalk ends 6 m apart (inside the 8 m radius) with a vehicle
    // lane running across the gap at z = 43: the radius alone would join
    // them; the carriageway between them severs the join.
    let across = vec![[0.0, 0.0, 43.0], [16.0, 0.0, 43.0]];
    let g = graph(vec![
        road_with(0, 0.0, 0.0, vec![vec![[8.0, 0.0, 0.0], [8.0, 0.0, 40.0]]]),
        road_with(
            1,
            0.0,
            46.0,
            vec![vec![[8.0, 0.0, 46.0], [48.0, 0.0, 46.0]]],
        ),
        road_lane(2, 0.0, 0.0, across, Vec::new()),
    ]);
    let net = net_of(&g);
    assert!(net.corner(sw(0), true).is_empty());
    assert!(net.corner(sw(1), false).is_empty());
    assert_eq!(net.stats().joined_ends, 0);
    assert_eq!(net.stats().severed, 2, "counted from both ends");

    // Without the crossing lane the same pair is one corner.
    let g = graph(vec![
        road_with(0, 0.0, 0.0, vec![vec![[8.0, 0.0, 0.0], [8.0, 0.0, 40.0]]]),
        road_with(
            1,
            0.0,
            46.0,
            vec![vec![[8.0, 0.0, 46.0], [48.0, 0.0, 46.0]]],
        ),
    ]);
    let net = net_of(&g);
    assert_eq!(net.corner(sw(0), true), vec![sw(1)]);
    assert_eq!(net.stats().severed, 0);

    // A lane on another level (a bridge over the gap) does not sever it.
    let over = vec![[0.0, 6.0, 43.0], [16.0, 6.0, 43.0]];
    let g = graph(vec![
        road_with(0, 0.0, 0.0, vec![vec![[8.0, 0.0, 0.0], [8.0, 0.0, 40.0]]]),
        road_with(
            1,
            0.0,
            46.0,
            vec![vec![[8.0, 0.0, 46.0], [48.0, 0.0, 46.0]]],
        ),
        road_lane(2, 0.0, 0.0, over, Vec::new()),
    ]);
    assert_eq!(net_of(&g).corner(sw(0), true), vec![sw(1)]);
}

// ---------- advance ----------

fn walker(lane: LaneId, s: f32, dir: WalkDir) -> Walker {
    Walker {
        lane,
        s,
        dir,
        from: None,
    }
}

#[test]
fn a_walker_rounds_the_corner_then_turns_at_the_dead_end() {
    let g = corner_graph();
    let net = net_of(&g);
    let mut rng = NavRng::new(7);
    // 10 m left on road 0, then onto road 1.
    let mut w = walker(sw(0), 30.0, WalkDir::Forward);
    let step = net.advance(&mut w, 15.0, &mut rng);
    assert_eq!(step.hops, 1);
    assert_eq!(w.lane, sw(1));
    assert_eq!(w.dir, WalkDir::Forward);
    assert!((w.s - 5.0).abs() < 1e-4, "s = {}", w.s);
    assert_eq!(w.from, Some(sw(0)));
    let pose = net.sample(&g, &w).unwrap();
    assert!((pose.position[0] - 13.0).abs() < 1e-3);
    assert!(pose.tangent[0] > 0.9, "faces along +x: {:?}", pose.tangent);

    // Walk off road 1's far (dead) end: turned round, heading back.
    let step = net.advance(&mut w, 40.0, &mut rng);
    assert_eq!(step.turned_around, 1);
    assert_eq!(w.lane, sw(1));
    assert_eq!(w.dir, WalkDir::Backward);
    assert!(
        (w.s - 35.0).abs() < 1e-3,
        "35 m to the end, 5 m back: {}",
        w.s
    );
    let pose = net.sample(&g, &w).unwrap();
    assert!(
        pose.tangent[0] < -0.9,
        "backward walker faces -x: {:?}",
        pose.tangent
    );
}

#[test]
fn a_backward_walker_enters_the_next_curve_from_the_right_end() {
    let g = corner_graph();
    let net = net_of(&g);
    let mut rng = NavRng::new(1);
    // Backward along road 1 to its start, round into road 0's finish.
    let mut w = walker(sw(1), 3.0, WalkDir::Backward);
    let step = net.advance(&mut w, 8.0, &mut rng);
    assert_eq!(step.hops, 1);
    assert_eq!(w.lane, sw(0));
    assert_eq!(
        w.dir,
        WalkDir::Backward,
        "entered road 0 at its finish, walking back"
    );
    assert!((w.s - 35.0).abs() < 1e-3, "s = {}", w.s);
}

#[test]
fn a_corner_prefers_a_curve_other_than_the_one_just_left() {
    // Three curves meet at one corner: A's finish touches B's and C's
    // starts. A walker arriving on B from A, then turned round at B's
    // far end and coming back, must go to C rather than straight back.
    let g = graph(vec![
        road_with(0, 0.0, 0.0, vec![vec![[8.0, 0.0, 0.0], [8.0, 0.0, 40.0]]]),
        road_with(1, 0.0, 0.0, vec![vec![[8.0, 0.0, 41.0], [48.0, 0.0, 41.0]]]),
        road_with(2, 0.0, 0.0, vec![vec![[8.5, 0.0, 41.0], [8.5, 0.0, 81.0]]]),
    ]);
    let net = net_of(&g);
    assert_eq!(net.corner(sw(0), true).len(), 2);
    for seed in 0..32 {
        let mut rng = NavRng::new(seed);
        // Arrive on road 1 from road 0 ...
        let mut w = Walker {
            lane: sw(1),
            s: 0.0,
            dir: WalkDir::Backward,
            from: Some(sw(0)),
        };
        // ... then step backward off its start: the corner offers road 0
        // and road 2; the one just left (road 0) is skipped.
        net.advance(&mut w, 1.0, &mut rng);
        assert_eq!(w.lane, sw(2), "seed {seed}");
    }
}

#[test]
fn advance_is_bounded_and_never_leaves_the_net() {
    let g = corner_graph();
    let net = net_of(&g);
    let mut rng = NavRng::new(99);
    let mut w = walker(sw(0), 12.0, WalkDir::Forward);
    let mut lanes = BTreeSet::new();
    for i in 0..5_000 {
        // Odd steps, including one past the clamp.
        let ds = [0.4, 3.0, 17.5, 120.0, 0.0][i % 5];
        let step = net.advance(&mut w, ds, &mut rng);
        assert!(step.hops <= 64 && step.turned_around <= 64);
        let len = net
            .length(w.lane)
            .expect("walker stays on a walkable curve");
        assert!((0.0..=len).contains(&w.s), "s {} outside 0..{len}", w.s);
        assert!(
            net.sample(&g, &w)
                .unwrap()
                .position
                .iter()
                .all(|c| c.is_finite())
        );
        lanes.insert((w.lane.road, w.dir == WalkDir::Forward));
    }
    // It really did walk both curves of the corner, both ways.
    assert!(lanes.iter().any(|(r, _)| *r == 0));
    assert!(lanes.iter().any(|(r, _)| *r == 1));
}

#[test]
fn a_step_is_clamped_and_hostile_steps_move_nothing() {
    let g = corner_graph();
    let net = net_of(&g);
    let mut rng = NavRng::new(3);
    for ds in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -5.0, 0.0] {
        let mut w = walker(sw(2), 10.0, WalkDir::Forward);
        let step = net.advance(&mut w, ds, &mut rng);
        assert_eq!(step, WalkStep::default(), "ds = {ds}");
        assert_eq!((w.lane, w.s), (sw(2), 10.0));
    }
    // A huge step is clamped: on a 30 m dead-end curve, 50 m moves a
    // walker at most MAX_ADVANCE of path length (here: out, round, back).
    let mut w = walker(sw(2), 0.0, WalkDir::Forward);
    net.advance(&mut w, 1.0e9, &mut rng);
    // 50 m = 30 m out, turn, 20 m back → s = 10, backward.
    assert_eq!(MAX_ADVANCE, 50.0);
    assert_eq!(w.dir, WalkDir::Backward);
    assert!((w.s - 10.0).abs() < 1e-3, "s = {}", w.s);
    // A walker on a curve the net does not carry is left alone.
    let mut stranger = walker(
        LaneId {
            road: 0,
            side: Side::Right,
            index: 0,
            kind: LaneKind::Vehicle,
        },
        5.0,
        WalkDir::Forward,
    );
    assert_eq!(
        net.advance(&mut stranger, 3.0, &mut rng),
        WalkStep::default()
    );
    assert_eq!(stranger.s, 5.0);
    assert!(net.sample(&g, &stranger).is_none());
}

use mm2_game::pedwalk::WalkStep;

// ---------- planning ----------

fn policy() -> WalkPolicy {
    WalkPolicy {
        max_active: 20,
        min_player_distance: 5.0,
        recycle_distance: 60.0,
        spacing: 2.0,
        ..WalkPolicy::default()
    }
}

fn plan(g: &NavGraph, net: &SidewalkNet, seed: u64, density: f32, at: &[[f32; 3]]) -> PedPlan {
    plan_pedestrians(net, g, seed, density, at, &policy())
}

#[test]
fn the_plan_places_walkers_on_nearby_sidewalks_inside_the_spawn_band() {
    let g = corner_graph();
    let net = net_of(&g);
    let here = [[20.0, 0.0, 20.0]];
    let p = plan(&g, &net, 11, 1.0, &here);
    assert_eq!(p.target, 20);
    assert!(!p.spawns.is_empty() && p.spawns.len() <= p.target);
    assert_eq!(p.spawns.len() + p.dropped, p.target);
    // Only the corner's two curves are near; the far one is not a candidate.
    assert_eq!(p.candidate_curves, 2);
    for (i, s) in p.spawns.iter().enumerate() {
        assert!(net.is_walkable(s.walker.lane), "spawn on a non-sidewalk");
        assert_ne!(s.walker.lane, sw(2), "spawned outside the bubble");
        assert!(in_walk_band(s.sample.position, &here, &policy()));
        let pose = net.sample(&g, &s.walker).unwrap();
        assert_eq!(pose.position, s.sample.position);
        for o in &p.spawns[..i] {
            let d: f32 = (0..3)
                .map(|k| (o.sample.position[k] - s.sample.position[k]).powi(2))
                .sum();
            assert!(d >= 4.0 - 1e-4, "two walkers {d} m² apart");
        }
    }
}

#[test]
fn the_same_seed_gives_the_same_crowd_and_another_seed_a_different_one() {
    let g = corner_graph();
    let net = net_of(&g);
    let here = [[20.0, 0.0, 20.0]];
    let a = plan(&g, &net, 5, 0.6, &here);
    let b = plan(&g, &net, 5, 0.6, &here);
    assert_eq!(a.spawns, b.spawns);
    let c = plan(&g, &net, 6, 0.6, &here);
    assert_ne!(a.spawns, c.spawns);
    assert_eq!(a.target, target_population(0.6, &policy()));
}

#[test]
fn density_scales_the_population_and_clamps() {
    let g = corner_graph();
    let net = net_of(&g);
    let here = [[20.0, 0.0, 20.0]];
    assert!(plan(&g, &net, 1, 0.0, &here).spawns.is_empty());
    let half = plan(&g, &net, 1, 0.5, &here);
    let full = plan(&g, &net, 1, 1.0, &here);
    assert_eq!((half.target, full.target), (10, 20));
    assert!(half.spawns.len() <= full.spawns.len());
    // Out-of-range and non-finite densities read as clamped / zero.
    assert_eq!(plan(&g, &net, 1, 7.0, &here).target, 20);
    assert_eq!(plan(&g, &net, 1, -1.0, &here).target, 0);
    assert_eq!(plan(&g, &net, 1, f32::NAN, &here).target, 0);
    // No plan ever exceeds the cap, however many attempts it has.
    assert!(full.spawns.len() <= policy().max_active);
}

#[test]
fn nothing_spawns_without_somebody_nearby_or_without_sidewalks() {
    let g = corner_graph();
    let net = net_of(&g);
    let none = plan(&g, &net, 1, 1.0, &[]);
    assert!(none.spawns.is_empty());
    assert!(none.issues.contains(&WalkIssue::NoInterest));
    // A player 500 m from every sidewalk gets an empty, finite plan.
    let far = plan(&g, &net, 1, 1.0, &[[2000.0, 0.0, 2000.0]]);
    assert!(far.spawns.is_empty());
    assert_eq!(far.candidate_curves, 0);
    // Standing on the only sidewalk with a huge exclusion radius: every
    // draw is dropped (counted), none forced.
    let tight = WalkPolicy {
        min_player_distance: 500.0,
        ..policy()
    };
    let p = plan_pedestrians(&net, &g, 1, 1.0, &[[8.0, 0.0, 20.0]], &tight);
    assert!(p.spawns.is_empty());
    assert_eq!(p.dropped, p.target);

    // A graph with vehicle lanes only has nobody to place.
    let roads_only = graph(vec![road_with(0, 0.0, 0.0, Vec::new())]);
    let net = net_of(&roads_only);
    let p = plan(&roads_only, &net, 1, 1.0, &[[0.0, 0.0, 10.0]]);
    assert!(p.spawns.is_empty());
    assert!(p.issues.contains(&WalkIssue::NoSidewalks));
}

#[test]
fn two_players_each_get_a_population_and_leave_each_others_exclusion() {
    let g = corner_graph();
    let net = net_of(&g);
    // One player on the corner's first curve, one far away beside the
    // lone third curve.
    let both = [[8.0, 0.0, 20.0], [208.0, 0.0, 15.0]];
    let p = plan(&g, &net, 21, 1.0, &both);
    assert_eq!(p.candidate_curves, 3);
    assert!(p.spawns.iter().any(|s| s.walker.lane == sw(2)));
    for s in &p.spawns {
        for me in &both {
            let d: f32 = (0..3).map(|k| (s.sample.position[k] - me[k]).powi(2)).sum();
            assert!(d >= 25.0 - 1e-4, "spawned within the exclusion of a player");
        }
    }
}

#[test]
fn the_bubble_tests_follow_every_interest_point() {
    let p = policy();
    let a = [0.0, 0.0, 0.0];
    let b = [500.0, 0.0, 0.0];
    assert!(in_walk_band([30.0, 0.0, 0.0], &[a, b], &p));
    assert!(
        !in_walk_band([2.0, 0.0, 0.0], &[a, b], &p),
        "inside a's exclusion"
    );
    assert!(
        !in_walk_band([250.0, 0.0, 0.0], &[a, b], &p),
        "outside both bubbles"
    );
    assert!(!in_walk_band([30.0, 0.0, 0.0], &[], &p), "nobody, no band");
    assert!(within_bubble([520.0, 0.0, 0.0], &[a, b], &p), "near b only");
    assert!(!within_bubble([250.0, 0.0, 0.0], &[a, b], &p));
    assert!(!within_bubble([0.0, 0.0, 0.0], &[], &p));
}
