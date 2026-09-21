//! Game-domain state for the MM2-inspired engine.
//!
//! Shared contracts that neither rendering nor app bootstrap should own:
//! the world mode being played, the typed [`SessionConfig`] a session is
//! started from, the [`Session`] lifecycle/ownership state machine, and
//! the marker components gameplay and app code query by. This is not a
//! framework — gameplay systems grow here over time.

use bevy::prelude::*;
use mm2_assets::Vfs;

pub mod banger;
pub mod config;
pub mod ids;
pub mod impact;
pub mod nav;
pub mod opponent;
pub mod props;
pub mod race;
pub mod result;
pub mod session;
pub mod surface;
pub mod telemetry;

pub use banger::{
    Banger, BangerCause, BangerDefinition, BangerPhase, BangerPool, BangerStateChanged,
    DEFAULT_ACTIVE_POOL,
};
pub use config::{
    CameraPose, ConfigError, Densities, DevOverrides, Difficulty, EventRef, EventTableKind,
    NavOverlay, SelectorError, SessionAuthority, SessionConditions, SessionConfig, SessionMode,
    SpawnPose, TimeOfDay, VehicleSelection, Weather,
};
pub use ids::{AuthorityRole, ObjectId, ObjectIdentity, Player, PlayerControl, PlayerId};
pub use impact::{ImpactDedup, ImpactEvent, ImpactId, ImpactPolicy};
pub use nav::{
    ArcEnd, ArcExit, ArcId, LaneHit, LaneId, LaneKind, LaneQuery, LaneSample, NavArc, NavBuild,
    NavGraph, NavIntersection, NavIssue, NavLane, NavOverrides, NavRng, NavRoad, NavStats, Route,
    RouteCursor, RouteError, RouteOptions, TravelDir, TurnKind,
};
pub use opponent::{
    OpponentIssue, OpponentRoster, OpponentRoute, OpponentRoutePoint, OpponentSpec,
};
pub use props::{MAX_PROP_RULE_STAMPS, PropStamp, PropWalk, PropWalkStats, walk_prop_rules};
pub use race::{
    Checkpoint, CheckpointRule, DEFAULT_CHECKPOINT_HEIGHT, DEFAULT_COUNTDOWN_TICKS, EventParams,
    NavTarget, ParticipantState, ProgressOutcome, RACE_TICK_HZ, RaceDefinition, RaceError,
    RacePhase, RaceProgress, RaceStart, RaceStarted, RaceState, TargetSelection, cycle_target,
    live_order, navigation_target, relative_bearing,
};
pub use result::{DuplicateResult, ResultId, ResultLedger, SessionOutcome, SessionResult};
pub use session::{
    Session, SessionEntity, SessionError, SessionPhase, advance_session_tick,
    despawn_session_entities,
};
pub use surface::{SurfaceMaterial, SurfaceState};
pub use telemetry::{DamageSignals, VehicleTelemetry, WheelTelemetry};

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
