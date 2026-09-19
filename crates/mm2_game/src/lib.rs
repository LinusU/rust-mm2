//! Game-domain state for the MM2-inspired engine.
//!
//! Kept deliberately small for the vertical slice: the world mode being
//! played and which city is loaded. Cities, traffic, pedestrians and races
//! will grow here over time — this is not a framework, just the shared
//! domain state that neither rendering nor app bootstrap should own.

use bevy::prelude::*;
use mm2_assets::Vfs;

/// What the app should do at startup.
#[derive(Debug, Clone, Default)]
pub enum WorldMode {
    /// Synthetic development playground — no MM2 data required.
    #[default]
    DevWorld,
    /// Load MM2 city content through the VFS.
    City {
        /// Logical path of the PSDL, e.g. `city/london.psdl`.
        psdl: String,
    },
}

/// Resource holding the resolved world mode.
#[derive(Resource, Debug, Clone, Default)]
pub struct ActiveWorld(pub WorldMode);

/// Resource wrapping the mounted virtual filesystem, when an MM2
/// installation (or any content) has been provided.
#[derive(Resource)]
pub struct Mm2Vfs(pub Vfs);

/// Marker component for entities belonging to loaded city content, so it can
/// be torn down or queried independently of the development world.
#[derive(Component)]
pub struct CityEntity;

/// Marker for the player-controlled vehicle.
#[derive(Component)]
pub struct PlayerVehicle;
