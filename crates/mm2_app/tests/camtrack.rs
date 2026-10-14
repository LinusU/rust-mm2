//! F22-B.3 authored `camTrackCS` chase-rig coverage: the documented
//! HUD-3 chain (Chase Near → Cockpit → Chase Far) with the dev Free
//! camera appended, lens-driven boom/projection, the `CollideType`
//! occlusion pull-in, and VFS loading of the `_near`/`_far` records.

use avian3d::prelude::{Collider, Gravity, LinearVelocity, PhysicsPlugins, RigidBody};
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::camera::{
    CameraMode, ChaseCamera, ChaseLens, CollideKind, DriverState, FreeCamera, ReverseSwing,
    TrackReport, approach_axis, chase_follow, desired_eye, dev_cam_cycle_at, follow_rate_target,
    hill_bias, hill_mid, hill_normal, hill_pitch, load_track_cams, slew_follow_rate, toggle_camera,
    track_broken,
};
use mm2_app::dash::CockpitCamera;
use mm2_app::input::vehicle_input;
use mm2_app::session::SpawnPoint;
use mm2_assets::Vfs;
use mm2_formats::camtrack::TrackCamSpec;
use mm2_game::{
    AuthorityRole, DevOverrides, ObjectId, PlayerVehicle, Session, SessionConfig, SessionPhase,
    VehicleTelemetry, WheelTelemetry,
};
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

/// Interpolation runs before Update while global propagation follows it.
/// The chase boom must share the current root render pose with the body.
#[test]
fn chase_follows_current_render_pose_before_global_propagation() {
    let mut app = base_app(CameraMode::Chase);
    let previous = Transform::from_xyz(12.0, 2.0, -10.0);
    let car = spawn_vehicle(&mut app, previous.translation, Vec3::NEG_Z * 20.0);
    let lens = near_lens();
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: lens.clone(),
            far: None,
            ..default()
        },
        true,
    );
    app.update();
    // Reproduce the point between interpolation and PostUpdate: the rendered
    // root has moved, but GlobalTransform deliberately retains the old pose.
    let current = Transform::from_xyz(12.0, 2.0, -10.44).with_rotation(Quat::from_rotation_y(0.15));
    *app.world_mut().get_mut::<Transform>(car).unwrap() = current;
    *app.world_mut().get_mut::<GlobalTransform>(car).unwrap() = previous.into();
    app.world_mut().run_system_once(chase_follow).unwrap();

    let expected = desired_eye(current.translation, current.rotation, &lens).1;
    let actual = app.world().get::<Transform>(cam).unwrap().translation;
    assert!(
        actual.distance(expected) < 1e-5,
        "chase targets current render pose: {actual:?} vs {expected:?}"
    );
    assert_eq!(
        app.world().get::<ChaseCamera>(cam).unwrap().last_pos,
        Some(current.translation)
    );
    assert_eq!(
        app.world()
            .get::<GlobalTransform>(car)
            .unwrap()
            .translation(),
        previous.translation,
        "the fixture must retain the stale global pose during the camera update"
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
    // Rest boom = |Offset| = √(1²+4²) ≈ 4.12 from the aim point once
    // the smoothing converges.
    let xf = app.world().get::<Transform>(cam).unwrap();
    let boom = (xf.translation - near_lens().aim).length();
    assert!(
        (boom - (1.0f32 + 16.0).sqrt()).abs() < 0.15,
        "near rest boom ≈ authored |Offset|, got {boom}"
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
    let boom = (xf.translation - far_lens().aim).length();
    assert!(
        (boom - (1.8f32 * 1.8 + 64.0).sqrt()).abs() < 0.15,
        "far rest boom ≈ authored |Offset|, got {boom}"
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

/// The field-of-view setting widens whichever authored lens is active
/// and returns to the authored value when set back, never compounding.
#[test]
fn the_field_of_view_setting_widens_the_active_chase_lens() {
    use mm2_app::settings::{FieldOfView, GraphicsSettings};
    let mut app = base_app(CameraMode::Chase);
    app.init_resource::<GraphicsSettings>();
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
    let fov = |app: &App| {
        let Projection::Perspective(p) = app.world().get::<Projection>(cam).unwrap() else {
            panic!("chase camera keeps a perspective projection");
        };
        p.fov.to_degrees()
    };
    let set = |app: &mut App, f: FieldOfView| {
        app.world_mut()
            .resource_mut::<GraphicsSettings>()
            .field_of_view = f;
        for _ in 0..3 {
            app.update();
        }
    };

    for _ in 0..3 {
        app.update();
    }
    assert!(
        (fov(&app) - 70.0).abs() < 0.01,
        "authored near lens by default"
    );
    set(&mut app, FieldOfView::Wide);
    assert!(
        (fov(&app) - 80.0).abs() < 0.01,
        "near lens +10°, got {}",
        fov(&app)
    );
    set(&mut app, FieldOfView::Wider);
    set(&mut app, FieldOfView::Wider);
    assert!(
        (fov(&app) - 90.0).abs() < 0.01,
        "+20° from authored, not compounded"
    );

    // The far lens widens from its own authored 65°.
    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::ChaseFar;
    set(&mut app, FieldOfView::Wider);
    assert!(
        (fov(&app) - 85.0).abs() < 0.01,
        "far lens +20°, got {}",
        fov(&app)
    );

    set(&mut app, FieldOfView::Authored);
    assert!(
        (fov(&app) - 65.0).abs() < 0.01,
        "back to the authored far lens"
    );
}

/// Speed does not extend the boom (UNK-36): at full speed the eye
/// still settles within `[MinDist, MaxDist]` of the aim point, trailing
/// the desired position by the (loose) fast follow rate only.
#[test]
fn speed_window_does_not_extend_boom() {
    let mut app = base_app(CameraMode::Chase);
    app.add_systems(Update, chase_follow);
    // Constant 20 m/s (MaxSpeed) forward is −Z: the vehicle is kept in
    // place so the eye must converge on the rest boom, not MaxDist.
    spawn_vehicle(&mut app, Vec3::ZERO, Vec3::new(0.0, 0.0, -20.0));
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: near_lens(),
            far: None,
            ..Default::default()
        },
        true,
    );
    for _ in 0..600 {
        app.update();
    }
    let xf = app.world().get::<Transform>(cam).unwrap();
    let boom = (xf.translation - near_lens().aim).length();
    assert!(
        (boom - 17.0f32.sqrt()).abs() < 0.1,
        "boom stays at |Offset| under full speed, got {boom}"
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
            ..Default::default()
        },
        true,
    );
    for _ in 0..120 {
        app.update();
    }
    let xf = app.world().get::<Transform>(cam).unwrap();
    // The recovered pull-in is `hit + near − 0.33`: the eye lands 0.17 m
    // behind the face at z=3 (the 0.5 m near plane stays in front of it).
    assert!(
        xf.translation.z < 3.3,
        "unregistered trailer still pulls the boom in, z={}",
        xf.translation.z
    );
}

