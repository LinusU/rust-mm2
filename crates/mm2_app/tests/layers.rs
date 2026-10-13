//! Collision layers against a real Avian world: kinematic scenery keeps
//! out of the static city geometry and out of other scenery — the pairs
//! that cost the narrow phase — and still meets everything else.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::layers::GameLayer;

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
        .insert_resource(Gravity(Vec3::ZERO))
        .add_plugins(TransformPlugin);
    app.finish();
    app.cleanup();
    app
}

fn body(
    app: &mut App,
    kind: RigidBody,
    size: Vec3,
    at: Vec3,
    layers: Option<CollisionLayers>,
) -> Entity {
    let mut e = app.world_mut().spawn((
        kind,
        Collider::cuboid(size.x, size.y, size.z),
        Position(at),
        Transform::from_translation(at),
    ));
    if let Some(layers) = layers {
        e.insert(layers);
    }
    e.id()
}

fn paired(app: &mut App, a: Entity, b: Entity) -> bool {
    app.world_mut()
        .run_system_once(move |c: Collisions| c.contains(a, b))
        .unwrap()
}

#[test]
fn scenery_skips_the_world_and_other_scenery_but_meets_vehicles() {
    let mut app = physics_app();
    let ground = body(
        &mut app,
        RigidBody::Static,
        Vec3::new(10.0, 1.0, 10.0),
        Vec3::ZERO,
        Some(GameLayer::world()),
    );
    // Two overlapping movers, both sunk into the ground — a train
    // carriage against its coupled neighbour, a bridge leaf against its
    // partner.
    let mover_a = body(
        &mut app,
        RigidBody::Kinematic,
        Vec3::splat(2.0),
        Vec3::new(0.0, 0.5, 0.0),
        Some(GameLayer::scenery()),
    );
    let mover_b = body(
        &mut app,
        RigidBody::Kinematic,
        Vec3::splat(2.0),
        Vec3::new(1.0, 0.5, 0.0),
        Some(GameLayer::scenery()),
    );
    // A default-layer vehicle overlapping everything.
    let vehicle = body(
        &mut app,
        RigidBody::Dynamic,
        Vec3::splat(1.0),
        Vec3::new(0.5, 0.5, 0.0),
        None,
    );
    // The control: a kinematic body on the default layer sunk into the
    // ground pairs with it — what the movers did before the layers, and
    // what ambient traffic still does with parked cars.
    let control = body(
        &mut app,
        RigidBody::Kinematic,
        Vec3::splat(2.0),
        Vec3::new(0.0, 0.5, 4.0),
        None,
    );

    for _ in 0..4 {
        app.update();
    }

    assert!(
        paired(&mut app, control, ground),
        "the control must pair, or the test proves nothing"
    );
    assert!(
        !paired(&mut app, mover_a, ground),
        "scenery paired with the world"
    );
    assert!(
        !paired(&mut app, mover_b, ground),
        "scenery paired with the world"
    );
    assert!(
        !paired(&mut app, mover_a, mover_b),
        "scenery paired with scenery"
    );
    assert!(
        paired(&mut app, vehicle, mover_a),
        "a vehicle must still meet scenery"
    );
    assert!(
        paired(&mut app, vehicle, ground),
        "a vehicle must still meet the world"
    );
}

