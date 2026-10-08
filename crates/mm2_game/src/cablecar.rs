//! The cable car's route and motion — the one special actor the retail
//! executable builds from the AI map instead of a pathset.
//!
//! The car is an ordinary AI vehicle object (`va_cablecar_f`) that
//! follows a road's tram curve and, at the end of the road, hops onto
//! the next tram road (`mm2_formats::bai::Bai::tram_hop`). Its speed
//! controller is small and was read from `Midtown2.exe`
//! (`docs/research/specials.md` § *How a cable car drives*):
//!
//! - it accelerates at its own constant rate ([`cable_accel`]: 1.5 plus
//!   a random 0–2 m/s², drawn once when the car is built) up to
//!   [`CABLE_CRUISE_SPEED`], 15 m/s;
//! - inside [`CABLE_LOOKAHEAD`] (25 m) of the end of a road it asks
//!   whether it may go on — the junction's rule and signal. Yes: it keeps
//!   its speed. No: it brakes at the constant rate that stops it
//!   [`CABLE_STOP_GAP`] short of the end, waits, and goes the moment the
//!   answer turns yes. A yes is final until the next road;
//! - something ahead within [`CABLE_OBSTACLE_RANGE`] makes it brake to
//!   stop [`CABLE_OBSTACLE_GAP`] behind it, and it sets off again when
//!   nothing is ahead;
//! - between roads (inside the junction) it never brakes.
//!
//! [`CableRoute`] is the closed sequence of tram curves one circuit
//! drives, smoothed through the junction hops and sampled into an
//! arc-length table; [`CableMotion`] is the controller walking it. The
//! spline the original draws the junction hop with is not reproduced —
//! the hop is a smooth curve of this module's own (an implementation
//! choice).

use bevy::math::Vec3;

/// The cable car's model (`geometry/va_cablecar_f.pkg`), named in the
/// executable's AI-map init.
pub const CABLE_CAR_MODEL: &str = "va_cablecar_f";
/// Cruise speed, m/s (a constant in the executable's data, `0x5d6d5c`).
pub const CABLE_CRUISE_SPEED: f32 = 15.0;
/// Distance from the end of a road at which the car starts asking
/// whether it may go on, m (`0x5d6d58`).
pub const CABLE_LOOKAHEAD: f32 = 25.0;
/// The lowest acceleration a car draws, m/s².
pub const CABLE_ACCEL_MIN: f32 = 1.5;
/// The width of the acceleration draw, m/s².
pub const CABLE_ACCEL_SPREAD: f32 = 2.0;
/// How far short of the end of a road a braking car aims to stop, m.
pub const CABLE_STOP_GAP: f32 = 0.25;
/// How far behind an obstacle a braking car aims to stop, m.
pub const CABLE_OBSTACLE_GAP: f32 = 2.5;
/// How far ahead the car looks for an obstacle, m (the probe the
/// executable casts is 30 m long and 2 m wide).
pub const CABLE_OBSTACLE_RANGE: f32 = 30.0;
/// Below this speed a car braking within [`CABLE_HALT_DISTANCE`] of the
/// end of its road counts as stopped, m/s.
pub const CABLE_HALT_SPEED: f32 = 0.2;
/// See [`CABLE_HALT_SPEED`], m.
pub const CABLE_HALT_DISTANCE: f32 = 0.5;
/// The target speed the controller sets: a hair over cruise so the
/// clamp that follows lands exactly on it.
const TARGET_SPEED: f32 = CABLE_CRUISE_SPEED + 0.0001;
/// A car this close under its target speed while braking is stopped
/// there and then.
const SNAP_SPEED: f32 = 0.05;
/// Half the chord the heading is read over, m.
const HEADING_SPAN: f32 = 2.0;
/// Spacing of the route's arc-length table, m.
const SAMPLE_SPACING: f32 = 1.0;
/// The most key points a route accepts, and the most samples (and per
/// segment) its table may hold — bounds on what a malformed map can ask
/// the builder for.
pub const MAX_ROUTE_POINTS: usize = 1 << 16;
const MAX_ROUTE_SAMPLES: usize = 1 << 21;
const MAX_SEGMENT_SAMPLES: f32 = 1e5;

