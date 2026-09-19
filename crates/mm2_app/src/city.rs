//! Import of MM2 city content (PSDL geometry, INST placements, PKG props,
//! TEX textures) through the VFS into Bevy meshes/materials.
//!
//! Coordinate convention: MM2 data is treated as left-handed (Direct3D-era).
//! Conversion to Bevy's right-handed frame mirrors Z (`x, y, -z`) everywhere
//! positions, normals and basis vectors appear, via [`v3`]. Ground geometry
//! is authored clockwise in the (x, z) plane (verified on retail London:
//! ~98% of fans), so emitters using [`MeshBuilder::tri`] push the reversed
//! index order to get +Y-facing front faces after the mirror. Wall
//! attributes (facades, slivers, bounds) are emitted with their authored
//! left→right bottom edge; measured against retail London this faces away
//! from the block interior for the dominant authored order.
//!
//! Attribute payloads follow the community-verified layouts (see
//! `docs/research/psdl.md`): counted forms store an explicit leading word,
//! inline forms use the subtype as the count. Height and vertex references
//! are checked; a bad reference fails the affected attribute with context in
//! [`CityReport`] — it does not silently re-wire neighbouring vertices.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use avian3d::prelude::*;
use bevy::{
    asset::RenderAssetUsages,
    image::{
        CompressedImageFormats, ImageAddressMode, ImageSampler, ImageSamplerDescriptor, ImageType,
    },
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use mm2_assets::{Resolved, Vfs};
use mm2_formats::{
    inst::{self, InstPlacement},
    pkg::{Pkg, PkgStrip},
    psdl::{AttributeType, Psdl, RoomAttribute},
    tex::TexFile,
};
use mm2_game::CityEntity;
use tracing::{debug, info, warn};

/// Whether to mirror Z when converting MM2 coordinates to Bevy space.
const MIRROR_Z: bool = true;

/// World scale (metres) per texture repeat for planar-mapped city surfaces.
/// The PSDL format does not store UVs for most ground attributes; this is a
/// documented approximation.
const PLANAR_UV_SCALE: f32 = 8.0;

/// Sidewalks sit this far above road vertices (per `Room_attributes`:
/// "the road surface vertices are expected to be located 0.15 units below
/// the sidewalk vertices").
const SIDEWALK_LIFT: f32 = 0.15;

/// Height used for invisible (type-0) divider collision bounds. The bound
/// height is not stored in the attribute; this documented approximation is
/// enough to keep the car out of the median.
const INVISIBLE_DIVIDER_HEIGHT: f32 = 0.8;

/// Clearance above the road surface for the player spawn point.
const SPAWN_CLEARANCE: f32 = 1.5;

/// MM2 texture lookup order for a logical stem: lossless formats first so a
/// mod can ship a `.png` next to the original `.tex`.
const TEXTURE_EXTS: &[&str] = &["png", "ktx2", "tga", "tex"];

#[inline]
fn v3(p: [f32; 3]) -> Vec3 {
    if MIRROR_Z {
        Vec3::new(p[0], p[1], -p[2])
    } else {
        Vec3::new(p[0], p[1], p[2])
    }
}

// ---------------------------------------------------------------------------
// Mesh accumulation
// ---------------------------------------------------------------------------

/// Accumulates triangles for one material/texture group. `normals` is only
/// filled when the source data provides authored normals (PKG strips); when
/// its length matches `positions` the authored normals are used verbatim.
#[derive(Default)]
struct MeshBuilder {
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    normals: Vec<[f32; 3]>,
    indices: Vec<u32>,
}

impl MeshBuilder {
    fn vert(&mut self, p: Vec3, uv: [f32; 2]) -> u32 {
        let i = self.positions.len() as u32;
        self.positions.push(p.to_array());
        self.uvs.push(uv);
        i
    }

    fn vert_n(&mut self, p: Vec3, uv: [f32; 2], n: Vec3) -> u32 {
        let i = self.vert(p, uv);
        self.normals.push(n.to_array());
        i
    }

    /// Push a triangle with reversed winding — positions are already
    /// mirrored and ground attributes are authored clockwise, so reversing
    /// yields +Y-facing front faces.
    fn tri(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend_from_slice(&[a, c, b]);
    }

    /// Push a triangle with the indices exactly as given — for emitters
    /// whose winding is already correct in Bevy space (walls, authored
    /// quads).
    fn tri_keep(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend_from_slice(&[a, b, c]);
    }

    /// Planar UVs for ground-like surfaces.
    fn planar_uv(p: Vec3) -> [f32; 2] {
        [p.x / PLANAR_UV_SCALE, p.z / PLANAR_UV_SCALE]
    }

    /// Triangle fan from world positions with planar UVs. `pts[0]` is the
    /// pivot; the rest surround it in authored order.
    fn fan(&mut self, pts: &[Vec3]) {
        if pts.len() < 3 {
            return;
        }
        let base: Vec<u32> = pts
            .iter()
            .map(|&p| self.vert(p, Self::planar_uv(p)))
            .collect();
        for i in 1..base.len() - 1 {
            self.tri(base[0], base[i], base[i + 1]);
        }
    }

    /// Flat strip between left/right vertex chains. Verified to emit
    /// +Y-facing quads for ~97% of retail London road sections.
    fn strip(&mut self, left: &[Vec3], right: &[Vec3]) {
        for i in 0..left.len().saturating_sub(1) {
            let l0 = self.vert(left[i], Self::planar_uv(left[i]));
            let l1 = self.vert(left[i + 1], Self::planar_uv(left[i + 1]));
            let r0 = self.vert(right[i], Self::planar_uv(right[i]));
            let r1 = self.vert(right[i + 1], Self::planar_uv(right[i + 1]));
            self.tri(l0, r0, l1);
            self.tri(l1, r0, r1);
        }
    }

    /// Strip for chains that may bend around corners (sidewalk strips):
    /// each quad's winding is chosen so its normal points up.
    fn strip_up(&mut self, left: &[Vec3], right: &[Vec3]) {
        for i in 0..left.len().saturating_sub(1) {
            let down = (left[i + 1] - left[i]).cross(right[i] - left[i]).y < 0.0;
            let l0 = self.vert(left[i], Self::planar_uv(left[i]));
            let l1 = self.vert(left[i + 1], Self::planar_uv(left[i + 1]));
            let r0 = self.vert(right[i], Self::planar_uv(right[i]));
            let r1 = self.vert(right[i + 1], Self::planar_uv(right[i + 1]));
            if down {
                self.tri(r0, l0, r1);
                self.tri(r1, l0, l1);
            } else {
                self.tri(l0, r0, l1);
                self.tri(l1, r0, r1);
            }
        }
    }

    /// Arbitrary quad emitted so its front normal points toward `facing`
    /// (dot product > 0). Used for divider parts and other authored quads
    /// whose orientation varies by part.
    fn quad_facing(
        &mut self,
        a: Vec3,
        b: Vec3,
        c: Vec3,
        d: Vec3,
        uvs: [[f32; 2]; 4],
        facing: Vec3,
    ) {
        let n = (b - a).cross(c - a);
        let ia = self.vert(a, uvs[0]);
        let ib = self.vert(b, uvs[1]);
        let ic = self.vert(c, uvs[2]);
        let id = self.vert(d, uvs[3]);
        if n.dot(facing) >= 0.0 {
            self.tri_keep(ia, ib, ic);
            self.tri_keep(ia, ic, id);
        } else {
            self.tri_keep(ia, ic, ib);
            self.tri_keep(ia, id, ic);
        }
    }

    /// Vertical wall quad with literal winding — the authored left→right
    /// bottom edge produces the outward face in Bevy space (verified
    /// against the dominant authored order on retail London). `v` runs from
    /// `v_rep` at the bottom edge to 0 at the top so textures sit upright
    /// (D3D v=0 at the image top).
    fn wall_quad(&mut self, l: Vec3, r: Vec3, bottom: f32, top: f32, u_rep: f32, v_rep: f32) {
        let bl = self.vert(Vec3::new(l.x, bottom, l.z), [0.0, v_rep]);
        let br = self.vert(Vec3::new(r.x, bottom, r.z), [u_rep, v_rep]);
        let tr = self.vert(Vec3::new(r.x, top, r.z), [u_rep, 0.0]);
        let tl = self.vert(Vec3::new(l.x, top, l.z), [0.0, 0.0]);
        self.tri_keep(bl, br, tr);
        self.tri_keep(bl, tr, tl);
    }

    /// Slanted-bottom wall (sliver): the bottom edge follows the authored
    /// vertex heights, the top is horizontal at `top`. `v_scale` converts
    /// height above the lowest bottom corner to texture v.
    fn sliver_quad(&mut self, l: Vec3, r: Vec3, top: f32, v_scale: f32) {
        let min = l.y.min(r.y);
        let bl = self.vert(l, [0.0, (l.y - min) * v_scale]);
        let br = self.vert(r, [1.0, (r.y - min) * v_scale]);
        let tr = self.vert(Vec3::new(r.x, top, r.z), [1.0, (top - min) * v_scale]);
        let tl = self.vert(Vec3::new(l.x, top, l.z), [0.0, (top - min) * v_scale]);
        self.tri_keep(bl, br, tr);
        self.tri_keep(bl, tr, tl);
    }

    fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    fn build(self) -> Mesh {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        if self.normals.len() == self.positions.len() && !self.normals.is_empty() {
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals);
        }
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs);
        mesh.insert_indices(Indices::U32(self.indices));
        if mesh.attribute(Mesh::ATTRIBUTE_NORMAL).is_none() {
            mesh.compute_normals();
        }
        mesh
    }
}

