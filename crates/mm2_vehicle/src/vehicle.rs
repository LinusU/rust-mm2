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

/// The body's velocity as the physics step began, before the solver
/// answered any contact. Written by `vehicle_simulation` every step.
///
/// It is what a post-solver correction restores when it has to take
/// back a response the solver should never have given — a dormant prop
/// answered as a wall. Undoing that response as one impulse along one
/// contact normal missed whatever the solver spread over other points
/// and substeps: a parking meter at 80 km/h sent the Beetle up at
/// 5.5 m/s spinning on every axis.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct PreStepVelocity {
    pub linear: Vec3,
    pub angular: Vec3,
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
    /// Cornering utilization: the tire's lateral force over its lateral
    /// limit, signed, before the friction ellipse shares the budget with
    /// the drive. Traction control reads the car's hardest-cornering
    /// tire from the previous step.
    pub cornering: f32,
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
    /// 0..1 slide intensity under the original model's tyre (`≥ 0.5` a
    /// major slip; always `0` under the arcade model).
    pub slip_visual: f32,
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

/// Latched spin-maneuver state for the authored `vehGyro` assists
/// (F05-B.4).
///
/// Once the handbrake-spin trigger lands, the assist keeps driving the
/// yaw servo until the car has rotated ~180°, the driver releases the
/// maneuver inputs, or the bounded age expires. A per-frame gate cannot
/// express this: halfway through a 180 the car slides sideways, its
/// *forward* speed collapses to zero, and any speed-gated check would
/// drop the assist at 90° — stalling the maneuver exactly where it is
/// supposed to be doing the work. The recovered `mmCarSim` keeps a
/// `SpinState` machine for the same reason; the original's trigger
/// tests are unrecovered (UNK-13), so the latch conditions are the
/// documented designed reading.
#[derive(Debug, Clone, Copy)]
pub struct GyroSpin {
    /// Signed yaw rate the servo drives toward, rad/s (negative yaws the
    /// nose right — positive steering spins right).
    pub rate: f32,
    /// Rotation already delivered in the commanded direction, radians.
    /// Can regress if something knocks the car back — the maneuver then
    /// simply has further to go.
    pub rotated: f32,
    /// Maneuver age in seconds — bounds the latch so a wedged car is not
    /// servoed forever.
    pub age: f32,
}

/// Per-wheel state the original car model carries between steps.
#[derive(Debug, Clone, Copy, Default)]
pub struct OriginalWheelState {
    /// Suspension compression, metres (`0` on the modelled pivot).
    pub x: f32,
    /// Tyre bristle displacements.
    pub bristles: crate::original::Bristles,
    /// Last step's tyre reaction torque `F_long · r`, N·m — what the
    /// wheel's drivetrain answers this step.
    pub reaction: f32,
    /// Wheel spin, rad/s, `+` rolling forward.
    pub omega: f32,
}

/// State of the original car model (see
/// [`OriginalHandling`](crate::config::OriginalHandling)).
#[derive(Debug, Clone)]
pub struct OriginalState {
    /// Engine, clutch and gearbox.
    pub powertrain: crate::original::Powertrain,
    /// The engine's drivetrain: shaft speed (wheel rad/s, `+` forward)
    /// and limited-slip bias.
    pub drive_omega: f32,
    pub drive_bias: f32,
    /// Per-wheel state, parallel to `VehicleConfig::wheels`.
    pub wheels: Vec<OriginalWheelState>,
    /// Forces from the preceding post-integration wheel update.
    pub pending_force: Vec3,
    pub pending_torque: Vec3,
    /// World angular momentum written before the previous pose advance.
    pub angular_momentum: Option<Vec3>,
    /// Angular velocity submitted before Avian applies body contacts.
    pub integrated_angular_velocity: Vec3,
    /// Player rules read the preceding frame's pre-integration Speed,
    /// one sample older than the public post-integration telemetry.
    pub player_speed: f32,
    /// Recorder steering uses parameters computed by the previous player
    /// update from its cached Speed: two samples behind body telemetry.
    pub steering_speed: f32,
    /// Coupled implicit suspension Jacobians A, B, C (research 01).
    pub implicit_a: Mat3,
    pub implicit_b: Mat3,
    pub implicit_c: Mat3,
    /// Position-only bump-stop correction. Never adds kinetic energy.
    pub push: Vec3,
}

