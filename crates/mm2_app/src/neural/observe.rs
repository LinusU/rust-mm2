//! What a neural driver sees and how its outputs become pedals.
//!
//! One observation is [`OBS_DIM`] floats, all in the car's own heading
//! frame and scaled to roughly unit size:
//!
//! | Block | Size | Contents |
//! | --- | ---: | --- |
//! | car | [`CAR_DIM`] | what this car is: size, mass, power, grip, steering lock, throttle ceiling |
//! | motion | [`MOTION_DIM`] | velocity, spin, tilt, steering angle, wheel contact, last action |
//! | guide | [`GUIDE_DIM`] | offset and heading against the line, then the line ahead at [`LOOKAHEAD`] |
//! | rays | [`RAYS`]`.len()` | free distance to the world: the corridor ahead, then a fan round the car |
//! | floor | [`FLOOR`]`.len()` | where the ground ahead is, against where level ground would be |
//! | neighbours | [`NEIGHBOURS`]` × `[`NEIGHBOUR_DIM`] | the nearest other vehicles |
//!
//! The training environment and the game build observations through
//! this one function, so the policy drives in the game exactly what it
//! drove in training. Changing anything here changes the meaning of
//! every trained weight: bump [`OBS_VERSION`] and retrain.

use bevy::math::{Quat, Vec3};
use mm2_vehicle::{HandlingMetrics, VehicleConfig, VehicleInput};

use super::guide::Guide;

/// Layout version, recorded with trained weights by the trainer.
pub const OBS_VERSION: u32 = 3;

/// Distances (m) down the guide at which the line ahead is sampled.
pub const LOOKAHEAD: [f32; 10] = [5.0, 10.0, 16.0, 24.0, 34.0, 48.0, 66.0, 90.0, 125.0, 170.0];

/// World rays: angle off the nose (degrees, positive right), reach
/// (m) and where across the car the ray starts, in half-widths from its
/// centre line.
///
/// The first three run straight ahead from the left edge, the centre
/// and the right edge: the corridor the car is about to sweep. A fan of
/// rays from one point leaves gaps a parked car fits in — at 40 m, rays
/// 4° apart are 2.8 m apart — so something standing a metre off the
/// heading line went unseen until half a second before the bumper
/// reached it. The rest fan out from the centre, close together ahead
/// and wider to the sides and rear.
pub const RAYS: [(f32, f32, f32); 24] = [
    (0.0, 120.0, -1.0),
    (0.0, 120.0, 0.0),
    (0.0, 120.0, 1.0),
    (-3.0, 120.0, 0.0),
    (3.0, 120.0, 0.0),
    (-6.0, 110.0, 0.0),
    (6.0, 110.0, 0.0),
    (-10.0, 100.0, 0.0),
    (10.0, 100.0, 0.0),
    (-15.0, 90.0, 0.0),
    (15.0, 90.0, 0.0),
    (-22.0, 70.0, 0.0),
    (22.0, 70.0, 0.0),
    (-32.0, 55.0, 0.0),
    (32.0, 55.0, 0.0),
    (-45.0, 45.0, 0.0),
    (45.0, 45.0, 0.0),
    (-65.0, 35.0, 0.0),
    (65.0, 35.0, 0.0),
    (-90.0, 25.0, 0.0),
    (90.0, 25.0, 0.0),
    (-135.0, 15.0, 0.0),
    (135.0, 15.0, 0.0),
    (180.0, 15.0, 0.0),
];

/// Floor probes: angle off the nose (degrees, positive right) and the
/// distance ahead (m) at which the probe would meet level ground. Each
/// is cast from roof height down toward that point; a hit short of it
/// is rising ground or an obstacle, a long one a dip, none a drop — a
/// quay, a raised drawbridge, the far side of a crest.
pub const FLOOR: [(f32, f32); 6] = [
    (0.0, 6.0),
    (-25.0, 6.0),
    (25.0, 6.0),
    (0.0, 14.0),
    (-25.0, 14.0),
    (25.0, 14.0),
];

