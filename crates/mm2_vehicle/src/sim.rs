//! Pure vehicle-dynamics math — no ECS, no engine types.
//!
//! Chassis-space convention: +x right, +y up, **-z forward** (Bevy).
//! All SI units.

use crate::config::{EngineConfig, MIN_STEER_LOCK, SteeringConfig, TireConfig, TransmissionConfig};

/// Shape a steering input symmetrically: `sign(x)·|x|^curve`.
/// `curve > 1` softens response around the centre.
pub fn steering_response(input: f32, curve: f32) -> f32 {
    input.signum() * input.abs().powf(curve)
}

/// Speed-sensitive steering lock: interpolates between the low-speed and
/// high-speed maxima.
pub fn max_steer_angle(speed: f32, cfg: &SteeringConfig) -> f32 {
    let t = (speed / cfg.high_speed.max(1.0)).clamp(0.0, 1.0);
    cfg.low_speed_max_angle + (cfg.high_speed_max_angle - cfg.low_speed_max_angle) * t
}

/// Largest steer angle worth commanding at `speed`, radians.
///
/// Two terms, straight out of the steady-state bicycle model:
///
/// * **Geometry.** A steer angle `d` turns a radius `wheelbase / d`, so it
///   demands `v² · d / wheelbase` of lateral acceleration. Inverting that
///   for `grip_limit` times the tires' limit gives a lock falling off as
///   `1/v²` — the shape real speed-sensitive steering has, derived per car
///   rather than authored.
/// * **Slip.** A tire makes force by slipping, so the front wheels must be
///   turned *past* the path the car is taking before they pull at all.
///   `slip_allowance` is how much further — the understeer term, the slip
///   the front tires need beyond what the rears are already giving.
///
/// Leaving the slip term out caps the wheels at the geometric angle, where
/// tires that need a lot of slip make almost none of their grip: MM2's
/// London Cab asks 0.40 rad of its fronts and 0.14 of its rears, and
/// without the allowance it will not turn at all above walking pace.
///
/// Returns [`MIN_STEER_LOCK`] at the very least, so the driver always has
/// something to steer with, and `f32::INFINITY` when the cap is disabled.
pub fn grip_limited_steer_angle(
    speed: f32,
    wheelbase: f32,
    lateral_grip: f32,
    grip_limit: f32,
    slip_allowance: f32,
) -> f32 {
    if grip_limit <= 0.0 {
        return f32::INFINITY;
    }
    let v_sq = speed * speed;
    if v_sq < 1e-3 {
        return f32::INFINITY;
    }
    let max_accel = lateral_grip * 9.81 * grip_limit;
    let geometric = max_accel * wheelbase.max(1e-3) / v_sq;
    (geometric + slip_allowance.max(0.0)).max(MIN_STEER_LOCK)
}

/// Advance the actual steering angle toward `target` at the configured rates.
pub fn step_steer_angle(current: f32, target: f32, cfg: &SteeringConfig, dt: f32) -> f32 {
    // Moving away from centre uses input_rate; returning uses return_rate.
    let rate = if target.abs() > current.abs() {
        cfg.input_rate
    } else {
        cfg.return_rate
    };
    let delta = (target - current).clamp(-rate * dt, rate * dt);
    current + delta
}

/// Torque (N·m) at the rated power peak (`peak_power_rpm`), accounting for
/// the optional power anchor.
fn power_peak_torque(cfg: &EngineConfig) -> f32 {
    match (cfg.peak_power_rpm, cfg.max_power_w) {
        (Some(pp), Some(pw)) => {
            let omega = pp * std::f32::consts::TAU / 60.0;
            if omega > 0.0 {
                pw / omega
            } else {
                cfg.peak_torque_nm
            }
        }
        _ => cfg.peak_torque_nm,
    }
}

