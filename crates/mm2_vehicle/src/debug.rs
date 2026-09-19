//! Debug gizmo rendering for vehicle physics.

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::vehicle::{Vehicle, VehicleState};

/// Resource toggling vehicle debug visualization.
#[derive(Resource, Default)]
pub struct VehicleDebugEnabled(pub bool);

/// Draw per-wheel probes, contact data, forces, and vehicle vectors.
pub fn debug_draw(
    mut gizmos: Gizmos,
    enabled: Res<VehicleDebugEnabled>,
    vehicles: Query<(
        &Vehicle,
        &VehicleState,
        &Position,
        &Rotation,
        &LinearVelocity,
    )>,
) {
    if !enabled.0 {
        return;
    }
    for (vehicle, state, pos, rot, linvel) in &vehicles {
        let cfg = &vehicle.config;
        let rot = rot.0;
        let com = pos.0 + rot * Vec3::from(cfg.center_of_mass);
        let forward = rot * Vec3::NEG_Z;
        let up = rot * Vec3::Y;
        let down = -up;

        // Centre of mass + velocity + forward.
        gizmos.sphere(
            Isometry3d::from_translation(com),
            0.08,
            Color::srgb(1.0, 0.2, 1.0),
        );
        gizmos.line(pos.0, pos.0 + forward * 2.0, Color::srgb(0.2, 0.4, 1.0));
        gizmos.line(com, com + linvel.0 * 0.2, Color::srgb(1.0, 0.6, 0.0));

        for (i, wheel) in cfg.wheels.iter().enumerate() {
            let Some(ws) = state.wheels.get(i) else {
                continue;
            };
            let hardpoint = pos.0 + rot * Vec3::from(wheel.position);
            let cast_len = cfg.suspension.travel + wheel.radius;
            let ray_end = hardpoint + down * cast_len;

            // Wheel ray (green when grounded, red airborne).
            let ray_color = if ws.grounded {
                Color::srgb(0.2, 0.9, 0.2)
            } else {
                Color::srgb(0.9, 0.2, 0.2)
            };
            gizmos.line(hardpoint, ray_end, ray_color);

            if ws.grounded {
                // Contact point + normal.
                gizmos.sphere(
                    Isometry3d::from_translation(ws.contact_point),
                    0.05,
                    Color::srgb(0.2, 1.0, 0.2),
                );
                gizmos.line(
                    ws.contact_point,
                    ws.contact_point + ws.contact_normal * 0.4,
                    Color::srgb(0.4, 1.0, 0.4),
                );
                // Suspension travel indicator.
                gizmos.line(
                    hardpoint,
                    hardpoint + down * (cast_len - ws.compression),
                    Color::srgb(0.9, 0.9, 0.2),
                );
                // Tire forces at the contact patch (scaled for visibility).
                let steer = if wheel.steered {
                    state.steer_angle
                } else {
                    0.0
                };
                let fwd_flat = Quat::from_axis_angle(up, -steer) * forward;
                let n = ws.contact_normal;
                let tire_fwd = (fwd_flat - n * fwd_flat.dot(n)).normalize_or_zero();
                let tire_right = tire_fwd.cross(n).normalize_or_zero();
                gizmos.line(
                    ws.contact_point,
                    ws.contact_point + tire_fwd * ws.longitudinal_force * 5e-4,
                    Color::srgb(0.2, 0.8, 1.0),
                );
                gizmos.line(
                    ws.contact_point,
                    ws.contact_point + tire_right * ws.lateral_force * 5e-4,
                    Color::srgb(1.0, 0.3, 0.6),
                );
            }
        }
    }
}
