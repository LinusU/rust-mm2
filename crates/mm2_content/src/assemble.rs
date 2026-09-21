//! Vehicle assembly: resolve and parse every dependency of a catalog id
//! through the VFS, convert tuning into a [`VehicleConfig`], and build the
//! intermediate model. This is the single path used by the game, the
//! inspector and headless validation.

use mm2_assets::Vfs;
use mm2_formats::bnd::BndFile;
use mm2_formats::mtx::Mtx;
use mm2_formats::pkg::Pkg;
use mm2_formats::tune::TuneFile;
use mm2_formats::veh::{AsNode, VehCarSim, VehTrailer};
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
    /// Conversion audit trail.
    pub report: ConversionReport,
    /// Resolved logical paths used for each dependency.
    pub sources: Vec<String>,
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
    /// Trailer wheel geometry (positions/radii) in trailer space.
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

    Ok(VehicleDef {
        id: id.to_string(),
        display_name,
        paints,
        config,
        model,
        trailer,
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

    let wheel_geoms: Vec<WheelGeom> = model
        .wheels
        .iter()
        .map(|w| WheelGeom {
            index: w.index,
            origin: w.origin,
            radius: w.radius,
        })
        .collect();

    let Converted { config, report } =
        convert_trailer(id, t, &wheel_geoms, bound.as_ref(), body_aabb)
            .map_err(LoadError::Convert)?;

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
