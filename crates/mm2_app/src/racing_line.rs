//! Opponent racing line (F15): where along its route an opponent is,
//! where it should aim, and how fast it may be going to make the
//! corners ahead.
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
//! - [`plan_speed`] scans the line out to braking distance, estimates
//!   each stretch's radius from how far its heading turns, and returns
//!   the speed from which the car can still brake to every corner's
//!   grip-limited speed; [`pace`] turns that into throttle and brake.
//!
//! The car's limits come from its own handling ([`CarLimits::of`]), so
//! a bus and a GTR plan different corner speeds on the same street.
//! All of this is designed policy (DSN-66): the original opponent
//! controller is unrecovered (UNK-11).

use bevy::math::Vec3;
use mm2_game::OpponentRoute;
use mm2_vehicle::sim::steering_response;
use mm2_vehicle::{HandlingMetrics, VehicleConfig};

use crate::opponents::route_is_closed;

/// Standard gravity (m/s²).
const G: f32 = 9.81;

/// Share of the analytic tire limit the planner corners at. The
/// handling probe measures cars cornering at about 0.8 of the tires'
/// analytic peak (the Beetle: 1.32 g on 1.65 g tires), and the planner
/// wants a little in hand on top of that for bumps, camber and traffic.
const CORNER_GRIP_SHARE: f32 = 0.7;
/// Share of the car's straight-line braking limit the planner assumes
/// — braking while still turning in shares the friction ellipse.
const BRAKE_GRIP_SHARE: f32 = 0.6;
/// `cornerSpeedMultiplier` value that leaves the planned corner grip
/// unchanged — `RegisterRoute`'s default (R4). Retail authors
/// 0.89–2.29, amateur rows the low end.
const CORNER_MULT_DEFAULT: f32 = 2.0;
/// Bounds on the authored multiplier's effect on corner grip, so an
/// extreme row stays a timid or a bold driver rather than a stopped or
/// a crashing one.
const CORNER_MULT_RANGE: (f32, f32) = (0.5, 1.2);
/// `cornerBrakingThreshold` when the row authors none — the
/// `RegisterRoute` default (R4).
pub const CORNER_BRAKE_DEFAULT: f32 = 0.7;
/// Ceiling on the authored threshold: a driver that waits for a full
/// stop's worth of demand before touching the brake has no margin left
/// for a misjudged corner.
const CORNER_BRAKE_MAX: f32 = 0.9;

/// Spacing (m) of the samples the speed scan takes along the line.
const SAMPLE_STEP: f32 = 4.0;
/// Samples either side of a point whose heading change sets its
/// curvature — a ±12 m window, wide enough that densified lane points'
/// small offsets average out and a sharp authored corner (one vertex)
/// still reads as the tight radius it is.
const CURVE_HALF_WINDOW: usize = 3;
/// Slack (m) the scan looks past the car's braking distance.
const HORIZON_SLACK: f32 = 40.0;
/// Furthest the speed scan looks (m) — beyond any car's braking
/// distance from its top speed.
const MAX_HORIZON: f32 = 320.0;
/// Speed (m/s) the planner never asks a car to drop below for a
/// corner — a hairpin is taken slowly, never at a standstill.
const MIN_CORNER_SPEED: f32 = 7.0;
/// Overspeed (m/s) the pace law tolerates before acting — inside it the
/// car holds its pace instead of hunting between throttle and brake.
const PACE_DEADBAND: f32 = 0.5;

