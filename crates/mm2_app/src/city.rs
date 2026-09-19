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
///
/// Off: PSDL coordinates are used as authored. Mirroring Z reflects the
/// whole city — verified on retail San Francisco, where it put the city on
/// the wrong side of the Golden Gate bridge and rendered facade signage
/// back to front (a shopfront reading `PASTA` came out as `ATSAP`).
const MIRROR_Z: bool = false;

/// World scale (metres) per texture repeat for planar-mapped city surfaces.
/// The PSDL format does not store UVs for most ground attributes; this is a
/// documented approximation.
const PLANAR_UV_SCALE: f32 = 8.0;

/// Metres of road per repeat of a road texture along its length. Road
/// textures are authored with `u` along the road (dashes ≈ 4 m at this
/// scale); the format stores no UVs, so the length is an approximation.
const ROAD_TILE_LENGTH: f32 = 10.0;

/// Metres of sidewalk per texture repeat along its length.
const SIDEWALK_TILE_LENGTH: f32 = 4.0;

/// Portion of a sidewalk texture (from v = 0) holding the kerb stones.
const CURB_V: f32 = 0.07;

/// Sidewalks sit this far above road vertices (per `Room_attributes`:
/// "the road surface vertices are expected to be located 0.15 units below
/// the sidewalk vertices").
const SIDEWALK_LIFT: f32 = 0.15;

/// Extra depth added below curb bottom edges to seal hairline seams
/// against neighbouring surfaces.
const CURB_SINK: f32 = 0.1;

/// Height used for invisible (type-0) divider collision bounds. The bound
/// height is not stored in the attribute; this documented approximation is
/// enough to keep the car out of the median.
const INVISIBLE_DIVIDER_HEIGHT: f32 = 0.8;

/// Clearance above the road surface for the player spawn point.
const SPAWN_CLEARANCE: f32 = 1.5;

/// Playback rate of animated texture sequences (`<stem>-0001`, `-0002`, …).
/// The rate is not stored in the data; this is an approximation.
const TEXTURE_ANIM_FPS: f32 = 10.0;

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

