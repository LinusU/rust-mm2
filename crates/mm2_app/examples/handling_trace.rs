//! Deterministic retail-input trajectory on a flat _default road.
//! cargo run -p mm2_app --example handling_trace -- <install> <car> <scenario> [seconds] [friction] [settle_frames]
use avian3d::prelude::*;
use bevy::{prelude::*, time::TimeUpdateStrategy};
use mm2_assets::{InstallMount, Vfs, mount_install};
use mm2_vehicle::player_input::quantize_steering;
use mm2_vehicle::{
    HumanDriver, TireSurface, VehicleInput, VehiclePlugin, VehicleState, vehicle_bundle,
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
        Collider::cuboid(40000.0, 1.0, 40000.0),
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    let car = app
        .world_mut()
        .spawn((
            vehicle_bundle(&cfg),
            HumanDriver::default(),
            Position(Vec3::new(0.0, 1.5, 0.0)),
            Transform::from_xyz(0.0, 1.5, 0.0),
        ))
        .id();
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
    writeln!(
        out,
        "frame,time,x,y,z,vx,vy,vz,yaw,yaw_rate,speed,rpm,gear,steer,grounded,original_gear,shaft_spin,throttle,brake,handbrake"
    )
    .unwrap();
    for frame in 0..=(seconds * 60.0) as usize {
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
        let w = world.get::<AngularVelocity>(car).unwrap().0;
        let st = world.get::<VehicleState>(car).unwrap();
        writeln!(out, "{frame},{t:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.3},{},{:.6},{},{},{:.6},{throttle},{brake},{handbrake}",
            p.x,p.y,p.z,v.x,v.y,v.z,(-f.x).atan2(-f.z),w.y,st.forward_speed,st.rpm,st.gear,
            quantize_steering(filtered),st.wheels.iter().filter(|x| x.grounded).count(),
            st.original.as_ref().map_or(0, |s| s.powertrain.gear),
            st.original.as_ref().map_or(0.0, |s| s.drive_omega)).unwrap();
    }
}