/// A car's acceleration from the draw in `0..1` the original takes
/// from `rand() / 32768`.
pub fn cable_accel(draw: f32) -> f32 {
    CABLE_ACCEL_MIN + CABLE_ACCEL_SPREAD * draw
}

/// Where one leg of the circuit lies along the route.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CableLeg {
    /// Arc length at the leg's first curve point.
    pub start: f32,
    /// Arc length at its last curve point — the end of the road, where a
    /// car stops for a red light. What follows, up to the next leg's
    /// `start`, is the junction.
    pub line: f32,
}

/// A closed circuit of tram curves, as an arc-length table.
#[derive(Debug, Clone)]
pub struct CableRoute {
    points: Vec<Vec3>,
    /// `cum[i]` is the arc length at `points[i]`; one entry longer than
    /// `points` — the last is the circuit's length, back at the start.
    cum: Vec<f32>,
    legs: Vec<CableLeg>,
}

impl CableRoute {
    /// The route through `legs`, each a tram curve in travel order, the
    /// last hopping back onto the first. `None` for no legs, a leg of
    /// fewer than two points, a non-finite point, a degenerate (zero
    /// length) circuit, more than [`MAX_ROUTE_POINTS`] points, or a
    /// circuit too long to tabulate at one-metre spacing.
    pub fn new(legs: &[Vec<Vec3>]) -> Option<Self> {
        let total: usize = legs.iter().map(Vec::len).sum();
        if legs.is_empty()
            || total > MAX_ROUTE_POINTS
            || legs.iter().any(|l| l.len() < 2)
            || legs.iter().flatten().any(|p| !p.is_finite())
        {
            return None;
        }
        let keys: Vec<Vec3> = legs.iter().flatten().copied().collect();
        let n = keys.len();
        // The travel direction at each key point. A road's own curve is
        // authoritative, so its end points take their direction from the
        // road alone — the junction hop is what bends to meet them.
        let mut dirs = Vec::with_capacity(n);
        for leg in legs {
            for (j, &p) in leg.iter().enumerate() {
                let (from, to) = match j {
                    0 => (p, leg[1]),
                    j if j + 1 == leg.len() => (leg[j - 1], p),
                    j => (leg[j - 1], leg[j + 1]),
                };
                dirs.push((to - from).try_normalize().unwrap_or(Vec3::ZERO));
            }
        }
        let mut points = Vec::new();
        let mut first_sample = Vec::with_capacity(n);
        for i in 0..n {
            first_sample.push(points.len());
            let (a, b) = (keys[i], keys[(i + 1) % n]);
            let chord = a.distance(b);
            if chord <= f32::EPSILON {
                // A repeated point adds no length; keep one sample so the
                // key point still has an entry.
                points.push(a);
                continue;
            }
            let steps = (chord / SAMPLE_SPACING).ceil().max(2.0);
            if steps > MAX_SEGMENT_SAMPLES || points.len() + steps as usize > MAX_ROUTE_SAMPLES {
                return None;
            }
            // Tangents as long as the segment, so a short hop next to a
            // long curve segment cannot overshoot.
            let (t0, t1) = (dirs[i] * chord, dirs[(i + 1) % n] * chord);
            let steps = steps as usize;
            for k in 0..steps {
                points.push(hermite(a, b, t0, t1, k as f32 / steps as f32));
            }
        }
        let mut table = Vec::with_capacity(points.len() + 1);
        let mut acc = 0.0f32;
        table.push(0.0);
        for (i, p) in points.iter().enumerate() {
            acc += p.distance(points[(i + 1) % points.len()]);
            table.push(acc);
        }
        if acc <= f32::EPSILON {
            return None;
        }
        let mut out = Vec::with_capacity(legs.len());
        let mut at = 0usize;
        for leg in legs {
            let (first, last) = (at, at + leg.len() - 1);
            out.push(CableLeg {
                start: table[first_sample[first]],
                line: table[first_sample[last]],
            });
            at += leg.len();
        }
        Some(Self {
            points,
            cum: table,
            legs: out,
        })
    }

    /// The circuit's length, m.
    pub fn length(&self) -> f32 {
        self.cum.last().copied().unwrap_or(0.0)
    }