/// What a car can do, distilled from its handling for the planner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CarLimits {
    /// Lateral acceleration (m/s²) the planner corners at.
    pub corner_accel: f32,
    /// Deceleration (m/s²) the planner brakes at.
    pub brake_accel: f32,
    /// The car's full straight-line deceleration (m/s²) — the
    /// denominator of a corner's brake demand.
    pub max_brake_accel: f32,
    /// Axle-to-axle distance (m) — the steering geometry's lever.
    pub wheelbase: f32,
    /// Mean lateral grip coefficient of the steered tires.
    pub front_grip: f32,
    /// Slip angle (rad) at which the steered tires make their peak
    /// grip — how far past the path the wheels must turn to pull.
    pub front_peak_slip: f32,
    /// Steering lock (rad) at a standstill and at `lock_speed` and
    /// above, interpolated between — the vehicle's own
    /// `max_steer_angle` — so a wheel angle can be turned back into the
    /// normalized input that commands it.
    pub lock_low: f32,
    /// Steering lock (rad) from `lock_speed` up.
    pub lock_high: f32,
    /// Speed (m/s) at which `lock_high` fully applies.
    pub lock_speed: f32,
    /// The input shaping exponent (`steering_response`).
    pub response_curve: f32,
}

impl CarLimits {
    /// Limits for `config`, with the corner grip scaled by the authored
    /// `cornerSpeedMultiplier` (`None` = the `RegisterRoute` default).
    /// The multiplier is read as a scale on the lateral acceleration
    /// the driver accepts — a designed reading of the documented name
    /// (RACE-14): amateur rows (≈0.9) corner noticeably slower than
    /// professional ones (≈2.2).
    pub fn of(config: &VehicleConfig, corner_mult: Option<f32>) -> Self {
        let metrics = HandlingMetrics::of(config);
        // A car that tips before it slides must corner below the tip
        // threshold, not its tires' limit.
        let lateral_g = metrics
            .peak_lateral_g
            .min(metrics.assisted_tip_threshold_g)
            .clamp(0.4, 2.5);
        let mult = (corner_mult.unwrap_or(CORNER_MULT_DEFAULT) / CORNER_MULT_DEFAULT)
            .clamp(CORNER_MULT_RANGE.0, CORNER_MULT_RANGE.1);
        let mass = config.mass.max(1.0);
        let brake_force = config.brakes.max_brake_force * config.wheels.len().max(1) as f32;
        let long_g = config.tires.longitudinal_grip.clamp(0.4, 2.5);
        let max_brake = ((brake_force / (mass * G)).min(long_g) * G).max(3.0);
        let steered: Vec<_> = config
            .wheels
            .iter()
            .filter(|w| w.steered)
            .map(|w| w.tires.as_ref().unwrap_or(&config.tires))
            .collect();
        let mean = |f: fn(&mm2_vehicle::config::TireConfig) -> f32, fallback: f32| {
            if steered.is_empty() {
                fallback
            } else {
                steered.iter().map(|t| f(t)).sum::<f32>() / steered.len() as f32
            }
        };
        Self {
            corner_accel: lateral_g * CORNER_GRIP_SHARE * mult * G,
            brake_accel: max_brake * BRAKE_GRIP_SHARE,
            max_brake_accel: max_brake,
            wheelbase: config.wheelbase.max(0.5),
            front_grip: mean(|t| t.lateral_grip, config.tires.lateral_grip).max(0.1),
            front_peak_slip: mean(|t| t.peak_slip_angle, config.tires.peak_slip_angle).max(0.0),
            lock_low: config.steering.low_speed_max_angle,
            lock_high: config.steering.high_speed_max_angle,
            lock_speed: config.steering.high_speed.max(1.0),
            response_curve: config.steering.response_curve.max(0.1),
        }
    }

    /// Distance (m) to brake from `v` to a standstill at the planned
    /// deceleration.
    pub fn braking_distance(&self, v: f32) -> f32 {
        v * v / (2.0 * self.brake_accel)
    }
}

