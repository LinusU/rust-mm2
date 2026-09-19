//! Tunable vehicle definition. Everything that affects handling lives here —
//! no magic constants in systems.
//!
//! Units: metres, kilograms, seconds, radians, newtons, metres/second.

use serde::{Deserialize, Serialize};

/// Where a wheel sits and what it does.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WheelConfig {
    /// Suspension hardpoint in chassis space (metres; +x right, +y up, -z forward
    /// in Bevy convention).
    pub position: [f32; 3],
    /// Wheel radius in metres.
    pub radius: f32,
    /// Whether the engine drives this wheel.
    pub driven: bool,
    /// Whether the steering input turns this wheel.
    pub steered: bool,
    /// Fraction of braking force this wheel gets for the foot brake.
    pub brake_bias: f32,
    /// Whether the handbrake locks this wheel.
    pub handbrake: bool,
}

/// Spring/damper suspension parameters.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SuspensionConfig {
    /// Spring rate, N/m.
    pub spring_rate: f32,
    /// Damper rate while compressing, N·s/m.
    pub damping_compression: f32,
    /// Damper rate while rebounding, N·s/m.
    pub damping_rebound: f32,
    /// Length of the suspension travel below the hardpoint (rest length +
    /// travel) in metres.
    pub travel: f32,
    /// Suspension force applied at this fraction of wheel radius above the
    /// contact point, keeping roll plausible.
    pub force_apply_offset: f32,
    /// Maximum suspension force (prevents explosions on huge compressions).
    pub max_force: f32,
}

/// Engine model: a simple RPM/torque curve.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    /// Idle RPM.
    pub idle_rpm: f32,
    /// Redline RPM.
    pub redline_rpm: f32,
    /// RPM where peak torque occurs.
    pub peak_torque_rpm: f32,
    /// Peak torque in N·m.
    pub peak_torque_nm: f32,
    /// Torque at redline as a fraction of peak torque (curve falloff).
    pub redline_torque_fraction: f32,
    /// How fast RPM follows the load demand (1/s smoothing).
    pub rpm_response: f32,
    /// Engine braking torque at zero throttle, N·m.
    pub engine_brake_nm: f32,
}

/// Gearing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransmissionConfig {
    /// Forward gear ratios, low to high.
    pub gear_ratios: Vec<f32>,
    /// Reverse gear ratio (positive number).
    pub reverse_ratio: f32,
    /// Final drive ratio.
    pub final_drive: f32,
    /// Time between gears, seconds.
    pub shift_time: f32,
    /// Overall driveline efficiency (0..1).
    pub efficiency: f32,
}

/// Tire behaviour.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TireConfig {
    /// Peak lateral friction coefficient (grip × normal load).
    pub lateral_grip: f32,
    /// Peak longitudinal friction coefficient.
    pub longitudinal_grip: f32,
    /// Slip angle (radians) at which lateral force peaks.
    pub peak_slip_angle: f32,
    /// Longitudinal slip ratio at which force peaks.
    pub peak_slip_ratio: f32,
    /// Fraction of peak force retained at large slip (slide grip).
    pub slide_fraction: f32,
    /// Rolling resistance coefficient (force = c · normal load).
    pub rolling_resistance: f32,
    /// Load sensitivity: how much grip falls off with load (0 = linear).
    pub load_sensitivity: f32,
}

/// Steering feel.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SteeringConfig {
    /// Maximum wheel angle at low speed, radians.
    pub low_speed_max_angle: f32,
    /// Maximum wheel angle at/above `high_speed`, radians.
    pub high_speed_max_angle: f32,
    /// Speed at which the high-speed limit fully applies, m/s.
    pub high_speed: f32,
    /// Rate at which steering angle approaches the target, rad/s.
    pub input_rate: f32,
    /// Rate at which steering returns to centre with no input, rad/s.
    pub return_rate: f32,
    /// Exponent shaping input response (>1 = softer centre feel).
    pub response_curve: f32,
}

/// Brakes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BrakeConfig {
    /// Maximum brake force per wheel before tire limit, N.
    pub max_brake_force: f32,
    /// Handbrake force multiplier vs. foot brake.
    pub handbrake_strength: f32,
}

/// Aerodynamics and rolling losses.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AeroConfig {
    /// Drag coefficient × frontal area, kg/m (F = -c·v²).
    pub drag_coefficient: f32,
    /// Downforce coefficient, kg/m (F = -c·v², applied downward).
    pub downforce_coefficient: f32,
}

