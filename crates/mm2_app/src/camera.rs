//! Chase and free/debug cameras, independent of vehicle simulation.

use avian3d::prelude::{LinearVelocity, SpatialQuery, SpatialQueryFilter};
use bevy::{
    camera::Viewport,
    input::mouse::MouseMotion,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use mm2_assets::Vfs;
use mm2_formats::{
    camtrack::{TrackCamSpec, usable1},
    dash::PovCamSpec,
};
use mm2_game::{PlayerVehicle, Session, SessionEntity, SessionPhase};

use crate::input::control_just_pressed;

/// Active camera mode.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CameraMode {
    /// Chase-near: smooth follow on the vehicle's near lens
    /// (`_near.camtrackcs` when authored, HUD-3).
    #[default]
    Chase,
    /// Authored `camPovCS` cockpit/dash view (HUD-3; F22-B.1).
    Cockpit,
    /// Chase-far: the vehicle's authored `_far.camtrackcs` lens
    /// (HUD-3). The slot exists only while a record bound — the
    /// `C` chain skips it otherwise.
    ChaseFar,
    /// Free-fly debug camera — a dev extension appended after the
    /// documented near → cockpit → far chain (HUD-3, DSN-48).
    Free,
}

impl CameraMode {
    /// `C` cycle order: Chase Near → Cockpit → Chase Far → Free →
    /// Chase (HUD-3 documents the first three; Free is the dev
    /// extension, DSN-48).
    fn next(self) -> Self {
        match self {
            Self::Chase => Self::Cockpit,
            Self::Cockpit => Self::ChaseFar,
            Self::ChaseFar => Self::Free,
            Self::Free => Self::Chase,
        }
    }
}

/// One `camTrackCS` chase view distilled for the controller — the
/// authored near or far rig, or a size-derived fallback when the
/// vehicle ships no record. `offset`/`aim` are car-space: `+Z` is
/// rearward (vehicle forward is `−Z`), `+Y` up.
///
/// The follow model is the recovered retail one (UNK-36,
/// `docs/research/camtrack.md`): the aim point `T` is `TrackTo` through
/// the full car matrix, and the desired eye is `T` plus `Offset` in a
/// **yaw-only, gravity-aligned** frame ([`desired_eye`]) — car pitch
/// and roll never tilt the boom. The eye then approaches that point at
/// a live follow rate that the car's speed interpolates between
/// `MaxAppXZPos` (stopped) and `MinAppXZPos` (fast) ([`follow_rate_target`]).
#[derive(Debug, Clone)]
pub struct ChaseLens {
    /// `Offset` — yaw-frame offset from the aim point to the desired eye
    /// (`+X` right, `+Y` up, `+Z` behind).
    pub offset: Vec3,
    /// `TrackTo` — car-local aim point.
    pub aim: Vec3,
    /// `MinDist`/`MaxDist` — hard clamp of the eye–aim distance after the
    /// approach. Not gated by `MinMaxOn` (recovered, UNK-36); `dist_max
    /// == 0` (or `<= dist_min`) means no clamp, as in the original.
    pub dist_min: f32,
    pub dist_max: f32,
    /// `MinSpeed`/`MaxSpeed` — car forward speed window (m/s) across
    /// which the follow rate lerps `MaxAppXZPos`→`MinAppXZPos`
    /// (recovered, UNK-36; the window does not touch the boom length).
    pub speed_min: f32,
    pub speed_max: f32,
    /// `MaxAppXZPos` / `MinAppXZPos` — follow rate (1/s) when stopped /
    /// at `speed_max`.
    pub app_xz_max: f32,
    pub app_xz_min: f32,
    /// `AppInc`/`AppDec` — follow-rate slew up/down, per second.
    pub app_inc: f32,
    pub app_dec: f32,
    /// `AppYPos` — vertical follow rate (1/s).
    pub app_y: f32,
    /// `AppPosMin` — soft-knee distance of the approach.
    pub app_pos_min: f32,
    /// `AppApp` low-pass factor, `None` when `AppAppOn` is 0.
    pub app_app: Option<f32>,
    /// `ApproachOn`: when false the eye is set straight to the desired
    /// position every frame.
    pub approach: bool,
    /// `AppXZPos` as authored — the follow rate before the first frame
    /// rewrites it; absent records start at the speed-lerp target.
    pub app_xz_init: Option<f32>,
    /// `VertOffset` — scales the look target's lift
    /// `LookAbove = (Offset.y − 0.8)·VertOffset` (recovered, UNK-36).
    pub vert_offset: f32,
    /// `CollideType` nonzero: the boom pulls in front of world
    /// geometry that would occlude the car.
    pub collide: bool,
    /// `CameraFOV`/`CameraNear`/`CameraFar` — projection.
    pub fov_deg: f32,
    /// `CameraNear`.
    pub clip_near: f32,
    /// `CameraFar`.
    pub clip_far: f32,
    /// Whether the values came from an authored record (smoke `trk=`
    /// provenance — a sized fallback never claims authored data).
    pub authored: bool,
}

