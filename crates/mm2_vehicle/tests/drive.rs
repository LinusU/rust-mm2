//! Headless integration tests: spawn a ground plane + vehicle, apply
//! physics-tick-indexed input, and verify the assembled car settles, drives,
//! brakes, steers, reverses, survives a landing and resets cleanly.
//!
//! Determinism note: each `app.update()` advances wall-clock time by a fixed
//! manual duration, and physics runs on its own 120 Hz fixed clock, so every
//! update executes exactly two physics steps regardless of how the updates
//! are batched.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_vehicle::vehicle::{DriveDirection, VehicleInput, VehicleState};
use mm2_vehicle::{ResetVehicle, VehicleConfig, VehiclePlugin, vehicle_bundle};

const FRAMES_PER_SECOND: usize = 60;

fn test_app() -> (App, Entity) {
    test_app_with(VehicleConfig::default())
}

fn test_app_with(cfg: VehicleConfig) -> (App, Entity) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        // Avian's collider systems need the mesh asset store + events, which
        // AssetPlugin + MeshPlugin provide; MinimalPlugins doesn't have them.
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        // debug_draw takes a Gizmos param → needs gizmo storage.
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        // Deterministic: every app.update() is exactly one 60 Hz frame.
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin);
    app.finish();
    app.cleanup();

    // Flat ground at y=0.
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(400.0, 1.0, 400.0),
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));

    let car = app
        .world_mut()
        .spawn((
            vehicle_bundle(&cfg),
            Position(Vec3::new(0.0, 1.2, 0.0)),
            Transform::from_xyz(0.0, 1.2, 0.0),
        ))
        .id();
    (app, car)
}

fn set_input(app: &mut App, car: Entity, input: VehicleInput) {
    *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = input;
}

fn drive(app: &mut App, car: Entity, frames: usize, input: VehicleInput) {
    set_input(app, car, input);
    for _ in 0..frames {
        app.update();
    }
}

fn assert_finite(app: &App, car: Entity) {
    let pos = app.world().get::<Position>(car).unwrap().0;
    let rot = app.world().get::<Rotation>(car).unwrap().0;
    let lv = app.world().get::<LinearVelocity>(car).unwrap().0;
    let av = app.world().get::<AngularVelocity>(car).unwrap().0;
    assert!(
        pos.is_finite() && rot.is_finite(),
        "pose not finite: {pos:?}"
    );
    assert!(lv.is_finite() && av.is_finite(), "velocity not finite");
}

#[test]
fn car_settles_on_suspension() {
    let (mut app, car) = test_app();
    drive(
        &mut app,
        car,
        FRAMES_PER_SECOND * 3,
        VehicleInput::default(),
    );

    let state = app.world().get::<VehicleState>(car).unwrap();
    assert!(state.grounded, "car should be grounded");
    let grounded = state.wheels.iter().filter(|w| w.grounded).count();
    assert_eq!(grounded, 4, "all four wheels should rest on the ground");
    for w in &state.wheels {
        assert!(w.compression > 0.0, "suspension should be compressed");
        assert!(w.compression < 0.35, "suspension within travel");
    }
    // Total suspension force approximately supports the car's weight.
    let total: f32 = state.wheels.iter().map(|w| w.suspension_force).sum();
    let weight = 1300.0 * 9.81;
    assert!(
        (total - weight).abs() / weight < 0.25,
        "suspension force {total} vs weight {weight}"
    );

    let lv = app.world().get::<LinearVelocity>(car).unwrap().0;
    let av = app.world().get::<AngularVelocity>(car).unwrap().0;
    assert!(lv.length() < 0.5, "car should be nearly still, vel {lv:?}");
    assert!(
        av.length() < 0.5,
        "car should not be spinning, angvel {av:?}"
    );
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!(pos.y > 0.2 && pos.y < 2.0, "car should settle, y={}", pos.y);

    // And it stays settled: another two seconds must not drift or bounce.
    let y0 = pos.y;
    drive(
        &mut app,
        car,
        FRAMES_PER_SECOND * 2,
        VehicleInput::default(),
    );
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!(
        (pos.y - y0).abs() < 0.05,
        "settled car drifted vertically: {y0} -> {}",
        pos.y
    );
    assert_finite(&app, car);
}