/// Collision triangles accumulated per room (positions + indices; winding
/// is irrelevant to the physics backend).
#[derive(Default)]
struct ColliderBuilder {
    positions: Vec<Vec3>,
    tris: Vec<[u32; 3]>,
}

impl ColliderBuilder {
    fn tri(&mut self, a: Vec3, b: Vec3, c: Vec3) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(&[a, b, c]);
        self.tris.push([base, base + 1, base + 2]);
    }

    fn fan(&mut self, pts: &[Vec3]) {
        for i in 1..pts.len().saturating_sub(1) {
            self.tri(pts[0], pts[i], pts[i + 1]);
        }
    }

    fn strip(&mut self, left: &[Vec3], right: &[Vec3]) {
        for i in 0..left.len().saturating_sub(1) {
            self.tri(left[i], right[i], left[i + 1]);
            self.tri(left[i + 1], right[i], right[i + 1]);
        }
    }

    fn quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3) {
        self.tri(a, b, c);
        self.tri(a, c, d);
    }
}

// ---------------------------------------------------------------------------
// Attribute decoding (typed semantics over the raw `u16` payloads)
// ---------------------------------------------------------------------------

/// A decode failure for one attribute: reported with context, the
/// attribute is skipped and neighbouring geometry is never re-wired.
#[derive(Debug)]
enum AttrError {
    BadVertexRef(u16),
    BadHeightRef(u16),
    Malformed(&'static str),
}

impl std::fmt::Display for AttrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadVertexRef(i) => write!(f, "vertex ref {i} out of range"),
            Self::BadHeightRef(i) => write!(f, "height ref {i} out of range"),
            Self::Malformed(k) => write!(f, "malformed {k} payload"),
        }
    }
}

/// Texture state while walking a room's attribute stream.
#[derive(Clone, Copy)]
enum TexState {
    /// No texture reference seen yet — attributes get the fallback material.
    Unset,
    /// Texture index into `Psdl::textures`.
    Index(usize),
    /// TextureRef 0: rendering AND collision suppressed for following attrs.
    Suppressed,
}

/// Decode a `TextureRef` attribute: `n = data + 256 * subtype - 1`;
/// raw 0 suppresses rendering and collision.
fn decode_texture_ref(attr: &RoomAttribute) -> TexState {
    let raw = attr.data.first().copied().unwrap_or(0) as usize + (attr.subtype as usize) * 256;
    if raw == 0 {
        TexState::Suppressed
    } else {
        TexState::Index(raw - 1)
    }
}

/// Strip the leading count word of a `subtype == 0` attribute and validate
/// the payload length against `per_item` words per element.
fn counted_refs<'a>(
    attr: &'a RoomAttribute,
    per_item: usize,
    kind: &'static str,
) -> Result<&'a [u16], AttrError> {
    if attr.subtype == 0 {
        let (&n, rest) = attr.data.split_first().ok_or(AttrError::Malformed(kind))?;
        if rest.len() != n as usize * per_item {
            return Err(AttrError::Malformed(kind));
        }
        Ok(rest)
    } else {
        if attr.data.len() != attr.subtype as usize * per_item {
            return Err(AttrError::Malformed(kind));
        }
        Ok(&attr.data)
    }
}

fn vertex(i: u16, verts: &[Vec3]) -> Result<Vec3, AttrError> {
    verts
        .get(i as usize)
        .copied()
        .ok_or(AttrError::BadVertexRef(i))
}

fn height(i: u16, heights: &[f32]) -> Result<f32, AttrError> {
    heights
        .get(i as usize)
        .copied()
        .ok_or(AttrError::BadHeightRef(i))
}

// ---------------------------------------------------------------------------
// Import report
// ---------------------------------------------------------------------------

/// What happened while turning a PSDL into geometry — the distinction
/// between *emitted*, *suppressed*, *approximated*, *unsupported* and
/// *rejected* content lives here, not in the log stream.
#[derive(Debug, Default)]
pub struct CityReport {
    /// Rooms in the file.
    pub rooms: usize,
    /// Attribute records decoded.
    pub attributes: usize,
    /// Attributes that produced geometry or collision.
    pub emitted: usize,
    /// Attributes skipped because a TextureRef suppressed them.
    pub suppressed: usize,
    /// Attributes skipped due to a bad vertex/height reference or a
    /// malformed payload (count did not match the word count).
    pub rejected: usize,
    /// Attributes of a known type we deliberately do not handle yet, and
    /// other categorized gaps.
    pub unsupported: BTreeMap<String, usize>,
    /// Words left unparsed by the format layer (preserved, not lost).
    pub unparsed_words: usize,
    /// Attributes whose payloads were approximated rather than fully
    /// interpreted (e.g. elevated/wedged dividers).
    pub approximated: usize,
    /// Rooms that produced a collision mesh.
    pub collider_rooms: usize,
    /// Render mesh groups produced (room × material).
    pub mesh_groups: usize,
    /// PSDL texture names that did not resolve in the VFS.
    pub missing_textures: BTreeSet<String>,
    /// INST placements spawned.
    pub props_spawned: usize,
    /// INST placements skipped because the PKG was missing/undecodable.
    pub props_failed: usize,
}

