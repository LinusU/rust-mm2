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
pub mod breakaway;
pub mod config;
pub mod damage;
pub mod effects;
pub mod ids;
pub mod impact;
pub mod nav;
pub mod opponent;
pub mod profile;
pub mod progression;
pub mod props;
pub mod race;
pub mod recovery;
pub mod result;
pub mod session;
pub mod stuck;
pub mod surface;
pub mod telemetry;
pub mod traffic;

pub use banger::{
    Banger, BangerCause, BangerDefinition, BangerPhase, BangerPool, BangerStateChanged,
    DEFAULT_ACTIVE_POOL,
};
pub use breakaway::{BreakPartSpec, BreakPartState, PartDetached, VehicleBreaks};
pub use config::{
    CameraPose, ConfigError, Densities, DevOverrides, Difficulty, EventRef, EventTableKind,
    NavOverlay, SelectorError, SessionAuthority, SessionConditions, SessionConfig,
    SessionCustomization, SessionMode, SpawnPose, TimeOfDay, VehicleSelection, Weather,
};
pub use damage::{
    DISABLED_PENALTY_TICKS, DamageEvent, DamageSpec, DamageState, DamageTier, DamageVerdict,
    DisabledOutcome, ImpairmentPolicy, VehicleDamage, disabled_outcome,
};
pub use effects::{
    ParticleSpec, SmokeEmitter, SmokePolicy, SmokePuff, Spark, SparkPolicy, VehicleSmoke,
    VehicleSparks,
};
pub use ids::{AuthorityRole, ObjectId, ObjectIdentity, Player, PlayerControl, PlayerId};
pub use impact::{ImpactDedup, ImpactEvent, ImpactId, ImpactPolicy};
pub use nav::{
    ArcEnd, ArcExit, ArcId, CrossingPath, EndSignal, LaneHit, LaneId, LaneKind, LaneQuery,
    LaneSample, NavArc, NavBuild, NavGraph, NavIntersection, NavIssue, NavLane, NavOverrides,
    NavRng, NavRoad, NavSignal, NavStats, Route, RouteCursor, RouteError, RouteOptions, TravelDir,
    TurnKind,
};
pub use opponent::{
    OpponentDriveParams, OpponentIssue, OpponentRoster, OpponentRoute, OpponentRoutePoint,
    OpponentSpec,
};
pub use profile::{
    EventKey, EventRecord, MAX_NAME_CHARS, PROFILE_SCHEMA_VERSION, PlayerProfile, ProfileError,
    ProfileId, ProfileKind, ProfileLoad, ProfileMeta, ProfileProgress, ProfileSelections,
    ProfileStore, ProfileSummary, VehicleChoice,
};
pub use progression::{
    ApplyOutcome, AvailabilityRow, AvailabilityTable, EventAvailability, EventGate, GarageRow,
    GarageTable, Grant, Ineligible, PaintGate, RewardRequirement, RewardRule, RewardTable, Unlock,
    VehicleAvailability, VehicleGate, apply_result, place_requirement, record_eligibility,
};
pub use props::{
    Carriageway, MAX_PATHSET_STAMPS, MAX_PROP_RULE_STAMPS, PathStampSite, PathStampSites,
    PropStamp, PropWalk, PropWalkStats, SIDEWALK_KERB_LIFT, SurfaceBand, carriageways,
    path_stamp_sites, stamp_content_offset, stamp_space_verts, walk_prop_rules, walkable_surfaces,
    yawed_basis,
};
pub use race::{
    CatchUpPolicy, Checkpoint, CheckpointRule, DEFAULT_CHECKPOINT_HEIGHT, DEFAULT_COUNTDOWN_TICKS,
    EventParams, NavTarget, ParticipantState, ProgressOutcome, RACE_TICK_HZ, RaceDefinition,
    RaceError, RacePhase, RaceProgress, RaceStart, RaceStarted, RaceState, TargetSelection,
    catch_up_factor, course_progress, cycle_target, effective_conditions, live_order,
    mean_gate_spacing, navigation_target, relative_bearing,
};
pub use recovery::{
    GroundContact, RecoveryCause, RecoveryEvent, RecoveryPolicy, RecoveryVerdict, VehicleRecovery,
};
pub use result::{DuplicateResult, ResultId, ResultLedger, SessionOutcome, SessionResult, ordinal};
pub use session::{
    Session, SessionEntity, SessionError, SessionPhase, advance_session_tick,
    despawn_session_entities,
};
pub use stuck::{StuckEvent, StuckSpec, StuckVerdict, VehicleStuck};
pub use surface::{SurfaceMaterial, SurfaceState};
pub use telemetry::{DamageSignals, VehicleTelemetry, WheelTelemetry};
pub use traffic::{
    AmbientPlan, AmbientRoster, AmbientSpec, Crossing, FollowPolicy, JunctionGate, JunctionPolicy,
    Junctions, KnockPolicy, LaneAdvance, LaneCursor, SignalAspect, SpawnDirective, SpawnDraw,
    SpawnPolicy, StuckPolicy, StuckWindow, TrafficIssue, advance_lane_cursor, corridor_gap,
    draw_spawn, eligible_lanes, follow_speed, in_spawn_band, inside_junction_zone, junction_speed,
    junction_zone, plan_ambient, spawn_occupied, within_interest,
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
