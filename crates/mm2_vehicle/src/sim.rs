//! Pure vehicle-dynamics math — no ECS, no engine types.
//!
//! Chassis-space convention: +x right, +y up, **-z forward** (Bevy).
//! All SI units.

use crate::config::{EngineConfig, SteeringConfig, TireConfig, TransmissionConfig};

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

/// Engine torque (N·m) at `rpm`: idle→peak is a rising ramp, peak→redline
/// falls off toward `redline_torque_fraction`.
pub fn engine_torque(rpm: f32, cfg: &EngineConfig) -> f32 {
    if rpm <= cfg.idle_rpm {
        return cfg.peak_torque_nm * 0.8;
    }
    if rpm <= cfg.peak_torque_rpm {
        let t = (rpm - cfg.idle_rpm) / (cfg.peak_torque_rpm - cfg.idle_rpm).max(1.0);
        return cfg.peak_torque_nm * (0.8 + 0.2 * t);
    }
    if rpm <= cfg.redline_rpm {
        let t = (rpm - cfg.peak_torque_rpm) / (cfg.redline_rpm - cfg.peak_torque_rpm).max(1.0);
        return cfg.peak_torque_nm * (1.0 - (1.0 - cfg.redline_torque_fraction) * t);
    }
    0.0
}

/// Pick the gear for the current wheel speed, using RPM hysteresis.
pub fn select_gear(
    current_gear: usize,
    wheel_rps: f32,
    cfg: &TransmissionConfig,
    engine: &EngineConfig,
) -> usize {
    let n = cfg.gear_ratios.len().max(1);
    let mut gear = current_gear.min(n - 1);
    // Upshift if RPM exceeds ~92% of redline, downshift below ~35%.
    for _ in 0..n {
        let rpm = wheel_rps * cfg.gear_ratios[gear] * cfg.final_drive * 60.0;
        if rpm > engine.redline_rpm * 0.92 && gear + 1 < n {
            gear += 1;
        } else if rpm < engine.redline_rpm * 0.35 && gear > 0 {
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
        };
        assert_eq!(max_steer_angle(0.0, &cfg), 0.6);
        assert!((max_steer_angle(30.0, &cfg) - 0.1).abs() < 1e-6);
        assert!((max_steer_angle(15.0, &cfg) - 0.35).abs() < 1e-6);
        assert!((max_steer_angle(90.0, &cfg) - 0.1).abs() < 1e-6); // clamped
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
        };
        let eng = EngineConfig {
            idle_rpm: 900.0,
            redline_rpm: 6000.0,
            peak_torque_rpm: 4000.0,
            peak_torque_nm: 300.0,
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
}