/// F19-B.8b: a walker's body capsule rides the `Pedestrian` layer, so it
/// pairs with a vehicle (the car that catches it) and with nothing
/// static, moving or another walker. The unit test in
/// `mm2_app::layers` checks the bitmasks; this checks Avian actually
/// builds the pairs.
#[test]
fn a_pedestrian_body_meets_vehicles_but_not_the_world_movers_or_walkers() {
    let mut app = physics_app();
    let ground = body(
        &mut app,
        RigidBody::Static,
        Vec3::new(10.0, 1.0, 10.0),
        Vec3::ZERO,
        Some(GameLayer::world()),
    );
    // A mover the walker overlaps, so only the layer keeps the pair out.
    let mover = body(
        &mut app,
        RigidBody::Kinematic,
        Vec3::splat(2.0),
        Vec3::new(0.0, 0.5, 0.0),
        Some(GameLayer::scenery()),
    );
    // Two overlapping walker bodies on the pedestrian layer, both sunk
    // into the ground and the mover.
    let walker = |app: &mut App, at: Vec3| {
        app.world_mut()
            .spawn((
                RigidBody::Kinematic,
                Collider::capsule_endpoints(
                    0.25,
                    Vec3::new(0.0, -0.5, 0.0),
                    Vec3::new(0.0, 0.5, 0.0),
                ),
                GameLayer::pedestrian(),
                Position(at),
                Transform::from_translation(at),
            ))
            .id()
    };
    let walker_a = walker(&mut app, Vec3::new(0.0, 0.5, 0.0));
    let walker_b = walker(&mut app, Vec3::new(0.3, 0.5, 0.0));
    // A default-layer vehicle overlapping it — the car that catches the
    // walker.
    let vehicle = body(
        &mut app,
        RigidBody::Dynamic,
        Vec3::splat(1.0),
        Vec3::new(0.0, 0.5, 0.0),
        None,
    );

    for _ in 0..4 {
        app.update();
    }

    assert!(
        paired(&mut app, walker_a, vehicle),
        "a car must be blocked by a walker (F19-B.8b)"
    );
    assert!(
        !paired(&mut app, walker_a, ground),
        "no kinematic walker-vs-static-world pair"
    );
    assert!(
        !paired(&mut app, walker_a, mover),
        "a mover must not strike a walker"
    );
    assert!(
        !paired(&mut app, walker_a, walker_b),
        "walkers must not collide with one another"
    );
}

/// A walker's capsule standing on the eye→car ray, and the world behind it.
fn ray_scene() -> (App, Entity, Entity, Entity) {
    let mut app = physics_app();
    let wall = body(
        &mut app,
        RigidBody::Static,
        Vec3::new(10.0, 10.0, 1.0),
        Vec3::new(0.0, 0.0, -10.0),
        Some(GameLayer::world()),
    );
    let walker = app
        .world_mut()
        .spawn((
            RigidBody::Kinematic,
            Collider::capsule_endpoints(0.25, Vec3::new(0.0, -0.5, 0.0), Vec3::new(0.0, 0.5, 0.0)),
            GameLayer::pedestrian(),
            Position(Vec3::new(0.0, 0.0, -5.0)),
            Transform::from_translation(Vec3::new(0.0, 0.0, -5.0)),
        ))
        .id();
    let car = body(
        &mut app,
        RigidBody::Dynamic,
        Vec3::splat(1.0),
        Vec3::new(0.0, 0.0, 5.0),
        None,
    );
    for _ in 0..4 {
        app.update();
    }
    (app, wall, walker, car)
}

/// #1390: the chase camera and the recovery probes build their filter
/// with `GameLayer::probe_filter`, so a walker crossing the eye→car ray
/// neither pulls the camera in nor holds a car up, while the world and
/// the car itself still are seen.
#[test]
fn probe_filter_ignores_a_walker_on_the_ray_but_sees_the_world() {
    let (mut app, wall, walker, car) = ray_scene();
    let cast = move |app: &mut App, filter: SpatialQueryFilter| {
        app.world_mut()
            .run_system_once(move |q: SpatialQuery| {
                q.cast_ray(Vec3::ZERO, Dir3::NEG_Z, 20.0, true, &filter)
                    .map(|h| h.entity)
            })
            .unwrap()
    };
    assert_eq!(
        cast(&mut app, SpatialQueryFilter::default()),
        Some(walker),
        "control: the default filter sees the capsule"
    );
    assert_eq!(
        cast(&mut app, GameLayer::probe_filter([car])),
        Some(wall),
        "the probe filter skips the walker and lands on the world"
    );
}

/// #1390: `VehicleProbeLayers` (what the app installs) keeps the wheel
/// and original-step rays off walkers; its default sees every layer.
#[test]
fn vehicle_probe_layers_skip_a_walker_and_the_car_itself() {
    use mm2_vehicle::VehicleProbeLayers;
    let (mut app, wall, walker, car) = ray_scene();
    let cast = move |app: &mut App, layers: VehicleProbeLayers| {
        app.world_mut()
            .run_system_once(move |q: SpatialQuery| {
                q.cast_ray(
                    Vec3::new(0.0, 0.0, 8.0),
                    Dir3::NEG_Z,
                    30.0,
                    true,
                    &layers.filter(car),
                )
                .map(|h| h.entity)
            })
            .unwrap()
    };
    assert_eq!(cast(&mut app, VehicleProbeLayers::default()), Some(walker));
    assert_eq!(
        cast(&mut app, VehicleProbeLayers(GameLayer::probe_mask())),
        Some(wall)
    );
}