/// Arcade driving assists — intentional design, all tunable.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AssistConfig {
    /// Yaw damping strength: bleeds angular velocity toward the velocity
    /// heading (0 = off).
    pub yaw_stability: f32,
    /// Traction control: max allowed slip ratio under power (0 = off).
    pub traction_control: f32,
    /// Countersteer assistance: extra steering authority correcting yaw
    /// error during slides (0 = off).
    pub countersteer: f32,
    /// Air control: torque authority to level the car while airborne
    /// (0 = off).
    pub air_control: f32,
}

/// The complete vehicle definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VehicleConfig {
    /// Human-readable name.
    pub name: String,
    /// Chassis mass, kg.
    pub mass: f32,
    /// Centre of mass offset from the body origin, metres.
    pub center_of_mass: [f32; 3],
    /// Distance front↔rear axle, metres.
    pub wheelbase: f32,
    /// Distance left↔right wheels, metres.
    pub track_width: f32,
    /// Chassis collider full extents (x, y, z), metres.
    pub chassis_size: [f32; 3],
    /// Wheels (typically four).
    pub wheels: Vec<WheelConfig>,
    /// Suspension.
    pub suspension: SuspensionConfig,
    /// Engine.
    pub engine: EngineConfig,
    /// Transmission.
    pub transmission: TransmissionConfig,
    /// Tires.
    pub tires: TireConfig,
    /// Steering.
    pub steering: SteeringConfig,
    /// Brakes.
    pub brakes: BrakeConfig,
    /// Aerodynamics.
    pub aero: AeroConfig,
    /// Assists.
    pub assists: AssistConfig,
}

impl Default for VehicleConfig {
    /// A fun, grippy arcade setup — deliberately more Burnout than simulator.
    fn default() -> Self {
        let track = 1.7;
        let wheelbase = 2.7;
        let radius = 0.34;
        let wheel = |x: f32, z: f32, driven: bool, steered: bool, handbrake: bool| WheelConfig {
            position: [x, -0.15, z],
            radius,
            driven,
            steered,
            brake_bias: 0.25,
            handbrake,
        };
        Self {
            name: "Dev Car".to_string(),
            mass: 1300.0,
            center_of_mass: [0.0, -0.25, 0.0],
            wheelbase,
            track_width: track,
            chassis_size: [1.85, 0.55, 4.4],
            wheels: vec![
                wheel(-track / 2.0, -wheelbase / 2.0, false, true, false), // FL
                wheel(track / 2.0, -wheelbase / 2.0, false, true, false),  // FR
                wheel(-track / 2.0, wheelbase / 2.0, true, false, true),   // RL
                wheel(track / 2.0, wheelbase / 2.0, true, false, true),    // RR
            ],
            suspension: SuspensionConfig {
                spring_rate: 55_000.0,
                damping_compression: 4_500.0,
                damping_rebound: 5_500.0,
                travel: 0.35,
                force_apply_offset: 0.3,
                max_force: 40_000.0,
            },
            engine: EngineConfig {
                idle_rpm: 900.0,
                redline_rpm: 7_200.0,
                peak_torque_rpm: 4_200.0,
                peak_torque_nm: 320.0,
                redline_torque_fraction: 0.75,
                rpm_response: 8.0,
                engine_brake_nm: 60.0,
            },
            transmission: TransmissionConfig {
                gear_ratios: vec![3.6, 2.4, 1.7, 1.3, 1.0, 0.82],
                reverse_ratio: 3.2,
                final_drive: 3.9,
                shift_time: 0.25,
                efficiency: 0.85,
            },
            tires: TireConfig {
                lateral_grip: 1.6,
                longitudinal_grip: 1.7,
                peak_slip_angle: 0.16,
                peak_slip_ratio: 0.18,
                slide_fraction: 0.75,
                rolling_resistance: 0.012,
                load_sensitivity: 0.35,
            },
            steering: SteeringConfig {
                low_speed_max_angle: 0.62,
                high_speed_max_angle: 0.12,
                high_speed: 35.0,
                input_rate: 3.5,
                return_rate: 6.0,
                response_curve: 1.6,
            },
            brakes: BrakeConfig {
                max_brake_force: 9_000.0,
                handbrake_strength: 0.7,
            },
            aero: AeroConfig {
                drag_coefficient: 0.42,
                downforce_coefficient: 0.35,
            },
            assists: AssistConfig {
                yaw_stability: 2.5,
                traction_control: 0.9,
                countersteer: 0.35,
                air_control: 4.0,
            },
        }
    }
}
