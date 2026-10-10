//! Retail vehicle dynamics and a configurable dev-car model on Avian physics.
//!
//! Layout:
//! - [`analysis`]: closed-form handling diagnostics (rollover, ride, clearance)
//! - [`config`]: the fully serializable handling definition
//! - [`vehicle`]: ECS components (`Vehicle`, `VehicleInput`, `VehicleState`)
//! - [`original`]: recovered retail tyres, suspension, engine and drivetrain
//! - [`player_input`]: per-car human steering filters and input quantization
//! - [`sim`]: generic dev-car steering, torque, slip and grip
//! - [`systems`]: the per-physics-step simulation + reset handling
//! - [`debug`]: gizmo visualization
//!
//! Convention: metres/kilograms/seconds/radians; vehicle forward = local `-Z`.

use avian3d::prelude::*;
use bevy::prelude::*;

pub mod analysis;
pub mod config;
pub mod debug;
pub mod original;
mod original_contacts;
mod original_step;
pub mod player_input;
pub mod sim;
pub mod surface;
pub mod systems;
pub mod vehicle;

pub use analysis::{HandlingMetrics, WheelMetrics, hull_points};
pub use config::VehicleConfig;
pub use debug::VehicleDebugEnabled;
pub use surface::{OriginalContactMaterial, TireConditions, TireSurface};
pub use systems::{UprightLanding, seat_level, seated_upright_pose, upright_recovery_pose};
pub use vehicle::{
    DriveDirection, EngineImpairment, HumanDriver, PreStepVelocity, RemoteReplica, ResetAuthority,
    ResetPending, ResetVehicle, SelfRightOptOut, StrikeBound, Teleported, Vehicle, VehicleInput,
    VehicleState, WheelState,
};

/// Registers the vehicle simulation. Requires [`PhysicsPlugins`] and a fixed
/// timestep (`Time<Fixed>`) configured by the app.
pub struct VehiclePlugin;

impl Plugin for VehiclePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ResetVehicle>()
            .init_resource::<VehicleDebugEnabled>()
            // A session under a remote authority overwrites this at
            // load; standalone and authoritative worlds keep the
            // default-on local resets.
            .init_resource::<ResetAuthority>()
            // The tire path reads this every physics step; sessions
            // overwrite it at load with their environment modifier.
            .init_resource::<TireConditions>()
            .init_resource::<original_contacts::OriginalContactSnapshots>()
            .add_systems(
                PhysicsSchedule,
                (
                    original_step::prepare_integration,
                    systems::vehicle_simulation,
                    original_step::integrate,
                )
                    .chain()
                    .in_set(PhysicsStepSystems::First)
                    // Avian First systems update collider/interpolation
                    // metadata independently of this body velocity update.
                    .ambiguous_with(PhysicsStepSystems::First),
            )
            .add_systems(
                PhysicsSchedule,
                (
                    original_contacts::apply_original_contact_materials,
                    original_contacts::suppress_original_static_contacts,
                )
                    .chain()
                    .in_set(NarrowPhaseSystems::Last),
            )
            .add_systems(
                PhysicsSchedule,
                original_contacts::restore_original_static_contacts
                    .after(PhysicsStepSystems::Solver)
                    .before(PhysicsStepSystems::Sleeping),
            )
            .add_systems(
                PhysicsSchedule,
                (
                    original_step::evaluate,
                    original_contacts::respond_original_static_contacts,
                    original_step::push_out,
                )
                    .chain()
                    .in_set(PhysicsStepSystems::Last),
            )
            // `vehicle_self_right` emits `ResetVehicle`; the chained
            // `vehicle_reset` applies it the same frame — and every
            // Update-scheduled reset writer orders ahead of it so a
            // teleport's `ResetEpoch` bump lands in the same `Snap`.
            .add_systems(
                Update,
                (systems::vehicle_self_right, systems::vehicle_reset).chain(),
            )
            .add_systems(Update, debug::debug_draw);
    }
}

/// The original's body speed limit, m/s.
const ORIGINAL_MAX_SPEED: f32 = 500.0;

/// Bundle for spawning a vehicle. Add `Transform`/`Position` to place it.
///
/// Uses `collider_points` for a convex-hull collider when configured,
/// otherwise a cuboid of `chassis_size`. `inertia` overrides the principal
/// angular inertia Avian would derive from the collider. The entity also
/// carries a [`StrikeBound`] — the `striker_points` hull when present —
/// for prop-strike overlap queries; it is not a world collider.
pub fn vehicle_bundle(config: &VehicleConfig) -> impl Bundle {
    let collider = config
        .collider_points
        .as_ref()
        .and_then(|pts| {
            Collider::convex_hull(pts.iter().map(|p| Vec3::from(*p)).collect::<Vec<_>>())
        })
        .unwrap_or_else(|| {
            let chassis = Vec3::from(config.chassis_size);
            Collider::cuboid(chassis.x, chassis.y, chassis.z)
        });
    // The strike surface prefers the unmodified authored bound and
    // falls back to the world collider itself — a config with no bound
    // source strikes props with the same shape the world sees.
    let strike_bound = config
        .striker_points
        .as_ref()
        .and_then(|pts| {
            Collider::convex_hull(pts.iter().map(|p| Vec3::from(*p)).collect::<Vec<_>>())
        })
        .map_or_else(|| StrikeBound(collider.clone()), StrikeBound);
    // Inertia: authored principal tensor when present, otherwise the solid
    // cuboid estimate so `AngularInertia` is always explicit.
    let inertia = config.inertia.unwrap_or_else(|| {
        let [w, h, d] = config.chassis_size;
        let m = config.mass;
        [
            m * (h * h + d * d) / 12.0,
            m * (w * w + d * d) / 12.0,
            m * (w * w + h * h) / 12.0,
        ]
    });
    (
        (
            Vehicle {
                config: config.clone(),
            },
            VehicleState::new(config),
            VehicleInput::default(),
            StrikeBound(strike_bound.0),
            PreStepVelocity::default(),
            Name::new(config.name.clone()),
        ),
        (
            RigidBody::Dynamic,
            collider,
            Mass(config.mass),
            AngularInertia {
                principal: Vec3::from(inertia),
                local_frame: Quat::IDENTITY,
            },
            CenterOfMass(Vec3::from(config.center_of_mass)),
            Friction::new(config.collider_friction),
            Restitution::new(config.collider_restitution),
        ),
        // We apply our own drag; keep Avian's damping out of the way.
        // The original model's body has no damping at all beyond its
        // `vehAero` terms, and the original's speed limits.
        (
            LinearDamping(0.0),
            AngularDamping(if config.original.is_some() { 0.0 } else { 0.02 }),
            // Retail clamps each body axis in airborne explicit mode;
            // Avian's norm clamp would also limit grounded implicit spins.
            MaxAngularSpeed(f32::INFINITY),
            MaxLinearSpeed(if config.original.is_some() {
                ORIGINAL_MAX_SPEED
            } else {
                f32::INFINITY
            }),
            LinearVelocity::ZERO,
            AngularVelocity::ZERO,
            SleepingDisabled,
            // Vehicles participate in collision-event reporting so the
            // app's impact pipeline sees their contacts.
            CollisionEventsEnabled,
        ),
    )
}