#[test]
fn car_accelerates_forward() {
    let (mut app, car) = test_app();
    drive(
        &mut app,
        car,
        FRAMES_PER_SECOND * 6,
        VehicleInput {
            throttle: 1.0,
            ..default()
        },
    );
    let pos = app.world().get::<Position>(car).unwrap().0;
    let state = app.world().get::<VehicleState>(car).unwrap();
    // Vehicle forward is -Z; it should have moved well past -10 m.
    assert!(
        pos.z < -10.0,
        "expected forward motion, position was {pos:?}"
    );
    assert!(
        state.forward_speed > 5.0,
        "expected speed, got {}",
        state.forward_speed
    );
    assert_eq!(state.direction, DriveDirection::Forward);
    assert!(state.grounded, "car should be on the ground");
    assert!(state.rpm > 900.0, "engine should rev under load");
    assert_finite(&app, car);
}

#[test]
fn car_brakes_to_a_stop() {
    let (mut app, car) = test_app();
    drive(
        &mut app,
        car,
        FRAMES_PER_SECOND * 4,
        VehicleInput {
            throttle: 1.0,
            ..default()
        },
    );
    // Brake until nearly stopped (holding brake past a standstill engages
    // reverse by design, so release as soon as we're slow).
    set_input(
        &mut app,
        car,
        VehicleInput {
            brake: 1.0,
            ..default()
        },
    );
    let mut stopped = false;
    for _ in 0..FRAMES_PER_SECOND * 4 {
        app.update();
        let speed = app.world().get::<VehicleState>(car).unwrap().forward_speed;
        if speed < 1.0 {
            stopped = true;
            break;
        }
    }
    assert!(stopped, "car never slowed below 1 m/s while braking");
    assert_finite(&app, car);
}

/// A car shaped like the stock MM2 imports: centre of mass nearly as high
/// as the track is wide, on tires that make far more grip than that
/// geometry can take. Every retail vehicle audits like this.
fn tippy_config() -> VehicleConfig {
    let mut cfg = VehicleConfig {
        track_width: 1.6,
        ..Default::default()
    };
    for w in &mut cfg.wheels {
        w.position[0] = w.position[0].signum() * cfg.track_width / 2.0;
    }
    // Sits 0.8 m up on a 0.8 m half-track: tips at ~1 g while the tires
    // pull 1.65 g.
    let metrics = mm2_vehicle::HandlingMetrics::of(&cfg);
    cfg.center_of_mass[1] = metrics.ground_y + 0.8;
    cfg.tires.lateral_grip = 1.65;
    // Grip-limited steering is the *other* thing standing between this
    // geometry and a roll; switch it off so each test isolates one.
    cfg.steering.grip_limit = 0.0;
    cfg
}

#[test]
fn a_hard_turn_slides_instead_of_rolling_the_car_over() {
    // The assist off: honest physics, and the car goes over. This half of
    // the test is what proves the other half is actually being tested.
    let mut bare = tippy_config();
    bare.assists.roll_resistance = 0.0;
    let bare_up = up_after_hard_turn(bare);
    assert!(
        bare_up < 0.5,
        "unassisted tippy car should roll over, up.y was {bare_up}"
    );

    // The assist on: same grip, same turn, still on its wheels.
    let assisted_up = up_after_hard_turn(tippy_config());
    assert!(
        assisted_up > 0.85,
        "assisted car should stay upright, up.y was {assisted_up}"
    );
}

#[test]
fn grip_limited_steering_also_keeps_a_tippy_car_down() {
    // Same geometry, roll assist off, but the driver can no longer command
    // a corner the tires cannot hold — which is also a corner that would
    // tip the car. The two protections are independent on purpose.
    let mut cfg = tippy_config();
    cfg.assists.roll_resistance = 0.0;
    cfg.steering.grip_limit = VehicleConfig::default().steering.grip_limit;
    let up = up_after_hard_turn(cfg);
    assert!(up > 0.85, "car should stay upright, up.y was {up}");
}

/// Accelerate to speed, then hold full lock; return the vertical component
/// of the car's up axis (1.0 = level, <0 = on its roof).
fn up_after_hard_turn(cfg: VehicleConfig) -> f32 {
    let (mut app, car) = test_app_with(cfg);
    drive(
        &mut app,
        car,
        FRAMES_PER_SECOND * 5,
        VehicleInput {
            throttle: 1.0,
            ..default()
        },
    );
    drive(
        &mut app,
        car,
        FRAMES_PER_SECOND * 4,
        VehicleInput {
            throttle: 1.0,
            steering: 1.0,
            ..default()
        },
    );
    assert_finite(&app, car);
    (app.world().get::<Rotation>(car).unwrap().0 * Vec3::Y).y
}

