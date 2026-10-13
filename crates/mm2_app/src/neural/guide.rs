//! The line a neural driver is asked to follow.
//!
//! A [`Guide`] is a polyline with arc lengths — nothing more. Where it
//! comes from is the caller's business: a race opponent's authored
//! `.opp` route re-pathed through the road graph, a nav-graph path to a
//! Cops & Robbers objective, or just the straight segment to a target
//! with no road data at all. The policy only ever sees samples of it in
//! the car's own frame, so every source drives the same way.
//!
//! Position on the guide is an arc length `s`, tracked by a *local*
//! search around the previous value ([`Guide::advance`]). Routes revisit
//! streets and cross themselves, so a global nearest-point search would
//! jump between passes; the windowed search cannot.

use bevy::math::Vec3;
use mm2_game::OpponentRoute;

/// Vertical distances count this many times over in the nearest-point
/// search, so a car under a bridge projects onto the road it is on
/// rather than the deck above it.
const HEIGHT_WEIGHT: f32 = 3.0;
/// How far behind the previous arc position (m) the tracker looks.
const SEARCH_BACK: f32 = 20.0;
/// How far ahead of the previous arc position (m) the tracker looks.
const SEARCH_AHEAD: f32 = 60.0;
/// Points closer together than this (m) are merged when a guide is
/// built — zero-length legs have no direction.
const MIN_LEG: f32 = 0.05;

/// A polyline to drive along, with cumulative arc lengths.
#[derive(Debug, Clone, PartialEq)]
pub struct Guide {
    points: Vec<Vec3>,
    /// `cum[i]` is the arc length (m, 3D) from point 0 to point `i`.
    cum: Vec<f32>,
    /// Whether the line continues from its last point back to its first.
    closed: bool,
    /// Whole length (m), the closing leg included on a closed guide.
    total: f32,
}

impl Guide {
    /// A guide through `points`. `closed` joins the last point back to
    /// the first. `None` when fewer than two distinct finite points
    /// remain.
    pub fn new(points: impl IntoIterator<Item = Vec3>, closed: bool) -> Option<Self> {
        let mut pts: Vec<Vec3> = Vec::new();
        for p in points {
            if !p.is_finite() {
                continue;
            }
            if pts.last().is_none_or(|l| l.distance(p) >= MIN_LEG) {
                pts.push(p);
            }
        }
        if closed && pts.len() > 2 && pts[0].distance(*pts.last().unwrap()) < MIN_LEG {
            pts.pop();
        }
        if pts.len() < 2 {
            return None;
        }
        let mut cum = Vec::with_capacity(pts.len());
        let mut total = 0.0;
        cum.push(0.0);
        for w in pts.windows(2) {
            total += w[0].distance(w[1]);
            cum.push(total);
        }
        if closed {
            total += pts[pts.len() - 1].distance(pts[0]);
        }
        Some(Self {
            points: pts,
            cum,
            closed,
            total,
        })
    }

    /// The guide along an opponent's driving line.
    pub fn from_route(route: &OpponentRoute) -> Option<Self> {
        Self::new(
            route.points.iter().map(|p| p.position),
            crate::opponents::route_is_closed(route),
        )
    }

    /// Whole length (m).
    pub fn length(&self) -> f32 {
        self.total
    }

    /// Whether the line loops.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// The polyline's points.
    pub fn points(&self) -> &[Vec3] {
        &self.points
    }

    fn legs(&self) -> usize {
        if self.closed {
            self.points.len()
        } else {
            self.points.len() - 1
        }
    }

    fn leg(&self, i: usize) -> (Vec3, Vec3, f32, f32) {
        let a = self.points[i];
        let b = self.points[(i + 1) % self.points.len()];
        let start = self.cum[i];
        let end = if i + 1 < self.points.len() {
            self.cum[i + 1]
        } else {
            self.total
        };
        (a, b, start, end)
    }

    /// Arc length wrapped (closed) or clamped (open) into the guide.
    pub fn normalize(&self, s: f32) -> f32 {
        if self.closed {
            s.rem_euclid(self.total)
        } else {
            s.clamp(0.0, self.total)
        }
    }

    /// Index of the leg containing arc length `s` (already normalized).
    fn leg_at(&self, s: f32) -> usize {
        let i = self.cum.partition_point(|&c| c <= s);
        i.saturating_sub(1).min(self.legs() - 1)
    }

    /// The point at arc length `s` — wrapping on a closed guide, held at
    /// the ends of an open one.
    pub fn point_at(&self, s: f32) -> Vec3 {
        let s = self.normalize(s);
        let (a, b, start, end) = self.leg(self.leg_at(s));
        let len = end - start;
        if len > 1e-6 {
            a + (b - a) * ((s - start) / len).clamp(0.0, 1.0)
        } else {
            a
        }
    }

    /// Unit direction of travel at arc length `s`.
    pub fn tangent_at(&self, s: f32) -> Vec3 {
        let s = self.normalize(s);
        let (a, b, _, _) = self.leg(self.leg_at(s));
        (b - a).normalize_or_zero()
    }

    /// Metres of guide left after `s` on an open guide; `None` on a
    /// closed one, which never ends.
    pub fn remaining(&self, s: f32) -> Option<f32> {
        (!self.closed).then(|| (self.total - s).max(0.0))
    }

