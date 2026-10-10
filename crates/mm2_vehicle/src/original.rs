//! The original game's car model — pure math, no ECS.
//!
//! A port of the retail `Midtown2.exe` vehicle simulation recovered in
//! `docs/research/vehicle-physics/`: the wheel, suspension and stick–slip
//! tyre (02), the engine, clutch, gearbox and drivetrain spin (03), the
//! aero rotational damping (04) and the player's steering filter (05).
//! [`crate::original_step`] runs it once per physics step;
//! [`OriginalHandling`](crate::config::OriginalHandling) carries the
//! authored tuning it reads.
//!
//! One convention differs from the research notes: wheel and drivetrain
//! spin is positive rolling **forward** (the exe's is negative), so every
//! formula that touches a spin carries that sign flip.
//!
//! The original stepped once per frame — one 1/60 s step on the hardware
//! it was tuned on — and several of its terms are per step rather than
//! per second. [`ORIGINAL_STEP`] is the production step. Other step rates
//! retain the original's frame-rate dependence.

use std::f32::consts::TAU;

use bevy::math::Vec3;

use crate::config::{OriginalAero, OriginalEngine, OriginalTrain, OriginalWheel};

/// The step the original's per-step terms were tuned at, seconds.
pub const ORIGINAL_STEP: f32 = 1.0 / 60.0;
/// The suspension probe starts this far above `SuspensionLimit`, metres.
pub const PROBE_ABOVE: f32 = 0.3;
/// A probe hit counts only if the wheel's up axis is at least this
/// aligned with the surface normal.
pub const MIN_UP_DOT_N: f32 = 0.02;
/// A surface whose normal has less vertical component than this gives
/// no grip — the side of a wall.
pub const STEEP_NORMAL_Y: f32 = 0.001;
/// Clamp on the suspension compression rate, m/s.
pub const XDOT_CLAMP: f32 = 10.0;
/// Share of the approach speed the bump stop removes per original step.
pub const BUMP_STOP_K: f32 = 0.25;
/// Tyre bristle relaxation per metre rolled.
pub const RELAX_K: f32 = 0.1;
/// Constant drag on every drivetrain, N·m — the original's only rolling
/// resistance.
pub const DRAG_TORQUE: f32 = 50.0;
/// Spin inertia of a train with no engine, per kilogram of car.
pub const FREE_INERTIA_PER_KG: f32 = 0.005;
/// Added to the engine inertia reflected through the gear, kg·m².
pub const ENGINE_INERTIA_ADD: f32 = 0.02;
/// Limited-slip bias cap at a standstill and above
/// [`DIFF_BIAS_SPEED`], and the shaft speed between them (rad/s).
pub const DIFF_BIAS_LOW: f32 = 1.25;
pub const DIFF_BIAS_HIGH: f32 = 1.03;
pub const DIFF_BIAS_SPEED: f32 = 50.0;
/// Share of the way the bias moves toward its target per original step.
pub const DIFF_SMOOTH: f32 = 0.1;
/// Forward speed below which brake and throttle together release the
/// rear brakes (the burnout "brake-stand"), m/s.
pub const BRAKE_STAND_SPEED: f32 = 1.0;
/// Pedal level both pedals must pass for a brake-stand.
pub const BRAKE_STAND_PEDAL: f32 = 0.95;
/// m/s → mph, the original's `2.236025`.
pub const MPS_TO_MPH: f32 = 2.236025;
/// mph → metres per minute (`1609.344 / 60`).
const MPH_TO_M_PER_MIN: f32 = 26.8224;

/// `sign` with `sign(0) == 0`, as the original's x87 code computes it.
pub fn sgn(x: f32) -> f32 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// Load-dependent constants of one wheel (`vehWheel::SetNormalLoad`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelConstants {
    /// Spring rate at rest, N/m.
    pub ks: f32,
    /// Spring progressivity, 1/m.
    pub k2: f32,
    /// Damper, N·s/m.
    pub cs: f32,
    /// Lateral bristle stiffness, N/m.
    pub k_lat: f32,
    /// Lateral bristle damping, N·s/m.
    pub c_lat: f32,
    /// Longitudinal bristle stiffness, N/m.
    pub k_long: f32,
    /// Longitudinal bristle damping, N·s/m.
    pub c_long: f32,
    /// Foot-brake torque at full pedal, N·m.
    pub brake_max: f32,
    /// Handbrake torque at full lever, N·m.
    pub handbrake_max: f32,
}

impl WheelConstants {
    /// Constants for `w` on a wheel of `radius`, with the quarter mass
    /// taken at `gravity` (the original's 19.6).
    pub fn of(w: &OriginalWheel, radius: f32, gravity: f32) -> Self {
        let load = w.static_load.max(1.0);
        let extent = w.suspension_extent.max(1e-3);
        let limit = w.suspension_limit.max(0.0);
        let factor = w.suspension_factor.max(0.75);
        let a = 1.0 / ((limit + extent) * extent);
        let ks = (factor * extent + limit) * a * load;
        let k2 = (factor - 1.0) / (factor * extent + limit);
        let cs = 2.0 * (ks * load).sqrt() * w.suspension_damp_coef.max(0.0);
        let quarter_mass = load / gravity.max(1e-3);
        let k_long = 2.0 * load / w.tire_disp_limit_long.max(1e-4);
        let k_lat = 2.0 * load / w.tire_disp_limit_lat.max(1e-4);
        Self {
            ks,
            k2,
            cs,
            k_lat,
            c_lat: 2.0 * (k_lat * quarter_mass).sqrt() * w.tire_damp_coef_lat.max(0.0),
            k_long,
            c_long: 2.0 * (k_long * quarter_mass).sqrt() * w.tire_damp_coef_long.max(0.0),
            brake_max: w.brake_coef.max(0.0) * w.static_fric * radius * load,
            handbrake_max: w.handbrake_coef.max(0.0) * w.static_fric * radius * load,
        }
    }
}

