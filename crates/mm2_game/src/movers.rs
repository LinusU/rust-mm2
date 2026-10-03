//! Moving scenery — the retail sailboat, ferry and Underground
//! managers.
//!
//! Each manager reads its `race/<city>/<city>_<object>[_<event>].pathset`
//! ([`crate::object_pathset_candidates`]) and drives one object per
//! path (three cars per path for the train) with the same spline
//! follower. Everything here is recovered from the retail executable —
//! see `docs/research/movers.md`:
//!
//! - [`PathFollower`]: a closed-loop Catmull-Rom curve through the path
//!   points (Hermite segments, tangents `(P[i+1] − P[i−1]) / 2`, indices
//!   wrapping), advanced at a constant speed in metres per second
//!   against a per-segment length estimated through the segment's
//!   midpoint. A two-point path is a parked object at its first point
//!   facing its second.
//! - Objects face their local +Z along the curve's tangent, local +Y
//!   kept up ([`mover_rotation`]).
//! - Sailboats (tugs, water taxis, ducks): speed = the path's spacing ±
//!   [`SAILBOAT_SPEED_SPREAD`]; ferries: [`FERRY_SPEED`].
//! - The train: [`TRAIN_CARS`] cars [`TRAIN_CAR_GAP`] seconds apart at
//!   [`TRAIN_SPEED`], shuttling end to end — [`TrainMotion`].

use bevy::math::{Quat, Vec3};

/// Ferry speed, m/s (the manager's base speed with zero spread).
pub const FERRY_SPEED: f32 = 0.75;
/// Sailboat speeds are drawn uniformly within this many m/s of the
/// path's spacing.
pub const SAILBOAT_SPEED_SPREAD: f32 = 1.0;
/// Train speed, m/s.
pub const TRAIN_SPEED: f32 = 40.0;
/// Cars per train.
pub const TRAIN_CARS: usize = 3;
/// Seconds of travel between consecutive cars (17.6 m at speed).
pub const TRAIN_CAR_GAP: f32 = 0.44;
/// Seconds a train waits at each end of its line.
pub const TRAIN_WAIT: f32 = 10.0;
/// Rate the train's speed fraction ramps up and down, per second.
pub const TRAIN_RAMP: f32 = 0.51;

/// One Hermite segment, as cubic coefficients per axis
/// (`p(t) = ((a·t + b)·t + c)·t + d`).
#[derive(Debug, Clone, Copy, Default)]
struct Segment {
    a: Vec3,
    b: Vec3,
    c: Vec3,
    d: Vec3,
}

impl Segment {
    fn hermite(p0: Vec3, p1: Vec3, t0: Vec3, t1: Vec3) -> Self {
        Self {
            a: 2.0 * p0 - 2.0 * p1 + t0 + t1,
            b: -3.0 * p0 + 3.0 * p1 - 2.0 * t0 - t1,
            c: t0,
            d: p0,
        }
    }

    fn position(&self, t: f32) -> Vec3 {
        ((self.a * t + self.b) * t + self.c) * t + self.d
    }

    fn tangent(&self, t: f32) -> Vec3 {
        (3.0 * self.a * t + 2.0 * self.b) * t + self.c
    }
}

/// A point travelling a closed Catmull-Rom loop through a path's
/// points at constant speed.
#[derive(Debug, Clone)]
pub struct PathFollower {
    points: Vec<Vec3>,
    speed: f32,
    /// Seconds into the current segment (the original's parameter:
    /// `t = speed · u / length`).
    u: f32,
    length: f32,
    index: usize,
    next: usize,
    segment: Segment,
}

impl PathFollower {
    /// A follower at the start of `points` (the original's reset: the
    /// first segment runs from point 0 to point 1). `None` for fewer
    /// than two points.
    pub fn new(points: Vec<Vec3>, speed: f32) -> Option<Self> {
        if points.len() < 2 {
            return None;
        }
        let mut f = Self {
            index: points.len() - 1,
            points,
            speed,
            u: 0.0,
            length: 1.0,
            next: 0,
            segment: Segment::default(),
        };
        if f.points.len() > 2 {
            f.step_forward();
        }
        Some(f)
    }

