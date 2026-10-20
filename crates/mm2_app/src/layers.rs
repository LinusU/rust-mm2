//! Physics collision layers.
//!
//! Avian builds a contact pair for every overlapping `(kinematic,
//! static)` pair and then runs full narrow-phase manifold generation on
//! it — for a trimesh against a trimesh, the most expensive query it
//! has. The city's scenery movers (the underground train, ferries,
//! sailboats, drawbridge leaves) are kinematic trimeshes that sweep
//! through the city's static trimeshes all session, and their own
//! pieces overlap one another (coupled train carriages, the two leaves
//! of a bridge). Neither side of such a pair can respond and nothing
//! reads the contact, yet measured on London "Tower Tour" they were most
//! of the narrow phase (~3 ms of the then-120 Hz step, in bursts as the
//! timed bridges and the train moved) until they were kept apart.
//!
//! Only those pairs are separated: [`GameLayer::World`] is the static
//! city geometry (ground, room colliders, prop colliders),
//! [`GameLayer::Scenery`] is the movers, which interact with everything
//! *except* `World` and each other, and [`GameLayer::Pedestrian`] is the
//! walkers, which interact with the default layer (the cars) alone.
//! Vehicles, bangers and traffic stay on the default layer, so the
//! player still collides with the train and the ground, ambient traffic
//! still strikes parked cars, a mover still shoves a dynamic prop, and a
//! car is now blocked by a walker it catches (F19-B.8b). Spatial queries
//! default to every layer, so a query that must not see a walker says so
//! with [`GameLayer::probe_mask`]: wheel rays and the original step's
//! probes (via `VehicleProbeLayers`), the chase camera and the recovery
//! anchor. The few that deliberately see walkers are listed in
//! `docs/research/pedanim.md` "Car contact".

use avian3d::prelude::*;
use bevy::ecs::entity::Entity;

/// The game's collision layers.
#[derive(PhysicsLayer, Clone, Copy, Debug, Default)]
pub enum GameLayer {
    /// Everything not named below — vehicles, bangers, traffic.
    #[default]
    Default,
    /// Static city geometry: ground, room and prop colliders.
    World,
    /// Kinematic scenery movers: trains, ferries, sailboats, drawbridges.
    Scenery,
    /// Kinematic pedestrians (F19-B.8b): each walker's body capsule.
    Pedestrian,
    /// A banger still waiting for its first strike (see
    /// [`GameLayer::dormant_prop`]).
    DormantProp,
    /// Deep-water surfaces (`deepwater`-class drag): the visible plane a
    /// car sinks through rather than drives on. It collides with
    /// nothing and no probe sees it; `water::float_vehicles` carries the
    /// car instead.
    Water,
}

impl GameLayer {
    /// Layers for a static city-geometry collider: a `World` member that
    /// collides with everything the layers allow.
    pub fn world() -> CollisionLayers {
        CollisionLayers::new(GameLayer::World, LayerMask::ALL)
    }

    /// Layers for a deep-water surface collider: a `Water` member that
    /// collides with nothing, so neither a chassis nor a wheel ray is
    /// held up by it (see [`GameLayer::Water`]).
    pub fn water() -> CollisionLayers {
        CollisionLayers::new(GameLayer::Water, LayerMask::NONE)
    }

    /// Layers for a kinematic scenery mover: a `Scenery` member that
    /// collides with everything except the static `World` and other
    /// scenery.
    pub fn scenery() -> CollisionLayers {
        CollisionLayers::new(
            GameLayer::Scenery,
            !LayerMask::from([GameLayer::World, GameLayer::Scenery]),
        )
    }

    /// Layers for a dormant banger: a static sensor waiting for a
    /// striker. It is alone on `DormantProp`, which cars, active props,
    /// movers and walkers (via their own filters) still meet, but it
    /// filters out `World` and `DormantProp`. Avian pairs a static sensor
    /// with the static city trimeshes and with its neighbours (it skips
    /// static-static pairs only for solid colliders), and ~40,000 such
    /// manifolds were most of the headless fixed tick on sf. The only
    /// thing that read a dormant prop's contact with the ground was the
    /// activation gate, which on sf fired twice on a parking meter off a
    /// ~1e-5 m/s "strike" by the static ground (infinite striker mass
    /// makes any approach speed clear the limit). Every transition out
    /// of `Dormant` restores [`CollisionLayers::default`] along with removing the `Sensor`.
    pub fn dormant_prop() -> CollisionLayers {
        CollisionLayers::new(
            GameLayer::DormantProp,
            !LayerMask::from([GameLayer::World, GameLayer::DormantProp]),
        )
    }

