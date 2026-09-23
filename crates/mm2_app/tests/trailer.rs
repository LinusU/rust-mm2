//! Trailer hitch integration: `spawn_trailer` joins the trailer hull to
//! the towing car with a spherical joint whose anchors coincide at
//! spawn, so the two colliders overlap at the hitch. The joint carries
//! `JointCollisionDisabled` — without it the pair registers a standing
//! contact that fights the constraint (the vpsemi clearance finding).

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::car_visual;
use mm2_assets::Vfs;
use mm2_content::{TrailerDef, VehicleModel};
use mm2_game::SessionEntity;
use mm2_vehicle::{VehicleConfig, VehiclePlugin, vehicle_bundle};

fn physics_app() -> App {
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

/// Trailer whose hull overlaps the car's at the hitch anchors, the way
/// retail rigs sit.
fn trailer_def() -> TrailerDef {
    let config = VehicleConfig {
        trailer: true,
        ..VehicleConfig::default()
    };
    TrailerDef {
        config,
        model: VehicleModel::default(),
        car_hitch: [0.0, 0.0, 1.9],
        trailer_hitch: [0.0, 0.0, -2.4],
        wheels: Vec::new(),
    }
}

#[test]
fn the_hitch_joint_disables_car_trailer_contacts() {
    let mut app = physics_app();
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(80.0, 1.0, 80.0),
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    let car_pos = Vec3::new(0.0, 1.2, 0.0);
    let car = app
        .world_mut()
        .spawn((
            vehicle_bundle(&VehicleConfig::default()),
            CollidingEntities::default(),
            Position(car_pos),
            Transform::from_translation(car_pos),
            Rotation(Quat::IDENTITY),
        ))
        .id();

    let def = trailer_def();
    let trailer = {
        let world = app.world_mut();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let ent = {
            let mut commands = Commands::new(&mut queue, world);
            let (mut meshes, mut images, mut materials) = (
                Assets::<Mesh>::default(),
                Assets::<Image>::default(),
                Assets::<StandardMaterial>::default(),
            );
            let car_tf = *world.get::<Transform>(car).unwrap();
            let (e, _missing) = car_visual::spawn_trailer(
                &mut commands,
                &Vfs::new(),
                &def,
                0,
                &mut meshes,
                &mut images,
                &mut materials,
                car,
                car_tf,
                SessionEntity(1),
            );
            commands.entity(e).insert(CollidingEntities::default());
            e
        };
        queue.apply(world);
        ent
    };

    // The joint entity itself carries the marker.
    let mut joints = app.world_mut().query::<&JointCollisionDisabled>();
    assert_eq!(
        joints.iter(app.world()).count(),
        1,
        "exactly one collision-disabled joint"
    );

    for _ in 0..180 {
        app.update();
    }

    // Overlapping hitched hulls must not register a standing contact.
    let world = app.world();
    assert!(
        !world
            .get::<CollidingEntities>(car)
            .unwrap()
            .contains(&trailer),
        "car still registers a contact with its trailer"
    );
    assert!(
        !world
            .get::<CollidingEntities>(trailer)
            .unwrap()
            .contains(&car),
        "trailer still registers a contact with its car"
    );

    // …while the hitch still holds the trailer behind the car.
    let car_pos = world.get::<Position>(car).unwrap().0;
    let trailer_pos = world.get::<Position>(trailer).unwrap().0;
    assert!(
        (trailer_pos - car_pos).length() < 8.0,
        "trailer drifted off the hitch: {trailer_pos} vs {car_pos}"
    );
}
