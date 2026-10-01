//! F22-B.3 authored `camTrackCS` chase-rig coverage: the documented
//! HUD-3 chain (Chase Near → Cockpit → Chase Far) with the dev Free
//! camera appended, lens-driven boom/projection, the `CollideType`
//! occlusion pull-in, and VFS loading of the `_near`/`_far` records.

use avian3d::prelude::{Collider, Gravity, LinearVelocity, PhysicsPlugins, RigidBody};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::camera::{
    CameraMode, ChaseCamera, ChaseLens, FreeCamera, TrackReport, chase_follow, dev_cam_cycle_at,
    load_track_cams, toggle_camera,
};
use mm2_app::dash::CockpitCamera;
use mm2_app::input::vehicle_input;
use mm2_app::session::SpawnPoint;
use mm2_assets::Vfs;
use mm2_formats::camtrack::TrackCamSpec;
use mm2_game::{DevOverrides, PlayerVehicle, Session, SessionConfig, SessionPhase};
use mm2_vehicle::VehicleInput;
use std::time::Duration;

const NEAR_TEXT: &str = "type: a\ncamTrackCS {\n  Offset 0.0 1.0 4.0\n  CollideType 1\n  MinMaxOn 1\n  MinDist 3.5\n  MaxDist 5.0\n  MinSpeed 0.0\n  MaxSpeed 20.0\n  TrackTo 0.0 1.7 0.0\n  BlendTime 1.2\n  CameraFOV 70.0\n  CameraNear 0.5\n  CameraFar 600.0\n}\n";

const FAR_TEXT: &str = "type: a\ncamTrackCS {\n  Offset 0.0 1.8 8.0\n  CollideType 1\n  MinMaxOn 1\n  MinDist 4.0\n  MaxDist 10.0\n  MinSpeed 5.0\n  MaxSpeed 35.0\n  TrackTo 0.0 1.3 0.0\n  CameraFOV 65.0\n  CameraNear 0.5\n  CameraFar 400.0\n}\n";

fn near_lens() -> ChaseLens {
    ChaseLens::authored(&TrackCamSpec::parse(NEAR_TEXT).unwrap())
}

fn far_lens() -> ChaseLens {
    ChaseLens::authored(&TrackCamSpec::parse(FAR_TEXT).unwrap())
}

fn base_app(mode: CameraMode) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(TransformPlugin)
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .init_resource::<ButtonInput<KeyCode>>()
        .insert_resource(mode);
    app
}

fn press(app: &mut App, key: KeyCode) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(key);
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .reset_all();
}

/// The first connected pad: one `Gamepad` entity whose `digital_mut`
/// bevy documents as the mocking surface for gamepad input.
fn spawn_pad(app: &mut App) {
    app.world_mut().spawn(Gamepad::default());
}

/// One pad-button edge, `press`-equivalent: press for exactly one
/// update, then `reset_all` ends it like the event loop would.
fn pad_press(app: &mut App, button: GamepadButton) {
    let mut pads = app.world_mut().query::<&mut Gamepad>();
    for mut pad in pads.iter_mut(app.world_mut()) {
        pad.digital_mut().press(button);
    }
    app.update();
    let mut pads = app.world_mut().query::<&mut Gamepad>();
    for mut pad in pads.iter_mut(app.world_mut()) {
        pad.digital_mut().reset_all();
    }
}

fn spawn_vehicle(app: &mut App, pos: Vec3, vel: Vec3) -> Entity {
    app.world_mut()
        .spawn((
            PlayerVehicle,
            RigidBody::Dynamic,
            Collider::cuboid(2.0, 1.2, 4.0),
            LinearVelocity(vel),
            Transform::from_translation(pos),
        ))
        .id()
}

fn spawn_chase(app: &mut App, cam: ChaseCamera, active: bool) -> Entity {
    app.world_mut()
        .spawn((
            Camera3d::default(),
            Camera {
                is_active: active,
                ..default()
            },
            cam,
            Transform::from_xyz(0.0, 4.0, 9.0),
        ))
        .id()
}

