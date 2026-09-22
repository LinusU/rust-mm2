//! Vehicle damage / stuck / gyro record census and breakaway-part
//! inventory (F05-A.1).
//!
//! [`DamageAudit::scan`] walks every discovered
//! `tune/vehicle/*.{vehcardamage,vehstuck,vehgyro}` through the
//! production `mm2_formats::veh` decoders — the same files
//! [`crate::assemble::load_vehicle`] attaches to `VehicleDef` — and
//! pairs each catalog vehicle with its authored breakaway inventory:
//! the `BREAK*` chunks inside `geometry/<id>.pkg`, the
//! `geometry/<id>_break*.mtx` transform parts and the
//! `tune/banger/<id>_break*.dgbangerdata` fragment records that bind
//! them (the `<base>_break<N>` → `BREAK<N>`-in-`<base>.pkg` convention
//! the banger audit resolves fragment references through).
//!
//! Records on ids outside the vehicle catalog stay in the denominator
//! as [`DamageAudit::uncatalogued`]. Strict failures are parse/decode
//! rejects and `validate()` issues; missing records (`vpmoonrover`
//! ships no `vehcardamage`, four ids ship partial `vehgyro`), orphaned
//! records and dead break fragments are reported findings, not
//! failures — authored gaps and extras are legitimate data.

use std::collections::BTreeSet;

use mm2_assets::Vfs;
use mm2_formats::pkg::Pkg;
use mm2_formats::tune::TuneFile;
use mm2_formats::veh::{DamageIssue, VehCarDamage, VehGyro, VehStuck};

use crate::catalog::VehicleCatalog;
use crate::model::{PartRole, classify_stem, split_lod};
use crate::traffic::AssetCheck;

/// Outcome for one discovered `tune/vehicle/<id>.<ext>` damage-family
/// record — parse, decode and authored-consistency check.
#[derive(Debug)]
pub struct RecordCheck {
    /// Vehicle id (file stem under `tune/vehicle/`).
    pub id: String,
    /// Logical path of the record.
    pub logical: String,
    /// Read/parse/decode status. `Missing` never appears here — the
    /// denominator is discovered files; per-vehicle absence is an
    /// [`AssetCheck::Missing`] on [`VehicleDamageAssets`] instead.
    pub status: AssetCheck,
    /// `validate()` issues on a decoded record.
    pub issues: Vec<String>,
    /// Decoder warnings (unrecognized fields preserved verbatim).
    pub warnings: Vec<String>,
}

/// One catalog entry's damage-family records plus its authored
/// breakaway-part inventory.
#[derive(Debug)]
pub struct VehicleDamageAssets {
    /// Catalog id.
    pub id: String,
    /// `tune/vehicle/<id>.vehcardamage`.
    pub damage: AssetCheck,
    /// `tune/vehicle/<id>.vehstuck`.
    pub stuck: AssetCheck,
    /// `tune/vehicle/<id>.vehgyro`.
    pub gyro: AssetCheck,
    /// `validate()` issues across the decoded records.
    pub issues: Vec<String>,
    /// Decoder warnings across the decoded records.
    pub warnings: Vec<String>,
    /// Distinct `BREAK*` stems authored in `geometry/<id>.pkg`
    /// (`None` when the pkg does not resolve or fails to parse — the
    /// inventory is unknowable, not empty).
    pub break_chunks: Option<Vec<String>>,
    /// `geometry/<id>_break*.mtx` stems discovered.
    pub break_mtx: Vec<String>,
    /// `tune/banger/<id>_break*.dgbangerdata` stems discovered — the
    /// fragment records the banger system binds.
    pub break_records: Vec<String>,
    /// Break-record stems binding to neither a pkg chunk nor an mtx
    /// part — authored dead fragments (reported findings; the banger
    /// audit owns their strict status).
    pub dead_breaks: Vec<String>,
}

/// The damage-family census: every catalog vehicle's record coverage
/// and breakaway inventory, plus records on uncatalogued ids.
#[derive(Debug)]
pub struct DamageAudit {
    /// Per catalog entry, in catalog order.
    pub assets: Vec<VehicleDamageAssets>,
    /// Damage/stuck/gyro records whose id is not a catalog vehicle —
    /// one [`RecordCheck`] per `(id, extension)` pair. Empty on a
    /// stock install (every retail damage-family id is catalogued via
    /// its `.vehcarsim`); mod installs can introduce orphans, which
    /// stay in the denominator here rather than being filtered out.
    pub uncatalogued: Vec<RecordCheck>,
    /// Non-fatal findings: pkg failures, dead break fragments.
    pub diagnostics: Vec<String>,
}

