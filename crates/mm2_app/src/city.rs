//! Import of MM2 city content (PSDL geometry, INST placements, PKG props,
//! TEX textures) through the VFS into Bevy meshes/materials.
//!
//! Coordinate convention: MM2 data is treated as left-handed (Direct3D-era).
//! Conversion to Bevy's right-handed frame mirrors Z (`x, y, -z`) and flips
//! triangle winding. This is *inferred* — if a city renders mirrored, flip
//! [`mirror_z`].
//!
//! PSDL room attributes are a partially documented format; the emitters below
//! cover the verified subset and document the approximations inline. Unknown
//! or ambiguous attributes are skipped, not guessed.

use std::collections::HashMap;

use avian3d::prelude::*;
use bevy::{
    asset::RenderAssetUsages,
    image::{CompressedImageFormats, ImageSampler, ImageType},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use mm2_assets::{Resolved, Vfs};
use mm2_formats::{
    inst::{self, InstPlacement},
    pkg::Pkg,
    psdl::{AttributeType, Psdl},
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

/// Sidewalks sit this far above road vertices (documented in Room_attributes).
const SIDEWALK_LIFT: f32 = 0.15;

#[inline]
fn v3(p: [f32; 3]) -> Vec3 {
    if MIRROR_Z {
        Vec3::new(p[0], p[1], -p[2])
    } else {
        Vec3::new(p[0], p[1], p[2])
    }
}

/// Accumulates triangles for one material/texture group.
#[derive(Default)]
struct MeshBuilder {
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl MeshBuilder {
    fn vert(&mut self, p: Vec3, uv: [f32; 2]) -> u32 {
        let i = self.positions.len() as u32;
        self.positions.push(p.to_array());
        self.uvs.push(uv);
        i
    }

    /// Positions are already mirrored; push with flipped winding so faces
    /// remain front-facing after the Z mirror.
    fn tri(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend_from_slice(&[a, c, b]);
    }

    /// Planar UVs for ground-like surfaces.
    fn planar_uv(p: Vec3) -> [f32; 2] {
        [p.x / PLANAR_UV_SCALE, p.z / PLANAR_UV_SCALE]
    }

    /// Triangle fan from world positions with planar UVs.
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

    /// Flat strip between left/right vertex chains.
    fn strip(&mut self, left: &[Vec3], right: &[Vec3]) {
        if left.len() < 2 {
            return;
        }
        for i in 0..left.len() - 1 {
            let l0 = self.vert(left[i], Self::planar_uv(left[i]));
            let l1 = self.vert(left[i + 1], Self::planar_uv(left[i + 1]));
            let r0 = self.vert(right[i], Self::planar_uv(right[i]));
            let r1 = self.vert(right[i + 1], Self::planar_uv(right[i + 1]));
            self.tri(l0, r0, l1);
            self.tri(l1, r0, r1);
        }
    }

    /// Vertical quad between two bottom positions and a top height, with
    /// repeat UVs.
    fn wall_quad(&mut self, l: Vec3, r: Vec3, bottom: f32, top: f32, u_rep: f32, v_rep: f32) {
        let bl = self.vert(Vec3::new(l.x, bottom, l.z), [0.0, 0.0]);
        let br = self.vert(Vec3::new(r.x, bottom, r.z), [u_rep, 0.0]);
        let tr = self.vert(Vec3::new(r.x, top, r.z), [u_rep, v_rep]);
        let tl = self.vert(Vec3::new(l.x, top, l.z), [0.0, v_rep]);
        self.tri(bl, br, tr);
        self.tri(bl, tr, tl);
    }

    fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    fn build(self, smooth_normals: bool) -> (Mesh, Vec<[u32; 3]>) {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.positions.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs);
        mesh.insert_indices(Indices::U32(self.indices.clone()));
        if smooth_normals {
            mesh.compute_smooth_normals();
        } else {
            mesh.compute_normals();
        }
        let tris = self
            .indices
            .chunks_exact(3)
            .map(|c| [c[0], c[1], c[2]])
            .collect();
        (mesh, tris)
    }
}

/// Result of loading a city: where to put the player.
pub struct LoadedCity {
    /// Suggested vehicle spawn point (above the city centre).
    pub spawn: Vec3,
}

/// Resolve an MM2 texture name to a Bevy `Image`, going through the logical
/// VFS lookup so mods can supply `png`/`ktx2`/`tga` replacements for `.tex`.
fn load_image(vfs: &Vfs, stem: &str, images: &mut Assets<Image>) -> Option<Handle<Image>> {
    const EXTS: &[&str] = &["png", "ktx2", "tga", "tex"];
    let resolved: Resolved = vfs.resolve_preferred(&format!("texture/{stem}"), EXTS)?;
    let bytes = match vfs.read(&resolved) {
        Ok(b) => b,
        Err(e) => {
            warn!(logical = %resolved.logical, error = %e, "failed to read texture");
            return None;
        }
    };
    let ext = resolved.logical.rsplit('.').next().unwrap_or_default();
    let image = if ext == "tex" {
        let tex = match TexFile::parse(&bytes) {
            Ok(t) => t,
            Err(e) => {
                warn!(logical = %resolved.logical, error = %e, "failed to parse TEX");
                return None;
            }
        };
        let Some(rgba) = tex.decode_rgba(0) else {
            warn!(logical = %resolved.logical, "TEX decode failed");
            return None;
        };
        Image::new(
            Extent3d {
                width: tex.header.width as u32,
                height: tex.header.height as u32,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            rgba,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        )
    } else {
        match Image::from_buffer(
            &bytes,
            ImageType::Extension(ext),
            CompressedImageFormats::NONE,
            true,
            ImageSampler::Default,
            RenderAssetUsages::default(),
        ) {
            Ok(img) => img,
            Err(e) => {
                warn!(logical = %resolved.logical, error = %e, "failed to decode image");
                return None;
            }
        }
    };
    Some(images.add(image))
}

/// Cache of name → material for the whole city.
struct MaterialCache<'a> {
    vfs: &'a Vfs,
    images: &'a mut Assets<Image>,
    materials: &'a mut Assets<StandardMaterial>,
    by_name: HashMap<String, Handle<StandardMaterial>>,
    fallback: Handle<StandardMaterial>,
}

impl<'a> MaterialCache<'a> {
    fn new(
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
            by_name: HashMap::new(),
            fallback,
        }
    }

    /// Material for an MM2 texture base name (e.g. `road_london`). Empty or
    /// unresolvable names get the fallback.
    fn get(&mut self, name: &str) -> Handle<StandardMaterial> {
        let key = name.to_ascii_lowercase();
        if key.is_empty() {
            return self.fallback.clone();
        }
        if let Some(m) = self.by_name.get(&key) {
            return m.clone();
        }
        let mat = match load_image(self.vfs, &key, self.images) {
            Some(tex) => {
                // Conservative defaults: MM2 textures only carry diffuse.
                // Use alpha-cutout when the texture actually has alpha so
                // trees/fences work without shipping blend sorting problems.
                let has_alpha = self
                    .images
                    .get(&tex)
                    .map(|img| {
                        img.data
                            .as_ref()
                            .map(|d| d.chunks_exact(4).any(|px| px[3] < 250))
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);
                self.materials.add(StandardMaterial {
                    base_color_texture: Some(tex),
                    alpha_mode: if has_alpha {
                        AlphaMode::Mask(0.5)
                    } else {
                        AlphaMode::Opaque
                    },
                    perceptual_roughness: 0.95,
                    cull_mode: None,
                    ..default()
                })
            }
            None => {
                debug!(texture = %key, "texture not found; using fallback");
                self.fallback.clone()
            }
        };
        self.by_name.insert(key, mat.clone());
        mat
    }
}

/// Decode the texture index a `TextureRef` attribute selects.
/// `None` = render with the fallback material; `0` (encoded) suppresses
/// rendering entirely per the format docs.
fn texture_index(attr_data: &[u16], subtype: u8) -> Option<Option<usize>> {
    let raw = attr_data.first().copied().unwrap_or(0) as usize + (subtype as usize) * 256;
    if raw == 0 {
        return None; // suppress rendering
    }
    Some(Some(raw - 1))
}

/// Build Bevy meshes for all rooms of a parsed PSDL. Returns spawned
/// entity count.
fn build_city_geometry(
    commands: &mut Commands,
    psdl: &Psdl,
    meshes: &mut Assets<Mesh>,
    mats: &mut MaterialCache<'_>,
) -> usize {
    let verts: Vec<Vec3> = psdl.vertices.iter().map(|&p| v3(p)).collect();
    let mut groups: HashMap<Option<usize>, MeshBuilder> = HashMap::new();
    let mut cur_tex: Option<usize> = None; // None = suppress
    let mut suppressed = false;

    for room in &psdl.rooms {
        for attr in &room.attributes {
            if attr.kind == AttributeType::TextureRef {
                match texture_index(&attr.data, attr.subtype) {
                    Some(idx) => {
                        cur_tex = idx;
                        suppressed = false;
                    }
                    None => suppressed = true,
                }
                continue;
            }
            if suppressed {
                continue;
            }
            let key = Some(cur_tex.unwrap_or(usize::MAX));
            let b = groups.entry(key).or_default();
            emit_attribute(b, attr, &verts, &psdl.heights);
        }
    }

    let mut count = 0;
    for (tex_idx, builder) in groups {
        if builder.is_empty() {
            continue;
        }
        let material = match tex_idx {
            Some(usize::MAX) | None => mats.fallback.clone(),
            Some(i) => mats.get(&psdl.textures.get(i).cloned().unwrap_or_default()),
        };
        let (mesh, tris) = builder.build(true);
        let positions: Vec<Vec3> = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|a| a.as_float3())
            .map(|f| f.iter().map(|v| Vec3::from(*v)).collect())
            .unwrap_or_default();
        let collider = if positions.len() >= 3 {
            Some(Collider::trimesh(positions, tris))
        } else {
            None
        };
        let mut e = commands.spawn((
            CityEntity,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            RigidBody::Static,
            Name::new(format!(
                "city-{}",
                tex_idx.map(|i| i.to_string()).unwrap_or("none".into())
            )),
        ));
        if let Some(c) = collider {
            e.insert(c);
        }
        count += 1;
    }
    count
}

/// Emit one room attribute's triangles into `b`.
fn emit_attribute(
    b: &mut MeshBuilder,
    attr: &mm2_formats::psdl::RoomAttribute,
    verts: &[Vec3],
    heights: &[f32],
) {
    let d = &attr.data;
    let get_v = |i: u16| verts.get(i as usize).copied();

    match attr.kind {
        AttributeType::Fan | AttributeType::RoadFan => {
            let pts: Vec<Vec3> = d.iter().filter_map(|&i| get_v(i)).collect();
            b.fan(&pts);
        }
        AttributeType::RoofFan => {
            // data = [heightRef, v0, v1, ...]; all y := heights[heightRef]
            let Some(&h) = d.first().and_then(|&h| heights.get(h as usize)) else {
                return;
            };
            let pts: Vec<Vec3> = d[1..]
                .iter()
                .filter_map(|&i| get_v(i))
                .map(|p| Vec3::new(p.x, h, p.z))
                .collect();
            b.fan(&pts);
        }
        AttributeType::RoadWithSidewalks => {
            // 4 verts per section: [sw_l, road_l, road_r, sw_r]
            let n = d.len() / 4;
            let mut road_l = Vec::with_capacity(n);
            let mut road_r = Vec::with_capacity(n);
            let mut sw_l = Vec::with_capacity(n);
            let mut sw_r = Vec::with_capacity(n);
            for s in 0..n {
                let Some([a, c, e, f]) = d.get(s * 4..s * 4 + 4).map(|w| [w[0], w[1], w[2], w[3]])
                else {
                    continue;
                };
                let (Some(a), Some(c), Some(e), Some(f)) = (get_v(a), get_v(c), get_v(e), get_v(f))
                else {
                    continue;
                };
                sw_l.push(a);
                road_l.push(c);
                road_r.push(e);
                sw_r.push(f);
            }
            // Road surface between the two road edges.
            b.strip(&road_l, &road_r);
            // Sidewalk tops: outer edge at authored height, inner edge
            // lifted SIDEWALK_LIFT above the road vertex.
            let lift = |v: &Vec3| Vec3::new(v.x, v.y + SIDEWALK_LIFT, v.z);
            let sw_l_inner: Vec<Vec3> = road_l.iter().map(lift).collect();
            let sw_r_inner: Vec<Vec3> = road_r.iter().map(lift).collect();
            b.strip(&sw_l, &sw_l_inner);
            b.strip(&sw_r_inner, &sw_r);
        }
        AttributeType::SidewalkStrip | AttributeType::RoadNoSidewalks => {
            let n = d.len() / 2;
            let mut l = Vec::with_capacity(n);
            let mut r = Vec::with_capacity(n);
            for s in 0..n {
                if let (Some(a), Some(c)) = (get_v(d[s * 2]), get_v(d[s * 2 + 1])) {
                    l.push(a);
                    r.push(c);
                }
            }
            b.strip(&l, &r);
        }
        AttributeType::DividedRoad => {
            // 6 verts per section: [sw_l, rl_out, rl_in, rr_in, rr_out, sw_r]
            // plus two trailing words (flags/value).
            let n = (d.len().saturating_sub(2)) / 6;
            let mut rl_out = Vec::with_capacity(n);
            let mut rl_in = Vec::with_capacity(n);
            let mut rr_in = Vec::with_capacity(n);
            let mut rr_out = Vec::with_capacity(n);
            let mut sw_l = Vec::with_capacity(n);
            let mut sw_r = Vec::with_capacity(n);
            for s in 0..n {
                let base = s * 6;
                let idx: Vec<Vec3> = d[base..base + 6].iter().filter_map(|&i| get_v(i)).collect();
                if idx.len() < 6 {
                    continue;
                }
                sw_l.push(idx[0]);
                rl_out.push(idx[1]);
                rl_in.push(idx[2]);
                rr_in.push(idx[3]);
                rr_out.push(idx[4]);
                sw_r.push(idx[5]);
            }
            b.strip(&rl_out, &rl_in);
            b.strip(&rr_in, &rr_out);
            let lift = |v: &Vec3| Vec3::new(v.x, v.y + SIDEWALK_LIFT, v.z);
            b.strip(&sw_l, &rl_out.iter().map(lift).collect::<Vec<_>>());
            b.strip(&rr_out.iter().map(lift).collect::<Vec<_>>(), &sw_r);
            // Median strip between the two inner road edges, slightly raised.
            let med_l: Vec<Vec3> = rl_in.iter().map(lift).collect();
            let med_r: Vec<Vec3> = rr_in.iter().map(lift).collect();
            b.strip(&med_l, &med_r);
        }
        AttributeType::Crosswalk => {
            // Four corner verts; emit as a quad (ordering approximated).
            if d.len() >= 4 {
                let pts: Vec<Vec3> = d[..4].iter().filter_map(|&i| get_v(i)).collect();
                if pts.len() == 4 {
                    b.fan(&[pts[0], pts[1], pts[3], pts[2]]);
                }
            }
        }
        AttributeType::Sliver => {
            // data = [vLeft, vRight, heightTop]: bottom follows the ground
            // verts, top is horizontal at heights[heightTop].
            if d.len() >= 3
                && let (Some(l), Some(r), Some(&top)) =
                    (get_v(d[0]), get_v(d[1]), heights.get(d[2] as usize))
            {
                b.wall_quad(l, r, l.y.min(r.y), top, 1.0, 1.0);
            }
        }
        AttributeType::Facade => {
            // data = [vLeft, vRight, hBottom, hTop, uRepeat, vRepeat]
            if d.len() >= 6
                && let (Some(l), Some(r), Some(&hb), Some(&ht)) = (
                    get_v(d[0]),
                    get_v(d[1]),
                    heights.get(d[2] as usize),
                    heights.get(d[3] as usize),
                )
            {
                b.wall_quad(l, r, hb, ht, d[4] as f32, d[5] as f32);
            }
        }
        AttributeType::FacadeBound | AttributeType::Tunnel | AttributeType::TextureRef => {
            // FacadeBound is collision-only (approximated by the facade mesh);
            // Tunnel parameters aren't decoded yet; TextureRef handled by caller.
        }
        AttributeType::Unknown(_) => {}
    }
}

/// Build a renderable model (best LOD mesh + material) from a parsed PKG.
/// Returns meshes keyed by nothing — a flat merged mesh per LOD group is
/// enough for props in this iteration.
fn pkg_to_meshes(pkg: &Pkg, mats: &mut MaterialCache<'_>) -> Vec<(Mesh, Handle<StandardMaterial>)> {
    // Group geometry chunks by stem (name minus LOD suffix); keep best LOD.
    let mut best: HashMap<String, (u8, &str)> = HashMap::new();
    let lod_rank = |name: &str| -> u8 {
        let lower = name.to_ascii_lowercase();
        match lower.rsplit('_').next().unwrap_or("") {
            "vl" => 0,
            "l" => 1,
            "m" => 2,
            "h" => 3,
            _ => 3,
        }
    };
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
    // Skip shadow/damage chunks — named *_SHADOW* / DAMAGE variants observed.
    let skip = |stem: &str| stem.contains("shadow") || stem.contains("dmg");

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
        if skip(&stem) {
            continue;
        }
        let Some((_, geo)) = pkg.geometries().find(|(n, _)| *n == name) else {
            continue;
        };
        // Merge all sections into one mesh per material.
        let mut by_shader: HashMap<i32, MeshBuilder> = HashMap::new();
        for section in &geo.sections {
            let b = by_shader.entry(section.shader_offset).or_default();
            for strip in &section.strips {
                // Positions/UVs mirrored into Bevy space.
                let base = b.positions.len() as u32;
                for v in &strip.vertices {
                    let p = v3(v.position);
                    let uv = v.tex_coords.first().copied().unwrap_or([0.0, 0.0]);
                    b.vert(p, uv);
                }
                for t in strip.indices.chunks_exact(3) {
                    b.tri(base + t[0] as u32, base + t[1] as u32, base + t[2] as u32);
                }
            }
        }
        for (shader_off, builder) in by_shader {
            if builder.is_empty() {
                continue;
            }
            let mat = match shader_at(shader_off) {
                Some(s) => {
                    let m = mats.get(&s.texture);
                    // Apply diffuse tint/alpha when the shader asks for it.
                    self_material(s, &m, mats.materials).unwrap_or(m)
                }
                None => mats.fallback.clone(),
            };
            let (mesh, _tris) = builder.build(true);
            out.push((mesh, mat));
        }
    }
    out
}

