//! Vehicle assembly: resolve and parse every dependency of a catalog id
//! through the VFS, convert tuning into a [`VehicleConfig`], and build the
//! intermediate model. This is the single path used by the game, the
//! inspector and headless validation.

use mm2_assets::Vfs;
use mm2_formats::banger::BangerData;
use mm2_formats::bnd::BndFile;
use mm2_formats::mtx::Mtx;
use mm2_formats::pkg::Pkg;
use mm2_formats::tune::TuneFile;
use mm2_formats::veh::{AsNode, VehCarDamage, VehCarSim, VehGyro, VehStuck, VehTrailer};
use mm2_game::BangerDefinition;
use mm2_vehicle::config::VehicleConfig;

use crate::catalog::VehicleCatalog;
use crate::convert::{
    ConversionReport, ConvertInput, Converted, WheelGeom, convert, convert_trailer,
};
use crate::model::{VehicleModel, build_model};

/// Everything needed to spawn a stock/modded vehicle.
#[derive(Debug)]
pub struct VehicleDef {
    /// Catalog id.
    pub id: String,
    /// Display name from metadata.
    pub display_name: String,
    /// Paint names (`Colors`), zero-based.
    pub paints: Vec<String>,
    /// Effective handling config (MM2-converted).
    pub config: VehicleConfig,
    /// Intermediate visual model.
    pub model: VehicleModel,
    /// Towed trailer, when the vehicle has one (`vpsemi`, `vpcentury`).
    pub trailer: Option<TrailerDef>,
    /// `tune/vehicle/<id>.vehcardamage` — the authored damage model and
    /// its embedded effect spec (F05-A). `None` when the record does not
    /// resolve or does not decode; the vehicle still loads — the F05-B
    /// runtime treats `None` as "no authored damage tuning" and reports
    /// it rather than fabricating bounds.
    pub damage: Option<VehCarDamage>,
    /// `tune/vehicle/<id>.vehstuck` — authored stuck-detection
    /// thresholds. `None` when absent/undecodable (same policy as
    /// `damage`).
    pub stuck: Option<VehStuck>,
    /// `tune/vehicle/<id>.vehgyro` — authored rollover-recovery gyro
    /// assist. `None` when absent/undecodable (same policy as `damage`).
    pub gyro: Option<VehGyro>,
    /// Authored breakaway parts (F05-B.3): every `BREAK<NN>` model part
    /// whose fragment record `tune/banger/<id>_<stem>.dgbangerdata`
    /// resolves and decodes. A part missing either side — chunk or
    /// record — never enters the list: it stays bolted on rather than
    /// borrowing a fabricated threshold (same absence policy as
    /// `damage`/`stuck`/`gyro`; the census's dead fragments and
    /// record-less chunks are documented authored gaps, REC-1).
    pub breaks: Vec<BreakPart>,
    /// Conversion audit trail.
    pub report: ConversionReport,
    /// Resolved logical paths used for each dependency.
    pub sources: Vec<String>,
}

/// One detach-capable breakaway part: a `BREAK<NN>` model part paired
/// with its own distilled fragment record (F05-B.3).
#[derive(Debug, Clone)]
pub struct BreakPart {
    /// Model part stem, lowercase (`break0`, `break01`, ...) — the
    /// `VehicleModel.parts` entry carrying the intact geometry.
    pub name: String,
    /// `tune/banger/<id>_<name>.dgbangerdata` distilled — the part's
    /// authored detach threshold and fragment physicals.
    pub def: BangerDefinition,
}

/// Trailer assembly: physics config + model + joint anchors.
#[derive(Debug)]
pub struct TrailerDef {
    pub config: VehicleConfig,
    pub model: VehicleModel,
    /// Anchor point on the car (car space).
    pub car_hitch: [f32; 3],
    /// Anchor point on the trailer (trailer space).
    pub trailer_hitch: [f32; 3],
    /// Simulated trailer wheel geometry (positions/radii) in trailer
    /// space — decorative `twhl` detail parts are excluded.
    pub wheels: Vec<WheelGeom>,
}

