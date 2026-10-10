//! Tunable vehicle definition. Everything that affects handling lives here —
//! no magic constants in systems.
//!
//! Units: metres, kilograms, seconds, radians, newtons, metres/second.

use std::path::Path;

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
    /// Steering angle scale for a steered wheel relative to the front-axle
    /// maximum (`1.0` = full lock). Used for rear axles with reduced lock
    /// and axles with deliberate rear steer.
    #[serde(default = "default_steer_scale")]
    pub steer_scale: f32,
    /// Fraction of braking force this wheel gets for the foot brake.
    pub brake_bias: f32,
    /// Whether the handbrake locks this wheel.
    pub handbrake: bool,
    /// Per-wheel handbrake force coefficient: when set, handbrake force is
    /// `input · max_brake_force · handbrake_coef` for this wheel. When absent
    /// the global `brakes.handbrake_strength` multiplier applies.
    #[serde(default)]
    pub handbrake_coef: Option<f32>,
    /// Fraction of total drive torque delivered to this wheel when driven.
    /// When `None` on every wheel, torque splits evenly across driven
    /// wheels. When set on any wheel, the shares are normalised to sum to 1.
    #[serde(default)]
    pub drive_share: Option<f32>,
    /// Per-wheel suspension override (front/rear or per-axle tuning).
    #[serde(default)]
    pub suspension: Option<SuspensionConfig>,
    /// Per-wheel tire override.
    #[serde(default)]
    pub tires: Option<TireConfig>,
}

fn default_steer_scale() -> f32 {
    1.0
}

fn default_collider_friction() -> f32 {
    0.5
}

fn default_roll_resistance() -> f32 {
    0.8
}

fn default_self_right_delay() -> f32 {
    2.0
}

fn default_steering_grip_limit() -> f32 {
    1.3
}

/// Steering authority never caps below this angle (radians): a driver must
/// keep something to steer with, however fast the car is going.
pub const MIN_STEER_LOCK: f32 = 0.04;

/// Fraction of drive torque still delivered at the start of a gear change.
///
/// Cutting drive entirely for `shift_time` is what a real clutch does, but
/// MM2 authors change times of 0.8-0.9 s, and five of those on the way to
/// top speed turn acceleration into a staircase — the car stops pulling,
/// pitches forward on its springs and loses speed before the next gear
/// bites. Torque dips to this fraction and ramps back instead: the shift
/// is still felt, and the car never stops accelerating.
pub const SHIFT_TORQUE_FLOOR: f32 = 0.25;

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
///
/// The curve is anchored on both a torque peak and a power peak. Between
/// `peak_torque_rpm` and `peak_power_rpm` torque interpolates from
/// `peak_torque_nm` toward the value that yields `max_power_w` at
/// `peak_power_rpm` (`torque = power / angular velocity`); past the power
/// peak it decays toward `redline_torque_fraction` at the redline. When the
/// optional power fields are absent the torque peak doubles as the power
/// peak (legacy shape).
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
    /// RPM where rated (peak) power occurs.
    #[serde(default)]
    pub peak_power_rpm: Option<f32>,
    /// Rated power in watts at `peak_power_rpm`.
    #[serde(default)]
    pub max_power_w: Option<f32>,
    /// Torque at redline as a fraction of the power-peak torque (curve
    /// falloff). With `max_power_w` set the falloff is relative to the
    /// torque at `peak_power_rpm`.
    pub redline_torque_fraction: f32,
    /// How fast RPM follows the load demand (1/s smoothing).
    pub rpm_response: f32,
    /// Engine braking torque at zero throttle, N·m.
    pub engine_brake_nm: f32,
}