    /// The point count.
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Always `false` — a follower holds at least two points.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Whether this is a parked (two-point) object.
    pub fn is_parked(&self) -> bool {
        self.points.len() == 2
    }

    /// Index of the point the current segment starts at.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Index of the point the current segment ends at.
    pub fn next_index(&self) -> usize {
        self.next
    }

    /// The path points.
    pub fn points(&self) -> &[Vec3] {
        &self.points
    }

    /// Parameter within the current segment, `0..=1`.
    pub fn t(&self) -> f32 {
        if self.length > 0.0 {
            self.speed * self.u / self.length
        } else {
            0.0
        }
    }

    fn wrap(&self, i: usize) -> usize {
        if i >= self.points.len() { 0 } else { i }
    }

    /// Build the segment from `self.index` to `next` with Catmull-Rom
    /// tangents from `prev` and `next2`.
    fn build(&mut self, prev: usize, next: usize, next2: usize) {
        let p = &self.points;
        let p0 = p[self.index];
        let p1 = p[next];
        let t0 = (p1 - p[prev]) * 0.5;
        let t1 = (p[next2] - p0) * 0.5;
        self.segment = Segment::hermite(p0, p1, t0, t1);
        self.next = next;
        let mid = self.segment.position(0.5);
        self.length = p0.distance(mid) + mid.distance(p1);
    }

    fn step_forward(&mut self) {
        let prev = self.index;
        self.index = self.wrap(self.index + 1);
        let next = self.wrap(self.index + 1);
        let next2 = self.wrap(next + 1);
        self.build(prev, next, next2);
    }

    fn step_backward(&mut self) {
        let n = self.points.len();
        let old = self.index;
        self.index = if self.index == 0 {
            n - 1
        } else {
            self.index - 1
        };
        let prev = if self.index == 0 {
            n - 1
        } else {
            self.index - 1
        };
        let next2 = self.wrap(old + 1);
        self.build(prev, old, next2);
    }

    /// Advance by `dt` seconds (negative runs backwards) — at most one
    /// segment boundary per call, as the original.
    pub fn advance(&mut self, dt: f32) {
        if self.is_parked() || !dt.is_finite() || self.speed <= 0.0 {
            return;
        }
        self.u += dt;
        let mut t = self.t();
        if t > 1.0 {
            self.step_forward();
            t -= 1.0;
            self.u = t * self.length / self.speed;
        }
        if t < 0.0 {
            self.step_backward();
            t += 1.0;
            self.u = t * self.length / self.speed;
        }
    }

    /// Current position and (unnormalised) travel direction.
    pub fn pose(&self) -> (Vec3, Vec3) {
        if self.is_parked() {
            return (self.points[0], self.points[1] - self.points[0]);
        }
        let t = self.t();
        (self.segment.position(t), self.segment.tangent(t))
    }
}

/// The world rotation of an object travelling along `dir`: local +Z
/// on the direction, +X = up × Z, +Y = Z × X (the original's
/// orthonormalisation). Identity for a degenerate direction.
pub fn mover_rotation(dir: Vec3) -> Quat {
    let Some(z) = dir.try_normalize() else {
        return Quat::IDENTITY;
    };
    let Some(x) = Vec3::Y.cross(z).try_normalize() else {
        return Quat::IDENTITY;
    };
    let y = z.cross(x);
    Quat::from_mat3(&bevy::math::Mat3::from_cols(x, y, z))
}

/// A sailboat's speed from its path spacing and a uniform draw in
/// `0..1` (the original's `rand` float).
pub fn sailboat_speed(spacing: f32, draw: f32) -> f32 {
    let lo = spacing - SAILBOAT_SPEED_SPREAD;
    let hi = spacing + SAILBOAT_SPEED_SPREAD;
    draw * (hi - lo) + lo
}

