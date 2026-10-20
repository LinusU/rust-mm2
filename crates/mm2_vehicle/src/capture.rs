//! Typed capture and restore of everything the next vehicle step reads.
//!
//! NETPLAY-03A (`docs/netcode-redesign.md`): replaying a car from an
//! earlier tick needs its *complete* simulation state, not just a pose.
//! [`VehicleCapture`] is an explicit, serialisable DTO — plain numbers,
//! no ECS reflection and no `Entity` ids — taken with [`capture_vehicle`]
//! and put back with [`restore_vehicle`]. The inventory table in the
//! design note classifies every field.
//!
//! Scope: this is the state the *vehicle* systems own plus the Avian body
//! components they read and write (pose, velocities, mass). Avian's own
//! internal caches (contact manifolds and solver warm-start impulses,
//! broad-phase pairs, sleep timers) are the physics world's, restored or
//! invalidated by NETPLAY-03B, and are deliberately not in here.
//!
//! Epoch references ([`EpochRef`]) are opaque numbers the caller stamps:
//! the session generation and the vehicle's reset epoch live in the app
//! crate, and a restore must not resurrect a state from before a reset.

use avian3d::prelude::*;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::original::{Bristles, Powertrain, PowertrainHidden};
use crate::vehicle::{
    DriveDirection, EngineImpairment, GyroSpin, OriginalState, OriginalWheelState, PreStepVelocity,
    Vehicle, VehicleInput, VehicleState, WheelState,
};

/// Opaque references tying a capture to the session it came from. The
/// vehicle crate never interprets them; the replay layer rejects a
/// capture whose epoch is not the vehicle's current one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpochRef {
    /// Session generation (a new match or reconnect bumps it).
    pub generation: u32,
    /// The vehicle's reset epoch (every authority teleport bumps it).
    pub reset_epoch: u32,
}

/// Rigid-body state the vehicle step reads and writes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BodyCapture {
    pub position: [f32; 3],
    /// Quaternion `x, y, z, w`.
    pub rotation: [f32; 4],
    pub linear_velocity: [f32; 3],
    pub angular_velocity: [f32; 3],
    /// Body mass — Cops & Robbers gold changes it mid-match.
    pub mass: f32,
    /// Principal angular inertia, also carried-mass dependent.
    pub inertia_principal: [f32; 3],
    /// Rotation of the inertia principal frame, `x, y, z, w`.
    pub inertia_frame: [f32; 4],
}

/// The command state in effect at the capture boundary.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct InputCapture {
    pub throttle: f32,
    pub brake: f32,
    pub steering: f32,
    pub handbrake: f32,
    pub forced_gear: Option<u32>,
}

/// One wheel of the generic model's [`WheelState`], minus the collider
/// `Entity` the wheel ray hit (re-derived by the next step's ray cast;
/// it only feeds telemetry lookups).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WheelCapture {
    pub grounded: bool,
    pub contact_point: [f32; 3],
    pub contact_normal: [f32; 3],
    pub compression: f32,
    pub suspension_force: f32,
    pub vel_long: f32,
    pub vel_lat: f32,
    pub slip_angle: f32,
    pub traction_demand: f32,
    pub lateral_force: f32,
    pub cornering: f32,
    pub longitudinal_force: f32,
    pub surface_grip: f32,
    pub surface_drag: f32,
    pub spin: f32,
    pub slip_visual: f32,
}

/// A latched gyro spin maneuver.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GyroSpinCapture {
    pub rate: f32,
    pub rotated: f32,
    pub age: f32,
}

/// The original model's engine, clutch and gearbox, including the
/// private gear-change latch the next `update_engine` reads.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PowertrainCapture {
    pub gear: u32,
    pub engine_omega: f32,
    pub clutch: bool,
    pub torque: f32,
    pub rpm: f32,
    pub time_in_gear: f32,
    pub gear_changed: bool,
    pub in_gear_change: bool,
    pub lag_left: f32,
    pub rpm_at_shift: f32,
}

/// One wheel of the original model's [`OriginalWheelState`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OriginalWheelCapture {
    pub x: f32,
    pub bristle_lat: f32,
    pub bristle_long: f32,
    pub reaction: f32,
    pub omega: f32,
}