impl std::fmt::Display for CityReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} rooms, {} attrs: {} emitted, {} suppressed, {} rejected, {} approximated, {} unparsed words",
            self.rooms,
            self.attributes,
            self.emitted,
            self.suppressed,
            self.rejected,
            self.approximated,
            self.unparsed_words,
        )?;
        for (k, v) in &self.unsupported {
            write!(f, ", {k}×{v}")?;
        }
        write!(
            f,
            "; {} mesh groups, {} collider rooms, {} props ({} failed), {} missing textures",
            self.mesh_groups,
            self.collider_rooms,
            self.props_spawned,
            self.props_failed,
            self.missing_textures.len(),
        )
    }
}

// ---------------------------------------------------------------------------
// Pure import: PSDL → geometry + collision + spawn + report (no Bevy ECS)
// ---------------------------------------------------------------------------

/// One renderable mesh group: a room's triangles for one texture.
pub struct MeshGroup {
    /// Room index in `Psdl::rooms` (room id = index + 1).
    pub room: usize,
    /// Texture index into `Psdl::textures`, or `None` for the fallback.
    pub texture: Option<usize>,
    /// Vertex positions (Bevy space).
    pub positions: Vec<[f32; 3]>,
    /// UVs.
    pub uvs: Vec<[f32; 2]>,
    /// Triangle indices.
    pub indices: Vec<u32>,
}

/// Static collision triangles for one room.
pub struct RoomCollider {
    /// Room index.
    pub room: usize,
    /// Vertices (Bevy space).
    pub positions: Vec<Vec3>,
    /// Triangles.
    pub tris: Vec<[u32; 3]>,
}

/// The Bevy-free result of importing a parsed PSDL.
pub struct CityImport {
    /// Render groups (per room × texture).
    pub meshes: Vec<MeshGroup>,
    /// Collision meshes (per room).
    pub colliders: Vec<RoomCollider>,
    /// Suggested player spawn: on road geometry near the city centre.
    pub spawn: Vec3,
    /// What was emitted, approximated, skipped or unsupported.
    pub report: CityReport,
}

/// Emit all room attributes into mesh groups and collider meshes.
pub fn emit_psdl(psdl: &Psdl) -> CityImport {
    let verts: Vec<Vec3> = psdl.vertices.iter().map(|&p| v3(p)).collect();
    let heights = &psdl.heights;
    let mut report = CityReport {
        rooms: psdl.rooms.len(),
        ..Default::default()
    };

    let mut meshes = Vec::new();
    let mut colliders = Vec::new();
    // Road-attribute rooms → (surface centroid, highest road y).
    let mut road_surfaces: Vec<(Vec3, f32)> = Vec::new();

    // Texture state persists across rooms (rooms normally open with their
    // own refs); attributes emitted before the first ref get the fallback
    // material and are counted.
    let mut tex = TexState::Unset;
    let mut unset_emits = 0usize;

    for (room_idx, room) in psdl.rooms.iter().enumerate() {
        report.unparsed_words += room.unparsed_attributes.len();
        let mut groups: BTreeMap<i64, MeshBuilder> = BTreeMap::new();
        let mut collider = ColliderBuilder::default();
        let mut road_acc = Vec3::ZERO;
        let mut road_n = 0usize;
        let mut road_max_y = f32::MIN;

        for attr in &room.attributes {
            report.attributes += 1;
            if attr.kind == AttributeType::TextureRef {
                tex = decode_texture_ref(attr);
                continue;
            }
            let tex_key: i64 = match tex {
                TexState::Suppressed => {
                    report.suppressed += 1;
                    continue;
                }
                TexState::Unset => {
                    unset_emits += 1;
                    -1
                }
                TexState::Index(i) => i as i64,
            };
            let mut ctx = EmitCtx {
                verts: &verts,
                heights,
                groups: &mut groups,
                collider: &mut collider,
                tex_key,
                road_acc: &mut road_acc,
                road_n: &mut road_n,
                road_max_y: &mut road_max_y,
                report: &mut report,
            };
            match emit_attribute(&mut ctx, attr) {
                Ok(Outcome::Emitted) => report.emitted += 1,
                Ok(Outcome::Unsupported(name)) => {
                    *report.unsupported.entry(name).or_insert(0) += 1;
                }
                Err(e) => {
                    report.rejected += 1;
                    debug!(
                        room = room_idx + 1,
                        kind = ?attr.kind,
                        error = %e,
                        "skipped attribute"
                    );
                }
            }
        }

        if road_n > 0 {
            road_surfaces.push((road_acc / road_n as f32, road_max_y));
        }
        for (key, builder) in groups {
            if builder.is_empty() {
                continue;
            }
            meshes.push(MeshGroup {
                room: room_idx,
                texture: if key < 0 { None } else { Some(key as usize) },
                positions: builder.positions,
                uvs: builder.uvs,
                indices: builder.indices,
            });
        }
        if !collider.tris.is_empty() {
            colliders.push(RoomCollider {
                room: room_idx,
                positions: collider.positions,
                tris: collider.tris,
            });
        }
    }

    report.mesh_groups = meshes.len();
    report.collider_rooms = colliders.len();
    if unset_emits > 0 {
        report
            .unsupported
            .insert("emitted-before-first-texture-ref".into(), unset_emits);
    }

    let spawn = choose_spawn(psdl, &road_surfaces);
    CityImport {
        meshes,
        colliders,
        spawn,
        report,
    }
}

/// Pick a spawn on verified road geometry: the road-attribute room nearest
/// the city centre, a vehicle height above its highest road vertex.
fn choose_spawn(psdl: &Psdl, roads: &[(Vec3, f32)]) -> Vec3 {
    let center = v3(psdl.bounds_center);
    let best = roads.iter().min_by(|a, b| {
        let da = (a.0.xz() - center.xz()).length_squared();
        let db = (b.0.xz() - center.xz()).length_squared();
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    });
    match best {
        Some((centroid, max_y)) => Vec3::new(centroid.x, max_y + SPAWN_CLEARANCE, centroid.z),
        // No road attributes at all: above the bounds centre.
        None => Vec3::new(center.x, center.y + 10.0, center.z),
    }
}

struct EmitCtx<'a> {
    verts: &'a [Vec3],
    heights: &'a [f32],
    groups: &'a mut BTreeMap<i64, MeshBuilder>,
    collider: &'a mut ColliderBuilder,
    tex_key: i64,
    road_acc: &'a mut Vec3,
    road_n: &'a mut usize,
    road_max_y: &'a mut f32,
    report: &'a mut CityReport,
}

impl EmitCtx<'_> {
    /// The mesh group for a texture relative to the current ref (0 = n,
    /// 1 = n+1, ...). Negative/out-of-range resolves clamp to the fallback
    /// group (−1).
    fn builder(&mut self, rel: i64) -> &mut MeshBuilder {
        let key = if self.tex_key < 0 {
            -1
        } else {
            self.tex_key + rel
        };
        self.groups.entry(key.max(-1)).or_default()
    }

    /// The mesh group for an absolute texture index (−1 = fallback).
    fn builder_at(&mut self, abs: i64) -> &mut MeshBuilder {
        self.groups.entry(abs.max(-1)).or_default()
    }

    fn note_road(&mut self, pts: &[Vec3]) {
        for p in pts {
            *self.road_acc += *p;
            *self.road_n += 1;
            *self.road_max_y = self.road_max_y.max(p.y);
        }
    }

    fn resolve(&self, refs: &[u16]) -> Result<Vec<Vec3>, AttrError> {
        refs.iter().map(|&i| vertex(i, self.verts)).collect()
    }
}