/// The wheel's steer angle for a steering input (`vehWheel::SetInputs`),
/// radians, **positive turning right**. A front wheel follows the input
/// through its lock with the Ackermann gain (`SteeringOffset`) turning
/// the inside wheel further; a rear wheel steers the opposite way by its
/// own lock (the Beetle, Dune Buggy, VW Cup and TT counter-steer theirs
/// by a degree or two).
pub fn wheel_steer(w: &OriginalWheel, steering: f32) -> f32 {
    let input = if w.rear { -steering } else { steering };
    // The original's angle is positive to the *left*.
    let a = -input * w.steering_limit;
    let left = (1.0 - a * w.steering_offset * w.side) * a;
    -left
}

/// One suspension update (`vehWheel` `0x4d2710`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Suspension {
    /// Compression, metres: `0` at rest on the modelled pivot, `-Extent`
    /// at full droop, `Limit` at the bump stop.
    pub x: f32,
    /// Compression rate, m/s (clamped ±[`XDOT_CLAMP`]).
    pub xdot: f32,
    /// Spring and damper force along the ground normal, N.
    pub force: f32,
    /// How far the probe put the wheel past the bump stop, metres; the
    /// caller answers it with [`bump_stop_force`].
    pub overshoot: f32,
}

/// Run the suspension from `x_old` given the probe's compression
/// (`None` when the wheel found no ground).
///
/// The finite-difference damper is the authored one. Its coupled implicit
/// coefficient is accumulated separately by the production body adapter.
pub fn suspension(
    x_old: f32,
    probe: Option<f32>,
    w: &OriginalWheel,
    k: &WheelConstants,
    dt: f32,
) -> Suspension {
    let extent = w.suspension_extent.max(1e-3);
    let limit = w.suspension_limit.max(0.0);
    let load = w.static_load.max(1.0);
    let x = probe.map_or(-extent, |x_in| x_in.max(-extent));
    let damping = k.cs;
    let rate = |x: f32| ((x - x_old) / dt).clamp(-XDOT_CLAMP, XDOT_CLAMP);
    let xdot = rate(x);
    let force = (damping * xdot + k.ks * x) * (1.0 + k.k2 * x) + load;
    if force < 0.0 {
        // Unloaded: the wheel droops toward full extension with the
        // first-order lag its damper allows.
        let x = (x_old * k.cs - dt * k.ks * extent) / (dt * k.ks + k.cs).max(1e-6);
        return Suspension {
            x,
            xdot: (x - x_old) / dt,
            force: 0.0,
            overshoot: 0.0,
        };
    }
    if x > limit {
        let xdot = rate(limit);
        return Suspension {
            x: limit,
            xdot,
            force: (damping * xdot + k.ks * limit) * (1.0 + k.k2 * limit) + load,
            overshoot: x - limit,
        };
    }
    Suspension {
        x,
        xdot,
        force,
        overshoot: 0.0,
    }
}

/// The bump stop's extra force along the normal, N, for a wheel that has
/// gone `overshoot` past `SuspensionLimit` with the contact point
/// approaching the ground at `-v_n` (m/s). `effective_mass` is the body's
/// mass as seen at the contact point along the normal.
///
/// The original removes a quarter of the approach speed per step. It also pushes the body straight back out by the overshoot,
/// a position write that adds no speed: the step does that part (see
/// `OriginalState::push`). A spring in its place would give the landing
/// back — a car dropped from 4 m bounced off at three quarters of the
/// speed it hit at.
pub fn bump_stop_force(effective_mass: f32, v_n: f32, overshoot: f32, dt: f32) -> f32 {
    if overshoot <= 0.0 {
        return 0.0;
    }
    effective_mass * BUMP_STOP_K * (-v_n).max(0.0) / dt
}

/// A tyre's bristle displacements, metres (the stick–slip state).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Bristles {
    /// Lateral, `+` right.
    pub lat: f32,
    /// Longitudinal, `+` forward slip.
    pub long: f32,
}

/// What a tyre sees this step.
#[derive(Debug, Clone, Copy)]
pub struct TyreInput {
    /// Contact-patch velocity along the tyre's right axis, m/s.
    pub v_lat: f32,
    /// Along its forward axis, m/s.
    pub v_fwd: f32,
    /// Longitudinal slip velocity `v_fwd − ω·r`, m/s.
    pub v_slip: f32,
    /// Normal load, N.
    pub load: f32,
    /// Peak μ on this surface (`StaticFric × surface`).
    pub mu_static: f32,
    /// Sliding μ on this surface (`SlidingFric × surface`).
    pub mu_sliding: f32,
    /// `|ω·r|`, m/s — what relaxes a sliding bristle.
    pub rolling_speed: f32,
    /// Step length, seconds.
    pub dt: f32,
}

/// What a tyre produces this step.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TyreOutput {
    /// Lateral force along the tyre's right axis, N.
    pub f_lat: f32,
    /// Longitudinal force along its forward axis, N.
    pub f_long: f32,
    /// Lateral slip ratio (tangent of the slip angle), ±1.
    pub s_lat: f32,
    /// Longitudinal slip ratio, ±1.
    pub s_long: f32,
    /// 0..1 slide intensity — the original's skid/smoke drive; `≥ 0.5`
    /// is a major slip.
    pub slip_visual: f32,
}

/// `a / |b|` saturating at ±1, `0` when `a` is: the slip ratio.
fn slip_ratio(a: f32, b: f32) -> f32 {
    if a == 0.0 {
        0.0
    } else if a.abs() <= b.abs() {
        a / b.abs()
    } else {
        sgn(a)
    }
}

