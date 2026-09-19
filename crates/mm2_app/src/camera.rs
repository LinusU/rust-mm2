//! Chase and free/debug cameras, independent of vehicle simulation.

use avian3d::prelude::LinearVelocity;
use bevy::{
    input::mouse::MouseMotion,
    prelude::*,
    window::{CursorGrabMode, CursorOptions},
};
use mm2_game::PlayerVehicle;

/// Active camera mode.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CameraMode {
    /// Smooth follow behind the player vehicle.
    #[default]
    Chase,
    /// Free-fly debug camera.
    Free,
}

/// Chase camera tuning on the camera entity.
#[derive(Component)]
pub struct ChaseCamera {
    /// Distance behind the vehicle at rest, metres.
    pub distance: f32,
    /// Height above the vehicle, metres.
    pub height: f32,
    /// Extra distance per m/s of speed.
    pub speed_stretch: f32,
    /// Position smoothing (1/s).
    pub smoothness: f32,
    /// Look-ahead point height above vehicle origin.
    pub look_height: f32,
}

impl Default for ChaseCamera {
    fn default() -> Self {
        Self {
            distance: 7.5,
            height: 3.0,
            speed_stretch: 0.06,
            smoothness: 6.0,
            look_height: 1.0,
        }
    }
}

/// Free-fly camera tuning.
#[derive(Component)]
pub struct FreeCamera {
    /// Fly speed in m/s.
    pub speed: f32,
    /// Look sensitivity (radians per pixel).
    pub sensitivity: f32,
    /// Current yaw/pitch (radians).
    pub yaw: f32,
    pub pitch: f32,
}

impl Default for FreeCamera {
    fn default() -> Self {
        Self {
            speed: 30.0,
            sensitivity: 0.003,
            yaw: 0.0,
            pitch: -0.2,
        }
    }
}

/// Toggle chase ↔ free with `C`; also swaps `is_active` so only one renders.
pub fn toggle_camera(
    keys: Res<ButtonInput<KeyCode>>,
    mut mode: ResMut<CameraMode>,
    mut cams: Query<(&mut Camera, Option<&ChaseCamera>)>,
    mut cursor: Query<&mut CursorOptions>,
) {
    if !keys.just_pressed(KeyCode::KeyC) {
        return;
    }
    *mode = match *mode {
        CameraMode::Chase => CameraMode::Free,
        CameraMode::Free => CameraMode::Chase,
    };
    for (mut cam, chase) in &mut cams {
        cam.is_active = (*mode == CameraMode::Chase) == chase.is_some();
    }
    for mut opts in &mut cursor {
        let free = *mode == CameraMode::Free;
        opts.grab_mode = if free {
            CursorGrabMode::Locked
        } else {
            CursorGrabMode::None
        };
        opts.visible = !free;
    }
}

/// Smooth follow: position eased toward a speed-stretched boom, look at the
/// vehicle with a slight velocity lead.
pub fn chase_follow(
    time: Res<Time>,
    mode: Res<CameraMode>,
    mut cams: Query<(&ChaseCamera, &mut Transform)>,
    vehicle: Query<(&GlobalTransform, &LinearVelocity), With<PlayerVehicle>>,
) {
    if *mode != CameraMode::Chase {
        return;
    }
    let Ok((veh_xf, vel)) = vehicle.single() else {
        return;
    };
    let veh_pos = veh_xf.translation();
    let veh_rot = veh_xf.rotation();
    let planar_speed = (vel.x * vel.x + vel.z * vel.z).sqrt();
    for (cam, mut xf) in &mut cams {
        let back = veh_rot * Vec3::Z; // vehicle forward is -Z
        let dist = cam.distance + planar_speed * cam.speed_stretch;
        let target = veh_pos + back * dist + Vec3::Y * cam.height;
        let t = 1.0 - (-cam.smoothness * time.delta_secs()).exp();
        xf.translation = xf.translation.lerp(target, t);
        xf.look_at(veh_pos + Vec3::Y * cam.look_height + vel.0 * 0.05, Vec3::Y);
    }
}

/// Free-fly movement: WASD+QE; mouse look while the cursor is locked.
pub fn free_fly(
    time: Res<Time>,
    mode: Res<CameraMode>,
    keys: Res<ButtonInput<KeyCode>>,
    mut mouse: MessageReader<MouseMotion>,
    mut cams: Query<(&mut FreeCamera, &mut Transform)>,
) {
    if *mode != CameraMode::Free {
        mouse.clear();
        return;
    }
    for (mut free, mut xf) in &mut cams {
        for ev in mouse.read() {
            free.yaw -= ev.delta.x * free.sensitivity;
            free.pitch = (free.pitch - ev.delta.y * free.sensitivity).clamp(-1.5, 1.5);
        }
        xf.rotation = Quat::from_euler(EulerRot::YXZ, free.yaw, free.pitch, 0.0);

        let mut dir = Vec3::ZERO;
        if keys.pressed(KeyCode::KeyW) {
            dir -= Vec3::Z;
        }
        if keys.pressed(KeyCode::KeyS) {
            dir += Vec3::Z;
        }
        if keys.pressed(KeyCode::KeyA) {
            dir -= Vec3::X;
        }
        if keys.pressed(KeyCode::KeyD) {
            dir += Vec3::X;
        }
        if keys.pressed(KeyCode::KeyE) {
            dir += Vec3::Y;
        }
        if keys.pressed(KeyCode::KeyQ) {
            dir -= Vec3::Y;
        }
        let boost = if keys.pressed(KeyCode::ShiftLeft) {
            4.0
        } else {
            1.0
        };
        let step = xf.rotation * dir.normalize_or_zero() * free.speed * boost * time.delta_secs();
        xf.translation += step;
    }
}