/// The documented HUD-3 chain reaches the authored far lens; Free sits
/// behind it as the dev extension.
#[test]
fn chain_reaches_far_when_bound() {
    let mut app = base_app(CameraMode::Chase);
    app.add_systems(Update, toggle_camera);
    let chase = spawn_chase(
        &mut app,
        ChaseCamera {
            near: near_lens(),
            far: Some(far_lens()),
            ..default()
        },
        true,
    );
    app.world_mut().spawn((
        Camera3d::default(),
        Camera::default(),
        FreeCamera::default(),
    ));

    press(&mut app, KeyCode::KeyC);
    assert_eq!(
        *app.world().resource::<CameraMode>(),
        CameraMode::ChaseFar,
        "Cockpit absent → the chain lands on the authored far lens"
    );
    assert!(app.world().get::<Camera>(chase).unwrap().is_active);

    press(&mut app, KeyCode::KeyC);
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Free);
    assert!(!app.world().get::<Camera>(chase).unwrap().is_active);

    press(&mut app, KeyCode::KeyC);
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Chase);
    assert!(app.world().get::<Camera>(chase).unwrap().is_active);
}

/// A rig with no `_far` record keeps the slot closed: the chain reads
/// Chase → (Cockpit skipped) → Free, never dead-ending on ChaseFar.
#[test]
fn chain_skips_far_when_unbound() {
    let mut app = base_app(CameraMode::Chase);
    app.add_systems(Update, toggle_camera);
    spawn_chase(&mut app, ChaseCamera::default(), true);
    app.world_mut().spawn((
        Camera3d::default(),
        Camera::default(),
        FreeCamera::default(),
    ));

    press(&mut app, KeyCode::KeyC);
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Free);
    press(&mut app, KeyCode::KeyC);
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Chase);
}

/// Near → Cockpit → Far → Free ordering with all four cameras present.
#[test]
fn chain_orders_all_four_views() {
    let mut app = base_app(CameraMode::Chase);
    app.add_systems(Update, toggle_camera);
    spawn_chase(
        &mut app,
        ChaseCamera {
            near: near_lens(),
            far: Some(far_lens()),
            ..default()
        },
        true,
    );
    app.world_mut().spawn((
        Camera3d::default(),
        Camera::default(),
        CockpitCamera {
            offset: Vec3::ZERO,
            reverse_offset: None,
            pitch: 0.0,
            look_yaw: 0.0,
        },
    ));
    app.world_mut().spawn((
        Camera3d::default(),
        Camera::default(),
        FreeCamera::default(),
    ));

    let mut seen = Vec::new();
    for _ in 0..4 {
        press(&mut app, KeyCode::KeyC);
        seen.push(*app.world().resource::<CameraMode>());
    }
    assert_eq!(
        seen,
        [
            CameraMode::Cockpit,
            CameraMode::ChaseFar,
            CameraMode::Free,
            CameraMode::Chase
        ]
    );
}

/// F22-AC06 pad leg (designed `input::pad` map): the right stick click
/// walks the same `C` chain, and `West` is the `V` cockpit toggle —
/// the pad shares the key's path verbatim.
#[test]
fn pad_walks_the_same_chain() {
    let mut app = base_app(CameraMode::Chase);
    app.add_systems(Update, toggle_camera);
    spawn_chase(&mut app, ChaseCamera::default(), true);
    app.world_mut().spawn((
        Camera3d::default(),
        Camera::default(),
        CockpitCamera {
            offset: Vec3::ZERO,
            reverse_offset: None,
            pitch: 0.0,
            look_yaw: 0.0,
        },
    ));
    app.world_mut().spawn((
        Camera3d::default(),
        Camera::default(),
        FreeCamera::default(),
    ));
    spawn_pad(&mut app);

    // No far lens: the chain reads Chase → Cockpit → Free like C does.
    pad_press(&mut app, GamepadButton::RightThumb);
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Cockpit);
    pad_press(&mut app, GamepadButton::RightThumb);
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Free);
    pad_press(&mut app, GamepadButton::RightThumb);
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Chase);

    // `West` is the V shortcut: straight into the cockpit and back.
    pad_press(&mut app, GamepadButton::West);
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Cockpit);
    pad_press(&mut app, GamepadButton::West);
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Chase);
}

