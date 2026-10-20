//! Shared fixtures: a hand-authored retail-shaped car (synthetic, not
//! retail data, so tests run in CI without `MM2_RETAIL`).

use mm2_vehicle::VehicleConfig;
use mm2_vehicle::config::{
    OriginalAero, OriginalAxle, OriginalEngine, OriginalGearbox, OriginalHandling, OriginalTrain,
    OriginalWheel,
};
use mm2_vehicle::original::{EngineConstants, gear_ratios, shift_points};

#[derive(Clone, Copy, Debug)]
pub enum Model {
    Generic,
    Original,
}

/// A hand-authored retail-shaped car (Beetle-like tokens): synthetic, not
/// retail data, so the test runs in CI without `MM2_RETAIL`.
pub fn config(model: Model) -> VehicleConfig {
    let mut cfg = VehicleConfig::default();
    if matches!(model, Model::Generic) {
        return cfg;
    }
    let gravity = 19.6;
    let engine = OriginalEngine {
        max_power_w: 260.0 * 746.0,
        idle_rpm: 750.0,
        opt_rpm: 5800.0,
        max_rpm: 8500.0,
        ang_inertia: 1.0,
        gear_change_lag: 0.25,
    };
    let constants = EngineConstants::of(&engine);
    let ratios = gear_ratios(engine.opt_rpm, 0.34, 30.0, 20.0, 90.0, 6, 0.5);
    let shifts = shift_points(&ratios, &constants, engine.max_rpm, 0.1, 0.1, 0.3);
    let wheel = |rear: bool, side: f32| OriginalWheel {
        static_load: cfg.mass * gravity / 4.0,
        rear,
        side,
        width: 0.25,
        suspension_extent: 0.15,
        suspension_limit: 0.05,
        suspension_factor: 1.0,
        suspension_damp_coef: 0.1,
        steering_limit: if rear { 0.0 } else { 0.4 },
        steering_offset: 0.25,
        brake_coef: 0.625,
        handbrake_coef: if rear { 1.0 } else { 0.0 },
        tire_disp_limit_lat: 0.125,
        tire_disp_limit_long: 0.125,
        tire_damp_coef_lat: 0.25,
        tire_damp_coef_long: 0.25,
        tire_drag_coef_lat: 0.05,
        tire_drag_coef_long: 0.02,
        optimum_slip: 0.16,
        static_fric: 3.0,
        sliding_fric: 2.7,
    };
    let train = OriginalTrain {
        ang_inertia: 5000.0,
        brake_dynamic_coef: 1.0,
        brake_static_coef: 1.2,
    };
    cfg.original = Some(OriginalHandling {
        bound_friction: 0.3,
        bound_elasticity: 0.2,
        gravity,
        surface_friction: 0.9,
        wheels: vec![
            wheel(false, -1.0),
            wheel(false, 1.0),
            wheel(true, -1.0),
            wheel(true, 1.0),
        ],
        driven: vec![2, 3],
        free: vec![0, 1],
        axles: vec![
            OriginalAxle {
                wheels: [0, 1],
                torque_coef: 20.0,
                damp_coef: 0.3,
            },
            OriginalAxle {
                wheels: [2, 3],
                torque_coef: 20.0,
                damp_coef: 0.3,
            },
        ],
        drivetrain: train,
        freetrain: train,
        engine,
        gearbox: OriginalGearbox {
            ratios,
            upshift_rpm: shifts.upshift,
            downshift_full_rpm: shifts.downshift_full,
            downshift_zero_rpm: shifts.downshift_zero,
            equal_power_rpm: shifts.equal_power,
            gear_change_time: 0.8,
        },
        aero: OriginalAero {
            ang_c_damp: [1.3, 1.4, 1.0],
            ang_vel_damp: [0.0; 3],
            ang_vel2_damp: [0.14, 2.0, 1.0],
            drag: 0.5,
            down: 0.0,
        },
        max_angular_speed: 4.0 * std::f32::consts::PI,
    });
    cfg.validate().expect("fixture config is valid");
    cfg
}