/// Read, parse and decode `tune/vehicle/<id>.<ext>` through `decode`,
/// which returns the record's decoder warnings and `validate()`
/// issues. Issues/warnings accumulate into the out-params with the
/// logical path prefixed.
fn check_record(
    vfs: &Vfs,
    id: &str,
    ext: &str,
    issues: &mut Vec<String>,
    warnings: &mut Vec<String>,
    decode: impl FnOnce(&TuneFile) -> Result<(Vec<String>, Vec<DamageIssue>), String>,
) -> AssetCheck {
    let logical = format!("tune/vehicle/{id}.{ext}");
    let Some(resolved) = vfs.resolve(&logical) else {
        return AssetCheck::Missing;
    };
    let bytes = match vfs.read(&resolved) {
        Ok(b) => b,
        Err(e) => return AssetCheck::Failed(format!("{logical}: read failed: {e}")),
    };
    let tune = match TuneFile::parse(&String::from_utf8_lossy(&bytes)) {
        Ok(t) => t,
        Err(e) => return AssetCheck::Failed(format!("{logical}: {e}")),
    };
    match decode(&tune) {
        Ok((w, i)) => {
            warnings.extend(w.into_iter().map(|w| format!("{logical}: {w}")));
            issues.extend(i.into_iter().map(|i| format!("{logical}: {i}")));
            AssetCheck::Parsed
        }
        Err(e) => AssetCheck::Failed(format!("{logical}: {e}")),
    }
}