/// What became of one attribute.
enum Outcome {
    /// Geometry or collision was produced.
    Emitted,
    /// The attribute was understood but is intentionally not handled yet.
    Unsupported(String),
}

/// Emit one attribute. Returns `Err` when a referenced vertex/height is out
/// of range or the payload is malformed — the attribute is skipped whole.
fn emit_attribute(ctx: &mut EmitCtx<'_>, attr: &RoomAttribute) -> Result<Outcome, AttrError> {
    Ok(match attr.kind {
        AttributeType::Fan | AttributeType::RoadFan => {
            // subtype = nTriangles, then nTriangles + 2 vertex refs
            // (pivot first, ring after); subtype 0 → leading count word
            // = nTriangles.
            let refs: &[u16] = if attr.subtype == 0 {
                let (&n, rest) = attr.data.split_first().ok_or(AttrError::Malformed("fan"))?;
                if rest.len() != n as usize + 2 {
                    return Err(AttrError::Malformed("fan"));
                }
                rest
            } else {
                if attr.data.len() != attr.subtype as usize + 2 {
                    return Err(AttrError::Malformed("fan"));
                }
                &attr.data
            };
            let pts = ctx.resolve(refs)?;
            if matches!(attr.kind, AttributeType::RoadFan) {
                ctx.note_road(&pts);
            }
            ctx.builder(0).fan(&pts);
            ctx.collider.fan(&pts);
            Outcome::Emitted
        }
        AttributeType::RoofFan => {
            // [heightRef, v0..v_n]: subtype = nVertices - 1 (0 → leading
            // count = nVertices - 1, then heightRef, then nVertices refs).
            let data: &[u16] = if attr.subtype == 0 {
                let (&n, rest) = attr
                    .data
                    .split_first()
                    .ok_or(AttrError::Malformed("rooffan"))?;
                if rest.len() != n as usize + 2 {
                    return Err(AttrError::Malformed("rooffan"));
                }
                rest
            } else {
                if attr.data.len() != attr.subtype as usize + 2 {
                    return Err(AttrError::Malformed("rooffan"));
                }
                &attr.data
            };
            let h = height(data[0], ctx.heights)?;
            let pts: Vec<Vec3> = ctx
                .resolve(&data[1..])?
                .into_iter()
                .map(|p| Vec3::new(p.x, h, p.z))
                .collect();
            ctx.builder(0).fan(&pts);
            ctx.collider.fan(&pts);
            Outcome::Emitted
        }
        AttributeType::RoadWithSidewalks => {
            // Cross-sections of 4: [sw_l, road_l, road_r, sw_r]. Textures:
            // n = road, n+1 = sidewalks (n+2 = whole-road low-LOD, unused —
            // we render the high LOD only).
            let refs = counted_refs(attr, 4, "road")?;
            let mut sw_l = Vec::new();
            let mut rl = Vec::new();
            let mut rr = Vec::new();
            let mut sw_r = Vec::new();
            for s in refs.chunks_exact(4) {
                sw_l.push(vertex(s[0], ctx.verts)?);
                rl.push(vertex(s[1], ctx.verts)?);
                rr.push(vertex(s[2], ctx.verts)?);
                sw_r.push(vertex(s[3], ctx.verts)?);
            }
            ctx.builder(0).strip(&rl, &rr);
            ctx.collider.strip(&rl, &rr);
            ctx.note_road(&rl);
            ctx.note_road(&rr);
            emit_sidewalk(ctx, &sw_l, &rl, true);
            emit_sidewalk(ctx, &sw_r, &rr, false);
            Outcome::Emitted
        }
        AttributeType::SidewalkStrip => {
            let refs = counted_refs(attr, 2, "sidewalk")?;
            // End piece: the first two refs both 0 or both 1 mark a
            // triangular cap; refs 3–4 are the bottom verts, a point
            // 0.15 above ref 3 forms the apex.
            if refs.len() >= 4 && refs[0] == refs[1] && (refs[0] == 0 || refs[0] == 1) {
                let a = vertex(refs[2], ctx.verts)?;
                let b = vertex(refs[3], ctx.verts)?;
                let apex = a + Vec3::Y * SIDEWALK_LIFT;
                ctx.builder(0).fan(&[b, a, apex]);
                ctx.collider.tri(a, b, apex);
                return Ok(Outcome::Emitted);
            }
            // Pairs (outer, inner): the top runs outer → inner+0.15, plus
            // the vertical curb face on the inner edge. Strips may bend
            // around corners, so winding is fixed per quad.
            let mut outer = Vec::new();
            let mut inner = Vec::new();
            for s in refs.chunks_exact(2) {
                outer.push(vertex(s[0], ctx.verts)?);
                inner.push(vertex(s[1], ctx.verts)?);
            }
            let lifted: Vec<Vec3> = inner.iter().map(|v| *v + Vec3::Y * SIDEWALK_LIFT).collect();
            ctx.builder(0).strip_up(&outer, &lifted);
            ctx.builder(0).strip_up(&inner, &lifted);
            ctx.collider.strip(&outer, &lifted);
            ctx.collider.strip(&inner, &lifted);
            ctx.note_road(&lifted);
            Outcome::Emitted
        }
        AttributeType::RoadNoSidewalks => {
            // Walkway: plain two-chain strip, texture n.
            let refs = counted_refs(attr, 2, "walkway")?;
            let mut l = Vec::new();
            let mut r = Vec::new();
            for s in refs.chunks_exact(2) {
                l.push(vertex(s[0], ctx.verts)?);
                r.push(vertex(s[1], ctx.verts)?);
            }
            ctx.builder(0).strip(&l, &r);
            ctx.collider.strip(&l, &r);
            ctx.note_road(&l);
            ctx.note_road(&r);
            Outcome::Emitted
        }
        AttributeType::DividedRoad => {
            // The packed word: low byte = flags<<3 | dividerType, high
            // byte = divider texture index + 1. Inline form:
            // [packed, value, subtype×6 refs]; counted form:
            // [count, packed, value, count×6 refs].
            let (packed, value, refs): (u16, u16, &[u16]) = if attr.subtype == 0 {
                if attr.data.len() < 3 {
                    return Err(AttrError::Malformed("divroad"));
                }
                let c = attr.data[0] as usize;
                let r = &attr.data[3..];
                if r.len() != c * 6 {
                    return Err(AttrError::Malformed("divroad"));
                }
                (attr.data[1], attr.data[2], r)
            } else {
                if attr.data.len() < 2 || attr.data[2..].len() != attr.subtype as usize * 6 {
                    return Err(AttrError::Malformed("divroad"));
                }
                (attr.data[0], attr.data[1], &attr.data[2..])
            };
            let div_type = (packed & 0x7) as u8;
            let div_tex = ((packed >> 8) & 0xff) as i64 - 1;
            let sections: Vec<[u16; 6]> = refs
                .chunks_exact(6)
                .map(|c| [c[0], c[1], c[2], c[3], c[4], c[5]])
                .collect();
            let mut sw_l = Vec::new();
            let mut rl_out = Vec::new();
            let mut rl_in = Vec::new();
            let mut rr_in = Vec::new();
            let mut rr_out = Vec::new();
            let mut sw_r = Vec::new();
            for s in &sections {
                sw_l.push(vertex(s[0], ctx.verts)?);
                rl_out.push(vertex(s[1], ctx.verts)?);
                rl_in.push(vertex(s[2], ctx.verts)?);
                rr_in.push(vertex(s[3], ctx.verts)?);
                rr_out.push(vertex(s[4], ctx.verts)?);
                sw_r.push(vertex(s[5], ctx.verts)?);
            }
            // Two road surfaces: outer→inner on each side (texture n).
            ctx.builder(0).strip(&rl_out, &rl_in);
            ctx.builder(0).strip(&rr_in, &rr_out);
            ctx.collider.strip(&rl_out, &rl_in);
            ctx.collider.strip(&rr_in, &rr_out);
            ctx.note_road(&rl_out);
            ctx.note_road(&rl_in);
            ctx.note_road(&rr_in);
            ctx.note_road(&rr_out);
            emit_sidewalk(ctx, &sw_l, &rl_out, true);
            emit_sidewalk(ctx, &sw_r, &rr_out, false);
            emit_divider(ctx, div_type, div_tex, value, &rl_in, &rr_in);
            Outcome::Emitted
        }
        AttributeType::Crosswalk => {
            // Four corner refs → one textured quad.
            if attr.data.len() != 4 {
                return Err(AttrError::Malformed("crosswalk"));
            }
            let pts = ctx.resolve(&attr.data[..4])?;
            ctx.builder(0).fan(&pts);
            ctx.collider.fan(&pts);
            Outcome::Emitted
        }
        AttributeType::Sliver => {
            // [top, textureScale, left, right] — top and textureScale are
            // height-list refs (the scale values are small, e.g. 0.25).
            if attr.data.len() != 4 {
                return Err(AttrError::Malformed("sliver"));
            }
            let top = height(attr.data[0], ctx.heights)?;
            let scale = height(attr.data[1], ctx.heights)?;
            let l = vertex(attr.data[2], ctx.verts)?;
            let r = vertex(attr.data[3], ctx.verts)?;
            ctx.builder(0).sliver_quad(l, r, top, scale);
            Outcome::Emitted
        }
        AttributeType::FacadeBound => {
            // [angle, top, left, right]: collision-only quad; bottom edge
            // follows the authored verts (may be slanted), top horizontal.
            if attr.data.len() != 4 {
                return Err(AttrError::Malformed("fbound"));
            }
            let top = height(attr.data[1], ctx.heights)?;
            let l = vertex(attr.data[2], ctx.verts)?;
            let r = vertex(attr.data[3], ctx.verts)?;
            ctx.collider
                .quad(l, r, Vec3::new(r.x, top, r.z), Vec3::new(l.x, top, l.z));
            Outcome::Emitted
        }
        AttributeType::Facade => {
            // [bottom, top, uRepeat, vRepeat, left, right]: bottom and top
            // are height-list refs (absolute Y); the rectangle is
            // Y-aligned, texture repeated u×v times, v = 0 at the top.
            if attr.data.len() != 6 {
                return Err(AttrError::Malformed("facade"));
            }
            let hb = height(attr.data[0], ctx.heights)?;
            let ht = height(attr.data[1], ctx.heights)?;
            let l = vertex(attr.data[4], ctx.verts)?;
            let r = vertex(attr.data[5], ctx.verts)?;
            ctx.builder(0).wall_quad(
                l,
                r,
                hb,
                ht,
                attr.data[2].max(1) as f32,
                attr.data[3].max(1) as f32,
            );
            Outcome::Emitted
        }
        // Railing/tunnel parameters: layout partially known, no
        // geometry emitted yet.
        AttributeType::Tunnel => Outcome::Unsupported("tunnel".into()),
        AttributeType::TextureRef => Outcome::Emitted, // handled by the caller
        AttributeType::Unknown(raw) => {
            Outcome::Unsupported(format!("unknown-attribute-type-{raw:#04x}"))
        }
    })
}

