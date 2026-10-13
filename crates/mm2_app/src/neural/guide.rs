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
use mm2_game::nav::{LaneQuery, NavGraph};

/// Vertical distances count this many times over in the nearest-point
/// search, so a car under a bridge projects onto the road it is on
/// rather than the deck above it.
const HEIGHT_WEIGHT: f32 = 3.0;
/// How far behind the previous arc position (m) the tracker looks.
const SEARCH_BACK: f32 = 20.0;
/// How far ahead of the previous arc position (m) the tracker looks.
const SEARCH_AHEAD: f32 = 60.0;
/// Distance (m, in plan) from its line within which a car is still on
/// it: its place follows it, and what it drives counts as progress.
/// Past any road's width and a generous cut across a corner; short of
/// the next street over.
pub const ON_LINE: f32 = 25.0;
/// How many metres of line a car may gain per metre it moves
/// ([`Guide::follow`]) — past 1 so that a car that cut a corner, and so
/// drove less than the line it skipped, catches its place up.
const FOLLOW_GAIN: f32 = 3.0;
/// Points closer together than this (m) are merged when a guide is
/// built — zero-length legs have no direction.
const MIN_LEG: f32 = 0.05;
/// Spacing (m) of the height samples [`Guide::from_route_on`] lays
/// along a leg.
const GROUND_STEP: f32 = 10.0;
/// How far (m, XZ) from a sample a road lane may run and still give it
/// its height — a wide road's half width.
const GROUND_LANE: f32 = 12.0;
/// How far above or below the straight line (m) the road is looked for.
const GROUND_SEARCH: f32 = 40.0;
/// Spacing (m) of the samples a route's guide is smoothed over.
const SMOOTH_STEP: f32 = 5.0;

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
    /// Arc length of each point the guide was built from, merged and
    /// dropped ones included — what maps a position on the guide back
    /// to an index into the source route ([`Guide::chase_index`]).
    source_arc: Vec<f32>,
}

impl Guide {
    /// A guide through `points`. `closed` joins the last point back to
    /// the first. `None` when fewer than two distinct finite points
    /// remain.
    pub fn new(points: impl IntoIterator<Item = Vec3>, closed: bool) -> Option<Self> {
        Self::with_sources(points.into_iter().map(|p| (p, true)), closed)
    }