/// Other vehicles described per observation.
pub const NEIGHBOURS: usize = 4;
/// Floats per neighbour slot.
pub const NEIGHBOUR_DIM: usize = 9;
/// Floats describing the car itself.
pub const CAR_DIM: usize = 11;
/// Floats describing its motion.
pub const MOTION_DIM: usize = 15;
/// Floats describing the guide.
pub const GUIDE_DIM: usize = 4 + 3 * LOOKAHEAD.len();
/// Whole observation size.
pub const OBS_DIM: usize =
    CAR_DIM + MOTION_DIM + GUIDE_DIM + RAYS.len() + FLOOR.len() + NEIGHBOURS * NEIGHBOUR_DIM;
/// Action size: steering, pedal (throttle positive, brake negative),
/// handbrake (on when positive).
pub const ACT_DIM: usize = 3;

/// Height (m) above the ground under the car that world rays start at:
/// over kerbs and road crowns, under everything that stops a car.
const RAY_HEIGHT: f32 = 0.7;
/// A surface whose normal points at least this much upward is ground to
/// drive on, not a wall — the ray carries on along it.
const GROUND_NORMAL_Y: f32 = 0.7;
/// Times one ray may be carried along ground it met.
const RAY_BOUNCES: usize = 2;
/// Height (m) above the ground under the car that floor probes start
/// at — about the roof.
const FLOOR_HEIGHT: f32 = 1.5;
/// A floor probe reaches this many times the distance to level ground
/// before the ground counts as gone.
const FLOOR_REACH: f32 = 2.0;
/// Neighbours further than this (m) are not described.
const NEIGHBOUR_RANGE: f32 = 90.0;
/// Neighbours more than this far above or below (m) are on another
/// level and not described.
const NEIGHBOUR_LEVEL: f32 = 8.0;
/// Bound on every observation value.
const CLIP: f32 = 5.0;

/// The fixed part of an observation: what kind of car this is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CarProfile {
    /// Everything but the throttle ceiling, which is the driver's.
    features: [f32; CAR_DIM - 1],
    /// Half the body's width (m).
    pub half_width: f32,
    /// Half the body's length (m).
    pub half_length: f32,
    /// Steering lock at a standstill (rad).
    lock: f32,
    /// Body-space height of the world rays' origin.
    ray_y: f32,
}

impl CarProfile {
    /// Distil `config` into the car block of the observation.
    pub fn of(config: &VehicleConfig) -> Self {
        let metrics = HandlingMetrics::of(config);
        let half_width = config.chassis_size[0] * 0.5;
        let half_length = config.chassis_size[2] * 0.5;
        let mass = config.mass.max(1.0);
        let power = config
            .original
            .as_ref()
            .map(|o| o.engine.max_power_w)
            .or(config.engine.max_power_w)
            .unwrap_or(0.0);
        let lock = config.steering.low_speed_max_angle.max(1e-3);
        let finite = |v: f32| if v.is_finite() { v } else { 0.0 };
        let features = [
            config.wheelbase / 4.0,
            half_width / 1.5,
            half_length / 3.0,
            (mass / 1500.0).ln(),
            power / mass / 150.0,
            config.top_speed_mps.unwrap_or(50.0) / 60.0,
            lock / 0.6,
            config.steering.high_speed_max_angle / 0.6,
            finite(metrics.peak_lateral_g) / 2.0,
            finite(metrics.com_height),
        ]
        .map(|v| finite(v).clamp(-CLIP, CLIP));
        Self {
            features,
            half_width,
            half_length,
            lock,
            ray_y: finite(metrics.ground_y) + RAY_HEIGHT,
        }
    }
}

/// A car's motion this instant, as the physics reports it.
#[derive(Debug, Clone, Copy)]
pub struct Motion {
    pub position: Vec3,
    pub rotation: Quat,
    pub linear_velocity: Vec3,
    pub angular_velocity: Vec3,
    /// Current steering angle at the wheels (rad).
    pub steer_angle: f32,
    /// Share of the wheels touching the ground, `0..=1`.
    pub grounded: f32,
    /// Whether the drivetrain is in reverse.
    pub reversing: bool,
}

impl Motion {
    /// Unit nose direction flattened onto the ground plane, or `None`
    /// when the car points straight up or down.
    fn heading(&self) -> Option<Vec3> {
        let f = self.rotation * Vec3::NEG_Z;
        Vec3::new(f.x, 0.0, f.z).try_normalize()
    }
}