/// The friction curve (`ComputeFriction`): a parabola from 0 to the peak
/// μ at the optimum slip, falling back down it past the optimum to the
/// sliding floor. Returns `(μ, slide intensity)`.
pub fn friction(s: f32, mu_static: f32, mu_sliding: f32, optimum: f32) -> (f32, f32) {
    let s = s.abs();
    let opt = optimum.max(1e-4);
    let f = mu_static * s * (2.0 * opt - s) / (opt * opt);
    if s <= opt {
        (f, 0.5 * s / opt)
    } else if f > mu_sliding {
        (
            f,
            ((mu_static - f) + 0.5 * (f - mu_sliding)) / (mu_static - mu_sliding),
        )
    } else {
        (mu_sliding, 1.0)
    }
}

/// One bristle axis: its μ, displacement limit and slide intensity.
fn axis_limit(s: f32, d: f32, step: f32, u: f32, input: &TyreInput, opt: f32) -> (f32, f32, f32) {
    let curve = || {
        let (mu, vis) = friction(s, input.mu_static, input.mu_sliding, opt);
        (mu, mu * u, vis)
    };
    if s.abs() <= opt {
        return curve();
    }
    // Kinematically past the optimum — but a bristle with room to take
    // this step still holds on static friction.
    let lim = input.mu_static * u;
    let holds = if step > 0.0 {
        d < lim && lim - d >= step
    } else {
        d > lim && lim - d <= step
    };
    if holds {
        (input.mu_static, lim, 0.0)
    } else {
        curve()
    }
}

/// Advance one bristle by `step` toward `lim`: `(displacement, velocity
/// step, sticking)`.
fn bristle(d: f32, step: f32, lim: f32, relax: f32) -> (f32, f32, bool) {
    let cand = d + step;
    let within = if step >= 0.0 {
        cand <= lim
    } else {
        cand >= lim
    };
    if within {
        (cand, step, true)
    } else if step >= 0.0 {
        ((d - relax).max(lim), 0.0, false)
    } else {
        ((d + relax).min(lim), 0.0, false)
    }
}

/// The stick–slip tyre (`vehWheel::Update`, `0x4d34d0`): a stiff
/// spring–damper per axis while its bristle holds inside `μ·N/k`, the
/// slip curve's `μ(s)·N` once it slides, and a friction circle on top.
pub fn tyre(
    bristles: &mut Bristles,
    w: &OriginalWheel,
    k: &WheelConstants,
    input: &TyreInput,
) -> TyreOutput {
    let dt = input.dt.max(1e-6);
    let opt = w.optimum_slip.max(1e-4);
    let n = input.load.max(0.0);
    let s_lat = slip_ratio(input.v_lat, input.v_fwd);
    let s_long = slip_ratio(input.v_slip, input.v_fwd);
    let u_long = sgn(input.v_slip) * n / k.k_long.max(1e-3);
    let u_lat = sgn(input.v_lat) * n / k.k_lat.max(1e-3);
    let step_long = dt * input.v_slip;
    let step_lat = dt * input.v_lat;
    let (mu_long, mut lim_long, vis_long) =
        axis_limit(s_long, bristles.long, step_long, u_long, input, opt);
    let (mu_lat, mut lim_lat, vis_lat) =
        axis_limit(s_lat, bristles.lat, step_lat, u_lat, input, opt);

    // The circle's coefficient: static while both axes are inside the
    // optimum, else the dominant axis's, which also caps the other.
    let mu_c = if s_long.abs().max(s_lat.abs()) < opt {
        input.mu_static
    } else if s_long.abs() >= s_lat.abs() {
        if mu_lat > mu_long {
            lim_lat = mu_long * u_lat;
        }
        mu_long
    } else {
        if mu_long > mu_lat {
            lim_long = mu_lat * u_long;
        }
        mu_lat
    };

    let relax = input.rolling_speed.abs() * dt * RELAX_K;
    let (d_long, vel_long, stick_long) = bristle(bristles.long, step_long, lim_long, relax);
    let (d_lat, vel_lat, stick_lat) = bristle(bristles.lat, step_lat, lim_lat, relax);
    bristles.long = d_long;
    bristles.lat = d_lat;

    let mut f_lat = -k.k_lat * d_lat - vel_lat / dt * k.c_lat;
    let mut f_long = -k.k_long * d_long - vel_long / dt * k.c_long;
    let f_max = mu_c * n;
    let f_sq = f_lat * f_lat + f_long * f_long;
    if f_sq > f_max * f_max {
        let gripping = (stick_long || s_long.abs() <= opt) && (stick_lat || s_lat.abs() <= opt);
        let slip_speed = input.v_lat.hypot(input.v_slip);
        if gripping || slip_speed <= 0.0 {
            let scale = f_max / f_sq.sqrt();
            f_lat *= scale;
            f_long *= scale;
        } else {
            // Sliding: the force opposes the slip velocity.
            let scale = f_max / slip_speed;
            f_lat = -scale * input.v_lat;
            f_long = -scale * input.v_slip;
        }
    }

    let slip_visual = match (stick_lat, stick_long) {
        (true, true) => 0.0,
        (false, true) => vis_lat,
        (true, false) => vis_long,
        (false, false) => vis_lat.max(vis_long),
    };
    TyreOutput {
        f_lat,
        f_long,
        s_lat,
        s_long,
        slip_visual: slip_visual.clamp(0.0, 1.0),
    }
}

/// An engine attached to a drivetrain through the current gear.
#[derive(Debug, Clone, Copy)]
pub struct Coupling {
    /// Overall ratio of the selected gear (negative in reverse).
    pub ratio: f32,
    /// Engine torque this step, N·m.
    pub torque: f32,
    /// Engine inertia, kg·m².
    pub inertia: f32,
    /// Engine speed, rad/s.
    pub omega: f32,
    /// The limiter, rad/s.
    pub omega_max: f32,
}