/// Gearing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransmissionConfig {
    /// Forward gear ratios, low to high. Ratios are engine-speed to
    /// wheel-speed (`gear_ratio · final_drive` combined when `final_drive`
    /// is not 1).
    pub gear_ratios: Vec<f32>,
    /// Reverse gear ratio (positive number).
    pub reverse_ratio: f32,
    /// Final drive ratio. `1.0` when `gear_ratios` already include it.
    pub final_drive: f32,
    /// Time between gears, seconds. Drive torque dips to
    /// [`SHIFT_TORQUE_FLOOR`] and ramps back over this long, rather than
    /// switching off — see the comment on that constant.
    pub shift_time: f32,
    /// Overall driveline efficiency (0..1).
    pub efficiency: f32,
    /// RPM at which the automatic upshifts. Defaults to 92% of redline.
    #[serde(default)]
    pub upshift_rpm: Option<f32>,
    /// RPM at which the automatic downshifts. Defaults to 35% of redline.
    #[serde(default)]
    pub downshift_rpm: Option<f32>,
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
    /// Multiple of the tires' lateral limit the steering lock may ask for
    /// (`0` = no cap).
    ///
    /// A steer angle that demands far more cornering force than the tires
    /// can make does not corner harder — it just ploughs. Stock MM2 locks
    /// ask for 8-64 g at their high-speed limit on tires good for 1.7, so
    /// at any real speed full lock is pure understeer with no feedback.
    /// Capping the angle by what the tires can deliver gives a
    /// speed-sensitive lock that is derived per car rather than authored,
    /// and leaves the front tires with grip to spare for a correction.
    /// Slightly above `1.0` still lets the player provoke a slide.
    #[serde(default = "default_steering_grip_limit")]
    pub grip_limit: f32,
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
    /// How much of the roll moment from lateral tire force is cancelled,
    /// `0.0`–`1.0`.
    ///
    /// A tire's grip acts at the contact patch, so it pushes on the car a
    /// full centre-of-mass height below the mass it is turning — that lever
    /// is what stands a car on its door. Real suspension resists it
    /// geometrically through the roll centre; this is the arcade version of
    /// the same idea, raising the point at which lateral force is applied
    /// toward the centre of mass. `0.0` applies force at the patch (honest,
    /// and tips), `1.0` applies it level with the centre of mass (no roll
    /// moment at all). Around `0.8` keeps visible body roll while making a
    /// hard corner slide instead of trip.
    #[serde(default = "default_roll_resistance")]
    pub roll_resistance: f32,
    /// How much of the pitch moment from longitudinal tire force is
    /// cancelled, `0.0`–`1.0` — [`roll_resistance`](Self::roll_resistance)
    /// for squat and dive.
    ///
    /// Drive and brake force act at the contact patch too, and the lever
    /// they pitch the car on is the centre-of-mass height against the
    /// wheelbase. On a car of ordinary proportions that is squat and dive
    /// worth keeping, so `0.0` (force at the patch) is the default. A car
    /// carrying its mass high over a very short wheelbase lifts its front
    /// wheels under its own drive instead, and this raises the point the
    /// force is applied toward the centre of mass — the arcade form of
    /// anti-squat geometry. `1.0` would leave no pitch moment at all.
    #[serde(default)]
    pub pitch_resistance: f32,
    /// Yaw damping strength: bleeds angular velocity toward the velocity
    /// heading (0 = off).
    pub yaw_stability: f32,
    /// Traction control: the share of a driven tire's grip the drive
    /// may use (0 = off) — of what the car's hardest-cornering tire has
    /// left once it is cornering, so throttle held through a corner
    /// cannot spend the grip the steering needs.
    pub traction_control: f32,
    /// Countersteer assistance: extra steering authority correcting yaw
    /// error during slides (0 = off).
    pub countersteer: f32,
    /// How briskly the car levels itself while airborne, as the natural
    /// frequency in rad/s of a critically damped return to level
    /// (`0` = off). The car settles in roughly `4 / air_control` seconds,
    /// so `8.0` is about half a second — quick enough to land a jump on
    /// its wheels rather than its nose. Yaw is left alone.
    pub air_control: f32,
    /// Seconds a car must lie on its side or roof, near stationary, before
    /// it flops back onto its wheels. `0` disables recovery, which strands
    /// the player.
    #[serde(default = "default_self_right_delay")]
    pub self_right_delay: f32,
    /// How firmly a slide is pulled back into line once the handbrake is
    /// let go, as the natural frequency in rad/s of a spring turning the
    /// body back toward the way it is travelling (`0` = off). It acts only
    /// on the slide beyond a few degrees, and never while the handbrake is
    /// held — a held handbrake is a spin, a released one a slide to catch.
    ///
    /// Without it a slide past about 45° cannot end: both axles slide
    /// equally hard, nothing turns the body back, and a half-second
    /// handbrake flick runs on into a full spin.
    #[serde(default)]
    pub slide_recovery: f32,
}

/// Authored `vehGyro` gains, carried verbatim from the tuning record.
/// Original handling applies driven-wheel-spin-scaled yaw torques and
/// brake-dependent airborne levelling (research/vehicle-physics/04).
/// The generic arcade model retains its older yaw-floor adaptation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct GyroConfig {
    /// `Spin180` — wheel-spin-scaled yaw gain for the handbrake spin
    /// while travelling forward. `0` = the car authors no spin assist.
    pub spin180: f32,
    /// `Reverse180` — wheel-spin-scaled yaw gain for the reverse spin
    /// (J-turn) while travelling backwards.
    pub reverse180: f32,
    /// `Drift` — driven-wheel-spin-scaled yaw gain, quadratic in steering.
    pub drift: f32,
    /// `Pitch` — authored airborne pitch-righting rate (rad/s natural
    /// frequency). Absent on four retail records (`None`), authored
    /// `0.0` on the rest — stock cars get no righting this way.
    pub pitch: Option<f32>,
    /// `Roll` — authored airborne roll-righting rate; same presence and
    /// values as [`Self::pitch`].
    pub roll: Option<f32>,
}

fn default_original_bound_friction() -> f32 {
    0.3
}
fn default_original_bound_elasticity() -> f32 {
    0.2
}