    /// Layers for a kinematic pedestrian body: a `Pedestrian` member
    /// that collides with the default layer (vehicles and active props),
    /// the dormant props and nothing else. Static `World` is left out on purpose — a walker
    /// stands on the sidewalk without needing a kinematic-vs-trimesh
    /// pair, and `Scenery` movers do not strike walkers.
    pub fn pedestrian() -> CollisionLayers {
        CollisionLayers::new(
            GameLayer::Pedestrian,
            [GameLayer::Default, GameLayer::DormantProp],
        )
    }

    /// Every layer a car's or camera's ray may hit: all but
    /// `Pedestrian` and `Water`. A walker meets a car through the collision pair
    /// only; a wheel ray, an anchor probe or the chase camera that ended
    /// on a capsule would change how the car drives or where the camera
    /// sits (#1390).
    pub fn probe_mask() -> LayerMask {
        !LayerMask::from([GameLayer::Pedestrian, GameLayer::Water])
    }

    /// A [`SpatialQueryFilter`] over [`GameLayer::probe_mask`] that skips
    /// `excluded`.
    pub fn probe_filter(excluded: impl IntoIterator<Item = Entity>) -> SpatialQueryFilter {
        SpatialQueryFilter::from_mask(Self::probe_mask()).with_excluded_entities(excluded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_water_is_neither_collided_with_nor_probed() {
        let water = GameLayer::water();
        assert!(!water.interacts_with(CollisionLayers::default()));
        assert!(!water.interacts_with(GameLayer::world()));
        assert!(!GameLayer::probe_mask().has_all(LayerMask::from(GameLayer::Water)));
        assert!(GameLayer::probe_mask().has_all(LayerMask::from(GameLayer::World)));
    }

    #[test]
    fn scenery_skips_the_world_and_itself_and_nothing_else() {
        let world = GameLayer::world();
        let scenery = GameLayer::scenery();
        let anything = CollisionLayers::default();
        assert!(
            !scenery.interacts_with(world),
            "the pair that cost the narrow phase"
        );
        assert!(
            !scenery.interacts_with(scenery),
            "carriage against carriage, leaf against leaf"
        );
        assert!(
            world.interacts_with(anything),
            "vehicles still hit the ground"
        );
        assert!(
            scenery.interacts_with(anything),
            "a mover still hits the player"
        );
        assert!(anything.interacts_with(anything));
    }

    #[test]
    fn a_dormant_prop_skips_the_world_and_its_neighbours_only() {
        let dormant = GameLayer::dormant_prop();
        let car = CollisionLayers::default();
        assert!(!dormant.interacts_with(GameLayer::world()));
        assert!(!dormant.interacts_with(dormant), "dormant against dormant");
        assert!(dormant.interacts_with(car), "a striker still meets it");
        assert!(dormant.interacts_with(GameLayer::pedestrian()));
        assert!(dormant.interacts_with(GameLayer::scenery()));
        assert!(
            GameLayer::world().interacts_with(CollisionLayers::default()),
            "a woken prop (default layers) hits the ground again"
        );
    }

    #[test]
    fn a_pedestrian_contacts_the_cars_and_nothing_static() {
        let pedestrian = GameLayer::pedestrian();
        let car = CollisionLayers::default();
        assert!(
            pedestrian.interacts_with(car) && car.interacts_with(pedestrian),
            "a car is blocked by a walker (F19-B.8b)"
        );
        assert!(
            !pedestrian.interacts_with(GameLayer::world()),
            "no kinematic-vs-static-trimesh pair under every walker"
        );
        assert!(
            !pedestrian.interacts_with(GameLayer::scenery()),
            "movers do not strike walkers"
        );
        assert!(
            !pedestrian.interacts_with(pedestrian),
            "walkers do not collide with one another"
        );
    }
}