    /// A guide through `points`, of which only those flagged `true`
    /// are *source* points — the ones [`Guide::chase_index`] counts.
    /// The rest shape the line between them.
    fn with_sources(points: impl IntoIterator<Item = (Vec3, bool)>, closed: bool) -> Option<Self> {
        let mut pts: Vec<Vec3> = Vec::new();
        // For each source point, the kept point it became.
        let mut kept: Vec<usize> = Vec::new();
        for (p, source) in points {
            if p.is_finite() && pts.last().is_none_or(|l| l.distance(p) >= MIN_LEG) {
                pts.push(p);
            }
            if source {
                kept.push(pts.len().saturating_sub(1));
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
        // A source point merged into a dropped closing point sits at
        // the end of the lap.
        let source_arc = kept
            .iter()
            .map(|&i| cum.get(i).copied().unwrap_or(total))
            .collect();
        Some(Self {
            points: pts,
            cum,
            closed,
            total,
            source_arc,
        })
    }

    /// The guide along an opponent's driving line.
    pub fn from_route(route: &OpponentRoute) -> Option<Self> {
        Self::from_route_on(route, None)
    }

    /// The guide along an opponent's driving line, laid on the road.
    ///
    /// A route's points can sit 200 m apart, and the straight line
    /// between two of them is not the road: over a San Francisco hill
    /// it runs up to 12 m under the crest or above the dip. So each leg
    /// is sampled every [`GROUND_STEP`] metres and each sample takes
    /// the height of the road lane under it, where `nav` has one within
    /// [`GROUND_LANE`]. The line keeps its plan shape; only its heights
    /// change. Where no lane runs — a park, a plaza — the straight
    /// line's height stands.
    pub fn from_route_on(route: &OpponentRoute, nav: Option<&NavGraph>) -> Option<Self> {
        let closed = crate::opponents::route_is_closed(route);
        let n = route.points.len();
        let Some(nav) = nav else {
            return Self::new(route.points.iter().map(|p| p.position), closed);
        };
        let road_height = |p: Vec3| -> Option<f32> {
            let query = LaneQuery::vehicles(GROUND_SEARCH);
            // The nearest lane in plan; of two at different levels, the
            // one nearer the line's own height.
            nav.lane_hits(p.into(), &query)
                .into_iter()
                .filter(|h| (h.point[0] - p.x).hypot(h.point[2] - p.z) <= GROUND_LANE)
                .min_by(|a, b| {
                    (a.point[1] - p.y)
                        .abs()
                        .total_cmp(&(b.point[1] - p.y).abs())
                })
                .map(|h| h.point[1])
        };
        let mut points: Vec<(Vec3, bool)> = Vec::new();
        for i in 0..n {
            let a = route.points[i].position;
            points.push((a, true));
            let last = i + 1 == n;
            if last && !closed {
                break;
            }
            let b = route.points[(i + 1) % n].position;
            let length = (b.x - a.x).hypot(b.z - a.z);
            if !length.is_finite() {
                continue;
            }
            let steps = (length / GROUND_STEP).floor() as usize;
            for k in 1..steps {
                let mut p = a.lerp(b, k as f32 / steps as f32);
                if let Some(y) = road_height(p) {
                    p.y = y;
                    // The lane was found near the straight line's
                    // height; look again from its own.
                    p.y = road_height(p).unwrap_or(y);
                }
                points.push((p, false));
            }
        }
        Self::with_sources(points, closed).map(Self::smoothed)
    }

    /// The same line with its small-scale wiggle taken out.
    ///
    /// A route is stitched from authored points inside junctions and
    /// road centre lines between them, and where one meets the other
    /// the line can jink a few metres sideways and back — a spur no car
    /// should follow and a driver shown it has to learn to ignore. The
    /// line is resampled every [`SMOOTH_STEP`] metres and each sample
    /// averaged with its neighbours over about ±10 m, twice: spurs
    /// flatten, a square corner becomes a curve of a few metres'
    /// radius, and everything longer than a car is left where it was.
    /// An open line keeps its end points.
    fn smoothed(self) -> Self {
        let count = (self.total / SMOOTH_STEP).ceil() as usize;
        if count < 4 {
            return self;
        }
        let spacing = self.total / count as f32;
        let samples = if self.closed { count } else { count + 1 };
        let mut pts: Vec<Vec3> = (0..samples)
            .map(|i| self.point_at(i as f32 * spacing))
            .collect();
        const WEIGHTS: [f32; 5] = [1.0, 2.0, 3.0, 2.0, 1.0];
        for _ in 0..2 {
            let before = pts.clone();
            for (i, p) in pts.iter_mut().enumerate() {
                if !self.closed && (i == 0 || i + 1 == samples) {
                    continue;
                }
                let mut sum = Vec3::ZERO;
                for (k, w) in WEIGHTS.iter().enumerate() {
                    let j = i as isize + k as isize - 2;
                    let j = if self.closed {
                        j.rem_euclid(samples as isize) as usize
                    } else {
                        j.clamp(0, samples as isize - 1) as usize
                    };
                    sum += before[j] * *w;
                }
                *p = sum / 9.0;
            }
        }
        let mut cum = Vec::with_capacity(samples);
        let mut total = 0.0;
        cum.push(0.0);
        for w in pts.windows(2) {
            total += w[0].distance(w[1]);
            cum.push(total);
        }
        if self.closed {
            total += pts[samples - 1].distance(pts[0]);
        }
        // A source point keeps its place among the samples.
        let source_arc = self
            .source_arc
            .iter()
            .map(|&arc| {
                let at = (arc / spacing).clamp(0.0, count as f32);
                let i = (at.floor() as usize).min(samples - 1);
                let next = cum.get(i + 1).copied().unwrap_or(total);
                cum[i] + (next - cum[i]) * (at - i as f32).clamp(0.0, 1.0)
            })
            .collect();
        Self {
            points: pts,
            cum,
            closed: self.closed,
            total,
            source_arc,
        }
    }

    /// The index of the source point a car at arc length `s` is
    /// heading for: the first one past `s`. On a closed guide the
    /// stretch after the last point heads for point 0; on an open one
    /// the end of the guide returns the source's length — past its last
    /// point, the route consumed.
    pub fn chase_index(&self, s: f32) -> usize {
        let s = self.normalize(s);
        let i = self.source_arc.partition_point(|&a| a <= s);
        if self.closed && i >= self.source_arc.len() {
            0
        } else {
            i
        }
    }

    /// Arc length of source point `index` (clamped to the last one).
    pub fn source_arc(&self, index: usize) -> f32 {
        self.source_arc
            .get(index)
            .or(self.source_arc.last())
            .copied()
            .unwrap_or(0.0)
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
        self.locate_within(prev, pos, SEARCH_BACK, SEARCH_AHEAD)
    }

    /// The tracker's step for a car that drove: the arc length nearest
    /// `pos`, no further from `prev` than the car could have brought it
    /// by moving `moved` metres since `prev` was taken.
    ///
    /// A car's place on its line changes because the car moves. Tying
    /// the search window to the distance moved — [`FOLLOW_GAIN`] times
    /// it, so a car that cut a corner catches its place up — means a
    /// car that is lifted and put down elsewhere, or shoved while
    /// standing, does not find itself further along its route for it.
    /// Progress is what was driven.
    ///
    /// Nor does a car that has left its line: further than
    /// [`ON_LINE`] from it, its place stays where it left until it
    /// comes back. A car shunting about in a yard beside its route
    /// would otherwise creep its place forward abreast of itself.
    pub fn follow(&self, prev: f32, moved: f32, pos: Vec3) -> f32 {
        // Where the car is on the line, if it is on it at all...
        let target = self.locate_within(prev, pos, SEARCH_BACK, SEARCH_AHEAD);
        let at = self.point_at(target);
        if (pos.x - at.x).hypot(pos.z - at.z) > ON_LINE {
            return self.normalize(prev);
        }
        // ... and how far toward there its place may move this step.
        let reach = if moved.is_finite() {
            moved.max(0.0) * FOLLOW_GAIN
        } else {
            0.0
        };
        self.normalize(prev + self.delta(prev, target).clamp(-reach, reach))
    }

    /// The arc length at which the line passes closest to `point` in
    /// plan (XZ) — where a checkpoint off the line is bound to it.
    pub fn closest_approach(&self, point: Vec3) -> f32 {
        (0..self.legs())
            .map(|i| {
                let (a, b, start, end) = self.leg(i);
                let (dx, dz) = (b.x - a.x, b.z - a.z);
                let len2 = dx * dx + dz * dz;
                let t = if len2 > 1e-9 {
                    (((point.x - a.x) * dx + (point.z - a.z) * dz) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let miss = (a.x + dx * t - point.x).hypot(a.z + dz * t - point.z);
                (miss, start + (end - start) * t)
            })
            .min_by(|x, y| x.0.total_cmp(&y.0))
            .map_or(0.0, |(_, s)| s)
    }

    /// The arc length nearest `pos` between `back` metres behind `prev`
    /// and `ahead` metres past it — [`Guide::advance`] with the window
    /// spelled out, for a car that was moved rather than drove.
    pub fn locate_within(&self, prev: f32, pos: Vec3, back: f32, ahead: f32) -> f32 {
        let lo = prev - back;
        let hi = prev + ahead;
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
    fn the_chase_index_names_the_next_source_point() {
        let g = square();
        assert_eq!(g.chase_index(0.0), 1);
        assert_eq!(g.chase_index(99.0), 1);
        assert_eq!(g.chase_index(100.0), 2);
        assert_eq!(g.chase_index(350.0), 0, "the closing leg heads for point 0");
        let open = Guide::new(
            [
                Vec3::ZERO,
                Vec3::new(10.0, 0.0, 0.0),
                // A duplicate the guide merges away still counts.
                Vec3::new(10.0, 0.0, 0.0),
                Vec3::new(30.0, 0.0, 0.0),
            ],
            false,
        )
        .unwrap();
        assert_eq!(open.chase_index(5.0), 1);
        assert_eq!(open.chase_index(10.0), 3);
        assert_eq!(open.chase_index(30.0), 4, "the route is consumed");
    }

    #[test]
    fn smoothing_flattens_a_spur_and_keeps_the_course() {
        // 200 m east with a 6 m sideways jink at the half-way junction,
        // then a square corner north.
        let g = Guide::with_sources(
            [
                (Vec3::new(0.0, 0.0, 0.0), true),
                (Vec3::new(96.0, 0.0, 0.0), false),
                (Vec3::new(100.0, 0.0, 6.0), true),
                (Vec3::new(104.0, 0.0, 0.0), false),
                (Vec3::new(200.0, 0.0, 0.0), true),
                (Vec3::new(200.0, 0.0, -150.0), true),
            ],
            false,
        )
        .unwrap();
        let before = g.locate(Vec3::new(100.0, 0.0, 6.0));
        assert!(g.point_at(before).z > 5.9, "the spur is there");
        let s = g.smoothed();
        let spur = s.point_at(s.locate(Vec3::new(100.0, 0.0, 6.0)));
        assert!(spur.z < 2.5, "the spur is mostly gone: {spur:?}");
        // The ends stay put and the corner is still a corner, rounded.
        assert_eq!(s.point_at(0.0), Vec3::ZERO);
        assert!(
            s.point_at(s.length())
                .distance(Vec3::new(200.0, 0.0, -150.0))
                < 1e-3
        );
        let corner = s.point_at(s.locate(Vec3::new(200.0, 0.0, 0.0)));
        assert!(
            corner.distance(Vec3::new(200.0, 0.0, 0.0)) < 5.0,
            "{corner:?}"
        );
        // Source points keep their order and roughly their place.
        assert_eq!(s.chase_index(50.0), 1);
        assert_eq!(s.chase_index(150.0), 2);
        assert_eq!(s.chase_index(s.length() - 20.0), 3);
        assert!((s.source_arc(2) - 200.0).abs() < 8.0, "{}", s.source_arc(2));
    }

    #[test]
    fn following_moves_only_as_far_as_the_car_did() {
        let g = Guide::new([Vec3::ZERO, Vec3::new(1000.0, 0.0, 0.0)], false).unwrap();
        // Driven 2 m: the place moves with it.
        let s = g.follow(100.0, 2.0, Vec3::new(102.0, 0.0, 0.5));
        assert!((s - 102.0).abs() < 1e-3);
        // Put down 60 m further on without having moved: no gain.
        assert_eq!(g.follow(100.0, 0.0, Vec3::new(160.0, 0.0, 0.0)), 100.0);
        // Having moved 4 m, at most three times that.
        let s = g.follow(100.0, 4.0, Vec3::new(110.0, 0.0, 20.0));
        assert!((s - 110.0).abs() < 1e-3, "{s}");
        let s = g.follow(100.0, 2.0, Vec3::new(110.0, 0.0, 20.0));
        assert!((s - 106.0).abs() < 1e-3, "{s}");
        // Off its line, its place stays where it left.
        assert_eq!(g.follow(100.0, 4.0, Vec3::new(104.0, 0.0, 40.0)), 100.0);
    }

    #[test]
    fn the_closest_approach_binds_a_point_off_the_line() {
        let g = square();
        assert!((g.closest_approach(Vec3::new(40.0, 5.0, -30.0)) - 40.0).abs() < 1e-3);
        assert!((g.closest_approach(Vec3::new(130.0, 0.0, 50.0)) - 150.0).abs() < 1e-3);
    }

    #[test]
    fn degenerate_input_makes_no_guide() {
        assert!(Guide::new([Vec3::ZERO, Vec3::ZERO], false).is_none());
        assert!(Guide::new([Vec3::ZERO, Vec3::splat(f32::NAN)], false).is_none());
    }
}
