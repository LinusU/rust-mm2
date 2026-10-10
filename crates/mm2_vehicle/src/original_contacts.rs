//! Source material pairing and delayed native static-world contact response.
use avian3d::collision::contact_types::ContactId;
use avian3d::prelude::*;
use bevy::prelude::*;

use crate::vehicle::RemoteReplica;
use crate::{OriginalContactMaterial, Vehicle, VehicleState};

fn pair_products(
    hull1: Option<OriginalContactMaterial>,
    hull2: Option<OriginalContactMaterial>,
    surface1: Option<OriginalContactMaterial>,
    surface2: Option<OriginalContactMaterial>,
    static1: bool,
    static2: bool,
) -> Option<(f32, f32)> {
    match (hull1, hull2) {
        (Some(a), Some(b)) => a.combined(b),
        (Some(a), None) if static2 => a.combined(surface2?),
        (None, Some(b)) if static1 => surface1?.combined(b),
        _ => None,
    }
}

/// Runs after narrow-phase material combination, before constraint preparation.
/// Unmarked terrain and generic dynamic props retain their existing response.
pub(crate) fn apply_original_contact_materials(
    mut graph: ResMut<ContactGraph>,
    vehicles: Query<&Vehicle>,
    materials: Query<&OriginalContactMaterial>,
    bodies: Query<&RigidBody>,
) {
    for pair in graph.active_pairs_mut() {
        let hull = |body: Option<Entity>, collider: Entity| {
            body.and_then(|e| vehicles.get(e).ok())
                .or_else(|| vehicles.get(collider).ok())
                .and_then(|v| v.config.original.as_ref())
                .map(|o| OriginalContactMaterial {
                    friction: o.bound_friction,
                    elasticity: o.bound_elasticity,
                })
        };
        let material = |body: Option<Entity>, collider: Entity| {
            materials
                .get(collider)
                .ok()
                .copied()
                .or_else(|| body.and_then(|e| materials.get(e).ok().copied()))
        };
        let is_static = |body: Option<Entity>| {
            body.and_then(|e| bodies.get(e).ok())
                .is_some_and(RigidBody::is_static)
        };
        let Some((friction, restitution)) = pair_products(
            hull(pair.body1, pair.collider1),
            hull(pair.body2, pair.collider2),
            material(pair.body1, pair.collider1),
            material(pair.body2, pair.collider2),
            is_static(pair.body1),
            is_static(pair.body2),
        ) else {
            continue;
        };
        for manifold in &mut pair.manifolds {
            manifold.friction = friction;
            manifold.restitution = restitution;
        }
    }
}

/// Points are removed only while Avian prepares/solves constraints. Manifolds,
/// graph handles and pair status remain intact throughout that interval.
#[derive(Resource, Default)]
pub(crate) struct OriginalContactSnapshots {
    points: Vec<(ContactId, usize, Vec<ContactPoint>)>,
}

fn original_static_side(
    pair: &ContactPair,
    vehicles: &Query<&Vehicle, Without<RemoteReplica>>,
    materials: &Query<&OriginalContactMaterial>,
    bodies: &Query<&RigidBody>,
    sensors: &Query<(), With<Sensor>>,
) -> Option<(Entity, bool, OriginalContactMaterial)> {
    if [
        Some(pair.collider1),
        Some(pair.collider2),
        pair.body1,
        pair.body2,
    ]
    .into_iter()
    .flatten()
    .any(|entity| sensors.contains(entity))
    {
        return None;
    }
    let native = |body: Option<Entity>, collider: Entity| {
        let entity = body.unwrap_or(collider);
        let vehicle = vehicles.get(entity).ok()?;
        vehicle.config.original.as_ref()?;
        Some(entity)
    };
    let world = |body: Option<Entity>, collider: Entity| {
        let entity = body.unwrap_or(collider);
        if !bodies.get(entity).ok()?.is_static() {
            return None;
        }
        materials
            .get(collider)
            .ok()
            .copied()
            .or_else(|| materials.get(entity).ok().copied())
    };
    if let (Some(vehicle), Some(material)) = (
        native(pair.body1, pair.collider1),
        world(pair.body2, pair.collider2),
    ) {
        Some((vehicle, true, material))
    } else if let (Some(material), Some(vehicle)) = (
        world(pair.body1, pair.collider1),
        native(pair.body2, pair.collider2),
    ) {
        Some((vehicle, false, material))
    } else {
        None
    }
}

