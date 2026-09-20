//! Game-domain state for the MM2-inspired engine.
//!
//! Shared contracts that neither rendering nor app bootstrap should own:
//! the world mode being played, the typed [`SessionConfig`] a session is
//! started from, the [`Session`] lifecycle/ownership state machine, and
//! the marker components gameplay and app code query by. This is not a
//! framework — gameplay systems grow here over time.

use bevy::prelude::*;
use mm2_assets::Vfs;

pub mod config;
pub mod session;

pub use config::{
    CameraPose, ConfigError, Densities, DevOverrides, Difficulty, EventRef, EventTableKind,
    SelectorError, SessionAuthority, SessionConditions, SessionConfig, SessionMode, TimeOfDay,
    VehicleSelection, Weather,
};
pub use session::{
    Session, SessionEntity, SessionError, SessionPhase, advance_session_tick,
    despawn_session_entities,
};

/// What the app should do at startup.
#[derive(Debug, Clone, Default, PartialEq)]
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