/// What a drivetrain integrates over one step.
#[derive(Debug, Clone, Copy)]
pub struct TrainInput<'a> {
    /// Shaft speed, wheel rad/s, `+` rolling forward.
    pub omega: f32,
    /// Left/right speed bias of the limited slip.
    pub bias: f32,
    /// Sum of the train's wheels' brake torques, N·m.
    pub brake_torque: f32,
    /// Last step's tyre reaction torque (`F_long · r`) per wheel, in
    /// left/right pairs for a train of two or four.
    pub reactions: &'a [f32],
    /// The engine, when this is the attached primary train.
    pub coupling: Option<Coupling>,
    /// Spin inertia of the train without an engine, kg·m².
    pub free_inertia: f32,
    /// The train's authored block.
    pub train: &'a OriginalTrain,
    /// Step length, seconds.
    pub dt: f32,
}

/// A drivetrain after one step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrainOutput {
    /// New shaft speed, wheel rad/s.
    pub omega: f32,
    /// New limited-slip bias.
    pub bias: f32,
    /// The engine's new speed, when the train set it.
    pub engine_omega: Option<f32>,
}

/// One drivetrain step (`vehDrivetrain::Update`, `0x4d9e80`): brakes,
/// engine and last step's tyre reactions integrated into the shaft spin
/// through `I + dt·AngInertia`, the brake's stop at zero, the engine's
/// limiter, and the kinematic limited slip.
///
/// Use the actual step in the implicit denominator, as retail does.
/// Production runs at 60 Hz; probes using another rate deliberately measure
/// the original model's frame-rate dependence.
pub fn step_train(input: &TrainInput) -> TrainOutput {
    let TrainInput {
        omega,
        bias,
        brake_torque,
        reactions,
        coupling,
        free_inertia,
        train,
        dt,
    } = *input;
    let dt = dt.max(1e-6);
    let coef = if omega != 0.0 {
        train.brake_dynamic_coef
    } else {
        train.brake_static_coef
    };
    let brake = DRAG_TORQUE + brake_torque * coef;
    let (mut torque, inertia) = match coupling {
        Some(c) => (
            c.ratio * c.torque + c.ratio * c.inertia * (c.omega - c.ratio * omega) / dt,
            c.ratio * c.ratio * c.inertia + ENGINE_INERTIA_ADD,
        ),
        None => (0.0, free_inertia),
    };
    torque -= reactions.iter().sum::<f32>();
    let can_stop = if omega != 0.0 {
        let can_stop = torque.abs() <= brake;
        torque -= sgn(omega) * brake;
        can_stop
    } else {
        // A stopped train's brake holds up to its full torque.
        torque = if torque >= 0.0 {
            (torque - brake).max(0.0)
        } else {
            (torque + brake).min(0.0)
        };
        false
    };

    let bias = if reactions.len() >= 2 && omega.abs() >= 0.001 {
        let speed = omega.abs();
        let max_bias = if speed >= DIFF_BIAS_SPEED {
            DIFF_BIAS_HIGH
        } else {
            (DIFF_BIAS_LOW * (DIFF_BIAS_SPEED - speed) + DIFF_BIAS_HIGH * speed) / DIFF_BIAS_SPEED
        };
        let imbalance: f32 = reactions
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| p[0] - p[1])
            .sum();
        let target = bias - imbalance / (train.ang_inertia.max(1.0) * omega);
        let smooth = DIFF_SMOOTH;
        bias + smooth * (target.clamp(1.0 / max_bias, max_bias) - bias)
    } else {
        1.0
    };

    let mut next = omega + dt * torque / (inertia + dt * train.ang_inertia.max(0.0));
    if can_stop && sgn(next) != sgn(omega) {
        next = 0.0;
    }
    let mut engine_omega = None;
    if let Some(c) = coupling {
        let e = c.ratio * next;
        if e < 0.0 {
            // The wheels never drive the engine backwards.
            next = 0.0;
        } else if e > c.omega_max {
            next = c.omega_max / c.ratio;
            engine_omega = Some(c.omega_max);
        } else {
            engine_omega = Some(e);
        }
    }
    TrainOutput {
        omega: next,
        bias,
        engine_omega,
    }
}

/// The engine's derived constants (`vehEngine::ComputeConstants`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineConstants {
    /// Idle, peak-power and limiter speeds, rad/s.
    pub omega_idle: f32,
    pub omega_opt: f32,
    pub omega_max: f32,
    /// Torque at peak power, N·m.
    pub torque_opt: f32,
    /// Engine inertia, kg·m².
    pub inertia: f32,
    /// Seconds of zero torque per gear change.
    pub gear_change_lag: f32,
    /// Curve scale `P / ω_opt³`.
    p: f32,
}

/// `(√5 + 1)/2` and `(√5 − 1)/2`: they make the curve below `OptRPM` a
/// parabola through `T_opt` at both zero and `OptRPM`.
// Clippy's `approx_constant` (1.99+) recognises this as the golden ratio.
// It is: the constant is the algebraic value above, written out so the
// crate does not depend on `f32::consts::GOLDEN_RATIO` being stable on
// every toolchain it builds with.
#[allow(clippy::approx_constant)]
const CURVE_A: f32 = 1.618_034;
const CURVE_B: f32 = 0.618_034;

/// rpm → rad/s.
pub fn rpm_to_omega(rpm: f32) -> f32 {
    rpm * TAU / 60.0
}

impl EngineConstants {
    pub fn of(e: &OriginalEngine) -> Self {
        let omega_opt = rpm_to_omega(e.opt_rpm).max(1.0);
        Self {
            omega_idle: rpm_to_omega(e.idle_rpm).max(0.0),
            omega_opt,
            omega_max: rpm_to_omega(e.max_rpm).max(omega_opt * 1.001),
            torque_opt: e.max_power_w / omega_opt,
            inertia: e.ang_inertia.max(1e-3),
            gear_change_lag: e.gear_change_lag.max(0.0),
            p: e.max_power_w / (omega_opt * omega_opt * omega_opt),
        }
    }