/// The original car model's carried state ([`OriginalState`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OriginalCapture {
    pub powertrain: PowertrainCapture,
    pub drive_omega: f32,
    pub drive_bias: f32,
    pub wheels: Vec<OriginalWheelCapture>,
    pub pending_force: [f32; 3],
    pub pending_torque: [f32; 3],
    pub angular_momentum: Option<[f32; 3]>,
    pub integrated_angular_velocity: [f32; 3],
    pub player_speed: f32,
    pub steering_speed: f32,
    /// Column-major `A`, `B`, `C` implicit-suspension Jacobians.
    pub implicit: [[[f32; 3]; 3]; 3],
    pub push: [f32; 3],
    pub last_push: [f32; 3],
    pub applied_push: [f32; 3],
    pub collision_impulse: [f32; 3],
    pub collision_angular_impulse: [f32; 3],
    pub step_start_position: [f32; 3],
    pub step_start_rotation: [f32; 4],
}

/// Everything the next step of one vehicle needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VehicleCapture {
    pub epoch: EpochRef,
    pub body: BodyCapture,
    pub input: InputCapture,
    /// Velocity the body had as the step began (post-solver correction).
    pub pre_step_linear: [f32; 3],
    pub pre_step_angular: [f32; 3],
    /// [`EngineImpairment`] factor, when the component is present.
    pub impairment: Option<f32>,
    pub wheels: Vec<WheelCapture>,
    pub steer_angle: f32,
    /// `true` when the drivetrain is engaged for reverse.
    pub reverse: bool,
    pub gear: u32,
    pub rpm: f32,
    pub engine_load: f32,
    pub grounded: bool,
    pub forward_speed: f32,
    pub shifting: f32,
    pub upended_for: f32,
    pub gyro_spin: Option<GyroSpinCapture>,
    /// Run-evidence counters; kept so a restore does not rewrite history.
    pub gyro_spins: u32,
    pub gyro_completed: u32,
    pub original: Option<OriginalCapture>,
}

/// Why a restore was refused. Nothing is written when one is returned.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RestoreError {
    #[error("entity has no vehicle components")]
    NotAVehicle,
    #[error("capture has {capture} wheels, vehicle has {vehicle}")]
    WheelCount { capture: usize, vehicle: usize },
    #[error("capture and vehicle disagree on whether the original model runs")]
    ModelMismatch,
    #[error("capture holds a non-finite value in `{0}`")]
    NonFinite(&'static str),
    /// The capture is from before a session change or a vehicle reset
    /// (see `crate::lifetime`).
    #[error("capture is from another session generation or reset epoch")]
    StaleEpoch,
}

fn v3(v: Vec3) -> [f32; 3] {
    v.to_array()
}

fn q4(q: Quat) -> [f32; 4] {
    q.to_array()
}

impl VehicleCapture {
    /// Capture from already-borrowed components.
    #[allow(clippy::too_many_arguments)] // one borrow per source component
    pub fn from_parts(
        epoch: EpochRef,
        body: BodyCapture,
        input: &VehicleInput,
        pre_step: PreStepVelocity,
        impairment: Option<EngineImpairment>,
        state: &VehicleState,
    ) -> Self {
        Self {
            epoch,
            body,
            input: InputCapture {
                throttle: input.throttle,
                brake: input.brake,
                steering: input.steering,
                handbrake: input.handbrake,
                forced_gear: input.forced_gear.map(|g| g as u32),
            },
            pre_step_linear: v3(pre_step.linear),
            pre_step_angular: v3(pre_step.angular),
            impairment: impairment.map(|e| e.0),
            wheels: state.wheels.iter().map(WheelCapture::of).collect(),
            steer_angle: state.steer_angle,
            reverse: state.direction == DriveDirection::Reverse,
            gear: state.gear as u32,
            rpm: state.rpm,
            engine_load: state.engine_load,
            grounded: state.grounded,
            forward_speed: state.forward_speed,
            shifting: state.shifting,
            upended_for: state.upended_for,
            gyro_spin: state.gyro_spin.map(|g| GyroSpinCapture {
                rate: g.rate,
                rotated: g.rotated,
                age: g.age,
            }),
            gyro_spins: state.gyro_spins,
            gyro_completed: state.gyro_completed,
            original: state.original.as_ref().map(OriginalCapture::of),
        }
    }