/// Engine torque (N·m) at `rpm`.
///
/// Curve anchors: 80% of peak torque at idle, `peak_torque_nm` at
/// `peak_torque_rpm`, the torque implied by `max_power_w` at
/// `peak_power_rpm` (falling back to `peak_torque_nm` when no power anchor
/// is set), then a decay to `redline_torque_fraction` at the redline.
pub fn engine_torque(rpm: f32, cfg: &EngineConfig) -> f32 {
    if rpm <= cfg.idle_rpm {
        return cfg.peak_torque_nm * 0.8;
    }
    if rpm <= cfg.peak_torque_rpm {
        let t = (rpm - cfg.idle_rpm) / (cfg.peak_torque_rpm - cfg.idle_rpm).max(1.0);
        return cfg.peak_torque_nm * (0.8 + 0.2 * t);
    }
    let pp_rpm = cfg.peak_power_rpm.unwrap_or(cfg.peak_torque_rpm);
    let t_opt = power_peak_torque(cfg);
    if rpm <= pp_rpm {
        let t = (rpm - cfg.peak_torque_rpm) / (pp_rpm - cfg.peak_torque_rpm).max(1.0);
        return cfg.peak_torque_nm + (t_opt - cfg.peak_torque_nm) * t;
    }
    if rpm <= cfg.redline_rpm {
        let t = (rpm - pp_rpm) / (cfg.redline_rpm - pp_rpm).max(1.0);
        return t_opt * (1.0 - (1.0 - cfg.redline_torque_fraction) * t);
    }
    0.0
}

/// Power (watts) the engine delivers at `rpm`.
pub fn engine_power(rpm: f32, cfg: &EngineConfig) -> f32 {
    engine_torque(rpm, cfg) * rpm * std::f32::consts::TAU / 60.0
}

/// Pick the gear for the current wheel speed, using RPM hysteresis.
///
/// Upshift at `upshift_rpm` (default 92% of redline), downshift at
/// `downshift_rpm` (default 35% of redline).
pub fn select_gear(
    current_gear: usize,
    wheel_rps: f32,
    cfg: &TransmissionConfig,
    engine: &EngineConfig,
) -> usize {
    let n = cfg.gear_ratios.len().max(1);
    let mut gear = current_gear.min(n - 1);
    let upshift = cfg.upshift_rpm.unwrap_or(engine.redline_rpm * 0.92);
    let downshift = cfg.downshift_rpm.unwrap_or(engine.redline_rpm * 0.35);
    for _ in 0..n {
        let rpm = wheel_rps * cfg.gear_ratios[gear] * cfg.final_drive * 60.0;
        if rpm > upshift && gear + 1 < n {
            gear += 1;
        } else if rpm < downshift && gear > 0 {
            gear -= 1;
        } else {
            break;
        }
    }
    gear
}

/// Engine RPM implied by wheel speed and current gear.
pub fn engine_rpm(
    wheel_rps: f32,
    gear: usize,
    cfg: &TransmissionConfig,
    engine: &EngineConfig,
) -> f32 {
    let ratio = cfg.gear_ratios.get(gear).copied().unwrap_or(1.0);
    (wheel_rps * ratio * cfg.final_drive * 60.0).max(engine.idle_rpm)
}

/// Slip angle in radians from longitudinal/lateral contact velocity.
/// Positive when the contact point is moving toward the tire's right;
/// tire forces must oppose it (push along `-sign(slip)` on the right axis).
pub fn slip_angle(vel_long: f32, vel_lat: f32) -> f32 {
    if vel_long.abs() < 0.5 {
        // At very low speed slip angle is meaningless — clamp lateral slip.
        return vel_lat.atan2(0.5);
    }
    vel_lat.atan2(vel_long.abs())
}

/// Simplified lateral force curve: linear rise to the peak at
/// `peak_slip_angle`, then a falloff plateau at `slide_fraction` of peak.
/// Returns force in Newtons opposing the slip direction.
pub fn lateral_force(slip_angle: f32, normal_load: f32, cfg: &TireConfig) -> f32 {
    let peak = cfg.lateral_grip * normal_load;
    let a = slip_angle.abs();
    let shape = if a <= cfg.peak_slip_angle {
        a / cfg.peak_slip_angle.max(1e-4)
    } else {
        // Fall off toward slide_fraction at ~3× peak slip.
        let t = ((a - cfg.peak_slip_angle) / (cfg.peak_slip_angle * 2.0)).min(1.0);
        1.0 - (1.0 - cfg.slide_fraction) * t
    };
    -slip_angle.signum() * peak * shape
}

