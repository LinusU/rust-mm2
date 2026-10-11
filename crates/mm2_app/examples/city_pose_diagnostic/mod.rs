//! Isolate within-frame pose rounding from the production contact/force laws.
//! This is an instrument only; captures using it are not ordinary replay evidence.
use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_vehicle::{Vehicle, VehicleState};
use std::collections::HashMap;

#[derive(Resource)]
struct PoseMode {
    com: bool,
    rotation: bool,
    age: bool,
}

#[derive(Resource, Default)]
struct RawBasisHistory(HashMap<Entity, Mat3>);

pub fn install(app: &mut App, mode: &str) {
    let (com, rotation, age) = match mode {
        "com" => (true, false, false),
        "rotation" => (false, true, false),
        "both" => (true, true, false),
        "age" => (false, true, true),
        _ => panic!("MM2_DIAGNOSTIC_POSE must be com, rotation, both or age"),
    };
    app.insert_resource(PoseMode { com, rotation, age })
        .init_resource::<RawBasisHistory>()
        .add_systems(
            PhysicsSchedule,
            advance_once
                .after(PhysicsStepSystems::Solver)
                .before(PhysicsStepSystems::Sleeping)
                .before(PhysicsStepSystems::Last),
        );
}

fn advance_once(
    time: Res<Time>,
    mode: Res<PoseMode>,
    mut history: ResMut<RawBasisHistory>,
    mut cars: Query<(
        Entity,
        &Vehicle,
        &VehicleState,
        &LinearVelocity,
        &AngularVelocity,
        &mut Position,
        &mut Rotation,
    )>,
) {
    for (entity, vehicle, state, velocity, omega, mut position, mut rotation) in &mut cars {
        let Some(os) = state.original.as_ref() else {
            continue;
        };
        let local_com = Vec3::from(vehicle.config.center_of_mass);
        let before_com = os.step_start_position + os.step_start_rotation * local_com;
        // Preserve the final COM if changing only orientation. This isolates
        // orientation rounding from the separate COM accumulation experiment.
        let advanced_com = if mode.com {
            before_com + velocity.0 * time.delta_secs()
        } else {
            position.0 + rotation.0 * local_com
        };
        if mode.rotation {
            let turn = omega.0 * time.delta_secs();
            rotation.0 = if mode.age {
                let basis = history
                    .0
                    .entry(entity)
                    .or_insert_with(|| Mat3::from_quat(os.step_start_rotation));
                let length_squared = f64::from(turn.x) * f64::from(turn.x)
                    + f64::from(turn.y) * f64::from(turn.y)
                    + f64::from(turn.z) * f64::from(turn.z);
                let theta = length_squared.sqrt() as f32;
                if length_squared > 1e-15 {
                    let axis = turn * (1.0 / theta);
                    let cosine = f64::from(theta).cos();
                    let sine = f64::from(f64::from(theta).sin() as f32);
                    let t = f64::from((1.0 - cosine) as f32);
                    let x = f64::from(axis.x);
                    let y = f64::from(axis.y);
                    let z = f64::from(axis.z);
                    let delta = Mat3::from_cols(
                        Vec3::new(
                            (cosine + x * x * t) as f32,
                            (y * x * t + z * sine) as f32,
                            (z * x * t - y * sine) as f32,
                        ),
                        Vec3::new(
                            (x * y * t - z * sine) as f32,
                            (cosine + y * y * t) as f32,
                            (z * y * t + x * sine) as f32,
                        ),
                        Vec3::new(
                            (x * z * t + y * sine) as f32,
                            (y * z * t - x * sine) as f32,
                            (cosine + z * z * t) as f32,
                        ),
                    );
                    let product = |v: Vec3| {
                        let row = |a: f32, b: f32, c: f32| {
                            (f64::from(a) * f64::from(v.x)
                                + f64::from(b) * f64::from(v.y)
                                + f64::from(c) * f64::from(v.z)) as f32
                        };
                        Vec3::new(
                            row(delta.x_axis.x, delta.y_axis.x, delta.z_axis.x),
                            row(delta.x_axis.y, delta.y_axis.y, delta.z_axis.y),
                            row(delta.x_axis.z, delta.y_axis.z, delta.z_axis.z),
                        )
                    };
                    *basis = Mat3::from_cols(
                        product(basis.x_axis),
                        product(basis.y_axis),
                        product(basis.z_axis),
                    );
                }
                // This diagnostic retains raw matrix history but the force
                // adapter still consumes its normalized quaternion projection.
                // It is not a complete port of source matrix arithmetic.
                Quat::from_mat3(basis).normalize()
            } else {
                (Quat::from_scaled_axis(turn) * os.step_start_rotation).normalize()
            };
        }
        position.0 = advanced_com - rotation.0 * local_com;
    }
}