/// The active lens owns the boom and the projection.
#[test]
fn authored_lens_drives_boom_and_projection() {
    let mut app = base_app(CameraMode::Chase);
    app.add_systems(Update, chase_follow);
    spawn_vehicle(&mut app, Vec3::ZERO, Vec3::ZERO);
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: near_lens(),
            far: Some(far_lens()),
            ..default()
        },
        true,
    );

    for _ in 0..240 {
        app.update();
    }
    // Rest boom = |Offset| = √(1²+4²) ≈ 4.12 once the smoothing
    // converges.
    let xf = app.world().get::<Transform>(cam).unwrap();
    assert!(
        (xf.translation.length() - (1.0f32 + 16.0).sqrt()).abs() < 0.15,
        "near rest boom ≈ authored |Offset|, got {}",
        xf.translation.length()
    );
    let Projection::Perspective(p) = app.world().get::<Projection>(cam).unwrap() else {
        panic!("chase camera keeps a perspective projection");
    };
    assert!((p.fov.to_degrees() - 70.0).abs() < 0.5, "authored near FOV");
    assert!((p.near - 0.5).abs() < 1e-4, "authored near clip");
    assert!((p.far - 600.0).abs() < 1.0, "authored far clip");

    // ChaseFar swaps to the far lens — boom and FOV both move.
    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::ChaseFar;
    for _ in 0..240 {
        app.update();
    }
    let xf = app.world().get::<Transform>(cam).unwrap();
    assert!(
        (xf.translation.length() - (1.8f32 * 1.8 + 64.0).sqrt()).abs() < 0.15,
        "far rest boom ≈ authored |Offset|, got {}",
        xf.translation.length()
    );
    let Projection::Perspective(p) = app.world().get::<Projection>(cam).unwrap() else {
        panic!();
    };
    assert!(
        (p.fov.to_degrees() - 65.0).abs() < 0.5,
        "authored far FOV, got {}",
        p.fov.to_degrees()
    );
}

/// `MinSpeed`..`MaxSpeed` extends the boom toward `MaxDist`.
#[test]
fn speed_window_extends_boom() {
    let mut app = base_app(CameraMode::Chase);
    app.add_systems(Update, chase_follow);
    // At MaxSpeed (20 m/s, forward −Z) the boom reaches MaxDist 5.0.
    spawn_vehicle(&mut app, Vec3::ZERO, Vec3::new(0.0, 0.0, -20.0));
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: near_lens(),
            far: None,
            smoothness: 30.0,
            ..Default::default()
        },
        true,
    );
    for _ in 0..240 {
        app.update();
    }
    let xf = app.world().get::<Transform>(cam).unwrap();
    assert!(
        (xf.translation.length() - 5.0).abs() < 0.1,
        "boom at MaxDist under full speed, got {}",
        xf.translation.length()
    );
}

/// `CollideType 1`: geometry between the aim point and the boom pulls
/// the camera in front of it instead of letting the wall occlude the
/// car.
fn physics_app() -> App {
    // AssetPlugin+MeshPlugin initialize the `AssetEvent<Mesh>` queue
    // avian's collider cache drains each update (the banger harness
    // carries the same set); without them `PhysicsPlugins` panics.
    let mut app = base_app(CameraMode::Chase);
    app.add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Gravity(Vec3::ZERO))
        .insert_resource(Time::<Fixed>::from_hz(120.0));
    app.finish();
    app.cleanup();
    app
}

#[test]
fn collide_type_pulls_boom_in() {
    let mut app = physics_app();
    app.add_systems(Update, chase_follow);
    spawn_vehicle(&mut app, Vec3::ZERO, Vec3::ZERO);
    // Wall 2 m behind the car (rearward is +Z at identity yaw), well
    // inside the authored 4.12 m rest boom.
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(8.0, 8.0, 0.4),
        Transform::from_xyz(0.0, 1.0, 2.0),
    ));
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: near_lens(),
            far: None,
            smoothness: 60.0,
            ..Default::default()
        },
        true,
    );

    for _ in 0..120 {
        app.update();
    }
    let xf = app.world().get::<Transform>(cam).unwrap();
    // The ray runs from the aim point toward the boom: the camera must
    // sit on the near side of the wall, not behind it at the authored
    // distance.
    assert!(
        xf.translation.z < 2.0,
        "boom pulled in front of the wall, z={}",
        xf.translation.z
    );
}

/// A `CollideType 0` rig is not pulled in — the flag is authored, not
/// assumed.
#[test]
fn collide_type_zero_ignores_occluder() {
    let mut app = physics_app();
    app.add_systems(Update, chase_follow);
    spawn_vehicle(&mut app, Vec3::ZERO, Vec3::ZERO);
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(8.0, 8.0, 0.4),
        Transform::from_xyz(0.0, 1.0, 2.0),
    ));
    let mut spec = TrackCamSpec::parse(NEAR_TEXT).unwrap();
    spec.collide_type = Some(0.0);
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: ChaseLens::authored(&spec),
            far: None,
            smoothness: 60.0,
            ..Default::default()
        },
        true,
    );
    for _ in 0..120 {
        app.update();
    }
    let xf = app.world().get::<Transform>(cam).unwrap();
    assert!(
        xf.translation.z > 3.0,
        "CollideType 0 keeps the authored boom, z={}",
        xf.translation.z
    );
}

