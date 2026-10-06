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
//! of the narrow phase (~3 ms of the 120 Hz step, in bursts as the
//! timed bridges and the train moved) until they were kept apart.
//!
//! Only those pairs are separated: [`GameLayer::World`] is the static
//! city geometry (ground, room colliders, prop colliders) and
//! [`GameLayer::Scenery`] is the movers, which interact with everything
//! *except* `World` and each other. Vehicles, bangers and traffic stay on the default
//! layer, so the player still collides with the train and the ground,
//! ambient traffic still strikes parked cars, and a mover still shoves a
//! dynamic prop. Spatial queries (wheel rays, the camera) default to
//! every layer, so they still see the world.

use avian3d::prelude::*;

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
}

impl GameLayer {
    /// Layers for a static city-geometry collider: a `World` member that
    /// collides with everything the layers allow.
    pub fn world() -> CollisionLayers {
        CollisionLayers::new(GameLayer::World, LayerMask::ALL)
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