/// Where a train is in its shuttle cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainPhase {
    /// Stopped at an end, counting down [`TRAIN_WAIT`].
    Waiting,
    /// Ramping up to speed.
    Accelerating,
    /// At speed, running to the far end.
    Running,
    /// Ramping down at the end of the line.
    Braking,
}

/// One train: [`TRAIN_CARS`] followers on the same path plus the
/// shuttle state.
#[derive(Debug, Clone)]
pub struct TrainMotion {
    /// The cars, front (at the path start) first.
    pub cars: Vec<PathFollower>,
    /// The shuttle phase.
    pub phase: TrainPhase,
    /// Seconds waited.
    pub timer: f32,
    /// Speed fraction, `0..=1`.
    pub scale: f32,
    /// Running towards the path's end (else back towards its start).
    pub forward: bool,
}

impl TrainMotion {
    /// A train at the start of `points`, cars spaced along it. `None`
    /// for fewer than two points.
    pub fn new(points: &[Vec3]) -> Option<Self> {
        let mut cars = Vec::with_capacity(TRAIN_CARS);
        for i in 0..TRAIN_CARS {
            let mut car = PathFollower::new(points.to_vec(), TRAIN_SPEED)?;
            car.advance(i as f32 * TRAIN_CAR_GAP);
            cars.push(car);
        }
        Some(Self {
            cars,
            phase: TrainPhase::Waiting,
            timer: 0.0,
            scale: 1.0,
            forward: true,
        })
    }

    /// Whether the train is moving (the sound's speed switch).
    pub fn moving(&self) -> bool {
        self.phase != TrainPhase::Waiting
    }

    fn at_end(&self) -> bool {
        if self.forward {
            let car = &self.cars[0];
            car.index() + 3 >= car.len()
        } else {
            self.cars[TRAIN_CARS - 1].index() <= 1
        }
    }

    fn advance_cars(&mut self, dt: f32) {
        for car in &mut self.cars {
            car.advance(dt);
        }
    }

    /// Advance by `dt` seconds.
    pub fn step(&mut self, dt: f32) {
        let signed = if self.forward { dt } else { -dt };
        match self.phase {
            TrainPhase::Waiting => {
                self.timer += dt;
                if self.timer > TRAIN_WAIT {
                    self.timer = 0.0;
                    self.phase = TrainPhase::Accelerating;
                }
            }
            TrainPhase::Running => {
                self.advance_cars(signed);
                if self.at_end() {
                    self.phase = TrainPhase::Braking;
                }
            }
            TrainPhase::Braking => {
                self.scale = (self.scale - TRAIN_RAMP * dt).max(0.0);
                if self.scale > 0.0 {
                    self.advance_cars(signed * self.scale);
                } else {
                    self.phase = TrainPhase::Waiting;
                    self.forward = !self.forward;
                }
            }
            TrainPhase::Accelerating => {
                self.scale = (self.scale + TRAIN_RAMP * dt).min(1.0);
                if self.scale < 1.0 {
                    self.advance_cars(signed * self.scale);
                } else {
                    self.phase = TrainPhase::Running;
                }
            }
        }
    }