    /// The legs, in driving order.
    pub fn legs(&self) -> &[CableLeg] {
        &self.legs
    }

    fn wrap(&self, s: f32) -> f32 {
        let len = self.length();
        if s.is_finite() {
            s.rem_euclid(len)
        } else {
            0.0
        }
    }

    /// Position and unit travel direction at arc length `s` (wrapped
    /// onto the circuit). The direction is the chord over
    /// [`HEADING_SPAN`] metres either side, so a tight turn (a terminus
    /// turnaround) swings the heading over several steps instead of
    /// snapping it from sample to sample.
    pub fn pose(&self, s: f32) -> (Vec3, Vec3) {
        let here = self.position(s);
        let dir = (self.position(s + HEADING_SPAN) - self.position(s - HEADING_SPAN))
            .try_normalize()
            .unwrap_or(Vec3::Z);
        (here, dir)
    }

    fn position(&self, s: f32) -> Vec3 {
        let s = self.wrap(s);
        let i = self
            .cum
            .partition_point(|&c| c <= s)
            .saturating_sub(1)
            .min(self.points.len() - 1);
        let a = self.points[i];
        let b = self.points[(i + 1) % self.points.len()];
        let span = self.cum[i + 1] - self.cum[i];
        let t = if span > f32::EPSILON {
            ((s - self.cum[i]) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        a.lerp(b, t)
    }

    /// The leg arc length `s` lies on or just after: the last leg that
    /// has started.
    pub fn leg_at(&self, s: f32) -> usize {
        let s = self.wrap(s);
        self.legs
            .partition_point(|l| l.start <= s)
            .saturating_sub(1)
    }

    /// Distance from `s` to the end of its road, or `None` while it is
    /// in the junction after one.
    pub fn distance_to_line(&self, s: f32) -> Option<f32> {
        let s = self.wrap(s);
        let line = self.legs[self.leg_at(s)].line;
        (s <= line).then_some(line - s)
    }

    /// How far ahead of `from` (along the circuit) `to` lies, in
    /// `0..length`.
    pub fn gap_ahead(&self, from: f32, to: f32) -> f32 {
        (self.wrap(to) - self.wrap(from)).rem_euclid(self.length())
    }

    /// The nearest body on the track ahead of arc length `from` (the
    /// car's nose): for each body, the distance along the route to the
    /// [`SAMPLE_SPACING`] sample nearest its centre, if that sample is
    /// within the [`CableCorridor`], less the body's own half-length and
    /// floored at 0. The corridor follows the curve (a body beside the
    /// track on a bend is not ahead of the car), and a body behind
    /// `from` is not ahead. `None` when no body stands within `range`.
    pub fn blocker_gap(
        &self,
        from: f32,
        range: f32,
        corridor: &CableCorridor,
        blockers: &[Vec3],
    ) -> Option<f32> {
        if blockers.is_empty() || !range.is_finite() || range <= 0.0 {
            return None;
        }
        let steps = (range.min(self.length()) / SAMPLE_SPACING).ceil() as usize;
        let lift = Vec3::Y * corridor.lift;
        let track: Vec<Vec3> = (0..=steps)
            .map(|k| self.position(from + k as f32 * SAMPLE_SPACING) + lift)
            .collect();
        blockers
            .iter()
            .filter_map(|b| {
                let (k, dist) = track
                    .iter()
                    .enumerate()
                    .map(|(k, c)| (k, (b.x - c.x).hypot(b.z - c.z)))
                    .min_by(|a, b| a.1.total_cmp(&b.1))?;
                let rise = (b.y - track[k].y).abs();
                (dist <= corridor.clearance && rise <= corridor.max_rise)
                    .then(|| (k as f32 * SAMPLE_SPACING - corridor.blocker_reach).max(0.0))
            })
            .min_by(f32::total_cmp)
    }
}

/// The strip of track a body must stand in to count as in the car's way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CableCorridor {
    /// Horizontal distance from the track at which a body's centre blocks
    /// the car: the car's half-width plus a body's, m.
    pub clearance: f32,
    /// How far above or below the track a body may be and still block it
    /// (keeps an overpass from stopping the car under it), m.
    pub max_rise: f32,
    /// Height of the car's body origin above the track curve, m.
    pub lift: f32,
    /// How much of a blocker lies ahead of its centre, m: the distance to
    /// its centre overstates the room in front of its bumper by this.
    pub blocker_reach: f32,
}

impl CableCorridor {
    /// The corridor for a car `half_width` wide whose origin rides `lift`
    /// above the curve. Blocker extents are those of an ordinary car:
    /// 1 m half-width, 2 m half-length; 2.5 m of height.
    pub fn for_car(half_width: f32, lift: f32) -> Self {
        Self {
            clearance: half_width + 1.0,
            max_rise: 2.5,
            lift,
            blocker_reach: 2.0,
        }
    }
}

fn hermite(p0: Vec3, p1: Vec3, t0: Vec3, t1: Vec3, t: f32) -> Vec3 {
    let (t2, t3) = (t * t, t * t * t);
    p0 * (2.0 * t3 - 3.0 * t2 + 1.0)
        + t0 * (t3 - 2.0 * t2 + t)
        + p1 * (-2.0 * t3 + 3.0 * t2)
        + t1 * (t3 - t2)
}

/// What the car senses this step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CableSense {
    /// May the car go on past the end of its road? The caller's
    /// junction decision (rule and signal) for the car's current road;
    /// read only while the car is inside [`CABLE_LOOKAHEAD`] of the end.
    pub gate_open: bool,
    /// Distance to the nearest thing ahead on the car's lane, from the
    /// car's nose, if any.
    pub obstacle: Option<f32>,
}