/// `camTrackCS` constructor defaults (`0x51d750`, UNK-36) for the
/// follow-dynamics fields a record may omit.
mod ctor {
    pub const MIN_APP_XZ: f32 = 1.8;
    pub const MAX_APP_XZ: f32 = 12.0;
    pub const MIN_SPEED: f32 = 5.0;
    pub const MAX_SPEED: f32 = 35.0;
    pub const APP_INC: f32 = 15.0;
    pub const APP_DEC: f32 = 10.0;
    pub const APP_Y: f32 = 5.0;
    pub const APP_APP: f32 = 0.7;
    pub const APP_POS_MIN: f32 = 0.25;
    pub const VERT_OFFSET: f32 = 0.6;
}

impl ChaseLens {
    /// Distill an authored `camTrackCS` record. A field the record omits
    /// keeps the constructor default for the follow dynamics, and a
    /// designed value for `Offset`/`TrackTo`. Every field reads through
    /// the spec's usable-checked accessors — a `nan`/`inf` or
    /// beyond-[`USABLE_BOUND`](mm2_formats::camtrack::USABLE_BOUND)
    /// authored value reads unauthored and `validate` names it: a
    /// non-finite `Offset` would poison the eye into NaN, a `3e38`
    /// `TrackTo` would overflow `veh_rot * aim` into a non-finite look
    /// target, and a `nan` `CameraFar` through a `.max(1.0)` sink would
    /// silently clamp the far plane to a metre.
    pub fn authored(spec: &TrackCamSpec) -> Self {
        let offset = spec
            .offset_vec()
            .map(Vec3::from)
            .unwrap_or(Vec3::new(0.0, 1.8, 5.0));
        let rate =
            |v: Option<f32>, default: f32| usable1(v).filter(|r| *r >= 0.0).unwrap_or(default);
        let app_app = spec.app_app_enabled().then(|| {
            usable1(spec.app_app)
                .unwrap_or(ctor::APP_APP)
                .clamp(0.0, 1.0)
        });
        Self {
            offset,
            aim: spec
                .track_to_vec()
                .map(Vec3::from)
                .unwrap_or(Vec3::new(0.0, 1.0, 0.0)),
            dist_min: spec.min_dist_m().unwrap_or(0.0).max(0.0),
            dist_max: spec.max_dist_m().unwrap_or(0.0).max(0.0),
            speed_min: spec.min_speed_mps().unwrap_or(ctor::MIN_SPEED),
            speed_max: spec.max_speed_mps().unwrap_or(ctor::MAX_SPEED),
            app_xz_max: rate(spec.max_app_xz_pos, ctor::MAX_APP_XZ),
            app_xz_min: rate(spec.min_app_xz_pos, ctor::MIN_APP_XZ),
            app_inc: rate(spec.app_inc, ctor::APP_INC),
            app_dec: rate(spec.app_dec, ctor::APP_DEC),
            app_y: rate(spec.app_y_pos, ctor::APP_Y),
            app_pos_min: rate(spec.app_pos_min, ctor::APP_POS_MIN),
            app_app,
            approach: spec.approach_enabled(),
            app_xz_init: usable1(spec.app_xz_pos).filter(|r| *r >= 0.0),
            vert_offset: usable1(spec.vert_offset).unwrap_or(ctor::VERT_OFFSET),
            collide: spec.collides(),
            // `camera_fov_deg` reads an undrawable `CameraFOV`
            // (non-finite or outside `(0, 180)`) as unauthored — the
            // designed 70° stands in, `validate` reports the record.
            fov_deg: spec.camera_fov_deg().unwrap_or(70.0),
            clip_near: TrackCamSpec::RUNTIME_NEAR_M,
            clip_far: spec.camera_far_m().unwrap_or(600.0).max(1.0),
            authored: true,
        }
    }

