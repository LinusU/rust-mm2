//! Decal pathset stamping (F03-B.4): `decals.pathset` beside the PSDL
//! paints road markings — the SF cable-car rails (`r4i_rails_f`),
//! London's zigzag junction lines (`decal_zigzag_l`), box-junction
//! crosshatch (`decal_x_inter_l`) and crossings (`decal_rxwalk03_l`).
//!
//! Geometry, measured on the retail `decals*.pathset` files (see
//! `docs/research/pathset.md`): a decal path is a `LineStrip` whose
//! points interleave the ribbon's two edges — even indices one edge,
//! odd the other — each (even, odd) pair a cross-section with the
//! authored width (~1 m zigzag, ~2 m rails, 8–20 m junction paint).
//! Consecutive sections join into quads; `u` runs across the pair in
//! authored order, `v` along the strip's centre line, tiled every
//! `spacing` metres (the quarter-metre field, defaulting to 5 m like
//! Angel's own `dgPath::setSpacing` guard).
//!
//! Material policy — inferred, not verified: palette-alpha is honored
//! on every palette format (the P8 decals carry authored translucency
//! that only the decal renderer reads), alpha-bearing textures render
//! alpha-blended, and every decal is lit and double-sided with no
//! collision — a small vertex lift plus depth bias sits it over its
//! surface without z-fighting. Texture stems resolve through the VFS
//! with the shared extension order, so a mod can override
//! `texture/decal_zigzag_l` like any other texture.
//!
//! `props.pathset` also names decal textures (31 `r4i_rails_f` paths
//! on SF — 26 byte-identical duplicates of `decals.pathset` entries);
//! the prop channel keeps classifying them without stamping — they are
//! authoring leftovers the prop loader ignores, and double-stamping
//! would z-fight the rail street.

use std::collections::{BTreeMap, BTreeSet};

use bevy::{
    mesh::{Indices, PrimitiveTopology},
    pbr::StandardMaterial,
    prelude::*,
};
use mm2_assets::Vfs;
use mm2_formats::pathset::{self, PathKind};
use mm2_game::{CityEntity, SessionEntity};
use tracing::{debug, info, warn};

use crate::city::{MaterialCache, TEXTURE_EXTS, v3};

/// Hard bound on ribbon quads one consumed file may emit — retail
/// produces ~600/city; a hostile file is counted, not walked
/// unbounded.
const MAX_DECAL_QUADS: usize = 100_000;

/// Lift above the authored surface applied to ribbon vertices. Decals
/// are authored coplanar with the road; a small offset plus
/// [`DECAL_DEPTH_BIAS`] keeps them from z-fighting it.
const DECAL_LIFT: f32 = 0.02;

/// Depth bias pulling decal fragments toward the viewer (wgpu:
/// negative = closer). Authored decal Y already hugs the surface, so
/// this is the far-field guard where [`DECAL_LIFT`] falls below depth
/// precision. `pub(crate)` — [`MaterialCache::get_decal`] applies it.
pub(crate) const DECAL_DEPTH_BIAS: f32 = -1.0;

/// Default `v` tile length when a path carries `spacing == 0` — Angel's
/// own `dgPath::setSpacing` guard defaults nonpositive spacing to 5 m.
const DEFAULT_V_TILE: f32 = 5.0;

/// One path's ribbon output: authored-edge quads plus diagnostics.
#[derive(Debug, Default)]
pub struct DecalRibbon {
    /// World-space ribbon vertices (edge pairs interleaved as authored).
    pub positions: Vec<Vec3>,
    /// `(u, v)` per vertex — `u` 0→1 across the authored pair, `v`
    /// centre-line arclength / tile.
    pub uvs: Vec<[f32; 2]>,
    /// Accumulated per-vertex normals (normalized after all quads).
    pub normals: Vec<Vec3>,
    /// Triangle indices into `positions` (local to this ribbon).
    pub indices: Vec<u32>,
    /// Quads emitted.
    pub quads: usize,
    /// Quads dropped: non-finite or degenerate sections.
    pub skipped_quads: usize,
    /// The path had an odd point count — the trailing point is dropped.
    pub odd_tail: bool,
}