    /// Every float in a fixed order, named, for tolerance comparison.
    fn floats(&self) -> Vec<(&'static str, f32)> {
        let mut out = Vec::new();
        let mut put = |name: &'static str, vals: &[f32]| {
            out.extend(vals.iter().map(|v| (name, *v)));
        };
        let b = &self.body;
        put("body.position", &b.position);
        put("body.rotation", &b.rotation);
        put("body.linear_velocity", &b.linear_velocity);
        put("body.angular_velocity", &b.angular_velocity);
        put("body.mass", &[b.mass]);
        put("body.inertia", &b.inertia_principal);
        put("body.inertia_frame", &b.inertia_frame);
        let i = &self.input;
        put("input", &[i.throttle, i.brake, i.steering, i.handbrake]);
        put("pre_step_linear", &self.pre_step_linear);
        put("pre_step_angular", &self.pre_step_angular);
        put(
            "impairment",
            &self.impairment.into_iter().collect::<Vec<_>>(),
        );
        for w in &self.wheels {
            put("wheel.contact_point", &w.contact_point);
            put("wheel.contact_normal", &w.contact_normal);
            put(
                "wheel.scalars",
                &[
                    w.compression,
                    w.suspension_force,
                    w.vel_long,
                    w.vel_lat,
                    w.slip_angle,
                    w.traction_demand,
                    w.lateral_force,
                    w.cornering,
                    w.longitudinal_force,
                    w.surface_grip,
                    w.surface_drag,
                    w.spin,
                    w.slip_visual,
                ],
            );
        }
        put(
            "state.scalars",
            &[
                self.steer_angle,
                self.rpm,
                self.engine_load,
                self.forward_speed,
                self.shifting,
                self.upended_for,
            ],
        );
        if let Some(g) = &self.gyro_spin {
            put("gyro_spin", &[g.rate, g.rotated, g.age]);
        }
        if let Some(o) = &self.original {
            let p = &o.powertrain;
            put(
                "original.powertrain",
                &[
                    p.engine_omega,
                    p.torque,
                    p.rpm,
                    p.time_in_gear,
                    p.lag_left,
                    p.rpm_at_shift,
                ],
            );
            put(
                "original.scalars",
                &[
                    o.drive_omega,
                    o.drive_bias,
                    o.player_speed,
                    o.steering_speed,
                ],
            );
            for w in &o.wheels {
                put(
                    "original.wheel",
                    &[w.x, w.bristle_lat, w.bristle_long, w.reaction, w.omega],
                );
            }
            put("original.pending_force", &o.pending_force);
            put("original.pending_torque", &o.pending_torque);
            put(
                "original.angular_momentum",
                &o.angular_momentum.into_iter().flatten().collect::<Vec<_>>(),
            );
            put(
                "original.integrated_angular_velocity",
                &o.integrated_angular_velocity,
            );
            for m in &o.implicit {
                for col in m {
                    put("original.implicit", col);
                }
            }
            put("original.push", &o.push);
            put("original.last_push", &o.last_push);
            put("original.applied_push", &o.applied_push);
            put("original.collision_impulse", &o.collision_impulse);
            put(
                "original.collision_angular_impulse",
                &o.collision_angular_impulse,
            );
            put("original.step_start_position", &o.step_start_position);
            put("original.step_start_rotation", &o.step_start_rotation);
        }
        out
    }

    /// Integer and flag state in a fixed order, named, for exact comparison.
    fn discrete(&self) -> Vec<(&'static str, i64)> {
        let mut out = vec![
            ("epoch.generation", i64::from(self.epoch.generation)),
            ("epoch.reset_epoch", i64::from(self.epoch.reset_epoch)),
            (
                "input.forced_gear",
                self.input.forced_gear.map_or(-1, i64::from),
            ),
            ("impairment.present", i64::from(self.impairment.is_some())),
            ("wheels.len", self.wheels.len() as i64),
            ("reverse", i64::from(self.reverse)),
            ("gear", i64::from(self.gear)),
            ("grounded", i64::from(self.grounded)),
            ("gyro_spin.present", i64::from(self.gyro_spin.is_some())),
            ("gyro_spins", i64::from(self.gyro_spins)),
            ("gyro_completed", i64::from(self.gyro_completed)),
            ("original.present", i64::from(self.original.is_some())),
        ];
        out.extend(
            self.wheels
                .iter()
                .map(|w| ("wheel.grounded", i64::from(w.grounded))),
        );
        if let Some(o) = &self.original {
            let p = &o.powertrain;
            out.extend([
                ("original.gear", i64::from(p.gear)),
                ("original.clutch", i64::from(p.clutch)),
                ("original.gear_changed", i64::from(p.gear_changed)),
                ("original.in_gear_change", i64::from(p.in_gear_change)),
                ("original.wheels.len", o.wheels.len() as i64),
                (
                    "original.angular_momentum.present",
                    i64::from(o.angular_momentum.is_some()),
                ),
            ]);
        }
        out
    }

    /// Compare two captures. Discrete state (gear, clutch, contact and
    /// presence flags, counters, epochs) must match exactly or the
    /// first differing field is named in the error; otherwise the
    /// largest absolute float difference and the field it is in.
    pub fn deviation(&self, other: &Self) -> Result<Deviation, &'static str> {
        let (a, b) = (self.discrete(), other.discrete());
        if a.len() != b.len() {
            return Err("shape");
        }
        if let Some(((name, _), _)) = a.iter().zip(&b).find(|(x, y)| x.1 != y.1) {
            return Err(name);
        }
        let (a, b) = (self.floats(), other.floats());
        if a.len() != b.len() {
            return Err("shape");
        }
        let mut worst = Deviation {
            max_abs: 0.0,
            field: "",
        };
        for ((name, x), (_, y)) in a.iter().zip(&b) {
            // Equal infinities and NaNs of the same kind are not a
            // deviation; anything else non-finite is infinitely far.
            let d = if x.to_bits() == y.to_bits() {
                0.0
            } else {
                let d = (x - y).abs();
                if d.is_nan() { f32::INFINITY } else { d }
            };
            if d > worst.max_abs {
                worst = Deviation {
                    max_abs: d,
                    field: name,
                };
            }
        }
        Ok(worst)
    }

    /// The first non-finite float, if any.
    pub(crate) fn first_non_finite(&self) -> Option<&'static str> {
        self.floats()
            .into_iter()
            .find(|(_, v)| !v.is_finite())
            .map(|(name, _)| name)
    }
}