    /// Full-throttle torque at `omega` (`CalcTorqueAtFullThrottle`):
    /// peak power exactly at `OptRPM`, `1.25 T_opt` at half of it, a
    /// quartic down to zero at `MaxRPM`.
    pub fn full_throttle(&self, omega: f32) -> f32 {
        let (opt, max) = (self.omega_opt, self.omega_max);
        let shape = (CURVE_A * opt - omega) * (CURVE_B * opt + omega);
        if omega <= opt {
            self.p * shape
        } else if omega <= max {
            self.p * (max - omega) * shape * (omega + max - 2.0 * opt) / ((max - opt) * (max - opt))
        } else {
            0.0
        }
    }

    /// Zero-throttle torque (`CalcTorqueAtZeroThrottle`): zero at idle,
    /// pushing up below it and braking linearly above it.
    pub fn zero_throttle(&self, omega: f32) -> f32 {
        0.75 * self.torque_opt * (self.omega_idle - omega)
            / (self.omega_opt - self.omega_idle).max(1.0)
    }

    /// Full-throttle power at `rpm`, watts — zero past the limiter.
    pub fn power_at_rpm(&self, rpm: f32) -> f32 {
        let omega = rpm_to_omega(rpm);
        if omega > self.omega_max {
            0.0
        } else {
            self.full_throttle(omega) * omega
        }
    }
}

/// The automatic box's overall ratios (`vehTransmission::ComputeConstants`)
/// for a primary drivetrain on wheels of `radius`: `[0]` reverse
/// (negative), `[1]` neutral, `[2]` first at `low_mph`, the top gear at
/// `high_mph`, each at `OptRPM`, the middle gears spaced geometrically
/// with `GearBias` pushing them taller. `gears` counts R and N.
pub fn gear_ratios(
    opt_rpm: f32,
    radius: f32,
    reverse_mph: f32,
    low_mph: f32,
    high_mph: f32,
    gears: usize,
    gear_bias: f32,
) -> Vec<f32> {
    let from_mph =
        |mph: f32| opt_rpm / (MPH_TO_M_PER_MIN * mph.max(0.1) / (TAU * radius.max(0.01)));
    let n = gears.max(3);
    let mut ratios = vec![0.0; n];
    ratios[0] = -from_mph(reverse_mph);
    ratios[2] = from_mph(low_mph);
    ratios[n - 1] = from_mph(high_mph);
    let span = n - 3;
    if span > 0 {
        let q = (ratios[n - 1] / ratios[2]).powf(1.0 / span as f32);
        for i in 1..span {
            let i_f = i as f32;
            let exponent = i_f + gear_bias * i_f * (span as f32 - i_f) / span as f32;
            ratios[2 + i] = ratios[2] * q.powf(exponent);
        }
    }
    ratios
}

/// The automatic box's shift points per gear index, rpm.
#[derive(Debug, Clone, PartialEq)]
pub struct ShiftPoints {
    /// Shift up above this.
    pub upshift: Vec<f32>,
    /// Shift down below this at full throttle.
    pub downshift_full: Vec<f32>,
    /// Shift down below this at zero throttle.
    pub downshift_zero: Vec<f32>,
    /// Full-throttle power here equals the power after the upshift.
    pub equal_power: Vec<f32>,
}

/// The automatic box's shift points (`vehTransmission::ComputeConstants`).
/// For each gear the equal-power rpm `c` — where full-throttle power in
/// it equals power after the upshift — is found by bisection; the box
/// shifts up at `(1 + UpshiftBias)·c` and back down at
/// `(1 − DownshiftBias)·c·v` in the next gear (`v` the ratio step). The
/// top gear "shifts up" at `MaxRPM`.
///
pub fn shift_points(
    ratios: &[f32],
    engine: &EngineConstants,
    max_rpm: f32,
    upshift_bias: f32,
    downshift_bias_min: f32,
    downshift_bias_max: f32,
) -> ShiftPoints {
    let n = ratios.len();
    let mut up = vec![max_rpm; n];
    let mut down_full = vec![0.0; n];
    let mut down_zero = vec![0.0; n];
    let mut equal_power = vec![max_rpm; n];
    let opt_rpm = engine.omega_opt * 60.0 / TAU;
    for g in 2..n.saturating_sub(1) {
        if ratios[g] <= 0.0 {
            continue;
        }
        let v = ratios[g + 1] / ratios[g];
        let (mut lo, mut hi) = (
            opt_rpm,
            if max_rpm <= v * opt_rpm {
                max_rpm
            } else {
                opt_rpm / v
            },
        );
        while hi - lo > 1.0 {
            let mid = 0.5 * (lo + hi);
            if engine.power_at_rpm(mid) > engine.power_at_rpm(mid * v) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let c = 0.5 * (lo + hi);
        equal_power[g] = c;
        up[g] = (1.0 + upshift_bias) * c;
        down_full[g + 1] = (1.0 - downshift_bias_min) * c * v;
        down_zero[g + 1] = (1.0 - downshift_bias_max) * c * v;
    }
    ShiftPoints {
        upshift: up,
        downshift_full: down_full,
        downshift_zero: down_zero,
        equal_power,
    }
}

/// Gear index of first in the original's numbering (`0` R, `1` N).
pub const FIRST_GEAR: usize = 2;
/// Gear index of reverse.
pub const REVERSE_GEAR: usize = 0;

/// Engine, clutch and automatic gearbox state (`vehEngine` +
/// `vehTransmission`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Powertrain {
    /// Selected gear in the original's numbering.
    pub gear: usize,
    /// Engine speed, rad/s.
    pub engine_omega: f32,
    /// Whether the automatic clutch has the drivetrain attached.
    pub clutch: bool,
    /// Engine torque this step, N·m (zero through a gear change).
    pub torque: f32,
    /// Displayed engine rpm (blended through a gear change).
    pub rpm: f32,
    /// Seconds in the current gear.
    pub time_in_gear: f32,
    /// A gear change waiting for its torque cut to finish.
    pub gear_changed: bool,
    in_gear_change: bool,
    lag_left: f32,
    rpm_at_shift: f32,
}

