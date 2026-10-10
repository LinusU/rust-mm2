//! Deterministic retail-input trajectory on a flat _default road.
//! cargo run -p mm2_app --example handling_trace -- <install> <car> <scenario> [seconds] [friction] [settle_frames]
use avian3d::prelude::*;
use bevy::{prelude::*, time::TimeUpdateStrategy};
use mm2_assets::{InstallMount, Vfs, mount_install};
use mm2_vehicle::player_input::quantize_steering;
use mm2_vehicle::{
    HumanDriver, OriginalContactMaterial, TireSurface, VehicleInput, VehiclePlugin, VehicleState,
    vehicle_bundle,
};
use std::{
    io::{self, Write},
    time::Duration,
};

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let dir = args.get(1).expect("installation path");
    let id = args.get(2).map(String::as_str).unwrap_or("vpbug");
    let scenario = args.get(3).map(String::as_str).unwrap_or("launch");
    let seconds: f32 = args
        .get(4)
        .map(|x| x.parse().expect("seconds"))
        .unwrap_or(15.0);
    let surface_friction: f32 = args
        .get(5)
        .map(|x| x.parse().expect("surface friction"))
        .unwrap_or(0.9);
    assert!(
        surface_friction.is_finite() && surface_friction >= 0.0,
        "surface friction must be finite and nonnegative"
    );
    let settle_frames: usize = args
        .get(6)
        .map(|x| x.parse().expect("settlement frames"))
        .unwrap_or(600);
    let mut vfs = Vfs::new();
    mount_install(&mut vfs, dir.as_ref(), &InstallMount::default()).unwrap();
    let cfg = mm2_content::load_vehicle(&vfs, id, 0).unwrap().config;
    if scenario == "fingerprint" {
        let original = cfg.original.as_ref().expect("retail handling");
        let [ix, iy, iz] = cfg.inertia.expect("retail inertia tensor");
        let inertia_box =
            [iy + iz - ix, ix + iz - iy, ix + iy - iz].map(|sum| (6.0 * sum / cfg.mass).sqrt());
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "car": id,
                "live_tuning": {
                    "mass": cfg.mass,
                    "inertiaBox": inertia_box,
                    "horsepower": original.engine.max_power_w / 746.0,
                    "idleRpm": original.engine.idle_rpm,
                    "optimalRpm": original.engine.opt_rpm,
                    "maxRpm": original.engine.max_rpm,
                    "frontRadius": cfg.wheels.iter().zip(&original.wheels)
                        .find(|(_, wheel)| !wheel.rear).unwrap().0.radius,
                    "rearRadius": cfg.wheels.iter().zip(&original.wheels)
                        .find(|(_, wheel)| wheel.rear).unwrap().0.radius,
                },
                "diagnostics": {
                    "center_of_mass": cfg.center_of_mass,
                    "inertia": cfg.inertia,
                    "wheels": cfg.wheels,
                    "original": original,
                    "gyro": cfg.gyro,
                    "collider_points": cfg.collider_points,
                    "striker_points": cfg.striker_points,
                },
            }))
            .unwrap()
        );
        return;
    }
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::asset::AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(TransformPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 19.6))
        .add_plugins(VehiclePlugin);
    app.finish();
    app.cleanup();
    app.world_mut().spawn((
        RigidBody::Static,
        TireSurface {
            grip: surface_friction / cfg.original.as_ref().map_or(0.9, |o| o.surface_friction),
            drag: 0.0,
        },
        Collider::half_space(Vec3::Y),
        Position(Vec3::ZERO),
        Transform::IDENTITY,
        OriginalContactMaterial {
            friction: surface_friction,
            elasticity: 0.5,
        },
    ));
    let car = app
        .world_mut()
        .spawn((
            vehicle_bundle(&cfg),
            HumanDriver::default(),
            Position(Vec3::new(1200.0, 1.0, 1200.0)),
            Transform::from_xyz(1200.0, 1.0, 1200.0),
        ))
        .id();
    if std::env::var_os("MM2_TRACE_DISABLE_BODY_CONTACT").is_some() {
        app.world_mut().entity_mut(car).insert(Sensor);
    }
    // The first Bevy update initializes its clock without a fixed step.
    // Prime it so the requested settlement count is actual physics ticks.
    app.update();
    for _ in 0..settle_frames {
        app.update();
    }
    let com_offset = Vec3::from(cfg.center_of_mass);
    let start = app.world().get::<Position>(car).unwrap().0
        + app.world().get::<Rotation>(car).unwrap().0 * com_offset;
    let ramp = cfg.player_steering.unwrap_or_default();
    let mut steering = 0.0;
    let mut out = io::BufWriter::new(io::stdout().lock());
    let log_contacts = std::env::var_os("MM2_TRACE_CONTACTS").is_some();
    write!(
        out,
        "frame,time,x,y,z,vx,vy,vz,yaw,yaw_rate,speed,rpm,gear,steer,grounded,original_gear,shaft_spin,throttle,brake,handbrake,up_x,up_y,up_z,right_x,right_y,right_z,omega_x,omega_z,last_push_x,last_push_y,last_push_z"
    )
    .unwrap();
    for i in 0..4 {
        write!(out, ",wheel{i}_grounded,wheel{i}_x,wheel{i}_load,wheel{i}_f_lat,wheel{i}_f_long,wheel{i}_bristle_lat,wheel{i}_bristle_long,wheel{i}_omega").unwrap();
    }
    write!(out, ",force_x,force_y,force_z,torque_x,torque_y,torque_z").unwrap();
    write!(out, ",impulse_x,impulse_y,impulse_z,angular_impulse_x,angular_impulse_y,angular_impulse_z,applied_push_x,applied_push_y,applied_push_z").unwrap();
    writeln!(out).unwrap();
    for frame in 0..=(seconds * 60.0) as usize {
        if log_contacts {
            let graph = app.world().resource::<ContactGraph>();
            for pair in graph.active_pairs() {
                if pair.is_touching() {
                    eprintln!("CONTACT {frame} {pair:?}");
                }
            }
        }
        let t = frame as f32 / 60.0;
        let (throttle, brake, target, handbrake) = match scenario {
            "launch" => (1.0, 0.0, 0.0, 0.0),
            "coast" => (if t < 5.0 { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0),
            "brake" => (
                if t < 5.0 { 1.0 } else { 0.0 },
                if t >= 5.0 { 1.0 } else { 0.0 },
                0.0,
                0.0,
            ),
            "turn" => (
                1.0,
                0.0,
                if (5.0..7.0).contains(&t) { 1.0 } else { 0.0 },
                0.0,
            ),
            "handbrake" => (
                if t < 5.0 { 1.0 } else { 0.0 },
                0.0,
                if (5.0..7.0).contains(&t) { 1.0 } else { 0.0 },
                if (5.0..7.0).contains(&t) { 1.0 } else { 0.0 },
            ),
            "powerslide" => (
                1.0,
                0.0,
                if (300..375).contains(&frame) {
                    1.0
                } else if (375..435).contains(&frame) {
                    -1.0
                } else {
                    0.0
                },
                if (330..348).contains(&frame) {
                    1.0
                } else {
                    0.0
                },
            ),
            "slalom" => (
                1.0,
                0.0,
                match frame {
                    300..360 | 420..480 => 1.0,
                    360..420 | 480..540 => -1.0,
                    _ => 0.0,
                },
                0.0,
            ),
            "lift_turn" => (
                if (330..390).contains(&frame) {
                    0.0
                } else {
                    1.0
                },
                0.0,
                if (300..450).contains(&frame) {
                    1.0
                } else {
                    0.0
                },
                0.0,
            ),
            "brake_turn" => (
                if (330..390).contains(&frame) {
                    0.0
                } else {
                    1.0
                },
                if (330..390).contains(&frame) {
                    1.0
                } else {
                    0.0
                },
                match frame {
                    300..420 => 1.0,
                    420..480 => -1.0,
                    _ => 0.0,
                },
                0.0,
            ),
            _ => panic!("unknown scenario {scenario}"),
        };
        let speed = app
            .world()
            .get::<VehicleState>(car)
            .unwrap()
            .human_steering_speed();
        let (next, filtered) = ramp.discrete_step(steering, target, speed, 1.0 / 60.0);
        steering = next;
        *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = VehicleInput {
            throttle,
            brake,
            steering: quantize_steering(filtered),
            handbrake,
            ..default()
        };
        app.update();
        let world = app.world();
        let p = world.get::<Position>(car).unwrap().0
            + world.get::<Rotation>(car).unwrap().0 * com_offset
            - start;
        let v = world.get::<LinearVelocity>(car).unwrap().0;
        let r = world.get::<Rotation>(car).unwrap().0;
        let f = r * Vec3::NEG_Z;
        let up = r * Vec3::Y;
        let right = r * Vec3::X;
        let w = world.get::<AngularVelocity>(car).unwrap().0;
        let st = world.get::<VehicleState>(car).unwrap();
        let original_state = st.original.as_ref().unwrap();
        write!(out, "{frame},{t:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.3},{},{:.6},{},{},{:.6},{throttle},{brake},{handbrake},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}",
            p.x,p.y,p.z,v.x,v.y,v.z,(-f.x).atan2(-f.z),w.y,st.forward_speed,st.rpm,st.gear,
            quantize_steering(filtered),st.wheels.iter().filter(|x| x.grounded).count(),
            st.original.as_ref().map_or(0, |s| s.powertrain.gear),
            st.original.as_ref().map_or(0.0, |s| s.drive_omega),
            up.x,up.y,up.z,right.x,right.y,right.z,w.x,w.z,
            original_state.last_push.x,original_state.last_push.y,original_state.last_push.z).unwrap();
        for (wheel, original_wheel) in st.wheels.iter().zip(&original_state.wheels) {
            write!(
                out,
                ",{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}",
                u8::from(wheel.grounded),
                original_wheel.x,
                wheel.suspension_force,
                wheel.lateral_force,
                wheel.longitudinal_force,
                original_wheel.bristles.lat,
                original_wheel.bristles.long,
                original_wheel.omega
            )
            .unwrap();
        }
        let force = original_state.pending_force;
        let torque = original_state.pending_torque;
        write!(
            out,
            ",{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}",
            force.x, force.y, force.z, torque.x, torque.y, torque.z
        )
        .unwrap();
        let impulse = original_state.collision_impulse;
        let angular_impulse = original_state.collision_angular_impulse;
        let applied_push = original_state.applied_push;
        write!(
            out,
            ",{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}",
            impulse.x,
            impulse.y,
            impulse.z,
            angular_impulse.x,
            angular_impulse.y,
            angular_impulse.z,
            applied_push.x,
            applied_push.y,
            applied_push.z
        )
        .unwrap();
        writeln!(out).unwrap();
    }
}
