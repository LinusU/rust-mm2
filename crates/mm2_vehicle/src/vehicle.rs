//! ECS components describing a simulated vehicle.

use bevy::prelude::*;

use crate::config::VehicleConfig;

/// Normalized driver input. Physics systems read this — they never look at
/// keyboards or gamepads directly.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct VehicleInput {
    /// 0..1 throttle.
    pub throttle: f32,
    /// 0..1 brake (also reverse when nearly stopped).
    pub brake: f32,
    /// -1..1 steering (negative = left).
    pub steering: f32,
    /// 0..1 handbrake.
    pub handbrake: f32,
}

/// The vehicle entity marker + static configuration.
#[derive(Component)]
pub struct Vehicle {
    /// Handling definition.
    pub config: VehicleConfig,
}

/// Per-wheel runtime state (useful for debug drawing and tuning).
#[derive(Debug, Clone, Copy, Default)]
pub struct WheelState {
    /// Whether the wheel has ground contact.
    pub grounded: bool,
    /// World-space contact point.
    pub contact_point: Vec3,
    /// World-space contact normal.
    pub contact_normal: Vec3,
    /// Current compression of the suspension, metres.
    pub compression: f32,
    /// Last applied suspension (normal) force, N.
    pub suspension_force: f32,
    /// Longitudinal velocity at the contact patch, m/s.
    pub vel_long: f32,
    /// Lateral velocity at the contact patch, m/s.
    pub vel_lat: f32,
    /// Slip angle, radians.
    pub slip_angle: f32,
    /// Slip ratio estimate (0..1.5).
    pub slip_ratio: f32,
    /// Applied lateral force, N.
    pub lateral_force: f32,
    /// Applied longitudinal force, N.
    pub longitudinal_force: f32,
    /// Accumulated spin for visuals, radians.
    pub spin: f32,
}

/// Mutable simulation state of a vehicle.
#[derive(Component)]
pub struct VehicleState {
    /// Per-wheel state, parallel to `VehicleConfig::wheels`.
    pub wheels: Vec<WheelState>,
    /// Current actual steering angle at the front wheels, radians.
    pub steer_angle: f32,
    /// Current gear (index into `gear_ratios`).
    pub gear: usize,
    /// Estimated engine RPM.
    pub rpm: f32,
    /// Whether any wheel is grounded.
    pub grounded: bool,
    /// Speed along the vehicle forward axis, m/s (signed).
    pub forward_speed: f32,
    /// Whether a gear change is in progress.
    pub shifting: f32,
}

impl VehicleState {
    /// Fresh state for `config`.
    pub fn new(config: &VehicleConfig) -> Self {
        Self {
            wheels: vec![WheelState::default(); config.wheels.len()],
            steer_angle: 0.0,
            gear: 0,
            rpm: config.engine.idle_rpm,
            grounded: false,
            forward_speed: 0.0,
            shifting: 0.0,
        }
    }
}

/// Message requesting a vehicle reset to a pose.
#[derive(Message, Debug, Clone, Copy)]
pub struct ResetVehicle {
    /// Entity to reset; `None` resets every vehicle.
    pub entity: Option<Entity>,
    /// World position to respawn at.
    pub position: Vec3,
    /// Yaw angle, radians.
    pub yaw: f32,
}
