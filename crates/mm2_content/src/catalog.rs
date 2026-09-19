//! Deterministic vehicle catalog built from the mounted VFS.
//!
//! Roster enumeration is data-driven: a vehicle id is a basename shared by
//! `tune/<id>.info` (metadata), `tune/vehicle/<id>.vehcarsim` (tuning) and
//! `geometry/<id>.pkg` (model). Entries that exist but are incomplete are
//! kept in the catalog with an explicit error list — they are audited, not
//! silently dropped.
//!
//! Suffix-tagged tuning variants (`_opp` opponent, `_cop` police pursuit,
//! `_dash` dashboard input, `_old`/`bak` backups) are not standalone cars
//! and are excluded from enumeration.

use std::collections::BTreeSet;

use mm2_assets::{Resolved, Vfs};
use mm2_formats::info::InfoFile;

/// Suffixes that mark tuning variants rather than standalone vehicles.
/// `_trailer` models are attachments assembled through their host vehicle,
/// never standalone cars.
const VARIANT_SUFFIXES: &[&str] = &["_opp", "_cop", "_dash", "_old", "_trailer"];

/// The stock player roster, audited against a retail MM2 installation.
///
/// This is the independent expected set used by validation: it must match
/// what the catalog discovers on a stock install, but it is not a filter —
/// extra mod cars still appear, and missing stock cars are reportable
/// failures. Twenty `.info` files plus `vpmoonrover` (`.inf`).
pub const EXPECTED_STOCK_ROSTER: &[&str] = &[
    "vp4x4",
    "vpauditt",
    "vpbug",
    "vpbullet",
    "vpbus",
    "vpcab",
    "vpcaddie",
    "vpcentury",
    "vpcoop",
    "vpcoop2k",
    "vpcop",
    "vpdb7",
    "vpddbus",
    "vpdune",
    "vpford",
    "vpmoonrover",
    "vpmustang99",
    "vppanoz",
    "vppanozgt",
    "vpsemi",
    "vpvwcup",
];

/// Where a catalog entry's data comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VehicleClass {
    /// Resolved entirely from the mounted installation (archives/loose).
    Stock,
    /// At least one required file resolves from a mod source.
    Mod,
    /// No stock data at all — only mod-supplied files.
    ModOnly,
}

/// Which dependency families an entry has resolved.
#[derive(Debug, Clone, Default)]
pub struct DepSet {
    /// Logical path + winning source for the metadata file.
    pub info: Option<String>,
    /// `tune/vehicle/<id>.vehcarsim`.
    pub carsim: Option<String>,
    /// `geometry/<id>.pkg`.
    pub model: Option<String>,
    /// `bound/<id>_bound.bnd`.
    pub bound: Option<String>,
    /// Wheel part transforms `geometry/<id>_whlN.mtx`, contiguous from 0.
    pub wheel_mtx: u32,
    /// `tune/vehicle/<id>.vehtrailer` (the vehicle tows a trailer).
    pub trailer: Option<String>,
    /// `tune/<id>.asnode` steering-assist data.
    pub asnode: Option<String>,
}

/// Completeness of a catalog entry.
#[derive(Debug, Clone)]
pub enum EntryStatus {
    /// All required dependencies resolve.
    Ready,
    /// One or more required dependencies are missing or failed.
    Incomplete {
        /// Human-readable reasons.
        missing: Vec<String>,
    },
}

/// One catalog entry: a candidate vehicle id plus its audit status.
#[derive(Debug, Clone)]
pub struct CatalogEntry {
    /// Stable catalog id (lowercase basename, e.g. `vpbug`).
    pub id: String,
    /// Display name from metadata (`Description`), or the id when absent.
    pub display_name: String,
    /// Paint variant names from `Colors` (zero-based index into this list).
    pub paints: Vec<String>,
    /// Locked/reward vehicle in stock progression (`UnlockScore`/`UnlockFlags`).
    pub locked: bool,
    /// Stock / mod classification.
    pub class: VehicleClass,
    /// Resolved dependency paths.
    pub deps: DepSet,
    /// Completeness status.
    pub status: EntryStatus,
    /// Metadata diagnostics and notes.
    pub notes: Vec<String>,
}