/// Largest float difference between two captures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Deviation {
    pub max_abs: f32,
    /// Name of the field holding it (`""` when identical).
    pub field: &'static str,
}

impl WheelCapture {
    fn of(w: &WheelState) -> Self {
        Self {
            grounded: w.grounded,
            contact_point: v3(w.contact_point),
            contact_normal: v3(w.contact_normal),
            compression: w.compression,
            suspension_force: w.suspension_force,
            vel_long: w.vel_long,
            vel_lat: w.vel_lat,
            slip_angle: w.slip_angle,
            traction_demand: w.traction_demand,
            lateral_force: w.lateral_force,
            cornering: w.cornering,
            longitudinal_force: w.longitudinal_force,
            surface_grip: w.surface_grip,
            surface_drag: w.surface_drag,
            spin: w.spin,
            slip_visual: w.slip_visual,
        }
    }

    fn to_state(self) -> WheelState {
        WheelState {
            grounded: self.grounded,
            contact_point: Vec3::from(self.contact_point),
            contact_normal: Vec3::from(self.contact_normal),
            contact_entity: None,
            compression: self.compression,
            suspension_force: self.suspension_force,
            vel_long: self.vel_long,
            vel_lat: self.vel_lat,
            slip_angle: self.slip_angle,
            traction_demand: self.traction_demand,
            lateral_force: self.lateral_force,
            cornering: self.cornering,
            longitudinal_force: self.longitudinal_force,
            surface_grip: self.surface_grip,
            surface_drag: self.surface_drag,
            spin: self.spin,
            slip_visual: self.slip_visual,
        }
    }
}

