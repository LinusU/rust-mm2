//! Chase and free/debug cameras, independent of vehicle simulation.

use avian3d::prelude::LinearVelocity;
use bevy::{
    camera::Viewport,
    input::mouse::MouseMotion,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use mm2_formats::dash::PovCamSpec;
use mm2_game::{PlayerVehicle, Session, SessionEntity, SessionPhase};

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

/// Marker for the rear-view mirror strip camera (F22-B.2; HUD-3/CTL-1
/// `BACKSPACE rearview mirror`). A rigid child of the player vehicle
/// facing rearward — the car's pitch/roll moves the mirror view exactly
/// like a windshield-mounted mirror. `drive_mirror` owns its
/// `is_active` and top-strip viewport. It is deliberately excluded
/// from `WorldCamera3d`: an active mirror must never become the audio
/// listener, PVS view source, sky-dome anchor or `cam` pose readout.
#[derive(Component)]
pub struct MirrorCamera;

/// Whether the rear-view mirror is switched on (HUD-3/CTL-1:
/// `BACKSPACE` toggles; `--mirror` arms it for captures). Session-
/// agnostic like [`CameraMode`]: a restart respawns the strip camera
/// and `drive_mirror` re-applies the driver's choice.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct RearView(pub bool);

/// Mirror strip geometry as window fractions — the original's strip
/// placement/extent is unrecovered, so a top-centre third-width strip
/// is a designed reading (DSN-50, UNK-29).
const MIRROR_WIDTH_FRAC: f32 = 1.0 / 3.0;
const MIRROR_HEIGHT_FRAC: f32 = 1.0 / 8.0;

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
        // No recognised session camera exists for any mode — the menu
        // phase or an empty world. Cycling would only drift the mode
        // (the loop settles on an arbitrary step) away from whatever
        // cameras a later session spawns, and the activation pass below
        // is a no-op either way.
        if !have(m, &cams) {
            return;
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

/// The active world camera's pose as `x,y,z,yaw,pitch` (angles in
/// degrees) — the exact value `--cam` accepts, so a screenshot's view
/// can be reproduced.
///
/// Reads [`GlobalTransform`]: the authored cockpit camera is a *child*
/// of the vehicle, so its local `Transform` is the car-space eye
/// offset — not the world pose the HUD `cam` readout, screenshot
/// filenames and the `--cam` round-trip contract need (the damage
/// billboards' camera query is the same precedent). The propagated
/// pose is one frame stale at worst.
pub fn active_cam_pose(
    cameras: &Query<(&Camera, &GlobalTransform), crate::hudmap::WorldCamera3d>,
) -> Option<String> {
    let (_, xf) = cameras.iter().find(|(c, _)| c.is_active)?;
    let xf = xf.compute_transform();
    let (yaw, pitch, _) = xf.rotation.to_euler(EulerRot::YXZ);
    let p = xf.translation;
    Some(format!(
        "{:.1},{:.1},{:.1},{:.0},{:.0}",
        p.x,
        p.y,
        p.z,
        // `+ 0.0` turns a rounded −0 into 0.
        yaw.to_degrees().round() + 0.0,
        pitch.to_degrees().round() + 0.0
    ))
}

/// The root UI nodes pinned to the active camera — the HUD plus the
/// pause/results overlays, which share the session's render target.
/// Every HUD-layer instrument belongs here: without a
/// `UiTargetCamera`, a root falls back to the *highest-order*
/// primary-window camera — the F22-B.2 mirror strip (order 2) — so
/// an unlisted instrument renders inside the strip viewport (or
/// nowhere) whenever it is armed.
type HudNodes = Or<(
    With<crate::session::Hud>,
    With<crate::session::ErrorText>,
    With<crate::pause::PauseUi>,
    With<crate::results::ResultsUi>,
    With<crate::race::CountdownBanner>,
    With<crate::navarrow::NavArrow>,
    With<crate::race::LowTimeWarning>,
    With<crate::racetime::RaceTimer>,
)>;

/// Keep the HUD on whichever world camera is active — UI otherwise
/// stays on the first camera and disappears in free-camera mode.
///
/// The pick goes through [`crate::hudmap::WorldCamera3d`] like every
/// other "the active camera" consumer (F22-B.1): the HUD-map camera
/// renders only the map layer and the F22-B.2 mirror strip is a
/// *second* active `Camera3d` while armed — and it spawns ahead of
/// the cockpit camera (`load_session_world` parents it to the
/// vehicle before `spawn_dash` runs), so a first-active pick without
/// the filter lands on the strip deterministically under
/// `CameraMode::Cockpit` and shrinks the HUD, pause menu, results
/// screen and countdown banner into the ⅓×⅛ top strip.
pub fn retarget_hud(
    mut commands: Commands,
    cameras: Query<(Entity, &Camera), crate::hudmap::WorldCamera3d>,
    ui: Query<(Entity, Option<&UiTargetCamera>), HudNodes>,
) {
    let Some((active, _)) = cameras.iter().find(|(_, c)| c.is_active) else {
        return;
    };
    for (node, target) in &ui {
        if target.is_none_or(|t| t.0 != active) {
            commands.entity(node).insert(UiTargetCamera(active));
        }
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

/// Spawn the rear-view mirror camera as a rigid child of `vehicle`.
///
/// The eye rides at the authored `camPovCS` `Offset` when the car
/// carries one (the driver-seat position the real mirror reflects
/// from) and `fallback_eye` otherwise — a designed seat height, not
/// authored data. Facing is vehicle-local +Z (rearward): the car's
/// pitch and roll move the strip view like a windshield-mounted
/// mirror, and being a child means resets and finishes never strand
/// it. `is_active` starts false — [`RearView`] state owns it through
/// [`drive_mirror`] — so a reload always rebuilds the camera even
/// when the mirror is on. `dash::sync_dash_visibility`'s
/// cockpit/exterior split skips `MirrorCamera` children explicitly:
/// the strip renders over the cockpit too, and its render gate is
/// `is_active`, never `Visibility`.
pub fn spawn_mirror(
    commands: &mut Commands,
    pov: Option<&PovCamSpec>,
    fallback_eye: Vec3,
    vehicle: Entity,
    owner: SessionEntity,
    fog: Option<bevy::pbr::DistanceFog>,
) -> Entity {
    let eye = pov
        .and_then(|p| p.offset)
        .map(Vec3::from)
        .unwrap_or(fallback_eye);
    let cam = commands
        .spawn((
            owner,
            MirrorCamera,
            Camera3d::default(),
            Camera {
                // Over the world view and the map inset, under the UI.
                order: 2,
                is_active: false,
                ..default()
            },
            Projection::Perspective(PerspectiveProjection {
                fov: pov.and_then(|p| p.camera_fov).unwrap_or(60.0).to_radians(),
                near: pov.and_then(|p| p.camera_near).unwrap_or(0.1).max(0.01),
                far: pov.and_then(|p| p.camera_far).unwrap_or(600.0).max(1.0),
                ..default()
            }),
            // Vehicle forward is -Z: a π yaw looks out the rear.
            Transform::from_translation(eye)
                .with_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
        ))
        .id();
    if let Some(f) = fog {
        commands.entity(cam).insert(f);
    }
    commands.entity(vehicle).add_child(cam);
    cam
}

/// `BACKSPACE` toggles the rear-view mirror (HUD-3/CTL-1) in the live
/// phases where the mirror can matter. `Paused`/`Results`/menu
/// contexts keep the key for their overlays (`Back`), and
/// `Unloading`/`Loading` have no camera to aim — the toggle would be
/// invisible either way. The key's old `restart the session` role
/// moved to `F4`, the documented original binding (CTL-1).
pub fn mirror_input(
    keys: Res<ButtonInput<KeyCode>>,
    session: Res<Session>,
    mut mirror: ResMut<RearView>,
) {
    if !matches!(
        session.phase(),
        SessionPhase::Playing | SessionPhase::Countdown
    ) {
        return;
    }
    if keys.just_pressed(KeyCode::Backspace) {
        mirror.0 = !mirror.0;
    }
}

/// Keep the mirror strip honest: `is_active` follows [`RearView`]
/// except under `CameraMode::Free` (the dev camera yields nothing to
/// the HUD), and the viewport tracks the window — a top-centre strip,
/// flush against the top edge. Runs ungated by `capturing` like
/// `drive_hud_map`: a `--frames`/`--screenshot` run with `--mirror`
/// must see the strip with live input frozen. Headless runs have no
/// `PrimaryWindow`; the activity gate still applies so the `mir=`
/// record field reports the real camera state.
pub fn drive_mirror(
    mirror: Res<RearView>,
    mode: Res<CameraMode>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut cams: Query<&mut Camera, With<MirrorCamera>>,
) {
    let want = mirror.0 && *mode != CameraMode::Free;
    let viewport = window.single().ok().map(|w| {
        let size = UVec2::new(
            w.resolution.physical_width(),
            w.resolution.physical_height(),
        );
        let width = (size.x as f32 * MIRROR_WIDTH_FRAC).round().max(1.0) as u32;
        let height = (size.y as f32 * MIRROR_HEIGHT_FRAC).round().max(1.0) as u32;
        Viewport {
            physical_position: UVec2::new(size.x.saturating_sub(width) / 2, 0),
            physical_size: UVec2::new(width.min(size.x), height.min(size.y)),
            depth: 0.0..1.0,
        }
    });
    for mut cam in &mut cams {
        if cam.is_active != want {
            cam.is_active = want;
        }
        // Write-on-diff: a per-frame viewport assignment would mark the
        // camera changed even when the window never moved.
        let stale = match (&cam.viewport, &viewport) {
            (Some(a), Some(b)) => {
                a.physical_position != b.physical_position || a.physical_size != b.physical_size
            }
            (a, b) => a.is_some() != b.is_some(),
        };
        if want && stale {
            cam.viewport = viewport.clone();
        }
    }
}