impl CatalogEntry {
    /// Whether every required dependency resolved.
    pub fn is_ready(&self) -> bool {
        matches!(self.status, EntryStatus::Ready)
    }
}

/// The discovered vehicle roster.
#[derive(Debug, Default)]
pub struct VehicleCatalog {
    /// All candidate entries, sorted by id.
    pub entries: Vec<CatalogEntry>,
}

fn is_variant(id: &str) -> bool {
    VARIANT_SUFFIXES.iter().any(|s| id.ends_with(s))
        || id.starts_with("copy of ")
        || id.ends_with(".vehcarsimold")
}

fn describe(resolved: &Resolved) -> String {
    let src = &resolved.source;
    match (&src.label, src.archive_offset) {
        (Some(label), _) => format!("{} [mod {label}]", resolved.logical),
        (None, Some(off)) => format!("{} [{}#{off:#x}]", resolved.logical, src.path.display()),
        (None, None) => format!("{} [{}]", resolved.logical, src.path.display()),
    }
}

/// A resolution is mod-supplied when its source carries a mod label.
fn is_mod(resolved: &Resolved) -> bool {
    resolved.source.label.is_some()
}

impl VehicleCatalog {
    /// Scan the mounted VFS and enumerate candidate vehicles.
    pub fn scan(vfs: &Vfs) -> Self {
        let mut ids: BTreeSet<String> = BTreeSet::new();
        for path in vfs.list() {
            let lower = path.to_ascii_lowercase();
            // tune/<id>.{info,inf,vinfo} and tune/<id>.info.bak
            if let Some(rest) = lower.strip_prefix("tune/") {
                if let Some(base) = rest
                    .strip_suffix(".info")
                    .or_else(|| rest.strip_suffix(".inf"))
                    .or_else(|| rest.strip_suffix(".vinfo"))
                    .or_else(|| rest.strip_suffix(".info.bak"))
                {
                    if !base.contains('/') {
                        ids.insert(base.to_string());
                    }
                }
            }
            if let Some(rest) = lower.strip_prefix("tune/vehicle/") {
                if let Some(base) = rest.strip_suffix(".vehcarsim") {
                    ids.insert(base.to_string());
                }
            }
            // Player-car geometry uses the `vp` prefix; traffic (`va`),
            // props and scenery are excluded.
            if let Some(rest) = lower.strip_prefix("geometry/") {
                if let Some(base) = rest.strip_suffix(".pkg") {
                    if base.starts_with("vp") && !base.contains('/') {
                        ids.insert(base.to_string());
                    }
                }
            }
        }

        let mut entries = Vec::new();
        for id in ids {
            if is_variant(&id) {
                continue;
            }
            entries.push(Self::probe(vfs, &id));
        }
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        VehicleCatalog { entries }
    }

