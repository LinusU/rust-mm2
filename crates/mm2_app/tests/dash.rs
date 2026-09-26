//! F22-B.1 cockpit/dashboard synthetic coverage: needle/wheel/gear drive
//! mapping from authoritative vehicle state, the cockpit-vs-exterior
//! visibility split, marker-based camera cycling (including the map
//! camera staying untouched) and the authored-data absence gate.

use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::camera::{CameraMode, ChaseCamera, FreeCamera, active_cam_pose, toggle_camera};
use mm2_app::car_visual::{GlowKind, GlowPart, HeadlightsOn, update_glows};
use mm2_app::dash::{
    CockpitCamera, CockpitHidden, CockpitPart, DashNode, DashRole, GearGlyph, cockpit_look,
    drive_dash, spawn_dash, sync_dash_visibility,
};
use mm2_app::hudmap::HudMapCamera;
use mm2_assets::Vfs;
use mm2_formats::dash::PovCamSpec;
use mm2_game::{PlayerVehicle, Session, SessionConfig, SessionEntity, SessionPhase};
use mm2_vehicle::{DriveDirection, Vehicle, VehicleConfig, VehicleInput, VehicleState};
use std::time::Duration;

fn test_config() -> VehicleConfig {
    let mut c = VehicleConfig {
        top_speed_mps: Some(50.0),
        ..Default::default()
    };
    c.engine.redline_rpm = 6000.0;
    c.engine.idle_rpm = 800.0;
    c.steering.low_speed_max_angle = 0.5;
    c
}

fn playing_session() -> Session {
    let mut s = Session::new();
    s.begin(SessionConfig::default()).unwrap();
    s.transition(SessionPhase::Ready).unwrap();
    s.transition(SessionPhase::Countdown).unwrap();
    s.transition(SessionPhase::Playing).unwrap();
    s
}

fn base_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<mm2_app::hud::HudVisible>()
        .insert_resource(CameraMode::Chase)
        .insert_resource(playing_session());
    app
}

