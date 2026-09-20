//! VFS producer and per-PSDL resolution for the authored surface
//! tables (F06-A): the global `city/materials.{mtl,csv}` pair.
//!
//! [`SurfaceTables`] is the session's surface identity space: a
//! `SurfaceMaterial::Authored(i)` on a collider indexes
//! [`MaterialSet::defs`]. The authored lookup chain is texture name →
//! `materials.csv` → `materials.mtl` (WLD-19); which original consumer
//! used it for which query stays unverified (UNK-23), so this module
//! only classifies — it does not claim what `friction` or `sound` mean
//! to the original force path.
//!
//! Like `race_def`/`nav`, this crate reads the VFS and runs
//! `mm2_formats` parsers; the domain component the classification
//! fills (`SurfaceMaterial`) stays in `mm2_game`.

use std::collections::BTreeSet;
use std::fmt;

use bevy::prelude::Resource;
use mm2_assets::{AssetsError, Resolved, Vfs};
use mm2_formats::FormatError;
use mm2_formats::materials::{MaterialMap, MaterialSet, NONE_PHYSICS};
use mm2_formats::tex::frame_base_stem;

/// Logical path of the material property blocks.
pub const MTL_PATH: &str = "city/materials.mtl";
/// Logical path of the texture → material map.
pub const CSV_PATH: &str = "city/materials.csv";

/// What one PSDL texture-table name resolves to through the tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceSlot {
    /// A named material — the index into [`MaterialSet::defs`] that a
    /// `SurfaceMaterial::Authored` component carries.
    Material(u16),
    /// The `none` keyword: the default surface — authored data, not a
    /// defect.
    Default,
    /// An authored blank slot in the texture table.
    Blank,
    /// The tables could not classify the name — no csv row, or a row
    /// pointing at an undefined material (the retail `ash`/`mud` dead
    /// refs). The conservative policy is the default surface plus a
    /// recorded diagnostic (F06-AC04).
    Unmapped,
}

impl SurfaceSlot {
    /// The `SurfaceMaterial::Authored` index, when this slot names a
    /// defined material.
    pub fn material_index(self) -> Option<u16> {
        match self {
            SurfaceSlot::Material(i) => Some(i),
            _ => None,
        }
    }
}

/// The loaded `city/materials.{mtl,csv}` pair — the session's surface
/// identity space, held as a session-scoped resource so consumers can
/// resolve a `SurfaceMaterial::Authored(i)` code back to the material
/// name and its authored fields.
#[derive(Resource, Debug, Clone)]
pub struct SurfaceTables {
    /// Material property blocks in authored order — the index space
    /// `SurfaceMaterial::Authored` refers to.
    pub set: MaterialSet,
    /// Texture-stem → material-name map.
    pub map: MaterialMap,
}

impl SurfaceTables {
    /// Classify one PSDL texture-table name. The full name is tried
    /// first, then the animated-frame base stem (`<stem>-NNNN`):
    /// `materials.csv` keys a sequence's base name while the table
    /// references single frames — the same convention the materials
    /// audit applies (WLD-19).
    pub fn slot_for(&self, texture: &str) -> SurfaceSlot {
        if texture.is_empty() {
            return SurfaceSlot::Blank;
        }
        self.slot_of_stem(texture)
            .or_else(|| frame_base_stem(texture).and_then(|b| self.slot_of_stem(b)))
            .unwrap_or(SurfaceSlot::Unmapped)
    }

    /// One csv row: `None` when no row names `stem`.
    fn slot_of_stem(&self, stem: &str) -> Option<SurfaceSlot> {
        let physics = self.map.lookup(stem)?;
        Some(if physics == NONE_PHYSICS {
            SurfaceSlot::Default
        } else {
            match self.set.index(physics).and_then(|i| u16::try_from(i).ok()) {
                Some(i) => SurfaceSlot::Material(i),
                // A dead authored ref (or a table with > u16::MAX
                // materials) classifies as unmapped, not as the named
                // surface it cannot point at.
                None => SurfaceSlot::Unmapped,
            }
        })
    }

