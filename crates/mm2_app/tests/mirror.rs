//! F22-B.2 rear-view mirror synthetic coverage (HUD-3/CTL-1
//! `BACKSPACE rearview mirror`): the toggle, the phase and dev-camera
//! gates on what renders, the strip camera's exclusion from the
//! world-camera picks, and the authored-eye/fallback spawn path.

use std::time::Duration;

use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::camera::{
    CameraMode, ChaseCamera, MirrorCamera, RearView, active_cam_pose, drive_mirror, mirror_input,
    spawn_mirror,
};
use mm2_app::hudmap::WorldCamera3d;
use mm2_formats::dash::PovCamSpec;
use mm2_game::{PlayerVehicle, Session, SessionConfig, SessionEntity, SessionPhase};

fn session_at(phase: SessionPhase) -> Session {
    use SessionPhase::*;
    let mut s = Session::new();
    if phase == Menu {
        return s;
    }
    s.begin(SessionConfig::default()).unwrap(); // Loading
    // Walk the phase machine's legal edges to the requested phase —
    // e.g. Results is reached off Playing, never off Paused.
    let path: &[SessionPhase] = match phase {
        Ready => &[Ready],
        Countdown => &[Ready, Countdown],
        Playing => &[Ready, Countdown, Playing],
        Paused => &[Ready, Countdown, Playing, Paused],
        Results => &[Ready, Countdown, Playing, Results],
        other => panic!("no session_at path to {other:?}"),
    };
    for step in path {
        s.transition(step.clone()).unwrap();
    }
    s
}

fn base_app(phase: SessionPhase) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .init_resource::<ButtonInput<KeyCode>>()
        .insert_resource(CameraMode::Chase)
        .insert_resource(RearView::default())
        .insert_resource(session_at(phase))
        // Chained like the production order — the toggle is read
        // ahead of the driver that applies it (`drive_mirror` runs
        // after `drive_session` in the binary).
        .add_systems(Update, (mirror_input, drive_mirror).chain());
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

fn spawn_strip(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            MirrorCamera,
            Camera3d::default(),
            Camera {
                is_active: false,
                ..default()
            },
        ))
        .id()
}

/// BACKSPACE toggles the strip on and off while driving; the camera's
/// `is_active` follows the toggle through `drive_mirror`.
#[test]
fn backspace_toggles_the_strip_while_driving() {
    let mut app = base_app(SessionPhase::Playing);
    let strip = spawn_strip(&mut app);

    press(&mut app, KeyCode::Backspace);
    assert!(app.world().resource::<RearView>().0);
    assert!(app.world().get::<Camera>(strip).unwrap().is_active);

    press(&mut app, KeyCode::Backspace);
    assert!(!app.world().resource::<RearView>().0);
    assert!(!app.world().get::<Camera>(strip).unwrap().is_active);
}

/// Countdown counts as a driving phase — the mirror can be armed
/// before the green light.
#[test]
fn the_strip_arms_during_countdown() {
    let mut app = base_app(SessionPhase::Countdown);
    let strip = spawn_strip(&mut app);

    press(&mut app, KeyCode::Backspace);
    assert!(app.world().resource::<RearView>().0);
    assert!(app.world().get::<Camera>(strip).unwrap().is_active);
}

/// Outside the driving phases the key belongs to whoever owns the
/// screen — `Paused`/`Results` overlays use it for `Back`, the menu
/// for back/erase — so the toggle must not hear it there.
#[test]
fn the_key_stays_with_the_overlay_phases() {
    for phase in [
        SessionPhase::Menu,
        SessionPhase::Paused,
        SessionPhase::Results,
    ] {
        let mut app = base_app(phase.clone());
        let strip = spawn_strip(&mut app);
        press(&mut app, KeyCode::Backspace);
        assert!(
            !app.world().resource::<RearView>().0,
            "Backspace reached the mirror toggle in {phase:?}"
        );
        assert!(!app.world().get::<Camera>(strip).unwrap().is_active);
    }
}

/// The dev free camera yields nothing to the HUD: an armed mirror
/// renders in Chase and Cockpit but never over `Free`.
#[test]
fn free_camera_suppresses_the_strip() {
    let mut app = base_app(SessionPhase::Playing);
    let strip = spawn_strip(&mut app);
    *app.world_mut().resource_mut::<RearView>() = RearView(true);

    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::Free;
    app.update();
    assert!(
        !app.world().get::<Camera>(strip).unwrap().is_active,
        "the strip must not overlay the dev camera"
    );

    for mode in [CameraMode::Chase, CameraMode::Cockpit] {
        *app.world_mut().resource_mut::<CameraMode>() = mode;
        app.update();
        assert!(
            app.world().get::<Camera>(strip).unwrap().is_active,
            "the strip renders over {mode:?}"
        );
    }
}