/// Map a Bevy-space z back to the authored (x, z) plane the room polygons
/// and facing tests work in. The Z mirror is its own inverse, so this is
/// also the way in.
#[inline]
fn authored_z(z: f32) -> f32 {
    if MIRROR_Z { -z } else { z }
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

    /// Push a triangle with the authored winding resolved to a +Y-facing
    /// front face. Ground attributes are authored clockwise in the (x, z)
    /// plane, which already gives +Y in Bevy space; mirroring Z flips the
    /// winding, so it is reversed only when [`MIRROR_Z`] is set.
    fn tri(&mut self, a: u32, b: u32, c: u32) {
        if MIRROR_Z {
            self.tri_rev(a, b, c);
        } else {
            self.tri_keep(a, b, c);
        }
    }

    /// Push a triangle with the winding reversed from the indices given.
    fn tri_rev(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend_from_slice(&[a, c, b]);
    }

    /// Push a triangle with whichever winding produces an upward-facing
    /// normal. Ground surfaces are ~98% authored clockwise, but a small
    /// minority are not — a down-facing ground triangle is never
    /// intentional, so each emitted triangle is oriented by its
    /// geometric normal.
    fn tri_up(&mut self, a: u32, b: u32, c: u32) {
        let pa = Vec3::from_array(self.positions[a as usize]);
        let pb = Vec3::from_array(self.positions[b as usize]);
        let pc = Vec3::from_array(self.positions[c as usize]);
        // tri_rev(a,b,c) emits (a, c, b): its normal is (c−a)×(b−a).
        if (pc - pa).cross(pb - pa).y >= 0.0 {
            self.tri_rev(a, b, c);
        } else {
            self.tri_keep(a, b, c);
        }
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
            self.tri_up(base[0], base[i], base[i + 1]);
        }
    }

    /// Strip between chains `a` and `b` with lengthwise UVs: `us[i]` is
    /// the texture u of cross-section `i`, `va`/`vb` the v on each chain.
    /// Road and sidewalk textures are authored this way (u along the
    /// surface, v across it). Each triangle is oriented by its geometric
    /// normal — the authored clockwise order is ~97% consistent, and the
    /// remainder must not render face-down.
    fn strip_uv(&mut self, a: &[Vec3], b: &[Vec3], us: &[f32], va: f32, vb: f32) {
        for i in 0..a.len().min(b.len()).saturating_sub(1) {
            let a0 = self.vert(a[i], [us[i], va]);
            let a1 = self.vert(a[i + 1], [us[i + 1], va]);
            let b0 = self.vert(b[i], [us[i], vb]);
            let b1 = self.vert(b[i + 1], [us[i + 1], vb]);
            self.tri_up(a0, b0, a1);
            self.tri_up(a1, b0, b1);
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

    /// Single triangle emitted so its front normal points toward `facing`.
    fn tri_facing(&mut self, p: [Vec3; 3], uvs: [[f32; 2]; 3], facing: Vec3) {
        let n = (p[1] - p[0]).cross(p[2] - p[0]);
        let i = [0, 1, 2].map(|k| self.vert(p[k], uvs[k]));
        if n.dot(facing) >= 0.0 {
            self.tri_keep(i[0], i[1], i[2]);
        } else {
            self.tri_keep(i[0], i[2], i[1]);
        }
    }

    /// Vertical wall quad facing `facing` (horizontal). The authored
    /// left→right bottom edge runs along the (clockwise) block perimeter,
    /// so the street-facing side is conventionally to the *left* of the
    /// edge — the caller's `facing` refines that per quad via the room
    /// polygon (see [`EmitCtx::wall_facing`]). `v` runs from `v_rep` at
    /// the bottom edge to 0 at the top so textures sit upright (D3D v=0
    /// at the image top).
    fn wall_quad(&mut self, l: Vec3, r: Vec3, bottom: f32, top: f32, reps: [f32; 2], facing: Vec3) {
        let [u_rep, v_rep] = reps;
        // The two height refs are not ordered: ~8% of retail facades name
        // the higher one first, which would put the image's bottom along
        // the quad's upper edge.
        let (lo, hi) = if bottom <= top {
            (bottom, top)
        } else {
            (top, bottom)
        };
        self.quad_facing(
            Vec3::new(l.x, lo, l.z),
            Vec3::new(r.x, lo, r.z),
            Vec3::new(r.x, hi, r.z),
            Vec3::new(l.x, hi, l.z),
            [[0.0, v_rep], [u_rep, v_rep], [u_rep, 0.0], [0.0, 0.0]],
            facing,
        );
    }

    /// Slanted-bottom wall (sliver): the bottom edge follows the authored
    /// vertex heights, the top is horizontal at `top`. `v_scale` converts
    /// depth below the top edge to texture v, so — as in [`wall_quad`] —
    /// v is 0 along the top edge and grows downwards.
    fn sliver_quad(&mut self, l: Vec3, r: Vec3, top: f32, v_scale: f32, facing: Vec3) {
        self.quad_facing(
            l,
            r,
            Vec3::new(r.x, top, r.z),
            Vec3::new(l.x, top, l.z),
            [
                [0.0, (top - l.y) * v_scale],
                [1.0, (top - r.y) * v_scale],
                [1.0, 0.0],
                [0.0, 0.0],
            ],
            facing,
        );
    }

    /// Triangle fan whose front faces `facing` — picks whichever winding
    /// produces a geometric normal pointing along `facing` (for wall-like
    /// vertical fans whose authored winding is inconsistent).
    fn fan_facing(&mut self, pts: &[Vec3], facing: Vec3) {
        if pts.len() < 3 {
            return;
        }
        // UVs on the vertical plane spanned by the fan's horizontal tangent
        // and +Y, so the texture isn't collapsed to a texel row.
        let t = facing.cross(Vec3::Y).normalize_or_zero();
        let uv = |p: Vec3| {
            [
                (p.x * t.x + p.z * t.z) / PLANAR_UV_SCALE,
                p.y / PLANAR_UV_SCALE,
            ]
        };
        let base: Vec<u32> = pts.iter().map(|&p| self.vert(p, uv(p))).collect();
        // tri_rev() emits (a, c, b): its geometric normal is (c−a)×(b−a).
        let mut keep = false;
        for i in 1..pts.len() - 1 {
            let n = (pts[i + 1] - pts[0]).cross(pts[i] - pts[0]);
            if n.length_squared() > 1e-6 {
                keep = n.dot(facing) < 0.0;
                break;
            }
        }
        for i in 1..base.len() - 1 {
            if keep {
                self.tri_keep(base[0], base[i], base[i + 1]);
            } else {
                self.tri_rev(base[0], base[i], base[i + 1]);
            }
        }
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
    /// Heading (rotation about +Y) that points the vehicle along the road.
    pub spawn_yaw: f32,
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
    // Points on a road's midline, with the road direction there.
    let mut road_midpoints: Vec<(Vec3, Vec3)> = Vec::new();

    // Texture state persists across rooms (rooms normally open with their
    // own refs); attributes emitted before the first ref get the fallback
    // material and are counted.
    let mut tex = TexState::Unset;
    let mut unset_emits = 0usize;

    for (room_idx, room) in psdl.rooms.iter().enumerate() {
        report.unparsed_words += room.unparsed_attributes.len();
        // The room perimeter (authored x, z) drives the facing test for
        // walls; street rooms are the ones carrying drivable geometry.
        let poly: Vec<(f32, f32)> = room
            .perimeter
            .iter()
            .filter_map(|p| psdl.vertices.get(p.vertex as usize).map(|v| (v[0], v[2])))
            .collect();
        let perim: Vec<Vec3> = room
            .perimeter
            .iter()
            .filter_map(|p| psdl.vertices.get(p.vertex as usize).map(|&v| v3(v)))
            .collect();
        let room_is_street = room.attributes.iter().any(|a| {
            matches!(
                a.kind,
                AttributeType::RoadWithSidewalks
                    | AttributeType::RoadNoSidewalks
                    | AttributeType::SidewalkStrip
                    | AttributeType::DividedRoad
                    | AttributeType::RoadFan
                    | AttributeType::Crosswalk
            )
        });
        let mut groups: BTreeMap<i64, MeshBuilder> = BTreeMap::new();
        let mut collider = ColliderBuilder::default();
        let mut road_acc = Vec3::ZERO;
        let mut road_n = 0usize;
        let mut road_max_y = f32::MIN;
        let mut road_tunnel: Option<RoadTunnel> = None;

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
                perim: &perim,
                poly: &poly,
                room_is_street,
                groups: &mut groups,
                collider: &mut collider,
                tex_key,
                road_tunnel: &mut road_tunnel,
                road_midpoints: &mut road_midpoints,
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

    let (spawn, spawn_yaw) = choose_spawn(psdl, &road_midpoints, &road_surfaces);
    CityImport {
        meshes,
        colliders,
        spawn,
        spawn_yaw,
        report,
    }
}

/// Pick a spawn on verified road geometry: the road midline point nearest
/// the city centre, heading along the road. A midline point lies on the
/// emitted surface by construction — a room's vertex centroid does not
/// (curved or hilly roads put it beside or under the road). Cities without
/// road strips fall back to the nearest road room's centroid, a vehicle
/// height above its highest road vertex.
fn choose_spawn(psdl: &Psdl, midpoints: &[(Vec3, Vec3)], roads: &[(Vec3, f32)]) -> (Vec3, f32) {
    let center = v3(psdl.bounds_center);
    let dist = |p: Vec3| (p.xz() - center.xz()).length_squared();
    if let Some((pos, dir)) = midpoints
        .iter()
        .min_by(|a, b| dist(a.0).total_cmp(&dist(b.0)))
    {
        // Vehicle forward is local −Z.
        return (*pos + Vec3::Y * SPAWN_CLEARANCE, (-dir.x).atan2(-dir.z));
    }
    match roads.iter().min_by(|a, b| dist(a.0).total_cmp(&dist(b.0))) {
        Some((centroid, max_y)) => (
            Vec3::new(centroid.x, max_y + SPAWN_CLEARANCE, centroid.z),
            0.0,
        ),
        // No road attributes at all: above the bounds centre.
        None => (Vec3::new(center.x, center.y + 10.0, center.z), 0.0),
    }
}

/// Decoded road tunnel/railing parameters (attribute 0x09 subtype 3):
/// walls are rendered along the road attributes that *follow* it in the
/// same room's attribute stream. `tex_key` is the texture reference in
/// effect when the tunnel attribute is read — the tunnel's six texture
/// slots (left/right wall, ceiling, right/left outside, ground) are
/// relative to it.
#[derive(Clone, Copy)]
struct RoadTunnel {
    flags: u16,
    /// Wall height above each road-edge vertex (metres).
    height: f32,
    /// Second height value — the ceiling apex on curved ceilings;
    /// ignored for flat/railing geometry.
    height2: f32,
    tex_key: i64,
}

impl RoadTunnel {
    const LEFT: u16 = 1 << 0;
    const RIGHT: u16 = 1 << 1;
    /// Flat ceiling between the wall tops. (Bit 2 only selects the thick
    /// wall style — SF's open-air freeway retaining walls set it, real
    /// tunnels do not — verified on retail data by texture set.)
    const FLAT_CEILING: u16 = 1 << 3;
    /// Curved ceiling rising to `height2` above the road's midline.
    const CURVED_CEILING: u16 = 1 << 8;
    /// Bits 4–7 close/chamfer wall ends; bits 9–12 chamfer corners; not
    /// modelled — counted as approximated.
    const DETAIL_MASK: u16 = 0x1ef0;

    /// Tunnel heights are 8.8 fixed-point metres (0x0580 = 5.5 m).
    fn metres(word: u16) -> f32 {
        word as f32 / 256.0
    }
}

struct EmitCtx<'a> {
    verts: &'a [Vec3],
    heights: &'a [f32],
    /// Perimeter entry vertices (Bevy space) — junction-tunnel walls run
    /// along enabled perimeter edges.
    perim: &'a [Vec3],
    /// The room's perimeter polygon in authored (x, z) space — used to
    /// pick the visible side of wall-like geometry.
    poly: &'a [(f32, f32)],
    /// Whether the room contains drivable geometry: walls in street rooms
    /// face the polygon interior; walls in facade-only building blocks
    /// face outward.
    room_is_street: bool,
    groups: &'a mut BTreeMap<i64, MeshBuilder>,
    collider: &'a mut ColliderBuilder,
    tex_key: i64,
    /// Pending road-tunnel spec shared across the room's attributes:
    /// subtype-3 tunnel attributes set it, road attributes read it.
    road_tunnel: &'a mut Option<RoadTunnel>,
    road_midpoints: &'a mut Vec<(Vec3, Vec3)>,
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

    /// Record spawn candidates: the midpoint of each segment of a
    /// drivable strip between chains `l` and `r`, with its direction.
    fn note_midline(&mut self, l: &[Vec3], r: &[Vec3]) {
        for i in 0..l.len().min(r.len()).saturating_sub(1) {
            let a = (l[i] + r[i]) * 0.5;
            let b = (l[i + 1] + r[i + 1]) * 0.5;
            let dir = (b - a).normalize_or_zero();
            if dir != Vec3::ZERO {
                self.road_midpoints.push(((a + b) * 0.5, dir));
            }
        }
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

    /// The direction the visible face of a wall on edge `l→r` should
    /// point (Bevy space, horizontal). Convention: the visible face is on
    /// the left of the authored edge — verified on retail London, where
    /// perimeter-aligned facade edges always follow the clockwise
    /// perimeter traversal and facades live in road-less building blocks.
    /// The room polygon refines the choice: offsetting the edge midpoint
    /// along each candidate normal, the street side is inside the polygon
    /// for street rooms and outside it for building blocks. Ambiguous
    /// cases (courtyard chords, missing perimeters) keep the authored
    /// left-of-edge convention.
    fn wall_facing(&self, l: Vec3, r: Vec3) -> Vec3 {
        // Authored coordinates undo the Z mirror.
        let (ax, az) = (l.x, authored_z(l.z));
        let (bx, bz) = (r.x, authored_z(r.z));
        let (dx, dz) = (bx - ax, bz - az);
        let len = (dx * dx + dz * dz).sqrt();
        if len < 1e-3 || self.poly.len() < 3 {
            let (nx, nz) = (-dz, dx);
            return Vec3::new(nx, 0.0, authored_z(nz)).normalize_or_zero();
        }
        // Authored left-of-edge normal.
        let (nx, nz) = (-dz / len, dx / len);
        let mid = ((ax + bx) * 0.5, (az + bz) * 0.5);
        let eps = (len * 0.25).clamp(0.05, 0.6);
        let inside_l = point_in_poly((mid.0 + nx * eps, mid.1 + nz * eps), self.poly);
        let inside_r = point_in_poly((mid.0 - nx * eps, mid.1 - nz * eps), self.poly);
        let face_left = match (inside_l, inside_r) {
            (true, false) => self.room_is_street,
            (false, true) => !self.room_is_street,
            _ => true,
        };
        let (fx, fz) = if face_left { (nx, nz) } else { (-nx, -nz) };
        Vec3::new(fx, 0.0, authored_z(fz))
    }

    /// For mostly-vertical fans (gables, embankment walls — unlike ground
    /// fans their authored winding isn't reliable): the Bevy-space
    /// direction the fan should face, or `None` for horizontal/degenerate
    /// fans and ambiguous sides (which keep the authored winding).
    fn vertical_facing(&self, pts: &[Vec3]) -> Option<Vec3> {
        let mut n = Vec3::ZERO;
        for i in 1..pts.len().saturating_sub(1) {
            let c = (pts[i] - pts[0]).cross(pts[i + 1] - pts[0]);
            if c.length_squared() > 1e-6 {
                n = c.normalize();
                break;
            }
        }
        if n.length_squared() < 0.5 || n.y.abs() >= 0.3 || self.poly.len() < 3 {
            return None;
        }
        let mut mid = Vec3::ZERO;
        for p in pts {
            mid += *p;
        }
        mid /= pts.len() as f32;
        // Authored (x, z) space again.
        let (mx, mz) = (mid.x, authored_z(mid.z));
        let (nx, nz) = (n.x, authored_z(n.z));
        let nl = (nx * nx + nz * nz).sqrt().max(1e-6);
        let (nx, nz) = (nx / nl, nz / nl);
        let inside_p = point_in_poly((mx + nx * 0.5, mz + nz * 0.5), self.poly);
        let inside_m = point_in_poly((mx - nx * 0.5, mz - nz * 0.5), self.poly);
        if inside_p == inside_m {
            return None;
        }
        // Face the street side: polygon interior for street rooms,
        // exterior for building blocks.
        let face_with_n = inside_p == self.room_is_street;
        Some(if face_with_n { n } else { -n })
    }
}

/// Ray-cast point-in-polygon over the authored (x, z) perimeter.
fn point_in_poly(p: (f32, f32), poly: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (xi, zi) = poly[i];
        let (xj, zj) = poly[(i + 1) % n];
        if (zi > p.1) != (zj > p.1) && p.0 < (xj - xi) * (p.1 - zi) / (zj - zi) + xi {
            inside = !inside;
        }
    }
    inside
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
            match ctx.vertical_facing(&pts) {
                // Wall-like fan: face the street side of the room.
                Some(facing) => ctx.builder(0).fan_facing(&pts, facing),
                None => ctx.builder(0).fan(&pts),
            }
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
            emit_road_surface(ctx, &rl, &rr);
            ctx.collider.strip(&rl, &rr);
            ctx.note_road(&rl);
            ctx.note_road(&rr);
            ctx.note_midline(&rl, &rr);
            emit_sidewalk(ctx, &sw_l, &rl);
            emit_sidewalk(ctx, &sw_r, &rr);
            emit_road_tunnel(ctx, &sw_l, &sw_r);
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
                // The cap is a slanted ramp whose authored winding varies;
                // emit both windings so it never back-face-culls away.
                ctx.builder(1).fan(&[b, a, apex]);
                ctx.builder(1).fan(&[a, b, apex]);
                ctx.collider.tri(a, b, apex);
                return Ok(Outcome::Emitted);
            }
            // Pairs (a, b): `a` is authored at ground level and lifted
            // SIDEWALK_LIFT to form the top's inner edge (verified: b.y −
            // a.y == 0.15 for every pair in retail London); `b` is authored
            // at sidewalk-top height. The top spans lifted-a → b, and the
            // vertical curb face sits on the `a` edge facing away from b.
            let mut ground = Vec::new();
            let mut top = Vec::new();
            for s in refs.chunks_exact(2) {
                ground.push(vertex(s[0], ctx.verts)?);
                top.push(vertex(s[1], ctx.verts)?);
            }
            let lifted: Vec<Vec3> = ground
                .iter()
                .map(|v| *v + Vec3::Y * SIDEWALK_LIFT)
                .collect();
            // Intersection textures: n = road, n+1 = sidewalk, n+2 =
            // crosswalk. Sidewalk textures have the kerb stones at v = 0.
            let us = chain_u(&lifted, &top, SIDEWALK_TILE_LENGTH);
            ctx.builder(1).strip_uv(&lifted, &top, &us, 0.0, 1.0);
            emit_curb(ctx, &ground, &lifted, &top);
            ctx.collider.strip(&lifted, &top);
            ctx.collider.strip(&ground, &lifted);
            ctx.note_road(&lifted);
            ctx.note_road(&top);
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
            // Walkway textures span the full width (e.g. subway rails).
            let us = chain_u(&l, &r, ROAD_TILE_LENGTH);
            ctx.builder(0).strip_uv(&l, &r, &us, 0.0, 1.0);
            ctx.collider.strip(&l, &r);
            ctx.note_road(&l);
            ctx.note_road(&r);
            emit_road_tunnel(ctx, &l, &r);
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
            let us = chain_u(&rl_out, &rr_out, ROAD_TILE_LENGTH);
            ctx.builder(0).strip_uv(&rl_out, &rl_in, &us, 1.0, 0.0);
            ctx.builder(0).strip_uv(&rr_in, &rr_out, &us, 0.0, 1.0);
            ctx.collider.strip(&rl_out, &rl_in);
            ctx.collider.strip(&rr_in, &rr_out);
            ctx.note_road(&rl_out);
            ctx.note_road(&rl_in);
            ctx.note_road(&rr_in);
            ctx.note_road(&rr_out);
            ctx.note_midline(&rr_in, &rr_out);
            emit_sidewalk(ctx, &sw_l, &rl_out);
            emit_sidewalk(ctx, &sw_r, &rr_out);
            emit_divider(ctx, div_type, div_tex, value, &rl_in, &rr_in);
            emit_road_tunnel(ctx, &sw_l, &sw_r);
            Outcome::Emitted
        }
        AttributeType::Crosswalk => {
            // Four corner refs in strip order — (0, 1) is one short end,
            // (2, 3) the other — textured with n+2. Crosswalk textures
            // carry their border lines at u = 0 and u = 1, so u spans the
            // short side and v repeats along the crossing, one tile per
            // crossing depth.
            if attr.data.len() != 4 {
                return Err(AttrError::Malformed("crosswalk"));
            }
            let pts = ctx.resolve(&attr.data[..4])?;
            let depth = (pts[1] - pts[0]).length().max(0.1);
            let reps = ((pts[2] - pts[0]).length() / depth).round().max(1.0);
            let (a, b) = ([pts[0], pts[1]], [pts[2], pts[3]]);
            ctx.builder(2).strip_uv(&a, &b, &[0.0, 1.0], 0.0, reps);
            ctx.collider.strip(&a, &b);
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
            let facing = ctx.wall_facing(l, r);
            ctx.builder(0).sliver_quad(l, r, top, scale, facing);
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
            let facing = ctx.wall_facing(l, r);
            ctx.builder(0).wall_quad(
                l,
                r,
                hb,
                ht,
                [attr.data[2].max(1) as f32, attr.data[3].max(1) as f32],
                facing,
            );
            Outcome::Emitted
        }
        AttributeType::Tunnel => {
            if attr.subtype == 0 {
                // Junction tunnel/railing: [nSize, flags, h1w, h2w,
                // unk3, enabledWalls…]. Bit i of the wall array draws a
                // wall on the perimeter edge from entry i−1 to entry i
                // (wrapping); entries may repeat a vertex when a point
                // links several rooms — such edges are degenerate and
                // skipped. Junction walls use texture n+0 for the face
                // toward the room interior and n+4 outside.
                if attr.data.len() < 5 {
                    return Err(AttrError::Malformed("junction"));
                }
                let flags = attr.data[1];
                let height = RoadTunnel::metres(attr.data[2]);
                let walls = &attr.data[5..];
                let n = ctx.perim.len();
                for i in 0..n {
                    let set = walls.get(i / 16).map_or(0, |w| (w >> (i % 16)) & 1);
                    if set == 0 {
                        continue;
                    }
                    let a = ctx.perim[(i + n - 1) % n];
                    let b = ctx.perim[i];
                    if (b - a).length_squared() < 1e-4 {
                        continue;
                    }
                    let facing = ctx.wall_facing(a, b);
                    emit_wall(ctx, a, b, height, ctx.tex_key, ctx.tex_key + 4, facing);
                }
                if flags & (RoadTunnel::FLAT_CEILING | RoadTunnel::CURVED_CEILING) != 0 && n >= 3 {
                    // Ceiling over the whole junction at wall height,
                    // fanned from the centroid (perimeters may be concave).
                    let lift = Vec3::Y * height;
                    let centre = ctx.perim.iter().sum::<Vec3>() / n as f32 + lift;
                    for i in 0..n {
                        let a = ctx.perim[i] + lift;
                        let b = ctx.perim[(i + 1) % n] + lift;
                        let tri = [centre, a, b];
                        ctx.builder(2).tri_facing(
                            tri,
                            tri.map(MeshBuilder::planar_uv),
                            Vec3::NEG_Y,
                        );
                        ctx.collider.tri(centre, a, b);
                    }
                }
                Outcome::Emitted
            } else {
                // Road tunnel/railing: [flags, h1w, h2w]; heights are 8.8
                // fixed-point metres. Applies to the room's following
                // road attributes.
                if attr.data.len() < 3 {
                    return Err(AttrError::Malformed("tunnel"));
                }
                *ctx.road_tunnel = Some(RoadTunnel {
                    flags: attr.data[0],
                    height: RoadTunnel::metres(attr.data[1]),
                    height2: RoadTunnel::metres(attr.data[2]),
                    tex_key: ctx.tex_key,
                });
                if attr.data[0] & RoadTunnel::DETAIL_MASK != 0 {
                    ctx.report.approximated += 1;
                }
                Outcome::Emitted
            }
        }
        AttributeType::TextureRef => Outcome::Emitted, // handled by the caller
        AttributeType::Unknown(raw) => {
            Outcome::Unsupported(format!("unknown-attribute-type-{raw:#04x}"))
        }
    })
}