/// Sidewalk top + vertical curb face on one side of a road. `outer` is the
/// outer sidewalk edge (authored height), `road` the adjacent road edge;
/// the inner sidewalk edge sits SIDEWALK_LIFT above the road vertex.
/// `left_side` selects the chain order that faces the road.
fn emit_sidewalk(ctx: &mut EmitCtx<'_>, outer: &[Vec3], road: &[Vec3], left_side: bool) {
    let inner: Vec<Vec3> = road.iter().map(|v| *v + Vec3::Y * SIDEWALK_LIFT).collect();
    if left_side {
        ctx.builder(1).strip(outer, &inner); // top
        ctx.builder(1).strip(road, &inner); // curb face, toward the road
    } else {
        ctx.builder(1).strip(&inner, outer);
        ctx.builder(1).strip(&inner, road);
    }
    ctx.collider.strip(outer, &inner);
    ctx.collider.strip(road, &inner);
    ctx.note_road(&inner);
}

/// Divider geometry for a divided road. `div_tex` is the resolved texture
/// index (−1 when the packed byte was 0). Types per the format docs:
/// 0 invisible (collision bound only), 1 flat, 2 elevated, 3 wedged.
/// Elevated/wedged geometry is approximated as a raised median strip and
/// counted in the report; end caps (flags bits 7–8) are not emitted.
fn emit_divider(
    ctx: &mut EmitCtx<'_>,
    div_type: u8,
    div_tex: i64,
    value: u16,
    rl_in: &[Vec3],
    rr_in: &[Vec3],
) {
    let n = rl_in.len().min(rr_in.len());
    if n < 2 {
        return;
    }
    // Divider textures are relative to the packed divider texture index
    // (stored +1); −1 = no divider texture → fallback material.
    let dt = |rel: i64| if div_tex < 0 { -1 } else { div_tex + rel };
    match div_type {
        0 => {
            // Invisible divider: collision bound only (approximated).
            for i in 0..n - 1 {
                let mid0 = (rl_in[i] + rr_in[i]) * 0.5;
                let mid1 = (rl_in[i + 1] + rr_in[i + 1]) * 0.5;
                ctx.collider.quad(
                    mid0,
                    mid1,
                    mid1 + Vec3::Y * INVISIBLE_DIVIDER_HEIGHT,
                    mid0 + Vec3::Y * INVISIBLE_DIVIDER_HEIGHT,
                );
            }
            ctx.report.approximated += 1;
        }
        1 => {
            // Flat divider at road height, divider texture n+1 repeated
            // `value` times across.
            for i in 0..n - 1 {
                ctx.builder_at(dt(1)).quad_facing(
                    rl_in[i],
                    rr_in[i],
                    rr_in[i + 1],
                    rl_in[i + 1],
                    [
                        [0.0, 0.0],
                        [value.max(1) as f32, 0.0],
                        [value.max(1) as f32, 1.0],
                        [0.0, 1.0],
                    ],
                    Vec3::Y,
                );
            }
            ctx.collider.strip(rl_in, rr_in);
        }
        _ => {
            // 2 elevated / 3 wedged, approximated as a raised median strip:
            // vertical sides + flat top. Elevated uses `value` as height;
            // wedge is documented as 1 m. (Real wedges slope 0.5 m inward;
            // the approximation is counted in the report.)
            let h = if div_type == 3 {
                1.0
            } else {
                (value as f32).max(0.05)
            };
            let top_l: Vec<Vec3> = rl_in.iter().map(|v| *v + Vec3::Y * h).collect();
            let top_r: Vec<Vec3> = rr_in.iter().map(|v| *v + Vec3::Y * h).collect();
            let side_tex = if div_type == 2 { dt(0) } else { dt(1) };
            for i in 0..n - 1 {
                let quad_uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
                let mid0 = (rl_in[i] + rr_in[i]) * 0.5;
                // Left side faces the left roadway.
                ctx.builder_at(side_tex).quad_facing(
                    rl_in[i],
                    rl_in[i + 1],
                    top_l[i + 1],
                    top_l[i],
                    quad_uvs,
                    (rl_in[i] - mid0).normalize_or_zero(),
                );
                // Right side faces the right roadway.
                ctx.builder_at(side_tex).quad_facing(
                    rr_in[i],
                    rr_in[i + 1],
                    top_r[i + 1],
                    top_r[i],
                    quad_uvs,
                    (rr_in[i] - mid0).normalize_or_zero(),
                );
                // Top strip.
                ctx.builder_at(dt(2)).quad_facing(
                    top_l[i],
                    top_r[i],
                    top_r[i + 1],
                    top_l[i + 1],
                    quad_uvs,
                    Vec3::Y,
                );
            }
            ctx.collider.strip(&top_l, &top_r);
            ctx.collider.strip(rl_in, &top_l);
            ctx.collider.strip(&top_r, rr_in);
            ctx.report.approximated += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Texture / material loading (shared by city and the dev-world demo)
// ---------------------------------------------------------------------------

/// Decode a TEX file preserving every mip level (concatenated layer-major,
/// matching `Image::data` layout for multi-mip textures).
fn decode_tex(bytes: &[u8], logical: &str) -> Option<(Image, bool)> {
    let tex = match TexFile::parse(bytes) {
        Ok(t) => t,
        Err(e) => {
            warn!(logical = %logical, error = %e, "failed to parse TEX");
            return None;
        }
    };
    let mut data = Vec::new();
    let mut level_count = 0u32;
    for level in 0..tex.levels.len() {
        match tex.decode_rgba(level) {
            Some(rgba) => {
                data.extend_from_slice(&rgba);
                level_count += 1;
            }
            None => {
                warn!(logical = %logical, level, "TEX mip decode failed; dropping remaining levels");
                break;
            }
        }
    }
    if level_count == 0 {
        warn!(logical = %logical, "TEX decode produced no levels");
        return None;
    }
    // `Image::new` asserts `data` covers only the base level, so multi-mip
    // images are assembled field-by-field (like Bevy's own KTX2 loader).
    let mut image = Image::default();
    image.texture_descriptor.size = Extent3d {
        width: tex.header.width as u32,
        height: tex.header.height as u32,
        depth_or_array_layers: 1,
    };
    image.texture_descriptor.dimension = TextureDimension::D2;
    image.texture_descriptor.format = TextureFormat::Rgba8UnormSrgb;
    image.texture_descriptor.mip_level_count = level_count;
    image.data_order = bevy::render::render_resource::TextureDataOrder::MipMajor;
    image.data = Some(data);
    image.asset_usage = RenderAssetUsages::default();
    // TEX flags: ClampU = 0x01, ClampV = 0x10000 (per TEX.md); the default
    // is repeat — tiled roads rely on it.
    let bits = tex.header.bits;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: if bits & 0x01 != 0 {
            ImageAddressMode::ClampToEdge
        } else {
            ImageAddressMode::Repeat
        },
        address_mode_v: if bits & 0x1_0000 != 0 {
            ImageAddressMode::ClampToEdge
        } else {
            ImageAddressMode::Repeat
        },
        ..Default::default()
    });
    // Alpha is decided from the decoded pixels: only formats that actually
    // carry an alpha channel can produce transparency.
    let has_alpha = tex
        .decode_rgba(0)
        .map(|rgba| rgba.chunks_exact(4).any(|px| px[3] < 250))
        .unwrap_or(false);
    Some((image, has_alpha))
}

/// Decode PNG/TGA/KTX2 through Bevy's image loaders.
///
/// PNG and TGA decode losslessly. KTX2 is attempted with the compressed
/// formats compiled into this build — a recognized extension is not a
/// guarantee: unsupported encodings or missing GPU formats fail the decode
/// and are reported as a miss, never silently replaced.
fn decode_buffer_image(bytes: &[u8], ext: &str, logical: &str) -> Option<(Image, bool)> {
    let mut image = match Image::from_buffer(
        bytes,
        ImageType::Extension(ext),
        CompressedImageFormats::all(),
        true,
        ImageSampler::Default,
        RenderAssetUsages::default(),
    ) {
        Ok(img) => img,
        Err(e) => {
            warn!(logical = %logical, error = %e, "failed to decode image");
            return None;
        }
    };
    // City surfaces tile; repeat is the pipeline default.
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..Default::default()
    });
    let format = image.texture_descriptor.format;
    let has_alpha = match format {
        TextureFormat::Rgba8UnormSrgb
        | TextureFormat::Rgba8Unorm
        | TextureFormat::Bgra8UnormSrgb
        | TextureFormat::Bgra8Unorm => image
            .data
            .as_ref()
            .map(|d| d.chunks_exact(4).any(|px| px[3] < 250))
            .unwrap_or(false),
        // Compressed formats with an alpha channel.
        f if format_has_alpha(f) => true,
        _ => false,
    };
    Some((image, has_alpha))
}