/// The original game's vehicle model, carried as authored tuning.
///
/// When a [`VehicleConfig`] has one, the simulation runs the retail
/// `Midtown2.exe` car model recovered in
/// `docs/research/vehicle-physics/` instead of the arcade model the rest
/// of the config describes: 19.6 m/s² gravity, a stick–slip tyre per
/// wheel, drivetrains that integrate their own spin, the original
/// engine, clutch and automatic gearbox, `vehAero`'s rotational damping
/// and the `vehGyro` yaw torques. The arcade fields stay filled in as a
/// summary of what this model does, for the consumers that plan
/// against them (AI, analysis, presentation). Chassis mass, inertia,
/// centre of mass, wheel geometry and gyro tuning remain shared config.
///
/// Every field is the authored token (or a direct derivation from one)
/// so a tuned value means what it meant in the original; the few
/// departures from the original are named on the code that makes them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OriginalHandling {
    /// Raw hull material coefficients (`vehCarSim.BoundFriction/BoundElasticity`).
    /// Pair response multiplies materials; these retain the source values.
    #[serde(default = "default_original_bound_friction")]
    pub bound_friction: f32,
    #[serde(default = "default_original_bound_elasticity")]
    pub bound_elasticity: f32,
    /// Gravity the car falls at and its wheel loads are built on, m/s².
    /// The original's is 19.6 for every car.
    pub gravity: f32,
    /// Friction of the road material a tire sees on an unmarked surface
    /// — retail `_default` is `0.9`. Surface grip multiplies it.
    pub surface_friction: f32,
    /// One entry per wheel, parallel to [`VehicleConfig::wheels`].
    pub wheels: Vec<OriginalWheel>,
    /// Wheel indices on the engine's drivetrain, in left/right pairs
    /// (`DrivetrainType`: rear, front or all four).
    pub driven: Vec<usize>,
    /// Wheels that each spin on their own engineless freetrain.
    pub free: Vec<usize>,
    /// Authored anti-roll axles, with each wheel pair ordered left/right.
    #[serde(default)]
    pub axles: Vec<OriginalAxle>,
    /// `Drivetrain` block.
    pub drivetrain: OriginalTrain,
    /// `Freetrain` block, shared by both freetrains.
    pub freetrain: OriginalTrain,
    /// `Engine` block.
    pub engine: OriginalEngine,
    /// `Trans` block, with the automatic box's derived ratios and
    /// shift points.
    pub gearbox: OriginalGearbox,
    /// `Aero` block.
    pub aero: OriginalAero,
    /// Body angular speed limit, rad/s (the original's `4π` per axis).
    pub max_angular_speed: f32,
}

/// One `WheelFront`/`WheelBack` record applied to one wheel.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OriginalWheel {
    /// Static normal load `L` (N) — the share of `mass · gravity` the
    /// wheel carries at rest, split by `CenterOfGravity.z`. Every spring
    /// and tire constant is built on it.
    pub static_load: f32,
    /// Whether this wheel is on the rear axle: it steers opposite to the
    /// front by its own lock, and takes the handbrake.
    pub rear: bool,
    /// `-1` left, `+1` right, `0` centred — the Ackermann side.
    pub side: f32,
    /// Authored tire width (m), used by the contact probe's inner-edge pivot.
    /// Older external configs without this field keep a centered probe.
    #[serde(default)]
    pub width: f32,
    /// `SuspensionExtent` — droop travel below rest, which is also the
    /// static sag (m).
    pub suspension_extent: f32,
    /// `SuspensionLimit` — bump travel above rest before the bump stop.
    pub suspension_limit: f32,
    /// `SuspensionFactor` — spring progressivity (clamped ≥ 0.75).
    pub suspension_factor: f32,
    /// `SuspensionDampCoef`.
    pub suspension_damp_coef: f32,
    /// `SteeringLimit` — lock, rad.
    pub steering_limit: f32,
    /// `SteeringOffset` — Ackermann gain.
    pub steering_offset: f32,
    /// `BrakeCoef` — brake torque `coef · StaticFric · radius · L`.
    pub brake_coef: f32,
    /// `HandbrakeCoef`, same scale.
    pub handbrake_coef: f32,
    /// `TireDispLimitLat` — lateral bristle travel at full load.
    pub tire_disp_limit_lat: f32,
    /// `TireDispLimitLong`.
    pub tire_disp_limit_long: f32,
    /// `TireDampCoefLat` — fraction of critical for the quarter mass.
    pub tire_damp_coef_lat: f32,
    /// `TireDampCoefLong`.
    pub tire_damp_coef_long: f32,
    /// `TireDragCoefLat` — wading drag (water only on retail).
    pub tire_drag_coef_lat: f32,
    /// `TireDragCoefLong`.
    pub tire_drag_coef_long: f32,
    /// `OptimumSlipPercent` — slip ratio of peak grip.
    pub optimum_slip: f32,
    /// `StaticFric` — peak μ, times the surface friction.
    pub static_fric: f32,
    /// `SlidingFric` — sliding μ floor.
    pub sliding_fric: f32,
}

/// A `vehAxle` anti-roll spring and damper (`0x4d9a10`, `0x4d9b10`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OriginalAxle {
    /// Wheel indices ordered left/right.
    pub wheels: [usize; 2],
    /// `TorqueCoef`: spring stiffness divided by body roll inertia.
    pub torque_coef: f32,
    /// `DampCoef`: fraction of critical roll damping.
    pub damp_coef: f32,
}

/// A `Drivetrain`/`Freetrain` block.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OriginalTrain {
    /// `AngInertia` — not an inertia: the implicit spin stiffness in
    /// `Δω = dt·τ / (I + dt·AngInertia)`.
    pub ang_inertia: f32,
    /// `BrakeDynamicCoef` — brake multiplier while the train turns.
    pub brake_dynamic_coef: f32,
    /// `BrakeStaticCoef` — brake multiplier while it is stopped.
    pub brake_static_coef: f32,
}