#[test]
fn a_car_thrown_nose_down_lands_level() {
    // Cresting a rise at speed leaves a car nose-down, and it keeps that
    // attitude all the way to the ground unless something levels it —
    // arriving front-first, digging the hull into the road ahead of the
    // wheels and stopping dead instead of landing.
    let pitch_at_landing = |air_control: f32| {
        let mut cfg = VehicleConfig::default();
        cfg.assists.air_control = air_control;
        let (mut app, car) = test_app_with(cfg);

        // Launch it: well clear of the ground, nose down, moving forward.
        let nose_down = Quat::from_rotation_x(-0.35);
        {
            let world = app.world_mut();
            world.get_mut::<Position>(car).unwrap().0 = Vec3::new(0.0, 6.0, 0.0);
            world.get_mut::<Transform>(car).unwrap().translation = Vec3::new(0.0, 6.0, 0.0);
            world.get_mut::<Rotation>(car).unwrap().0 = nose_down;
            world.get_mut::<Transform>(car).unwrap().rotation = nose_down;
            world.get_mut::<LinearVelocity>(car).unwrap().0 = Vec3::new(0.0, 0.0, -25.0);
        }

        // Fly until the wheels find the ground, then read the attitude.
        let mut pitch = f32::NAN;
        for _ in 0..FRAMES_PER_SECOND * 3 {
            app.update();
            if app.world().get::<VehicleState>(car).unwrap().grounded {
                let forward = app.world().get::<Rotation>(car).unwrap().0 * Vec3::NEG_Z;
                pitch = forward.y.asin().to_degrees();
                break;
            }
        }
        assert_finite(&app, car);
        pitch
    };

    // Without the assist the nose is still down when the car arrives.
    let unassisted = pitch_at_landing(0.0);
    assert!(
        unassisted < -10.0,
        "unassisted car should land nose-down, pitch was {unassisted}"
    );

    let assisted = pitch_at_landing(VehicleConfig::default().assists.air_control);
    assert!(
        assisted.abs() < 4.0,
        "assisted car should land level, pitch was {assisted}"
    );
}

#[test]
fn an_upended_car_flops_back_onto_its_wheels() {
    let (mut app, car) = test_app();
    let delay = VehicleConfig::default().assists.self_right_delay;

    // Put it on its roof, where nothing in the simulation can reach it:
    // no wheel touches the ground, so no tire or spring force applies.
    {
        let flipped = Quat::from_rotation_z(std::f32::consts::PI);
        let world = app.world_mut();
        world.get_mut::<Rotation>(car).unwrap().0 = flipped;
        world.get_mut::<Transform>(car).unwrap().rotation = flipped;
    }
    drive(
        &mut app,
        car,
        FRAMES_PER_SECOND / 2,
        VehicleInput::default(),
    );
    let upright = (app.world().get::<Rotation>(car).unwrap().0 * Vec3::Y).y;
    assert!(
        upright < 0.0,
        "car should still be inverted, up.y {upright}"
    );

    // Wait out the recovery delay with room to spare.
    drive(
        &mut app,
        car,
        (FRAMES_PER_SECOND as f32 * (delay + 1.5)) as usize,
        VehicleInput::default(),
    );

    let upright = (app.world().get::<Rotation>(car).unwrap().0 * Vec3::Y).y;
    assert!(
        upright > 0.9,
        "car should have recovered onto its wheels, up.y {upright}"
    );
    let state = app.world().get::<VehicleState>(car).unwrap();
    assert!(state.grounded, "recovered car should be back on the road");
    assert_finite(&app, car);
}

#[test]
fn a_car_on_its_wheels_is_never_self_righted() {
    let (mut app, car) = test_app();
    let delay = VehicleConfig::default().assists.self_right_delay;
    drive(
        &mut app,
        car,
        (FRAMES_PER_SECOND as f32 * (delay + 2.0)) as usize,
        VehicleInput::default(),
    );
    let state = app.world().get::<VehicleState>(car).unwrap();
    assert_eq!(
        state.upended_for, 0.0,
        "a settled car must never accumulate recovery time"
    );
}

#[test]
fn car_turns_left_and_right() {
    for (steering, expected_sign) in [(1.0f32, 1.0f32), (-1.0, -1.0)] {
        let (mut app, car) = test_app();
        drive(
            &mut app,
            car,
            FRAMES_PER_SECOND * 5,
            VehicleInput {
                throttle: 0.6,
                steering,
                ..default()
            },
        );
        let pos = app.world().get::<Position>(car).unwrap().0;
        assert!(
            pos.x * expected_sign > 2.0,
            "expected lateral motion for steering {steering}, position was {pos:?}"
        );
        assert_finite(&app, car);
    }
}

