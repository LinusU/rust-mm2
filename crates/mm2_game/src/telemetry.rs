//! Read-only vehicle telemetry snapshot (F01-B contract).
//!
//! Presentation (HUD, audio, mirrors) and future network snapshots read
//! [`VehicleTelemetry`] instead of the mutable simulation state: the
//! snapshot is written once per fixed step while the session is playing,
//! stamped with the session tick and the vehicle's stable [`ObjectId`],
//! and declares whether it is authoritative or predicted.
//!
//! `DamageSignals` is the raw accumulated impact input — it is *not* a
//! damage model. F05 decides what impairment means; this is the honest
//! signal it will consume.

use bevy::prelude::*;

use crate::ids::{AuthorityRole, ObjectId};
use crate::surface::SurfaceState;

/// Accumulated impact signal on a simulation object — Σ impact
/// severities affecting it this session. A damage model (F05) reads
/// this; nothing here interprets it as impairment.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct DamageSignals {
    /// Total approach-speed severity of impacts involving this object,
    /// m/s.
    pub impact_total: f32,
    /// Number of impacts recorded.
    pub impact_count: u32,
}

/// Per-wheel telemetry for one fixed step.
#[derive(Debug, Clone, Copy, Default)]
pub struct WheelTelemetry {
    /// Whether the wheel has ground contact this step.
    pub grounded: bool,
    /// World-space contact point (valid when `grounded`).
    pub contact_point: Vec3,
    /// World-space contact normal (valid when `grounded`).
    pub contact_normal: Vec3,
    /// Slip angle at the contact patch, radians.
    pub slip_angle: f32,
    /// Fraction of the tire's longitudinal grip limit the controller
    /// demanded — force utilization, **not** measured wheel slip (the
    /// arcade model has no wheel-speed state). Beyond ±1 the demand
    /// exceeded what the tire could deliver.
    pub traction_demand: f32,
    /// Physical surface under the wheel.
    pub surface: SurfaceState,
}

/// The read-only per-step snapshot consumers see for one vehicle.
/// Published in `FixedLast` (after the physics step in
/// `FixedPostUpdate`), so `tick` names the step the state came from.
#[derive(Component, Debug)]
pub struct VehicleTelemetry {
    /// The vehicle's stable identity this session.
    pub object: ObjectId,
    /// Fixed-step session tick the snapshot describes.
    pub tick: u64,
    /// Whether this snapshot is authoritative or predicted.
    pub authority: AuthorityRole,
    /// World position of the rigid body, metres.
    pub position: Vec3,
    /// World rotation.
    pub rotation: Quat,
    /// World linear velocity, m/s.
    pub linear_velocity: Vec3,
    /// World angular velocity, rad/s.
    pub angular_velocity: Vec3,
    /// Signed speed along the vehicle forward axis, m/s.
    pub forward_speed: f32,
    /// Engine speed, rpm.
    pub rpm: f32,
    /// Drive force delivered as a fraction of what the drivetrain could
    /// deliver at the current rpm/gear — 0 when coasting or airborne,
    /// ~1 at full demand. Dip during gear changes and under traction
    /// control is deliberate: those are the engine actually working
    /// less hard.
    pub engine_load: f32,
    /// Current gear (index into `gear_ratios`).
    pub gear: usize,
    /// Whether the drivetrain is engaged for reverse.
    pub reverse: bool,
    /// Whether a gear change torque dip is in progress.
    pub shifting: bool,
    /// Per-wheel state, parallel to the vehicle's wheel list.
    pub wheels: Vec<WheelTelemetry>,
    /// Accumulated impact input (not a damage model).
    pub damage: DamageSignals,
}
