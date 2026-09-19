//! Non-physical wheel meshes that follow the vehicle's suspension state.

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_vehicle::vehicle::{Vehicle, VehicleState};

/// Marker for a wheel mesh following a vehicle's wheel `index`.
#[derive(Component)]
pub struct WheelVisual {
    /// Vehicle entity this wheel belongs to.
    pub vehicle: Entity,
    /// Index into `VehicleConfig::wheels`.
    pub index: usize,
}

/// Position/rotate wheel meshes: at the ray hit minus radius, steered and
/// spun. Runs in `Update` so it tracks the interpolated physics pose.
pub fn update_wheel_visuals(
    vehicles: Query<(&Vehicle, &VehicleState, &Position, &Rotation)>,
    mut wheels: Query<(&WheelVisual, &mut Transform)>,
) {
    for (vis, mut xf) in &mut wheels {
        let Ok((veh, state, pos, rot)) = vehicles.get(vis.vehicle) else {
            continue;
        };
        let cfg = &veh.config;
        let (Some(wheel), Some(ws)) = (cfg.wheels.get(vis.index), state.wheels.get(vis.index))
        else {
            continue;
        };
        let hardpoint = rot.0 * Vec3::from(wheel.position);
        let cast_len = cfg.suspension.travel + wheel.radius;
        let drop = if ws.grounded {
            cast_len - ws.compression - wheel.radius
        } else {
            cast_len - wheel.radius
        };
        xf.translation = pos.0 + hardpoint + (rot.0 * Vec3::NEG_Y) * drop;

        // Wheel axis is local X: cylinder is Y-aligned, so rotate Z by 90°,
        // then apply spin about X and steer about Y.
        let steer = if wheel.steered {
            state.steer_angle
        } else {
            0.0
        };
        xf.rotation = rot.0
            * Quat::from_rotation_y(-steer)
            * Quat::from_rotation_x(ws.spin)
            * Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    }
}
