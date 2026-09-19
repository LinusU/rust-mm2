//! Engine-independent intermediate vehicle model built from a parsed PKG
//! plus its `.mtx` part transforms.
//!
//! A [`VehicleModel`] preserves everything the renderer and validation need:
//! named parts, LOD membership, per-section shader bindings, paint-job
//! tables, wheel rig data, and attach transforms. Nothing is flattened into
//! a single mesh.
//!
//! ## Coordinate convention
//!
//! MM2 vehicle space is used verbatim (identity map): `-Z` forward, `+Y` up,
//! and the authored triangle winding is kept as-is — MM2 geometry renders
//! correctly in Bevy space without a mirror (see the city importer, where a
//! Z reflection rendered the whole world backwards).
//!
//! TEX textures decode top-down while PKG UVs are authored against the
//! bottom-up file order, so `v` is complemented when meshes are built.

use mm2_formats::mtx::Mtx;
use mm2_formats::pkg::{Pkg, PkgChunk, PkgGeometry, PkgShader, PkgShaders, PRIMTYPE_TRIANGLES};

/// Semantic role of a named part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartRole {
    /// `BODY` — the intact exterior.
    Body,
    /// `WHLn` — a wheel, positioned by its mtx origin.
    Wheel(usize),
    /// `TWHLn` — a trailer wheel.
    TrailerWheel(usize),
    /// `FNDRn` — fender/suspension piece following wheel `n`.
    Fender(usize),
    /// `BREAKnn` — attached breakable panel (intact representation).
    Break,
    /// `SRNn` — emergency-light bar elements.
    Siren(usize),
    /// `HEADLIGHTn` — headlight housing.
    Headlight(usize),
    /// `HLIGHT` — headlight glow quads.
    HeadlightGlow,
    /// `TLIGHT` — tail-light glow quads.
    TaillightGlow,
    /// `RLIGHT` — reverse-light glow quads.
    ReverseGlow,
    /// `BLIGHT` — brake-light glow quads.
    BrakeGlow,
    /// `SHADOW` — blob shadow (not rendered as part of the model).
    Shadow,
    /// `TRAILER_HITCH` — hitch geometry.
    Hitch,
    /// `TRAILER` — trailer body inside a `_trailer` pkg.
    TrailerBody,
    /// `EXHAUSTn` — exhaust tip.
    Exhaust(usize),
    /// Anything else, kept and reported.
    Other,
}

/// LOD tier of a part name's `_H`/`_M`/`_L`/`_VL` suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lod {
    Vl = 0,
    L = 1,
    M = 2,
    H = 3,
}

/// Split `WHL0_H` into stem `WHL0` + LOD `H`. Names without a recognised
/// suffix count as `H` (full detail).
pub fn split_lod(name: &str) -> (String, Lod) {
    let lower = name.to_ascii_lowercase();
    match lower.rsplit_once('_') {
        Some((stem, "h")) => (stem.to_string(), Lod::H),
        Some((stem, "m")) => (stem.to_string(), Lod::M),
        Some((stem, "l")) => (stem.to_string(), Lod::L),
        Some((stem, "vl")) => (stem.to_string(), Lod::Vl),
        _ => (lower, Lod::H),
    }
}

/// Classify a part stem (`whl0`, `break01`, `hlight`, ...) into a role.
pub fn classify_stem(stem_lower: &str) -> PartRole {
    let (base, num) = split_trailing_digits(stem_lower);
    match base.as_str() {
        "body" => PartRole::Body,
        "whl" => PartRole::Wheel(num.unwrap_or(0)),
        "twhl" => PartRole::TrailerWheel(num.unwrap_or(0)),
        "fndr" => PartRole::Fender(num.unwrap_or(0)),
        "srn" | "siren" => PartRole::Siren(num.unwrap_or(0)),
        "headlight" => PartRole::Headlight(num.unwrap_or(0)),
        "hlight" => PartRole::HeadlightGlow,
        "tlight" => PartRole::TaillightGlow,
        "rlight" => PartRole::ReverseGlow,
        "blight" => PartRole::BrakeGlow,
        "shadow" => PartRole::Shadow,
        "trailer_hitch" | "hitch" => PartRole::Hitch,
        "trailer" => PartRole::TrailerBody,
        "exhaust" => PartRole::Exhaust(num.unwrap_or(0)),
        _ => {
            if base.starts_with("break") {
                PartRole::Break
            } else {
                PartRole::Other
            }
        }
    }
}