fn spawn_player(app: &mut App, config: &VehicleConfig, state: VehicleState) -> Entity {
    app.world_mut()
        .spawn((
            PlayerVehicle,
            Vehicle {
                config: config.clone(),
            },
            state,
            Visibility::Visible,
        ))
        .id()
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

fn z_angle(q: Quat) -> f32 {
    let (_, _, z) = q.to_euler(EulerRot::XYZ);
    z
}

#[test]
fn needles_track_authoritative_state() {
    let mut app = base_app();
    app.add_systems(Update, drive_dash);
    let config = test_config();
    let mut state = VehicleState::new(&config);
    state.forward_speed = 25.0; // half the authored top speed
    state.rpm = 3000.0; // half redline
    let player = spawn_player(&mut app, &config, state);

    let speedo = app
        .world_mut()
        .spawn((
            DashNode {
                role: DashRole::Speed { min: 0.0, max: 3.6 },
            },
            Transform::default(),
        ))
        .id();
    let tach = app
        .world_mut()
        .spawn((
            DashNode {
                role: DashRole::Tach {
                    min: -0.1,
                    max: 5.0,
                },
            },
            Transform::default(),
        ))
        .id();
    let dmg = app
        .world_mut()
        .spawn((
            DashNode {
                role: DashRole::Damage { min: 0.0, max: 3.0 },
            },
            Transform::default(),
        ))
        .id();
    app.update();

    let w = app.world();
    let s = z_angle(w.get::<Transform>(speedo).unwrap().rotation);
    let t = z_angle(w.get::<Transform>(tach).unwrap().rotation);
    let d = z_angle(w.get::<Transform>(dmg).unwrap().rotation);
    assert!((s - 1.8).abs() < 1e-5, "speedo half-sweep, got {s}");
    assert!((t - 2.45).abs() < 1e-5, "tach half-sweep, got {t}");
    // No authored damage record → needle parked at min, not fabricated.
    assert_eq!(d, 0.0);

    // Full-scale pins at the authored max.
    app.world_mut()
        .get_mut::<VehicleState>(player)
        .unwrap()
        .forward_speed = 75.0; // beyond top speed → clamps
    app.update();
    let rot = app.world().get::<Transform>(speedo).unwrap().rotation;
    // 3.6 rad > π wraps in euler space — compare quaternions instead.
    assert!(rot.angle_between(Quat::from_rotation_z(3.6)) < 1e-4);
}

#[test]
fn wheel_and_gear_follow_the_sim() {
    let mut app = base_app();
    app.add_systems(Update, drive_dash);
    let config = test_config();
    let mut state = VehicleState::new(&config);
    state.steer_angle = 0.25; // half of the 0.5 rad lock
    state.direction = DriveDirection::Forward;
    state.gear = 2;
    spawn_player(&mut app, &config, state);

    let wheel = app
        .world_mut()
        .spawn((
            DashNode {
                role: DashRole::Wheel { factor: 0.9 },
            },
            Transform::default(),
        ))
        .id();
    // Gear glyph: authored slot table R, N, One…Six, D (9 materials).
    app.init_resource::<Assets<StandardMaterial>>();
    let mut mats = app.world_mut().resource_mut::<Assets<StandardMaterial>>();
    let glyph_mats: Vec<Handle<StandardMaterial>> = (0..9)
        .map(|_| mats.add(StandardMaterial::default()))
        .collect();
    let gear = app
        .world_mut()
        .spawn((
            GearGlyph {
                materials: glyph_mats.clone(),
                slot: usize::MAX,
            },
            MeshMaterial3d(glyph_mats[0].clone()),
        ))
        .id();
    app.update();

    let w = app.world();
    let rot = z_angle(w.get::<Transform>(wheel).unwrap().rotation);
    // 0.5 of lock × 0.9 half-turns × π, negative for right-hand steer.
    let want = -(0.25f32 / 0.5) * 0.9 * std::f32::consts::PI;
    assert!((rot - want).abs() < 1e-5, "wheel roll {rot} vs {want}");

    let glyph = w.get::<GearGlyph>(gear).unwrap();
    assert_eq!(glyph.slot, 4); // forward gear 2 (0-based) → slot 4 ('Three')
    let bound = w.get::<MeshMaterial3d<StandardMaterial>>(gear).unwrap();
    assert_eq!(bound.0, glyph_mats[4]);

    // Reverse binds slot 0 ('R').
    let mut st = app
        .world_mut()
        .query_filtered::<&mut VehicleState, With<PlayerVehicle>>()
        .single_mut(app.world_mut())
        .unwrap();
    st.direction = DriveDirection::Reverse;
    app.update();
    let glyph = app.world().get::<GearGlyph>(gear).unwrap();
    assert_eq!(glyph.slot, 0);
    let bound = app
        .world()
        .get::<MeshMaterial3d<StandardMaterial>>(gear)
        .unwrap();
    assert_eq!(bound.0, glyph_mats[0]);
}

#[test]
fn camera_cycle_uses_markers_and_skips_absent_cockpit() {
    let mut app = base_app();
    app.add_systems(Update, toggle_camera);
    let chase = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera {
                is_active: true,
                ..default()
            },
            ChaseCamera::default(),
        ))
        .id();
    let free = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera::default(),
            FreeCamera::default(),
        ))
        .id();
    // A stray camera with no session marker — the HUD-map case: its
    // is_active is owned by its own system and must not be touched.
    let map = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera {
                is_active: true,
                ..default()
            },
            HudMapCamera,
        ))
        .id();

    // C skips Cockpit (no CockpitCamera exists) → Free.
    press(&mut app, KeyCode::KeyC);
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Free);
    assert!(!app.world().get::<Camera>(chase).unwrap().is_active);
    assert!(app.world().get::<Camera>(free).unwrap().is_active);
    assert!(app.world().get::<Camera>(map).unwrap().is_active);

    press(&mut app, KeyCode::KeyC);
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Chase);
    assert!(app.world().get::<Camera>(chase).unwrap().is_active);

    // V with no cockpit cam is a no-op.
    press(&mut app, KeyCode::KeyV);
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Chase);
}

#[test]
fn camera_cycle_binds_the_cockpit() {
    let mut app = base_app();
    app.add_systems(Update, toggle_camera);
    let chase = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera {
                is_active: true,
                ..default()
            },
            ChaseCamera::default(),
        ))
        .id();
    let cockpit = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera::default(),
            CockpitCamera {
                offset: Vec3::new(0.0, 1.19, -0.55),
                reverse_offset: Some(Vec3::new(0.0, 1.7, 0.75)),
                pitch: 0.0,
                look_yaw: 0.0,
            },
        ))
        .id();

    // V jumps straight into the cockpit view.
    press(&mut app, KeyCode::KeyV);
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Cockpit);
    assert!(app.world().get::<Camera>(cockpit).unwrap().is_active);
    assert!(!app.world().get::<Camera>(chase).unwrap().is_active);

    // V again leaves it; C from Chase reaches Cockpit too.
    press(&mut app, KeyCode::KeyV);
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Chase);
    press(&mut app, KeyCode::KeyC);
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Cockpit);
    assert!(app.world().get::<Camera>(cockpit).unwrap().is_active);
}