/// The player's own trailer is part of the rig, not an occluder: a
/// trailered vehicle's boom anchor can sit *inside* the trailer box
/// (vpsemi's `_near` does on retail), so counting the trailer would
/// park the camera in the hitch gap.
#[test]
fn own_trailer_is_not_an_occluder() {
    let mut app = physics_app();
    app.add_systems(Update, chase_follow);
    spawn_vehicle(&mut app, Vec3::ZERO, Vec3::ZERO);
    // A trailer box straddling the boom ray (front face at z=3, well
    // inside the authored 4.12 m rest boom).
    let trailer = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            Collider::cuboid(4.0, 4.0, 4.0),
            Transform::from_xyz(0.0, 2.0, 5.0),
        ))
        .id();
    app.insert_resource(SpawnPoint {
        trailers: vec![(trailer, Vec3::new(0.0, -0.2, 8.0))],
        ..SpawnPoint::new(Vec3::ZERO, 0.0)
    });
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: near_lens(),
            far: None,
            smoothness: 60.0,
            ..Default::default()
        },
        true,
    );
    for _ in 0..120 {
        app.update();
    }
    let xf = app.world().get::<Transform>(cam).unwrap();
    assert!(
        xf.translation.z > 3.4,
        "own trailer excluded: boom reaches the authored anchor, z={}",
        xf.translation.z
    );
}

/// A trailer that is *not* the player's own still occludes like any
/// world object — the exclusion is scoped to the local rig.
#[test]
fn other_trailer_still_occludes() {
    let mut app = physics_app();
    app.add_systems(Update, chase_follow);
    spawn_vehicle(&mut app, Vec3::ZERO, Vec3::ZERO);
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(4.0, 4.0, 4.0),
        Transform::from_xyz(0.0, 2.0, 5.0),
    ));
    // SpawnPoint exists but lists no trailer for this entity.
    app.insert_resource(SpawnPoint::new(Vec3::ZERO, 0.0));
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: near_lens(),
            far: None,
            smoothness: 60.0,
            ..Default::default()
        },
        true,
    );
    for _ in 0..120 {
        app.update();
    }
    let xf = app.world().get::<Transform>(cam).unwrap();
    assert!(
        xf.translation.z < 3.0,
        "unregistered trailer still pulls the boom in, z={}",
        xf.translation.z
    );
}

/// Every drive view steers — the F22-B.3 gate widened from `Chase` to
/// every non-Free mode, and ChaseFar must keep throttle/steering live
/// (a `C` press into the far lens must not strand the driver).
#[test]
fn drive_views_steer_and_free_detaches() {
    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();

    let mut app = base_app(CameraMode::ChaseFar);
    app.insert_resource(session)
        .add_systems(Update, vehicle_input);
    let car = app
        .world_mut()
        .spawn((PlayerVehicle, VehicleInput::default()))
        .id();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);

    for mode in [CameraMode::Chase, CameraMode::Cockpit, CameraMode::ChaseFar] {
        *app.world_mut().resource_mut::<CameraMode>() = mode;
        app.update();
        assert_eq!(
            app.world().get::<VehicleInput>(car).unwrap().throttle,
            1.0,
            "{mode:?} is a drive view"
        );
    }
    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::Free;
    app.update();
    assert_eq!(
        app.world().get::<VehicleInput>(car).unwrap().throttle,
        0.0,
        "Free detaches driving input"
    );
}

/// The designed size-derived lens is what a record-less vehicle gets:
/// chassis-derived rest boom, 0–60 m/s window toward `rest + 3.6`,
/// no collision flag and `authored = false` so `trk=` reports `sized`.
#[test]
fn sized_lens_drives_the_fallback_boom() {
    let lens = ChaseLens::sized(2.0, 4.5);
    let offset = Vec3::new(0.0, 2.0 * 0.55 + 1.4, 4.5 * 0.85 + 3.5);
    assert!((lens.offset - offset).length() < 1e-5);
    let rest = lens.offset.length();
    assert!((lens.dist_max - (rest + 3.6)).abs() < 1e-5);
    assert_eq!((lens.speed_min, lens.speed_max), (0.0, 60.0));
    assert!(!lens.collide && !lens.authored);

    let mut app = base_app(CameraMode::Chase);
    app.add_systems(Update, chase_follow);
    spawn_vehicle(&mut app, Vec3::ZERO, Vec3::ZERO);
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: lens,
            far: None,
            smoothness: 30.0,
            ..Default::default()
        },
        true,
    );
    for _ in 0..240 {
        app.update();
    }
    let xf = app.world().get::<Transform>(cam).unwrap();
    assert!(
        (xf.translation.length() - rest).abs() < 0.15,
        "sized lens converges to its rest boom, got {}",
        xf.translation.length()
    );
}

