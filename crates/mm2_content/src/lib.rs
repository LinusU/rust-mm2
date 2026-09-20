//! Bridge between MM2 game data and the engine runtime.
//!
//! [`catalog`] discovers playable vehicles through the VFS, [`assemble`]
//! loads every dependency of one vehicle, [`convert`] maps MM2 tuning onto
//! the Avian-based [`mm2_vehicle`] config, and [`model`] builds the
//! intermediate part/LOD/paint representation consumed by the renderer.

pub mod assemble;
pub mod catalog;
pub mod convert;
pub mod expect;
pub mod model;

pub use assemble::{LoadError, TrailerDef, VehicleDef, load_by_id, load_vehicle};
pub use catalog::{
    CatalogEntry, DepSet, EXPECTED_STOCK_ROSTER, EntryStatus, VehicleCatalog, VehicleClass,
};
pub use convert::{
    ConversionReport, ConvertInput, Converted, Provenance, ReportEntry, WheelGeom, convert,
    convert_trailer,
};
pub use expect::{
    EXPECTED_AUDIO_FAMILIES, EXPECTED_CITIES, EXPECTED_EVENT_TABLES, EXPECTED_PEDS,
    EXPECTED_RACE_CITIES, ExpectedEvent, PED_REQUIRED_EXTS, PrimaryRecord, expected_events,
};
pub use model::{
    Lod, MeshGroup, ModelPart, PartRole, VehicleModel, WheelVisual, build_model, classify_stem,
    shader_for_paint, split_lod,
};