impl CableSense {
    /// A clear road and a green light.
    pub const CLEAR: Self = Self {
        gate_open: true,
        obstacle: None,
    };
}

/// One cable car's speed controller and place on its [`CableRoute`].
#[derive(Debug, Clone)]
pub struct CableMotion {
    /// Arc length along the route, wrapped onto it.
    pub s: f32,
    /// Speed, m/s.
    pub speed: f32,
    accel: f32,
    target: f32,
    accel_mag: f32,
    cleared: bool,
    leg: usize,
}

impl CableMotion {
    /// A car at rest at the start of `leg`, with the acceleration the
    /// draw (`0..1`) gives it.
    pub fn new(route: &CableRoute, leg: usize, draw: f32) -> Self {
        let leg = leg.min(route.legs().len() - 1);
        Self {
            s: route.legs()[leg].start,
            speed: 0.0,
            accel: 0.0,
            target: 0.0,
            accel_mag: cable_accel(draw),
            cleared: false,
            leg,
        }
    }

    /// The leg the car is on (or has just left, while in the junction).
    pub fn leg(&self) -> usize {
        self.leg
    }

    /// The car's own acceleration, m/s².
    pub fn acceleration(&self) -> f32 {
        self.accel_mag
    }

    /// Whether the car has been cleared to go on past the end of its
    /// current road.
    pub fn cleared(&self) -> bool {
        self.cleared
    }

    /// Distance from the car's nose to the end of its road, `nose`
    /// metres ahead of `s`; `None` in the junction.
    pub fn distance_to_line(&self, route: &CableRoute, nose: f32) -> Option<f32> {
        route.distance_to_line(self.s).map(|d| d - nose)
    }

    fn go(&mut self) {
        if self.target < CABLE_CRUISE_SPEED {
            self.accel = self.accel_mag;
            self.target = TARGET_SPEED;
        }
    }

    fn brake_to(&mut self, distance: f32) {
        // A distance the car cannot stop in is a hard stop, not a
        // negative one (which would accelerate it).
        let room = distance.max(0.05);
        self.accel = -(self.speed * self.speed) / (2.0 * room);
        self.target = 0.0;
    }