#[test]
fn cockpit_view_splits_visibility() {
    let mut app = base_app();
    app.add_systems(Update, sync_dash_visibility);
    let config = test_config();
    let player = spawn_player(&mut app, &config, VehicleState::new(&config));

    let exterior = app
        .world_mut()
        .spawn((Visibility::Visible, Transform::default()))
        .id();
    let interior = app
        .world_mut()
        .spawn((Visibility::Visible, CockpitPart, Transform::default()))
        .id();
    app.world_mut().entity_mut(player).add_child(exterior);
    app.world_mut().entity_mut(player).add_child(interior);

    // Chase: everything visible.
    app.update();
    assert_eq!(
        *app.world().get::<Visibility>(exterior).unwrap(),
        Visibility::Visible
    );

    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::Cockpit;
    app.update();
    assert_eq!(
        *app.world().get::<Visibility>(exterior).unwrap(),
        Visibility::Hidden
    );
    assert_eq!(
        *app.world().get::<Visibility>(interior).unwrap(),
        Visibility::Visible
    );

    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::Chase;
    app.update();
    assert_eq!(
        *app.world().get::<Visibility>(exterior).unwrap(),
        Visibility::Visible
    );
}

/// The split only ever restores what it hid: a node another system
/// already `Hidden` — the detached-breakaway-panel case — stays down in
/// every mode, `GlowPart` carriers stay `update_glows`' business, and a
/// lit lamp never wins inside the cockpit (the binary orders the split
/// after the glow owner for exactly that).
#[test]
fn cockpit_split_respects_other_visibility_owners() {
    let mut app = base_app();
    app.init_resource::<HeadlightsOn>();
    // The binary's order: the glow owner writes, the split reconciles.
    app.add_systems(Update, (update_glows, sync_dash_visibility).chain());
    let config = test_config();
    let mut state = VehicleState::new(&config);
    state.direction = DriveDirection::Forward;
    let player = spawn_player(&mut app, &config, state);
    app.world_mut()
        .entity_mut(player)
        .insert(VehicleInput::default());

    // An ordinary exterior panel, a node another system already hid
    // (the detached-breakaway case), and an unlit brake glow.
    let panel = app
        .world_mut()
        .spawn((Visibility::Visible, Transform::default()))
        .id();
    let detached = app
        .world_mut()
        .spawn((Visibility::Hidden, Transform::default()))
        .id();
    let brake = app
        .world_mut()
        .spawn((
            GlowPart(GlowKind::Brake),
            Visibility::Hidden,
            Transform::default(),
        ))
        .id();
    for child in [panel, detached, brake] {
        app.world_mut().entity_mut(player).add_child(child);
    }

    // Chase: the sweep must not re-show what it does not own — the
    // detached panel rendered on top of its fragment before.
    app.update();
    assert_eq!(
        *app.world().get::<Visibility>(panel).unwrap(),
        Visibility::Visible
    );
    for e in [detached, brake] {
        assert_eq!(
            *app.world().get::<Visibility>(e).unwrap(),
            Visibility::Hidden,
            "an owner-hidden node must stay hidden in default Chase"
        );
    }

    // Cockpit: the panel earns the split's tag; the owner-hidden nodes
    // do not.
    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::Cockpit;
    app.update();
    assert!(app.world().get::<CockpitHidden>(panel).is_some());
    assert!(app.world().get::<CockpitHidden>(detached).is_none());
    assert!(app.world().get::<CockpitHidden>(brake).is_none());
    for e in [panel, detached, brake] {
        assert_eq!(
            *app.world().get::<Visibility>(e).unwrap(),
            Visibility::Hidden
        );
    }

    // A lit lamp mid-cockpit still loses to the split — the owner wrote
    // `Visible`, the split hides it after.
    app.world_mut()
        .get_mut::<VehicleInput>(player)
        .unwrap()
        .brake = 1.0;
    app.update();
    assert_eq!(
        *app.world().get::<Visibility>(brake).unwrap(),
        Visibility::Hidden,
        "a lit brake glow stays hidden inside the cockpit"
    );
    app.world_mut()
        .get_mut::<VehicleInput>(player)
        .unwrap()
        .brake = 0.0;

    // Back in Chase: only the tagged panel comes back — the detached
    // node and the unlit glow keep their owners' Hidden.
    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::Chase;
    app.update();
    assert_eq!(
        *app.world().get::<Visibility>(panel).unwrap(),
        Visibility::Visible
    );
    assert!(app.world().get::<CockpitHidden>(panel).is_none());
    for e in [detached, brake] {
        assert_eq!(
            *app.world().get::<Visibility>(e).unwrap(),
            Visibility::Hidden,
            "an owner-hidden node must never be re-shown on exit"
        );
    }

    // And the glow's owner still owns it — braking relights the quad.
    app.world_mut()
        .get_mut::<VehicleInput>(player)
        .unwrap()
        .brake = 1.0;
    app.update();
    assert_eq!(
        *app.world().get::<Visibility>(brake).unwrap(),
        Visibility::Visible
    );
}