impl Powertrain {
    /// A fresh powertrain (`vehEngine::Reset`, 0x4d8cd0): the displayed
    /// RPM starts at idle, but the cold engine's actual spin starts at zero.
    pub fn new(engine: &EngineConstants) -> Self {
        Self {
            gear: FIRST_GEAR,
            engine_omega: 0.0,
            clutch: false,
            torque: 0.0,
            rpm: engine.omega_idle * 60.0 / TAU,
            time_in_gear: 0.0,
            gear_changed: true,
            in_gear_change: true,
            lag_left: engine.gear_change_lag,
            rpm_at_shift: engine.omega_idle * 60.0 / TAU,
        }
    }

    /// Seconds of torque cut left in the current gear change.
    pub fn shift_lag_left(&self) -> f32 {
        if self.in_gear_change {
            self.lag_left.max(0.0)
        } else {
            0.0
        }
    }

    /// `SetGear`: a change restarts the gear timer and the torque cut.
    pub fn set_gear(&mut self, gear: usize) {
        if gear != self.gear {
            self.gear = gear;
            self.time_in_gear = 0.0;
            self.gear_changed = true;
        }
    }

    /// `vehEngine::Update`: this step's torque from the throttle (drive
    /// scaled by `power_scale`), the automatic clutch — open below idle,
    /// closed again above twice idle, so the car never stalls and a
    /// launch is a clutch drop — free-revving while open, and the
    /// gear-change torque cut.
    pub fn update_engine(
        &mut self,
        engine: &EngineConstants,
        ratio: f32,
        throttle: f32,
        power_scale: f32,
        dt: f32,
    ) {
        if self.gear_changed && !self.in_gear_change {
            self.rpm_at_shift = self.rpm;
            self.lag_left = engine.gear_change_lag;
            self.in_gear_change = true;
        }
        let throttle = throttle.clamp(0.0, 1.0);
        let omega = self.engine_omega;
        let mut torque = throttle * engine.full_throttle(omega) * power_scale
            + (1.0 - throttle) * engine.zero_throttle(omega);
        if ratio == 0.0 || omega < engine.omega_idle {
            self.clutch = false;
        } else if omega > 2.0 * engine.omega_idle {
            self.clutch = true;
        }
        if !self.clutch {
            self.engine_omega = (omega + dt * torque / engine.inertia).clamp(0.0, engine.omega_max);
        }
        self.rpm = self.engine_omega * 60.0 / TAU;
        if self.in_gear_change {
            if self.lag_left <= 0.0 {
                self.in_gear_change = false;
                self.gear_changed = false;
                self.engine_omega = rpm_to_omega(self.rpm);
            } else {
                torque = 0.0;
                let lag = engine.gear_change_lag.max(1e-6);
                self.rpm =
                    (self.rpm_at_shift * self.lag_left + (lag - self.lag_left) * self.rpm) / lag;
                self.lag_left -= dt;
            }
        }
        self.torque = torque;
    }

    /// `vehTransmission::Update`: the automatic box shifts only with a
    /// wheel on the ground, only from a forward gear it has held for
    /// `GearChangeTime`, and never during a change.
    ///
    pub fn update_gearbox(
        &mut self,
        gearbox: &crate::config::OriginalGearbox,
        throttle: f32,
        on_ground: bool,
        automatic: bool,
        dt: f32,
    ) {
        if !on_ground {
            return;
        }
        let n = gearbox.ratios.len();
        if automatic
            && self.gear >= FIRST_GEAR
            && self.time_in_gear > gearbox.gear_change_time
            && !self.gear_changed
        {
            let g = self.gear;
            let at = |v: &[f32]| v.get(g).copied().unwrap_or(0.0);
            let throttle = throttle.clamp(0.0, 1.0);
            if g + 1 < n && self.rpm > at(&gearbox.upshift_rpm) {
                self.set_gear(g + 1);
            } else if g > FIRST_GEAR
                && self.rpm
                    < throttle * at(&gearbox.downshift_full_rpm)
                        + (1.0 - throttle) * at(&gearbox.downshift_zero_rpm)
            {
                self.set_gear(g - 1);
            }
        }
        self.time_in_gear += dt;
    }
}