impl OriginalState {
    /// A car at rest in first, with a freshly reset cold engine.
    pub fn new(original: &crate::config::OriginalHandling) -> Self {
        let engine = crate::original::EngineConstants::of(&original.engine);
        Self {
            powertrain: crate::original::Powertrain::new(&engine),
            drive_omega: 0.0,
            drive_bias: 1.0,
            wheels: vec![OriginalWheelState::default(); original.wheels.len()],
            push: Vec3::ZERO,
            pending_force: Vec3::ZERO,
            pending_torque: Vec3::ZERO,
            angular_momentum: None,
            integrated_angular_velocity: Vec3::ZERO,
            player_speed: 0.0,
            steering_speed: 0.0,
            implicit_a: Mat3::ZERO,
            implicit_b: Mat3::ZERO,
            implicit_c: Mat3::ZERO,
        }
    }
}

/// Marks a car a person drives. Under the original car model it gets
/// the player-side rules the original applied to human input only
/// (`mmGame::ApplyPlayerInput`, `mmPlayer::Update`): with auto reverse on,
/// holding the brake below 5 m/s swaps the pedals into reverse, and below
/// 4 mph off the throttle the handbrake holds the car. AI cars brake to
/// a stop before reversing; humans with auto reverse off keep their pedals.
#[derive(Component, Debug, Clone, Copy)]
pub struct HumanDriver {
    /// The player's auto-reverse option (on in the original by default).
    pub auto_reverse: bool,
}

impl Default for HumanDriver {
    fn default() -> Self {
        Self { auto_reverse: true }
    }
}

/// Fraction of rated engine drive torque delivered this step — the
/// physics-side input an app-level impairment feature writes (F05-B
/// damage, DSN-25); the sim itself knows nothing about damage. `1.0`
/// is full output and absence of the component is identical. The sim
/// sanitises the value every step: non-finite reads as `1.0`,
/// negative/`>1` clamps — a garbage factor can neither stall nor
/// over-drive the car.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct EngineImpairment(pub f32);

/// Marker for a vehicle entity whose truth arrives by replication from
/// a remote authority rather than the local sim (F25-B) — the kinematic
/// copies a predicted client keeps of the other participants' cars.
/// [`vehicle_simulation`](crate::systems::vehicle_simulation) skips
/// these: stepping them would burn wheel raycasts on a kinematic body
/// and overwrite the `VehicleState`/`VehicleInput` fields the snapshot
/// stream drives (steer angle, wheel spin, brake/direction flags).
/// Presentation systems still read those components normally — the
/// marker only excludes the *stepping* of the state, never its
/// consumption. The authority's own simulated remote cars are unmarked.
#[derive(Component, Debug, Clone, Copy)]
pub struct RemoteReplica;

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
    /// Latched gyro spin maneuver in progress, if any (see [`GyroSpin`]).
    pub gyro_spin: Option<GyroSpin>,
    /// Spins the gyro assist has latched (evidence counter — the
    /// headless record's `gyr=` field).
    pub gyro_spins: u32,
    /// Latched spins that ran to ~180° before releasing.
    pub gyro_completed: u32,
    /// The original car model's state, when the config runs it.
    pub original: Option<OriginalState>,
}