/// Arcade longitudinal traction model.
///
/// `requested` is the force the controller (engine, brakes, resistance)
/// demands; `normal_load * longitudinal_grip * traction_limit` is the most
/// the tire can deliver. Inside the limit the demand is delivered verbatim.
/// Demand beyond the limit behaves like over-driving the tire: delivered
/// force falls from the peak toward `slide_fraction` of it across an extra
/// `peak_slip_ratio` of over-demand — a deliberately simple stand-in for a
/// slip curve, not a measured wheel-speed model.
///
/// Returns `(applied_force, demand_ratio)` where `demand_ratio` is
/// `requested / limit` — a force-utilization figure (clamped to ±1.5 for
/// reporting), **not** measured slip.
pub fn longitudinal_force(
    requested: f32,
    normal_load: f32,
    cfg: &TireConfig,
    traction_limit: f32,
) -> (f32, f32) {
    let max_f = cfg.longitudinal_grip * normal_load * traction_limit;
    if max_f <= 1e-3 {
        return (0.0, 0.0);
    }
    let demand = requested / max_f;
    let over = (demand.abs() - 1.0).max(0.0);
    let falloff = ((over / cfg.peak_slip_ratio.max(1e-3)).min(1.0)) * (1.0 - cfg.slide_fraction);
    let applied = demand.signum() * max_f * (demand.abs().min(1.0) - falloff);
    (applied, demand.clamp(-1.5, 1.5))
}

/// Grip available for one wheel accounting for load sensitivity: grip per
/// unit load falls off as load rises above `reference`.
pub fn load_adjusted_grip(normal_load: f32, reference_load: f32, cfg: &TireConfig) -> f32 {
    if normal_load <= 0.0 {
        return 0.0;
    }
    let ratio = normal_load / reference_load.max(1.0);
    normal_load * ratio.powf(-cfg.load_sensitivity)
}

/// Rolling resistance opposing wheel motion.
pub fn rolling_resistance(vel_long: f32, normal_load: f32, cfg: &TireConfig) -> f32 {
    if vel_long.abs() < 0.05 {
        return 0.0;
    }
    -vel_long.signum() * cfg.rolling_resistance * normal_load
}

/// Aerodynamic drag force along the velocity direction.
pub fn drag_force(speed_sq: f32, forward_speed_sign: f32, drag_coefficient: f32) -> f32 {
    -forward_speed_sign * drag_coefficient * speed_sq
}