/// Texture u per cross-section: distance along the midline between the
/// two chains, in repeats of `tile` metres.
fn chain_u(a: &[Vec3], b: &[Vec3], tile: f32) -> Vec<f32> {
    let mut us = Vec::with_capacity(a.len());
    let mut dist = 0.0;
    let mut prev: Option<Vec3> = None;
    for (pa, pb) in a.iter().zip(b) {
        let mid = (*pa + *pb) * 0.5;
        if let Some(p) = prev {
            dist += (mid - p).length();
        }
        prev = Some(mid);
        us.push(dist / tile);
    }
    us
}

/// Two-way road surface between edge chains `l` and `r`. Road textures
/// hold half a road — centre line at v = 0, kerb at v = 1, clamped in v —
/// so the surface is split along its midline and the texture mirrored.
fn emit_road_surface(ctx: &mut EmitCtx<'_>, l: &[Vec3], r: &[Vec3]) {
    let mid: Vec<Vec3> = l.iter().zip(r).map(|(a, b)| (*a + *b) * 0.5).collect();
    let us = chain_u(l, r, ROAD_TILE_LENGTH);
    ctx.builder(0).strip_uv(l, &mid, &us, 1.0, 0.0);
    ctx.builder(0).strip_uv(&mid, r, &us, 0.0, 1.0);
}