impl VehicleState {
    /// Speed used by the original recorder's human steering parameters.
    pub fn human_steering_speed(&self) -> f32 {
        self.original
            .as_ref()
            .map_or(self.forward_speed.abs(), |s| s.steering_speed)
    }
    /// Make the car's wheels roll with the road at `forward_speed` (m/s)
    /// — for a car given that velocity from outside the sim, such as a
    /// test or probe launching it at speed. Under the original model
    /// wheel spin is state: without this the car would set off at speed
    /// on locked wheels. The engine and gearbox follow, in the highest
    /// forward gear the engine fits in. The arcade model has no wheel
    /// state, so there it does nothing.
    pub fn roll_at(&mut self, config: &VehicleConfig, forward_speed: f32) {
        let (Some(orig), Some(os)) = (config.original.as_ref(), self.original.as_mut()) else {
            return;
        };
        self.forward_speed = forward_speed;
        os.player_speed = forward_speed.abs();
        os.steering_speed = forward_speed.abs();
        for (i, w) in os.wheels.iter_mut().enumerate() {
            let radius = config.wheels.get(i).map_or(0.34, |c| c.radius).max(0.01);
            w.omega = forward_speed / radius;
            w.bristles = crate::original::Bristles::default();
            w.reaction = 0.0;
        }
        let radius = orig
            .driven
            .first()
            .and_then(|&i| config.wheels.get(i))
            .map_or(0.34, |c| c.radius)
            .max(0.01);
        os.drive_omega = forward_speed / radius;
        os.drive_bias = 1.0;
        let engine = crate::original::EngineConstants::of(&orig.engine);
        let ratios = &orig.gearbox.ratios;
        let pt = &mut os.powertrain;
        let gear = (crate::original::FIRST_GEAR..ratios.len())
            .find(|&g| ratios[g] * os.drive_omega <= engine.omega_opt)
            .unwrap_or(ratios.len().saturating_sub(1));
        pt.set_gear(gear);
        let omega = ratios.get(gear).copied().unwrap_or(0.0) * os.drive_omega;
        if omega > engine.omega_idle {
            pt.engine_omega = omega.min(engine.omega_max);
            pt.clutch = true;
        }
    }

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
            gyro_spin: None,
            gyro_spins: 0,
            gyro_completed: 0,
            original: config.original.as_ref().map(OriginalState::new),
        }
    }
}

/// Marks a vehicle the self-righting assist ([`systems::vehicle_self_right`])
/// leaves alone — the player's "automatic flip recovery: off" choice. The
/// assist is a modern addition (retail recovers a wreck through the
/// authored `vehstuck` detector and the manual reset key, both of which
/// stay available), so opting out strands nothing the original did not.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct SelfRightOptOut;

/// Whether this process may originate a [`ResetVehicle`] itself. A
/// standalone world and any authoritative session (`Local`, or the host
/// of a networked session) resolve their own teleports; a predicted
/// client never does — a locally-teleported car is a self-teleport the
/// authority's copy can never learn, so on a `Remote` session every
/// teleport arrives as the wire's declared reset epoch instead
/// (F25-A.7). The app stamps it from the session's authority at load;
/// the default is `true`, so a vehicle world that never joined a
/// networked session keeps its assists.
#[derive(Resource, Debug, Clone, Copy)]
pub struct ResetAuthority(pub bool);

impl Default for ResetAuthority {
    fn default() -> Self {
        Self(true)
    }
}

/// Message requesting a vehicle reset to a pose. Every authority-side
/// teleport lands here — the `R`/pad bundle, a scheduled `--reset-at`,
/// scripted and opponent re-anchors, the stuck/disabled/recovery
/// resolves, and the self-right assist — so
/// [`vehicle_reset`](crate::systems::vehicle_reset) stays the single
/// apply point and no teleport can skip the `Teleported` marker or, in
/// a hosted session, the wire reset epoch.
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

/// Marker a fixed-step resolver puts on a car it has written a
/// [`ResetVehicle`] for, and [`vehicle_reset`](crate::systems::vehicle_reset)
/// removes when the teleport lands. Fixed steps run at a higher rate
/// than frames, so the reset waits in the message queue while further
/// steps still see the old pose; observers that would act on a car's
/// pose (the recovery and stuck detectors) must skip a car carrying this
/// marker, or they fire a second time on the pose the reset is about to
/// replace. Only put it on an entity `vehicle_reset` can reach — a
/// `Vehicle` — or nothing ever clears it.
#[derive(Component, Debug, Clone, Copy)]
pub struct ResetPending;