/// With no recognised session camera at all — the menu phase, an empty
/// world — `C` must not drift `CameraMode`: the old loop settled on an
/// arbitrary step, which could leave a later session's mode pointing at
/// a camera that does not exist.
#[test]
fn camera_cycle_without_session_cameras_is_a_no_op() {
    let mut app = base_app();
    app.add_systems(Update, toggle_camera);
    // An unmarked camera (the HUD-map case) does not count — its
    // is_active is owned elsewhere.
    app.world_mut().spawn((Camera3d::default(), HudMapCamera));

    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::Cockpit;
    press(&mut app, KeyCode::KeyC);
    assert_eq!(
        *app.world().resource::<CameraMode>(),
        CameraMode::Cockpit,
        "a press with zero marked cameras must not drift the mode"
    );
    press(&mut app, KeyCode::KeyC);
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Cockpit);

    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::Chase;
    press(&mut app, KeyCode::KeyC);
    assert_eq!(*app.world().resource::<CameraMode>(), CameraMode::Chase);
}

#[test]
fn numpad_look_glances_and_reverses() {
    let mut app = base_app();
    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::Cockpit;
    app.add_systems(Update, cockpit_look);
    let cam = app
        .world_mut()
        .spawn((
            CockpitCamera {
                offset: Vec3::new(0.0, 1.19, -0.55),
                reverse_offset: Some(Vec3::new(0.0, 1.7, 0.75)),
                pitch: 0.0,
                look_yaw: 0.0,
            },
            Transform::from_translation(Vec3::new(0.0, 1.19, -0.55)),
        ))
        .id();

    // Held numpad-4 eases the view left.
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Numpad4);
    for _ in 0..30 {
        app.update();
    }
    let yaw = app.world().get::<CockpitCamera>(cam).unwrap().look_yaw;
    assert!(yaw > 1.3, "look_yaw eased toward +π/2, got {yaw}");

    // Releasing returns to the authored pose.
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .reset_all();
    for _ in 0..30 {
        app.update();
    }
    let c = app.world().get::<CockpitCamera>(cam).unwrap();
    assert!(
        c.look_yaw.abs() < 0.1,
        "released look eases home, got {}",
        c.look_yaw
    );

    // Numpad-2 rides the authored ReverseOffset.
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Numpad2);
    app.update();
    let xf = app.world().get::<Transform>(cam).unwrap();
    assert_eq!(xf.translation, Vec3::new(0.0, 1.7, 0.75));
}

/// `active_cam_pose` feeds the HUD `cam` readout and the screenshot
/// filename — the `--cam` round-trip contract. A camera *parented* to
/// the vehicle (the authored cockpit camera) must report its world
/// pose: its `Transform` is the car-space eye offset, and reading it
/// would print that offset instead of the pose `--cam` reproduces.
#[test]
fn cam_pose_reports_a_child_cameras_world_pose() {
    let mut app = base_app();
    app.add_plugins(TransformPlugin);
    let vehicle = app
        .world_mut()
        .spawn((
            PlayerVehicle,
            Transform::from_translation(Vec3::new(500.0, 10.0, -300.0)),
        ))
        .id();
    let eye = Vec3::new(0.0, 1.2, -0.5);
    let cam = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera {
                is_active: true,
                ..default()
            },
            CockpitCamera {
                offset: eye,
                reverse_offset: None,
                pitch: 0.0,
                look_yaw: 0.0,
            },
            Transform::from_translation(eye),
        ))
        .id();
    app.world_mut().entity_mut(vehicle).add_child(cam);
    app.update(); // PostUpdate propagates the child's world pose

    let mut cams = app
        .world_mut()
        .query_filtered::<(&Camera, &GlobalTransform), mm2_app::hudmap::WorldCamera3d>();
    let pose = active_cam_pose(&cams.query(app.world()));
    // The world eye pose — a `Transform` read would have printed the
    // car-local offset "0.0,1.2,-0.5,0,0" instead.
    assert_eq!(pose.as_deref(), Some("500.0,11.2,-300.5,0,0"));
}

