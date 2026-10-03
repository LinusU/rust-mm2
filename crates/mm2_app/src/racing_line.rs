//! Opponent racing line (F15): where along its route an opponent is
//! and where it should aim.
//!
//! The opponent driver used to chase the next raw `.opp` anchor. Those
//! sit 40–200 m apart, so a corner only came into view inside the
//! 14 m reach radius — about half a second at racing speed, against
//! the ~70 m a car needs to brake for it. Everything here instead
//! works on the polyline itself:
//!
//! - [`RouteCursor`] projects the car onto the leg it is chasing, so
//!   distances are measured *along the line* rather than from wherever
//!   a shove left the car.
//! - [`RouteCursor::point_ahead`] is the pure-pursuit aim: a point a
//!   speed-scaled distance down the line. A car knocked off the line
//!   aims back onto it instead of straight at a distant anchor through
//!   the block in between.
//!
//! All of this is designed policy (DSN-66): the original opponent
//! controller is unrecovered (UNK-11).

use mm2_game::OpponentRoute;

use crate::opponents::route_is_closed;

/// The car's place on its route: the leg it is chasing and how far
/// along that leg its projection sits. Leg `i` runs `points[i]` →
/// `points[(i + 1) % n]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteCursor {
    /// Leg index.
    pub leg: usize,
    /// Distance (m, XZ) along the leg from its start.
    pub along: f32,
}

impl RouteCursor {
    /// Project `pos` onto the leg leading into chase index `next` — the
    /// same leg the re-anchor projects onto. An open route still
    /// approaching its first anchor (`next == 0`) has no leg into it;
    /// the cursor then sits at the start of leg 0, and the car is
    /// expected to aim at the anchor itself. `None` for a route with
    /// fewer than two points or a chase index past an open route's end.
    pub fn locate(route: &OpponentRoute, next: usize, pos: bevy::math::Vec3) -> Option<Self> {
        let n = route.points.len();
        if n < 2 || next > n {
            return None;
        }
        let closed = route_is_closed(route);
        let legs = leg_count(n, closed);
        let leg = match next {
            0 if closed => legs - 1,
            0 => return Some(Self { leg: 0, along: 0.0 }),
            i if i >= n => return None,
            i => (i - 1).min(legs - 1),
        };
        let (a, b) = leg_ends(route, leg);
        let len = xz_len(a, b);
        let along = if len > 1e-3 {
            (((pos.x - a.x) * (b.x - a.x) + (pos.z - a.z) * (b.z - a.z)) / len).clamp(0.0, len)
        } else {
            0.0
        };
        Some(Self { leg, along })
    }

    /// The point `dist` metres further down the line, following legs
    /// past their anchors — wrapping on a closed route, stopping at the
    /// last anchor of an open one. Heights interpolate along the legs.
    pub fn point_ahead(&self, route: &OpponentRoute, dist: f32) -> bevy::math::Vec3 {
        let n = route.points.len();
        let closed = route_is_closed(route);
        let legs = leg_count(n, closed);
        let mut leg = self.leg;
        let mut along = self.along;
        let mut remain = dist.max(0.0);
        // One lap of legs is plenty: `dist` is a look-ahead, not a lap.
        for _ in 0..=legs {
            let (a, b) = leg_ends(route, leg);
            let len = xz_len(a, b);
            if along + remain <= len || (!closed && leg + 1 >= legs) {
                let t = if len > 1e-3 {
                    ((along + remain) / len).min(1.0)
                } else {
                    1.0
                };
                return a + (b - a) * t;
            }
            remain -= (len - along).max(0.0);
            along = 0.0;
            leg = (leg + 1) % legs;
        }
        leg_ends(route, leg).1
    }
}

fn leg_count(n: usize, closed: bool) -> usize {
    if closed { n } else { n - 1 }
}

fn leg_ends(route: &OpponentRoute, leg: usize) -> (bevy::math::Vec3, bevy::math::Vec3) {
    let n = route.points.len();
    (
        route.points[leg].position,
        route.points[(leg + 1) % n].position,
    )
}

fn xz_len(a: bevy::math::Vec3, b: bevy::math::Vec3) -> f32 {
    (b.x - a.x).hypot(b.z - a.z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Vec3;
    use mm2_game::OpponentRoutePoint;

    fn route(points: &[[f32; 2]]) -> OpponentRoute {
        OpponentRoute {
            points: points
                .iter()
                .map(|p| OpponentRoutePoint {
                    position: Vec3::new(p[0], 0.0, p[1]),
                    brake: 0.0,
                    forward_offset: 0.0,
                    side_offset: 0.0,
                    target_speed: 0.0,
                    speed_start: 0.0,
                    side_start: 0.0,
                })
                .collect(),
        }
    }

    /// The cursor projects onto the leg into the chased point, so a
    /// car shoved sideways keeps its distance along the line.
    #[test]
    fn cursor_projects_onto_the_chased_leg() {
        let r = route(&[[0.0, 0.0], [100.0, 0.0], [100.0, 100.0]]);
        let c = RouteCursor::locate(&r, 1, Vec3::new(30.0, 0.0, 20.0)).unwrap();
        assert_eq!(c.leg, 0);
        assert!((c.along - 30.0).abs() < 1e-4);
        // Past the leg's end clamps onto it.
        let c = RouteCursor::locate(&r, 1, Vec3::new(130.0, 0.0, 0.0)).unwrap();
        assert!((c.along - 100.0).abs() < 1e-4);
        // An open route's end has no leg to chase.
        assert!(RouteCursor::locate(&r, 3, Vec3::ZERO).is_none());
    }

    /// The aim follows the line round a corner instead of cutting it.
    #[test]
    fn point_ahead_follows_the_line_past_anchors() {
        let r = route(&[[0.0, 0.0], [100.0, 0.0], [100.0, 100.0]]);
        let c = RouteCursor {
            leg: 0,
            along: 90.0,
        };
        let p = c.point_ahead(&r, 30.0);
        assert!((p - Vec3::new(100.0, 0.0, 20.0)).length() < 1e-3, "{p}");
        // An open route's aim stops at its last anchor.
        let p = c.point_ahead(&r, 500.0);
        assert!((p - Vec3::new(100.0, 0.0, 100.0)).length() < 1e-3, "{p}");
    }

    /// A closed route wraps its aim onto the first leg again.
    #[test]
    fn point_ahead_wraps_a_closed_route() {
        let r = route(&[[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 10.0]]);
        // Chasing point 0 means driving the wrap leg 3 → 0.
        let c = RouteCursor::locate(&r, 0, Vec3::new(0.0, 0.0, 5.0)).unwrap();
        assert_eq!(c.leg, 3);
        let p = c.point_ahead(&r, 25.0);
        assert!((p - Vec3::new(20.0, 0.0, 0.0)).length() < 0.1, "{p}");
    }
}