/// The strip is a *second* active camera — every pick of "the" world
/// camera (`active_cam_pose` for the HUD `cam` readout/`--cam`
/// round-trip, the PVS source, the audio listener, the sky dome) goes
/// through `WorldCamera3d`, which must keep landing on the forward
/// view even while the mirror renders.
#[test]
fn the_strip_is_never_the_world_camera() {
    let mut app = base_app(SessionPhase::Playing);
    app.add_plugins(TransformPlugin);
    let vehicle = app
        .world_mut()
        .spawn((
            PlayerVehicle,
            Transform::from_translation(Vec3::new(500.0, 10.0, -300.0)),
        ))
        .id();
    app.world_mut().spawn((
        Camera3d::default(),
        Camera {
            is_active: true,
            ..default()
        },
        ChaseCamera::default(),
        Transform::from_translation(Vec3::new(10.0, 5.0, 0.0)),
    ));
    // The strip, armed on and facing rearward off the vehicle —
    // `drive_mirror` keeps `is_active` up the whole update.
    *app.world_mut().resource_mut::<RearView>() = RearView(true);
    let strip = app
        .world_mut()
        .spawn((
            MirrorCamera,
            Camera3d::default(),
            Camera {
                is_active: true,
                ..default()
            },
            Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
        ))
        .id();
    app.world_mut().entity_mut(vehicle).add_child(strip);
    app.update(); // PostUpdate propagates the child's world pose
    assert!(
        app.world().get::<Camera>(strip).unwrap().is_active,
        "the armed strip is live for this check"
    );

    let mut cams = app
        .world_mut()
        .query_filtered::<(&Camera, &GlobalTransform), WorldCamera3d>();
    let q = cams.query(app.world());
    // Only the forward view qualifies — the strip is not a pick at all.
    assert_eq!(q.iter().count(), 1);
    let pose = active_cam_pose(&q);
    assert_eq!(pose.as_deref(), Some("10.0,5.0,0.0,0,0"));
}

/// The cockpit/exterior split must never own the strip: under
/// `Cockpit` a `Hidden` camera renders nothing — the windshield mirror
/// would die inside the view it is most useful in — so the sweep skips
/// `MirrorCamera` children and never tags them `CockpitHidden`.
#[test]
fn the_visibility_split_never_claims_the_strip() {
    use mm2_app::dash::{CockpitHidden, sync_dash_visibility};

    let mut app = base_app(SessionPhase::Playing);
    app.add_systems(Update, sync_dash_visibility);
    let vehicle = app
        .world_mut()
        .spawn((PlayerVehicle, Visibility::Visible))
        .id();
    let strip = spawn_strip(&mut app);
    app.world_mut().entity_mut(vehicle).add_child(strip);
    let before = *app.world().get::<Visibility>(strip).unwrap();

    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::Cockpit;
    app.update();
    assert_eq!(
        *app.world().get::<Visibility>(strip).unwrap(),
        before,
        "the split must not hide the strip inside the cockpit"
    );
    assert!(
        app.world().get::<CockpitHidden>(strip).is_none(),
        "the split must not tag the strip"
    );

    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::Chase;
    app.update();
    assert_eq!(*app.world().get::<Visibility>(strip).unwrap(), before);
}

/// `spawn_mirror` parents the strip under the vehicle, rides the
/// authored `camPovCS` eye when the record exists (the seat position
/// the real mirror reflects from) and the designed fallback when it
/// does not — always rear-facing, always spawned inactive (`RearView`
/// owns `is_active`).
#[test]
fn spawn_binds_the_authored_eye_rear_facing() {
    let mut app = base_app(SessionPhase::Playing);
    let vehicle = app.world_mut().spawn(PlayerVehicle).id();
    let pov = PovCamSpec {
        offset: Some([0.4, 1.19, -0.55]),
        camera_fov: Some(70.0),
        camera_near: Some(0.05),
        camera_far: Some(800.0),
        ..PovCamSpec::default()
    };

    let strip = {
        let mut queue = CommandQueue::default();
        let strip = {
            let mut commands = Commands::new(&mut queue, app.world_mut());
            spawn_mirror(
                &mut commands,
                Some(&pov),
                Vec3::new(0.0, 9.9, 9.9),
                vehicle,
                SessionEntity(1),
                None,
            )
        };
        queue.apply(app.world_mut());
        strip
    };

    let w = app.world();
    let xf = w.get::<Transform>(strip).unwrap();
    assert_eq!(xf.translation, Vec3::new(0.4, 1.19, -0.55));
    // Rear-facing: yaw π off the vehicle's forward.
    assert!(
        xf.rotation
            .angle_between(Quat::from_rotation_y(std::f32::consts::PI))
            < 1e-5
    );
    assert!(!w.get::<Camera>(strip).unwrap().is_active);
    // `Camera3d` auto-inserts `Visibility`, which would put the strip
    // under `sync_dash_visibility`'s exterior sweep — it must be
    // skipped explicitly (a Hidden camera renders nothing, so the
    // mirror would die inside the cockpit view it is most useful in).
    assert!(
        w.get::<Visibility>(strip).is_some(),
        "Camera3d carries Visibility — the sweep exemption is load-bearing"
    );
    match w.get::<Projection>(strip).unwrap() {
        Projection::Perspective(p) => {
            assert!((p.fov - 70.0f32.to_radians()).abs() < 1e-6);
            assert!((p.near - 0.05).abs() < 1e-6);
            assert!((p.far - 800.0).abs() < 1e-6);
        }
        other => panic!("expected perspective, got {other:?}"),
    }

    // No record: the designed fallback eye and clip defaults apply.
    let strip = {
        let mut queue = CommandQueue::default();
        let strip = {
            let mut commands = Commands::new(&mut queue, app.world_mut());
            spawn_mirror(
                &mut commands,
                None,
                Vec3::new(0.0, 9.9, 9.9),
                vehicle,
                SessionEntity(1),
                None,
            )
        };
        queue.apply(app.world_mut());
        strip
    };
    assert_eq!(
        app.world().get::<Transform>(strip).unwrap().translation,
        Vec3::new(0.0, 9.9, 9.9)
    );
}