/// Whether a texture format carries a meaningful alpha channel.
fn format_has_alpha(f: TextureFormat) -> bool {
    use TextureFormat::*;
    matches!(
        f,
        Rgba8Unorm
            | Rgba8UnormSrgb
            | Bgra8Unorm
            | Bgra8UnormSrgb
            | Rgba16Float
            | Rgba32Float
            | Bc2RgbaUnorm
            | Bc2RgbaUnormSrgb
            | Bc3RgbaUnorm
            | Bc3RgbaUnormSrgb
            | Bc7RgbaUnorm
            | Bc7RgbaUnormSrgb
            | Etc2Rgba8Unorm
            | Etc2Rgba8UnormSrgb
            | EacRg11Unorm
            | EacRg11Snorm
    )
}

/// Resolve an MM2 texture name to a decoded image, going through the
/// logical VFS lookup so mods can supply `png`/`ktx2`/`tga` replacements
/// for `.tex`. Returns the image plus whether the decoded pixels carry
/// meaningful transparency.
pub fn load_image(vfs: &Vfs, stem: &str) -> Option<(Image, bool)> {
    let resolved: Resolved = vfs.resolve_preferred(&format!("texture/{stem}"), TEXTURE_EXTS)?;
    let bytes = match vfs.read(&resolved) {
        Ok(b) => b,
        Err(e) => {
            warn!(logical = %resolved.logical, error = %e, "failed to read texture");
            return None;
        }
    };
    let ext = resolved.logical.rsplit('.').next().unwrap_or_default();
    if ext == "tex" {
        decode_tex(&bytes, &resolved.logical)
    } else {
        decode_buffer_image(&bytes, ext, &resolved.logical)
    }
}

