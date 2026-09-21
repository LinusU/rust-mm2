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