/// Yaw-stability damping factor (0, 1]: how strongly the yaw damper is
/// allowed to pull the car back to its velocity heading this step.
///
/// `body_slip` (rad) and `handbrake` (0..1) both loosen it — a sliding
/// or handbraking car is allowed to rotate. `drift` (the authored
/// `vehGyro` value, clamped 0..1) relieves the *slip* term: the
/// Driftable gate lets a car the record says drifts hold its slide
/// instead of being straightened out — `drift = 1` never straightens a
/// slide at all, `drift = 0` is the unmodified policy exactly.
pub fn yaw_damp_factor(body_slip: f32, handbrake: f32, drift: f32) -> f32 {
    1.0 / (1.0
        + body_slip.abs() * 8.0 * (1.0 - drift.clamp(0.0, 1.0))
        + handbrake.clamp(0.0, 1.0) * 4.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steering_response_is_symmetric() {
        assert_eq!(steering_response(-0.5, 2.0), -steering_response(0.5, 2.0));
        assert_eq!(steering_response(1.0, 2.0), 1.0);
        assert_eq!(steering_response(0.0, 2.0), 0.0);
        assert!(steering_response(0.5, 2.0) < 0.5);
    }

    #[test]
    fn max_angle_drops_with_speed() {
        let cfg = SteeringConfig {
            low_speed_max_angle: 0.6,
            high_speed_max_angle: 0.1,
            high_speed: 30.0,
            input_rate: 3.0,
            return_rate: 6.0,
            response_curve: 1.0,
            grip_limit: 0.0,
        };
        assert_eq!(max_steer_angle(0.0, &cfg), 0.6);
        assert!((max_steer_angle(30.0, &cfg) - 0.1).abs() < 1e-6);
        assert!((max_steer_angle(15.0, &cfg) - 0.35).abs() < 1e-6);
        assert!((max_steer_angle(90.0, &cfg) - 0.1).abs() < 1e-6); // clamped
    }

    #[test]
    fn grip_limited_lock_falls_off_with_the_square_of_speed() {
        // A car on 1.6 g tires, allowed to ask for 1.25x that, whose front
        // and rear tires want the same slip (no understeer term).
        let lock = |v| grip_limited_steer_angle(v, 2.6, 1.6, 1.25, 0.0);

        // Doubling the speed quarters the angle.
        let slow = lock(15.0);
        let fast = lock(30.0);
        assert!(
            (slow / fast - 4.0).abs() < 1e-3,
            "expected a 4x drop, got {}",
            slow / fast
        );

        // The cap is what it claims: at that angle the bicycle model
        // demands exactly the allowed multiple of the tires' limit.
        let demand = 15.0 * 15.0 * slow / 2.6 / 9.81;
        assert!(
            (demand - 1.6 * 1.25).abs() < 1e-3,
            "demand at the cap was {demand} g"
        );

        // Authority is never taken away entirely...
        assert!(lock(200.0) >= MIN_STEER_LOCK);
        // ...and at a standstill there is nothing to cap.
        assert!(lock(0.0).is_infinite());
        // Disabled means disabled.
        assert!(grip_limited_steer_angle(30.0, 2.6, 1.6, 0.0, 0.0).is_infinite());
    }

    #[test]
    fn a_grippier_car_is_allowed_more_lock() {
        let slippery = grip_limited_steer_angle(25.0, 2.6, 0.8, 1.25, 0.0);
        let grippy = grip_limited_steer_angle(25.0, 2.6, 1.6, 1.25, 0.0);
        assert!(grippy > slippery);
    }

    #[test]
    fn lazy_front_tires_are_allowed_the_slip_they_need() {
        // The London Cab's shape: fronts wanting 0.40 rad of slip against
        // rears wanting 0.14. Without the allowance the geometric cap
        // alone leaves the fronts far short of making any force.
        let allowance = 0.40 - 0.14;
        let geometric = grip_limited_steer_angle(25.0, 2.6, 1.4, 1.25, 0.0);
        let with_slip = grip_limited_steer_angle(25.0, 2.6, 1.4, 1.25, allowance);
        assert!(
            (with_slip - geometric - allowance).abs() < 1e-5,
            "allowance should add on top of the geometric angle"
        );
        // A car whose axles want the same slip is unaffected.
        assert_eq!(
            grip_limited_steer_angle(25.0, 2.6, 1.4, 1.25, 0.0),
            geometric
        );
        // An oversteering balance never *removes* authority.
        assert_eq!(
            grip_limited_steer_angle(25.0, 2.6, 1.4, 1.25, -0.2),
            geometric
        );
    }

    #[test]
    fn steer_steps_toward_target() {
        let cfg = SteeringConfig {
            low_speed_max_angle: 0.6,
            high_speed_max_angle: 0.1,
            high_speed: 30.0,
            input_rate: 2.0,
            return_rate: 4.0,
            response_curve: 1.0,
            grip_limit: 0.0,
        };
        let a = step_steer_angle(0.0, 0.5, &cfg, 0.1);
        assert!((a - 0.2).abs() < 1e-6); // rate-limited
        let b = step_steer_angle(0.5, 0.0, &cfg, 0.1);
        assert!((b - 0.1).abs() < 1e-6); // faster return
    }

    #[test]
    fn engine_torque_curve_shape() {
        let cfg = EngineConfig {
            idle_rpm: 900.0,
            redline_rpm: 7000.0,
            peak_torque_rpm: 4000.0,
            peak_torque_nm: 300.0,
            peak_power_rpm: None,
            max_power_w: None,
            redline_torque_fraction: 0.7,
            rpm_response: 8.0,
            engine_brake_nm: 60.0,
        };
        assert_eq!(engine_torque(4000.0, &cfg), 300.0);
        assert!(engine_torque(7000.0, &cfg) < 300.0);
        assert_eq!(engine_torque(9000.0, &cfg), 0.0); // past redline
        assert!(engine_torque(900.0, &cfg) > 0.0);
    }

    #[test]
    fn gear_selection_shifts_up_and_down() {
        let tx = TransmissionConfig {
            gear_ratios: vec![3.0, 2.0, 1.0],
            reverse_ratio: 3.0,
            final_drive: 3.0,
            shift_time: 0.2,
            efficiency: 0.85,
            upshift_rpm: None,
            downshift_rpm: None,
        };
        let eng = EngineConfig {
            idle_rpm: 900.0,
            redline_rpm: 6000.0,
            peak_torque_rpm: 4000.0,
            peak_torque_nm: 300.0,
            peak_power_rpm: None,
            max_power_w: None,
            redline_torque_fraction: 0.7,
            rpm_response: 8.0,
            engine_brake_nm: 60.0,
        };
        // Very slow → gear 0.
        assert_eq!(select_gear(0, 1.0, &tx, &eng), 0);
        // Fast wheel speed → higher gear.
        assert!(select_gear(0, 20.0, &tx, &eng) > 0);
    }

    #[test]
    fn lateral_force_opposes_slip_and_saturates() {
        let cfg = TireConfig {
            lateral_grip: 1.5,
            longitudinal_grip: 1.6,
            peak_slip_angle: 0.15,
            peak_slip_ratio: 0.2,
            slide_fraction: 0.7,
            rolling_resistance: 0.01,
            load_sensitivity: 0.3,
        };
        let f_small = lateral_force(0.05, 3000.0, &cfg);
        let f_peak = lateral_force(0.15, 3000.0, &cfg);
        let f_slide = lateral_force(0.6, 3000.0, &cfg);
        assert!(f_small < 0.0); // opposes positive slip
        assert!(f_peak.abs() > f_small.abs());
        assert!(f_slide.abs() < f_peak.abs()); // falls off past peak
        assert!(f_slide.abs() > f_peak.abs() * 0.6); // but retains slide grip
    }

    #[test]
    fn longitudinal_force_limits_and_falls_off() {
        let cfg = TireConfig {
            lateral_grip: 1.5,
            longitudinal_grip: 1.5,
            peak_slip_angle: 0.15,
            peak_slip_ratio: 0.2,
            slide_fraction: 0.7,
            rolling_resistance: 0.01,
            load_sensitivity: 0.3,
        };
        // Inside the limit, demand is delivered verbatim, in both directions.
        let (f, d) = longitudinal_force(2000.0, 3000.0, &cfg, 1.0);
        assert_eq!(f, 2000.0);
        assert!(d < 1.0);
        let (f, _) = longitudinal_force(-2000.0, 3000.0, &cfg, 1.0);
        assert_eq!(f, -2000.0);
        // Slightly past the limit the force is near the peak and falling:
        // demand 1.1 → 1 - (0.1/0.2)*(1-0.7) = 0.85 of the limit.
        let (f, d) = longitudinal_force(4950.0, 3000.0, &cfg, 1.0);
        assert!((f - 4500.0 * 0.85).abs() < 1.0);
        assert!(d > 1.0);
        // Extreme over-demand slides toward slide_fraction of the limit.
        let (f, _) = longitudinal_force(100_000.0, 3000.0, &cfg, 1.0);
        assert!((f - 4500.0 * 0.7).abs() < 1.0);
        // And symmetric in reverse.
        let (f, _) = longitudinal_force(-100_000.0, 3000.0, &cfg, 1.0);
        assert!((f + 4500.0 * 0.7).abs() < 1.0);
        // No load, no force.
        assert_eq!(longitudinal_force(10_000.0, 0.0, &cfg, 1.0), (0.0, 0.0));
    }

    #[test]
    fn slip_angle_sign() {
        assert!(slip_angle(10.0, 2.0) > 0.0); // sliding right → positive
        assert!(slip_angle(10.0, -2.0) < 0.0);
        assert!(slip_angle(0.0, 0.0).is_finite());
    }

    #[test]
    fn yaw_damp_loosens_with_slip_handbrake_and_drift() {
        // No slip, no handbrake: full damping regardless of drift.
        assert_eq!(yaw_damp_factor(0.0, 0.0, 0.9), 1.0);
        // drift = 0 is the unmodified policy.
        assert_eq!(yaw_damp_factor(0.0, 1.0, 0.0), 0.2);
        assert_eq!(yaw_damp_factor(0.5, 0.0, 0.0), 0.2);
        // Drift relieves only the slip term, monotone toward free; the
        // handbrake term is untouched.
        let none = yaw_damp_factor(0.5, 0.0, 0.0);
        let half = yaw_damp_factor(0.5, 0.0, 0.5);
        let full = yaw_damp_factor(0.5, 0.0, 1.0);
        assert!(none < half && half < full);
        assert_eq!(full, 1.0);
        assert_eq!(
            yaw_damp_factor(0.0, 1.0, 0.0),
            yaw_damp_factor(0.0, 1.0, 1.0)
        );
        // Out-of-range authored values clamp rather than overshoot.
        assert_eq!(yaw_damp_factor(0.5, 0.0, 2.0), full);
        assert_eq!(yaw_damp_factor(0.5, 0.0, -1.0), none);
    }
}