/// `vehAero`'s rotational damping as an angular acceleration per car
/// axis (`x` pitch, `y` yaw, `z` roll), rad/s², for the body's angular
/// velocity in car axes: constant, linear and quadratic terms that never
/// reverse the spin within one step, faded out below 1 rad/s.
///
/// The original fades by the *world*-axis component of the angular
/// velocity, so how much pitch and roll damping a slow rotation got
/// depended on which way the car faced; this fades by the car-axis one
/// the rest of the routine uses.
pub fn aero_damping(omega_local: Vec3, aero: &OriginalAero, dt: f32) -> Vec3 {
    let dt = dt.max(1e-6);
    let mut alpha = [0.0f32; 3];
    for (a, out) in alpha.iter_mut().enumerate() {
        let w = omega_local[a];
        let mut acc = -sgn(w) * aero.ang_c_damp[a]
            - w * aero.ang_vel_damp[a]
            - w.abs() * w * aero.ang_vel2_damp[a];
        if acc.abs() * dt > w.abs() {
            acc = -w / dt;
        }
        if w.abs() < 1.0 {
            acc *= w.abs();
        }
        *out = acc;
    }
    Vec3::from(alpha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::OriginalGearbox;

    /// The Beetle's front wheel (`vpbug`, 07 § Front wheels).
    fn beetle_front() -> OriginalWheel {
        OriginalWheel {
            static_load: 4900.0,
            rear: false,
            side: -1.0,
            width: 0.25,
            suspension_extent: 0.15,
            suspension_limit: 0.05,
            suspension_factor: 1.0,
            suspension_damp_coef: 0.1,
            steering_limit: 0.4,
            steering_offset: 0.25,
            brake_coef: 0.625,
            handbrake_coef: 0.0,
            tire_disp_limit_lat: 0.125,
            tire_disp_limit_long: 0.125,
            tire_damp_coef_lat: 0.25,
            tire_damp_coef_long: 0.25,
            tire_drag_coef_lat: 0.05,
            tire_drag_coef_long: 0.02,
            optimum_slip: 0.16,
            static_fric: 3.0,
            sliding_fric: 2.7,
        }
    }

    fn beetle_engine() -> OriginalEngine {
        OriginalEngine {
            max_power_w: 260.0 * 746.0,
            idle_rpm: 750.0,
            opt_rpm: 5800.0,
            max_rpm: 8500.0,
            ang_inertia: 1.0,
            gear_change_lag: 0.25,
        }
    }

    #[test]
    fn wheel_constants_match_the_worked_beetle_example() {
        // 02 § SetNormalLoad: ks 32 667, cs 2 530, kLat 78 400, cLat 2 214.
        let k = WheelConstants::of(&beetle_front(), 0.337, 19.6);
        assert!((k.ks - 32_667.0).abs() < 1.0, "ks {}", k.ks);
        assert!((k.cs - 2_530.0).abs() < 2.0, "cs {}", k.cs);
        assert!((k.k_lat - 78_400.0).abs() < 1.0, "kLat {}", k.k_lat);
        assert!((k.c_lat - 2_214.0).abs() < 2.0, "cLat {}", k.c_lat);
        assert_eq!(k.k2, 0.0);
    }

    #[test]
    fn the_spring_carries_its_load_at_rest_and_nothing_at_full_droop() {
        let w = beetle_front();
        let k = WheelConstants::of(&w, 0.337, 19.6);
        let rest = suspension(0.0, Some(0.0), &w, &k, 1.0 / 120.0);
        assert!((rest.force - w.static_load).abs() < 1e-2);
        // Settled at full droop the spring exerts nothing.
        let droop = suspension(-0.15, Some(-0.15), &w, &k, 1.0 / 120.0);
        assert!(droop.force.abs() < 1e-2, "{}", droop.force);
        // Past the bump stop the wheel is held at the limit and the
        // overshoot reported.
        let bump = suspension(0.05, Some(0.08), &w, &k, 1.0 / 120.0);
        assert_eq!(bump.x, 0.05);
        assert!((bump.overshoot - 0.03).abs() < 1e-6);
        // A wheel off the ground droops with a lag instead of snapping.
        let air = suspension(0.0, None, &w, &k, 1.0 / 120.0);
        assert_eq!(air.force, 0.0);
        assert!(air.x < 0.0 && air.x > -0.15, "{}", air.x);
    }

    #[test]
    fn ackermann_turns_the_inside_wheel_further_and_the_rear_against() {
        let left = beetle_front();
        let right = OriginalWheel { side: 1.0, ..left };
        // Turning right the right wheel is inside: 02 § SetInputs has
        // 0.44 against 0.36 rad at full lock.
        assert!((wheel_steer(&right, 1.0) - 0.44).abs() < 1e-4);
        assert!((wheel_steer(&left, 1.0) - 0.36).abs() < 1e-4);
        let rear = OriginalWheel {
            rear: true,
            steering_limit: 0.03,
            steering_offset: 0.0,
            ..left
        };
        assert!((wheel_steer(&rear, 1.0) + 0.03).abs() < 1e-6);
    }

    #[test]
    fn the_friction_curve_peaks_at_the_optimum_and_floors_at_sliding() {
        let (mu, vis) = friction(0.16, 2.7, 2.43, 0.16);
        assert!((mu - 2.7).abs() < 1e-5 && (vis - 0.5).abs() < 1e-5);
        let (mu, _) = friction(0.08, 2.7, 2.43, 0.16);
        assert!((mu - 2.7 * 0.75).abs() < 1e-4);
        // 02: the Beetle front is fully sliding by s = 0.21.
        let (mu, vis) = friction(0.25, 2.7, 2.43, 0.16);
        assert_eq!((mu, vis), (2.43, 1.0));
    }

    #[test]
    fn a_cornering_tyre_settles_on_the_slip_curve() {
        let w = beetle_front();
        let k = WheelConstants::of(&w, 0.337, 19.6);
        let mut b = Bristles::default();
        let input = TyreInput {
            v_lat: 2.4,
            v_fwd: 30.0,
            v_slip: 0.0,
            load: 4900.0,
            mu_static: 2.7,
            mu_sliding: 2.43,
            rolling_speed: 30.0,
            dt: 1.0 / 120.0,
        };
        let mut out = TyreOutput::default();
        for _ in 0..240 {
            out = tyre(&mut b, &w, &k, &input);
        }
        // s = 0.08: half the optimum, 3/4 of peak μ, pushing left.
        let expected = -0.75 * 2.7 * 4900.0;
        assert!((out.f_lat - expected).abs() < 5.0, "{}", out.f_lat);
        assert!((out.s_lat - 0.08).abs() < 1e-6);
    }

    #[test]
    fn a_parked_tyre_holds_on_static_friction() {
        // A small creep is absorbed by the bristle: the force opposes it
        // and the bristle sticks rather than sliding.
        let w = beetle_front();
        let k = WheelConstants::of(&w, 0.337, 19.6);
        let mut b = Bristles::default();
        let out = tyre(
            &mut b,
            &w,
            &k,
            &TyreInput {
                v_lat: 0.01,
                v_fwd: 0.0,
                v_slip: 0.0,
                load: 4900.0,
                mu_static: 2.7,
                mu_sliding: 2.43,
                rolling_speed: 0.0,
                dt: 1.0 / 120.0,
            },
        );
        assert!(out.f_lat < 0.0);
        assert_eq!(out.slip_visual, 0.0);
        assert!(b.lat > 0.0);
    }

    #[test]
    fn a_braked_train_stops_at_zero_and_holds() {
        let train = OriginalTrain {
            ang_inertia: 2000.0,
            brake_dynamic_coef: 1.0,
            brake_static_coef: 1.2,
        };
        let mut omega = 3.0;
        for _ in 0..240 {
            omega = step_train(&TrainInput {
                omega,
                bias: 1.0,
                brake_torque: 5000.0,
                reactions: &[0.0],
                coupling: None,
                free_inertia: 5.0,
                train: &train,
                dt: 1.0 / 120.0,
            })
            .omega;
        }
        assert_eq!(omega, 0.0);
    }

    #[test]
    fn the_engine_curve_peaks_power_at_opt_rpm() {
        let e = EngineConstants::of(&beetle_engine());
        let t_opt = e.torque_opt;
        assert!((e.full_throttle(0.0) - t_opt).abs() < 1e-2);
        assert!((e.full_throttle(e.omega_opt) - t_opt).abs() < 1e-2);
        assert!((e.full_throttle(e.omega_opt * 0.5) - 1.25 * t_opt).abs() < 1e-1);
        assert!(e.full_throttle(e.omega_max).abs() < 1e-2);
        let peak = e.power_at_rpm(5800.0);
        assert!(e.power_at_rpm(5000.0) < peak && e.power_at_rpm(6500.0) < peak);
        assert!(e.zero_throttle(e.omega_idle).abs() < 1e-3);
        assert!(e.zero_throttle(e.omega_opt) < 0.0);
    }

    #[test]
    fn the_beetle_box_matches_its_derived_ratios() {
        // 07 § Derived numbers: 22.8 in first, 5.07 in top.
        let ratios = gear_ratios(5800.0, 0.337, 30.0, 20.0, 90.0, 6, 0.5);
        assert_eq!(ratios.len(), 6);
        assert!(ratios[0] < 0.0 && ratios[1] == 0.0);
        assert!((ratios[2] - 22.8).abs() < 0.2, "{ratios:?}");
        assert!((ratios[5] - 5.07).abs() < 0.05, "{ratios:?}");
        assert!(ratios[2] > ratios[3] && ratios[3] > ratios[4] && ratios[4] > ratios[5]);
        let e = EngineConstants::of(&beetle_engine());
        let ShiftPoints {
            upshift: up,
            downshift_full: down_full,
            downshift_zero: down_zero,
            equal_power,
        } = shift_points(&ratios, &e, 8500.0, 0.05, 0.05, 0.3);
        for g in 2..5 {
            assert!(equal_power[g] > 5800.0 && equal_power[g] < up[g]);
            assert!(up[g] > 5800.0 && up[g] < 8500.0, "{up:?}");
            assert!(down_full[g + 1] < up[g] * ratios[g + 1] / ratios[g]);
            assert!(down_zero[g + 1] < down_full[g + 1]);
        }
        assert_eq!(up[5], 8500.0);
    }

    #[test]
    fn reset_displays_idle_while_the_cold_engine_spins_up() {
        let e = EngineConstants::of(&beetle_engine());
        let mut p = Powertrain::new(&e);
        assert_eq!(p.engine_omega, 0.0);
        assert!((p.rpm - 750.0).abs() < 1e-3);
        assert!(p.in_gear_change);
        let dt = 1.0 / 60.0;
        let expected_omega = dt * e.zero_throttle(0.0) / e.inertia;
        p.update_engine(&e, 1.0, 0.0, 1.0, dt);
        assert!((p.engine_omega - expected_omega).abs() < 1e-6);
        assert!((p.rpm - 750.0).abs() < 1e-3);
        assert_eq!(p.torque, 0.0);
        assert!(!p.clutch);
        for _ in 0..180 {
            p.update_engine(&e, 1.0, 0.0, 1.0, dt);
        }
        assert!(p.engine_omega > expected_omega && p.engine_omega < e.omega_idle);
    }

    #[test]
    fn a_launch_drops_the_clutch_at_twice_idle() {
        let e = EngineConstants::of(&beetle_engine());
        let mut p = Powertrain::new(&e);
        let gearbox = OriginalGearbox {
            ratios: gear_ratios(5800.0, 0.337, 30.0, 20.0, 90.0, 6, 0.5),
            upshift_rpm: vec![8500.0; 6],
            downshift_full_rpm: vec![0.0; 6],
            downshift_zero_rpm: vec![0.0; 6],
            equal_power_rpm: Vec::new(),
            gear_change_time: 0.8,
        };
        let ratio = gearbox.ratios[p.gear];
        let mut steps = 0;
        while !p.clutch && steps < 240 {
            p.update_engine(&e, ratio, 1.0, 1.0, 1.0 / 120.0);
            steps += 1;
        }
        assert!(p.clutch, "clutch never closed");
        assert!(p.engine_omega > 2.0 * e.omega_idle);
    }

    #[test]
    fn aero_damping_never_reverses_a_spin() {
        let aero = OriginalAero {
            ang_c_damp: [1.3, 1.4, 1.0],
            ang_vel_damp: [0.0; 3],
            ang_vel2_damp: [0.14, 2.0, 1.0],
            drag: 0.5,
            down: 0.0,
        };
        // 04: the Beetle's yaw at 1 rad/s loses 3.4 rad/s².
        let a = aero_damping(Vec3::new(0.0, 1.0, 0.0), &aero, 1.0 / 120.0);
        assert!((a.y + 3.4).abs() < 1e-4, "{a}");
        let slow = Vec3::new(0.0, 0.001, 0.0);
        let a = aero_damping(slow, &aero, 1.0 / 120.0);
        assert!(slow.y + a.y / 120.0 >= -1e-9);
    }
}