/// Split `whl0` → (`whl`, Some(0)), `body` → (`body`, None),
/// `break01` → (`break`, Some(1)).
fn split_trailing_digits(stem: &str) -> (String, Option<usize>) {
    let digits: usize = stem
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .count();
    if digits == 0 {
        return (stem.to_string(), None);
    }
    let (base, num) = stem.split_at(stem.len() - digits);
    (base.to_string(), num.parse().ok())
}

/// One renderable mesh group of a part: all triangles sharing a shader.
#[derive(Debug, Clone, Default)]
pub struct MeshGroup {
    /// Index into the active paint job's shader list (`shader_offset`).
    pub shader_offset: usize,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// A named part with per-LOD mesh data and its attach transform.
#[derive(Debug, Clone)]
pub struct ModelPart {
    /// Part stem, lowercase, e.g. `whl0`, `body`, `headlight0`.
    pub name: String,
    pub role: PartRole,
    /// Geometry per LOD tier (ascending detail).
    pub lods: Vec<(Lod, Vec<MeshGroup>)>,
    /// Attach point in car space (mtx origin); `None` = authored in place.
    pub origin: Option<[f32; 3]>,
    /// Pivot/rotation centre from mtx, when meaningful.
    pub pivot: Option<[f32; 3]>,
    /// Translation to apply to geometry before placing the part at
    /// `origin` — used when a wheel's mesh is authored in place rather than
    /// around its own centre.
    pub recenter: Option<[f32; 3]>,
}

impl ModelPart {
    /// Best available LOD mesh groups.
    pub fn best_lod(&self) -> Option<&Vec<MeshGroup>> {
        self.lods.last().map(|(_, g)| g)
    }