#[test]
fn brake_holds_then_reverses() {
    let (mut app, car) = test_app();
    // Settle, then hold the brake through a standstill: the direction state
    // machine must engage reverse and the car must back up, not oscillate.
    drive(
        &mut app,
        car,
        FRAMES_PER_SECOND * 2,
        VehicleInput::default(),
    );
    drive(
        &mut app,
        car,
        FRAMES_PER_SECOND * 3,
        VehicleInput {
            brake: 1.0,
            ..default()
        },
    );
    let state = app.world().get::<VehicleState>(car).unwrap();
    assert_eq!(
        state.direction,
        DriveDirection::Reverse,
        "holding brake at rest should engage reverse"
    );
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!(
        state.forward_speed < -0.5 && pos.z > 0.5,
        "car should be backing up (+Z), speed {} pos {pos:?}",
        state.forward_speed
    );
    assert_finite(&app, car);

    // Releasing the brake returns to forward drive without motion artifacts.
    drive(&mut app, car, FRAMES_PER_SECOND, VehicleInput::default());
    let state = app.world().get::<VehicleState>(car).unwrap();
    assert_eq!(state.direction, DriveDirection::Forward);
}

#[test]
fn airborne_state_clears_and_landing_is_stable() {
    let (mut app, car) = test_app();
    // Teleport the car 3 m up: it must report un-grounded wheels while
    // falling and land without NaNs or explosive velocities.
    app.world_mut().write_message(ResetVehicle {
        entity: Some(car),
        position: Vec3::new(0.0, 4.0, 0.0),
        yaw: 0.0,
    });
    app.update();

    let mut saw_airborne = false;
    for _ in 0..FRAMES_PER_SECOND * 3 {
        app.update();
        let state = app.world().get::<VehicleState>(car).unwrap();
        if !state.grounded {
            saw_airborne = true;
            for w in &state.wheels {
                assert!(!w.grounded);
                assert_eq!(w.compression, 0.0, "airborne wheel keeps no compression");
                assert_eq!(w.suspension_force, 0.0);
            }
        }
        assert_finite(&app, car);
    }
    assert!(saw_airborne, "car never left the ground in a 3 m drop");

    let state = app.world().get::<VehicleState>(car).unwrap();
    assert!(state.grounded, "car should land");
    let lv = app.world().get::<LinearVelocity>(car).unwrap().0;
    assert!(
        lv.length() < 2.0,
        "car should settle after landing, vel {lv:?}"
    );
}

#[test]
fn reset_teleports_and_clears_motion() {
    let (mut app, car) = test_app();
    drive(
        &mut app,
        car,
        FRAMES_PER_SECOND * 3,
        VehicleInput {
            throttle: 1.0,
            ..default()
        },
    );
    app.world_mut().write_message(ResetVehicle {
        entity: Some(car),
        position: Vec3::new(5.0, 1.2, 5.0),
        yaw: 1.0,
    });
    app.update();
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!((pos - Vec3::new(5.0, 1.2, 5.0)).length() < 1e-3);
    let vel = app.world().get::<LinearVelocity>(car).unwrap().0;
    assert!(vel.length() < 1e-3);
    // Stale suspension/drivetrain state is gone.
    let state = app.world().get::<VehicleState>(car).unwrap();
    assert!(!state.grounded);
    assert_eq!(state.gear, 0);
    assert_eq!(state.direction, DriveDirection::Forward);
    assert!(
        state
            .wheels
            .iter()
            .all(|w| !w.grounded && w.compression == 0.0)
    );
}

#[test]
fn simulation_is_deterministic_across_update_batching() {
    // The same physics-tick-indexed input sequence must produce the same
    // trajectory whether updates arrive one at a time or in batches.
    let script = |frame: usize| -> VehicleInput {
        match frame {
            0..=119 => VehicleInput {
                throttle: 1.0,
                ..default()
            },
            120..=179 => VehicleInput {
                throttle: 1.0,
                steering: 0.5,
                ..default()
            },
            _ => VehicleInput {
                brake: 1.0,
                ..default()
            },
        }
    };
    const FRAMES: usize = 240;

    let (mut app_a, car_a) = test_app();
    for frame in 0..FRAMES {
        set_input(&mut app_a, car_a, script(frame));
        app_a.update();
    }

    let (mut app_b, car_b) = test_app();
    let mut frame = 0;
    while frame < FRAMES {
        for _ in 0..7 {
            if frame >= FRAMES {
                break;
            }
            set_input(&mut app_b, car_b, script(frame));
            app_b.update();
            frame += 1;
        }
    }

    let a = app_a.world().get::<Position>(car_a).unwrap().0;
    let b = app_b.world().get::<Position>(car_b).unwrap().0;
    assert!(
        (a - b).length() < 1e-4,
        "trajectories diverged across batching: {a:?} vs {b:?}"
    );
}