pub(crate) fn suppress_original_static_contacts(
    mut graph: ResMut<ContactGraph>,
    mut snapshots: ResMut<OriginalContactSnapshots>,
    vehicles: Query<&Vehicle, Without<RemoteReplica>>,
    materials: Query<&OriginalContactMaterial>,
    bodies: Query<&RigidBody>,
    sensors: Query<(), With<Sensor>>,
) {
    debug_assert!(
        snapshots.points.is_empty(),
        "contact points must be restored each tick"
    );
    for pair in graph.active_pairs_mut() {
        if original_static_side(pair, &vehicles, &materials, &bodies, &sensors).is_none() {
            continue;
        }
        for (index, manifold) in pair.manifolds.iter_mut().enumerate() {
            snapshots
                .points
                .push((pair.contact_id, index, std::mem::take(&mut manifold.points)));
        }
    }
}

pub(crate) fn restore_original_static_contacts(
    mut graph: ResMut<ContactGraph>,
    mut snapshots: ResMut<OriginalContactSnapshots>,
) {
    for (id, index, points) in snapshots.points.drain(..) {
        if let Some(manifold) = graph
            .active_pairs_mut()
            .iter_mut()
            .find(|pair| pair.contact_id == id)
            .and_then(|pair| pair.manifolds.get_mut(index))
        {
            manifold.points = points;
        }
        // A pair removed during the solver has no graph entry to restore.
    }
}

fn static_contact_impulse(
    mass: f32,
    inverse_inertia: Mat3,
    arm: Vec3,
    velocity: Vec3,
    normal: Vec3,
    friction: f32,
    elasticity: f32,
) -> Vec3 {
    if normal.dot(velocity) > 0.01 {
        return Vec3::ZERO;
    }
    let cross = Mat3::from_cols(arm.cross(Vec3::X), arm.cross(Vec3::Y), arm.cross(Vec3::Z));
    let k = Mat3::IDENTITY / mass + cross * inverse_inertia * cross.transpose();
    let mut impulse = k.inverse() * -velocity;
    let normal_impulse = impulse.dot(normal);
    let tangent = impulse - normal * normal_impulse;
    let tangent_length = tangent.length();
    if tangent_length >= friction * normal_impulse {
        let direction = normal + friction * tangent.try_normalize().unwrap_or(Vec3::ZERO);
        let denominator = normal.dot(k * direction);
        impulse = if denominator.abs() >= 1e-5 {
            direction * (-normal.dot(velocity) / denominator)
        } else {
            Vec3::ZERO
        };
    }
    impulse * (1.0 + elasticity)
}