    /// Advance `dt` seconds. `nose` is how far ahead of the car's origin
    /// its front is, so it stops with the front — not the middle — at
    /// the line.
    pub fn step(&mut self, route: &CableRoute, dt: f32, nose: f32, sense: CableSense) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        let leg = route.leg_at(self.s);
        if leg != self.leg {
            // A new road: the last road's answer is spent.
            self.leg = leg;
            self.cleared = false;
        }
        let to_line = self.distance_to_line(route, nose);
        let obstacle = to_line
            .and(sense.obstacle)
            .filter(|&d| d < CABLE_OBSTACLE_RANGE);
        if let Some(gap) = obstacle {
            if self.target != 0.0 {
                self.brake_to(gap - CABLE_OBSTACLE_GAP);
            }
        } else if let Some(d) = to_line.filter(|&d| d > 0.0 && d < CABLE_LOOKAHEAD) {
            if !self.cleared && sense.gate_open {
                self.cleared = true;
            }
            if self.cleared {
                self.go();
            } else if self.target != 0.0 {
                self.brake_to(d - CABLE_STOP_GAP);
            } else if self.speed < CABLE_HALT_SPEED && d < CABLE_HALT_DISTANCE {
                self.speed = 0.0;
            }
        } else {
            self.go();
        }
        self.speed += dt * self.accel;
        let braking_done = self.speed < self.target + SNAP_SPEED && self.accel < 0.0;
        let overshot = self.speed > self.target && self.accel > 0.0;
        if braking_done || overshot {
            self.speed = self.target;
            self.accel = 0.0;
        }
        self.speed = self.speed.max(0.0);
        self.s = route.wrap(self.s + dt * self.speed);
        self.leg = self.leg.min(route.legs().len() - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;
    /// The aimed-for stop gap plus the step-size error of the controller's
    /// semi-implicit Euler (about `a·dt·T/2` ≈ 0.12 m at 60 Hz).
    const STOP_SLACK: f32 = CABLE_STOP_GAP + 0.25;

    /// One leg 200 m long along +x, then a 4 m junction hop back to the
    /// start of the (same) leg: a route of one road.
    fn straight(len: f32) -> CableRoute {
        let leg: Vec<Vec3> = (0..=4)
            .map(|i| Vec3::new(i as f32 * len / 4.0, 0.0, 0.0))
            .collect();
        // Close the circuit with a far-away return so the hop is long
        // enough to be a junction rather than a kink.
        CableRoute::new(&[leg]).unwrap()
    }

    fn run(
        route: &CableRoute,
        car: &mut CableMotion,
        seconds: f32,
        mut sense: impl FnMut(&CableMotion) -> CableSense,
    ) {
        for _ in 0..(seconds / DT).round() as usize {
            let s = sense(car);
            car.step(route, DT, 0.0, s);
        }
    }

    #[test]
    fn acceleration_is_drawn_from_one_and_a_half_to_three_and_a_half() {
        assert_eq!(cable_accel(0.0), 1.5);
        assert_eq!(cable_accel(0.5), 2.5);
        assert!((cable_accel(32767.0 / 32768.0) - 3.5).abs() < 1e-3);
    }

    #[test]
    fn a_route_needs_a_circuit_to_drive() {
        assert!(CableRoute::new(&[]).is_none());
        assert!(CableRoute::new(&[vec![Vec3::ZERO]]).is_none());
        assert!(CableRoute::new(&[vec![Vec3::ZERO, Vec3::new(f32::NAN, 0.0, 0.0)]]).is_none());
        assert!(
            CableRoute::new(&[vec![Vec3::ONE, Vec3::ONE]]).is_none(),
            "a circuit of one point has no length"
        );
    }

    #[test]
    fn a_route_marks_where_each_road_starts_and_ends() {
        let a: Vec<Vec3> = vec![Vec3::ZERO, Vec3::new(100.0, 0.0, 0.0)];
        let b: Vec<Vec3> = vec![Vec3::new(110.0, 0.0, 0.0), Vec3::new(110.0, 0.0, 100.0)];
        let r = CableRoute::new(&[a, b]).unwrap();
        let legs = r.legs();
        assert_eq!(legs[0].start, 0.0);
        assert!((legs[0].line - 100.0).abs() < 0.5);
        assert!(legs[1].start > legs[0].line, "a junction lies between");
        assert!(legs[1].start - legs[0].line < 25.0);
        assert!((legs[1].line - legs[1].start - 100.0).abs() < 1.0);
        assert!(r.length() > 300.0);
        assert_eq!(r.leg_at(50.0), 0);
        assert_eq!(r.leg_at(legs[1].start + 1.0), 1);
        assert_eq!(r.distance_to_line(40.0).map(|d| d.round()), Some(60.0));
        assert_eq!(r.distance_to_line(legs[0].line + 1.0), None);
        // The pose follows the curve and wraps.
        let (p, d) = r.pose(50.0);
        assert!((p - Vec3::new(50.0, 0.0, 0.0)).length() < 0.5);
        assert!(d.dot(Vec3::X) > 0.99);
        assert_eq!(r.pose(50.0 + r.length()).0.round(), p.round());
        assert!((r.gap_ahead(10.0, 30.0) - 20.0).abs() < 1e-4);
        assert!((r.gap_ahead(30.0, 10.0) - (r.length() - 20.0)).abs() < 1e-3);
    }

    fn corridor() -> CableCorridor {
        CableCorridor::for_car(1.25, 0.0)
    }

    #[test]
    fn a_body_on_the_track_ahead_is_the_gap_to_its_front_edge() {
        let r = straight(200.0);
        let at_40 = [Vec3::new(40.0, 0.0, 0.0)];
        // Nose at 10 m: the centre is 30 m on; its own half-length is
        // room the car does not have.
        let gap = r.blocker_gap(10.0, 30.0, &corridor(), &at_40).unwrap();
        assert!((gap - 28.0).abs() <= SAMPLE_SPACING, "gap {gap}");
        // Out of range, behind, beside, above: not in the way.
        assert!(r.blocker_gap(10.0, 20.0, &corridor(), &at_40).is_none());
        assert!(r.blocker_gap(60.0, 30.0, &corridor(), &at_40).is_none());
        let aside = [Vec3::new(40.0, 0.0, 6.0)];
        assert!(r.blocker_gap(10.0, 30.0, &corridor(), &aside).is_none());
        let above = [Vec3::new(40.0, 6.0, 0.0)];
        assert!(r.blocker_gap(10.0, 30.0, &corridor(), &above).is_none());
        // Within the strip beside the rails it still blocks.
        let near = [Vec3::new(40.0, 1.0, 2.0)];
        assert!(r.blocker_gap(10.0, 30.0, &corridor(), &near).is_some());
        // The nearest of several wins; a body on the nose is a zero gap.
        let two = [Vec3::new(40.0, 0.0, 0.0), Vec3::new(25.0, 0.0, 0.0)];
        let gap = r.blocker_gap(10.0, 30.0, &corridor(), &two).unwrap();
        assert!((gap - 13.0).abs() <= SAMPLE_SPACING, "gap {gap}");
        let on_nose = [Vec3::new(10.5, 0.0, 0.0)];
        assert_eq!(r.blocker_gap(10.0, 30.0, &corridor(), &on_nose), Some(0.0));
    }

    #[test]
    fn nothing_or_a_nonsense_range_senses_nothing() {
        let r = straight(200.0);
        let body = [Vec3::new(20.0, 0.0, 0.0)];
        assert!(r.blocker_gap(0.0, 30.0, &corridor(), &[]).is_none());
        assert!(r.blocker_gap(0.0, 0.0, &corridor(), &body).is_none());
        assert!(r.blocker_gap(0.0, -3.0, &corridor(), &body).is_none());
        assert!(r.blocker_gap(0.0, f32::NAN, &corridor(), &body).is_none());
        assert!(
            r.blocker_gap(0.0, f32::INFINITY, &corridor(), &body)
                .is_none()
        );
        assert!(
            r.blocker_gap(f32::NAN, 30.0, &corridor(), &body).is_none(),
            "a poisoned arc length finds nothing rather than panicking"
        );
    }

    #[test]
    fn a_body_round_a_bend_is_found_along_the_curve_not_the_chord() {
        // 100 m east then 100 m south (+z): the corner is a hop of a few
        // metres. A body 20 m down the second road is 'ahead' by the
        // route even though the straight line to it cuts the corner.
        let a = vec![Vec3::new(-100.0, 0.0, 0.0), Vec3::ZERO];
        let b = vec![Vec3::new(4.0, 0.0, 4.0), Vec3::new(4.0, 0.0, 104.0)];
        let r = CableRoute::new(&[a, b]).unwrap();
        let down = [Vec3::new(4.0, 0.0, 24.0)];
        let from = r.legs()[0].line - 5.0;
        let gap = r.blocker_gap(from, 30.0, &corridor(), &down).unwrap();
        assert!((20.0..=30.0).contains(&gap), "gap {gap}");
        // A body on the first road's line extended past the corner (off
        // the track) is not on the route.
        let off = [Vec3::new(20.0, 0.0, 0.0)];
        assert!(r.blocker_gap(from, 30.0, &corridor(), &off).is_none());
    }

    #[test]
    fn a_short_hop_between_long_curves_does_not_loop_back() {
        // Two 90 m roads joined by a 3 m hop that turns a corner: the
        // smoothed hop stays near the 3 m between them.
        let a = vec![Vec3::new(-90.0, 0.0, 0.0), Vec3::ZERO];
        let b = vec![Vec3::new(3.0, 0.0, 3.0), Vec3::new(3.0, 0.0, 93.0)];
        let r = CableRoute::new(&[a, b]).unwrap();
        let (from, to) = (r.legs()[0].line, r.legs()[1].start);
        assert!((to - from - 4.3).abs() < 1.0, "hop of {}", to - from);
        let mut s = from;
        while s <= to {
            let p = r.pose(s).0;
            assert!(
                p.x > -1.0 && p.x < 5.0 && p.z > -1.0 && p.z < 5.0,
                "{p:?} at {s}"
            );
            s += 0.1;
        }
        // The roads themselves keep their own line.
        assert!((r.pose(r.legs()[0].start + 45.0).0 - Vec3::new(-45.0, 0.0, 0.0)).length() < 0.1);
    }

    #[test]
    fn a_car_accelerates_to_cruise_and_holds_it() {
        let route = straight(2000.0);
        let mut car = CableMotion::new(&route, 0, 0.0);
        run(&route, &mut car, 5.0, |_| CableSense::CLEAR);
        assert!(
            (car.speed - 7.5).abs() < 0.2,
            "1.5 m/s² for 5 s: {}",
            car.speed
        );
        run(&route, &mut car, 10.0, |_| CableSense::CLEAR);
        assert!(
            (car.speed - CABLE_CRUISE_SPEED).abs() < 0.001,
            "{}",
            car.speed
        );
        let before = car.s;
        run(&route, &mut car, 1.0, |_| CableSense::CLEAR);
        assert!((car.s - before - CABLE_CRUISE_SPEED).abs() < 0.2);
    }

    #[test]
    fn a_red_light_stops_the_car_at_the_line_and_green_sends_it_on() {
        let route = straight(400.0);
        let line = route.legs()[0].line;
        let mut car = CableMotion::new(&route, 0, 0.5);
        let red = |_: &CableMotion| CableSense {
            gate_open: false,
            obstacle: None,
        };
        run(&route, &mut car, 60.0, red);
        assert_eq!(car.speed, 0.0, "stopped");
        let to_line = line - car.s;
        assert!(
            (0.0..=STOP_SLACK).contains(&to_line),
            "stopped {to_line} m short of the line"
        );
        assert!(!car.cleared());
        // Green: it goes, crosses the line and gets back to cruise.
        run(&route, &mut car, 1.0, |_| CableSense::CLEAR);
        assert!(car.speed > 1.0);
        run(&route, &mut car, 20.0, |_| CableSense::CLEAR);
        assert!(car.s > line || car.leg() == 0);
        assert!((car.speed - CABLE_CRUISE_SPEED).abs() < 0.001);
    }

    #[test]
    fn a_light_that_turns_red_after_the_car_was_cleared_does_not_stop_it() {
        let route = straight(400.0);
        let line = route.legs()[0].line;
        let mut car = CableMotion::new(&route, 0, 0.5);
        // Green until the car is a metre inside the lookahead, red from
        // then on: it was cleared, so the red is not its answer any more.
        let mut slowest = f32::MAX;
        for _ in 0..(30.0 / DT).round() as usize {
            let gate_open = line - car.s > CABLE_LOOKAHEAD - 1.0;
            car.step(
                &route,
                DT,
                0.0,
                CableSense {
                    gate_open,
                    obstacle: None,
                },
            );
            if car.s > 80.0 {
                slowest = slowest.min(car.speed);
            }
        }
        assert!(car.s > line, "stopped short of the line at {}", car.s);
        assert!(slowest > CABLE_CRUISE_SPEED - 0.01, "slowed to {slowest}");
    }

    #[test]
    fn the_junction_is_never_braked_for() {
        let route = CableRoute::new(&[
            vec![Vec3::ZERO, Vec3::new(100.0, 0.0, 0.0)],
            vec![Vec3::new(120.0, 0.0, 0.0), Vec3::new(220.0, 0.0, 0.0)],
        ])
        .unwrap();
        let mut car = CableMotion::new(&route, 0, 0.5);
        // Cleared into the junction, then every sense turns hostile: a
        // closed gate and an obstacle, neither of which applies there.
        let line = route.legs()[0].line;
        let mut crossed = false;
        for _ in 0..(20.0 / DT).round() as usize {
            let in_junction = car.s > line && car.s < route.legs()[1].start;
            let sense = CableSense {
                gate_open: !in_junction && car.s < line,
                obstacle: in_junction.then_some(1.0),
            };
            car.step(&route, DT, 0.0, sense);
            if in_junction {
                crossed = true;
                assert!(car.speed > 10.0, "slowed in the junction: {}", car.speed);
            }
        }
        assert!(crossed, "the car reached the junction");
    }

    #[test]
    fn an_obstacle_ahead_stops_the_car_short_and_clearing_it_resumes() {
        let route = straight(2000.0);
        let mut car = CableMotion::new(&route, 0, 0.5);
        run(&route, &mut car, 12.0, |_| CableSense::CLEAR);
        assert!((car.speed - CABLE_CRUISE_SPEED).abs() < 0.001);
        // A stopped car 20 m ahead of the nose.
        let wall = car.s + 20.0;
        run(&route, &mut car, 10.0, |c| CableSense {
            gate_open: true,
            obstacle: Some(wall - c.s),
        });
        assert_eq!(car.speed, 0.0);
        let gap = wall - car.s;
        assert!(
            (CABLE_OBSTACLE_GAP - 0.2..=CABLE_OBSTACLE_GAP + 0.2).contains(&gap),
            "stopped {gap} m behind the obstacle"
        );
        // It was out of range 30 m back: the obstacle leaves, the car goes.
        run(&route, &mut car, 2.0, |_| CableSense::CLEAR);
        assert!(car.speed > 1.0);
    }

    #[test]
    fn an_obstacle_inside_the_gap_is_a_hard_stop_not_a_surge() {
        let route = straight(2000.0);
        let mut car = CableMotion::new(&route, 0, 0.5);
        run(&route, &mut car, 12.0, |_| CableSense::CLEAR);
        let s0 = car.s;
        car.step(
            &route,
            DT,
            0.0,
            CableSense {
                gate_open: true,
                obstacle: Some(1.0),
            },
        );
        assert!(car.speed < CABLE_CRUISE_SPEED, "no surge: {}", car.speed);
        assert!(car.s >= s0);
    }

    #[test]
    fn a_degenerate_step_leaves_the_car_alone() {
        let route = straight(400.0);
        let mut car = CableMotion::new(&route, 0, 0.5);
        for dt in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            car.step(&route, dt, 0.0, CableSense::CLEAR);
        }
        assert_eq!((car.s, car.speed), (0.0, 0.0));
        // And a leg index past the end is clamped, not a panic.
        assert_eq!(CableMotion::new(&route, 9, 0.5).leg(), 0);
    }

    #[test]
    fn the_nose_stops_the_front_not_the_middle_at_the_line() {
        let route = straight(400.0);
        let line = route.legs()[0].line;
        let mut car = CableMotion::new(&route, 0, 0.5);
        for _ in 0..(60.0 / DT).round() as usize {
            car.step(
                &route,
                DT,
                4.0,
                CableSense {
                    gate_open: false,
                    obstacle: None,
                },
            );
        }
        assert_eq!(car.speed, 0.0);
        let front_short = line - (car.s + 4.0);
        assert!((0.0..=STOP_SLACK).contains(&front_short), "{front_short}");
    }
}