/// Another vehicle near the observer.
#[derive(Debug, Clone, Copy)]
pub struct Neighbour {
    pub position: Vec3,
    pub velocity: Vec3,
    /// Its nose direction.
    pub forward: Vec3,
    pub half_width: f32,
    pub half_length: f32,
}

/// A world ray's first hit: distance along the ray and the surface
/// normal there.
pub type RayHit = (f32, Vec3);

/// Free distance along one ray, carried along any ground it meets so a
/// rising road is not read as a wall.
fn free_distance(
    mut origin: Vec3,
    mut dir: Vec3,
    reach: f32,
    cast: &mut impl FnMut(Vec3, Vec3, f32) -> Option<RayHit>,
) -> f32 {
    let mut travelled = 0.0;
    for _ in 0..=RAY_BOUNCES {
        let left = reach - travelled;
        if left <= 0.0 || !origin.is_finite() {
            break;
        }
        let Some((dist, normal)) = cast(origin, dir, left) else {
            return reach;
        };
        travelled += dist;
        if normal.y < GROUND_NORMAL_Y {
            return travelled.min(reach);
        }
        // Ground: lift off it and carry on along the slope.
        let Some(along) = (dir - normal * dir.dot(normal)).try_normalize() else {
            return travelled.min(reach);
        };
        origin += dir * dist + normal * RAY_HEIGHT;
        dir = along;
    }
    travelled.min(reach)
}

/// Fill `out` ([`OBS_DIM`] floats) with what the car at `motion` sees.
///
/// `s` is its arc position on `guide`, `last` the action it took last
/// decision, `neighbours` every other vehicle worth knowing about (any
/// number, any order — the nearest are picked), `throttle_cap` the
/// most throttle its pedal will be given, and `cast` a ray query
/// against the world that ignores vehicles and treats deadly water as
/// open space.
#[allow(clippy::too_many_arguments)] // one observation's full set of sources
pub fn observe(
    out: &mut [f32],
    car: &CarProfile,
    motion: &Motion,
    guide: &Guide,
    s: f32,
    last: &[f32; ACT_DIM],
    neighbours: &[Neighbour],
    throttle_cap: f32,
    mut cast: impl FnMut(Vec3, Vec3, f32) -> Option<RayHit>,
) {
    assert_eq!(out.len(), OBS_DIM);
    let fwd = motion.heading().unwrap_or(Vec3::NEG_Z);
    let right = Vec3::new(-fwd.z, 0.0, fwd.x);
    // World vector → heading frame (x right, y up, z forward).
    let local = |v: Vec3| Vec3::new(v.dot(right), v.y, v.dot(fwd));
    let mut w = Writer { out, at: 0 };

    w.extend(&car.features);
    w.push(throttle_cap);

    let v = local(motion.linear_velocity);
    let inv = motion.rotation.inverse();
    let spin = inv * motion.angular_velocity;
    let down = inv * Vec3::NEG_Y;
    w.extend(&[
        v.z / 40.0,
        v.x / 15.0,
        v.y / 10.0,
        // Yaw rate, positive turning right like the steering.
        -spin.y / 2.0,
        spin.x / 2.0,
        spin.z / 2.0,
        down.x,
        down.y,
        down.z,
        motion.steer_angle / car.lock,
        motion.grounded,
        if motion.reversing { 1.0 } else { 0.0 },
    ]);
    w.extend(last);

    let here = guide.point_at(s);
    let tangent = local(guide.tangent_at(s));
    let offset = local(motion.position - here);
    let tangent_len = tangent.x.hypot(tangent.z).max(1e-6);
    w.extend(&[
        offset.x / 5.0,
        offset.y / 3.0,
        tangent.x / tangent_len,
        tangent.z / tangent_len,
    ]);
    for d in LOOKAHEAD {
        let p = local(point_ahead(guide, s, d) - motion.position);
        w.extend(&[p.x / d, p.z / d, p.y / (0.3 * d)]);
    }

    let origin = motion.position + motion.rotation * Vec3::new(0.0, car.ray_y, 0.0);
    let nose = motion.rotation * Vec3::NEG_Z;
    let side = motion.rotation * Vec3::X;
    for (angle, reach, across) in RAYS {
        let (sin, cos) = angle.to_radians().sin_cos();
        let dir = (nose * cos + side * sin).normalize_or_zero();
        let from = origin + side * (across * car.half_width);
        let free = if dir == Vec3::ZERO {
            reach
        } else {
            free_distance(from, dir, reach, &mut cast)
        };
        w.push(free / reach);
    }

    let roof = motion.position
        + motion.rotation * Vec3::new(0.0, car.ray_y - RAY_HEIGHT + FLOOR_HEIGHT, 0.0);
    let below = motion.rotation * Vec3::NEG_Y;
    for (angle, ahead) in FLOOR {
        let (sin, cos) = angle.to_radians().sin_cos();
        let Some(dir) = ((nose * cos + side * sin) * ahead + below * FLOOR_HEIGHT).try_normalize()
        else {
            w.push(1.0);
            continue;
        };
        let level = ahead.hypot(FLOOR_HEIGHT);
        let hit = cast(roof, dir, level * FLOOR_REACH).map_or(FLOOR_REACH, |(d, _)| d / level);
        w.push(hit);
    }

    let mut near: Vec<(f32, &Neighbour)> = neighbours
        .iter()
        .filter_map(|n| {
            let rel = n.position - motion.position;
            if rel.y.abs() > NEIGHBOUR_LEVEL {
                return None;
            }
            let dist = rel.x.hypot(rel.z);
            // A car behind matters less than one the same distance ahead.
            let score = if rel.dot(fwd) >= 0.0 {
                dist
            } else {
                dist * 1.5
            };
            (dist <= NEIGHBOUR_RANGE && dist.is_finite()).then_some((score, n))
        })
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    for slot in 0..NEIGHBOURS {
        match near.get(slot) {
            Some((_, n)) => {
                let p = local(n.position - motion.position);
                let rv = local(n.velocity - motion.linear_velocity);
                let nf = local(n.forward);
                let len = nf.x.hypot(nf.z).max(1e-6);
                w.extend(&[
                    1.0,
                    p.x / 20.0,
                    p.z / 40.0,
                    rv.x / 20.0,
                    rv.z / 20.0,
                    nf.x / len,
                    nf.z / len,
                    n.half_width / 1.5,
                    n.half_length / 3.0,
                ]);
            }
            None => w.extend(&[0.0; NEIGHBOUR_DIM]),
        }
    }
    debug_assert_eq!(w.at, OBS_DIM);
}