/// Retail impacts are pending momentum, not velocity changes in this sample.
/// Geometry comes from Avian, but its generic static-world impulses are skipped.
#[allow(clippy::too_many_arguments)] // physics geometry, material and native state are separate ECS queries
pub(crate) fn respond_original_static_contacts(
    spatial: SpatialQuery,
    mut graph: ResMut<ContactGraph>,
    native: Query<&Vehicle, Without<RemoteReplica>>,
    mut states: Query<&mut VehicleState, Without<RemoteReplica>>,
    materials: Query<&OriginalContactMaterial>,
    bodies: Query<&RigidBody>,
    shapes: Query<(&Collider, &Position, &Rotation)>,
    motion: Query<(&Position, &Rotation, &LinearVelocity, &AngularVelocity)>,
    sensors: Query<(), With<Sensor>>,
) {
    let pairs: Vec<_> = graph
        .active_pairs_mut()
        .iter()
        .filter_map(|pair| {
            original_static_side(pair, &native, &materials, &bodies, &sensors)
                .map(|side| (pair.contact_id, pair.collider1, pair.collider2, side))
        })
        .collect();
    for (id, collider1, collider2, (entity, first, world_material)) in pairs {
        let Ok(vehicle) = native.get(entity) else {
            continue;
        };
        let Some(original) = vehicle.config.original.as_ref() else {
            continue;
        };
        let Some((friction, elasticity)) = (OriginalContactMaterial {
            friction: original.bound_friction,
            elasticity: original.bound_elasticity,
        })
        .combined(world_material) else {
            continue;
        };
        let Ok((shape1, position1, rotation1)) = shapes.get(collider1) else {
            continue;
        };
        let Ok((shape2, position2, rotation2)) = shapes.get(collider2) else {
            continue;
        };
        let Ok((position, rotation, linear, angular)) = motion.get(entity) else {
            continue;
        };
        let mut fresh = Vec::new();
        shape1.contact_manifolds(
            shape2,
            position1.0,
            *rotation1,
            position2.0,
            *rotation2,
            0.0,
            &mut fresh,
        );
        let count = fresh
            .iter()
            .flat_map(|m| &m.points)
            .filter(|p| p.penetration >= 0.0)
            .count();
        let Some(pair) = graph
            .active_pairs_mut()
            .iter_mut()
            .find(|p| p.contact_id == id)
        else {
            continue;
        };
        for point in pair.manifolds.iter_mut().flat_map(|m| &mut m.points) {
            point.normal_impulse = 0.0;
            point.normal_speed = 0.0;
            point.warm_start_normal_impulse = 0.0;
            point.warm_start_tangent_impulse = Vec2::ZERO;
        }
        if count == 0 {
            continue;
        }
        let com = position.0 + rotation.0 * Vec3::from(vehicle.config.center_of_mass);
        let rotation_matrix = Mat3::from_quat(rotation.0);
        let inverse_inertia = rotation_matrix
            * Mat3::from_diagonal(
                Vec3::from(vehicle.config.inertia.unwrap_or([vehicle.config.mass; 3])).recip(),
            )
            * rotation_matrix.transpose();
        let Ok(mut state) = states.get_mut(entity) else {
            continue;
        };
        let Some(os) = state.original.as_mut() else {
            continue;
        };
        for manifold in fresh {
            let normal = if first {
                -manifold.normal
            } else {
                manifold.normal
            };
            for point in manifold.points.into_iter().filter(|p| p.penetration >= 0.0) {
                // Retail's measured floor contact is the midpoint between
                // the moved hull point and its swept intersection, rather
                // than a projection straight down at the new pose.
                let hull_point = point.point - normal * (0.5 * point.penetration);
                let local_point = rotation.0.inverse() * (hull_point - position.0);
                let previous_point = os.step_start_position + os.step_start_rotation * local_point;
                let travel = hull_point - previous_point;
                let world_collider = if first { collider2 } else { collider1 };
                let impact_point = Dir3::new(travel)
                    .ok()
                    .and_then(|direction| {
                        spatial.cast_ray(
                            previous_point,
                            direction,
                            travel.length(),
                            true,
                            &SpatialQueryFilter::default().with_excluded_entities([entity]),
                        )
                    })
                    // A solid ray starting inside returns distance zero with no
                    // usable boundary normal; its origin is the swept endpoint.
                    .filter(|hit| {
                        hit.entity == world_collider
                            && (hit.distance <= 1e-6
                                || hit.normal.normalize_or_zero().dot(normal) >= 0.999)
                    })
                    .map_or(point.point, |hit| {
                        (previous_point + travel.normalize() * hit.distance + hull_point) * 0.5
                    });
                let arm = impact_point - com;
                let velocity = linear.0 + angular.0.cross(arm);
                let impulse = static_contact_impulse(
                    vehicle.config.mass,
                    inverse_inertia,
                    arm,
                    velocity,
                    normal,
                    friction,
                    elasticity,
                ) / count as f32;
                os.collision_impulse += impulse;
                os.collision_angular_impulse += arm.cross(impulse);
                let missing = point.penetration - os.push.dot(normal);
                if missing > 0.0 {
                    os.push += normal * missing;
                }
                if let Some((manifold_index, point_index)) = pair
                    .manifolds
                    .iter()
                    .enumerate()
                    .flat_map(|(mi, m)| m.points.iter().enumerate().map(move |(pi, p)| (mi, pi, p)))
                    .min_by(|a, b| {
                        a.2.point
                            .distance_squared(impact_point)
                            .total_cmp(&b.2.point.distance_squared(impact_point))
                    })
                    .map(|(mi, pi, _)| (mi, pi))
                {
                    let old_manifold = &mut pair.manifolds[manifold_index];
                    old_manifold.normal = manifold.normal;
                    let old = &mut old_manifold.points[point_index];
                    old.point = impact_point;
                    old.penetration = point.penetration;
                    old.normal_impulse += impulse.dot(normal).max(0.0);
                    old.normal_speed = old.normal_speed.min(normal.dot(velocity));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_contact_stops_then_bounces_normal_motion() {
        let impulse = static_contact_impulse(
            2.0,
            Mat3::IDENTITY,
            Vec3::ZERO,
            Vec3::new(0.0, -3.0, 0.0),
            Vec3::Y,
            0.9,
            0.25,
        );
        assert!((impulse - Vec3::new(0.0, 7.5, 0.0)).length() < 1e-5);
        assert_eq!(
            static_contact_impulse(2.0, Mat3::IDENTITY, Vec3::ZERO, Vec3::Y, Vec3::Y, 0.9, 0.25),
            Vec3::ZERO
        );
    }

    #[test]
    fn sliding_contact_uses_cone_edge_and_rotational_effective_mass() {
        let impulse = static_contact_impulse(
            2.0,
            Mat3::IDENTITY,
            Vec3::ZERO,
            Vec3::new(10.0, -3.0, 0.0),
            Vec3::Y,
            0.5,
            0.0,
        );
        assert!((impulse - Vec3::new(-3.0, 6.0, 0.0)).length() < 1e-5);
        let off_center = static_contact_impulse(
            2.0,
            Mat3::IDENTITY,
            Vec3::X,
            -Vec3::Y * 3.0,
            Vec3::Y,
            0.0,
            0.0,
        );
        assert!((off_center - Vec3::Y * 2.0).length() < 1e-5);
    }

    #[test]
    fn raw_pairing_is_limited_to_native_hulls_and_marked_static_world() {
        let hull = OriginalContactMaterial {
            friction: 0.9,
            elasticity: 0.5,
        };
        let world = OriginalContactMaterial {
            friction: 1.0,
            elasticity: 0.5,
        };
        assert_eq!(
            pair_products(Some(hull), None, None, Some(world), false, true),
            Some((0.9, 0.25))
        );
        assert_eq!(
            pair_products(None, Some(hull), Some(world), None, true, false),
            Some((0.9, 0.25))
        );
        assert_eq!(
            pair_products(Some(hull), Some(hull), None, None, false, false),
            Some((0.9 * 0.9, 0.25))
        );
        assert_eq!(
            pair_products(Some(hull), None, None, Some(world), false, false),
            None
        );
        assert_eq!(
            pair_products(Some(hull), None, None, None, false, true),
            None
        );
        assert_eq!(
            pair_products(None, None, Some(world), Some(world), true, false),
            None
        );
    }
}
