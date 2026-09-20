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
pub mod catalog;
pub mod convert;
pub mod events;
pub mod expect;
pub mod model;
pub mod nav;
pub mod race_def;

pub use assemble::{LoadError, TrailerDef, VehicleDef, load_by_id, load_vehicle};
pub use catalog::{
    CatalogEntry, DepSet, EXPECTED_STOCK_ROSTER, EntryStatus, VehicleCatalog, VehicleClass,
};
pub use convert::{
    ConversionReport, ConvertInput, Converted, Provenance, ReportEntry, WheelGeom, convert,
    convert_trailer,
};
pub use events::{
    CatalogEvent, EventCatalog, EventRecord, EventResolveError, EventStatus, EventTableStatus,
    ExtraRecord, FailedRef, RecordContent,
};
pub use expect::{
    EXPECTED_AUDIO_FAMILIES, EXPECTED_CITIES, EXPECTED_EVENT_TABLES, EXPECTED_PEDS,
    EXPECTED_RACE_CITIES, ExpectedEvent, PED_REQUIRED_EXTS, PrimaryRecord, expected_events,
};
pub use model::{
    Lod, MeshGroup, ModelPart, PartRole, VehicleModel, WheelVisual, build_model, classify_stem,
    shader_for_paint, split_lod,
};
pub use nav::{NavLoadError, load_nav_graph};
pub use race_def::{
    PLAYER_SLOT, RaceBuildError, RaceDefBuild, RaceDefEntry, RaceDefReport, RaceDefSummary,
    race_definition,
};