/// `MinMaxOn`: an overhead slab 5 m above the eye keeps the eye 0.5 m
/// under it (the rest eye sits ~1.2 m above the aim, so the slab at
/// y=1.9 must bind), and `MinMaxOn 0` leaves the eye alone.
#[test]
fn min_max_on_clamps_eye_under_a_ceiling() {
    for (min_max, binds) in [(1.0, true), (0.0, false)] {
        let mut app = physics_app();
        app.add_systems(Update, chase_follow);
        spawn_vehicle(&mut app, Vec3::ZERO, Vec3::ZERO);
        // Ceiling slab, underside at y=2.0 (rest eye is at y=2.7).
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(40.0, 1.0, 40.0),
            Transform::from_xyz(0.0, 2.5, 4.0),
        ));
        let mut spec = TrackCamSpec::parse(NEAR_TEXT).unwrap();
        spec.min_max_on = Some(min_max);
        spec.collide_type = Some(0.0);
        let cam = spawn_chase(
            &mut app,
            ChaseCamera {
                near: ChaseLens::authored(&spec),
                far: None,
                ..Default::default()
            },
            true,
        );
        for _ in 0..180 {
            app.update();
        }
        let y = app.world().get::<Transform>(cam).unwrap().translation.y;
        if binds {
            assert!(y <= 1.5 + 1e-3, "eye held 0.5 m under the slab, y={y}");
        } else {
            assert!(y > 2.0, "MinMaxOn 0 does not clamp, y={y}");
        }
    }
}

/// `MinMaxOn`: the floor bound (hit + 0.5) wins over the ceiling.
#[test]
fn min_max_on_floor_wins_over_ceiling() {
    let mut app = physics_app();
    app.add_systems(Update, chase_follow);
    spawn_vehicle(&mut app, Vec3::ZERO, Vec3::ZERO);
    // Floor top at y=2.4 and ceiling underside at y=2.9: the bounds
    // cross (floor+0.5=2.9 > ceiling-0.5=2.4); the floor wins.
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(40.0, 1.0, 40.0),
        Transform::from_xyz(0.0, 1.9, 4.0),
    ));
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(40.0, 1.0, 40.0),
        Transform::from_xyz(0.0, 3.4, 4.0),
    ));
    let mut spec = TrackCamSpec::parse(NEAR_TEXT).unwrap();
    spec.collide_type = Some(0.0);
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: ChaseLens::authored(&spec),
            far: None,
            ..Default::default()
        },
        true,
    );
    for _ in 0..180 {
        app.update();
    }
    let y = app.world().get::<Transform>(cam).unwrap().translation.y;
    assert!((y - 2.9).abs() < 0.05, "floor bound wins, y={y}");
}

/// `CollideType` values other than 1 and 2 do nothing, and 2 pulls the
/// boom in (eased) once it has settled.
#[test]
fn collide_type_modes() {
    for (mode, pulled) in [(1.0, true), (2.0, true), (3.0, false), (0.0, false)] {
        let mut app = physics_app();
        app.add_systems(Update, chase_follow);
        spawn_vehicle(&mut app, Vec3::ZERO, Vec3::ZERO);
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(8.0, 8.0, 0.4),
            Transform::from_xyz(0.0, 1.0, 2.0),
        ));
        let mut spec = TrackCamSpec::parse(NEAR_TEXT).unwrap();
        spec.collide_type = Some(mode);
        let cam = spawn_chase(
            &mut app,
            ChaseCamera {
                near: ChaseLens::authored(&spec),
                far: None,
                ..Default::default()
            },
            true,
        );
        for _ in 0..240 {
            app.update();
        }
        let z = app.world().get::<Transform>(cam).unwrap().translation.z;
        assert_eq!(z < 2.0, pulled, "CollideType {mode}: z={z}");
    }
}