/// Build a path's ribbon (F03-B.4 measured rule): even points are one
/// edge (`u = 0`), odd the other (`u = 1`); each index pair is a
/// cross-section and consecutive sections make a quad. `v` is the
/// centre-line arclength in `v_tile` metre tiles. Vertices sit
/// [`DECAL_LIFT`] above the authored points; normals accumulate the
/// quad normal oriented upward so banked strips shade like their road.
///
/// Anything malformed contributes counts, not panics: an odd trailing
/// point is dropped (`odd_tail`), non-finite sections kill their two
/// adjacent quads (`skipped_quads`), and zero-area quads are skipped.
/// A path under four points yields an empty ribbon — one cross-section
/// has no extent.
fn build_ribbon(points: &[[f32; 3]], v_tile: f32) -> DecalRibbon {
    let mut out = DecalRibbon::default();
    let sections = points.len() / 2;
    out.odd_tail = points.len() % 2 == 1;
    if sections == 0 {
        return out;
    }
    out.positions.reserve(sections * 2);
    out.uvs.reserve(sections * 2);
    out.normals.resize(sections * 2, Vec3::ZERO);

    // Section validity + centre-line arclength, section by section.
    let mut valid = Vec::with_capacity(sections);
    let mut s = 0.0f32;
    let mut prev_mid: Option<Vec3> = None;
    for i in 0..sections {
        let a = v3(points[2 * i]);
        let b = v3(points[2 * i + 1]);
        let mid = (a + b) * 0.5;
        let finite = a.is_finite() && b.is_finite();
        if finite {
            if let Some(p) = prev_mid {
                s += (mid - p).length();
            }
            prev_mid = Some(mid);
        } else {
            prev_mid = None;
        }
        valid.push(finite);
        out.positions.push(a + Vec3::Y * DECAL_LIFT);
        out.uvs.push([0.0, s / v_tile]);
        out.positions.push(b + Vec3::Y * DECAL_LIFT);
        out.uvs.push([1.0, s / v_tile]);
    }

    for i in 0..sections.saturating_sub(1) {
        if !valid[i] || !valid[i + 1] {
            out.skipped_quads += 1;
            continue;
        }
        let (a0, b0) = (out.positions[2 * i], out.positions[2 * i + 1]);
        let (a1, b1) = (out.positions[2 * i + 2], out.positions[2 * i + 3]);
        // cross(along, across) faces +Y on flat ground; average both
        // edges' advance so a degenerate edge still yields the strip's
        // real normal; orient upward so banked strips keep their
        // road-facing normal.
        let along = (a1 - a0) + (b1 - b0);
        let n = along.cross(b0 - a0);
        let Some(mut n) = n.try_normalize() else {
            out.skipped_quads += 1;
            continue;
        };
        if n.y < 0.0 {
            n = -n;
        }
        let base = 2 * i as u32;
        out.indices
            .extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
        for vi in [2 * i, 2 * i + 1, 2 * i + 2, 2 * i + 3] {
            out.normals[vi] += n;
        }
        out.quads += 1;
    }
    for n in &mut out.normals {
        *n = n.try_normalize().unwrap_or(Vec3::Y);
    }
    out
}