    /// Best available LOD with real geometry — some stock parts (e.g.
    /// trailer wheels) ship a degenerate high LOD.
    pub fn best_nonempty_lod(&self) -> Option<&Vec<MeshGroup>> {
        self.lods
            .iter()
            .rev()
            .map(|(_, g)| g)
            .find(|g| g.iter().any(|mg| mg.positions.len() >= 3))
    }
}

/// Wheel rig entry: identity + transform + measured size.
#[derive(Debug, Clone)]
pub struct WheelVisual {
    /// Wheel index (`whlN` → N; trailer wheels use `twhlN`).
    pub index: usize,
    /// True for `twhl` trailer wheels.
    pub trailer: bool,
    /// Wheel centre in car space.
    pub origin: [f32; 3],
    /// Radius measured from the best-LOD geometry (fallback: mtx bounds).
    pub radius: f32,
    /// Width measured the same way.
    pub width: f32,
    /// Model part indices that belong to this wheel (the wheel itself plus
    /// linked fenders).
    pub parts: Vec<usize>,
}

/// Intermediate vehicle model.
#[derive(Debug, Default)]
pub struct VehicleModel {
    pub parts: Vec<ModelPart>,
    /// Wheel rig, ordered by wheel index.
    pub wheels: Vec<WheelVisual>,
    /// Paint-job table from the shaders chunk.
    pub paint_jobs: usize,
    /// Shaders per paint job.
    pub shaders_per_paint_job: usize,
    /// `paint_jobs × shaders_per_paint_job` shader records.
    pub shaders: Vec<PkgShader>,
    /// AABB of the `BODY` part(s), or all visible parts when absent.
    pub body_aabb: Option<([f32; 3], [f32; 3])>,
    /// The PKG `offset` chunk, preserved for diagnostics. Not applied to
    /// geometry — stock data shows parts already share car space.
    pub offset: Option<[f32; 3]>,
    /// PKG xrefs (referenced sub-models), preserved for diagnostics.
    pub xrefs: Vec<String>,
    /// Non-fatal issues found while assembling.
    pub warnings: Vec<String>,
}

fn geometry_mesh(geo: &PkgGeometry, warnings: &mut Vec<String>, part: &str) -> Vec<MeshGroup> {
    let mut groups: Vec<MeshGroup> = Vec::new();
    for section in &geo.sections {
        let off = section.shader_offset.max(0) as usize;
        let gi = match groups.iter().position(|g| g.shader_offset == off) {
            Some(i) => i,
            None => {
                groups.push(MeshGroup {
                    shader_offset: off,
                    ..Default::default()
                });
                groups.len() - 1
            }
        };
        let g = &mut groups[gi];
        for strip in &section.strips {
            if strip.prim_type != PRIMTYPE_TRIANGLES {
                warnings.push(format!(
                    "part {part}: unsupported primitive type {}",
                    strip.prim_type
                ));
                continue;
            }
            let base = g.positions.len() as u32;
            for v in &strip.vertices {
                g.positions.push(v.position);
                g.normals.push(v.normal.unwrap_or([0.0, 1.0, 0.0]));
                // PKG UVs are authored against TEX's bottom-up row order,
                // which the decoder normalises to top-down, so v is
                // complemented (same as the city importer's strip path).
                g.uvs.push(
                    v.tex_coords
                        .first()
                        .map(|&[u, v]| [u, 1.0 - v])
                        .unwrap_or([0.0, 0.0]),
                );
            }
            for t in strip.indices.chunks_exact(3) {
                g.indices.extend(t.iter().map(|&i| base + i as u32));
            }
        }
    }
    groups
}

fn mesh_aabb(groups: &[MeshGroup]) -> Option<([f32; 3], [f32; 3])> {
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    let mut any = false;
    for g in groups {
        for p in &g.positions {
            any = true;
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
    }
    any.then_some((min, max))
}

/// Build the intermediate model from a parsed PKG and its part transforms.
///
/// `mtx_for` resolves a part stem (`whl0`, `headlight0`, ...) to the parsed
/// `geometry/<pkg>_<part>.mtx` when present.
pub fn build_model(
    pkg: &Pkg,
    mut mtx_for: impl FnMut(&str) -> Option<Mtx>,
) -> VehicleModel {
    let mut model = VehicleModel::default();
    let mut warnings = Vec::new();

    if let Some(sh) = pkg.shaders() {
        model.paint_jobs = sh.paint_jobs as usize;
        model.shaders_per_paint_job = sh.shaders_per_paint_job as usize;
        model.shaders = sh.shaders.clone();
    }
    for f in &pkg.files {
        match &f.data {
            PkgChunk::Offset(o) => model.offset = Some(*o),
            PkgChunk::Xref(xs) => model.xrefs = xs.iter().map(|x| x.name.clone()).collect(),
            _ => {}
        }
    }

    // Group geometry chunks by part stem.
    let mut order: Vec<String> = Vec::new();
    let mut parts: std::collections::HashMap<String, ModelPart> =
        std::collections::HashMap::new();
    for (name, geo) in pkg.geometries() {
        let (stem, lod) = split_lod(name);
        let entry = parts.entry(stem.clone()).or_insert_with(|| {
            order.push(stem.clone());
            ModelPart {
                name: stem.clone(),
                role: classify_stem(&stem),
                lods: Vec::new(),
                origin: None,
                pivot: None,
                recenter: None,
            }
        });
        entry.lods.push((lod, geometry_mesh(geo, &mut warnings, name)));
    }
    for stem in order {
        let mut part = parts.remove(&stem).unwrap();
        part.lods.sort_by_key(|(lod, _)| *lod);
        let mtx = mtx_for(&stem);
        if let Some(m) = mtx {
            part.origin = Some(m.origin);
            if m.pivot.iter().any(|v| v.abs() > 1e-6) {
                part.pivot = Some(m.pivot);
            }
        }
        model.parts.push(part);
    }

    // Wheel rig: parts with Wheel/TrailerWheel roles get a WheelVisual;
    // fenders link to the nearest wheel index.
    let mut recentres: Vec<(usize, [f32; 3], String)> = Vec::new();
    for (i, part) in model.parts.iter().enumerate() {
        let (index, trailer) = match part.role {
            PartRole::Wheel(n) => (n, false),
            PartRole::TrailerWheel(n) => (n, true),
            _ => continue,
        };
        let mtx = mtx_for(&part.name);
        let (origin, mtx_radius, mtx_width) = match &mtx {
            Some(m) => (m.origin, m.wheel_radius(), m.wheel_width()),
            None => {
                warnings.push(format!(
                    "wheel part {} has no mtx transform; using geometry centre",
                    part.name
                ));
                let c = part
                    .best_nonempty_lod()
                    .and_then(|g| mesh_aabb(g))
                    .map(|(mn, mx)| {
                        [
                            (mn[0] + mx[0]) * 0.5,
                            (mn[1] + mx[1]) * 0.5,
                            (mn[2] + mx[2]) * 0.5,
                        ]
                    })
                    .unwrap_or([0.0, 0.0, 0.0]);
                (c, 0.0, 0.0)
            }
        };
        // Radius: prefer the wheel geometry's own extent (matches visuals);
        // fall back to the mtx bound when geometry is degenerate.
        if let Some((mn, mx)) = part.best_nonempty_lod().and_then(|g| mesh_aabb(g)) {
            let centre = [
                (mn[0] + mx[0]) * 0.5,
                (mn[1] + mx[1]) * 0.5,
                (mn[2] + mx[2]) * 0.5,
            ];
            let r = (mx[1] - mn[1]) * 0.5;
            // In-place authored wheel meshes (stock trailer wheels) are
            // recentred onto their own centroid so the mtx origin applies.
            let off = (centre[0] * centre[0] + centre[1] * centre[1] + centre[2] * centre[2]).sqrt();
            if r > 0.05 && off > r * 0.75 && mtx.is_some() {
                recentres.push((i, centre, part.name.clone()));
            }
        }
        let (radius, width) = part
            .best_nonempty_lod()
            .and_then(|g| mesh_aabb(g))
            .map(|(mn, mx)| {
                let r = (mx[1] - mn[1]) * 0.5;
                let w = (mx[0] - mn[0]) * 0.5;
                // Wheel geometry is authored around its own centre only when
                // the bbox is symmetric around the origin; trailer wheels are
                // sometimes authored in place — measure y-extent either way.
                (r.max(0.01), w.max(0.01))
            })
            .filter(|(r, _)| *r > 0.05)
            .unwrap_or((mtx_radius.max(0.05), mtx_width.max(0.01)));
        model.wheels.push(WheelVisual {
            index,
            trailer,
            origin,
            radius,
            width,
            parts: vec![i],
        });
    }
    for (i, centre, name) in recentres {
        model.parts[i].recenter = Some(centre);
        warnings.push(format!(
            "wheel part {name} geometry authored in place; recentring onto origin"
        ));
    }
    model.wheels.sort_by_key(|w| (w.trailer, w.index));

    // Link fenders to wheel indices.
    for (i, part) in model.parts.iter().enumerate() {
        if let PartRole::Fender(n) = part.role {
            if let Some(w) = model
                .wheels
                .iter_mut()
                .find(|w| !w.trailer && w.index == n)
            {
                w.parts.push(i);
            }
        }
    }

    // Body AABB: from BODY parts; fall back to all non-shadow parts.
    let body = model
        .parts
        .iter()
        .filter(|p| p.role == PartRole::Body)
        .filter_map(|p| p.best_lod().and_then(|g| mesh_aabb(g)))
        .collect::<Vec<_>>();
    let combined = |aabbs: &[([f32; 3], [f32; 3])]| {
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for (mn, mx) in aabbs {
            for i in 0..3 {
                min[i] = min[i].min(mn[i]);
                max[i] = max[i].max(mx[i]);
            }
        }
        (min, max)
    };
    model.body_aabb = if !body.is_empty() {
        Some(combined(&body))
    } else {
        let rest: Vec<_> = model
            .parts
            .iter()
            .filter(|p| !matches!(p.role, PartRole::Shadow | PartRole::Wheel(_) | PartRole::TrailerWheel(_)))
            .filter_map(|p| {
                let aabb = p.best_lod().and_then(|g| mesh_aabb(g))?;
                // Parts authored around a local origin move to their attach
                // point.
                Some(match p.origin {
                    Some(o) => (
                        [aabb.0[0] + o[0], aabb.0[1] + o[1], aabb.0[2] + o[2]],
                        [aabb.1[0] + o[0], aabb.1[1] + o[1], aabb.1[2] + o[2]],
                    ),
                    None => aabb,
                })
            })
            .collect();
        (!rest.is_empty()).then(|| combined(&rest))
    };

    model.warnings = warnings;
    model
}

/// The `shaders` chunk accessor with bounds checking for paint selection.
pub fn shader_for_paint<'a>(
    shaders: &'a PkgShaders,
    paint: usize,
    offset: i32,
) -> Option<&'a PkgShader> {
    if paint >= shaders.paint_jobs as usize || offset < 0 {
        return None;
    }
    let idx = paint * shaders.shaders_per_paint_job as usize + offset as usize;
    shaders.shaders.get(idx)
}
