//! F15-A.1 opponent-roster contract tests: the distilled route/spec
//! data the spawn and driving systems will consume.

use bevy::prelude::*;
use mm2_game::*;

fn point(x: f32, y: f32, z: f32) -> OpponentRoutePoint {
    OpponentRoutePoint {
        position: Vec3::new(x, y, z),
        brake: 0.0,
        forward_offset: 0.0,
        side_offset: 0.0,
        target_speed: 15.0,
        speed_start: 0.0,
        side_start: 0.0,
    }
}

#[test]
fn route_length_sums_segment_distances() {
    let route = OpponentRoute {
        points: vec![
            point(0.0, 0.0, 0.0),
            point(3.0, 0.0, 0.0),
            point(3.0, 4.0, 0.0),
        ],
    };
    assert!((route.length() - 7.0).abs() < 1e-4);
    assert_eq!(OpponentRoute::default().length(), 0.0);
}

#[test]
fn skill_is_the_first_authored_param_only() {
    let spec = OpponentSpec {
        vehicle: "vpfoo".into(),
        params: vec![0.85, 0.0, 50.0],
        route: None,
    };
    assert_eq!(spec.skill(), Some(0.85));
    let bare = OpponentSpec {
        vehicle: "vpfoo".into(),
        params: vec![],
        route: None,
    };
    assert_eq!(bare.skill(), None);
}

/// F15-B.2/B.8: the ten-value `[Opponent]` tail decodes positionally
/// into the documented `RegisterRoute` vocabulary (RACE-14) — the
/// community format reference's column order, corroborated by the
/// recovered signature's flag-argument order. A retail `circuit0`
/// amateur row binds throttle, look-ahead, corner multiplier and all
/// four avoid flags; the rare `crash6` row separates `unused` (col 1)
/// from `badPathfinding` (col 8); a short tail leaves the missing
/// columns `None` rather than inventing zeros.
#[test]
fn drive_params_decodes_the_authored_tail() {
    let spec = OpponentSpec {
        vehicle: "vpcoop".into(),
        // A london `circuit0` amateur row, verbatim.
        params: vec![0.86, 0.0, 50.0, 0.7, 1.0, 1.0, 1.0, 1.0, 0.0, 1.0],
        route: None,
    };
    let p = spec.drive_params();
    assert_eq!(p.max_throttle, Some(0.86));
    assert_eq!(p.unused_flag, Some(false));
    assert_eq!(p.distance_padding, Some(50.0));
    assert_eq!(p.corner_brake, Some(0.7));
    assert_eq!(p.avoid_traffic, Some(true));
    assert_eq!(p.avoid_props, Some(true));
    assert_eq!(p.avoid_players, Some(true));
    assert_eq!(p.avoid_opponents, Some(true));
    assert_eq!(p.weird_pathfinding, Some(false));
    assert_eq!(p.corner_speed_multiplier, Some(1.0));

    // A london `crash6` follow-car row, verbatim — one of only three
    // retail rows authoring the rare flags: col 1 is the unused flag,
    // col 8 the badPathfinding switch.
    let follow = OpponentSpec {
        vehicle: "vpcab".into(),
        params: vec![1.0, 1.0, 50.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        route: None,
    };
    let p = follow.drive_params();
    assert_eq!(p.unused_flag, Some(true));
    assert_eq!(p.corner_brake, Some(1.0));
    assert_eq!(p.weird_pathfinding, Some(true));
    assert_eq!(p.avoid_props, Some(false));
    assert_eq!(p.avoid_players, Some(true));
    assert_eq!(p.avoid_opponents, Some(true));

    // The `stunt0` single-value row: the throttle column binds and
    // everything else stays absent — not authored, not zero.
    let stub = OpponentSpec {
        vehicle: "vpfoo".into(),
        params: vec![1.0],
        route: None,
    };
    let p = stub.drive_params();
    assert_eq!(p.max_throttle, Some(1.0));
    assert_eq!(p.avoid_players, None);
    assert_eq!(p.corner_speed_multiplier, None);
}

#[test]
fn resolved_routes_counts_only_wired_lines() {
    let roster = OpponentRoster {
        entries: vec![
            OpponentSpec {
                vehicle: "a".into(),
                params: vec![],
                route: Some(OpponentRoute {
                    points: vec![point(0.0, 0.0, 0.0)],
                }),
            },
            OpponentSpec {
                vehicle: "b".into(),
                params: vec![],
                route: None,
            },
        ],
        issues: vec![],
    };
    assert_eq!(roster.entries.len(), 2);
    assert_eq!(roster.resolved_routes(), 1);
}