/// `CollideType 1` leaves 0.33 m minus the near plane short of the wall
/// face (hit + near − margin), measured along the aim→eye line.
#[test]
fn collide_type_one_uses_the_recovered_margin() {
    let mut app = physics_app();
    app.add_systems(Update, chase_follow);
    spawn_vehicle(&mut app, Vec3::ZERO, Vec3::ZERO);
    // Wall front face at z = 2.0 (centre 2.2, half-depth 0.2).
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(40.0, 40.0, 0.4),
        Transform::from_xyz(0.0, 1.0, 2.2),
    ));
    let lens = ChaseLens::authored(&TrackCamSpec::parse(NEAR_TEXT).unwrap());
    let aim = lens.aim;
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: lens.clone(),
            far: None,
            ..Default::default()
        },
        true,
    );
    for _ in 0..240 {
        app.update();
    }
    let eye = app.world().get::<Transform>(cam).unwrap().translation;
    // The centre ray hits the face at distance 2.0/cos(angle) along the
    // boom; the eye then sits `near − 0.33` beyond that hit.
    let dir = (eye - aim).normalize();
    let hit = 2.0 / dir.z;
    let want = hit + lens.clip_near - 0.33;
    assert!(
        ((eye - aim).length() - want).abs() < 0.05,
        "eye {} from aim, want {want}",
        (eye - aim).length()
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
/// chassis-derived rest boom, constructor-default follow dynamics,
/// no collision flag and `authored = false` so `trk=` reports `sized`.
#[test]
fn sized_lens_drives_the_fallback_boom() {
    let lens = ChaseLens::sized(2.0, 4.5);
    let offset = Vec3::new(0.0, 2.0 * 0.10 + 1.4, 4.5 * 0.85 + 3.5);
    assert!((lens.offset - offset).length() < 1e-5);
    let rest = lens.offset.length();
    assert_eq!(lens.dist_max, 0.0, "no authored MaxDist, no clamp");
    assert_eq!((lens.speed_min, lens.speed_max), (5.0, 35.0));
    assert_eq!((lens.app_xz_min, lens.app_xz_max), (1.8, 12.0));
    assert!(lens.collide == CollideKind::Off && !lens.authored);
    // The eye lands where the retired constants put it.
    assert!((lens.anchor() - Vec3::new(0.0, 2.0 * 0.55 + 1.4, 4.5 * 0.85 + 3.5)).length() < 1e-5);
    let aim = lens.aim;

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
    for _ in 0..240 {
        app.update();
    }
    let xf = app.world().get::<Transform>(cam).unwrap();
    let boom = (xf.translation - aim).length();
    assert!(
        (boom - rest).abs() < 0.15,
        "sized lens converges to its rest boom, got {boom}"
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
    assert_eq!(
        lens.offset,
        Vec3::new(0.0, 1.8, 5.0),
        "non-finite Offset reads unauthored — the designed boom stands in"
    );
    assert_eq!(lens.aim, Vec3::new(0.0, 1.0, 0.0));
    assert_eq!(lens.dist_min, 0.0);
    assert_eq!(lens.dist_max, 0.0, "unusable MaxDist reads as no clamp");
    assert_eq!(
        (lens.speed_min, lens.speed_max),
        (5.0, 35.0),
        "unusable speed window takes the constructor defaults"
    );
    assert!(lens.collide == CollideKind::Off);
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
    assert_eq!(lens.offset, Vec3::new(0.0, 1.8, 5.0));
    assert_eq!(
        lens.aim,
        Vec3::new(0.0, 1.0, 0.0),
        "a finite-but-overflowing TrackTo reads unauthored"
    );
    assert_eq!(lens.dist_min, 0.0);
    assert_eq!(lens.dist_max, 0.0, "unusable MaxDist reads as no clamp");
    assert_eq!(
        (lens.speed_min, lens.speed_max),
        (5.0, 35.0),
        "unusable speed window takes the constructor defaults"
    );
    assert!(
        lens.collide == CollideKind::Off,
        "a beyond-bound flag reads off"
    );
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
    let expect = Vec3::new(200.0, 0.0, 0.0) + near_lens().anchor();
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
    let expect = Vec3::new(0.0, 0.0, -1.5) + near_lens().anchor();
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
    let expect = Vec3::new(80.0, 0.0, 0.0) + near_lens().anchor();
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
    let far_target = far_lens().anchor();
    assert!(
        xf.translation.distance(far_target) > 0.5,
        "near→far eases across the lens swap, got {:?}",
        xf.translation
    );
    assert!(
        xf.translation.distance(near_lens().anchor()) > 0.02,
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
    let expect = Vec3::new(500.0, 0.0, 500.0) + near_lens().anchor();
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

/// Retail sweep behind the UNK-37 reading: `Midtown2.exe`'s `Load`
/// (`0x4a1110`) calls the class's post-load virtual after a successful
/// parse, and `camPovCS`'s (`0x51d6f0`) stores 0.1 into `CameraNear`
/// while `camTrackCS`'s (`0x51dad0`) stores 0.5. This lists what the
/// stock records author, so the overwritten values are visible.
#[test]
fn retail_authored_camera_near_values_are_what_the_loader_overwrites() {
    let Some((retail, _slot)) = crate::support::retail_slot() else {
        return;
    };
    let vfs = crate::support::mount(&retail);
    let mut pov = std::collections::BTreeMap::<String, Vec<String>>::new();
    let mut track = std::collections::BTreeMap::<String, Vec<String>>::new();
    for path in vfs.list() {
        let lower = path.to_ascii_lowercase();
        let (map, is_pov) = if lower.ends_with(".campovcs") {
            (&mut pov, true)
        } else if lower.ends_with(".camtrackcs") {
            (&mut track, false)
        } else {
            continue;
        };
        let (bytes, _) = vfs.read_path(&path).unwrap();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let near = if is_pov {
            // Other `.campovcs` blocks (`PovCamCS`) are a different class.
            match mm2_formats::dash::PovCamSpec::parse(&text) {
                Ok(spec) => spec.camera_near,
                Err(e) => {
                    eprintln!("skipped {path}: {e}");
                    continue;
                }
            }
        } else {
            TrackCamSpec::parse(&text).unwrap().camera_near
        };
        map.entry(format!("{near:?}")).or_default().push(path);
    }
    for (name, m) in [("campovcs", &pov), ("camtrackcs", &track)] {
        for (near, paths) in m {
            eprintln!(
                "{name} CameraNear {near}: {} records {paths:?}",
                paths.len()
            );
        }
    }
    assert!(
        !pov.is_empty() && !track.is_empty(),
        "no retail camera records"
    );
}

/// `camTrackCS`'s post-load virtual stores 0.5 into `CameraNear`
/// (UNK-37), so five retail records that author 1.0 still bind 0.5.
#[test]
fn chase_lens_near_is_the_loaders_constant() {
    let mut spec = TrackCamSpec::parse(NEAR_TEXT).unwrap();
    for authored in [1.0, 0.02, 9.0] {
        spec.camera_near = Some(authored);
        assert_eq!(ChaseLens::authored(&spec).clip_near, 0.5, "{authored}");
    }
}

fn dynamics_lens() -> ChaseLens {
    ChaseLens::authored(
        &TrackCamSpec::parse(
            "type: a\ncamTrackCS {\n  Offset 0.5 1.0 4.0\n  MinAppXZPos 2.0\n  MaxAppXZPos 10.0\n  MinSpeed 5.0\n  MaxSpeed 25.0\n  AppInc 4.0\n  AppDec 2.0\n  TrackTo 0.0 1.5 0.0\n}\n",
        )
        .unwrap(),
    )
}

/// UNK-36 `0x51eb20`: speed lerps the follow rate from `MaxAppXZPos`
/// (at/below `MinSpeed`) to `MinAppXZPos` (at/above `MaxSpeed`), in m/s.
#[test]
fn follow_rate_lerps_with_speed() {
    let lens = dynamics_lens();
    assert_eq!(follow_rate_target(&lens, 0.0), 10.0);
    assert_eq!(follow_rate_target(&lens, 5.0), 10.0);
    assert!((follow_rate_target(&lens, 15.0) - 6.0).abs() < 1e-5);
    assert_eq!(follow_rate_target(&lens, 25.0), 2.0);
    assert_eq!(follow_rate_target(&lens, 80.0), 2.0);
}

/// The live rate slews at `AppInc` rising and `AppDec` falling per
/// second and never passes the target.
#[test]
fn follow_rate_slews_at_inc_and_dec() {
    let lens = dynamics_lens();
    let dt = 1.0 / 60.0;
    assert!(
        (slew_follow_rate(2.0, 10.0, lens.app_inc, lens.app_dec, dt) - (2.0 + 4.0 * dt)).abs()
            < 1e-6
    );
    assert!(
        (slew_follow_rate(10.0, 2.0, lens.app_inc, lens.app_dec, dt) - (10.0 - 2.0 * dt)).abs()
            < 1e-6
    );
    assert_eq!(slew_follow_rate(9.99, 10.0, 4.0, 2.0, dt), 10.0);
    assert_eq!(slew_follow_rate(2.01, 2.0, 4.0, 2.0, dt), 2.0);
    // Driven through the system: stopped car from a seeded fast rate
    // eases up to MaxAppXZPos at AppInc, not instantly.
    let mut app = base_app(CameraMode::Chase);
    app.add_systems(Update, chase_follow);
    spawn_vehicle(&mut app, Vec3::ZERO, Vec3::ZERO);
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: dynamics_lens(),
            far: None,
            ..Default::default()
        },
        true,
    );
    app.update();
    app.update();
    let rate = |app: &App| {
        app.world()
            .get::<ChaseCamera>(cam)
            .unwrap()
            .follow
            .app_xz
            .unwrap()
    };
    assert_eq!(rate(&app), 10.0, "a stopped car follows at MaxAppXZPos");
}

/// UNK-36 `0x51e3e0`: `Offset` is a yaw-only, gravity-aligned frame
/// around the car-local `TrackTo` point. Pitching or rolling the car
/// moves the aim point (through the full matrix) but never tilts the
/// boom; yawing the car swings it behind the heading.
#[test]
fn offset_frame_is_yaw_only() {
    let lens = dynamics_lens();
    let (t0, eye0) = desired_eye(Vec3::new(3.0, 0.0, -2.0), Quat::IDENTITY, &lens);
    assert!((t0 - Vec3::new(3.0, 1.5, -2.0)).length() < 1e-5);
    // +X right, +Y up, +Z behind a car heading −Z.
    assert!((eye0 - t0 - Vec3::new(0.5, 1.0, 4.0)).length() < 1e-5);

    // Pitch and roll: the boom vector from the aim point is unchanged.
    for rot in [
        Quat::from_rotation_x(0.5),
        Quat::from_rotation_z(-0.7),
        Quat::from_euler(EulerRot::YXZ, 0.0, 0.4, 0.3),
    ] {
        let (t, eye) = desired_eye(Vec3::new(3.0, 0.0, -2.0), rot, &lens);
        assert!(
            (eye - t - Vec3::new(0.5, 1.0, 4.0)).length() < 1e-4,
            "boom tilted by pitch/roll: {:?}",
            eye - t
        );
        assert!((t - Vec3::new(3.0, 0.0, -2.0) - rot * lens.aim).length() < 1e-5);
    }

    // Yaw 90° (car now heads −X): rearward is +X, right is −Z.
    let (t, eye) = desired_eye(
        Vec3::ZERO,
        Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
        &lens,
    );
    assert!(
        (eye - t - Vec3::new(4.0, 1.0, -0.5)).length() < 1e-4,
        "{:?}",
        eye - t
    );
}

/// The per-axis approach: soft knee below `AppPosMin`, `AppApp`
/// low-pass, and a step that never overshoots.
#[test]
fn approach_axis_knee_lowpass_and_clamp() {
    let lens = dynamics_lens(); // AppPosMin 0.25, AppApp 0.7 (ctor defaults)
    let dt = 1.0 / 60.0;
    // Far target: state low-passes toward d, step = state·rate·dt.
    let mut st = 0.0;
    let out = approach_axis(0.0, 10.0, &mut st, 6.0, &lens, dt);
    assert!((st - 7.0).abs() < 1e-4, "s ← 0 + (10 − 0)·0.7");
    assert!((out - 7.0 * 6.0 * dt).abs() < 1e-4);
    // Inside the knee d becomes d²/AppPosMin.
    let mut st = 0.0;
    approach_axis(0.0, 0.1, &mut st, 6.0, &lens, dt);
    assert!((st - 0.7 * (0.1 * 0.1 / 0.25)).abs() < 1e-5);
    // Never overshoots, either direction.
    let mut st = 100.0;
    assert_eq!(approach_axis(0.0, 0.01, &mut st, 50.0, &lens, dt), 0.01);
    let mut st = 100.0;
    assert_eq!(approach_axis(0.0, -0.01, &mut st, 50.0, &lens, dt), -0.01);
}

fn reverse_lens(extra: &str) -> ChaseLens {
    let text = format!(
        "type: a\ncamTrackCS {{\n  Offset 0.0 1.0 4.0\n  RevOnApp 2.0\n  RevOffApp 4.0\n  {extra}\n}}\n"
    );
    ChaseLens::authored(&TrackCamSpec::parse(&text).unwrap())
}

fn reversing(steer: f32) -> DriverState {
    DriverState {
        reverse_gear: true,
        throttle: 1.0,
        steer,
        handbrake: 0.0,
    }
}

/// UNK-36 `0x51dc50`: the swing needs 2.0 s (hard-coded; `RevDelay` is
/// never read) of throttle in reverse gear, with the side taken from the
/// steering at that moment (right, > 0.1, is -1).
#[test]
fn reverse_swing_waits_the_constant_delay_and_picks_the_side() {
    let dt = 1.0 / 60.0;
    for (steer, side) in [(0.5, -1.0), (0.1, 1.0), (-0.9, 1.0)] {
        // A record that authors RevDelay 9 still swings after 2.0 s.
        let lens = reverse_lens("RevDelay 9.0");
        let mut sw = ReverseSwing::default();
        for _ in 0..119 {
            sw.detect(&lens, &reversing(steer), dt);
        }
        assert!(!sw.reversing, "not before 2 s");
        sw.detect(&lens, &reversing(steer), dt);
        sw.detect(&lens, &reversing(steer), dt);
        assert!(sw.reversing && sw.side == side, "steer {steer}");
    }
    // Coasting in reverse gear never starts the timer.
    let mut sw = ReverseSwing::default();
    let idle = DriverState {
        throttle: 0.04,
        ..reversing(0.0)
    };
    for _ in 0..600 {
        sw.detect(&lens_default(), &idle, dt);
    }
    assert!(!sw.reversing && sw.timer == 0.0);
}

fn lens_default() -> ChaseLens {
    reverse_lens("")
}

/// `0x51e3e0`: the angle ramps to `side·π` at `RevOnApp`, back to 0 at
/// `RevOffApp`, and never overshoots either target.
#[test]
fn reverse_swing_ramps_at_the_authored_rates() {
    let lens = lens_default();
    let dt = 0.25;
    let mut sw = ReverseSwing {
        reversing: true,
        side: -1.0,
        ..Default::default()
    };
    sw.ramp(&lens, dt);
    assert!((sw.angle + 0.5).abs() < 1e-6);
    for _ in 0..20 {
        sw.ramp(&lens, dt);
    }
    assert_eq!(sw.angle, -std::f32::consts::PI);
    // Out of reverse: unwinds toward 0 at 4 rad/s, clamped at 0.
    sw.reversing = false;
    sw.ramp(&lens, dt);
    assert!((sw.angle - (-std::f32::consts::PI + 1.0)).abs() < 1e-5);
    for _ in 0..20 {
        sw.ramp(&lens, dt);
    }
    assert_eq!(sw.angle, 0.0);
}

/// Leaving reverse gear clears the flag; a swing within 0.001 rad of a
/// half turn is snapped to `side·π` first, so the unwind goes the way
/// the steering says.
#[test]
fn leaving_reverse_clears_the_flag_and_snaps_a_full_swing() {
    let lens = lens_default();
    let mut sw = ReverseSwing {
        reversing: true,
        side: -1.0,
        angle: -3.1412,
        timer: 2.5,
    };
    let forward = DriverState {
        steer: -0.5,
        ..Default::default()
    };
    sw.detect(&lens, &forward, 1.0 / 60.0);
    assert!(!sw.reversing && sw.timer == 0.0);
    assert_eq!(sw.angle, std::f32::consts::PI, "steered left: +π side");
    // A partial swing is left where it is.
    let mut sw = ReverseSwing {
        reversing: true,
        angle: 1.0,
        ..Default::default()
    };
    sw.detect(&lens, &forward, 1.0 / 60.0);
    assert_eq!(sw.angle, 1.0);
}

/// `ReverseOn` -1 pins the swing at π while the flag is set; 0 (and
/// any other value) never swings; the handbrake in reverse gear zeroes
/// the live follow rate only for `ReverseOn` 1.
#[test]
fn reverse_on_modes() {
    let dt = 1.0 / 60.0;
    let pinned = reverse_lens("ReverseOn -1");
    let mut sw = ReverseSwing::default();
    sw.detect(&pinned, &DriverState::default(), dt);
    sw.ramp(&pinned, dt);
    assert_eq!(sw.angle, std::f32::consts::PI);
    let off = reverse_lens("ReverseOn 0");
    let mut sw = ReverseSwing::default();
    for _ in 0..300 {
        sw.detect(&off, &reversing(0.0), dt);
        sw.ramp(&off, dt);
    }
    assert!(!sw.reversing && sw.angle == 0.0);
    let held = DriverState {
        handbrake: 0.6,
        ..reversing(0.0)
    };
    assert!(ReverseSwing::default().detect(&lens_default(), &held, dt));
    assert!(!ReverseSwing::default().detect(&off, &held, dt));
    let light = DriverState {
        handbrake: 0.5,
        ..reversing(0.0)
    };
    assert!(!ReverseSwing::default().detect(&lens_default(), &light, dt));
}

/// A record that omits the reverse cluster keeps the constructor
/// defaults (ReverseOn 1, RevOnApp 2, RevOffApp 4).
#[test]
fn reverse_defaults_follow_the_constructor() {
    let lens = reverse_lens("");
    assert_eq!(
        (lens.reverse_on, lens.rev_on_app, lens.rev_off_app),
        (1, 2.0, 4.0)
    );
    let bare = ChaseLens::authored(
        &TrackCamSpec::parse("type: a\ncamTrackCS {\n  Offset 0 1 4\n}\n").unwrap(),
    );
    assert_eq!(
        (bare.reverse_on, bare.rev_on_app, bare.rev_off_app),
        (1, 2.0, 4.0)
    );
}

/// `0x51dc50`: TrackBreak 2 always breaks, 1 only when the body's up
/// axis has y < 0.35, 0 never.
#[test]
fn track_break_modes() {
    assert!(track_broken(2, 1.0));
    assert!(track_broken(1, 0.34));
    assert!(!track_broken(1, 0.35));
    assert!(!track_broken(1, 1.0));
    assert!(!track_broken(0, -1.0));
}

/// End to end through `chase_follow`: a car held in reverse gear with
/// the pedal down swings the boom from behind the car to in front of it
/// after the 2 s delay (UNK-36 `0x51dc50`/`0x51e3e0`), and back once
/// the car leaves reverse.
#[test]
fn reversing_car_swings_the_chase_boom_round_and_back() {
    let mut app = base_app(CameraMode::Chase);
    app.add_systems(Update, chase_follow);
    let car = spawn_vehicle(&mut app, Vec3::ZERO, Vec3::ZERO);
    let text = "type: a\ncamTrackCS {\n  Offset 0.0 1.0 4.0\n  TrackTo 0.0 1.0 0.0\n  ApproachOn 0\n  RevOnApp 2.0\n  RevOffApp 4.0\n}\n";
    let cam = spawn_chase(
        &mut app,
        ChaseCamera {
            near: ChaseLens::authored(&TrackCamSpec::parse(text).unwrap()),
            ..default()
        },
        true,
    );
    let telemetry = |reverse| VehicleTelemetry {
        object: ObjectId {
            generation: 0,
            slot: 1,
        },
        tick: 0,
        authority: AuthorityRole::Authority,
        position: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        linear_velocity: Vec3::ZERO,
        angular_velocity: Vec3::ZERO,
        forward_speed: 0.0,
        rpm: 0.0,
        engine_load: 0.0,
        gear: 0,
        reverse,
        shifting: false,
        wheels: Vec::new(),
        damage: Default::default(),
    };
    let eye_z = |app: &App| app.world().get::<Transform>(cam).unwrap().translation.z;
    app.world_mut().entity_mut(car).insert((
        telemetry(true),
        VehicleInput {
            brake: 1.0,
            ..default()
        },
    ));
    app.update();
    assert!((eye_z(&app) - 4.0).abs() < 1e-4, "starts behind the car");
    // 2 s delay, then π at 2 rad/s: 1.5 s more.
    for _ in 0..(60 * 2 + 1) {
        app.update();
    }
    assert!(eye_z(&app) > 4.0 - 4.0 * 0.9, "swing has begun");
    for _ in 0..(60 * 2) {
        app.update();
    }
    assert!(
        (eye_z(&app) + 4.0).abs() < 1e-3,
        "in front: {}",
        eye_z(&app)
    );
    // Out of reverse: unwinds at 4 rad/s, π in 0.79 s.
    app.world_mut().entity_mut(car).insert(telemetry(false));
    for _ in 0..60 {
        app.update();
    }
    assert!(
        (eye_z(&app) - 4.0).abs() < 1e-3,
        "back behind: {}",
        eye_z(&app)
    );
}

/// A grounded wheel at car-space `local` with world hit normal `n`.
fn wheel(local: Vec3, n: Vec3) -> WheelTelemetry {
    WheelTelemetry {
        grounded: true,
        contact_point: local,
        contact_normal: n,
        ..Default::default()
    }
}

/// A stationary telemetry snapshot carrying `wheels`, at the origin.
fn hill_telemetry(wheels: Vec<WheelTelemetry>) -> VehicleTelemetry {
    VehicleTelemetry {
        object: ObjectId {
            generation: 0,
            slot: 1,
        },
        tick: 0,
        authority: AuthorityRole::Authority,
        position: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        linear_velocity: Vec3::ZERO,
        angular_velocity: Vec3::ZERO,
        forward_speed: 0.0,
        rpm: 0.0,
        engine_load: 0.0,
        gear: 0,
        reverse: false,
        shifting: false,
        wheels,
        damage: Default::default(),
    }
}

/// A lens with the stock hill authoring and no approach smoothing.
fn hill_lens() -> ChaseLens {
    ChaseLens::authored(
        &TrackCamSpec::parse(
            "type: a\ncamTrackCS {\n  Offset 0.0 1.0 4.0\n  TrackTo 0.0 1.7 0.0\n  ApproachOn 0\n  HillMin -0.28\n  HillMax 0.28\n  HillLerp 0.05\n}\n",
        )
        .unwrap(),
    )
}

/// `HillMin`/`HillMax`/`HillLerp` bind from the record; a record that
/// omits them (and the sized fallback) keeps the constructor defaults
/// −0.56 / 0.56 / 0.05, and the low-pass factor is clamped to `[0, 1]`.
#[test]
fn hill_fields_bind_from_the_record_and_the_ctor() {
    let lens = hill_lens();
    assert_eq!(
        (lens.hill_min, lens.hill_max, lens.hill_lerp),
        (-0.28, 0.28, 0.05)
    );
    let bare = ChaseLens::authored(
        &TrackCamSpec::parse("type: a\ncamTrackCS {\n  Offset 0 1 4\n}\n").unwrap(),
    );
    assert_eq!(
        (bare.hill_min, bare.hill_max, bare.hill_lerp),
        (-0.56, 0.56, 0.05)
    );
    let sized = ChaseLens::sized(2.0, 4.5);
    assert_eq!(
        (sized.hill_min, sized.hill_max, sized.hill_lerp),
        (-0.56, 0.56, 0.05)
    );

    let mut spec =
        TrackCamSpec::parse("type: a\ncamTrackCS {\n  Offset 0 1 4\n  HillLerp -3\n}\n").unwrap();
    assert_eq!(ChaseLens::authored(&spec).hill_lerp, 0.0);
    spec.hill_lerp = Some(4.0);
    assert_eq!(ChaseLens::authored(&spec).hill_lerp, 1.0);
    spec.hill_lerp = Some(f32::NAN);
    assert_eq!(ChaseLens::authored(&spec).hill_lerp, 0.05);
}

/// `0x51dec0`: the slope normal is the mean of the grounded front- and
/// rear-axle hit normals; a group whose mean cancels is dropped, and
/// nothing grounded reads as world up.
#[test]
fn hill_mid_averages_grounded_axle_hit_normals() {
    let tilt = Vec3::new(0.0, 0.98, 0.2).normalize();
    let mut wheels = vec![
        wheel(Vec3::new(-0.8, 0.0, -1.5), tilt),
        wheel(Vec3::new(0.8, 0.0, -1.5), tilt),
        wheel(Vec3::new(-0.8, 0.0, 1.5), tilt),
        wheel(Vec3::new(0.8, 0.0, 1.5), tilt),
    ];
    let mid = hill_mid(Vec3::ZERO, Quat::IDENTITY, &wheels);
    assert!((mid - tilt).length() < 1e-5, "{mid:?}");

    // Rear pair airborne → the front mean stands alone.
    wheels[2].grounded = false;
    wheels[3].grounded = false;
    let mid = hill_mid(Vec3::ZERO, Quat::IDENTITY, &wheels);
    assert!((mid - tilt).length() < 1e-5, "{mid:?}");

    // A front pair that cancels drops the group and the rear pair wins.
    wheels[0].contact_normal = Vec3::X;
    wheels[1].contact_normal = Vec3::NEG_X;
    wheels[2].grounded = true;
    wheels[3].grounded = true;
    let mid = hill_mid(Vec3::ZERO, Quat::IDENTITY, &wheels);
    assert!((mid - tilt).length() < 1e-5, "front cancel → rear: {mid:?}");

    // Nothing grounded → world up.
    for w in &mut wheels {
        w.grounded = false;
    }
    assert_eq!(hill_mid(Vec3::ZERO, Quat::IDENTITY, &wheels), Vec3::Y);
}

/// The recovered sign chain: flat ground reads 0; a front-up uphill tilts
/// the eye down (negative bias) and a front-down downhill raises it
/// (positive), each bounded by `HillMin`/`HillMax`; a side-on slope has no
/// fore/aft sense.
#[test]
fn hill_bias_is_zero_flat_and_signs_with_the_slope() {
    let lens = hill_lens();
    assert_eq!(hill_bias(&lens, Vec3::Y, Vec3::X), 0.0);

    let phi = 0.35f32; // ~20°
    let uphill = Vec3::new(0.0, phi.cos(), phi.sin());
    let downhill = Vec3::new(0.0, phi.cos(), -phi.sin());
    let up_bias = hill_bias(&lens, uphill, Vec3::X);
    let down_bias = hill_bias(&lens, downhill, Vec3::X);
    assert!(up_bias < 0.0, "uphill lowers the eye: {up_bias}");
    assert!(down_bias > 0.0, "downhill raises it: {down_bias}");
    assert!(
        up_bias >= lens.hill_min - 1e-6 && down_bias <= lens.hill_max + 1e-6,
        "bounded by HillMin/HillMax: {up_bias} {down_bias}"
    );

    let lateral = Vec3::new(phi.sin(), phi.cos(), 0.0);
    assert!(hill_bias(&lens, lateral, Vec3::X).abs() < 1e-5);
}

/// `0x51e3e0`: the reverse swing scales the bias by `1 − 2f`,
/// `f = |angle|/π` clamped, so a full swing flips its sign.
#[test]
fn hill_pitch_flips_sign_at_a_full_reverse_swing() {
    let lens = hill_lens();
    let uphill = Vec3::new(0.0, 0.94, 0.34);
    let bias = hill_bias(&lens, uphill, Vec3::X);
    assert!(bias < 0.0);
    assert!((hill_pitch(&lens, uphill, Vec3::X, 0.0) - bias).abs() < 1e-6);
    assert!(hill_pitch(&lens, uphill, Vec3::X, std::f32::consts::PI) > 0.0);
    assert!(hill_pitch(&lens, uphill, Vec3::X, std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    // Beyond a full turn the factor is clamped, not wrapped.
    assert!((hill_pitch(&lens, uphill, Vec3::X, 3.0 * std::f32::consts::PI) + bias).abs() < 1e-6);
}

/// `HillLerp` low-passes the slope normal, snapping on the reset frame and
/// easing frame-rate independently at 60 Hz.
#[test]
fn hill_normal_snaps_then_low_passes() {
    let tilt = Vec3::new(0.0, 0.94, 0.34).normalize();
    // First tracked frame (`prev` None) and a reset frame both snap.
    assert!((hill_normal(None, tilt, 0.05, 1.0 / 60.0, false) - tilt).length() < 1e-6);
    assert!((hill_normal(Some(Vec3::Y), tilt, 0.05, 1.0 / 60.0, true) - tilt).length() < 1e-6);

    let one = hill_normal(Some(Vec3::Y), tilt, 0.05, 1.0 / 60.0, false);
    let want = (Vec3::Y + (tilt - Vec3::Y) * 0.05).normalize();
    assert!((one - want).length() < 1e-6, "{one:?} vs {want:?}");

    let s1 = hill_normal(Some(Vec3::Y), tilt, 0.05, 1.0 / 120.0, false);
    let two = hill_normal(Some(s1), tilt, 0.05, 1.0 / 120.0, false);
    assert!(
        (two - one).length() < 1e-4,
        "frame-rate independent: {two:?} vs {one:?}"
    );
}

/// End to end through `chase_follow`: on a slope the low-passed wheel hit
/// normals tilt the boom about the car's right axis — lower on an uphill,
/// higher on a downhill — and the flat case is untouched.
#[test]
fn chase_follow_tilts_the_boom_on_a_slope() {
    let lens = hill_lens();
    let eye_y = |n: Vec3| {
        let mut app = base_app(CameraMode::Chase);
        app.add_systems(Update, chase_follow);
        let car = spawn_vehicle(&mut app, Vec3::ZERO, Vec3::ZERO);
        app.world_mut().entity_mut(car).insert(hill_telemetry(vec![
            wheel(Vec3::new(-0.8, 0.0, -1.5), n),
            wheel(Vec3::new(0.8, 0.0, -1.5), n),
            wheel(Vec3::new(-0.8, 0.0, 1.5), n),
            wheel(Vec3::new(0.8, 0.0, 1.5), n),
        ]));
        let cam = spawn_chase(
            &mut app,
            ChaseCamera {
                near: lens.clone(),
                ..default()
            },
            true,
        );
        app.update();
        app.world().get::<Transform>(cam).unwrap().translation.y
    };
    let phi = 0.35f32;
    let flat = eye_y(Vec3::Y);
    let up = eye_y(Vec3::new(0.0, phi.cos(), phi.sin()));
    let down = eye_y(Vec3::new(0.0, phi.cos(), -phi.sin()));
    assert!(flat - up > 0.05, "uphill eye lower: flat {flat} up {up}");
    assert!(
        down - flat > 0.05,
        "downhill eye higher: flat {flat} down {down}"
    );
}
