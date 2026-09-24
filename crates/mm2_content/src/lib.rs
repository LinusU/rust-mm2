//! Bridge between MM2 game data and the engine runtime.
//!
//! [`catalog`] discovers playable vehicles through the VFS, [`assemble`]
//! loads every dependency of one vehicle, [`convert`] maps MM2 tuning onto
//! the Avian-based [`mm2_vehicle`] config, [`model`] builds the
//! intermediate part/LOD/paint representation consumed by the renderer,
//! and [`events`] scans a city's authored race records into the
//! [`EventCatalog`]. This crate is the producer side of the contract
//! split: it reads the VFS and runs `mm2_formats` parsers, while the
//! domain types it fills in (`EventRef`, `EventTableKind`) stay in
//! `mm2_game`.

pub mod assemble;
pub mod availability;
pub mod catalog;
pub mod convert;
pub mod damage;
pub mod events;
pub mod expect;
pub mod garage;
pub mod model;
pub mod nav;
pub mod opponents;
pub mod race_def;
pub mod rewards;
pub mod surface;
pub mod traffic;

pub use assemble::{
    AmbientVehicle, LoadError, TrailerDef, VehicleDef, ambient_engine_audio, ambient_vehicle,
    load_by_id, load_opponent, load_vehicle,
};
pub use availability::availability_table;
pub use catalog::{
    CatalogEntry, DepSet, EXPECTED_STOCK_ROSTER, EntryStatus, VehicleCatalog, VehicleClass,
};
pub use convert::{
    ConversionReport, ConvertInput, Converted, Provenance, ReportEntry, WheelGeom, convert,
    convert_trailer,
};
pub use damage::{DamageAudit, RecordCheck, VehicleDamageAssets};
pub use events::{
    CatalogEvent, EventCatalog, EventRecord, EventResolveError, EventStatus, EventTableStatus,
    ExtraRecord, FailedRef, RecordContent, race_cities,
};
pub use expect::{
    EXPECTED_AMBIENTS, EXPECTED_AUDIO_FAMILIES, EXPECTED_CITIES, EXPECTED_EVENT_TABLES,
    EXPECTED_PEDS, EXPECTED_RACE_CITIES, ExpectedEvent, PED_REQUIRED_EXTS, PrimaryRecord,
    expected_events,
};
pub use garage::{garage_table, scan_garage};
pub use model::{
    Lod, MeshGroup, ModelPart, PartRole, VehicleModel, WheelVisual, build_model, classify_stem,
    shader_for_paint, split_lod,
};
pub use nav::{NavLoadError, load_nav_graph, load_nav_overrides};
pub use opponents::{
    EventAimap, ExtraRoster, OpponentReport, RosterBuild, RosterBuildError, RosterEntry,
    RosterSummary, audit_roster, event_aimap, opponent_roster, opponent_roster_from_aimap,
};
pub use race_def::{
    PLAYER_SLOT, RaceBuildError, RaceDefBuild, RaceDefEntry, RaceDefReport, RaceDefSummary,
    audit_build, race_definition,
};
pub use rewards::reward_table;
pub use surface::{
    PsdlSurfaces, SurfaceLoadError, SurfaceSlot, SurfaceTables, load_surface_tables,
};
pub use traffic::{
    AmbientAssets, AmbientSetup, AssetCheck, EventOverride, TrafficAudit, TrafficLoadError,
    ambient_roster, ambient_roster_from_aimap, ambient_setup, load_city_aimap,
};