/// `load_track_cams` resolves both authored records through the VFS and
/// stays honest about a missing far lens.
#[test]
fn load_track_cams_binds_what_ships() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("tune/camera");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("vpnear_near.camtrackcs"), NEAR_TEXT).unwrap();
    std::fs::write(dir.join("vpnear_far.camtrackcs"), FAR_TEXT).unwrap();
    std::fs::write(dir.join("vpnonfar_near.camtrackcs"), NEAR_TEXT).unwrap();
    let mut vfs = Vfs::new();
    vfs.mount_dir(tmp.path(), 0).unwrap();

    let t = load_track_cams(&vfs, "vpnear");
    assert!(t.near.is_some() && t.far.is_some());
    assert_eq!(t.far.as_ref().unwrap().camera_fov.unwrap(), 65.0);

    let t = load_track_cams(&vfs, "vpnonfar");
    assert!(t.near.is_some());
    assert!(t.far.is_none(), "absent far record stays absent");

    let t = load_track_cams(&vfs, "vpnone");
    assert!(t.near.is_none() && t.far.is_none());
}

/// An authored `CameraFOV` outside the drawable `(0, 180)` range —
/// or non-finite — never reaches the projection: `camera_fov_deg`
/// reads it as unauthored and the designed 70° stands in, with the
/// record's `validate` naming the field for the loader's warning
/// (authored-numbers audit finding 11).
#[test]
fn undrawable_authored_fov_falls_back_to_the_designed_lens() {
    for bad in [f32::NAN, f32::INFINITY, 0.0, -30.0, 720.0] {
        let mut spec = TrackCamSpec::parse(NEAR_TEXT).unwrap();
        spec.camera_fov = Some(bad);
        assert_eq!(spec.validate().len(), 1, "{bad}");
        let lens = ChaseLens::authored(&spec);
        assert_eq!(lens.fov_deg, 70.0, "{bad}");
        assert!(lens.projection().fov.is_finite(), "{bad}");
    }
    // A drawable authored value still binds verbatim.
    let mut spec = TrackCamSpec::parse(NEAR_TEXT).unwrap();
    spec.camera_fov = Some(92.5);
    assert_eq!(ChaseLens::authored(&spec).fov_deg, 92.5);
}

/// The rest of the authored record follows the same contract — every
/// consumed field reads through the spec's finite-checked accessors,
/// so a `nan`/`inf` `Offset`/`TrackTo` can never poison the boom into
/// a NaN transform, a `nan` `CameraFar` cannot sink through `.max(1.0)`
/// into a 1 m far plane, and a `nan` flag reads off rather than
/// silently `!= 0.0` truthy. `validate` names each field for the
/// loader warning; the raw record stays verbatim.
#[test]
fn non_finite_authored_fields_fall_back_to_the_designed_boom() {
    let mut spec = TrackCamSpec::parse(NEAR_TEXT).unwrap();
    spec.offset = Some([0.0, f32::NAN, 4.0]);
    spec.track_to = Some([f32::INFINITY, 1.7, 0.0]);
    spec.min_dist = Some(f32::NAN);
    spec.max_dist = Some(f32::NEG_INFINITY);
    spec.min_speed = Some(f32::NAN);
    spec.max_speed = Some(f32::INFINITY);
    spec.camera_near = Some(f32::NAN);
    spec.camera_far = Some(f32::INFINITY);
    spec.collide_type = Some(f32::NAN);
    spec.min_max_on = Some(f32::NAN);
    let lens = ChaseLens::authored(&spec);
    let rest = Vec3::new(0.0, 1.8, 5.0).length();
    assert_eq!(
        lens.offset,
        Vec3::new(0.0, 1.8, 5.0),
        "non-finite Offset reads unauthored — the designed boom stands in"
    );
    assert_eq!(lens.aim, Vec3::new(0.0, 1.0, 0.0));
    assert_eq!(lens.dist_min, 0.0);
    assert_eq!(lens.dist_max, rest);
    assert_eq!(lens.speed_min, 0.0);
    assert_eq!(lens.speed_max, 0.0);
    assert!(!lens.collide);
    assert_eq!(lens.clip_near, 0.5);
    assert_eq!(lens.clip_far, 600.0);
    let p = lens.projection();
    assert!(p.fov.is_finite() && p.near.is_finite() && p.far.is_finite());

    // An astronomical-but-finite Offset overflows the boom's rest
    // length to `inf` — that reads unauthored too.
    let mut spec = TrackCamSpec::parse(NEAR_TEXT).unwrap();
    spec.offset = Some([3e38, 3e38, 3e38]);
    assert_eq!(
        ChaseLens::authored(&spec).offset,
        Vec3::new(0.0, 1.8, 5.0),
        "an Offset whose length overflows f32 still reads unauthored"
    );

    // And the tracker never produces a NaN transform from it.
    let mut app = base_app(CameraMode::Chase);
    app.add_systems(Update, chase_follow);
    spawn_vehicle(&mut app, Vec3::ZERO, Vec3::ZERO);
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: lens,
            far: None,
            ..Default::default()
        },
        true,
    );
    for _ in 0..60 {
        app.update();
    }
    let xf = app.world().get::<Transform>(cam).unwrap();
    assert!(
        xf.translation.is_finite() && xf.rotation.is_finite(),
        "boom from a hostile spec stays finite, got {xf:?}"
    );
}