impl OriginalCapture {
    fn of(s: &OriginalState) -> Self {
        let hidden = s.powertrain.hidden();
        let p = &s.powertrain;
        Self {
            powertrain: PowertrainCapture {
                gear: p.gear as u32,
                engine_omega: p.engine_omega,
                clutch: p.clutch,
                torque: p.torque,
                rpm: p.rpm,
                time_in_gear: p.time_in_gear,
                gear_changed: p.gear_changed,
                in_gear_change: hidden.in_gear_change,
                lag_left: hidden.lag_left,
                rpm_at_shift: hidden.rpm_at_shift,
            },
            drive_omega: s.drive_omega,
            drive_bias: s.drive_bias,
            wheels: s
                .wheels
                .iter()
                .map(|w| OriginalWheelCapture {
                    x: w.x,
                    bristle_lat: w.bristles.lat,
                    bristle_long: w.bristles.long,
                    reaction: w.reaction,
                    omega: w.omega,
                })
                .collect(),
            pending_force: v3(s.pending_force),
            pending_torque: v3(s.pending_torque),
            angular_momentum: s.angular_momentum.map(v3),
            integrated_angular_velocity: v3(s.integrated_angular_velocity),
            player_speed: s.player_speed,
            steering_speed: s.steering_speed,
            implicit: [s.implicit_a, s.implicit_b, s.implicit_c].map(|m| m.to_cols_array_2d()),
            push: v3(s.push),
            last_push: v3(s.last_push),
            applied_push: v3(s.applied_push),
            collision_impulse: v3(s.collision_impulse),
            collision_angular_impulse: v3(s.collision_angular_impulse),
            step_start_position: v3(s.step_start_position),
            step_start_rotation: q4(s.step_start_rotation),
        }
    }

    fn apply(&self, s: &mut OriginalState) {
        let p = &self.powertrain;
        s.powertrain = Powertrain::from_capture(
            p.gear as usize,
            p.engine_omega,
            p.clutch,
            p.torque,
            p.rpm,
            p.time_in_gear,
            p.gear_changed,
            PowertrainHidden {
                in_gear_change: p.in_gear_change,
                lag_left: p.lag_left,
                rpm_at_shift: p.rpm_at_shift,
            },
        );
        s.drive_omega = self.drive_omega;
        s.drive_bias = self.drive_bias;
        s.wheels = self
            .wheels
            .iter()
            .map(|w| OriginalWheelState {
                x: w.x,
                bristles: Bristles {
                    lat: w.bristle_lat,
                    long: w.bristle_long,
                },
                reaction: w.reaction,
                omega: w.omega,
            })
            .collect();
        s.pending_force = Vec3::from(self.pending_force);
        s.pending_torque = Vec3::from(self.pending_torque);
        s.angular_momentum = self.angular_momentum.map(Vec3::from);
        s.integrated_angular_velocity = Vec3::from(self.integrated_angular_velocity);
        s.player_speed = self.player_speed;
        s.steering_speed = self.steering_speed;
        let [a, b, c] = self.implicit.map(|m| Mat3::from_cols_array_2d(&m));
        s.implicit_a = a;
        s.implicit_b = b;
        s.implicit_c = c;
        s.push = Vec3::from(self.push);
        s.last_push = Vec3::from(self.last_push);
        s.applied_push = Vec3::from(self.applied_push);
        s.collision_impulse = Vec3::from(self.collision_impulse);
        s.collision_angular_impulse = Vec3::from(self.collision_angular_impulse);
        s.step_start_position = Vec3::from(self.step_start_position);
        s.step_start_rotation = Quat::from_array(self.step_start_rotation);
    }
}

/// Capture one vehicle at the current tick boundary. `None` when the
/// entity is not a vehicle. Call between fixed steps, never inside one:
/// within a step the force accumulators and the contact snapshots are
/// live.
pub fn capture_vehicle(world: &World, entity: Entity, epoch: EpochRef) -> Option<VehicleCapture> {
    let e = world.get_entity(entity).ok()?;
    e.get::<Vehicle>()?;
    let state = e.get::<VehicleState>()?;
    let input = e.get::<VehicleInput>()?;
    let inertia = e.get::<AngularInertia>()?;
    let body = BodyCapture {
        position: v3(e.get::<Position>()?.0),
        rotation: q4(e.get::<Rotation>()?.0),
        linear_velocity: v3(e.get::<LinearVelocity>()?.0),
        angular_velocity: v3(e.get::<AngularVelocity>()?.0),
        mass: e.get::<Mass>()?.0,
        inertia_principal: v3(inertia.principal),
        inertia_frame: q4(inertia.local_frame),
    };
    let pre_step = e.get::<PreStepVelocity>().copied().unwrap_or_default();
    Some(VehicleCapture::from_parts(
        epoch,
        body,
        input,
        pre_step,
        e.get::<EngineImpairment>().copied(),
        state,
    ))
}

