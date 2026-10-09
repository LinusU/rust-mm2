//! What Avian's `SpatialQuery` does with a non-finite ray, pinned so the
//! producers that feed it (scripted re-anchor, camera, audio, banger)
//! are guarded on evidence rather than on a guess
//! (`docs/research/authored-numbers.md`, S3).

use std::time::Duration;

use avian3d::prelude::*;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

fn ground_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .add_plugins(TransformPlugin)
        .init_resource::<Assets<Mesh>>();
    app.finish();
    app.cleanup();
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(100.0, 1.0, 100.0),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    for _ in 0..3 {
        app.update();
    }
    app
}

fn cast(app: &mut App, origin: Vec3, len: f32) -> bool {
    app.world_mut()
        .run_system_once(move |spatial: SpatialQuery| {
            spatial
                .cast_ray(origin, Dir3::NEG_Y, len, true, &default())
                .is_some()
        })
        .expect("raycast system runs")
}

#[test]
fn a_finite_ray_hits_the_ground() {
    let mut app = ground_app();
    assert!(cast(&mut app, Vec3::new(0.0, 5.0, 0.0), 20.0));
}

/// Pinned so an Avian/obvhs upgrade that starts tolerating a
/// non-finite origin is noticed, and the producer guards
/// (`seat_on_static_ground`, `SupportProbe::holds_up`, the pedestrian
/// line-up, the traffic drape, the opponent wall feelers) can be
/// revisited. Until then every `cast_ray` origin must be checked
/// finite by its producer.
#[test]
#[should_panic(expected = "origin.is_finite()")]
fn avian_asserts_a_finite_ray_origin() {
    let mut app = ground_app();
    cast(&mut app, Vec3::new(f32::NAN, 5.0, 0.0), 20.0);
}

#[test]
#[should_panic(expected = "origin.is_finite()")]
fn avian_asserts_an_infinite_ray_origin() {
    let mut app = ground_app();
    cast(&mut app, Vec3::new(0.0, 5.0, f32::INFINITY), 20.0);
}

/// The banger bound-strike scan (`activate_bangers`) is the one
/// spatial-query site that is *not* a ray: it calls
/// `SpatialQuery::shape_intersections`, which builds an AABB and
/// traverses — no `Ray::new`, no finite-origin assert. A non-finite
/// striker pose therefore yields an empty hit list rather than a
/// panic, so no producer guard is needed there. Pinned so a future
/// Avian upgrade that routes the query through a ray is noticed.
#[test]
fn avian_shape_intersections_tolerate_a_non_finite_pose() {
    let mut app = ground_app();
    let hits = app
        .world_mut()
        .run_system_once(|spatial: SpatialQuery| {
            spatial.shape_intersections(
                &Collider::cuboid(2.0, 1.0, 4.0),
                Vec3::new(f32::NAN, 1.0, 0.0),
                Quat::IDENTITY,
                &SpatialQueryFilter::default(),
            )
        })
        .expect("intersection-query system runs");
    assert!(hits.is_empty(), "a NaN pose must not select any collider");
}
