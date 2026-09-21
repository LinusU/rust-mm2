//! ECS components describing a simulated vehicle.

use avian3d::prelude::Collider;
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
    /// Explicit gear-hold command: `Some(i)` pins the gearbox to gear `i`
    /// (clamped to the configured range) instead of running the automatic
    /// selector; `None` = automatic. A control command for AI, network
    /// input and dev tuning — device mappings leave it `None`.
    pub forced_gear: Option<usize>,
}

/// The vehicle entity marker + static configuration.
#[derive(Component)]
pub struct Vehicle {
    /// Handling definition.
    pub config: VehicleConfig,
}

/// The vehicle's unmodified authored bound as a convex-hull collider —
/// a prop-strike surface, **not** a world collider. It is stored on the
/// entity for shape-overlap queries only, so it can never touch roads,
/// bodies or the solver: the snag-avoidance reshaped chassis collider
/// remains the only world bound. `striker_points` on the config builds
/// it; without one it defaults to the chassis collider's shape.
#[derive(Component, Clone)]
pub struct StrikeBound(pub Collider);

/// Per-wheel runtime state (useful for debug drawing and tuning).
#[derive(Debug, Clone, Copy, Default)]
pub struct WheelState {
    /// Whether the wheel has ground contact.
    pub grounded: bool,
    /// World-space contact point.
    pub contact_point: Vec3,
    /// World-space contact normal.
    pub contact_normal: Vec3,
    /// The collider entity the wheel ray hit — lets telemetry resolve the
    /// surface under the wheel. `None` while airborne.
    pub contact_entity: Option<Entity>,
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
    /// Traction utilization: the fraction of the tire's longitudinal force
    /// limit the controller demanded this step (signed, roughly -1.5..1.5).
    ///
    /// This is **not** a measured wheel slip ratio — the arcade tire model
    /// has no wheel-speed state. Values beyond ±1 mean the demand exceeded
    /// what the tire could deliver.
    pub traction_demand: f32,
    /// Applied lateral force, N.
    pub lateral_force: f32,
    /// Applied longitudinal force, N.
    pub longitudinal_force: f32,
    /// The combined surface × environment grip multiplier the tire path
    /// applied to this wheel's force limits this step (F06-B): `1.0`
    /// is unmodified — airborne wheels and unmarked colliders report
    /// `1.0`, and the field is `0.0` only before the first physics step
    /// writes it.
    pub surface_grip: f32,
    /// The wading-resistance coefficient (`TireSurface::drag`) of the
    /// collider under this wheel: `0.0` is none — airborne wheels and
    /// unmarked colliders report `0.0`.
    pub surface_drag: f32,
    /// Accumulated spin for visuals, radians.
    pub spin: f32,
}

/// Which direction the drivetrain is engaged for.
///
/// Brake-to-reverse is a deliberate state transition, not a threshold
/// comparison each step: `Reverse` engages only when the car is nearly
/// stopped with the brake held and no throttle, and disengages on throttle
/// or when the brake is released. This keeps behaviour around a standstill
/// stable (no oscillation between brake and reverse force).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DriveDirection {
    /// Forward gears; brake pedal brakes.
    #[default]
    Forward,
    /// Reverse gear; brake pedal is the reverse throttle.
    Reverse,
}

/// Mutable simulation state of a vehicle.
#[derive(Component)]
pub struct VehicleState {
    /// Per-wheel state, parallel to `VehicleConfig::wheels`.
    pub wheels: Vec<WheelState>,
    /// Current actual steering angle at the front wheels, radians.
    pub steer_angle: f32,
    /// Engaged drivetrain direction (see [`DriveDirection`]).
    pub direction: DriveDirection,
    /// Current gear (index into `gear_ratios`).
    pub gear: usize,
    /// Estimated engine RPM.
    pub rpm: f32,
    /// Drive force delivered this step as a fraction of what the
    /// drivetrain could deliver at the current rpm/gear (0 coasting or
    /// airborne, ~1 at full demand; traction-control clamping lowers
    /// it — a gear change scales both sides of the ratio equally).
    pub engine_load: f32,
    /// Whether any wheel is grounded.
    pub grounded: bool,
    /// Speed along the vehicle forward axis, m/s (signed).
    pub forward_speed: f32,
    /// Whether a gear change is in progress.
    pub shifting: f32,
    /// How long the car has been lying on its side or roof and nearly
    /// still, seconds. Drives the self-righting assist.
    pub upended_for: f32,
}

impl VehicleState {
    /// Fresh state for `config`.
    pub fn new(config: &VehicleConfig) -> Self {
        Self {
            wheels: vec![WheelState::default(); config.wheels.len()],
            steer_angle: 0.0,
            direction: DriveDirection::Forward,
            gear: 0,
            rpm: config.engine.idle_rpm,
            engine_load: 0.0,
            grounded: false,
            forward_speed: 0.0,
            shifting: 0.0,
            upended_for: 0.0,
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

/// Marker [`vehicle_reset`](crate::systems::vehicle_reset) inserts on
/// each entity it teleports. Consumers that keep swept-segment state
/// derived from `Position` — the race runtime's per-participant segment
/// anchor is the current one — must re-anchor on this marker and remove
/// it, so the jump is never counted as motion. It persists until a
/// consumer clears it; a marker nobody claims is inert and despawns
/// with the entity.
#[derive(Component, Debug, Clone, Copy)]
pub struct Teleported;