/// Sidewalk top + vertical curb face on one side of a road. `outer` is the
/// outer sidewalk edge (authored height), `road` the adjacent road edge;
/// the inner sidewalk edge sits SIDEWALK_LIFT above the road vertex.
fn emit_sidewalk(ctx: &mut EmitCtx<'_>, outer: &[Vec3], road: &[Vec3]) {
    let inner: Vec<Vec3> = road.iter().map(|v| *v + Vec3::Y * SIDEWALK_LIFT).collect();
    let us = chain_u(&inner, outer, SIDEWALK_TILE_LENGTH);
    ctx.builder(1).strip_uv(&inner, outer, &us, 0.0, 1.0); // top
    emit_curb(ctx, road, &inner, outer);
    ctx.collider.strip(outer, &inner);
    ctx.collider.strip(road, &inner);
    ctx.note_road(&inner);
}

/// Vertical curb face on the `low` chain rising to `high`, each quad
/// emitted facing horizontally away from the `far` chain (toward the
/// surface the curb drops onto). The face takes the kerb-stone rows at
/// the top of the sidewalk texture (v = 0 at the sidewalk edge). The bottom
/// edge is sunk slightly below the authored vertex — the adjacent
/// surface's edge often sits a few centimetres lower, and a hairline
/// crack would otherwise show sky through the seam.
fn emit_curb(ctx: &mut EmitCtx<'_>, low: &[Vec3], high: &[Vec3], far: &[Vec3]) {
    let us = chain_u(low, high, SIDEWALK_TILE_LENGTH);
    for i in 0..low.len().saturating_sub(1) {
        let facing = {
            let d = (low[i] - far[i]) + (low[i + 1] - far[i + 1]);
            Vec3::new(d.x, 0.0, d.z).normalize_or_zero()
        };
        let b0 = low[i] - Vec3::Y * CURB_SINK;
        let b1 = low[i + 1] - Vec3::Y * CURB_SINK;
        ctx.builder(1).quad_facing(
            b0,
            b1,
            high[i + 1],
            high[i],
            [
                [us[i], CURB_V],
                [us[i + 1], CURB_V],
                [us[i + 1], 0.0],
                [us[i], 0.0],
            ],
            facing,
        );
    }
}

