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
    /// Authored `camPovCS` cockpit/dash view (HUD-3; F22-B.1).
    Cockpit,
    /// Free-fly debug camera.
    Free,
}

impl CameraMode {
    /// `C` cycle order: Chase → Cockpit → Free → Chase (HUD-3).
    fn next(self) -> Self {
        match self {
            Self::Chase => Self::Cockpit,
            Self::Cockpit => Self::Free,
            Self::Free => Self::Chase,
        }
    }
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

/// The recognised session cameras — each `Camera` carries at most one
/// of the three markers; unmarked cameras belong to other systems.
type SessionCameras<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Camera,
        Option<&'static ChaseCamera>,
        Option<&'static crate::dash::CockpitCamera>,
        Option<&'static FreeCamera>,
    ),
>;

/// `C` cycles the HUD-3 view chain (Chase → Cockpit → Free — Free is a
/// dev extension beyond the authored chase/cockpit pair, DSN-48);
/// `V` is the dashboard toggle — it jumps straight into or out of the
/// cockpit view. (The original's dash key was `D`, but enhanced input
/// put steering on `A`/`D` — the binding moves, the behavior stays;
/// HUD-3, DSN-48.) A mode whose camera was never spawned (no authored
/// `camPovCS`) is skipped so the key never dead-ends on a black screen.
///
/// Activation is marker-driven: each `Camera` carries exactly one of
/// `ChaseCamera`/`CockpitCamera`/`FreeCamera`, and only the matching
/// one is enabled. Cameras carrying none of them (the HUD-map camera,
/// overlays) are left to their own owners — previously a `C` press
/// flipped the map camera's `is_active` for a frame.
pub fn toggle_camera(
    keys: Res<ButtonInput<KeyCode>>,
    mut mode: ResMut<CameraMode>,
    mut cams: SessionCameras,
    mut cursor: Query<&mut CursorOptions>,
) {
    let have = |m: CameraMode, cams: &SessionCameras| -> bool {
        cams.iter().any(|(_, c, p, f)| match m {
            CameraMode::Chase => c.is_some(),
            CameraMode::Cockpit => p.is_some(),
            CameraMode::Free => f.is_some(),
        })
    };
    let next = if keys.just_pressed(KeyCode::KeyC) {
        let mut m = mode.next();
        for _ in 0..3 {
            if have(m, &cams) {
                break;
            }
            m = m.next();
        }
        m
    } else if keys.just_pressed(KeyCode::KeyV) {
        match *mode {
            CameraMode::Cockpit => CameraMode::Chase,
            _ if have(CameraMode::Cockpit, &cams) => CameraMode::Cockpit,
            _ => return,
        }
    } else {
        return;
    };
    if next == *mode {
        return;
    }
    *mode = next;
    for (mut cam, chase, pov, free) in &mut cams {
        let active = match *mode {
            CameraMode::Chase => chase.is_some(),
            CameraMode::Cockpit => pov.is_some(),
            CameraMode::Free => free.is_some(),
        };
        // Only claim the recognised session cameras — an unmarked one
        // (map, UI) is owned elsewhere.
        if chase.is_some() || pov.is_some() || free.is_some() {
            cam.is_active = active;
        }
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