/// The same contract covers finite-but-overflowing values — the
/// iteration-005 review residual: a `3e38` `TrackTo` has finite
/// components, yet `veh_rot * aim` can still reach `inf` inside the
/// quaternion product (NaN rotation via `look_at`), and a `3e38`
/// `MaxDist` overflows `dir * dist` into an `inf` boom target. Past
/// `USABLE_BOUND` every field reads unauthored, so the tracker below
/// only ever composes designed values.
#[test]
fn overflowing_authored_fields_fall_back_to_the_designed_boom() {
    let mut spec = TrackCamSpec::parse(NEAR_TEXT).unwrap();
    spec.offset = Some([3e38, 1.0, 4.0]);
    spec.track_to = Some([3e38, 0.0, 0.0]); // finite *length*, still unusable
    spec.min_dist = Some(3e38);
    spec.max_dist = Some(3e38);
    spec.min_speed = Some(2e6);
    spec.max_speed = Some(3e38);
    spec.camera_far = Some(3e38);
    spec.collide_type = Some(1e9);
    spec.min_max_on = Some(1e9);
    let lens = ChaseLens::authored(&spec);
    let rest = Vec3::new(0.0, 1.8, 5.0).length();
    assert_eq!(lens.offset, Vec3::new(0.0, 1.8, 5.0));
    assert_eq!(
        lens.aim,
        Vec3::new(0.0, 1.0, 0.0),
        "a finite-but-overflowing TrackTo reads unauthored"
    );
    assert_eq!(lens.dist_min, 0.0);
    assert_eq!(lens.dist_max, rest);
    assert_eq!(lens.speed_min, 0.0);
    assert_eq!(lens.speed_max, 0.0);
    assert!(!lens.collide, "a beyond-bound flag reads off");
    assert_eq!(lens.clip_far, 600.0);
    for issue in spec.validate() {
        assert!(issue.contains("exceeds the usable bound"), "{issue}");
    }

    // And the tracker never produces a non-finite pose from it.
    let mut app = base_app(CameraMode::Chase);
    app.add_systems(Update, chase_follow);
    spawn_vehicle(&mut app, Vec3::ZERO, Vec3::new(0.0, 0.0, -30.0));
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: lens,
            far: None,
            ..Default::default()
        },
        true,
    );
    for _ in 0..60 {
        app.update();
    }
    let xf = app.world().get::<Transform>(cam).unwrap();
    assert!(
        xf.translation.is_finite() && xf.rotation.is_finite(),
        "boom from an overflowing spec stays finite, got {xf:?}"
    );
}