    /// Resolve a PSDL texture table: one slot per name plus the set of
    /// names that fell back to the conservative default.
    pub fn resolve_psdl(&self, textures: &[String]) -> PsdlSurfaces {
        let mut unmapped = BTreeSet::new();
        let slots = textures
            .iter()
            .map(|t| {
                let slot = self.slot_for(t);
                if slot == SurfaceSlot::Unmapped {
                    unmapped.insert(t.clone());
                }
                slot
            })
            .collect();
        PsdlSurfaces { slots, unmapped }
    }

    /// Table-level issue count: `validate()` on both halves plus the
    /// csv → mtl dead-reference check the audit reports.
    pub fn issues(&self) -> usize {
        self.set.validate().len()
            + self.map.validate().len()
            + self.map.undefined_refs(&self.set).len()
    }
}

/// Per-PSDL resolution of [`SurfaceTables`]: `slots[i]` classifies
/// `psdl.textures[i]`.
#[derive(Debug)]
pub struct PsdlSurfaces {
    /// One slot per PSDL texture-table entry.
    pub slots: Vec<SurfaceSlot>,
    /// Names that resolved to [`SurfaceSlot::Unmapped`].
    pub unmapped: BTreeSet<String>,
}

/// Why the surface tables could not be produced.
#[derive(Debug)]
pub enum SurfaceLoadError {
    /// Only one half of the pair resolved — a partial table is a
    /// defect, not an absent feature.
    Missing(&'static str),
    /// The resolved bytes were not readable.
    Read(AssetsError),
    /// The file is not UTF-8 text.
    Utf8(&'static str),
    /// The file does not parse.
    Parse(FormatError),
}

impl fmt::Display for SurfaceLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SurfaceLoadError::Missing(p) => {
                write!(f, "{p}: absent while its pair half resolves")
            }
            SurfaceLoadError::Read(e) => write!(f, "read failed: {e}"),
            SurfaceLoadError::Utf8(p) => write!(f, "{p}: not UTF-8 text"),
            SurfaceLoadError::Parse(e) => write!(f, "parse failed: {e}"),
        }
    }
}

impl std::error::Error for SurfaceLoadError {}

/// Load the global `city/materials.{mtl,csv}` pair through the VFS.
///
/// `Ok(None)` means *neither* file resolves — the install carries no
/// surface tables and every collider stays `Unspecified`. A pair with
/// exactly one half present is `Err(Missing)`: a broken table, not an
/// absent one. Both files must decode as UTF-8 and parse; a corrupt
/// pair is `Err`, never a silently partial classification.
pub fn load_surface_tables(vfs: &Vfs) -> Result<Option<SurfaceTables>, SurfaceLoadError> {
    let (mtl, csv) = (vfs.resolve(MTL_PATH), vfs.resolve(CSV_PATH));
    let (Some(mtl), Some(csv)) = (&mtl, &csv) else {
        return match (mtl.is_none(), csv.is_none()) {
            (true, true) => Ok(None),
            (true, false) => Err(SurfaceLoadError::Missing(MTL_PATH)),
            (false, true) => Err(SurfaceLoadError::Missing(CSV_PATH)),
            (false, false) => unreachable!(),
        };
    };
    let text = |res: &Resolved, path: &'static str| {
        let bytes = vfs.read(res).map_err(SurfaceLoadError::Read)?;
        String::from_utf8(bytes).map_err(|_| SurfaceLoadError::Utf8(path))
    };
    let set = MaterialSet::parse(&text(mtl, MTL_PATH)?).map_err(SurfaceLoadError::Parse)?;
    let map = MaterialMap::parse(&text(csv, CSV_PATH)?).map_err(SurfaceLoadError::Parse)?;
    Ok(Some(SurfaceTables { set, map }))
}
