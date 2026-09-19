//! Headless integration test: spawn a ground plane + vehicle, apply input,
//! and verify the simulated car actually drives, brakes and resets.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_vehicle::vehicle::{VehicleInput, VehicleState};
use mm2_vehicle::{ResetVehicle, VehicleConfig, VehiclePlugin, vehicle_bundle};

fn test_app() -> (App, Entity) {
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

    let cfg = VehicleConfig::default();
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

fn drive(app: &mut App, car: Entity, frames: usize, input: VehicleInput) {
    *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = input;
    for _ in 0..frames {
        app.update();
    }
}

#[test]
fn car_accelerates_forward() {
    let (mut app, car) = test_app();
    drive(
        &mut app,
        car,
        60 * 6, // 6 simulated seconds
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
    assert!(state.grounded, "car should be on the ground");
    assert!(pos.y > 0.2 && pos.y < 2.0, "car should settle, y={}", pos.y);
}

#[test]
fn car_brakes_to_a_stop() {
    let (mut app, car) = test_app();
    drive(
        &mut app,
        car,
        60 * 4,
        VehicleInput {
            throttle: 1.0,
            ..default()
        },
    );
    // Brake until nearly stopped (holding brake past a standstill engages
    // reverse by design, so release as soon as we're slow).
    *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = VehicleInput {
        brake: 1.0,
        ..default()
    };
    let mut stopped = false;
    for _ in 0..60 * 4 {
        app.update();
        let speed = app.world().get::<VehicleState>(car).unwrap().forward_speed;
        if speed < 1.0 {
            stopped = true;
            break;
        }
    }
    assert!(stopped, "car never slowed below 1 m/s while braking");
}

#[test]
fn car_turns() {
    let (mut app, car) = test_app();
    drive(
        &mut app,
        car,
        60 * 5,
        VehicleInput {
            throttle: 0.6,
            steering: 1.0,
            ..default()
        },
    );
    let pos = app.world().get::<Position>(car).unwrap().0;
    // Steered hard right (+x) while driving forward (-z).
    assert!(pos.x > 2.0, "expected lateral motion, position was {pos:?}");
}

#[test]
fn reset_teleports_and_clears_motion() {
    let (mut app, car) = test_app();
    drive(
        &mut app,
        car,
        60 * 3,
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
}
