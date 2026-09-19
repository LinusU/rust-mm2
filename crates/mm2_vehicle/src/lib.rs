//! Configurable arcade vehicle simulation built on Avian physics.
//!
//! Layout:
//! - [`config`]: the fully serializable handling definition
//! - [`vehicle`]: ECS components (`Vehicle`, `VehicleInput`, `VehicleState`)
//! - [`sim`]: pure math (steering curves, torque, slip, grip) — unit tested
//! - [`systems`]: the per-physics-step simulation + reset handling
//! - [`debug`]: gizmo visualization
//!
//! Convention: metres/kilograms/seconds/radians; vehicle forward = local `-Z`.

use avian3d::prelude::*;
use bevy::prelude::*;

pub mod config;
pub mod debug;
pub mod sim;
pub mod systems;
pub mod vehicle;

pub use config::VehicleConfig;
pub use debug::VehicleDebugEnabled;
pub use vehicle::{ResetVehicle, Vehicle, VehicleInput, VehicleState, WheelState};

/// Registers the vehicle simulation. Requires [`PhysicsPlugins`] and a fixed
/// timestep (`Time<Fixed>`) configured by the app.
pub struct VehiclePlugin;

impl Plugin for VehiclePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ResetVehicle>()
            .init_resource::<VehicleDebugEnabled>()
            .add_systems(
                PhysicsSchedule,
                systems::vehicle_simulation
                    .in_set(PhysicsStepSystems::First)
                    // Deliberate: forces accumulate regardless of ordering vs.
                    // Avian's own `First` systems (same pattern Avian uses).
                    .ambiguous_with(PhysicsStepSystems::First),
            )
            .add_systems(Update, systems::vehicle_reset)
            .add_systems(Update, debug::debug_draw);
    }
}

/// Bundle for spawning a vehicle. Add `Transform`/`Position` to place it.
///
/// Uses `collider_points` for a convex-hull collider when configured,
/// otherwise a cuboid of `chassis_size`. `inertia` overrides the principal
/// angular inertia Avian would derive from the collider.
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
        (
            LinearDamping(0.0),
            AngularDamping(0.02),
            LinearVelocity::ZERO,
            AngularVelocity::ZERO,
            SleepingDisabled,
        ),
    )
}