/// Failure to load one vehicle.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// The id is not in the catalog.
    #[error("unknown vehicle {0:?} (see --list-cars)")]
    UnknownId(String),
    /// Catalog lookup failed (unknown or ambiguous query).
    #[error("{0}")]
    Lookup(String),
    /// A required file is absent from the VFS.
    #[error("{0}: missing {1}")]
    Missing(String, String),
    /// A file failed to parse.
    #[error("{0}: {1}")]
    Parse(String, String),
    /// Conversion failed.
    #[error("{0}")]
    Convert(String),
}

fn read(vfs: &Vfs, logical: &str, id: &str, what: &str) -> Result<(Vec<u8>, String), LoadError> {
    let (bytes, resolved) = vfs
        .read_path(logical)
        .map_err(|_| LoadError::Missing(id.into(), format!("{what} ({logical})")))?;
    let src = match (&resolved.source.label, resolved.source.archive_offset) {
        (Some(l), _) => format!("{logical} [mod {l}]"),
        (None, Some(off)) => format!("{logical} [{}#{off:#x}]", resolved.source.path.display()),
        (None, None) => format!("{logical} [{}]", resolved.source.path.display()),
    };
    Ok((bytes, src))
}

fn read_opt(vfs: &Vfs, logical: &str) -> Option<(Vec<u8>, String)> {
    let (bytes, resolved) = vfs.read_path(logical).ok()?;
    let src = match (&resolved.source.label, resolved.source.archive_offset) {
        (Some(l), _) => format!("{logical} [mod {l}]"),
        (None, Some(off)) => format!("{logical} [{}#{off:#x}]", resolved.source.path.display()),
        (None, None) => format!("{logical} [{}]", resolved.source.path.display()),
    };
    Some((bytes, src))
}

fn parse_tune(bytes: &[u8], id: &str, logical: &str) -> Result<TuneFile, LoadError> {
    TuneFile::parse(&String::from_utf8_lossy(bytes))
        .map_err(|e| LoadError::Parse(id.into(), format!("{logical}: {e}")))
}

fn load_mtx(vfs: &Vfs, pkg_stem: &str, part: &str) -> Option<Mtx> {
    let logical = format!("geometry/{pkg_stem}_{part}.mtx");
    let (bytes, _) = read_opt(vfs, &logical)?;
    Mtx::parse(&bytes).ok()
}