#[test]
fn spawn_dash_reports_absent_without_authored_records() {
    // No mounted content at all: every authored gate misses and the
    // report says why instead of a fabricated dash appearing.
    let mut app = base_app();
    app.init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>();
    let vehicle = app
        .world_mut()
        .spawn((PlayerVehicle, Visibility::Visible))
        .id();
    let vfs = Vfs::new();

    let report = app
        .world_mut()
        .resource_scope(|world, mut meshes: Mut<Assets<Mesh>>| {
            world.resource_scope(|world, mut images: Mut<Assets<Image>>| {
                world.resource_scope(|world, mut materials: Mut<Assets<StandardMaterial>>| {
                    let mut queue = CommandQueue::default();
                    let report = {
                        let mut commands = Commands::new(&mut queue, world);
                        spawn_dash(
                            &mut commands,
                            &vfs,
                            "nonexistent_car",
                            0,
                            None,
                            &mut meshes,
                            &mut images,
                            &mut materials,
                            vehicle,
                            SessionEntity(1),
                            CameraMode::Chase,
                            None,
                        )
                    };
                    queue.apply(world);
                    report
                })
            })
        });

    assert_eq!(report.absent.as_deref(), Some("missing-spec+pkg"));
    assert!(!report.cockpit);
    assert_eq!(report.parts, 0);
    let mut q = app
        .world_mut()
        .query_filtered::<Entity, With<CockpitPart>>();
    assert_eq!(q.iter(app.world()).count(), 0);
}

/// Four retail `_dash.campovcs` records author `CameraNear 3.0`, which
/// would clip the entire interior cluster (~1 m ahead of the eye) into
/// a bare windshield view. The cockpit camera caps the authored near
/// plane at the designed 0.5 m bound (UNK-37); smaller authored values
/// pass through untouched.
#[test]
fn cockpit_near_clip_is_capped_for_the_interior() {
    fn pov(near: f32) -> PovCamSpec {
        PovCamSpec {
            type_tag: None,
            offset: Some([0.0, 1.6, 0.7]),
            reverse_offset: None,
            pitch: Some(0.0),
            track_to: None,
            camera_fov: Some(56.0),
            camera_near: Some(near),
            camera_far: Some(1330.0),
            extra_fields: Vec::new(),
        }
    }
    fn spawn_with(near: f32) -> App {
        let mut app = base_app();
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<StandardMaterial>>();
        let vehicle = app
            .world_mut()
            .spawn((PlayerVehicle, Visibility::Visible))
            .id();
        let vfs = Vfs::new();
        app.world_mut()
            .resource_scope(|world, mut meshes: Mut<Assets<Mesh>>| {
                world.resource_scope(|world, mut images: Mut<Assets<Image>>| {
                    world.resource_scope(|world, mut materials: Mut<Assets<StandardMaterial>>| {
                        let mut queue = CommandQueue::default();
                        {
                            let mut commands = Commands::new(&mut queue, world);
                            spawn_dash(
                                &mut commands,
                                &vfs,
                                "nonexistent_car",
                                0,
                                Some(pov(near)),
                                &mut meshes,
                                &mut images,
                                &mut materials,
                                vehicle,
                                SessionEntity(1),
                                CameraMode::Cockpit,
                                None,
                            );
                        }
                        queue.apply(world);
                    })
                })
            });
        app
    }

    for (authored, want) in [(3.0_f32, 0.5_f32), (0.1, 0.1)] {
        let mut app = spawn_with(authored);
        let mut q = app
            .world_mut()
            .query_filtered::<&Projection, With<CockpitCamera>>();
        let Projection::Perspective(p) = q.single(app.world()).unwrap() else {
            panic!("cockpit camera keeps a perspective projection");
        };
        assert!(
            (p.near - want).abs() < 1e-4,
            "authored near {authored} → {want}, got {}",
            p.near
        );
    }
}