/// The guide point `d` metres past `s`. An open guide is continued
/// straight on past its last point, so its end does not look like a
/// wall of converging samples — what happens at the end of a route is
/// race logic, not driving.
fn point_ahead(guide: &Guide, s: f32, d: f32) -> Vec3 {
    match guide.remaining(s) {
        Some(left) if d > left => {
            let end = guide.length();
            guide.point_at(end) + guide.tangent_at(end) * (d - left)
        }
        _ => guide.point_at(s + d),
    }
}

struct Writer<'a> {
    out: &'a mut [f32],
    at: usize,
}

impl Writer<'_> {
    fn push(&mut self, v: f32) {
        self.out[self.at] = if v.is_finite() {
            v.clamp(-CLIP, CLIP)
        } else {
            0.0
        };
        self.at += 1;
    }

    fn extend(&mut self, vs: &[f32]) {
        for &v in vs {
            self.push(v);
        }
    }
}

/// Signed angle (rad, positive right) from the car's nose to its guide
/// [`BEARING_AHEAD`] metres on — what a backing-up car steers by.
pub fn guide_bearing(motion: &Motion, guide: &Guide, s: f32) -> f32 {
    let Some(fwd) = motion.heading() else {
        return 0.0;
    };
    let right = Vec3::new(-fwd.z, 0.0, fwd.x);
    let aim = point_ahead(guide, s, BEARING_AHEAD) - motion.position;
    aim.dot(right).atan2(aim.dot(fwd))
}

/// How far down the guide (m) [`guide_bearing`] aims.
const BEARING_AHEAD: f32 = 15.0;