/// Resolve and parse an optional `tune/vehicle/<id>.<ext>` record —
/// `None` when it does not resolve; a resolved-but-malformed file
/// reports through `warnings` instead of sinking the load or
/// disappearing.
fn opt_tune(
    vfs: &Vfs,
    id: &str,
    ext: &str,
    sources: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Option<TuneFile> {
    let logical = format!("tune/vehicle/{id}.{ext}");
    let (bytes, src) = read_opt(vfs, &logical)?;
    sources.push(src);
    match parse_tune(&bytes, id, &logical) {
        Ok(t) => Some(t),
        Err(e) => {
            warnings.push(e.to_string());
            None
        }
    }
}

/// Load and convert one catalog vehicle.
///
/// `paint` selects the paint-job shader set (zero-based); it is validated
/// here so an invalid index fails before any geometry is emitted.
pub fn load_vehicle(vfs: &Vfs, id: &str, paint: usize) -> Result<VehicleDef, LoadError> {
    let logical = format!("tune/vehicle/{id}.vehcarsim");
    let (bytes, src) = read(vfs, &logical, id, "tuning")?;
    let tune = parse_tune(&bytes, id, &logical)?;
    load_vehicle_impl(vfs, id, paint, tune, vec![src])
}

/// Load a catalog vehicle for AI opponent use (F15-A.2).
///
/// MM2 ships opponent tuning at `tune/vehicle/<id>_opp.vehcarsim` for
/// nearly every stock car. The files are *sparse overrides* over the
/// base `.vehcarsim`, not standalone tunes: authored values differ
/// (e.g. `vpbug_opp` has different inertia box, drivetrain and end-train
/// inertias, horsepower, top speed), but most `_opp` files omit required
/// fields and several author transmission data in an alternate schema
/// (`NumGears`/`GearRatios`/`UpshiftRPM`/`DownshiftRPM`/`DownshiftBias`
/// instead of the `ManualNumGears`/`Low`/`High` band schema our decoder
/// reads). The loader therefore merges the `_opp` document over the
/// base tune: fields the variant authors win, everything else inherits
/// the vehicle's own tuning, and `_opp`-only fields our schema does not
/// model surface through the usual unrecognised-field warnings rather
/// than failing the load. Mods or partial installs without the variant
/// fall back to the base tune: a missing variant is not a failure.
/// Every other dependency (`.info`, `.pkg`, `.bnd`, `.mtx`, `.asnode`,
/// `.vehtrailer`) stays the vehicle's own. The merge policy is designed:
/// the `_opp` files' existence and contents are verified retail data,
/// their exact original consumption is not (see RACE-12/UNK-11).
pub fn load_opponent(vfs: &Vfs, id: &str, paint: usize) -> Result<VehicleDef, LoadError> {
    let logical = format!("tune/vehicle/{id}.vehcarsim");
    let (bytes, src) = read(vfs, &logical, id, "tuning")?;
    let mut tune = parse_tune(&bytes, id, &logical)?;
    let mut tune_sources = vec![src];

    let opp_logical = format!("tune/vehicle/{id}_opp.vehcarsim");
    if let Some((bytes, src)) = read_opt(vfs, &opp_logical) {
        let opp = parse_tune(&bytes, id, &opp_logical)?;
        tune_sources.push(src);
        tune.root.merge_overlay(&opp.root);
    }
    load_vehicle_impl(vfs, id, paint, tune, tune_sources)
}

/// Shared loader behind [`load_vehicle`]/[`load_opponent`]: `id` owns
/// every dependency except the `.vehcarsim`, which arrives pre-parsed
/// (opponent loads merge the `_opp` override over the base document).
fn load_vehicle_impl(
    vfs: &Vfs,
    id: &str,
    paint: usize,
    tune: TuneFile,
    tune_sources: Vec<String>,
) -> Result<VehicleDef, LoadError> {
    let mut sources = tune_sources;

    // Metadata (optional but preferred; alternates handled by the catalog).
    let mut display_name = id.to_string();
    let mut paints = Vec::new();
    for ext in ["info", "inf", "vinfo", "info.bak"] {
        let logical = format!("tune/{id}.{ext}");
        if let Some((bytes, src)) = read_opt(vfs, &logical) {
            sources.push(src);
            let info = mm2_formats::info::InfoFile::parse(&String::from_utf8_lossy(&bytes));
            if let Some(d) = info.get("Description") {
                display_name = d.to_string();
            }
            paints = info.list("Colors");
            break;
        }
    }

    // Tuning — required; the caller supplies the parsed document
    // (`load_opponent` merges the authored `<id>_opp` override first).
    let sim =
        VehCarSim::from_tune(&tune).map_err(|e| LoadError::Parse(id.into(), e.to_string()))?;

    // Optional steering-assist data.
    let asnode = read_opt(vfs, &format!("tune/{id}.asnode")).and_then(|(bytes, src)| {
        sources.push(src);
        let tune = parse_tune(&bytes, id, "asnode").ok()?;
        Some(AsNode::from_tune(tune))
    });

    // Damage/recovery tuning (F05-A) — optional records, decoded
    // through the same tune grammar. A record that resolves but fails
    // to parse/decode is reported in the conversion warnings rather
    // than sinking the load or vanishing silently.
    let mut damage_warnings: Vec<String> = Vec::new();
    let damage =
        opt_tune(vfs, id, "vehcardamage", &mut sources, &mut damage_warnings).and_then(|t| {
            VehCarDamage::from_tune(&t)
                .map_err(|e| damage_warnings.push(format!("{id}.vehcardamage: {e}")))
                .ok()
        });
    let stuck = opt_tune(vfs, id, "vehstuck", &mut sources, &mut damage_warnings).and_then(|t| {
        VehStuck::from_tune(&t)
            .map_err(|e| damage_warnings.push(format!("{id}.vehstuck: {e}")))
            .ok()
    });
    let gyro = opt_tune(vfs, id, "vehgyro", &mut sources, &mut damage_warnings).and_then(|t| {
        VehGyro::from_tune(&t)
            .map_err(|e| damage_warnings.push(format!("{id}.vehgyro: {e}")))
            .ok()
    });
    // Surface decoder warnings too — unknown authored fields must not
    // drop silently.
    for w in damage
        .iter()
        .flat_map(|d| d.warnings.iter().cloned())
        .chain(stuck.iter().flat_map(|s| s.warnings.iter().cloned()))
        .chain(gyro.iter().flat_map(|g| g.warnings.iter().cloned()))
    {
        damage_warnings.push(w);
    }

    // Geometry — required.
    let logical = format!("geometry/{id}.pkg");
    let (bytes, src) = read(vfs, &logical, id, "model")?;
    sources.push(src);
    let pkg =
        Pkg::parse(&bytes).map_err(|e| LoadError::Parse(id.into(), format!("{logical}: {e}")))?;
    let model = build_model(&pkg, |stem| load_mtx(vfs, id, stem));
    for stem in model.parts.iter().map(|p| p.name.clone()) {
        let logical = format!("geometry/{id}_{stem}.mtx");
        if let Some((_, src)) = read_opt(vfs, &logical) {
            sources.push(src);
        }
    }

    // Breakaway inventory (F05-B.3): a `BREAK<NN>` model part detaches
    // only when its own `tune/banger/<id>_<stem>.dgbangerdata` record
    // resolves and decodes — the record carries the authored
    // `ImpulseLimit2` the runtime compares against. Chunks without
    // records and records without chunks stay attached/absent (the
    // census's documented authored gaps, REC-1); a resolved record
    // that fails to parse is a load warning, not a sink.
    let mut breaks = Vec::new();
    for part in &model.parts {
        if part.role != crate::model::PartRole::Break {
            continue;
        }
        let logical = format!("tune/banger/{id}_{}.dgbangerdata", part.name);
        let Some((bytes, src)) = read_opt(vfs, &logical) else {
            continue;
        };
        sources.push(src);
        match std::str::from_utf8(&bytes)
            .ok()
            .and_then(|t| BangerData::parse(t).ok())
        {
            Some(data) => {
                for issue in data.validate() {
                    damage_warnings.push(format!("{logical}: {issue}"));
                }
                breaks.push(BreakPart {
                    name: part.name.clone(),
                    def: BangerDefinition::from_record(format!("{id}_{}", part.name), &data),
                });
            }
            None => {
                damage_warnings.push(format!("{logical}: failed to parse"));
            }
        }
    }

    // Bounds — optional but expected for stock.
    let bound = read_opt(vfs, &format!("bound/{id}_bound.bnd")).and_then(|(bytes, src)| {
        sources.push(src);
        BndFile::parse(&String::from_utf8_lossy(&bytes)).ok()
    });

    let body_aabb = model.body_aabb.unwrap_or(([0.0; 3], [1.0, 1.0, 2.0]));

    // Wheel geometry for the physics rig.
    let wheel_geoms: Vec<WheelGeom> = model
        .wheels
        .iter()
        .filter(|w| !w.trailer)
        .map(|w| WheelGeom {
            index: w.index,
            origin: w.origin,
            radius: w.radius,
        })
        .collect();

    let input = ConvertInput {
        id,
        display_name: &display_name,
        sim: &sim,
        asnode: asnode.as_ref(),
        wheels: &wheel_geoms,
        bound: bound.as_ref(),
        body_aabb,
        stuck: stuck.as_ref(),
        gyro: gyro.as_ref(),
    };
    let Converted { config, report } = convert(&input).map_err(LoadError::Convert)?;

    if let Err(problems) = config.validate() {
        return Err(LoadError::Convert(format!(
            "{id}: converted config invalid: {}",
            problems.join("; ")
        )));
    }

    // Trailer (vpsemi / vpcentury).
    let mut report = report;
    let trailer = match read_opt(vfs, &format!("tune/vehicle/{id}.vehtrailer")) {
        Some((bytes, src)) => {
            sources.push(src);
            let tune = parse_tune(&bytes, id, "vehtrailer")?;
            let t = VehTrailer::from_tune(&tune)
                .map_err(|e| LoadError::Parse(id.into(), e.to_string()))?;
            let (def, treport) = load_trailer(vfs, id, &t, &mut sources, body_aabb)?;
            report.entries.extend(treport.entries);
            report.warnings.extend(treport.warnings);
            Some(def)
        }
        None => None,
    };

    // Paint validation: declared paints must map onto the shader table.
    let paint_jobs = model.paint_jobs.max(1);
    if paint >= paint_jobs {
        return Err(LoadError::Parse(
            id.into(),
            format!(
                "paint index {paint} out of range: model has {paint_jobs} paint job(s), metadata declares {}",
                paints.len()
            ),
        ));
    }
    if !paints.is_empty() && paints.len() != paint_jobs {
        report.warnings.push(format!(
            "metadata declares {} paints but the model has {paint_jobs} paint job(s)",
            paints.len()
        ));
    }
    report.warnings.extend(damage_warnings);

    Ok(VehicleDef {
        id: id.to_string(),
        display_name,
        paints,
        config,
        model,
        trailer,
        damage,
        stuck,
        gyro,
        breaks,
        report,
        sources,
    })
}

fn load_trailer(
    vfs: &Vfs,
    id: &str,
    t: &VehTrailer,
    sources: &mut Vec<String>,
    car_body_aabb: ([f32; 3], [f32; 3]),
) -> Result<(TrailerDef, ConversionReport), LoadError> {
    let pkg_id = format!("{id}_trailer");
    let logical = format!("geometry/{pkg_id}.pkg");
    let (bytes, src) = read(vfs, &logical, id, "trailer model")?;
    sources.push(src);
    let pkg =
        Pkg::parse(&bytes).map_err(|e| LoadError::Parse(id.into(), format!("{logical}: {e}")))?;
    let model = build_model(&pkg, |stem| load_mtx(vfs, &pkg_id, stem));

    let bound = read_opt(vfs, &format!("bound/{pkg_id}_bound.bnd")).and_then(|(bytes, src)| {
        sources.push(src);
        BndFile::parse(&String::from_utf8_lossy(&bytes)).ok()
    });
    let body_aabb = model.body_aabb.unwrap_or(([0.0; 3], [1.0, 1.0, 2.0]));

    // Only `simulated` wheel visuals become physics wheels — decorative
    // `twhl` detail parts (flagged in `build_model`) would otherwise add
    // raycasts that can never reach the ground and dilute the per-wheel
    // load the suspension rates are derived from.
    let wheel_geoms: Vec<WheelGeom> = model
        .wheels
        .iter()
        .filter(|w| w.simulated)
        .map(|w| WheelGeom {
            index: w.index,
            origin: w.origin,
            radius: w.radius,
        })
        .collect();

    let Converted { config, report } =
        convert_trailer(id, t, &wheel_geoms, bound.as_ref(), body_aabb)
            .map_err(LoadError::Convert)?;
    let mut report = report;
    report.warnings.extend(model.warnings.iter().cloned());

    // Hitch anchors: authored offsets when present; fall back to the car's
    // rear bound edge and the trailer's front bound edge.
    let car_hitch = t
        .car_hitch_offset
        .unwrap_or([0.0, 0.45, car_body_aabb.1[2] - 0.4]);
    let trailer_hitch = t
        .trailer_hitch_offset
        .unwrap_or([0.0, 0.55, body_aabb.0[2] + 0.4]);

    Ok((
        TrailerDef {
            config,
            model,
            car_hitch,
            trailer_hitch,
            wheels: wheel_geoms,
        },
        report,
    ))
}

/// Ambient (`va_*`) vehicle assembly: geometry + transforms + bound.
/// Ambient cars are not catalog vehicles — their `aivehicledata` tune
/// carries no drivetrain, so there is no `VehicleConfig` to convert;
/// the roster resolves the tune separately. The `.pkg`/`.mtx`/`.bnd`
/// set resolves through the same VFS paths a stock car's does.
#[derive(Debug)]
pub struct AmbientVehicle {
    /// Roster id (`va_*`).
    pub id: String,
    /// Intermediate visual model.
    pub model: VehicleModel,
    /// Parsed bound, when it resolves and parses.
    pub bound: Option<BndFile>,
}

/// Load one ambient class's visual/physical assets. `geometry/<id>.pkg`
/// is required — a class whose model does not resolve cannot spawn;
/// the bound is optional (the runtime falls back to a hull over the
/// tuning's authored `Size`).
pub fn ambient_vehicle(vfs: &Vfs, id: &str) -> Result<AmbientVehicle, LoadError> {
    let logical = format!("geometry/{id}.pkg");
    let (bytes, _) = read(vfs, &logical, id, "model")?;
    let pkg =
        Pkg::parse(&bytes).map_err(|e| LoadError::Parse(id.into(), format!("{logical}: {e}")))?;
    let model = build_model(&pkg, |stem| load_mtx(vfs, id, stem));
    let bound = read_opt(vfs, &format!("bound/{id}_bound.bnd"))
        .and_then(|(bytes, _)| BndFile::parse(&String::from_utf8_lossy(&bytes)).ok());
    Ok(AmbientVehicle {
        id: id.to_string(),
        model,
        bound,
    })
}

/// Convenience: catalog scan + vehicle load in one call.
pub fn load_by_id(vfs: &Vfs, query: &str, paint: usize) -> Result<VehicleDef, LoadError> {
    let catalog = VehicleCatalog::scan(vfs);
    let entry = catalog.find(query).map_err(LoadError::Lookup)?;
    let id = entry.id.clone();
    load_vehicle(vfs, &id, paint)
}

/// Apply a `--vehicle-config` TOML override on top of an imported vehicle.
///
/// The override replaces the handling definition wholesale (documented as
/// a *full* override), except that wheel positions and radii are re-pinned
/// to the imported rig — those are geometry, and changing them would
/// misalign the physics wheels against the visual wheel parts. A
/// wheel-count mismatch is rejected rather than silently misplacing
/// wheels. The vehicle's identity (`VehicleDef`) is unaffected.
pub fn apply_handling_override(
    imported: &VehicleConfig,
    mut over: VehicleConfig,
) -> Result<VehicleConfig, String> {
    if over.wheels.len() != imported.wheels.len() {
        return Err(format!(
            "override defines {} wheels but the selected vehicle's rig has {} — \
             a wheel-count change needs a matching model, not just tuning",
            over.wheels.len(),
            imported.wheels.len()
        ));
    }
    for (w, src) in over.wheels.iter_mut().zip(&imported.wheels) {
        w.position = src.position;
        w.radius = src.radius;
    }
    // Collision geometry is part of the model, not the handling override.
    over.collider_points = imported.collider_points.clone();
    over.striker_points = imported.striker_points.clone();
    over.chassis_size = imported.chassis_size;
    Ok(over)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The override wins handling fields but the imported rig's wheel
    /// positions/radii and collision geometry stay pinned — a tune file
    /// must not move the physics wheels off the visual ones.
    #[test]
    fn override_replaces_handling_but_repins_the_rig() {
        let mut imported = VehicleConfig::default();
        imported.wheels[0].position = [9.0, 9.0, 9.0];
        imported.wheels[0].radius = 0.99;
        imported.collider_points = Some(vec![[1.0, 2.0, 3.0]]);
        imported.chassis_size = [4.0, 5.0, 6.0];

        let mut over = VehicleConfig {
            mass: 2500.0,
            ..VehicleConfig::default()
        };
        over.engine.peak_torque_nm = 999.0;
        over.wheels[0].position = [-7.0, -7.0, -7.0];
        over.wheels[0].radius = 0.11;
        over.collider_points = Some(vec![[8.0, 8.0, 8.0]]);
        over.chassis_size = [1.0, 1.0, 1.0];

        let out = apply_handling_override(&imported, over).unwrap();
        assert_eq!(out.mass, 2500.0);
        assert_eq!(out.engine.peak_torque_nm, 999.0);
        assert_eq!(out.wheels[0].position, [9.0, 9.0, 9.0]);
        assert_eq!(out.wheels[0].radius, 0.99);
        assert_eq!(out.collider_points, Some(vec![[1.0, 2.0, 3.0]]));
        assert_eq!(out.chassis_size, [4.0, 5.0, 6.0]);
    }

    /// A wheel-count change needs a matching model — the override is
    /// rejected, not silently misplacing wheels.
    #[test]
    fn a_wheel_count_mismatch_is_rejected() {
        let imported = VehicleConfig::default();
        let mut over = VehicleConfig::default();
        over.wheels.pop();
        let err = apply_handling_override(&imported, over).unwrap_err();
        assert!(err.contains("wheels"), "{err}");
    }
}