    /// Probe a single vehicle id for its dependencies.
    fn probe(vfs: &Vfs, id: &str) -> CatalogEntry {
        let mut deps = DepSet::default();
        let mut notes = Vec::new();
        let mut any_mod = false;
        let mut all_mod = true;

        let note_src = |r: &Resolved, any_mod: &mut bool, all_mod: &mut bool| {
            if is_mod(r) {
                *any_mod = true;
            } else {
                *all_mod = false;
            }
        };

        // Metadata: prefer .info, fall back to known alternates.
        let info_res = vfs
            .resolve(&format!("tune/{id}.info"))
            .or_else(|| vfs.resolve(&format!("tune/{id}.inf")))
            .or_else(|| vfs.resolve(&format!("tune/{id}.vinfo")))
            .or_else(|| vfs.resolve(&format!("tune/{id}.info.bak")));

        let mut display_name = id.to_string();
        let mut paints = Vec::new();
        let mut locked = false;
        let mut saw_info = false;

        match &info_res {
            Some(r) => {
                note_src(r, &mut any_mod, &mut all_mod);
                deps.info = Some(describe(r));
                saw_info = true;
                match vfs.read(r) {
                    Ok(bytes) => {
                        let info = InfoFile::parse(&String::from_utf8_lossy(&bytes));
                        for d in &info.diagnostics {
                            notes.push(format!("metadata: {d}"));
                        }
                        if let Some(d) = info.get("Description") {
                            display_name = d.to_string();
                        }
                        paints = info.list("Colors");
                        locked = info.u32("UnlockScore").unwrap_or(0) > 0
                            || info.u32("UnlockFlags").unwrap_or(0) != 0;
                        if let Some(base) = info.get("BaseName") {
                            if !base.eq_ignore_ascii_case(id) {
                                notes.push(format!(
                                    "BaseName {base:?} differs from catalog id {id:?}"
                                ));
                            }
                        }
                    }
                    Err(e) => notes.push(format!("metadata read failed: {e}")),
                }
            }
            None => {}
        }

        if let Some(r) = vfs.resolve(&format!("tune/vehicle/{id}.vehcarsim")) {
            note_src(&r, &mut any_mod, &mut all_mod);
            deps.carsim = Some(describe(&r));
        }
        if let Some(r) = vfs.resolve(&format!("geometry/{id}.pkg")) {
            note_src(&r, &mut any_mod, &mut all_mod);
            deps.model = Some(describe(&r));
        }
        if let Some(r) = vfs.resolve(&format!("bound/{id}_bound.bnd")) {
            note_src(&r, &mut any_mod, &mut all_mod);
            deps.bound = Some(describe(&r));
        }
        for n in 0..16u32 {
            if vfs.resolve(&format!("geometry/{id}_whl{n}.mtx")).is_some() {
                deps.wheel_mtx += 1;
            } else {
                break;
            }
        }
        if let Some(r) = vfs.resolve(&format!("tune/vehicle/{id}.vehtrailer")) {
            note_src(&r, &mut any_mod, &mut all_mod);
            deps.trailer = Some(describe(&r));
        }
        if let Some(r) = vfs.resolve(&format!("tune/{id}.asnode")) {
            deps.asnode = Some(describe(&r));
        }

        let mut missing = Vec::new();
        if !saw_info {
            missing.push("metadata (tune/<id>.info)".to_string());
        }
        if deps.carsim.is_none() {
            missing.push("tuning (tune/vehicle/<id>.vehcarsim)".to_string());
        }
        if deps.model.is_none() {
            missing.push("model (geometry/<id>.pkg)".to_string());
        }
        if deps.bound.is_none() {
            missing.push("bounds (bound/<id>_bound.bnd)".to_string());
        }
        if deps.wheel_mtx == 0 {
            missing.push("wheel transforms (geometry/<id>_whl0.mtx)".to_string());
        }

        let class = if all_mod {
            VehicleClass::ModOnly
        } else if any_mod {
            VehicleClass::Mod
        } else {
            VehicleClass::Stock
        };

        CatalogEntry {
            id: id.to_string(),
            display_name,
            paints,
            locked,
            class,
            deps,
            status: if missing.is_empty() {
                EntryStatus::Ready
            } else {
                EntryStatus::Incomplete { missing }
            },
            notes,
        }
    }

    /// Look up an entry by id (case-insensitive) or unique display-name
    /// alias. Returns `Err` describing unknown/ambiguous lookups.
    pub fn find(&self, query: &str) -> Result<&CatalogEntry, String> {
        let q = query.to_ascii_lowercase();
        if let Some(e) = self.entries.iter().find(|e| e.id == q) {
            return Ok(e);
        }
        let alias_matches: Vec<&CatalogEntry> = self
            .entries
            .iter()
            .filter(|e| e.display_name.to_ascii_lowercase() == q)
            .collect();
        match alias_matches.len() {
            1 => return Ok(alias_matches[0]),
            0 => {}
            _ => {
                return Err(format!(
                    "vehicle name {query:?} is ambiguous: {}",
                    alias_matches
                        .iter()
                        .map(|e| e.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        Err(format!("unknown vehicle id {query:?} (see --list-cars)"))
    }

    /// Entries of the expected stock roster that are missing or incomplete.
    pub fn stock_audit_failures(&self) -> Vec<String> {
        let mut out = Vec::new();
        for id in EXPECTED_STOCK_ROSTER {
            match self.entries.iter().find(|e| e.id == *id) {
                None => out.push(format!("{id}: not discovered")),
                Some(e) => {
                    if let EntryStatus::Incomplete { missing } = &e.status {
                        out.push(format!("{id}: missing {}", missing.join(", ")));
                    }
                }
            }
        }
        out
    }
}