    /// Weighted squared distance and arc length of the closest point of
    /// leg `i` to `pos`.
    fn project(&self, i: usize, pos: Vec3) -> (f32, f32) {
        let (a, b, start, end) = self.leg(i);
        let ab = b - a;
        let len2 = ab.length_squared();
        let t = if len2 > 1e-9 {
            ((pos - a).dot(ab) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let d = pos - (a + ab * t);
        let dist2 = d.x * d.x + d.z * d.z + (d.y * HEIGHT_WEIGHT).powi(2);
        (dist2, start + (end - start) * t)
    }

    /// The arc length nearest `pos` anywhere on the guide — for seating
    /// a car that has no previous position on it.
    pub fn locate(&self, pos: Vec3) -> f32 {
        (0..self.legs())
            .map(|i| self.project(i, pos))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map_or(0.0, |(_, s)| s)
    }

    /// The arc length nearest `pos` within the search window around
    /// `prev` — the tracker's step. On a closed guide the result is
    /// normalized; compare with [`Guide::delta`] to get signed progress.
    pub fn advance(&self, prev: f32, pos: Vec3) -> f32 {
        let lo = prev - SEARCH_BACK;
        let hi = prev + SEARCH_AHEAD;
        let mut best = (f32::INFINITY, prev);
        let mut consider = |i: usize, shift: f32| {
            let (_, _, start, end) = self.leg(i);
            if end + shift < lo || start + shift > hi {
                return;
            }
            let (d, s) = self.project(i, pos);
            // Clip the candidate to the window so a long leg cannot
            // carry the cursor past it.
            let s = (s + shift).clamp(lo, hi);
            if d < best.0 {
                best = (d, s);
            }
        };
        if self.closed {
            for shift in [-self.total, 0.0, self.total] {
                for i in 0..self.legs() {
                    consider(i, shift);
                }
            }
        } else {
            for i in 0..self.legs() {
                consider(i, 0.0);
            }
        }
        self.normalize(best.1)
    }

    /// Signed progress (m) from arc length `from` to `to`, the short way
    /// round on a closed guide.
    pub fn delta(&self, from: f32, to: f32) -> f32 {
        let d = to - from;
        if self.closed {
            let half = self.total * 0.5;
            (d + half).rem_euclid(self.total) - half
        } else {
            d
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Guide {
        Guide::new(
            [
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(100.0, 0.0, 0.0),
                Vec3::new(100.0, 0.0, 100.0),
                Vec3::new(0.0, 0.0, 100.0),
            ],
            true,
        )
        .unwrap()
    }

    #[test]
    fn a_closed_guide_counts_its_closing_leg() {
        let g = square();
        assert_eq!(g.length(), 400.0);
        assert_eq!(g.point_at(350.0), Vec3::new(0.0, 0.0, 50.0));
        assert_eq!(g.point_at(450.0), Vec3::new(50.0, 0.0, 0.0));
        assert_eq!(g.tangent_at(350.0), Vec3::new(0.0, 0.0, -1.0));
    }

    #[test]
    fn an_open_guide_holds_its_ends() {
        let g = Guide::new([Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)], false).unwrap();
        assert_eq!(g.point_at(-5.0), Vec3::ZERO);
        assert_eq!(g.point_at(50.0), Vec3::new(10.0, 0.0, 0.0));
        assert_eq!(g.remaining(4.0), Some(6.0));
    }

    #[test]
    fn advance_follows_the_car_round_a_lap_boundary() {
        let g = square();
        let s = g.advance(395.0, Vec3::new(3.0, 0.0, 1.0));
        assert!((s - 3.0).abs() < 1e-3, "{s}");
        assert!((g.delta(395.0, s) - 8.0).abs() < 1e-3);
        assert!((g.delta(s, 395.0) + 8.0).abs() < 1e-3);
    }

    #[test]
    fn advance_stays_on_its_own_pass_of_a_revisited_street() {
        // Out along +X, a loop, then back along the same street.
        let g = Guide::new(
            [
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(200.0, 0.0, 0.0),
                Vec3::new(200.0, 0.0, 200.0),
                Vec3::new(210.0, 0.0, 200.0),
                Vec3::new(210.0, 0.0, 0.5),
                Vec3::new(0.0, 0.0, 0.5),
            ],
            false,
        )
        .unwrap();
        let pos = Vec3::new(100.0, 0.0, 0.2);
        assert!((g.advance(90.0, pos) - 100.0).abs() < 1.0);
        let back = g.advance(700.0, pos);
        assert!(back > 600.0, "second pass keeps its own arc: {back}");
    }

    #[test]
    fn height_separates_a_deck_from_the_road_under_it() {
        let g = Guide::new(
            [
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(100.0, 0.0, 0.0),
                Vec3::new(100.0, 12.0, 50.0),
                Vec3::new(50.0, 12.0, 0.0),
                Vec3::new(50.0, 12.0, -50.0),
            ],
            false,
        )
        .unwrap();
        // On the deck above the first leg.
        let s = g.locate(Vec3::new(50.0, 12.0, 0.0));
        assert!(s > 150.0, "{s}");
        let s = g.locate(Vec3::new(50.0, 0.5, 0.0));
        assert!((s - 50.0).abs() < 1.0, "{s}");
    }

    #[test]
    fn degenerate_input_makes_no_guide() {
        assert!(Guide::new([Vec3::ZERO, Vec3::ZERO], false).is_none());
        assert!(Guide::new([Vec3::ZERO, Vec3::splat(f32::NAN)], false).is_none());
    }
}