/// What one consumed `decals.pathset` produced, classified — every
/// path lands in a count, nothing is dropped silently.
#[derive(Debug, Default, Clone, Copy)]
pub struct DecalStampReport {
    /// Paths that produced at least one ribbon quad.
    pub ribbons: usize,
    /// Ribbon quads emitted across all paths.
    pub quads: usize,
    /// Merged mesh entities spawned (one per resolved texture stem).
    pub entities: usize,
    /// `PATHnn` route labels — never asset references.
    pub label_paths: usize,
    /// `giz_*` animated-object paths — counted, left for that work.
    pub animated_paths: usize,
    /// Paths naming a `geometry/*.pkg` prop rather than a texture —
    /// misfiled in a decal file, counted not stamped.
    pub prop_paths: usize,
    /// Paths whose name resolves to neither a texture nor a PKG.
    pub unresolved_paths: usize,
    /// Zero-point paths — authored empties (retail carries dozens).
    pub empty_paths: usize,
    /// Non-empty paths that produced no quads (< 4 points or all
    /// sections degenerate/non-finite).
    pub degenerate_paths: usize,
    /// Paths with an odd point count; the trailing point is dropped.
    pub odd_point_paths: usize,
    /// Texture-named paths whose `kind` is not `LineStrip` — retail
    /// decals are all strips; other kinds are uninterpreted.
    pub unsupported_kind_paths: usize,
    /// Quads dropped for non-finite or degenerate sections.
    pub skipped_quads: usize,
    /// Quads suppressed by the file's [`MAX_DECAL_QUADS`] budget.
    pub capped: usize,
    /// Texture stems that resolved but failed to read/decode (once
    /// per stem per file).
    pub missing_textures: usize,
    /// `Pathset::validate()` issues on the file.
    pub issues: usize,
}

/// Accumulates ribbons into one merged mesh.
#[derive(Default)]
struct RibbonBuilder {
    positions: Vec<Vec3>,
    uvs: Vec<[f32; 2]>,
    normals: Vec<Vec3>,
    indices: Vec<u32>,
}

impl RibbonBuilder {
    fn push(&mut self, ribbon: &DecalRibbon) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(&ribbon.positions);
        self.uvs.extend_from_slice(&ribbon.uvs);
        self.normals.extend_from_slice(&ribbon.normals);
        self.indices.extend(ribbon.indices.iter().map(|i| base + i));
    }

    fn build(self) -> Mesh {
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals);
        mesh.insert_indices(Indices::U32(self.indices));
        mesh
    }
}