    /// A car's pose: the curve position with its height replaced by
    /// the straight interpolation between the segment's end points
    /// (the original keeps cars level with the authored rails), and
    /// the curve tangent.
    pub fn car_pose(&self, car: usize) -> (Vec3, Vec3) {
        let f = &self.cars[car];
        let (mut pos, dir) = f.pose();
        let pts = f.points();
        let (y0, y1) = (pts[f.index()].y, pts[f.next_index()].y);
        pos.y = y0 + (y1 - y0) * f.t().clamp(0.0, 1.0);
        (pos, dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<Vec3> {
        vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(100.0, 0.0, 0.0),
            Vec3::new(100.0, 0.0, 100.0),
            Vec3::new(0.0, 0.0, 100.0),
        ]
    }

    #[test]
    fn follower_passes_through_the_points_and_loops() {
        let mut f = PathFollower::new(square(), 10.0).unwrap();
        assert!(f.pose().0.distance(Vec3::ZERO) < 1e-4);
        let mut seen = vec![false; 4];
        for _ in 0..20_000 {
            f.advance(0.01);
            let (p, _) = f.pose();
            for (i, q) in square().iter().enumerate() {
                if p.distance(*q) < 0.2 {
                    seen[i] = true;
                }
            }
        }
        assert!(seen.iter().all(|s| *s), "{seen:?}");
    }

    #[test]
    fn follower_speed_is_about_its_metres_per_second() {
        let mut f = PathFollower::new(square(), 10.0).unwrap();
        let mut travelled = 0.0;
        let mut last = f.pose().0;
        for _ in 0..500 {
            f.advance(0.01);
            let p = f.pose().0;
            travelled += p.distance(last);
            last = p;
        }
        // 5 s at 10 m/s, within the midpoint length estimate's error.
        assert!((travelled - 50.0).abs() < 5.0, "{travelled}");
    }

    #[test]
    fn follower_runs_backwards_too() {
        let mut f = PathFollower::new(square(), 10.0).unwrap();
        for _ in 0..300 {
            f.advance(0.01);
        }
        let ahead = f.pose().0;
        for _ in 0..300 {
            f.advance(-0.01);
        }
        assert!(f.pose().0.distance(Vec3::ZERO) < 0.5, "{ahead}");
    }

    #[test]
    fn two_point_paths_park() {
        let mut f =
            PathFollower::new(vec![Vec3::ZERO, Vec3::new(0.0, 0.0, -5.0)], FERRY_SPEED).unwrap();
        f.advance(100.0);
        let (p, d) = f.pose();
        assert_eq!(p, Vec3::ZERO);
        let fwd = mover_rotation(d) * Vec3::Z;
        assert!(fwd.distance(Vec3::NEG_Z) < 1e-5);
    }

    #[test]
    fn rotation_faces_local_z_along_travel_and_stays_upright() {
        let q = mover_rotation(Vec3::new(1.0, 0.2, 1.0));
        let fwd = q * Vec3::Z;
        assert!(fwd.dot(Vec3::new(1.0, 0.2, 1.0).normalize()) > 0.999);
        assert!((q * Vec3::X).y.abs() < 1e-5, "no roll");
    }

    #[test]
    fn sailboat_speeds_spread_one_metre_per_second() {
        assert_eq!(sailboat_speed(5.0, 0.0), 4.0);
        assert_eq!(sailboat_speed(5.0, 1.0), 6.0);
    }

    #[test]
    fn train_shuttles_end_to_end() {
        let line: Vec<Vec3> = (0..9)
            .map(|i| Vec3::new(i as f32 * 80.0, -13.0, 0.0))
            .collect();
        let mut train = TrainMotion::new(&line).unwrap();
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        let mut left_at = None;
        let mut reversed_at = None;
        let mut max_x: f32 = 0.0;
        while t < 120.0 {
            train.step(dt);
            t += dt;
            max_x = max_x.max(train.car_pose(0).0.x);
            if left_at.is_none() && train.moving() {
                left_at = Some(t);
            }
            if reversed_at.is_none() && !train.forward {
                reversed_at = Some(t);
            }
        }
        assert!((left_at.unwrap() - TRAIN_WAIT).abs() < 0.05);
        assert!(reversed_at.is_some(), "reached the end and turned round");
        assert!(max_x > 400.0, "{max_x}");
        // Cars keep their spacing.
        let gap = train.car_pose(0).0.distance(train.car_pose(1).0);
        assert!((gap - TRAIN_SPEED * TRAIN_CAR_GAP).abs() < 3.0, "{gap}");
    }

    #[test]
    fn train_cars_ride_level_with_the_rails() {
        let line = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(100.0, -10.0, 0.0),
            Vec3::new(200.0, -10.0, 0.0),
            Vec3::new(300.0, 0.0, 0.0),
        ];
        let mut train = TrainMotion::new(&line).unwrap();
        for _ in 0..(12 * 60) {
            train.step(1.0 / 60.0);
        }
        let (p, _) = train.car_pose(0);
        assert!(p.y <= 0.0 && p.y >= -10.0, "{p}");
    }
}