    /// A lens with the constructor-default follow dynamics around a
    /// given boom — the shape shared by the size-derived fallback and
    /// [`ChaseCamera::default`].
    fn with_ctor_dynamics(offset: Vec3, aim: Vec3) -> Self {
        Self {
            offset,
            aim,
            dist_min: 0.0,
            dist_max: 0.0,
            speed_min: ctor::MIN_SPEED,
            speed_max: ctor::MAX_SPEED,
            app_xz_max: ctor::MAX_APP_XZ,
            app_xz_min: ctor::MIN_APP_XZ,
            app_inc: ctor::APP_INC,
            app_dec: ctor::APP_DEC,
            app_y: ctor::APP_Y,
            app_pos_min: ctor::APP_POS_MIN,
            app_app: Some(ctor::APP_APP),
            approach: true,
            app_xz_init: None,
            vert_offset: ctor::VERT_OFFSET,
            collide: false,
            fov_deg: PerspectiveProjection::default().fov.to_degrees(),
            clip_near: PerspectiveProjection::default().near,
            clip_far: PerspectiveProjection::default().far,
            authored: false,
        }
    }

    /// Designed boom sized from the chassis (`h`/`d` metres) — the
    /// pre-authored fallback for a vehicle without records: rest eye
    /// `d*0.85 + 3.5` back and `h*0.10 + 1.4` above the aim point `h*0.45`,
    /// following with the constructor-default dynamics.
    pub fn sized(h: f32, d: f32) -> Self {
        Self::with_ctor_dynamics(
            Vec3::new(0.0, h * 0.10 + 1.4, d * 0.85 + 3.5),
            Vec3::new(0.0, h * 0.45, 0.0),
        )
    }

    /// The rest eye position in car space for an upright car: `Offset`
    /// hung off the `aim` point.
    pub fn anchor(&self) -> Vec3 {
        self.aim + self.offset
    }

    /// The perspective projection this lens asks for.
    pub fn projection(&self) -> PerspectiveProjection {
        PerspectiveProjection {
            fov: self.fov_deg.to_radians(),
            near: self.clip_near.max(0.01),
            far: self.clip_far.max(1.0),
            ..default()
        }
    }
}

/// Live follow state of the chase eye — what the original keeps in
/// `AppXZPos` and the per-axis approach rate states (UNK-36).
#[derive(Debug, Clone, Copy, Default)]
pub struct FollowState {
    /// The live horizontal follow rate (1/s); `None` until the first
    /// tracked frame seeds it.
    pub app_xz: Option<f32>,
    /// Per-axis low-passed approach distance (`AppApp`).
    pub axis: Vec3,
}