/// A jump the frame delta cannot explain — the `R` reset's
/// `ResetVehicle` teleport, a water/stuck/disabled recovery or a
/// scripted re-anchor — snaps the boom to the new target on the next
/// tracked frame. Lerping would sweep the view in a straight line
/// across the world through whatever stands between the two poses
/// (AC05's reset leg).
#[test]
fn teleport_snaps_the_boom() {
    let mut app = base_app(CameraMode::Chase);
    app.add_systems(Update, chase_follow);
    let car = spawn_vehicle(&mut app, Vec3::ZERO, Vec3::ZERO);
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: near_lens(),
            far: None,
            ..Default::default()
        },
        true,
    );
    for _ in 0..30 {
        app.update();
    }

    // Reset-style teleport 200 m out: the propagated pose reaches the
    // tracker one update after the write.
    app.world_mut()
        .get_mut::<Transform>(car)
        .unwrap()
        .translation = Vec3::new(200.0, 0.0, 0.0);
    app.update();
    app.update();

    let xf = app.world().get::<Transform>(cam).unwrap();
    let expect = Vec3::new(200.0, 0.0, 0.0) + near_lens().offset;
    assert!(
        xf.translation.distance(expect) < 0.05,
        "boom snapped onto the new anchor, got {:?}",
        xf.translation
    );
}

/// Motion under the rate still eases — an in-place-scale hop (the
/// upright recovery's ~decimetre) keeps the smoothing rather than
/// snapping, so the threshold only fires on real teleports.
#[test]
fn small_displacements_stay_smooth() {
    let mut app = base_app(CameraMode::Chase);
    app.add_systems(Update, chase_follow);
    let car = spawn_vehicle(&mut app, Vec3::ZERO, Vec3::ZERO);
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: near_lens(),
            far: None,
            ..Default::default()
        },
        true,
    );
    for _ in 0..30 {
        app.update();
    }
    let settled = app.world().get::<Transform>(cam).unwrap().translation;

    app.world_mut()
        .get_mut::<Transform>(car)
        .unwrap()
        .translation = Vec3::new(0.0, 0.0, -1.5);
    app.update();
    app.update();

    let xf = app.world().get::<Transform>(cam).unwrap();
    let expect = Vec3::new(0.0, 0.0, -1.5) + near_lens().offset;
    assert!(
        xf.translation.distance(expect) > 0.5,
        "a 1.5 m hop eases instead of snapping, got {:?}",
        xf.translation
    );
    assert!(
        xf.translation.distance(settled) > 0.02,
        "the boom still tracks the hop, got {:?} vs {:?}",
        xf.translation,
        settled
    );
}

/// Re-entering a chase mode after the car drove on under another
/// view: the tracker went stale while `chase_follow` was gated out,
/// so the first chase frame reads the whole displacement as one jump
/// and snaps — the same fix that covers resets, exercised through the
/// mode-transition leg (AC05).
#[test]
fn mode_reentry_snaps_the_stale_boom() {
    let mut app = base_app(CameraMode::Chase);
    app.add_systems(Update, chase_follow);
    let car = spawn_vehicle(&mut app, Vec3::ZERO, Vec3::ZERO);
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: near_lens(),
            far: None,
            ..Default::default()
        },
        true,
    );
    for _ in 0..30 {
        app.update();
    }

    // Drive on under Cockpit: the chase pass is gated out while the
    // car covers 80 m.
    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::Cockpit;
    app.world_mut()
        .get_mut::<Transform>(car)
        .unwrap()
        .translation = Vec3::new(80.0, 0.0, 0.0);
    for _ in 0..3 {
        app.update();
    }

    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::Chase;
    app.update();
    let xf = app.world().get::<Transform>(cam).unwrap();
    let expect = Vec3::new(80.0, 0.0, 0.0) + near_lens().offset;
    assert!(
        xf.translation.distance(expect) < 0.05,
        "re-entry snapped onto the car, got {:?}",
        xf.translation
    );
}

/// The near→far lens swap is *not* a jump — the vehicle didn't move,
/// so the boom eases between the two authored anchors. Snapping there
/// would jolt every `C` press into ChaseFar.
#[test]
fn lens_transition_stays_smooth() {
    let mut app = base_app(CameraMode::Chase);
    app.add_systems(Update, chase_follow);
    spawn_vehicle(&mut app, Vec3::ZERO, Vec3::ZERO);
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: near_lens(),
            far: Some(far_lens()),
            smoothness: 6.0,
            ..Default::default()
        },
        true,
    );
    for _ in 0..30 {
        app.update();
    }

    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::ChaseFar;
    app.update();
    let xf = app.world().get::<Transform>(cam).unwrap();
    let far_target = far_lens().offset;
    assert!(
        xf.translation.distance(far_target) > 0.5,
        "near→far eases across the lens swap, got {:?}",
        xf.translation
    );
    assert!(
        xf.translation.distance(near_lens().offset) > 0.02,
        "and it is already moving toward the far anchor, got {:?}",
        xf.translation
    );
}