/// What the line ahead asks of the car's speed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeedPlan {
    /// Speed (m/s) the car may carry now and still brake, at the
    /// planned deceleration, to every corner in range —
    /// `f32::INFINITY` when nothing ahead binds.
    pub limit: f32,
    /// Share of the car's full braking the most demanding corner ahead
    /// needs from the current speed (0 when none needs any) — what the
    /// authored `cornerBrakingThreshold` is compared against.
    pub demand: f32,
}

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
    pub fn locate(route: &OpponentRoute, next: usize, pos: Vec3) -> Option<Self> {
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
    pub fn point_ahead(&self, route: &OpponentRoute, dist: f32) -> Vec3 {
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

// Discrete junction policy for explicit evidence guides. These are pursuit
// geometry margins, not road width, authored speed or vehicle-physics edits.
const SHARP_TURN_MIN: f32 = std::f32::consts::FRAC_PI_3;
const CORNER_AIM_ROLLOUT: f32 = 3.0;
const CORNER_LINE_CUT: f32 = 2.0;

fn junction_turn(route: &OpponentRoute, leg: usize, legs: usize) -> f32 {
    let (a, b) = leg_ends(route, leg);
    let (_, c) = leg_ends(route, (leg + 1) % legs);
    wrap_angle((c.z - b.z).atan2(c.x - b.x) - (b.z - a.z).atan2(b.x - a.x)).abs()
}

/// Keep the pursuit chord near a sharp vertex until the car has traversed its
/// outgoing tangent. Advancing a dense cursor does not mean the body has yet
/// completed its turn. Straight lines and smooth bends keep ordinary lookahead.
pub fn corner_aim_distance(route: &OpponentRoute, cursor: RouteCursor, wanted: f32) -> f32 {
    let closed = route_is_closed(route);
    let legs = leg_count(route.points.len(), closed);
    let mut behind = cursor.along;
    for offset in 0..legs {
        if !closed && offset >= cursor.leg {
            break;
        }
        let incoming = (cursor.leg + legs - 1 - offset) % legs;
        if junction_turn(route, incoming, legs) >= SHARP_TURN_MIN && behind < wanted {
            return wanted.min(CORNER_AIM_ROLLOUT);
        }
        let (a, b) = leg_ends(route, incoming);
        behind += xz_len(a, b);
        if behind >= wanted {
            break;
        }
    }
    let mut distance = -cursor.along;
    for offset in 0..legs {
        let leg = (cursor.leg + offset) % legs;
        let (a, b) = leg_ends(route, leg);
        distance += xz_len(a, b);
        if distance > wanted || (!closed && leg + 1 >= legs) {
            break;
        }
        if junction_turn(route, leg, legs) >= SHARP_TURN_MIN {
            return wanted.min(distance.max(0.0) + CORNER_AIM_ROLLOUT);
        }
    }
    wanted
}

/// Radius, speed and tangent length of a rounded discrete junction. A circle's
/// closest-line inset is radius * (1 - cos(turn/2)); cap that inset while
/// respecting the car's mechanical steering radius. The steering-speed cap
/// inverts this car's existing speed-dependent lock, leaving 2% for corrections.
fn junction_geometry(turn: f32, limits: &CarLimits) -> (f32, f32) {
    let radius = (CORNER_LINE_CUT / (1.0 - (turn * 0.5).cos()))
        .max(limits.wheelbase / (limits.lock_low * 0.98).tan().max(0.1));
    let angle = (limits.wheelbase / radius).atan();
    let steering_speed = if limits.lock_low > limits.lock_high && angle > limits.lock_high {
        limits.lock_speed
            * ((limits.lock_low - angle) / (limits.lock_low - limits.lock_high)).clamp(0.0, 1.0)
    } else {
        f32::INFINITY
    };
    let speed = (limits.corner_accel * radius)
        .sqrt()
        .min(steering_speed)
        .max(1.0);
    (speed, (radius * (turn * 0.5).tan()).min(HORIZON_SLACK))
}

/// Brake to the entry tangent of a sharp junction, then retain its handling-
/// limited speed through the outgoing tangent. The broad smooth-bend window
/// cannot represent a discrete street corner's entry or steering-lock demand.
pub fn sharp_corner_plan(
    route: &OpponentRoute,
    cursor: RouteCursor,
    speed: f32,
    limits: &CarLimits,
) -> SpeedPlan {
    let closed = route_is_closed(route);
    let legs = leg_count(route.points.len(), closed);
    let mut plan = SpeedPlan {
        limit: f32::INFINITY,
        demand: 0.0,
    };
    let mut behind = cursor.along;
    for offset in 0..legs {
        if !closed && offset >= cursor.leg {
            break;
        }
        let incoming = (cursor.leg + legs - 1 - offset) % legs;
        let turn = junction_turn(route, incoming, legs);
        if turn >= SHARP_TURN_MIN {
            let (corner, tangent) = junction_geometry(turn, limits);
            if behind < tangent + limits.wheelbase {
                plan.limit = plan.limit.min(corner);
                if speed > corner {
                    plan.demand = plan.demand.max(1.0);
                }
            }
        }
        let (a, b) = leg_ends(route, incoming);
        behind += xz_len(a, b);
        if behind > HORIZON_SLACK {
            break;
        }
    }
    let mut distance = -cursor.along;
    for offset in 0..legs {
        let leg = (cursor.leg + offset) % legs;
        let (a, b) = leg_ends(route, leg);
        distance += xz_len(a, b);
        if distance > limits.braking_distance(speed) + HORIZON_SLACK || (!closed && leg + 1 >= legs)
        {
            break;
        }
        let turn = junction_turn(route, leg, legs);
        if turn < SHARP_TURN_MIN {
            continue;
        }
        let (corner, tangent) = junction_geometry(turn, limits);
        let entry = (distance - tangent).max(0.0);
        // Reserve the normal pace actuator share in the entry-distance budget;
        // once over the envelope, brake rather than waiting to reach the vertex.
        let reachable =
            (corner * corner + 2.0 * limits.brake_accel * CORNER_BRAKE_DEFAULT * entry).sqrt();
        plan.limit = plan.limit.min(reachable);
        if speed > reachable {
            plan.demand = plan.demand.max(1.0);
        }
        if speed > corner {
            plan.demand = plan.demand.max(
                (speed * speed - corner * corner) / (2.0 * entry.max(1.0) * limits.max_brake_accel),
            );
        }
    }
    plan
}

/// Scan the line ahead of `cursor` out to braking distance from
/// `speed` and work out what its corners ask: the highest speed the car
/// may carry now and still brake to each corner's grip-limited speed,
/// and how hard it would have to brake from `speed` to make the most
/// demanding one.
///
/// Curvature comes from the heading change across a sliding window of
/// samples, so a sharp corner authored as one vertex and a smooth bend
/// densified into many lane points both read as the radius actually
/// driven.
pub fn plan_speed(
    route: &OpponentRoute,
    cursor: RouteCursor,
    speed: f32,
    limits: &CarLimits,
) -> SpeedPlan {
    let speed = speed.max(0.0);
    let horizon = (limits.braking_distance(speed) + HORIZON_SLACK).min(MAX_HORIZON);
    let count = (horizon / SAMPLE_STEP).ceil() as usize + 2 * CURVE_HALF_WINDOW + 1;
    let samples: Vec<Vec3> = (0..count)
        .map(|k| cursor.point_ahead(route, k as f32 * SAMPLE_STEP))
        .collect();
    // Heading of each sample-to-sample step; a zero-length step (the
    // end of an open route) carries the previous heading.
    let mut headings: Vec<f32> = Vec::with_capacity(count.saturating_sub(1));
    for w in samples.windows(2) {
        let (dx, dz) = (w[1].x - w[0].x, w[1].z - w[0].z);
        let h = if dx.hypot(dz) > 1e-3 {
            dz.atan2(dx)
        } else {
            headings.last().copied().unwrap_or(0.0)
        };
        headings.push(h);
    }
    let span = 2.0 * CURVE_HALF_WINDOW as f32 * SAMPLE_STEP;
    let mut plan = SpeedPlan {
        limit: f32::INFINITY,
        demand: 0.0,
    };
    for k in CURVE_HALF_WINDOW..headings.len().saturating_sub(CURVE_HALF_WINDOW) {
        let turn = wrap_angle(headings[k + CURVE_HALF_WINDOW] - headings[k - CURVE_HALF_WINDOW]);
        if turn.abs() < 1e-3 {
            continue;
        }
        let radius = span / turn.abs();
        let corner = (limits.corner_accel * radius).sqrt().max(MIN_CORNER_SPEED);
        // Brake to the corner by the start of its window.
        let dist = (k - CURVE_HALF_WINDOW) as f32 * SAMPLE_STEP;
        let reachable = (corner * corner + 2.0 * limits.brake_accel * dist).sqrt();
        plan.limit = plan.limit.min(reachable);
        if speed > corner {
            let need = (speed * speed - corner * corner) / (2.0 * dist.max(1.0));
            plan.demand = plan.demand.max(need / limits.max_brake_accel);
        }
    }
    plan
}

/// Throttle and brake (both `0..=1`) that hold `speed` to `plan`.
///
/// Under the limit the car drives at `throttle_cap` (eased by the
/// steering's own `bearing_throttle` band when the aim swings wide).
/// Over it, the authored `cornerBrakingThreshold` decides between
/// lifting and braking — the documented reading (R3) is a brake-demand
/// floor: a corner that needs less than that share of the car's
/// braking is made by lifting off, one that needs more by braking at
/// the demand it needs. The threshold is clamped to
/// [`CORNER_BRAKE_MAX`] so a driver never waits for a full stop's
/// worth of demand.
pub fn pace(speed: f32, plan: SpeedPlan, throttle: f32, corner_brake_threshold: f32) -> (f32, f32) {
    if speed <= plan.limit {
        return (throttle, 0.0);
    }
    if speed <= plan.limit + PACE_DEADBAND {
        return (0.0, 0.0);
    }
    let threshold = corner_brake_threshold.clamp(0.0, CORNER_BRAKE_MAX);
    if plan.demand < threshold {
        return (0.0, 0.0);
    }
    (0.0, plan.demand.clamp(0.2, 1.0))
}

/// Normalized steering (`-1..=1`, positive right) that puts the car on
/// the arc through an aim point `bearing` radians off its nose and
/// `aim_dist` metres away — pure pursuit, from this car's geometry
/// instead of a fixed gain on the bearing.
///
/// The arc's curvature `2·sin(bearing)/aim_dist` becomes a wheel angle
/// through the wheelbase, plus the slip the front tires need to make
/// that much cornering force at `speed` (the share of their grip it
/// uses, times their peak slip angle). The angle is then divided by
/// the lock the car actually has at `speed` and passed back through
/// the inverse of its input shaping, so the vehicle's own steering
/// produces it. An aim behind the car takes full lock toward it.
pub fn steer_toward(bearing: f32, aim_dist: f32, speed: f32, limits: &CarLimits) -> f32 {
    if bearing.abs() >= std::f32::consts::FRAC_PI_2 {
        return bearing.signum();
    }
    let curvature = 2.0 * bearing.sin() / aim_dist.max(1.0);
    let geometric = (limits.wheelbase * curvature).atan();
    let lateral = speed * speed * curvature.abs();
    let used = (lateral / (limits.front_grip * G)).min(1.0);
    let angle = geometric + bearing.signum() * used * limits.front_peak_slip;
    let t = (speed.abs() / limits.lock_speed).clamp(0.0, 1.0);
    let lock = (limits.lock_low + (limits.lock_high - limits.lock_low) * t).max(1e-3);
    let shaped = (angle / lock).clamp(-1.0, 1.0);
    // `steering_response` is `sign·|x|^curve`; its inverse is the same
    // form with the reciprocal exponent.
    steering_response(shaped, 1.0 / limits.response_curve)
}

/// Feeler angles (rad) off the nose, each side: a long narrow pair that
/// sees a wall the line runs toward, and a short wide pair that sees
/// one the car is brushing.
const FEELERS: [(f32, f32, f32); 2] = [
    // (angle, base length m, extra length per m/s)
    (0.3, 6.0, 0.6),
    (0.8, 3.0, 0.15),
];
/// Longest any feeler reaches (m).
const FEELER_MAX: f32 = 30.0;
/// Steering correction at a feeler's root — a wall touching the nose
/// adds this much opposite lock on top of the pursuit's own.
const WALL_GAIN: f32 = 0.8;
/// Hits whose surface normal points more upward than this are ground —
/// a road climbing ahead, a ramp, a kerb top — not a wall.
const WALL_NORMAL_Y: f32 = 0.6;

/// What the feelers found: a steering correction away from walls and
/// which side is more open.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WallSense {
    /// Steering to add (positive right), already signed away from the
    /// closer wall.
    pub steer: f32,
    /// How close a wall sits on the left (0 clear … 1 touching).
    pub left: f32,
    /// How close a wall sits on the right.
    pub right: f32,
}

impl WallSense {
    /// The side (±1, positive right) with more room, or `None` when
    /// neither side sees a wall.
    pub fn open_side(&self) -> Option<f32> {
        if self.left == 0.0 && self.right == 0.0 {
            None
        } else if self.left > self.right {
            Some(1.0)
        } else {
            Some(-1.0)
        }
    }
}

/// Probe static walls around the nose of a car heading `fwd` (XZ) at
/// `speed` and steer away from them (DSN-66). `probe(dir, len)` casts a
/// horizontal ray and returns the hit distance and surface normal;
/// ground-facing hits are ignored. Each side's closeness is the worst
/// of its feelers, weighted by how much of the feeler the wall leaves,
/// squared so a distant wall nudges and a near one shoves.
pub fn sense_walls(
    fwd: Vec3,
    speed: f32,
    probe: impl Fn(Vec3, f32) -> Option<(f32, Vec3)>,
) -> WallSense {
    let flat = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or_zero();
    if flat == Vec3::ZERO {
        return WallSense::default();
    }
    let mut sense = WallSense::default();
    for (angle, base, per_speed) in FEELERS {
        let len = (base + per_speed * speed.max(0.0)).min(FEELER_MAX);
        // Positive yaw turns right: right = (-fwd.z, 0, fwd.x).
        for side in [-1.0f32, 1.0] {
            let a = angle * side;
            let dir = Vec3::new(
                flat.x * a.cos() - flat.z * a.sin(),
                0.0,
                flat.z * a.cos() + flat.x * a.sin(),
            );
            let Some((dist, normal)) = probe(dir, len) else {
                continue;
            };
            if normal.y > WALL_NORMAL_Y {
                continue;
            }
            let close = (1.0 - dist / len).clamp(0.0, 1.0).powi(2);
            let slot = if side < 0.0 {
                &mut sense.left
            } else {
                &mut sense.right
            };
            *slot = slot.max(close);
        }
    }
    sense.steer = (sense.left - sense.right) * WALL_GAIN;
    sense
}

fn leg_count(n: usize, closed: bool) -> usize {
    if closed { n } else { n - 1 }
}

fn leg_ends(route: &OpponentRoute, leg: usize) -> (Vec3, Vec3) {
    let n = route.points.len();
    (
        route.points[leg].position,
        route.points[(leg + 1) % n].position,
    )
}

fn xz_len(a: Vec3, b: Vec3) -> f32 {
    (b.x - a.x).hypot(b.z - a.z)
}

/// `a` wrapped into `(-π, π]`.
fn wrap_angle(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let mut a = a % tau;
    if a > std::f32::consts::PI {
        a -= tau;
    } else if a <= -std::f32::consts::PI {
        a += tau;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn limits() -> CarLimits {
        CarLimits {
            corner_accel: 8.0,
            brake_accel: 5.0,
            max_brake_accel: 10.0,
            wheelbase: 2.5,
            front_grip: 1.5,
            front_peak_slip: 0.1,
            lock_low: 0.6,
            lock_high: 0.3,
            lock_speed: 30.0,
            response_curve: 1.0,
        }
    }

    /// A straight line asks nothing of the car's speed.
    #[test]
    fn a_straight_line_sets_no_limit() {
        let r = route(&[[0.0, 0.0], [1000.0, 0.0]]);
        let c = RouteCursor { leg: 0, along: 0.0 };
        let plan = plan_speed(&r, c, 30.0, &limits());
        assert_eq!(plan.limit, f32::INFINITY);
        assert_eq!(plan.demand, 0.0);
    }

    /// A right-angle corner ahead binds the speed: the limit falls as
    /// the car closes on it and sits near the corner speed at its
    /// entry, so braking starts in time instead of at the apex.
    #[test]
    fn a_corner_ahead_limits_speed_by_braking_distance() {
        let r = route(&[[0.0, 0.0], [300.0, 0.0], [300.0, 300.0]]);
        let far = plan_speed(
            &r,
            RouteCursor {
                leg: 0,
                along: 100.0,
            },
            40.0,
            &limits(),
        );
        let near = plan_speed(
            &r,
            RouteCursor {
                leg: 0,
                along: 280.0,
            },
            40.0,
            &limits(),
        );
        assert!(
            far.limit.is_finite() && near.limit < far.limit,
            "{far:?} {near:?}"
        );
        // At the corner the ±12 m window reads a ~15 m radius:
        // sqrt(8 × 15.3) ≈ 11 m/s.
        let at = plan_speed(
            &r,
            RouteCursor {
                leg: 0,
                along: 290.0,
            },
            11.0,
            &limits(),
        );
        assert!((9.0..14.0).contains(&at.limit), "{at:?}");
        // Carrying 40 m/s 20 m out needs far more than full braking.
        assert!(near.demand > 1.0, "{near:?}");
    }

    /// Under the limit the car drives; over it, a corner that needs less
    /// than the authored threshold of braking is made by lifting off,
    /// one that needs more by braking at that demand.
    #[test]
    fn pace_lifts_or_brakes_by_the_authored_threshold() {
        let plan = |demand| SpeedPlan {
            limit: 20.0,
            demand,
        };
        assert_eq!(pace(15.0, plan(0.0), 0.8, 0.7), (0.8, 0.0));
        assert_eq!(pace(20.3, plan(0.9), 0.8, 0.7), (0.0, 0.0), "deadband");
        assert_eq!(pace(25.0, plan(0.5), 0.8, 0.7), (0.0, 0.0), "lift");
        assert_eq!(pace(25.0, plan(0.8), 0.8, 0.7), (0.0, 0.8), "brake");
        assert_eq!(pace(25.0, plan(0.5), 0.8, 0.1), (0.0, 0.5), "eager");
    }

    /// Pursuit steering is signed like the bearing, zero dead ahead,
    /// full lock for an aim behind, and asks for more input at speed —
    /// the lock shrinks and the tires need slip to pull.
    #[test]
    fn steer_toward_follows_the_arc_from_the_cars_geometry() {
        let l = limits();
        assert_eq!(steer_toward(0.0, 20.0, 10.0, &l), 0.0);
        let right = steer_toward(0.2, 20.0, 10.0, &l);
        assert!(right > 0.0 && right < 1.0, "{right}");
        assert_eq!(steer_toward(-0.2, 20.0, 10.0, &l), -right);
        assert_eq!(steer_toward(2.5, 20.0, 10.0, &l), 1.0);
        assert!(steer_toward(0.2, 20.0, 25.0, &l) > right);
        // A nearer aim at the same bearing is a tighter arc.
        assert!(steer_toward(0.2, 10.0, 10.0, &l) > right);
    }

    /// A wall close on the left steers right and marks the right as the
    /// open side; ground-facing hits are not walls.
    #[test]
    fn feelers_steer_away_from_a_wall() {
        let fwd = Vec3::NEG_Z;
        // Right = (-fwd.z, 0, fwd.x) = +X, so a left feeler points -X.
        let left_wall = |dir: Vec3, _len: f32| (dir.x < 0.0).then_some((2.0, Vec3::X));
        let s = sense_walls(fwd, 10.0, left_wall);
        assert!(s.left > 0.0 && s.right == 0.0, "{s:?}");
        assert!(s.steer > 0.0, "steers right, away: {s:?}");
        assert_eq!(s.open_side(), Some(1.0));

        let ground = |_: Vec3, _: f32| Some((2.0, Vec3::Y));
        assert_eq!(sense_walls(fwd, 10.0, ground), WallSense::default());
        assert_eq!(WallSense::default().open_side(), None);
    }
}