/// Wall quad on edge `a`–`b` rising `height` above each endpoint (sloped
/// walls follow the ground). The `tex_in`/`tex_out` texture keys select
/// the mesh group for the face pointing along `facing` and its backface
/// (always emitted — railings are alpha-cutout and must render from both
/// sides, and underpass walls are visible from above ground). UVs repeat
/// horizontally every `height` metres, v = 1 at the base.
fn emit_wall(
    ctx: &mut EmitCtx<'_>,
    a: Vec3,
    b: Vec3,
    height: f32,
    tex_in: i64,
    tex_out: i64,
    facing: Vec3,
) {
    let s = ((b - a).length() / height.max(0.1)).round().max(1.0);
    let uvs = [[0.0, 1.0], [s, 1.0], [s, 0.0], [0.0, 0.0]];
    let at = a + Vec3::Y * height;
    let bt = b + Vec3::Y * height;
    ctx.builder_at(tex_in)
        .quad_facing(a, b, bt, at, uvs, facing);
    ctx.builder_at(tex_out)
        .quad_facing(a, b, bt, at, uvs, -facing);
    ctx.collider.quad(a, b, bt, at);
}

/// Walls (and, for the wall style, a flat ceiling) along the outermost
/// chains of a road covered by a pending road-tunnel attribute. Faces
/// toward the opposite chain are the tunnel interior; texture slots are
/// relative to the tunnel attribute's own texture ref (n+0/n+1 inner
/// walls, n+3/n+4 outer faces, n+2 ceiling).
fn emit_road_tunnel(ctx: &mut EmitCtx<'_>, left: &[Vec3], right: &[Vec3]) {
    let Some(t) = *ctx.road_tunnel else {
        return;
    };
    for (chain, other, tex_in, tex_out, enabled) in [
        (left, right, 0i64, 4i64, t.flags & RoadTunnel::LEFT != 0),
        (right, left, 1, 3, t.flags & RoadTunnel::RIGHT != 0),
    ] {
        if !enabled {
            continue;
        }
        for i in 0..chain.len().saturating_sub(1) {
            let mid = (chain[i] + chain[i + 1]) * 0.5;
            let inward = {
                let d = (other[i] + other[i + 1]) * 0.5 - mid;
                Vec3::new(d.x, 0.0, d.z).normalize_or_zero()
            };
            emit_wall(
                ctx,
                chain[i],
                chain[i + 1],
                t.height,
                t.tex_key + tex_in,
                t.tex_key + tex_out,
                inward,
            );
        }
    }
    let ceiling = t.flags & (RoadTunnel::FLAT_CEILING | RoadTunnel::CURVED_CEILING);
    if ceiling != 0 {
        // Flat: one span between the wall tops. Curved: approximated as a
        // ridge rising to `height2` over the midline.
        let top = |c: &[Vec3]| -> Vec<Vec3> { c.iter().map(|p| *p + Vec3::Y * t.height).collect() };
        let (lt, rt) = (top(left), top(right));
        let spans: Vec<(Vec<Vec3>, Vec<Vec3>)> = if ceiling & RoadTunnel::CURVED_CEILING != 0 {
            let ridge: Vec<Vec3> = left
                .iter()
                .zip(right)
                .map(|(l, r)| (*l + *r) * 0.5 + Vec3::Y * t.height2.max(t.height))
                .collect();
            vec![(lt, ridge.clone()), (ridge, rt)]
        } else {
            vec![(lt, rt)]
        };
        for (a, b) in &spans {
            for i in 0..a.len().min(b.len()).saturating_sub(1) {
                let quad = [a[i], a[i + 1], b[i + 1], b[i]];
                ctx.builder_at(t.tex_key + 2).quad_facing(
                    quad[0],
                    quad[1],
                    quad[2],
                    quad[3],
                    quad.map(MeshBuilder::planar_uv),
                    Vec3::NEG_Y,
                );
                ctx.collider.quad(quad[0], quad[1], quad[2], quad[3]);
            }
        }
    }
}