/// Cache of name → material, keyed by the normalized logical stem —
/// resolution is deterministic per mount set, so a stem always maps to the
/// same selected source.
pub struct MaterialCache<'a> {
    vfs: &'a Vfs,
    images: &'a mut Assets<Image>,
    materials: &'a mut Assets<StandardMaterial>,
    by_key: HashMap<String, Handle<StandardMaterial>>,
    fallback: Handle<StandardMaterial>,
    /// Texture stems that failed to resolve (for the import report).
    missing: BTreeSet<String>,
}

impl<'a> MaterialCache<'a> {
    pub fn new(
        vfs: &'a Vfs,
        images: &'a mut Assets<Image>,
        materials: &'a mut Assets<StandardMaterial>,
    ) -> Self {
        let fallback = materials.add(StandardMaterial {
            base_color: Color::srgb(0.6, 0.6, 0.65),
            perceptual_roughness: 0.9,
            ..default()
        });
        Self {
            vfs,
            images,
            materials,
            by_key: HashMap::new(),
            fallback,
            missing: BTreeSet::new(),
        }
    }

    /// Material for an MM2 texture base name. Empty names get the
    /// fallback; unresolvable ones are recorded once.
    pub fn get(&mut self, name: &str) -> Handle<StandardMaterial> {
        let key = name.to_ascii_lowercase();
        if key.is_empty() {
            return self.fallback.clone();
        }
        if let Some(m) = self.by_key.get(&key) {
            return m.clone();
        }
        let mat = match load_image(self.vfs, &key) {
            Some((image, has_alpha)) => {
                let tex = self.images.add(image);
                // Alpha follows the decoded pixels: textures carrying real
                // transparency (trees, fences) become alpha-cutout — the
                // conservative choice that avoids blend sorting — and
                // everything else stays opaque.
                self.materials.add(StandardMaterial {
                    base_color_texture: Some(tex),
                    alpha_mode: if has_alpha {
                        AlphaMode::Mask(0.5)
                    } else {
                        AlphaMode::Opaque
                    },
                    perceptual_roughness: 0.95,
                    ..default()
                })
            }
            None => {
                debug!(texture = %key, "texture not found; using fallback");
                self.missing.insert(key.clone());
                self.fallback.clone()
            }
        };
        self.by_key.insert(key, mat.clone());
        mat
    }
}

// ---------------------------------------------------------------------------
// PKG props
// ---------------------------------------------------------------------------

/// LOD rank of a geometry chunk name: `*_vl` < `*_l` < `*_m` < `*_h` —
/// higher rank = higher detail. Names without a recognized LOD suffix rank
/// as `*_h`; only the last `_`-separated component is interpreted so real
/// part names are preserved.
fn lod_rank(name: &str) -> u8 {
    let lower = name.to_ascii_lowercase();
    match lower.rsplit('_').next().unwrap_or("") {
        "vl" => 0,
        "l" => 1,
        "m" => 2,
        _ => 3,
    }
}

/// Build renderable parts (best-LOD mesh per stem, grouped by shader) plus
/// a convex-hull point set for collision from a parsed PKG.
fn pkg_to_parts(
    pkg: &Pkg,
    mats: &mut MaterialCache<'_>,
    meshes: &mut Assets<Mesh>,
    missing_prims: &mut usize,
) -> Vec<(Handle<Mesh>, Handle<StandardMaterial>, Vec<Vec3>)> {
    let mut best: HashMap<String, (u8, &str)> = HashMap::new();
    for (name, _geo) in pkg.geometries() {
        let lower = name.to_ascii_lowercase();
        let stem = lower
            .rsplit_once('_')
            .map(|(s, _)| s.to_string())
            .unwrap_or(lower.clone());
        let rank = lod_rank(name);
        let entry = best.entry(stem).or_insert((rank, name));
        if rank > entry.0 {
            *entry = (rank, name);
        }
    }

    let shaders = pkg.shaders();
    let shader_at = |offset: i32| -> Option<&mm2_formats::pkg::PkgShader> {
        let s = shaders?;
        if offset < 0 {
            return None;
        }
        s.shaders.get(offset as usize)
    };

    let mut out = Vec::new();
    for (stem, (_, name)) in best {
        // Shadow/damage stand-ins are not rendered props.
        if stem.contains("shadow") || stem.contains("dmg") {
            continue;
        }
        let Some((_, geo)) = pkg.geometries().find(|(n, _)| *n == name) else {
            continue;
        };
        let mut by_shader: HashMap<i32, MeshBuilder> = HashMap::new();
        let mut hull_points: Vec<Vec3> = Vec::new();
        for section in &geo.sections {
            let b = by_shader.entry(section.shader_offset).or_default();
            for strip in &section.strips {
                if strip.prim_type != mm2_formats::pkg::PRIMTYPE_TRIANGLES {
                    // Only triangle lists are interpreted; other primitive
                    // types are reported, never guessed at.
                    *missing_prims += 1;
                    continue;
                }
                emit_strip(b, strip, &mut hull_points);
            }
        }
        for (shader_off, builder) in by_shader {
            if builder.is_empty() {
                continue;
            }
            let mat = match shader_at(shader_off) {
                Some(s) => {
                    let m = mats.get(&s.texture);
                    adjust_material(s, &m, mats.materials).unwrap_or(m)
                }
                None => mats.fallback.clone(),
            };
            out.push((meshes.add(builder.build()), mat, hull_points.clone()));
        }
    }
    out
}

/// Emit one PKG strip into a builder; authored normals and UVs preserved.
fn emit_strip(b: &mut MeshBuilder, strip: &PkgStrip, hull: &mut Vec<Vec3>) {
    let base = b.positions.len() as u32;
    for v in &strip.vertices {
        let p = v3(v.position);
        let uv = v.tex_coords.first().copied().unwrap_or([0.0, 0.0]);
        match v.normal {
            Some(n) => {
                b.vert_n(p, uv, v3(n));
            }
            None => {
                b.vert(p, uv);
            }
        }
        hull.push(p);
    }
    for t in strip.indices.chunks_exact(3) {
        b.tri(base + t[0] as u32, base + t[1] as u32, base + t[2] as u32);
    }
}

/// If the shader wants a non-opaque or tinted material, clone and adjust.
fn adjust_material(
    s: &mm2_formats::pkg::PkgShader,
    base: &Handle<StandardMaterial>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<Handle<StandardMaterial>> {
    let needs_blend = s.diffuse[3] < 0.99;
    let tinted = (s.diffuse[0] - 1.0).abs() > 0.01
        || (s.diffuse[1] - 1.0).abs() > 0.01
        || (s.diffuse[2] - 1.0).abs() > 0.01;
    if !needs_blend && !tinted {
        return None;
    }
    let mut mat = materials
        .get(base)
        .cloned()
        .unwrap_or_else(StandardMaterial::default);
    mat.base_color = Color::srgba(s.diffuse[0], s.diffuse[1], s.diffuse[2], s.diffuse[3]);
    if needs_blend {
        mat.alpha_mode = AlphaMode::Blend;
    }
    Some(materials.add(mat))
}

/// Mesh + material + hull pairs prepared from one PKG.
type PropParts = Vec<(Handle<Mesh>, Handle<StandardMaterial>, Vec<Vec3>)>;

/// Cache of PKG name → prepared meshes+materials handles.
struct PropCache<'a> {
    vfs: &'a Vfs,
    meshes: &'a mut Assets<Mesh>,
    mats: MaterialCache<'a>,
    cache: HashMap<String, Option<PropParts>>,
    /// Strips with unsupported primitive types encountered while building.
    missing_prims: usize,
}