/// A rig's first tracked frame always snaps — there is no history to
/// ease from, so the boom starts on the authored anchor rather than
/// gliding in from wherever the camera spawned.
#[test]
fn first_track_lands_on_the_boom() {
    let mut app = base_app(CameraMode::Chase);
    app.add_systems(Update, chase_follow);
    spawn_vehicle(&mut app, Vec3::new(500.0, 0.0, 500.0), Vec3::ZERO);
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: near_lens(),
            far: None,
            ..Default::default()
        },
        true,
    );
    app.update();
    app.update();
    let xf = app.world().get::<Transform>(cam).unwrap();
    let expect = Vec3::new(500.0, 0.0, 500.0) + near_lens().offset;
    assert!(
        xf.translation.distance(expect) < 0.05,
        "first tracked frame lands on the anchor, got {:?}",
        xf.translation
    );
}

/// A `Playing` session with a camera override the harness drives —
/// `dev_cam_cycle_at` reads the config out of the resource like every
/// session-scoped override.
fn playing_session(cam_cycle_at: Option<u64>) -> Session {
    let mut session = Session::new();
    session
        .begin(SessionConfig {
            dev: DevOverrides {
                cam_cycle_at,
                ..DevOverrides::default()
            },
            ..SessionConfig::default()
        })
        .unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    session
}

/// `--cam-cycle-at` walks the same successor/activation path a `C`
/// press takes — once, at its session tick — so a frozen-input
/// capture can inspect a mid-drive transition (AC05).
#[test]
fn cam_cycle_at_advances_the_chain_at_its_tick() {
    let mut app = base_app(CameraMode::Chase);
    app.insert_resource(playing_session(Some(0)));
    app.add_systems(Update, dev_cam_cycle_at);
    let chase = spawn_chase(
        &mut app,
        ChaseCamera {
            near: near_lens(),
            far: Some(far_lens()),
            ..Default::default()
        },
        true,
    );
    let pov = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera::default(),
            CockpitCamera {
                offset: Vec3::ZERO,
                reverse_offset: None,
                pitch: 0.0,
                look_yaw: 0.0,
            },
        ))
        .id();

    app.update();
    assert_eq!(
        *app.world().resource::<CameraMode>(),
        CameraMode::Cockpit,
        "the chain advanced once at its tick"
    );
    assert!(!app.world().get::<Camera>(chase).unwrap().is_active);
    assert!(app.world().get::<Camera>(pov).unwrap().is_active);

    // One-shot: a later update does not cycle again.
    app.update();
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Cockpit);
}

/// The scheduled cycle honours the same absent-camera skip the key
/// does — no cockpit and no far lens means Chase → Free.
#[test]
fn cam_cycle_at_skips_modes_without_cameras() {
    let mut app = base_app(CameraMode::Chase);
    app.insert_resource(playing_session(Some(0)));
    app.add_systems(Update, dev_cam_cycle_at);
    let chase = spawn_chase(&mut app, ChaseCamera::default(), true);
    let free = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera::default(),
            FreeCamera::default(),
        ))
        .id();

    app.update();
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Free);
    assert!(!app.world().get::<Camera>(chase).unwrap().is_active);
    assert!(app.world().get::<Camera>(free).unwrap().is_active);
}

/// The gate is the session clock, not mere `Playing`: a threshold the
/// run has not reached leaves the view alone.
#[test]
fn cam_cycle_at_before_its_tick_is_inert() {
    let mut app = base_app(CameraMode::Chase);
    app.insert_resource(playing_session(Some(1)));
    app.add_systems(Update, dev_cam_cycle_at);
    let chase = spawn_chase(&mut app, ChaseCamera::default(), true);

    app.update();
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Chase);
    assert!(app.world().get::<Camera>(chase).unwrap().is_active);
}

#[test]
fn report_names_the_bound_lenses() {
    assert_eq!(
        TrackReport {
            near_authored: true,
            far_authored: true
        }
        .smoke_detail(),
        "near+far"
    );
    assert_eq!(
        TrackReport {
            near_authored: true,
            far_authored: false
        }
        .smoke_detail(),
        "near"
    );
    assert_eq!(TrackReport::default().smoke_detail(), "sized");
}