/// The `Engine` block.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OriginalEngine {
    /// `MaxHorsePower` in watts (`× 746`), delivered at `OptRPM`.
    pub max_power_w: f32,
    /// `IdleRPM`.
    pub idle_rpm: f32,
    /// `OptRPM` — peak power.
    pub opt_rpm: f32,
    /// `MaxRPM` — torque reaches zero; the hard limiter.
    pub max_rpm: f32,
    /// `AngInertia` — engine inertia, kg·m².
    pub ang_inertia: f32,
    /// `GCL` — seconds of zero engine torque per gear change.
    pub gear_change_lag: f32,
}

/// The `Trans` block, as the automatic box uses it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OriginalGearbox {
    /// Overall ratios (engine revs per wheel rev) in the original's
    /// order: `[0]` reverse (negative), `[1]` neutral (`0`), `[2..]`
    /// forward gears from first.
    pub ratios: Vec<f32>,
    /// Engine rpm above which the box shifts up out of each gear.
    pub upshift_rpm: Vec<f32>,
    /// Engine rpm below which it shifts down at full throttle.
    pub downshift_full_rpm: Vec<f32>,
    /// Engine rpm below which it shifts down at zero throttle.
    pub downshift_zero_rpm: Vec<f32>,
    /// Engine rpm at which full-throttle power in each gear equals the
    /// power after the upshift — what the upshift points are biased
    /// from. Empty when unknown.
    #[serde(default)]
    pub equal_power_rpm: Vec<f32>,
    /// `GearChangeTime` — minimum seconds in a gear before the next
    /// automatic shift.
    pub gear_change_time: f32,
}

/// The `Aero` block. The damping terms are angular *accelerations* per
/// car axis (`x` pitch, `y` yaw, `z` roll), turned into torque by the
/// body's own inertia.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OriginalAero {
    /// `AngCDamp` — constant angular deceleration, rad/s².
    pub ang_c_damp: [f32; 3],
    /// `AngVelDamp` — linear, 1/s.
    pub ang_vel_damp: [f32; 3],
    /// `AngVel2Damp` — quadratic, 1/rad.
    pub ang_vel2_damp: [f32; 3],
    /// `Drag`, kg/m: `F = −Drag · |v_fwd| · v`.
    pub drag: f32,
    /// `Down`, kg/m: `F = −Down · v_fwd²` along the car's up axis.
    pub down: f32,
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
    /// Chassis collider full extents (x, y, z), metres — used for the
    /// fallback cuboid collider and camera framing.
    pub chassis_size: [f32; 3],
    /// Principal angular inertia (kg·m²) about the chassis axes, overriding
    /// the inertia Avian derives from the collider when present.
    #[serde(default)]
    pub inertia: Option<[f32; 3]>,
    /// Convex-hull points for the chassis collider (metres, chassis space).
    /// When absent a cuboid of `chassis_size` is used.
    #[serde(default)]
    pub collider_points: Option<Vec<[f32; 3]>>,
    /// Convex-hull points of the *unmodified* authored bound (metres,
    /// chassis space) — the prop-strike surface. `collider_points` may be
    /// reshaped away from the authored bound (the snag-avoidance
    /// underside); props are still tested against the shape the original
    /// bound-vs-bound prop collision used. `None` means the chassis
    /// collider is the only bound.
    #[serde(default)]
    pub striker_points: Option<Vec<[f32; 3]>>,
    /// Chassis collider friction coefficient (panel friction, not tires).
    #[serde(default = "default_collider_friction")]
    pub collider_friction: f32,
    /// Chassis collider restitution (bounciness).
    #[serde(default)]
    pub collider_restitution: f32,
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
    /// Authored `vehGyro` assist rates — `None` when the vehicle ships
    /// no record (nothing is fabricated; the spin/drift/righting
    /// channels all stay off).
    #[serde(default)]
    pub gyro: Option<GyroConfig>,
    /// Whether this body is a towed trailer (no drivetrain expected).
    #[serde(default)]
    pub trailer: bool,
    /// Authored top speed, m/s (`vehCarSim.Trans.High`) — the speedo
    /// needle's full-scale value (`mmDashView::MaxSpeed` binds to the
    /// carsim rather than the dash file). `None` for unauthored rigs
    /// (dev car, trailers) — presentation consumers must not fabricate
    /// one.
    #[serde(default)]
    pub top_speed_mps: Option<f32>,
    /// The original game's car model — when present the simulation runs
    /// it in place of the arcade model (see [`OriginalHandling`]).
    #[serde(default)]
    pub original: Option<OriginalHandling>,
    /// Authored human steering filter, applied by the input layer.
    #[serde(default)]
    pub player_steering: Option<crate::player_input::PlayerSteeringConfig>,
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
            steer_scale: 1.0,
            brake_bias: 0.25,
            handbrake,
            handbrake_coef: None,
            drive_share: None,
            suspension: None,
            tires: None,
        };
        Self {
            name: "Dev Car".to_string(),
            mass: 1300.0,
            center_of_mass: [0.0, -0.25, 0.0],
            wheelbase,
            track_width: track,
            chassis_size: [1.85, 0.55, 4.4],
            inertia: None,
            collider_points: None,
            striker_points: None,
            collider_friction: default_collider_friction(),
            collider_restitution: 0.0,
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
                peak_power_rpm: None,
                max_power_w: None,
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
                upshift_rpm: None,
                downshift_rpm: None,
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
                grip_limit: default_steering_grip_limit(),
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
                roll_resistance: default_roll_resistance(),
                pitch_resistance: 0.0,
                yaw_stability: 2.5,
                traction_control: 0.9,
                countersteer: 0.35,
                air_control: 8.0,
                self_right_delay: default_self_right_delay(),
                slide_recovery: 2.5,
            },
            gyro: None,
            trailer: false,
            top_speed_mps: None,
            original: None,
            player_steering: None,
        }
    }
}