impl<'a> PropCache<'a> {
    fn get(&mut self, name: &str) -> Option<&PropParts> {
        let key = name.to_ascii_lowercase();
        if !self.cache.contains_key(&key) {
            let built = self.build(&key);
            self.cache.insert(key.clone(), built);
        }
        self.cache.get(&key).and_then(|o| o.as_ref())
    }

    fn build(&mut self, name: &str) -> Option<PropParts> {
        let resolved = self
            .vfs
            .resolve_preferred(&format!("geometry/{name}"), &["pkg"])
            .or_else(|| self.vfs.resolve_preferred(name, &["pkg"]))?;
        let bytes = self.vfs.read(&resolved).ok()?;
        let pkg = match Pkg::parse(&bytes) {
            Ok(p) => p,
            Err(e) => {
                warn!(pkg = %name, error = %e, "PKG parse failed");
                return None;
            }
        };
        let parts = pkg_to_parts(&pkg, &mut self.mats, self.meshes, &mut self.missing_prims);
        if parts.is_empty() {
            return None;
        }
        Some(parts)
    }
}

/// Convert an INST coordinate placement to a Bevy `Mat4` in mirrored space.
fn inst_transform(c: &inst::InstCoordinate) -> Mat4 {
    // M' = S·M·S with S = diag(1,1,-1): mirror each column's z, then negate
    // the whole z column, and mirror the origin.
    let x = v3(c.x_axis);
    let y = v3(c.y_axis);
    let z = -v3(c.z_axis);
    let o = v3(c.origin);
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), o.extend(1.0))
}

// ---------------------------------------------------------------------------
// Orchestration
// ---------------------------------------------------------------------------

/// Result of loading a city: where to put the player plus the import
/// report for diagnostics.
pub struct LoadedCity {
    /// Validated spawn point on road geometry.
    pub spawn: Vec3,
    /// Import statistics.
    pub report: CityReport,
}

/// Why a city failed to load.
#[derive(Debug)]
pub enum LoadCityError {
    /// The PSDL could not be read from the VFS.
    Missing(String),
    /// The PSDL bytes did not parse.
    Malformed(String),
    /// The file parsed but produced no renderable or collidable geometry —
    /// spawning into an empty world would be a fake success.
    Empty(String),
}

impl std::fmt::Display for LoadCityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(m) => write!(f, "city data missing: {m}"),
            Self::Malformed(m) => write!(f, "city data malformed: {m}"),
            Self::Empty(m) => write!(f, "city produced no geometry: {m}"),
        }
    }
}

impl std::error::Error for LoadCityError {}

/// Load a city from `psdl_path` (e.g. `city/london.psdl`) plus its sibling
/// `.inst` placement file.
pub fn load_city(
    commands: &mut Commands,
    vfs: &Vfs,
    psdl_path: &str,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Result<LoadedCity, LoadCityError> {
    let (bytes, resolved) = vfs
        .read_path(psdl_path)
        .map_err(|e| LoadCityError::Missing(format!("{psdl_path}: {e}")))?;
    let psdl = Psdl::parse(&bytes)
        .map_err(|e| LoadCityError::Malformed(format!("{}: {e}", resolved.logical)))?;
    info!(
        path = %resolved.logical,
        rooms = psdl.rooms.len(),
        textures = psdl.textures.len(),
        "loading city"
    );

    let import = emit_psdl(&psdl);
    if import.meshes.is_empty() && import.colliders.is_empty() {
        return Err(LoadCityError::Empty(resolved.logical.clone()));
    }

    let mut mats = MaterialCache::new(vfs, images, materials);
    let mut report = import.report;

    // Render meshes: one entity per (room, texture) — spatially bounded,
    // retaining the room id in the entity name.
    for group in import.meshes {
        let builder = MeshBuilder {
            positions: group.positions,
            uvs: group.uvs,
            normals: Vec::new(),
            indices: group.indices,
        };
        let material = match group.texture {
            Some(i) => mats.get(&psdl.textures.get(i).cloned().unwrap_or_default()),
            None => mats.fallback.clone(),
        };
        commands.spawn((
            CityEntity,
            Mesh3d(meshes.add(builder.build())),
            MeshMaterial3d(material),
            Name::new(format!(
                "city-room{}-tex{}",
                group.room + 1,
                group.texture.map(|i| i as i64).unwrap_or(-1)
            )),
        ));
    }
    // Collision: one static trimesh per room.
    for col in import.colliders {
        commands.spawn((
            CityEntity,
            RigidBody::Static,
            Collider::trimesh(col.positions, col.tris),
            Name::new(format!("city-room{}-collider", col.room + 1)),
        ));
    }

    // INST placements → PKG props, each with an explicit collision policy:
    // a convex hull over the best-LOD vertices (documented approximation —
    // good enough for lamps, signs and rails in a driving slice).
    let inst_path = psdl_path.replace(".psdl", ".inst");
    match vfs.read_path(&inst_path) {
        Ok((inst_bytes, inst_res)) => match inst::parse(&inst_bytes) {
            Ok(comps) => {
                let mut cache = PropCache {
                    vfs,
                    meshes,
                    mats,
                    cache: HashMap::new(),
                    missing_prims: 0,
                };
                for comp in &comps {
                    let Some(parts) = cache.get(&comp.package_name) else {
                        report.props_failed += 1;
                        continue;
                    };
                    let mat4 = match &comp.placement {
                        InstPlacement::Coordinate(c) => inst_transform(c),
                        InstPlacement::Simple(s) => {
                            let pos = v3(s.location);
                            let dir = v3([s.x_delta, 0.0, s.z_delta]);
                            let scale = dir.length().max(0.001);
                            let yaw = if MIRROR_Z {
                                dir.x.atan2(-dir.z)
                            } else {
                                dir.x.atan2(dir.z)
                            };
                            Mat4::from_rotation_translation(Quat::from_rotation_y(yaw), pos)
                                * Mat4::from_scale(Vec3::splat(scale))
                        }
                    };
                    let transform = Transform::from_matrix(mat4);
                    for (mesh, material, hull) in parts {
                        let mut e = commands.spawn((
                            CityEntity,
                            Mesh3d(mesh.clone()),
                            MeshMaterial3d(material.clone()),
                            transform,
                            RigidBody::Static,
                            Name::new(format!("prop-{}", comp.package_name)),
                        ));
                        if let Some(c) = Collider::convex_hull(hull.clone()) {
                            e.insert(c);
                        }
                    }
                    report.props_spawned += 1;
                }
                if cache.missing_prims > 0 {
                    report
                        .unsupported
                        .insert("pkg-non-triangle-strips".into(), cache.missing_prims);
                }
                report.missing_textures = std::mem::take(&mut cache.mats.missing);
                info!(path = %inst_res.logical, props = report.props_spawned, "inst props spawned");
            }
            Err(e) => {
                warn!(path = %inst_res.logical, error = %e, "INST parse failed");
                report.missing_textures = std::mem::take(&mut mats.missing);
            }
        },
        Err(_) => {
            debug!(path = %inst_path, "no INST file; skipping props");
            report.missing_textures = std::mem::take(&mut mats.missing);
        }
    }
    info!(report = %report, "city import");

    Ok(LoadedCity {
        spawn: import.spawn,
        report,
    })
}
