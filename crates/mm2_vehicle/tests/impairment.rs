//! F05-B.7 sim-side tests: `EngineImpairment` scales drive output and
//! nothing else — an impaired car accelerates less, a full/garbage
//! factor is indistinguishable from no component, and a zero factor
//! delivers no drive without poisoning the rest of the vehicle.
//!
//! Same determinism contract as `drive.rs`: every `app.update()` is
//! exactly one 60 Hz frame and two 120 Hz physics steps.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_vehicle::vehicle::{VehicleInput, VehicleState};
use mm2_vehicle::{EngineImpairment, VehicleConfig, VehiclePlugin, vehicle_bundle};

fn test_app(scale: Option<f32>) -> (App, Entity) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin);
    app.finish();
    app.cleanup();

    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(400.0, 1.0, 400.0),
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));

    let car = app
        .world_mut()
        .spawn((
            vehicle_bundle(&VehicleConfig::default()),
            Position(Vec3::new(0.0, 1.2, 0.0)),
            Transform::from_xyz(0.0, 1.2, 0.0),
        ))
        .id();
    if let Some(scale) = scale {
        app.world_mut()
            .entity_mut(car)
            .insert(EngineImpairment(scale));
    }
    (app, car)
}

/// Settle, then hold full throttle for `frames`; returns the distance
/// driven along -Z (the car's forward).
fn drive_distance(scale: Option<f32>, frames: usize) -> f32 {
    let (mut app, car) = test_app(scale);
    for _ in 0..30 {
        app.update();
    }
    *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = VehicleInput {
        throttle: 1.0,
        ..default()
    };
    for _ in 0..frames {
        app.update();
    }
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!(pos.is_finite(), "position must stay finite: {pos}");
    -pos.z
}

#[test]
fn an_impaired_engine_accelerates_slower_but_still_drives() {
    let healthy = drive_distance(None, 300);
    let impaired = drive_distance(Some(0.4), 300);
    assert!(
        healthy > 10.0,
        "baseline sanity — the healthy car must drive: {healthy}"
    );
    assert!(
        impaired < healthy * 0.9,
        "impaired must clearly lag: {impaired} vs {healthy}"
    );
    assert!(
        impaired > healthy * 0.15,
        "a 0.4 factor limps, it does not stall: {impaired} vs {healthy}"
    );
}

#[test]
fn full_scale_and_absent_component_are_identical() {
    // A factor of 1.0 must not perturb the sim at all — the impaired
    // leg stays bit-identical to the unmodified run.
    let healthy = drive_distance(None, 300);
    assert_eq!(drive_distance(Some(1.0), 300), healthy);
    // Garbage sanitises to full output rather than stalling or
    // boosting the car.
    assert_eq!(drive_distance(Some(f32::NAN), 300), healthy);
    assert_eq!(drive_distance(Some(f32::INFINITY), 300), healthy);
    assert_eq!(drive_distance(Some(2.5), 300), healthy);
}

#[test]
fn zero_scale_delivers_no_drive_but_the_car_stays_healthy() {
    let (mut app, car) = test_app(Some(0.0));
    for _ in 0..30 {
        app.update();
    }
    *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = VehicleInput {
        throttle: 1.0,
        ..default()
    };
    for _ in 0..120 {
        app.update();
    }
    let state = app.world().get::<VehicleState>(car).unwrap();
    assert!(
        state.forward_speed.abs() < 0.1,
        "a zero factor delivers no drive: {}",
        state.forward_speed
    );
    // The rest of the vehicle is untouched: finite, grounded, and the
    // brakes still work (the wheels report contact normally).
    assert!(state.grounded);
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!(pos.is_finite());
}

#[test]
fn impairment_only_scales_engine_output_not_brakes() {
    // Build speed healthy, then swap in a zero factor and brake: the
    // stop must still happen — brakes are not engine output.
    let (mut app, car) = test_app(None);
    for _ in 0..30 {
        app.update();
    }
    *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = VehicleInput {
        throttle: 1.0,
        ..default()
    };
    for _ in 0..120 {
        app.update();
    }
    let fast = app.world().get::<VehicleState>(car).unwrap().forward_speed;
    assert!(fast > 5.0, "sanity — the car must be moving: {fast}");

    app.world_mut()
        .entity_mut(car)
        .insert(EngineImpairment(0.0));
    *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = VehicleInput {
        brake: 1.0,
        ..default()
    };
    for _ in 0..120 {
        app.update();
    }
    let speed = app.world().get::<VehicleState>(car).unwrap().forward_speed;
    assert!(
        speed.abs() < 0.5,
        "brakes are unimpaired — the car must stop: {speed}"
    );
}