/// Divider geometry for a divided road. `div_tex` is the resolved texture
/// index (−1 when the packed byte was 0). Types per the format docs:
/// 0 invisible (collision bound only), 1 flat, 2 elevated, 3 wedged.
/// Elevated/wedged geometry is approximated and counted in the report;
/// both ends are always closed (the cap flags are not interpreted).
/// `value` is 8.8 fixed point (verified on retail data: 38 → the 0.15 m
/// kerb height, 256 → the 1 m jersey barrier, 1280 → 5 repeats).
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
    let value = value as f32 / 256.0;
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
                        [value.max(1.0), 0.0],
                        [value.max(1.0), 1.0],
                        [0.0, 1.0],
                    ],
                    Vec3::Y,
                );
            }
            ctx.collider.strip(rl_in, rr_in);
        }
        3 => {
            // Wedge (jersey barrier): two faces sloping from the inner
            // road edges up to a ridge `value` metres above the midline,
            // texture n+1 with u along the barrier. (Real wedges have a
            // flat 0.5 m-inset top; the ridge is an approximation.)
            let h = value.max(0.05);
            let ridge: Vec<Vec3> = rl_in
                .iter()
                .zip(rr_in)
                .map(|(l, r)| (*l + *r) * 0.5 + Vec3::Y * h)
                .collect();
            let us = chain_u(rl_in, rr_in, SIDEWALK_TILE_LENGTH);
            for (base, sign) in [(rl_in, 1.0), (rr_in, -1.0)] {
                for i in 0..n - 1 {
                    let out = (rl_in[i] - rr_in[i]).normalize_or_zero() * sign;
                    ctx.builder_at(dt(1)).quad_facing(
                        base[i],
                        base[i + 1],
                        ridge[i + 1],
                        ridge[i],
                        [
                            [us[i], 1.0],
                            [us[i + 1], 1.0],
                            [us[i + 1], 0.0],
                            [us[i], 0.0],
                        ],
                        out + Vec3::Y,
                    );
                }
                ctx.collider.strip(base, &ridge);
            }
            // Close both ends: the road surface does not continue under
            // the divider, so an open end shows the void beneath.
            for (e, along) in [
                (0, rl_in[0] - rl_in[1]),
                (n - 1, rl_in[n - 1] - rl_in[n - 2]),
            ] {
                ctx.builder_at(dt(1)).tri_facing(
                    [rl_in[e], rr_in[e], ridge[e]],
                    [[0.0, 1.0], [1.0, 1.0], [0.5, 0.0]],
                    along,
                );
                ctx.collider.tri(rl_in[e], rr_in[e], ridge[e]);
            }
            ctx.report.approximated += 1;
        }
        _ => {
            // Elevated: a raised median strip `value` metres high — vertical
            // kerb sides (texture n), flat top (n+2), closed ends.
            let h = value.max(0.05);
            let top_l: Vec<Vec3> = rl_in.iter().map(|v| *v + Vec3::Y * h).collect();
            let top_r: Vec<Vec3> = rr_in.iter().map(|v| *v + Vec3::Y * h).collect();
            let side_tex = dt(0);
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
                // Top strip, planar-mapped like other ground (grass).
                let top = [top_l[i], top_r[i], top_r[i + 1], top_l[i + 1]];
                ctx.builder_at(dt(2)).quad_facing(
                    top[0],
                    top[1],
                    top[2],
                    top[3],
                    top.map(MeshBuilder::planar_uv),
                    Vec3::Y,
                );
            }
            for (e, along) in [
                (0, rl_in[0] - rl_in[1]),
                (n - 1, rl_in[n - 1] - rl_in[n - 2]),
            ] {
                ctx.builder_at(side_tex).quad_facing(
                    rl_in[e],
                    rr_in[e],
                    top_r[e],
                    top_l[e],
                    [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
                    along,
                );
                ctx.collider.quad(rl_in[e], rr_in[e], top_r[e], top_l[e]);
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
    // Glancing-angle surfaces (roads) smear badly without anisotropy; it
    // requires all-Linear filters.
    let mut sampler = ImageSamplerDescriptor::linear();
    sampler.anisotropy_clamp = 16;
    sampler.address_mode_u = if bits & 0x01 != 0 {
        ImageAddressMode::ClampToEdge
    } else {
        ImageAddressMode::Repeat
    };
    sampler.address_mode_v = if bits & 0x1_0000 != 0 {
        ImageAddressMode::ClampToEdge
    } else {
        ImageAddressMode::Repeat
    };
    image.sampler = ImageSampler::Descriptor(sampler);
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
    // City surfaces tile; repeat + anisotropy is the pipeline default.
    let mut sampler = ImageSamplerDescriptor::linear();
    sampler.anisotropy_clamp = 16;
    sampler.address_mode_u = ImageAddressMode::Repeat;
    sampler.address_mode_v = ImageAddressMode::Repeat;
    image.sampler = ImageSampler::Descriptor(sampler);
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

/// Load the frames of an animated texture sequence: MM2 stores animated
/// surfaces (water: `s_thames`, `s_pond`, `s_ocean`) as numbered frames
/// `<stem>-0001`, `<stem>-0002`, … with no plain `<stem>` texture.
pub fn load_image_sequence(vfs: &Vfs, stem: &str) -> Vec<(Image, bool)> {
    (1..)
        .map_while(|i| load_image(vfs, &format!("{stem}-{i:04}")))
        .collect()
}

/// Cycles a material's base colour texture through a frame sequence.
#[derive(Component)]
pub struct AnimatedTexture {
    material: Handle<StandardMaterial>,
    frames: Vec<Handle<Image>>,
}

/// Advance every [`AnimatedTexture`] to the frame for the current time.
pub fn animate_textures(
    time: Res<Time>,
    animated: Query<&AnimatedTexture>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let tick = (time.elapsed_secs() * TEXTURE_ANIM_FPS) as usize;
    for anim in &animated {
        let frame = &anim.frames[tick % anim.frames.len()];
        // Only touch the asset on a frame change: mutable access marks it
        // modified and re-prepares the material.
        if materials
            .get(&anim.material)
            .is_some_and(|m| m.base_color_texture.as_ref() != Some(frame))
            && let Some(mut mat) = materials.get_mut(&anim.material)
        {
            mat.base_color_texture = Some(frame.clone());
        }
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
    /// Materials backed by a frame sequence, to be animated once spawned.
    animated: Vec<AnimatedTexture>,
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
            animated: Vec::new(),
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
        let mut frames = match load_image(self.vfs, &key) {
            Some(single) => vec![single],
            None => load_image_sequence(self.vfs, &key),
        };
        let mat = match frames.first().map(|f| f.1) {
            Some(has_alpha) => {
                let frames: Vec<Handle<Image>> =
                    frames.drain(..).map(|f| self.images.add(f.0)).collect();
                // Alpha follows the decoded pixels: textures carrying real
                // transparency (trees, fences) become alpha-cutout — the
                // conservative choice that avoids blend sorting — and
                // everything else stays opaque.
                let material = self.materials.add(StandardMaterial {
                    base_color_texture: Some(frames[0].clone()),
                    alpha_mode: if has_alpha {
                        AlphaMode::Mask(0.5)
                    } else {
                        AlphaMode::Opaque
                    },
                    perceptual_roughness: 0.95,
                    ..default()
                });
                if frames.len() > 1 {
                    self.animated.push(AnimatedTexture {
                        material: material.clone(),
                        frames,
                    });
                }
                material
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

    /// Material for one PKG shader record: resolves its texture through the
    /// VFS (mods included), then applies the shader's tint/alpha. Distinct
    /// shader records get distinct material handles even when they share a
    /// texture, which is what makes paint-job selection work.
    pub fn shader_material(&mut self, s: &mm2_formats::pkg::PkgShader) -> Handle<StandardMaterial> {
        let base = self.get(&s.texture);
        adjust_material(s, &base, self.materials).unwrap_or(base)
    }

    /// Texture stems that could not be resolved while building materials.
    pub fn missing_textures(&self) -> &BTreeSet<String> {
        &self.missing
    }

    /// The plain untextured fallback material.
    pub fn fallback(&self) -> Handle<StandardMaterial> {
        self.fallback.clone()
    }

    /// Clone the material behind `base`, apply `f`, and register the result
    /// as a new material. Used for per-part adjustments (glow parts, tints)
    /// without disturbing the shared cache entry.
    pub fn adjusted(
        &mut self,
        base: &Handle<StandardMaterial>,
        f: impl FnOnce(&mut StandardMaterial),
    ) -> Handle<StandardMaterial> {
        let mut mat = self.materials.get(base).cloned().unwrap_or_default();
        f(&mut mat);
        self.materials.add(mat)
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

/// Collision triangles accumulated over a whole prop (all best-LOD
/// geometries, every section). Props are static, so their collision is the
/// authored triangle mesh: a convex hull would seal the openings of the
/// concave props the city is full of — archways, bridge trusses, tunnel
/// mouths — turning them into invisible walls and floors.
#[derive(Default)]
struct PropCollision {
    positions: Vec<Vec3>,
    tris: Vec<[u32; 3]>,
}

impl PropCollision {
    fn into_collider(self) -> Option<Collider> {
        if self.positions.is_empty() || self.tris.is_empty() {
            return None;
        }
        Some(Collider::trimesh(self.positions, self.tris))
    }
}

/// Build renderable parts (best-LOD mesh per stem, grouped by shader) plus
/// one triangle-mesh collider covering the whole prop.
fn pkg_to_parts(
    pkg: &Pkg,
    mats: &mut MaterialCache<'_>,
    meshes: &mut Assets<Mesh>,
    missing_prims: &mut usize,
) -> PropModel {
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
    let mut collision = PropCollision::default();
    for (stem, (_, name)) in best {
        // Shadow/damage stand-ins are not rendered props.
        if stem.contains("shadow") || stem.contains("dmg") {
            continue;
        }
        let Some((_, geo)) = pkg.geometries().find(|(n, _)| *n == name) else {
            continue;
        };
        let mut by_shader: HashMap<i32, MeshBuilder> = HashMap::new();
        for section in &geo.sections {
            let b = by_shader.entry(section.shader_offset).or_default();
            for strip in &section.strips {
                if strip.prim_type != mm2_formats::pkg::PRIMTYPE_TRIANGLES {
                    // Only triangle lists are interpreted; other primitive
                    // types are reported, never guessed at.
                    *missing_prims += 1;
                    continue;
                }
                emit_strip(b, strip, &mut collision);
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
            out.push((meshes.add(builder.build()), mat));
        }
    }
    PropModel {
        parts: out,
        collider: collision.into_collider(),
    }
}

/// Emit one PKG strip into a builder; authored normals and UVs preserved.
/// The same triangles are accumulated into `col` for collision (winding is
/// irrelevant to the physics backend).
fn emit_strip(b: &mut MeshBuilder, strip: &PkgStrip, col: &mut PropCollision) {
    let base = b.positions.len() as u32;
    let col_base = col.positions.len() as u32;
    for v in &strip.vertices {
        let p = v3(v.position);
        // PKG UVs are authored against TEX's bottom-up row order, which
        // `decode_rgba` now normalises to top-down, so v is complemented.
        // Unlike the city's walls, whose UVs this crate generates, these
        // come from the file and cannot simply adopt the new convention.
        let uv = v
            .tex_coords
            .first()
            .map(|&[u, v]| [u, 1.0 - v])
            .unwrap_or([0.0, 0.0]);
        match v.normal {
            Some(n) => {
                b.vert_n(p, uv, v3(n));
            }
            None => {
                b.vert(p, uv);
            }
        }
        col.positions.push(p);
    }
    for t in strip.indices.chunks_exact(3) {
        b.tri(base + t[0] as u32, base + t[1] as u32, base + t[2] as u32);
        col.tris.push([
            col_base + t[0] as u32,
            col_base + t[1] as u32,
            col_base + t[2] as u32,
        ]);
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

/// Everything prepared from one PKG: the renderable parts and the single
/// collider shared by every placement of that prop.
struct PropModel {
    parts: Vec<(Handle<Mesh>, Handle<StandardMaterial>)>,
    collider: Option<Collider>,
}

/// Cache of PKG name → prepared meshes+materials handles.
struct PropCache<'a> {
    vfs: &'a Vfs,
    meshes: &'a mut Assets<Mesh>,
    mats: MaterialCache<'a>,
    cache: HashMap<String, Option<PropModel>>,
    /// Strips with unsupported primitive types encountered while building.
    missing_prims: usize,
}

impl<'a> PropCache<'a> {
    fn get(&mut self, name: &str) -> Option<&PropModel> {
        let key = name.to_ascii_lowercase();
        if !self.cache.contains_key(&key) {
            let built = self.build(&key);
            self.cache.insert(key.clone(), built);
        }
        self.cache.get(&key).and_then(|o| o.as_ref())
    }

    fn build(&mut self, name: &str) -> Option<PropModel> {
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
        let model = pkg_to_parts(&pkg, &mut self.mats, self.meshes, &mut self.missing_prims);
        if model.parts.is_empty() {
            return None;
        }
        Some(model)
    }
}

/// Convert an INST coordinate placement to a Bevy `Mat4` in mirrored space.
fn inst_transform(c: &inst::InstCoordinate) -> Mat4 {
    // When mirroring, the placement is re-expressed in the mirrored frame
    // as M' = S·M·S with S = diag(1,1,-1): mirror each column's z, then
    // negate the whole z column, and mirror the origin. Without the mirror
    // the authored basis is used as-is.
    let x = v3(c.x_axis);
    let y = v3(c.y_axis);
    let z = if MIRROR_Z {
        -v3(c.z_axis)
    } else {
        v3(c.z_axis)
    };
    let o = v3(c.origin);
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), o.extend(1.0))
}

/// Convert an INST simple placement to a Bevy `Mat4`. The heading vector
/// is the image of the PKG's X axis — an unrotated object has `(1, 0)`
/// (verified on retail London: `wl_buckpalace_l`'s fence matches its room
/// perimeter only with this reading) — and its length the uniform scale.
fn simple_transform(s: &inst::InstSimple) -> Mat4 {
    let (dx, dz) = (s.x_delta, s.z_delta);
    let scale = (dx * dx + dz * dz).sqrt().max(0.001);
    inst_transform(&inst::InstCoordinate {
        x_axis: [dx, 0.0, dz],
        y_axis: [0.0, scale, 0.0],
        z_axis: [-dz, 0.0, dx],
        origin: s.location,
    })
}

// ---------------------------------------------------------------------------
// Orchestration
// ---------------------------------------------------------------------------

/// Result of loading a city: where to put the player plus the import
/// report for diagnostics.
pub struct LoadedCity {
    /// Validated spawn point on road geometry.
    pub spawn: Vec3,
    /// Heading along the road at the spawn point.
    pub spawn_yaw: f32,
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

    // INST placements → PKG props. Collision is the prop's own triangle
    // mesh, spawned once per placement beside its visual parts.
    let animated;
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
                    let Some(model) = cache.get(&comp.package_name) else {
                        report.props_failed += 1;
                        continue;
                    };
                    let mat4 = match &comp.placement {
                        InstPlacement::Coordinate(c) => inst_transform(c),
                        InstPlacement::Simple(s) => simple_transform(s),
                    };
                    let transform = Transform::from_matrix(mat4);
                    for (mesh, material) in &model.parts {
                        commands.spawn((
                            CityEntity,
                            Mesh3d(mesh.clone()),
                            MeshMaterial3d(material.clone()),
                            transform,
                            Name::new(format!("prop-{}", comp.package_name)),
                        ));
                    }
                    // One static body per placement — not one per shader
                    // group, which used to stack duplicate colliders.
                    if let Some(collider) = &model.collider {
                        commands.spawn((
                            CityEntity,
                            RigidBody::Static,
                            collider.clone(),
                            transform,
                            Name::new(format!("prop-{}-collider", comp.package_name)),
                        ));
                    }
                    report.props_spawned += 1;
                }
                if cache.missing_prims > 0 {
                    report
                        .unsupported
                        .insert("pkg-non-triangle-strips".into(), cache.missing_prims);
                }
                report.missing_textures = std::mem::take(&mut cache.mats.missing);
                animated = std::mem::take(&mut cache.mats.animated);
                info!(path = %inst_res.logical, props = report.props_spawned, "inst props spawned");
            }
            Err(e) => {
                warn!(path = %inst_res.logical, error = %e, "INST parse failed");
                report.missing_textures = std::mem::take(&mut mats.missing);
                animated = std::mem::take(&mut mats.animated);
            }
        },
        Err(_) => {
            debug!(path = %inst_path, "no INST file; skipping props");
            report.missing_textures = std::mem::take(&mut mats.missing);
            animated = std::mem::take(&mut mats.animated);
        }
    }
    for anim in animated {
        commands.spawn((CityEntity, anim));
    }
    info!(report = %report, "city import");

    Ok(LoadedCity {
        spawn: import.spawn,
        spawn_yaw: import.spawn_yaw,
        report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_placement_with_unit_heading_is_unrotated() {
        let m = simple_transform(&inst::InstSimple {
            x_delta: 1.0,
            z_delta: 0.0,
            location: [10.0, 2.0, 5.0],
        });
        // PKG-local points are mirrored like everything else: (x, y, -z).
        let p = m.transform_point3(v3([3.0, 0.0, 4.0]));
        assert!((p - v3([13.0, 2.0, 9.0])).length() < 1e-5);
    }

    #[test]
    fn simple_placement_heading_rotates_and_scales() {
        let m = simple_transform(&inst::InstSimple {
            x_delta: 0.0,
            z_delta: 2.0,
            location: [0.0, 0.0, 0.0],
        });
        // Local +X maps onto authored +Z, scaled by the heading length.
        let p = m.transform_point3(v3([1.0, 1.0, 0.0]));
        assert!((p - v3([0.0, 2.0, 2.0])).length() < 1e-5);
    }

    #[test]
    fn road_surface_mirrors_texture_about_the_centre_line() {
        let l = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, -20.0)];
        let r = [Vec3::new(8.0, 0.0, 0.0), Vec3::new(8.0, 0.0, -20.0)];
        let mut b = MeshBuilder::default();
        let mid: Vec<Vec3> = l.iter().zip(&r).map(|(a, b)| (*a + *b) * 0.5).collect();
        let us = chain_u(&l, &r, ROAD_TILE_LENGTH);
        b.strip_uv(&l, &mid, &us, 1.0, 0.0);
        b.strip_uv(&mid, &r, &us, 0.0, 1.0);
        assert_eq!(us, vec![0.0, 2.0]);
        for (p, uv) in b.positions.iter().zip(&b.uvs) {
            // v = 1 on both kerbs, 0 on the centre line.
            assert!((uv[1] - (p[0] - 4.0).abs() / 4.0).abs() < 1e-6);
        }
    }
}