/// Turn a policy's output into pedals: steering as given, the pedal
/// axis split into throttle and brake, the handbrake on when its output
/// is positive. `throttle_cap` is the driver's authored `maxThrottle`.
pub fn action_input(action: &[f32; ACT_DIM], throttle_cap: f32) -> VehicleInput {
    let clean = |v: f32| {
        if v.is_finite() {
            v.clamp(-1.0, 1.0)
        } else {
            0.0
        }
    };
    let pedal = clean(action[1]);
    VehicleInput {
        steering: clean(action[0]),
        throttle: pedal.max(0.0).min(throttle_cap.clamp(0.0, 1.0)),
        brake: (-pedal).max(0.0),
        handbrake: if action[2] > 0.0 { 1.0 } else { 0.0 },
        forced_gear: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn motion() -> Motion {
        Motion {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            linear_velocity: Vec3::new(0.0, 0.0, -20.0),
            angular_velocity: Vec3::ZERO,
            steer_angle: 0.0,
            grounded: 1.0,
            reversing: false,
        }
    }

    fn straight() -> Guide {
        Guide::new([Vec3::ZERO, Vec3::new(0.0, 0.0, -1000.0)], false).unwrap()
    }

    const GUIDE_AT: usize = CAR_DIM + MOTION_DIM;
    const RAYS_AT: usize = GUIDE_AT + GUIDE_DIM;
    const FLOOR_AT: usize = RAYS_AT + RAYS.len();
    const NEIGHBOURS_AT: usize = FLOOR_AT + FLOOR.len();

    fn see(
        m: &Motion,
        g: &Guide,
        s: f32,
        n: &[Neighbour],
        cast: impl FnMut(Vec3, Vec3, f32) -> Option<RayHit>,
    ) -> Vec<f32> {
        let mut out = vec![0.0; OBS_DIM];
        let car = CarProfile::of(&VehicleConfig::default());
        observe(&mut out, &car, m, g, s, &[0.0; ACT_DIM], n, 1.0, cast);
        out
    }

    #[test]
    fn a_car_on_a_straight_line_sees_it_dead_ahead() {
        let o = see(&motion(), &straight(), 0.0, &[], |_, _, _| None);
        assert!((o[CAR_DIM] - 0.5).abs() < 1e-6, "forward speed 20/40");
        // Offset zero, tangent straight ahead.
        assert_eq!(&o[GUIDE_AT..GUIDE_AT + 4], &[0.0, 0.0, 0.0, 1.0]);
        for k in 0..LOOKAHEAD.len() {
            let p = &o[GUIDE_AT + 4 + 3 * k..][..3];
            assert!(p[0].abs() < 1e-6 && (p[1] - 1.0).abs() < 1e-6, "{p:?}");
        }
        assert!(o[RAYS_AT..FLOOR_AT].iter().all(|&r| r == 1.0));
        assert!(
            o[FLOOR_AT..NEIGHBOURS_AT].iter().all(|&f| f == FLOOR_REACH),
            "nothing under the car: every probe finds a drop"
        );
        assert!(o[NEIGHBOURS_AT..].iter().all(|&v| v == 0.0));
        assert_eq!(o[CAR_DIM - 1], 1.0, "the throttle ceiling");
    }

    #[test]
    fn a_line_to_the_right_reads_positive_like_right_steering() {
        // The car faces −Z; +X is its right.
        let g = Guide::new(
            [Vec3::new(3.0, 0.0, 0.0), Vec3::new(3.0, 0.0, -1000.0)],
            false,
        )
        .unwrap();
        let o = see(&motion(), &g, 0.0, &[], |_, _, _| None);
        assert!(o[GUIDE_AT] < 0.0, "the car sits left of the line");
        assert!(o[GUIDE_AT + 4] > 0.0, "the line ahead is to the right");
    }

    #[test]
    fn an_open_guide_runs_straight_on_past_its_end() {
        let g = Guide::new([Vec3::ZERO, Vec3::new(0.0, 0.0, -20.0)], false).unwrap();
        let o = see(&motion(), &g, 10.0, &[], |_, _, _| None);
        let last = &o[GUIDE_AT + 4 + 3 * (LOOKAHEAD.len() - 1)..][..3];
        // 170 m ahead of arc 10 is 180 m down −Z; the car is at 0.
        assert!((last[1] - 180.0 / 170.0).abs() < 1e-4, "{last:?}");
    }

    #[test]
    fn a_wall_shortens_a_ray_and_a_slope_does_not() {
        let wall = see(&motion(), &straight(), 0.0, &[], |_, dir, _| {
            (dir.z < -0.99).then_some((30.0, Vec3::Z))
        });
        for corridor in 0..3 {
            assert!((wall[RAYS_AT + corridor] - 30.0 / 120.0).abs() < 1e-6);
        }
        assert_eq!(wall[RAYS_AT + 19], 1.0, "the side ray is clear");
        let mut casts = 0;
        let slope = see(&motion(), &straight(), 0.0, &[], |_, dir, _| {
            casts += 1;
            // The first cast of the first ray meets rising road.
            (casts == 1 && dir.z < -0.99).then_some((30.0, Vec3::new(0.0, 0.95, 0.31)))
        });
        assert_eq!(slope[RAYS_AT], 1.0, "carried on up the slope");
    }

    #[test]
    fn the_corridor_sees_what_a_fan_from_the_centre_misses() {
        // A post 1 m right of the heading line, 40 m ahead: only a ray
        // that starts right of the centre and runs straight ahead can
        // meet it.
        let o = see(&motion(), &straight(), 0.0, &[], |origin, dir, _| {
            (dir.z < -0.9999 && origin.x > 0.5).then_some((40.0, Vec3::Z))
        });
        assert_eq!(o[RAYS_AT], 1.0, "left edge clear");
        assert_eq!(o[RAYS_AT + 1], 1.0, "centre clear");
        assert!((o[RAYS_AT + 2] - 40.0 / 120.0).abs() < 1e-6, "right edge");
        assert!(o[RAYS_AT + 3..FLOOR_AT].iter().all(|&r| r == 1.0));
    }

    #[test]
    fn floor_probes_read_level_ground_as_one() {
        // A flat floor `FLOOR_HEIGHT` under the probes' origin.
        let floor_y =
            motion().position.y + CarProfile::of(&VehicleConfig::default()).ray_y - RAY_HEIGHT;
        let o = see(&motion(), &straight(), 0.0, &[], |origin, dir, reach| {
            if dir.y >= -1e-3 {
                return None;
            }
            let d = (floor_y - origin.y) / dir.y;
            (d <= reach).then_some((d, Vec3::Y))
        });
        for (k, f) in o[FLOOR_AT..NEIGHBOURS_AT].iter().enumerate() {
            assert!((f - 1.0).abs() < 1e-3, "probe {k}: {f}");
        }
    }

    #[test]
    fn the_nearest_neighbours_fill_the_slots_in_order() {
        let car = |x: f32, z: f32| Neighbour {
            position: Vec3::new(x, 0.0, z),
            velocity: Vec3::new(0.0, 0.0, -10.0),
            forward: Vec3::NEG_Z,
            half_width: 0.9,
            half_length: 2.1,
        };
        let others = [
            car(0.0, -60.0),
            car(2.0, -12.0),
            car(0.0, 300.0),
            Neighbour {
                position: Vec3::new(0.0, 20.0, -5.0),
                ..car(0.0, 0.0)
            },
        ];
        let o = see(&motion(), &straight(), 0.0, &others, |_, _, _| None);
        let slot = |i: usize| &o[NEIGHBOURS_AT + i * NEIGHBOUR_DIM..][..NEIGHBOUR_DIM];
        assert_eq!(slot(0)[0], 1.0);
        assert!((slot(0)[1] - 0.1).abs() < 1e-6, "2 m right");
        assert!((slot(0)[2] - 0.3).abs() < 1e-6, "12 m ahead");
        assert!((slot(0)[4] + 0.5).abs() < 1e-6, "closing at 10 m/s");
        assert!((slot(1)[2] - 1.5).abs() < 1e-6, "60 m ahead");
        assert_eq!(slot(2)[0], 0.0, "out of range and off-level are dropped");
    }

    #[test]
    fn actions_split_the_pedal_axis() {
        let go = action_input(&[0.3, 0.8, -1.0], 0.5);
        assert_eq!((go.steering, go.throttle, go.brake), (0.3, 0.5, 0.0));
        assert_eq!(go.handbrake, 0.0);
        let stop = action_input(&[-2.0, -0.4, 0.2], 1.0);
        assert_eq!((stop.steering, stop.throttle, stop.brake), (-1.0, 0.0, 0.4));
        assert_eq!(stop.handbrake, 1.0);
        assert_eq!(action_input(&[f32::NAN; 3], 1.0).steering, 0.0);
    }
}
