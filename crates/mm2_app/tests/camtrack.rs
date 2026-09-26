//! F22-B.3 authored `camTrackCS` chase-rig coverage: the documented
//! HUD-3 chain (Chase Near → Cockpit → Chase Far) with the dev Free
//! camera appended, lens-driven boom/projection, the `CollideType`
//! occlusion pull-in, and VFS loading of the `_near`/`_far` records.

use avian3d::prelude::{Collider, Gravity, LinearVelocity, PhysicsPlugins, RigidBody};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::camera::{
    CameraMode, ChaseCamera, ChaseLens, FreeCamera, TrackReport, chase_follow, load_track_cams,
    toggle_camera,
};
use mm2_app::dash::CockpitCamera;
use mm2_assets::Vfs;
use mm2_formats::camtrack::TrackCamSpec;
use mm2_game::PlayerVehicle;
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
