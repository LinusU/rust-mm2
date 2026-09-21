//! F06-B surface grip through the real Avian force path: a collider's
//! `TireSurface` times the session's `TireConditions` scale each
//! grounded wheel's tire limits — recorded per wheel in
//! `WheelState::surface_grip` and measurable in how the car drives.
//! Synthetic colliders, no game-content dependency.
//!
//! Determinism note: same as `tests/drive.rs` — each `app.update()`
//! advances one 60 Hz frame, two 120 Hz physics steps.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_vehicle::vehicle::{VehicleInput, VehicleState};
use mm2_vehicle::{TireConditions, TireSurface, VehicleConfig, VehiclePlugin, vehicle_bundle};

const FRAMES_PER_SECOND: usize = 60;

fn test_app() -> App {
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
    app
}

fn spawn_car(app: &mut App, pos: Vec3) -> Entity {
    app.world_mut()
        .spawn((
            vehicle_bundle(&VehicleConfig::default()),
            Position(pos),
            Transform::from_translation(pos),
        ))
        .id()
}

/// Spawn a flat static slab centred at `centre`, optionally carrying a
/// `TireSurface` — the collider marking the tire path reads.
fn slab(app: &mut App, centre: Vec3, surface: Option<TireSurface>) {
    let mut slab = app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(200.0, 1.0, 400.0),
        Position(centre),
        Transform::from_translation(centre),
    ));
    if let Some(surface) = surface {
        slab.insert(surface);
    }
}

fn drive(app: &mut App, car: Entity, frames: usize, input: VehicleInput) {
    *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = input;
    for _ in 0..frames {
        app.update();
    }
}

#[test]
fn each_wheel_reports_the_grip_of_the_collider_under_it() {
    let mut app = test_app();
    // Two abutting slabs: the unmarked left half is the neutral
    // reference, the right half is marked slippery. The car straddles
    // the seam — right-side wheels (local +x) read the marked grip.
    slab(&mut app, Vec3::new(-100.0, -0.5, 0.0), None);
    slab(
        &mut app,
        Vec3::new(100.0, -0.5, 0.0),
        Some(TireSurface { grip: 0.5 }),
    );
    let car = spawn_car(&mut app, Vec3::new(0.0, 1.2, 0.0));

    drive(
        &mut app,
        car,
        FRAMES_PER_SECOND * 2,
        VehicleInput::default(),
    );

    let cfg = VehicleConfig::default();
    let state = app.world().get::<VehicleState>(car).unwrap();
    for (i, w) in state.wheels.iter().enumerate() {
        assert!(w.grounded, "wheel {i} settled");
        let expected = if cfg.wheels[i].position[0] > 0.0 {
            0.5
        } else {
            1.0
        };
        assert_eq!(
            w.surface_grip, expected,
            "wheel {i} over x={}",
            cfg.wheels[i].position[0]
        );
    }
}

#[test]
fn the_environment_modifier_multiplies_the_materials_grip() {
    let mut app = test_app();
    app.insert_resource(TireConditions { traction: 0.5 });
    slab(
        &mut app,
        Vec3::new(0.0, -0.5, 0.0),
        Some(TireSurface { grip: 0.5 }),
    );
    let car = spawn_car(&mut app, Vec3::new(0.0, 1.2, 0.0));

    drive(
        &mut app,
        car,
        FRAMES_PER_SECOND * 2,
        VehicleInput::default(),
    );

    let state = app.world().get::<VehicleState>(car).unwrap();
    for w in &state.wheels {
        assert!(w.grounded);
        assert_eq!(w.surface_grip, 0.25, "material × environment");
    }
}

#[test]
fn the_environment_modifier_scales_an_unmarked_surface_too() {
    let mut app = test_app();
    app.insert_resource(TireConditions { traction: 0.6 });
    slab(&mut app, Vec3::new(0.0, -0.5, 0.0), None);
    let car = spawn_car(&mut app, Vec3::new(0.0, 1.2, 0.0));

    drive(
        &mut app,
        car,
        FRAMES_PER_SECOND * 2,
        VehicleInput::default(),
    );

    let state = app.world().get::<VehicleState>(car).unwrap();
    for w in &state.wheels {
        assert!(w.grounded);
        assert_eq!(w.surface_grip, 0.6);
    }
}

#[test]
fn an_airborne_wheel_reports_neutral_grip() {
    let mut app = test_app();
    app.insert_resource(TireConditions { traction: 0.5 });
    slab(
        &mut app,
        Vec3::new(0.0, -0.5, 0.0),
        Some(TireSurface { grip: 0.5 }),
    );
    // Still falling toward the marked slab: no contact, no surface.
    let car = spawn_car(&mut app, Vec3::new(0.0, 4.0, 0.0));
    for _ in 0..5 {
        app.update();
    }

    let state = app.world().get::<VehicleState>(car).unwrap();
    for w in &state.wheels {
        assert!(!w.grounded, "still falling");
        assert_eq!(w.surface_grip, 1.0, "airborne reports unmodified");
    }
}

/// Forward speed after `seconds` of full throttle from rest on a slab
/// carrying `surface` under `conditions`.
fn speed_after_launch(surface: Option<TireSurface>, conditions: f32, seconds: usize) -> f32 {
    let mut app = test_app();
    app.insert_resource(TireConditions {
        traction: conditions,
    });
    slab(&mut app, Vec3::new(0.0, -0.5, 0.0), surface);
    let car = spawn_car(&mut app, Vec3::new(0.0, 1.2, 0.0));
    drive(
        &mut app,
        car,
        FRAMES_PER_SECOND * seconds,
        VehicleInput {
            throttle: 1.0,
            ..default()
        },
    );
    app.world().get::<VehicleState>(car).unwrap().forward_speed
}

#[test]
fn a_slippery_surface_limits_delivered_drive_force() {
    // From a standstill the drivetrain asks far more than the tires can
    // hold, so delivered force sits at the limit the surface scales —
    // two distinct grips produce measurably different acceleration
    // (F06-AC02's surface leg).
    let reference = speed_after_launch(None, 1.0, 4);
    let slippery = speed_after_launch(Some(TireSurface { grip: 0.4 }), 1.0, 4);
    assert!(
        reference > 15.0,
        "reference launch should be well underway, {reference} m/s"
    );
    assert!(
        slippery < reference * 0.7,
        "slippery launch {slippery} m/s vs reference {reference} m/s"
    );
}

#[test]
fn a_wet_environment_limits_delivered_drive_force() {
    // The same measurement through the environment term alone — a
    // controlled wetness change produces the difference (F06-AC02's
    // modifier leg), without touching any collider.
    let dry = speed_after_launch(None, 1.0, 4);
    let wet = speed_after_launch(None, 0.4, 4);
    assert!(wet < dry * 0.7, "wet launch {wet} m/s vs dry {dry} m/s");
}