/// Failure to load or validate a [`VehicleConfig`].
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read.
    #[error("failed to read {path}: {source}")]
    Io {
        /// File path.
        path: std::path::PathBuf,
        /// I/O error.
        source: std::io::Error,
    },
    /// The file is not valid TOML for a `VehicleConfig`.
    #[error("failed to parse {path}: {reason}")]
    Parse {
        /// File path.
        path: std::path::PathBuf,
        /// Parse error.
        reason: String,
    },
    /// The parsed values are not a usable vehicle.
    #[error("invalid vehicle config in {path}:\n{}", problems.join("\n"))]
    Invalid {
        /// File path (or a description such as `<default>`).
        path: String,
        /// Every problem found.
        problems: Vec<String>,
    },
}

impl VehicleConfig {
    /// Load and validate a config from a TOML file.
    ///
    /// An explicit path that fails to read, parse or validate is an error —
    /// callers must not silently fall back to defaults for a file the user
    /// asked for.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        let cfg: Self = toml::from_str(&text).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
        cfg.validate().map_err(|problems| ConfigError::Invalid {
            path: path.display().to_string(),
            problems,
        })?;
        Ok(cfg)
    }

    /// Serialize to TOML (used for shipped examples and debugging).
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// Check that the config describes a usable vehicle. Returns every
    /// problem found so a bad file fails with context, not an indexing
    /// panic at spawn time.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut problems = Vec::new();
        macro_rules! check {
            ($name:expr, $ok:expr $(,)?) => {
                if !$ok {
                    problems.push(format!("{} has an invalid value", $name))
                }
            };
        }
        let finite = |v: f32| v.is_finite();

        check!("mass", finite(self.mass) && self.mass > 0.0);
        check!("wheelbase", finite(self.wheelbase) && self.wheelbase > 0.0);
        check!(
            "track_width",
            finite(self.track_width) && self.track_width > 0.0,
        );
        for (i, v) in self.center_of_mass.iter().enumerate() {
            check!(&format!("center_of_mass[{i}]"), finite(*v));
        }
        for (i, v) in self.chassis_size.iter().enumerate() {
            check!(&format!("chassis_size[{i}]"), finite(*v) && *v > 0.0);
        }
        if let Some(i) = self.inertia {
            for (j, v) in i.iter().enumerate() {
                check!(&format!("inertia[{j}]"), finite(*v) && *v > 0.0);
            }
        }
        if let Some(pts) = &self.collider_points {
            if pts.len() < 4 {
                problems.push("collider_points needs at least 4 points".to_string());
            }
            for (i, p) in pts.iter().enumerate() {
                for (j, v) in p.iter().enumerate() {
                    check!(&format!("collider_points[{i}][{j}]"), finite(*v));
                }
            }
        }
        if let Some(pts) = &self.striker_points {
            if pts.len() < 4 {
                problems.push("striker_points needs at least 4 points".to_string());
            }
            for (i, p) in pts.iter().enumerate() {
                for (j, v) in p.iter().enumerate() {
                    check!(&format!("striker_points[{i}][{j}]"), finite(*v));
                }
            }
        }
        check!(
            "collider_friction",
            finite(self.collider_friction) && self.collider_friction >= 0.0,
        );
        check!(
            "collider_restitution",
            finite(self.collider_restitution) && self.collider_restitution >= 0.0,
        );

        if self.wheels.is_empty() {
            problems.push("wheels must not be empty".to_string());
        }
        for (i, w) in self.wheels.iter().enumerate() {
            check!(
                &format!("wheels[{i}].radius"),
                finite(w.radius) && w.radius > 0.0,
            );
            for (j, v) in w.position.iter().enumerate() {
                check!(&format!("wheels[{i}].position[{j}]"), finite(*v));
            }
            check!(
                &format!("wheels[{i}].brake_bias"),
                finite(w.brake_bias) && w.brake_bias >= 0.0,
            );
            check!(&format!("wheels[{i}].steer_scale"), finite(w.steer_scale),);
            if let Some(ds) = w.drive_share {
                check!(&format!("wheels[{i}].drive_share"), finite(ds) && ds >= 0.0);
            }
            if let Some(hb) = w.handbrake_coef {
                check!(
                    &format!("wheels[{i}].handbrake_coef"),
                    finite(hb) && hb >= 0.0
                );
            }
            if let Some(s) = &w.suspension {
                check!(
                    &format!("wheels[{i}].suspension.spring_rate"),
                    finite(s.spring_rate) && s.spring_rate > 0.0
                );
                check!(
                    &format!("wheels[{i}].suspension.travel"),
                    finite(s.travel) && s.travel > 0.0
                );
            }
            if let Some(t) = &w.tires {
                check!(
                    &format!("wheels[{i}].tires.lateral_grip"),
                    finite(t.lateral_grip) && t.lateral_grip > 0.0
                );
            }
        }
        if !self.trailer && !self.wheels.iter().any(|w| w.driven) {
            problems.push("at least one wheel must be driven".to_string());
        }

        let s = &self.suspension;
        check!(
            "suspension.spring_rate",
            finite(s.spring_rate) && s.spring_rate > 0.0
        );
        check!(
            "suspension.damping_compression",
            finite(s.damping_compression) && s.damping_compression >= 0.0,
        );
        check!(
            "suspension.damping_rebound",
            finite(s.damping_rebound) && s.damping_rebound >= 0.0,
        );
        check!("suspension.travel", finite(s.travel) && s.travel > 0.0);
        check!(
            "suspension.max_force",
            finite(s.max_force) && s.max_force > 0.0
        );
        check!(
            "suspension.force_apply_offset",
            finite(s.force_apply_offset) && s.force_apply_offset >= 0.0,
        );

        let e = &self.engine;
        check!(
            "engine.idle_rpm",
            finite(e.idle_rpm) && e.idle_rpm > 0.0 && e.idle_rpm < e.redline_rpm,
        );
        check!(
            "engine.redline_rpm",
            finite(e.redline_rpm) && e.redline_rpm > e.peak_torque_rpm,
        );
        check!(
            "engine.peak_torque_rpm",
            finite(e.peak_torque_rpm) && e.peak_torque_rpm >= e.idle_rpm,
        );
        check!(
            "engine.peak_torque_nm",
            finite(e.peak_torque_nm) && e.peak_torque_nm > 0.0,
        );
        check!(
            "engine.redline_torque_fraction",
            finite(e.redline_torque_fraction) && (0.0..=1.0).contains(&e.redline_torque_fraction),
        );
        check!(
            "engine.rpm_response",
            finite(e.rpm_response) && e.rpm_response > 0.0,
        );
        check!(
            "engine.engine_brake_nm",
            finite(e.engine_brake_nm) && e.engine_brake_nm >= 0.0,
        );
        match (e.peak_power_rpm, e.max_power_w) {
            (Some(pp), Some(pw)) => {
                check!(
                    "engine.peak_power_rpm",
                    finite(pp) && pp >= e.peak_torque_rpm && pp <= e.redline_rpm,
                );
                check!("engine.max_power_w", finite(pw) && pw > 0.0);
            }
            (None, None) => {}
            _ => problems.push(
                "engine.peak_power_rpm and engine.max_power_w must be set together".to_string(),
            ),
        }

        let t = &self.transmission;
        if t.gear_ratios.is_empty() {
            problems.push("transmission.gear_ratios must not be empty".to_string());
        }
        for (i, g) in t.gear_ratios.iter().enumerate() {
            check!(
                &format!("transmission.gear_ratios[{i}]"),
                finite(*g) && *g > 0.0,
            );
        }
        check!(
            "transmission.reverse_ratio",
            finite(t.reverse_ratio) && t.reverse_ratio > 0.0,
        );
        check!(
            "transmission.final_drive",
            finite(t.final_drive) && t.final_drive > 0.0,
        );
        check!(
            "transmission.shift_time",
            finite(t.shift_time) && t.shift_time >= 0.0,
        );
        check!(
            "transmission.efficiency",
            finite(t.efficiency) && (0.0..=1.0).contains(&t.efficiency),
        );
        if let Some(u) = t.upshift_rpm {
            check!("transmission.upshift_rpm", finite(u) && u > 0.0);
        }
        if let Some(d) = t.downshift_rpm {
            check!("transmission.downshift_rpm", finite(d) && d > 0.0);
        }

        let tr = &self.tires;
        check!(
            "tires.lateral_grip",
            finite(tr.lateral_grip) && tr.lateral_grip > 0.0
        );
        check!(
            "tires.longitudinal_grip",
            finite(tr.longitudinal_grip) && tr.longitudinal_grip > 0.0,
        );
        check!(
            "tires.peak_slip_angle",
            finite(tr.peak_slip_angle) && tr.peak_slip_angle > 0.0,
        );
        check!(
            "tires.peak_slip_ratio",
            finite(tr.peak_slip_ratio) && tr.peak_slip_ratio > 0.0,
        );
        check!(
            "tires.slide_fraction",
            finite(tr.slide_fraction) && (0.0..=1.0).contains(&tr.slide_fraction),
        );
        check!(
            "tires.rolling_resistance",
            finite(tr.rolling_resistance) && tr.rolling_resistance >= 0.0,
        );
        check!(
            "tires.load_sensitivity",
            finite(tr.load_sensitivity) && (0.0..=1.0).contains(&tr.load_sensitivity),
        );

        let st = &self.steering;
        check!(
            "steering.low_speed_max_angle",
            finite(st.low_speed_max_angle) && st.low_speed_max_angle > 0.0,
        );
        check!(
            "steering.high_speed_max_angle",
            finite(st.high_speed_max_angle) && st.high_speed_max_angle >= 0.0,
        );
        check!(
            "steering.high_speed",
            finite(st.high_speed) && st.high_speed > 0.0
        );
        check!(
            "steering.input_rate",
            finite(st.input_rate) && st.input_rate > 0.0
        );
        check!(
            "steering.return_rate",
            finite(st.return_rate) && st.return_rate > 0.0,
        );
        check!(
            "steering.response_curve",
            finite(st.response_curve) && st.response_curve > 0.0,
        );
        check!(
            "steering.grip_limit",
            finite(st.grip_limit) && st.grip_limit >= 0.0,
        );

        let b = &self.brakes;
        check!(
            "brakes.max_brake_force",
            finite(b.max_brake_force) && b.max_brake_force >= 0.0,
        );
        check!(
            "brakes.handbrake_strength",
            finite(b.handbrake_strength) && b.handbrake_strength >= 0.0,
        );

        let a = &self.aero;
        check!(
            "aero.drag_coefficient",
            finite(a.drag_coefficient) && a.drag_coefficient >= 0.0,
        );
        check!(
            "aero.downforce_coefficient",
            finite(a.downforce_coefficient) && a.downforce_coefficient >= 0.0,
        );

        let asst = &self.assists;
        check!(
            "assists.roll_resistance",
            finite(asst.roll_resistance) && (0.0..=1.0).contains(&asst.roll_resistance),
        );
        check!(
            "assists.pitch_resistance",
            finite(asst.pitch_resistance) && (0.0..=1.0).contains(&asst.pitch_resistance),
        );
        check!(
            "assists.yaw_stability",
            finite(asst.yaw_stability) && asst.yaw_stability >= 0.0,
        );
        check!(
            "assists.traction_control",
            finite(asst.traction_control) && (0.0..=1.0).contains(&asst.traction_control),
        );
        check!(
            "assists.countersteer",
            finite(asst.countersteer) && asst.countersteer >= 0.0,
        );
        check!(
            "assists.air_control",
            finite(asst.air_control) && asst.air_control >= 0.0,
        );
        check!(
            "assists.self_right_delay",
            finite(asst.self_right_delay) && asst.self_right_delay >= 0.0,
        );
        check!(
            "assists.slide_recovery",
            finite(asst.slide_recovery) && asst.slide_recovery >= 0.0,
        );

        if let Some(g) = &self.gyro {
            check!("gyro.spin180", finite(g.spin180) && g.spin180 >= 0.0);
            check!(
                "gyro.reverse180",
                finite(g.reverse180) && g.reverse180 >= 0.0,
            );
            check!("gyro.drift", finite(g.drift) && g.drift >= 0.0);
            // Pitch/roll keep the authored sign (the decoder allows it —
            // direction may be meaningful); negative rates are inert at
            // application rather than anti-damping.
            if let Some(p) = g.pitch {
                check!("gyro.pitch", finite(p));
            }
            if let Some(r) = g.roll {
                check!("gyro.roll", finite(r));
            }
        }

        if let Some(original) = &self.original {
            let positive = |v: f32| v.is_finite() && v > 0.0;
            let nonnegative = |v: f32| v.is_finite() && v >= 0.0;
            check!("original.gravity", positive(original.gravity));
            check!(
                "original.bound_friction",
                nonnegative(original.bound_friction)
            );
            check!(
                "original.bound_elasticity",
                nonnegative(original.bound_elasticity)
            );
            check!(
                "original.surface_friction",
                nonnegative(original.surface_friction)
            );
            check!(
                "original.max_angular_speed",
                positive(original.max_angular_speed)
            );
            check!(
                "original.wheels",
                original.wheels.len() == 4 && self.wheels.len() == 4
            );
            let mut indices = original.driven.clone();
            indices.extend(&original.free);
            indices.sort_unstable();
            check!(
                "original.driven/free",
                indices == [0, 1, 2, 3] && !original.driven.is_empty()
            );
            for (i, axle) in original.axles.iter().enumerate() {
                let prefix = format!("original.axles[{i}]");
                check!(
                    format!("{prefix}.wheels"),
                    axle.wheels[0] < original.wheels.len()
                        && axle.wheels[1] < original.wheels.len()
                        && axle.wheels[0] != axle.wheels[1]
                );
                check!(
                    format!("{prefix}.torque_coef"),
                    nonnegative(axle.torque_coef)
                );
                check!(format!("{prefix}.damp_coef"), nonnegative(axle.damp_coef));
            }
            for (i, w) in original.wheels.iter().enumerate() {
                let prefix = format!("original.wheels[{i}]");
                for (field, value) in [
                    ("static_load", w.static_load),
                    ("suspension_extent", w.suspension_extent),
                    ("suspension_factor", w.suspension_factor),
                    ("tire_disp_limit_lat", w.tire_disp_limit_lat),
                    ("tire_disp_limit_long", w.tire_disp_limit_long),
                    ("optimum_slip", w.optimum_slip),
                ] {
                    check!(format!("{prefix}.{field}"), positive(value));
                }
                for (field, value) in [
                    ("width", w.width),
                    ("suspension_limit", w.suspension_limit),
                    ("suspension_damp_coef", w.suspension_damp_coef),
                    ("steering_limit", w.steering_limit),
                    ("brake_coef", w.brake_coef),
                    ("handbrake_coef", w.handbrake_coef),
                    ("tire_damp_coef_lat", w.tire_damp_coef_lat),
                    ("tire_damp_coef_long", w.tire_damp_coef_long),
                    ("tire_drag_coef_lat", w.tire_drag_coef_lat),
                    ("tire_drag_coef_long", w.tire_drag_coef_long),
                    ("static_fric", w.static_fric),
                    ("sliding_fric", w.sliding_fric),
                ] {
                    check!(format!("{prefix}.{field}"), nonnegative(value));
                }
                check!(format!("{prefix}.side"), matches!(w.side, -1.0 | 0.0 | 1.0));
                check!(
                    format!("{prefix}.steering_offset"),
                    finite(w.steering_offset)
                );
            }
            for (name, train) in [
                ("drivetrain", original.drivetrain),
                ("freetrain", original.freetrain),
            ] {
                check!(
                    format!("original.{name}.ang_inertia"),
                    nonnegative(train.ang_inertia)
                );
                check!(
                    format!("original.{name}.brake_dynamic_coef"),
                    nonnegative(train.brake_dynamic_coef)
                );
                check!(
                    format!("original.{name}.brake_static_coef"),
                    nonnegative(train.brake_static_coef)
                );
            }
            let e = &original.engine;
            check!("original.engine.max_power_w", positive(e.max_power_w));
            check!("original.engine.ang_inertia", positive(e.ang_inertia));
            check!(
                "original.engine.gear_change_lag",
                nonnegative(e.gear_change_lag)
            );
            check!(
                "original.engine.rpm",
                positive(e.idle_rpm)
                    && positive(e.opt_rpm)
                    && positive(e.max_rpm)
                    && e.idle_rpm < e.opt_rpm
                    && e.opt_rpm < e.max_rpm
            );
            let box_ = &original.gearbox;
            let gears = box_.ratios.len();
            check!(
                "original.gearbox.ratios",
                gears >= 3
                    && box_.ratios.iter().all(|r| finite(*r))
                    && box_.ratios.first().is_some_and(|r| *r < 0.0)
                    && box_.ratios.get(1) == Some(&0.0)
                    && box_.ratios.iter().skip(2).all(|r| *r > 0.0)
            );
            for (field, points) in [
                ("upshift_rpm", &box_.upshift_rpm),
                ("downshift_full_rpm", &box_.downshift_full_rpm),
                ("downshift_zero_rpm", &box_.downshift_zero_rpm),
            ] {
                check!(
                    format!("original.gearbox.{field}"),
                    points.len() == gears && points.iter().all(|r| nonnegative(*r))
                );
            }
            check!(
                "original.gearbox.equal_power_rpm",
                (box_.equal_power_rpm.is_empty() || box_.equal_power_rpm.len() == gears)
                    && box_.equal_power_rpm.iter().all(|r| nonnegative(*r))
            );
            check!(
                "original.gearbox.gear_change_time",
                nonnegative(box_.gear_change_time)
            );
            let aero = &original.aero;
            for (field, values) in [
                ("ang_c_damp", aero.ang_c_damp),
                ("ang_vel_damp", aero.ang_vel_damp),
                ("ang_vel2_damp", aero.ang_vel2_damp),
            ] {
                check!(
                    format!("original.aero.{field}"),
                    values.iter().all(|v| nonnegative(*v))
                );
            }
            check!("original.aero.drag", nonnegative(aero.drag));
            check!("original.aero.down", nonnegative(aero.down));
        }
        if let Some(p) = &self.player_steering {
            check!("player_steering.speed_sensitive", p.speed_sensitive <= 2);
            check!(
                "player_steering.speed_range",
                finite(p.speed_low)
                    && finite(p.speed_high)
                    && p.speed_low >= 0.0
                    && p.speed_high > p.speed_low
            );
            for (name, pair) in [
                ("delta_out", p.delta_out),
                ("delta_in", p.delta_in),
                ("exponent", p.exponent),
            ] {
                check!(
                    format!("player_steering.{name}"),
                    pair.iter().all(|v| finite(*v) && *v > 0.0)
                );
            }
            for (name, pair) in [
                ("mouse_divisor", p.mouse_divisor),
                ("mouse_exponent", p.mouse_exponent),
            ] {
                if let Some(pair) = pair {
                    check!(
                        format!("player_steering.{name}"),
                        pair.iter().all(|v| finite(*v) && *v > 0.0)
                    );
                }
            }
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        VehicleConfig::default().validate().unwrap();
    }

    #[test]
    fn rejects_empty_wheels_and_gears() {
        let mut cfg = VehicleConfig::default();
        cfg.wheels.clear();
        cfg.transmission.gear_ratios.clear();
        let problems = cfg.validate().unwrap_err();
        assert!(problems.iter().any(|p| p.contains("wheels")));
        assert!(problems.iter().any(|p| p.contains("gear_ratios")));
    }

    #[test]
    fn rejects_bad_numbers() {
        let mut cfg = VehicleConfig {
            mass: f32::NAN,
            ..Default::default()
        };
        cfg.wheels[0].radius = -1.0;
        cfg.engine.idle_rpm = 9000.0; // above redline
        let problems = cfg.validate().unwrap_err();
        assert!(problems.iter().any(|p| p.contains("mass")));
        assert!(problems.iter().any(|p| p.contains("radius")));
        assert!(problems.iter().any(|p| p.contains("idle_rpm")));
    }

    #[test]
    fn toml_round_trip() {
        let cfg = VehicleConfig::default();
        let text = cfg.to_toml();
        let parsed: VehicleConfig = toml::from_str(&text).unwrap();
        parsed.validate().unwrap();
        assert_eq!(parsed.name, cfg.name);
        assert_eq!(parsed.wheels.len(), cfg.wheels.len());
    }
}