fn check_damage(
    vfs: &Vfs,
    id: &str,
    issues: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> AssetCheck {
    check_record(vfs, id, "vehcardamage", issues, warnings, |t| {
        VehCarDamage::from_tune(t)
            .map(|d| {
                let v = d.validate();
                (d.warnings, v)
            })
            .map_err(|e| e.to_string())
    })
}

fn check_stuck(
    vfs: &Vfs,
    id: &str,
    issues: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> AssetCheck {
    check_record(vfs, id, "vehstuck", issues, warnings, |t| {
        VehStuck::from_tune(t)
            .map(|d| {
                let v = d.validate();
                (d.warnings, v)
            })
            .map_err(|e| e.to_string())
    })
}

fn check_gyro(
    vfs: &Vfs,
    id: &str,
    issues: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> AssetCheck {
    check_record(vfs, id, "vehgyro", issues, warnings, |t| {
        VehGyro::from_tune(t)
            .map(|d| {
                let v = d.validate();
                (d.warnings, v)
            })
            .map_err(|e| e.to_string())
    })
}

impl DamageAudit {
    /// Scan the mounted VFS: every catalog vehicle's damage coverage,
    /// every uncatalogued damage-family record and the authored
    /// breakaway inventory. Never fails as a whole — a partial
    /// install reports honestly.
    pub fn scan(vfs: &Vfs) -> Self {
        let catalog = VehicleCatalog::scan(vfs);
        let catalog_ids: BTreeSet<String> = catalog.entries.iter().map(|e| e.id.clone()).collect();

        // Logical paths once — the break inventory and the
        // uncatalogued-record census both read the same listing.
        let paths = vfs.list();
        let damage_files: BTreeSet<(String, String)> = paths
            .iter()
            .filter_map(|p| {
                let rest = p.strip_prefix("tune/vehicle/")?;
                for ext in ["vehcardamage", "vehstuck", "vehgyro"] {
                    if let Some(id) = rest.strip_suffix(&format!(".{ext}")) {
                        return Some((id.to_string(), ext.to_string()));
                    }
                }
                None
            })
            .collect();

        let mut diagnostics = Vec::new();
        let mut assets = Vec::new();
        for entry in &catalog.entries {
            let id = &entry.id;
            let mut issues = Vec::new();
            let mut warnings = Vec::new();
            let damage = check_damage(vfs, id, &mut issues, &mut warnings);
            let stuck = check_stuck(vfs, id, &mut issues, &mut warnings);
            let gyro = check_gyro(vfs, id, &mut issues, &mut warnings);

            // Breakaway inventory: BREAK* chunk stems in the pkg, the
            // _break*.mtx transform parts and the banger fragment
            // records, cross-checked for dead authored references.
            let logical = format!("geometry/{id}.pkg");
            let resolved_pkg = vfs.resolve(&logical);
            let break_chunks = resolved_pkg
                .as_ref()
                .and_then(|r| vfs.read(r).ok())
                .and_then(|bytes| Pkg::parse(&bytes).ok())
                .map(|pkg| {
                    pkg.files
                        .iter()
                        .filter_map(|f| {
                            let (stem, _lod) = split_lod(&f.name);
                            (classify_stem(&stem) == PartRole::Break).then_some(stem)
                        })
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect::<Vec<_>>()
                });
            if resolved_pkg.is_some() && break_chunks.is_none() {
                diagnostics.push(format!("{logical}: resolved but failed to parse"));
            }

            let geom_prefix = format!("geometry/{id}_break");
            let break_mtx: Vec<String> = paths
                .iter()
                .filter_map(|p| {
                    p.strip_prefix(&geom_prefix)
                        .and_then(|s| s.strip_suffix(".mtx"))
                        .map(|s| format!("break{s}"))
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let banger_prefix = format!("tune/banger/{id}_break");
            let break_records: Vec<String> = paths
                .iter()
                .filter_map(|p| {
                    p.strip_prefix(&banger_prefix)
                        .and_then(|s| s.strip_suffix(".dgbangerdata"))
                        .map(|s| format!("break{s}"))
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();

            // A record binds through either a pkg chunk or an mtx part.
            // When the pkg resolves but will not parse the chunk side is
            // unknowable — leave the dead-fragment claim to the banger
            // audit rather than flagging records we cannot disprove.
            let bound: BTreeSet<&String> = break_chunks
                .iter()
                .flatten()
                .chain(break_mtx.iter())
                .collect();
            let pkg_unparseable = resolved_pkg.is_some() && break_chunks.is_none();
            let dead_breaks: Vec<String> = if pkg_unparseable {
                Vec::new()
            } else {
                break_records
                    .iter()
                    .filter(|s| !bound.contains(*s))
                    .cloned()
                    .collect()
            };
            for d in &dead_breaks {
                diagnostics.push(format!(
                    "{id}: banger record {d} binds to no pkg chunk or mtx part"
                ));
            }

            assets.push(VehicleDamageAssets {
                id: id.clone(),
                damage,
                stuck,
                gyro,
                issues,
                warnings,
                break_chunks,
                break_mtx,
                break_records,
                dead_breaks,
            });
        }

        let uncatalogued = damage_files
            .iter()
            .filter(|(id, _)| !catalog_ids.contains(id))
            .map(|(id, ext)| {
                let mut issues = Vec::new();
                let mut warnings = Vec::new();
                let status = match ext.as_str() {
                    "vehcardamage" => check_damage(vfs, id, &mut issues, &mut warnings),
                    "vehstuck" => check_stuck(vfs, id, &mut issues, &mut warnings),
                    _ => check_gyro(vfs, id, &mut issues, &mut warnings),
                };
                RecordCheck {
                    id: id.clone(),
                    logical: format!("tune/vehicle/{id}.{ext}"),
                    status,
                    issues,
                    warnings,
                }
            })
            .collect();

        DamageAudit {
            assets,
            uncatalogued,
            diagnostics,
        }
    }

    /// Distinct vehicle ids carrying at least one damage-family record
    /// — the discovered denominator.
    pub fn discovered(&self) -> usize {
        self.assets
            .iter()
            .filter(|a| {
                a.damage != AssetCheck::Missing
                    || a.stuck != AssetCheck::Missing
                    || a.gyro != AssetCheck::Missing
            })
            .count()
            + self
                .uncatalogued
                .iter()
                .map(|r| &r.id)
                .collect::<BTreeSet<_>>()
                .len()
    }

    /// Strict-audit failures: any decode reject or validation issue on
    /// a catalogued or uncatalogued record. Missing records are
    /// authored absences — retail's `vpmoonrover` ships no
    /// `vehcardamage` and four ids ship partial `vehgyro` — reported
    /// findings, never failures.
    pub fn failures(&self) -> Vec<String> {
        let mut out = Vec::new();
        for a in &self.assets {
            for (what, check) in [
                ("vehcardamage", &a.damage),
                ("vehstuck", &a.stuck),
                ("vehgyro", &a.gyro),
            ] {
                if let AssetCheck::Failed(e) = check {
                    out.push(format!("{}: {what} failed: {e}", a.id));
                }
            }
            out.extend(a.issues.iter().map(|i| format!("{}: {i}", a.id)));
        }
        for r in &self.uncatalogued {
            if let AssetCheck::Failed(e) = &r.status {
                out.push(format!("{}: {e}", r.logical));
            }
            out.extend(r.issues.iter().map(|i| format!("{}: {i}", r.logical)));
        }
        out
    }
}