/// Stamp every decal path of a parsed `decals.pathset` (F03-B.4):
/// texture-named `LineStrip` paths become ribbon quads merged into one
/// mesh per texture stem — session-owned, render-only, no colliders.
/// All other names are classified like the prop channel: `PATHnn`
/// labels, `giz_*` animated objects, PKG-named props (misfiled here)
/// and dead references land in the report, never silently dropped.
///
/// `logical` names the consumed file in diagnostics; `name_prefix`
/// distinguishes spawned entities by consumer (`decal` for the ambient
/// city file). Materials come from the shared [`MaterialCache`] decal
/// path (palette-alpha honoring, blend/opaque by decoded alpha).
#[allow(clippy::too_many_arguments)]
pub fn stamp_decals(
    commands: &mut Commands,
    vfs: &Vfs,
    mats: &mut MaterialCache,
    pathset: &pathset::Pathset,
    logical: &str,
    name_prefix: &str,
    meshes: &mut Assets<Mesh>,
    owner: SessionEntity,
) -> DecalStampReport {
    let mut report = DecalStampReport::default();
    for issue in pathset.validate() {
        report.issues += 1;
        warn!(path = %logical, %issue, "decal pathset authored issue");
    }
    let mut missing: BTreeSet<String> = BTreeSet::new();
    // BTreeMap → deterministic entity order regardless of path order.
    let mut groups: BTreeMap<String, (RibbonBuilder, Handle<StandardMaterial>)> = BTreeMap::new();
    let mut quads_left = MAX_DECAL_QUADS;
    for path in &pathset.paths {
        let Some(name) = path.asset_name() else {
            report.label_paths += 1;
            continue;
        };
        if name.starts_with("giz_") {
            report.animated_paths += 1;
            continue;
        }
        // A decal path is a texture reference; a name resolving to a
        // PKG is a misfiled prop — classified, not stamped here.
        if vfs
            .resolve_preferred(&format!("texture/{name}"), TEXTURE_EXTS)
            .is_none()
        {
            if vfs.resolve(&format!("geometry/{name}.pkg")).is_some() {
                report.prop_paths += 1;
            } else {
                report.unresolved_paths += 1;
            }
            continue;
        }
        if path.points.is_empty() {
            report.empty_paths += 1;
            continue;
        }
        if path.kind() != Some(PathKind::LineStrip) {
            report.unsupported_kind_paths += 1;
            continue;
        }
        let mut v_tile = path.spacing_metres();
        if v_tile <= 0.0 {
            v_tile = DEFAULT_V_TILE;
        }
        let points: Vec<[f32; 3]> = path.points.iter().map(|p| p.position).collect();
        let ribbon = build_ribbon(&points, v_tile);
        if ribbon.odd_tail {
            report.odd_point_paths += 1;
        }
        report.skipped_quads += ribbon.skipped_quads;
        if ribbon.quads == 0 {
            report.degenerate_paths += 1;
            continue;
        }
        let take = ribbon.quads.min(quads_left);
        report.capped += ribbon.quads - take;
        quads_left -= take;
        let Some(material) = mats.get_decal(name) else {
            debug!(path = %path.name, "decal texture failed to decode; ribbon dropped");
            missing.insert(name.to_string());
            continue;
        };
        let trimmed = if take < ribbon.quads {
            DecalRibbon {
                indices: ribbon.indices[..take * 6].to_vec(),
                quads: take,
                ..ribbon
            } // positions/normals keep their harmless tail verts
        } else {
            ribbon
        };
        groups
            .entry(name.to_string())
            .or_insert_with(|| (RibbonBuilder::default(), material))
            .0
            .push(&trimmed);
        report.ribbons += 1;
        report.quads += take;
    }

    for (stem, (builder, material)) in groups {
        commands.spawn((
            CityEntity,
            owner,
            Mesh3d(meshes.add(builder.build())),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            Name::new(format!("{name_prefix}-{stem}")),
        ));
        report.entities += 1;
    }
    report.missing_textures = missing.len();
    info!(
        path = %logical,
        ribbons = report.ribbons,
        quads = report.quads,
        entities = report.entities,
        labels = report.label_paths,
        animated = report.animated_paths,
        props = report.prop_paths,
        unresolved = report.unresolved_paths,
        empty = report.empty_paths,
        degenerate = report.degenerate_paths,
        odd = report.odd_point_paths,
        unsupported = report.unsupported_kind_paths,
        skipped = report.skipped_quads,
        capped = report.capped,
        missing = report.missing_textures,
        issues = report.issues,
        "decal pathset stamped"
    );
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A straight 10 m ribbon, 2 m wide, running along +Z.
    fn straight() -> Vec<[f32; 3]> {
        vec![
            [0.0, 0.0, 0.0],  // A0
            [2.0, 0.0, 0.0],  // B0
            [0.0, 0.0, 10.0], // A1
            [2.0, 0.0, 10.0], // B1
        ]
    }

    #[test]
    fn ribbon_pairs_interleaved_edges_into_quads() {
        let r = build_ribbon(&straight(), 5.0);
        assert_eq!(r.quads, 1);
        assert_eq!(r.positions.len(), 4);
        assert_eq!(r.indices.len(), 6);
        // Edges: even points on u=0 at x=0, odd on u=1 at x=2, lifted.
        assert!((r.positions[0] - Vec3::new(0.0, DECAL_LIFT, 0.0)).length() < 1e-5);
        assert!((r.positions[1] - Vec3::new(2.0, DECAL_LIFT, 0.0)).length() < 1e-5);
        // u across, v arclength/tile: 10 m at 5 m tiles → v 0→2.
        assert_eq!(r.uvs[0], [0.0, 0.0]);
        assert_eq!(r.uvs[1], [1.0, 0.0]);
        assert_eq!(r.uvs[2], [0.0, 2.0]);
        assert_eq!(r.uvs[3], [1.0, 2.0]);
        // Flat strip faces +Y.
        for n in &r.normals {
            assert!((*n - Vec3::Y).length() < 1e-5, "{n:?}");
        }
    }

    #[test]
    fn ribbon_chains_sections_with_cumulative_v() {
        // Two 10 m quads, 4 m tile → v 0, 2.5, 5.
        let mut pts = straight();
        pts.extend_from_slice(&[[0.0, 0.0, 20.0], [2.0, 0.0, 20.0]]);
        let r = build_ribbon(&pts, 4.0);
        assert_eq!(r.quads, 2);
        assert_eq!(r.indices.len(), 12);
        let vs: Vec<f32> = r.uvs.iter().map(|uv| uv[1]).collect();
        assert_eq!(vs, vec![0.0, 0.0, 2.5, 2.5, 5.0, 5.0]);
        // Quad indices offset by vertex pairs.
        assert_eq!(&r.indices[6..], &[2, 3, 4, 3, 5, 4]);
    }

    #[test]
    fn ribbon_curved_strip_measures_centreline_v() {
        // A 90° turn: sections at (0,0)→(2,0) then (0,10)→(2,10)
        // sideways — centreline still 10 m.
        let pts = vec![
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [0.0, 0.0, 10.0],
            [-8.0, 0.0, 10.0], // edge B swings wide
        ];
        let r = build_ribbon(&pts, 5.0);
        assert_eq!(r.quads, 1);
        // Centreline: mid (1,0,0) → mid (-4,0,10) = sqrt(25+100) ≈ 11.18 m.
        let expect = (25.0f32 + 100.0).sqrt() / 5.0;
        assert!((r.uvs[2][1] - expect).abs() < 1e-5);
    }

    #[test]
    fn ribbon_under_four_points_stamps_nothing() {
        for pts in [
            vec![],
            vec![[0.0, 0.0, 0.0]],
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        ] {
            let r = build_ribbon(&pts, 5.0);
            assert_eq!(r.quads, 0);
            assert!(r.indices.is_empty());
        }
    }

    #[test]
    fn ribbon_drops_an_odd_trailing_point() {
        let mut pts = straight();
        pts.push([9.0, 9.0, 9.0]);
        let r = build_ribbon(&pts, 5.0);
        assert!(r.odd_tail);
        assert_eq!(r.quads, 1);
        assert_eq!(r.positions.len(), 4);
    }

    #[test]
    fn ribbon_skips_non_finite_and_degenerate_sections() {
        // Middle section non-finite: both adjacent quads die.
        let mut pts = straight();
        pts.extend_from_slice(&[
            [f32::NAN, 0.0, 20.0],
            [2.0, 0.0, 20.0],
            [0.0, 0.0, 30.0],
            [2.0, 0.0, 30.0],
        ]);
        let r = build_ribbon(&pts, 5.0);
        assert_eq!(r.quads, 1, "only the first quad survives");
        assert_eq!(r.skipped_quads, 2);
        // Zero-width section collapses the quad it borders.
        let pts = vec![
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [1.0, 0.0, 10.0],
            [1.0, 0.0, 10.0], // A1 == B1: zero width
        ];
        let r = build_ribbon(&pts, 5.0);
        // Quad (A0,B0)→(A1,B1): normal computable (along × across is
        // nonzero) — the quad is a triangle, not degenerate.
        assert_eq!(r.quads, 1);
        // Fully collapsed sections make a zero-area quad.
        let pts = vec![
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
        ];
        let r = build_ribbon(&pts, 5.0);
        assert_eq!(r.quads, 0);
        assert_eq!(r.skipped_quads, 1);
    }

    #[test]
    fn ribbon_is_deterministic() {
        let a = build_ribbon(&straight(), 5.0);
        let b = build_ribbon(&straight(), 5.0);
        assert_eq!(a.positions, b.positions);
        assert_eq!(a.uvs, b.uvs);
        assert_eq!(a.indices, b.indices);
    }
}