/// The follow rate the car's speed asks for (`0x51eb20`, UNK-36):
/// `MaxAppXZPos` at/below `MinSpeed`, `MinAppXZPos` at/above `MaxSpeed`,
/// linear between (`MaxAppXZPos` when the window is empty or
/// `MinAppXZPos` is 0). `speed` is the car's |forward velocity| in m/s.
pub fn follow_rate_target(lens: &ChaseLens, speed: f32) -> f32 {
    if lens.app_xz_min == 0.0 {
        return lens.app_xz_max;
    }
    let t = if lens.speed_max > lens.speed_min {
        ((speed - lens.speed_min) / (lens.speed_max - lens.speed_min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    lens.app_xz_max + (lens.app_xz_min - lens.app_xz_max) * t
}

/// Slew the live follow rate toward `target` at `inc` (rising) or `dec`
/// (falling) per second, never passing it (`0x51eb20`, UNK-36).
pub fn slew_follow_rate(cur: f32, target: f32, inc: f32, dec: f32, dt: f32) -> f32 {
    if cur < target {
        (cur + inc * dt).min(target)
    } else {
        (cur - dec * dt).max(target)
    }
}

/// The aim point and desired eye (`0x51e3e0`, UNK-36). `T = TrackTo ·
/// carMatrix`; with `b` the car's rearward axis flattened onto the
/// ground and `r = up × b`, `eye = T + r·Offset.x + b·max(Offset.z,
/// 0.01) + (0, Offset.y, 0)` — a yaw-only frame, so car pitch and roll
/// never tilt the boom. A car standing on its nose or tail (no ground
/// heading) falls back to the world +Z axis.
pub fn desired_eye(veh_pos: Vec3, veh_rot: Quat, lens: &ChaseLens) -> (Vec3, Vec3) {
    let t = veh_pos + veh_rot * lens.aim;
    let back = veh_rot * Vec3::Z;
    let b = Vec3::new(back.x, 0.0, back.z)
        .try_normalize()
        .unwrap_or(Vec3::Z);
    let r = Vec3::Y.cross(b);
    let eye =
        t + r * lens.offset.x + b * lens.offset.z.max(0.01) + Vec3::new(0.0, lens.offset.y, 0.0);
    (t, eye)
}

/// One axis of the eye approach (`0x522860`, UNK-36): distance `d` to the
/// target, squared over `AppPosMin` inside the knee, low-passed into
/// `state` at `AppApp`, then a step of `state · rate · dt` toward the
/// target that never overshoots. The original steps at its fixed 1/60 s;
/// the low-pass is applied as `1 − (1 − AppApp)^(60·dt)` so the result is
/// frame-rate independent and identical at 60 Hz.
pub fn approach_axis(
    eye: f32,
    target: f32,
    state: &mut f32,
    rate: f32,
    lens: &ChaseLens,
    dt: f32,
) -> f32 {
    let gap = target - eye;
    let mut d = gap.abs();
    if d < lens.app_pos_min {
        d = d * d / lens.app_pos_min;
    }
    *state = match lens.app_app {
        Some(a) => *state + (d - *state) * (1.0 - (1.0 - a).powf(60.0 * dt)),
        None => d,
    };
    let step = (*state * rate * dt).min(gap.abs());
    eye + step * gap.signum()
}

/// The authored chase-lens pair for one vehicle — `_near` and `_far`
/// `camTrackCS` records resolved through the VFS. An absent record
/// stays `None`: the far slot is authored-only, never fabricated.
pub struct TrackCams {
    /// `tune/camera/<car>_near.camtrackcs`.
    pub near: Option<TrackCamSpec>,
    /// `tune/camera/<car>_far.camtrackcs`.
    pub far: Option<TrackCamSpec>,
}

/// Read both authored chase-lens records for `car`.
pub fn load_track_cams(vfs: &Vfs, car: &str) -> TrackCams {
    let read = |suffix: &str| {
        let path = format!("tune/camera/{car}_{suffix}.camtrackcs");
        let spec = vfs
            .read_path(&path)
            .ok()
            .and_then(|(bytes, _)| TrackCamSpec::parse(&String::from_utf8_lossy(&bytes)).ok())?;
        for issue in spec.validate() {
            warn!(path = %path, issue = %issue, "camtrackcs spec issue");
        }
        Some(spec)
    };
    TrackCams {
        near: read("near"),
        far: read("far"),
    }
}

/// Smoke-record report of which chase lenses bound authored records
/// (F22-B.3). Inserted only when a stock vehicle def is selected so
/// dev-world records stay bit-identical.
#[derive(Resource, Default, Debug)]
pub struct TrackReport {
    /// `_near.camtrackcs` bound (vs the designed size fallback).
    pub near_authored: bool,
    /// `_far.camtrackcs` bound — this also gates the `C`-chain far slot.
    pub far_authored: bool,
}

impl TrackReport {
    /// `near+far` / `near` / `far` / `sized`.
    pub fn smoke_detail(&self) -> String {
        match (self.near_authored, self.far_authored) {
            (true, true) => "near+far".into(),
            (true, false) => "near".into(),
            (false, true) => "far".into(),
            (false, false) => "sized".into(),
        }
    }
}

/// Chase camera rig on the camera entity: the near lens plus an
/// optional far lens. A vehicle without authored records gets a
/// designed size-derived near lens and no far slot.
#[derive(Component)]
pub struct ChaseCamera {
    /// Chase-near lens (`CameraMode::Chase`).
    pub near: ChaseLens,
    /// Chase-far lens (`CameraMode::ChaseFar`) — `Some` only when the
    /// vehicle's `_far.camtrackcs` bound.
    pub far: Option<ChaseLens>,
    /// The live follow rate and per-axis approach state (UNK-36).
    pub follow: FollowState,
    /// Last vehicle position the boom tracked. A jump the frame's
    /// delta cannot explain — a `ResetVehicle` teleport (`R`, a
    /// water/stuck/disabled recovery, a scripted re-anchor) or a
    /// return to a chase mode after the car drove on under another
    /// view — must snap the boom rather than lerp a straight line
    /// across the world through whatever stands between the two poses
    /// (reset transitions, spec req 5; designed — UNK-36). `None`
    /// until the first tracked frame, which always snaps.
    pub last_pos: Option<Vec3>,
}

impl ChaseCamera {
    /// The lens the given mode drives — `ChaseFar` falls back to the
    /// near lens rather than a missing camera.
    pub fn lens(&self, mode: CameraMode) -> &ChaseLens {
        match mode {
            CameraMode::ChaseFar => self.far.as_ref().unwrap_or(&self.near),
            _ => &self.near,
        }
    }
}

impl Default for ChaseCamera {
    fn default() -> Self {
        Self {
            near: ChaseLens::with_ctor_dynamics(Vec3::new(0.0, 3.0, 7.5), Vec3::new(0.0, 1.0, 0.0)),
            far: None,
            follow: FollowState::default(),
            last_pos: None,
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

/// Does a session camera exist for `mode`? The far slot is
/// authored-only: a chase rig without a `_far` lens has no second
/// view to activate.
fn have_mode(m: CameraMode, cams: &SessionCameras) -> bool {
    cams.iter().any(|(_, c, p, f)| match m {
        CameraMode::Chase => c.is_some(),
        CameraMode::ChaseFar => c.is_some_and(|c| c.far.is_some()),
        CameraMode::Cockpit => p.is_some(),
        CameraMode::Free => f.is_some(),
    })
}

/// The `C`-chain successor that is actually spawned, skipping modes
/// whose camera never bound. `None` when no recognised session camera
/// exists at all (the menu phase or an empty world) — cycling there
/// would only drift the mode away from whatever cameras a later
/// session spawns.
fn next_available(mode: CameraMode, cams: &SessionCameras) -> Option<CameraMode> {
    let mut m = mode.next();
    for _ in 0..4 {
        if have_mode(m, cams) {
            return Some(m);
        }
        m = m.next();
    }
    None
}

/// The activation pass a mode switch performs: each recognised
/// session camera renders only while its mode is live, and Free
/// grabs/hides the cursor. Cameras carrying none of the three markers
/// (map, UI) are owned elsewhere and never touched.
fn activate_mode(
    mode: CameraMode,
    cams: &mut SessionCameras,
    cursor: &mut Query<&mut CursorOptions>,
) {
    for (mut cam, chase, pov, free) in cams.iter_mut() {
        let active = match mode {
            CameraMode::Chase | CameraMode::ChaseFar => chase.is_some(),
            CameraMode::Cockpit => pov.is_some(),
            CameraMode::Free => free.is_some(),
        };
        // Only claim the recognised session cameras — an unmarked one
        // (map, UI) is owned elsewhere.
        if chase.is_some() || pov.is_some() || free.is_some() {
            cam.is_active = active;
        }
    }
    for mut opts in cursor.iter_mut() {
        let free = mode == CameraMode::Free;
        opts.grab_mode = if free {
            CursorGrabMode::Locked
        } else {
            CursorGrabMode::None
        };
        opts.visible = !free;
    }
}

/// `C` cycles the HUD-3 view chain — Chase Near → Cockpit → Chase Far,
/// with the dev Free camera appended (DSN-48); `V` is the dashboard
/// toggle — it jumps straight into or out of the cockpit view. (The
/// original's dash key was `D`, but enhanced input put steering on
/// `A`/`D` — the binding moves, the behavior stays; HUD-3, DSN-48.) A
/// mode whose camera was never spawned (no authored `camPovCS`, no
/// `_far.camtrackcs` lens) is skipped so the key never dead-ends on a
/// black screen.
///
/// Activation is marker-driven: each `Camera` carries exactly one of
/// `ChaseCamera`/`CockpitCamera`/`FreeCamera`, and only the matching
/// one is enabled. Cameras carrying none of them (the HUD-map camera,
/// overlays) are left to their own owners — previously a `C` press
/// flipped the map camera's `is_active` for a frame.
pub fn toggle_camera(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    windows: Query<&Window>,
    controls: Option<Res<crate::controls::ControlSettings>>,
    mut mode: ResMut<CameraMode>,
    mut cams: SessionCameras,
    mut cursor: Query<&mut CursorOptions>,
) {
    use crate::controls::DriveAction;
    let pressed =
        |action| control_just_pressed(&keys, &pads, &windows, controls.as_deref(), action);
    let next = if pressed(DriveAction::Camera) {
        match next_available(*mode, &cams) {
            Some(m) => m,
            None => return,
        }
    } else if pressed(DriveAction::Cockpit) {
        match *mode {
            CameraMode::Cockpit => CameraMode::Chase,
            _ if have_mode(CameraMode::Cockpit, &cams) => CameraMode::Cockpit,
            _ => return,
        }
    } else {
        return;
    };
    if next == *mode {
        return;
    }
    *mode = next;
    activate_mode(next, &mut cams, &mut cursor);
}

/// `--cam-cycle-at TICK` (quarantined `DevOverrides`, evidence runs
/// only): advance the documented `C` chain once — the same
/// successor/activation logic [`toggle_camera`] runs for the key —
/// when the session clock reaches `cam_cycle_at` fixed ticks (60 Hz,
/// the `smoke` record's `ticks=` unit). A `--frames`/`--screenshot`
/// capture freezes live input, so this is how a mid-drive view
/// transition gets inspected on real content. Render-only like
/// `--cockpit`/`--far`: it re-aims a camera and cannot change a run's
/// outcome, so it stays out of `record_eligibility`. One-shot.
pub fn dev_cam_cycle_at(
    session: Res<Session>,
    mut mode: ResMut<CameraMode>,
    mut cams: SessionCameras,
    mut cursor: Query<&mut CursorOptions>,
    mut fired: Local<bool>,
) {
    if *fired {
        return;
    }
    let at = session.config().and_then(|c| c.dev.cam_cycle_at);
    if session.is_playing()
        && at.is_some_and(|at| session.tick() >= at)
        && let Some(next) = next_available(*mode, &cams)
    {
        *fired = true;
        if next != *mode {
            *mode = next;
            activate_mode(next, &mut cams, &mut cursor);
        }
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
    With<crate::hudmap::HudMapFrame>,
    With<crate::session::Hud>,
    With<crate::speedometer::Speedometer>,
    With<crate::session::ErrorText>,
    With<crate::pause::PauseUi>,
    With<crate::results::ResultsUi>,
    With<crate::race::CountdownBanner>,
    With<crate::navarrow::NavArrow>,
    With<crate::navarrow3d::NavArrowView>,
    With<crate::race::LowTimeWarning>,
    With<crate::racetime::RaceTimer>,
    With<crate::racestat::RaceStats>,
    With<crate::cnrhud::CnrScoreboard>,
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

/// Occlusion pull-in margin — the boom stops this far short of the
/// wall the ray found, and never lands closer than the floor to the
/// aim point. Designed constants (the record carries no margin).
const OCCLUSION_MARGIN: f32 = 0.25;
const OCCLUSION_FLOOR: f32 = 0.05;

/// Apparent vehicle speed (m/s) beyond which the boom concludes the
/// car teleported — a `ResetVehicle` reset/recovery jump or a re-entry
/// to a chase mode after the car drove on under another view — and
/// snaps instead of sweeping a straight line across the world through
/// whatever stands between the two poses. Compared as a rate so a
/// render hitch's long delta still legitimately covers more ground;
/// ~80 m/s is the fastest authored top speed (vppanozgt `Trans.High`
/// 180 mph), so the margin is wide. Designed — the original's
/// reset/transition camera behavior is unrecovered (UNK-36).
const BOOM_SNAP_SPEED: f32 = 120.0;

/// Chase follow on the active chase lens (recovered `camTrackCS`
/// runtime, UNK-36): the desired eye is `Offset` in the car's yaw-only
/// frame around the `TrackTo` aim point, the live follow rate lerps with
/// forward speed and slews at `AppInc`/`AppDec`, the eye approaches per
/// axis (`AppXZPos` for X/Z, `AppYPos` for Y) through the `AppPosMin`
/// knee and `AppApp` low-pass, and `[MinDist, MaxDist]` clamps the
/// eye–aim distance afterwards. The camera looks at `T + (0, LookAbove,
/// 0)`; the original's eased orientation approach (`AppRot`/`AppXRot`) is
/// inferred, not recovered, so the look is applied directly. The
/// projection follows the lens's `CameraFOV`/`CameraNear`/`CameraFar`,
/// and `CollideType` pulls the camera in front of occluding geometry.
pub fn chase_follow(
    time: Res<Time>,
    mode: Res<CameraMode>,
    spatial: Option<SpatialQuery>,
    spawn: Option<Res<crate::session::SpawnPoint>>,
    settings: Option<Res<crate::settings::GraphicsSettings>>,
    mut cams: Query<(&mut ChaseCamera, &mut Transform, &mut Projection)>,
    vehicle: Query<(Entity, &GlobalTransform, &LinearVelocity), With<PlayerVehicle>>,
) {
    if !matches!(*mode, CameraMode::Chase | CameraMode::ChaseFar) {
        return;
    }
    let Ok((veh_ent, veh_xf, vel)) = vehicle.single() else {
        return;
    };
    let veh_pos = veh_xf.translation();
    let veh_rot = veh_xf.rotation();
    // `|forward velocity|` (`carsim+0x248`): the car's −Z axis.
    let speed = vel.dot(veh_rot * Vec3::NEG_Z).abs();
    let dt = time.delta_secs();
    for (mut cam, mut xf, mut proj) in &mut cams {
        // A jump the frame delta cannot explain — a reset teleport or
        // a stale tracker after another camera mode ran — snaps to the
        // target: easing would sweep the view across the world
        // through walls. A lens swap is *not* a jump (the vehicle
        // didn't move), so near↔far stays a smooth boom transition.
        let jumped = cam
            .last_pos
            .is_none_or(|p| veh_pos.distance(p) > BOOM_SNAP_SPEED * dt);
        cam.last_pos = Some(veh_pos);
        let lens = cam.lens(*mode).clone();
        // The lens owns the projection — write-on-diff so toggling
        // near↔far swaps the authored FOV/clips without churning change
        // detection every frame.
        let mut want = lens.projection();
        if let Some(settings) = &settings {
            want.fov = settings.field_of_view.widen(want.fov);
        }
        let stale = match &*proj {
            Projection::Perspective(p) => {
                p.fov != want.fov || p.near != want.near || p.far != want.far
            }
            _ => true,
        };
        if stale {
            *proj = Projection::Perspective(want);
        }

        let (aim, desired) = desired_eye(veh_pos, veh_rot, &lens);
        let rate_target = follow_rate_target(&lens, speed);
        let rate = match cam.follow.app_xz {
            Some(cur) if !jumped => {
                slew_follow_rate(cur, rate_target, lens.app_inc, lens.app_dec, dt)
            }
            None if !jumped => lens.app_xz_init.unwrap_or(rate_target),
            _ => rate_target,
        };
        cam.follow.app_xz = Some(rate);
        let mut next = if jumped || !lens.approach {
            cam.follow.axis = Vec3::ZERO;
            desired
        } else {
            let eye = xf.translation;
            let mut st = cam.follow.axis;
            let out = Vec3::new(
                approach_axis(eye.x, desired.x, &mut st.x, rate, &lens, dt),
                approach_axis(eye.y, desired.y, &mut st.y, lens.app_y, &lens, dt),
                approach_axis(eye.z, desired.z, &mut st.z, rate, &lens, dt),
            );
            cam.follow.axis = st;
            out
        };
        // `[MinDist, MaxDist]` hard-clamps the eye–aim distance after
        // the approach, ungated by `MinMaxOn` (UNK-36).
        if lens.dist_max > 0.0 && lens.dist_max > lens.dist_min {
            let from = next - aim;
            let len = from.length();
            if len > 1e-6 {
                next = aim + from * (len.clamp(lens.dist_min, lens.dist_max) / len);
            }
        }
        let look = aim;
        let look_target = aim + Vec3::new(0.0, (lens.offset.y - 0.8) * lens.vert_offset, 0.0);

        // CollideType: clamp the smoothed position in front of whatever
        // would occlude the car. Clamping the *smoothed* candidate —
        // not the far target — keeps the pull-in immediate (no lagging
        // through a wall) while expansion still eases back out.
        if lens.collide
            && let Some(spatial) = &spatial
        {
            let seg = next - look;
            let len = seg.length();
            if len > 1e-3
                && let Ok(d) = Dir3::new(seg)
            {
                // The player's own rig never occludes itself: a towed
                // trailer sits between the cab and the authored boom
                // (vpsemi's `_near` anchor lands *inside* its trailer
                // box), so counting it would park the camera in the
                // hitch gap or behind the trailer's rear wall. Other
                // vehicles' trailers still occlude like any world
                // object (designed reading — UNK-36).
                let filter = SpatialQueryFilter::from_excluded_entities(
                    std::iter::once(veh_ent).chain(
                        spawn
                            .as_deref()
                            .into_iter()
                            .flat_map(|s| s.trailers.iter().map(|(e, _)| *e)),
                    ),
                );
                if let Some(hit) = spatial.cast_ray(look, d, len, true, &filter) {
                    next = look + d * (hit.distance - OCCLUSION_MARGIN).max(OCCLUSION_FLOOR);
                }
            }
        }

        xf.translation = next;
        xf.look_at(look_target, Vec3::Y);
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
        .and_then(|p| p.offset_vec())
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
                // `camera_fov_deg` reads an undrawable `CameraFOV` as
                // unauthored — the designed 60° stands in.
                fov: pov
                    .and_then(|p| p.camera_fov_deg())
                    .unwrap_or(60.0)
                    .to_radians(),
                near: pov.and_then(|p| p.camera_near_m()).unwrap_or(0.1).max(0.01),
                far: pov.and_then(|p| p.camera_far_m()).unwrap_or(600.0).max(1.0),
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
    pads: Query<&Gamepad>,
    windows: Query<&Window>,
    session: Res<Session>,
    controls: Option<Res<crate::controls::ControlSettings>>,
    mut mirror: ResMut<RearView>,
) {
    if !matches!(
        session.phase(),
        SessionPhase::Playing | SessionPhase::Countdown
    ) {
        return;
    }
    if control_just_pressed(
        &keys,
        &pads,
        &windows,
        controls.as_deref(),
        crate::controls::DriveAction::Mirror,
    ) {
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
    let size = window.single().ok().map(|w| {
        UVec2::new(
            w.resolution.physical_width(),
            w.resolution.physical_height(),
        )
    });
    // A minimized client reports a 0x0 window: there is nothing to
    // draw into, and a 1x1 strip would overrun the target. The strip
    // sleeps and re-arms from `RearView` when the window returns.
    let minimized = window.single().is_ok_and(crate::input::window_minimized);
    let want = mirror.0 && *mode != CameraMode::Free && !minimized;
    let viewport = size.filter(|_| !minimized).map(|size| {
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