/// If the shader wants a non-opaque or tinted material, clone and adjust.
fn self_material(
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

/// Mesh + material pairs prepared from one PKG.
type PropParts = Vec<(Handle<Mesh>, Handle<StandardMaterial>)>;

/// Cache of PKG name → prepared meshes+materials handles.
struct PropCache<'a> {
    vfs: &'a Vfs,
    meshes: &'a mut Assets<Mesh>,
    mats: MaterialCache<'a>,
    cache: HashMap<String, Option<PropParts>>,
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
        let parts = pkg_to_meshes(&pkg, &mut self.mats);
        if parts.is_empty() {
            return None;
        }
        Some(
            parts
                .into_iter()
                .map(|(m, mat)| (self.meshes.add(m), mat))
                .collect(),
        )
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

/// Load a city from `psdl_path` (e.g. `city/london.psdl`) plus its sibling
/// `.inst` placement file. Returns a suggested spawn point.
pub fn load_city(
    commands: &mut Commands,
    vfs: &Vfs,
    psdl_path: &str,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> LoadedCity {
    let mut mats = MaterialCache::new(vfs, images, materials);

    let (bytes, resolved) = match vfs.read_path(psdl_path) {
        Ok(v) => v,
        Err(e) => {
            warn!(path = %psdl_path, error = %e, "city PSDL not found");
            return LoadedCity {
                spawn: Vec3::new(0.0, 5.0, 0.0),
            };
        }
    };
    let psdl = match Psdl::parse(&bytes) {
        Ok(p) => p,
        Err(e) => {
            warn!(path = %resolved.logical, error = %e, "PSDL parse failed");
            return LoadedCity {
                spawn: Vec3::new(0.0, 5.0, 0.0),
            };
        }
    };
    info!(
        path = %resolved.logical,
        rooms = psdl.rooms.len(),
        textures = psdl.textures.len(),
        "loading city"
    );

    let n = build_city_geometry(commands, &psdl, meshes, &mut mats);
    info!(meshes = n, "city geometry spawned");

    // INST placements → PKG props.
    let inst_path = psdl_path.replace(".psdl", ".inst");
    if let Ok((inst_bytes, inst_res)) = vfs.read_path(&inst_path) {
        match inst::parse(&inst_bytes) {
            Ok(comps) => {
                let mut cache = PropCache {
                    vfs,
                    meshes,
                    mats,
                    cache: HashMap::new(),
                };
                let mut spawned = 0usize;
                for comp in &comps {
                    let Some(parts) = cache.get(&comp.package_name) else {
                        continue;
                    };
                    let mat4 = match &comp.placement {
                        InstPlacement::Coordinate(c) => inst_transform(c),
                        InstPlacement::Simple(s) => {
                            let pos = v3(s.location);
                            let dir = v3([s.x_delta, 0.0, s.z_delta]);
                            let scale = dir.length().max(0.001);
                            let yaw = if MIRROR_Z {
                                dir.x.atan2(-dir.z) // mirrored heading
                            } else {
                                dir.x.atan2(dir.z)
                            };
                            Mat4::from_rotation_translation(Quat::from_rotation_y(yaw), pos)
                                * Mat4::from_scale(Vec3::splat(scale))
                        }
                    };
                    let transform = Transform::from_matrix(mat4);
                    for (mesh, material) in parts {
                        commands.spawn((
                            CityEntity,
                            Mesh3d(mesh.clone()),
                            MeshMaterial3d(material.clone()),
                            transform,
                            Name::new(format!("prop-{}", comp.package_name)),
                        ));
                    }
                    spawned += 1;
                }
                info!(path = %inst_res.logical, placements = spawned, "inst props spawned");
            }
            Err(e) => warn!(path = %inst_res.logical, error = %e, "INST parse failed"),
        }
    }

    let center = v3(psdl.bounds_center);
    LoadedCity {
        spawn: Vec3::new(center.x, center.y + 10.0, center.z),
    }
}