/// Put a capture back. Validates first and writes nothing on error.
/// Restores the body pose, velocities, mass properties, the command
/// state, the vehicle state and `PreStepVelocity`; the
/// [`EngineImpairment`] component is inserted or removed to match.
/// `Transform` is rewritten from the restored pose (the vehicles run
/// without a parent) so the render layer does not lag a frame behind.
pub fn restore_vehicle(
    world: &mut World,
    entity: Entity,
    capture: &VehicleCapture,
) -> Result<(), RestoreError> {
    let session_stale = world
        .get_resource::<crate::lifetime::SessionEpoch>()
        .is_some_and(|s| s.0 != capture.epoch.generation);
    let mut e = world
        .get_entity_mut(entity)
        .map_err(|_| RestoreError::NotAVehicle)?;
    let vehicle = e.get::<Vehicle>().ok_or(RestoreError::NotAVehicle)?;
    if session_stale
        || e.get::<crate::lifetime::ResetEpoch>()
            .is_some_and(|r| capture.epoch.reset_epoch < r.0)
    {
        return Err(RestoreError::StaleEpoch);
    }
    let model_original = vehicle.config.original.is_some();
    let wheels = vehicle.config.wheels.len();
    if capture.wheels.len() != wheels {
        return Err(RestoreError::WheelCount {
            capture: capture.wheels.len(),
            vehicle: wheels,
        });
    }
    if capture.original.is_some() != model_original {
        return Err(RestoreError::ModelMismatch);
    }
    if let Some(o) = &capture.original
        && o.wheels.len() != wheels
    {
        return Err(RestoreError::WheelCount {
            capture: o.wheels.len(),
            vehicle: wheels,
        });
    }
    if let Some(name) = capture.first_non_finite() {
        return Err(RestoreError::NonFinite(name));
    }
    if e.get::<VehicleState>().is_none() || e.get::<VehicleInput>().is_none() {
        return Err(RestoreError::NotAVehicle);
    }

    let b = &capture.body;
    let position = Vec3::from(b.position);
    let rotation = Quat::from_array(b.rotation);
    if let Some(mut p) = e.get_mut::<Position>() {
        p.0 = position;
    }
    if let Some(mut r) = e.get_mut::<Rotation>() {
        r.0 = rotation;
    }
    if let Some(mut t) = e.get_mut::<Transform>() {
        t.translation = position;
        t.rotation = rotation;
    }
    if let Some(mut v) = e.get_mut::<LinearVelocity>() {
        v.0 = Vec3::from(b.linear_velocity);
    }
    if let Some(mut v) = e.get_mut::<AngularVelocity>() {
        v.0 = Vec3::from(b.angular_velocity);
    }
    if let Some(mut m) = e.get_mut::<Mass>() {
        m.0 = b.mass;
    }
    if let Some(mut i) = e.get_mut::<AngularInertia>() {
        i.principal = Vec3::from(b.inertia_principal);
        i.local_frame = Quat::from_array(b.inertia_frame);
    }
    if let Some(mut input) = e.get_mut::<VehicleInput>() {
        let c = &capture.input;
        *input = VehicleInput {
            throttle: c.throttle,
            brake: c.brake,
            steering: c.steering,
            handbrake: c.handbrake,
            forced_gear: c.forced_gear.map(|g| g as usize),
        };
    }
    e.insert(PreStepVelocity {
        linear: Vec3::from(capture.pre_step_linear),
        angular: Vec3::from(capture.pre_step_angular),
    });
    match capture.impairment {
        Some(f) => {
            e.insert(EngineImpairment(f));
        }
        None => {
            e.remove::<EngineImpairment>();
        }
    }
    if let Some(mut s) = e.get_mut::<VehicleState>() {
        s.wheels = capture.wheels.iter().map(|w| w.to_state()).collect();
        s.steer_angle = capture.steer_angle;
        s.direction = if capture.reverse {
            DriveDirection::Reverse
        } else {
            DriveDirection::Forward
        };
        s.gear = capture.gear as usize;
        s.rpm = capture.rpm;
        s.engine_load = capture.engine_load;
        s.grounded = capture.grounded;
        s.forward_speed = capture.forward_speed;
        s.shifting = capture.shifting;
        s.upended_for = capture.upended_for;
        s.gyro_spin = capture.gyro_spin.map(|g| GyroSpin {
            rate: g.rate,
            rotated: g.rotated,
            age: g.age,
        });
        s.gyro_spins = capture.gyro_spins;
        s.gyro_completed = capture.gyro_completed;
        if let (Some(c), Some(o)) = (&capture.original, s.original.as_mut()) {
            c.apply(o);
        }
    }
    Ok(())
}
