//! `export-glb`: write a vehicle as a binary glTF with embedded PNG
//! textures, built from the production loader so the file matches what
//! the game draws.
//!
//! The node tree mirrors `mm2_app::car_visual::spawn_vehicle_model`:
//! every part sits at its `.mtx` origin, wheels (followers included)
//! get a mount node with a `_spin` child carrying the geometry, fenders
//! hang off their wheel's mount, breakables and everything else hang off
//! the root, and a trailer is placed at its rest offset from the car
//! (`car_hitch − trailer_hitch`). `SHADOW` never renders and light
//! glows/flares are left out unless asked for — the game hides them
//! until a light is on.
//!
//! Materials follow `mm2_app::city::MaterialCache`: the paint job picks
//! the shader slot, a `<stem>_dmg` texture binds its clean stem when that
//! resolves, the shader diffuse tints/blends, decoded alpha makes a
//! cutout. Glows render additively in the game; glTF has no additive
//! mode, so their texture is re-encoded with alpha = the strongest
//! premultiplied channel and blended — the same result over a dark
//! background, slightly darker over a bright one.
//!
//! Coordinates are MM2 vehicle space verbatim (`+Y` up, `−Z` forward,
//! metres), so positions compare one-to-one with `mm2-inspect car`.

use std::collections::{BTreeSet, HashMap};
use std::io::Cursor;

use mm2_assets::Vfs;
use mm2_content::VehicleDef;
use mm2_content::model::{MeshGroup, ModelPart, PartRole, VehicleModel};
use mm2_formats::pkg::PkgShader;
use mm2_formats::tex::TexFile;
use serde_json::{Value, json};

use crate::TEXTURE_EXTS;

/// Same edge multiplier the game puts on `SRNn`/`HEADLIGHTn` flare quads
/// (`car_visual::FLARE_SCALE`, ledger COP-10).
const FLARE_SCALE: f32 = 3.0;

/// The glow sprite a flare binds when its shader names no texture
/// (`car_visual::FLARE_TEXTURE`).
const FLARE_TEXTURE: &str = "fxltglow";

/// Fallback material colour for an untextured or unresolved shader
/// (`MaterialCache::new`), sRGB.
const FALLBACK_SRGB: [f32; 3] = [0.6, 0.6, 0.65];

/// What to export.
pub struct ExportOptions {
    /// Paint job (zero-based shader set).
    pub paint: usize,
    /// Include light glows and flare quads.
    pub lights: bool,
}

/// What an export produced, for the command's summary line.
#[derive(Debug, Default)]
pub struct ExportStats {
    pub nodes: usize,
    pub meshes: usize,
    pub triangles: usize,
    pub materials: usize,
    pub images: usize,
    /// Texture stems that did not resolve or decode (fallback material).
    pub missing_textures: BTreeSet<String>,
}

/// Build the GLB bytes for `def` under `opts`.
pub fn export_vehicle(vfs: &Vfs, def: &VehicleDef, opts: &ExportOptions) -> (Vec<u8>, ExportStats) {
    let mut gltf = Builder::new(vfs);
    let mut children = gltf.add_model(&def.model, opts);
    if let Some(trailer) = &def.trailer {
        let rest_offset = sub(trailer.car_hitch, trailer.trailer_hitch);
        let trailer_children = gltf.add_model(&trailer.model, opts);
        children.push(gltf.node(json!({
            "name": "trailer",
            "translation": rest_offset,
            "children": trailer_children,
        })));
    }
    let paint_name = def.paints.get(opts.paint).cloned().unwrap_or_default();
    let root = gltf.node(json!({
        "name": def.id,
        "children": children,
        "extras": {
            "display_name": def.display_name,
            "paint": opts.paint,
            "paint_name": paint_name,
        },
    }));
    gltf.finish(root)
}

/// Which way a mesh group's material is built.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum MatKind {
    /// `group_material` without glow: the shader material as is.
    Solid,
    /// `group_material` with glow: tinted by the emissive, additive.
    Glow,
    /// `flare_material`: a lamp sprite tinted by its shader colour.
    Flare,
}

/// A decoded texture ready to embed.
struct Decoded {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    has_alpha: bool,
    /// TEX clamp bits (`ClampU` = 0x01, `ClampV` = 0x10000); PNG/TGA
    /// repeat like the game's buffer-image path.
    clamp_u: bool,
    clamp_v: bool,
}

struct Builder<'a> {
    vfs: &'a Vfs,
    nodes: Vec<Value>,
    meshes: Vec<Value>,
    materials: Vec<Value>,
    textures: Vec<Value>,
    images: Vec<Value>,
    samplers: Vec<Value>,
    accessors: Vec<Value>,
    buffer_views: Vec<Value>,
    bin: Vec<u8>,
    /// Decoded textures by stem (`None` = did not resolve or decode).
    decoded: HashMap<String, Option<Decoded>>,
    /// glTF texture index by (stem, additive re-encode).
    texture_index: HashMap<(String, bool), usize>,
    /// glTF material index by shader content + kind.
    material_index: HashMap<String, usize>,
    sampler_index: HashMap<(bool, bool), usize>,
    fallback: Option<usize>,
    stats: ExportStats,
}

impl<'a> Builder<'a> {
    fn new(vfs: &'a Vfs) -> Self {
        Self {
            vfs,
            nodes: Vec::new(),
            meshes: Vec::new(),
            materials: Vec::new(),
            textures: Vec::new(),
            images: Vec::new(),
            samplers: Vec::new(),
            accessors: Vec::new(),
            buffer_views: Vec::new(),
            bin: Vec::new(),
            decoded: HashMap::new(),
            texture_index: HashMap::new(),
            material_index: HashMap::new(),
            sampler_index: HashMap::new(),
            fallback: None,
            stats: ExportStats::default(),
        }
    }

    fn node(&mut self, node: Value) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    /// Add every rendered part of `model` and return the top-level node
    /// indices (children of the car or trailer root). Mirrors the two
    /// passes of `spawn_vehicle_model`.
    fn add_model(&mut self, model: &VehicleModel, opts: &ExportOptions) -> Vec<usize> {
        let mut top = Vec::new();
        // Pass 1: wheel mounts with their spin nodes, so fenders can link
        // to a mount whatever the part order.
        let mut is_mount = vec![false; model.parts.len()];
        let mut mount_children: Vec<Vec<usize>> = vec![Vec::new(); model.parts.len()];
        for (pi, part) in model.parts.iter().enumerate() {
            if !matches!(part.role, PartRole::Wheel(_) | PartRole::TrailerWheel(_)) {
                continue;
            }
            let mesh = self.part_mesh(model, opts.paint, part, MatKind::Solid);
            let spin = self.node(with_mesh(
                json!({ "name": format!("{}_spin", part.name) }),
                mesh,
            ));
            mount_children[pi].push(spin);
            is_mount[pi] = true;
        }

        // Pass 2: everything else.
        for part in &model.parts {
            let attach = part.origin.unwrap_or([0.0; 3]);
            match part.role {
                PartRole::Shadow | PartRole::Wheel(_) | PartRole::TrailerWheel(_) => continue,
                PartRole::Fender(n) => {
                    let wheel = model.wheels.iter().find(|w| !w.trailer && w.index == n);
                    let mount_part = wheel
                        .and_then(|w| w.parts.first().copied())
                        .filter(|pi| is_mount[*pi]);
                    let mesh = self.part_mesh(model, opts.paint, part, MatKind::Solid);
                    match (mount_part, wheel) {
                        (Some(pi), Some(w)) => {
                            let node = self.node(with_mesh(
                                json!({ "name": part.name, "translation": sub(attach, w.origin) }),
                                mesh,
                            ));
                            mount_children[pi].push(node);
                        }
                        _ => top.push(self.node(with_mesh(
                            json!({ "name": part.name, "translation": attach }),
                            mesh,
                        ))),
                    }
                }
                role => {
                    let glow = matches!(
                        role,
                        PartRole::HeadlightGlow
                            | PartRole::Headlight(_)
                            | PartRole::TaillightGlow
                            | PartRole::BrakeGlow
                            | PartRole::ReverseGlow
                    ) || (matches!(role, PartRole::Siren(_))
                        && part.name.starts_with("srn"));
                    if glow && !opts.lights {
                        continue;
                    }
                    let flare = glow && matches!(role, PartRole::Siren(_) | PartRole::Headlight(_));
                    let (kind, scale) = if flare {
                        (MatKind::Flare, FLARE_SCALE)
                    } else if glow {
                        (MatKind::Glow, 1.0)
                    } else {
                        (MatKind::Solid, 1.0)
                    };
                    let mesh = self.part_mesh(model, opts.paint, part, kind);
                    let mut node = json!({ "name": part.name, "translation": attach });
                    if scale != 1.0 {
                        node["scale"] = json!([scale, scale, scale]);
                    }
                    top.push(self.node(with_mesh(node, mesh)));
                }
            }
        }

        // The mounts go in last so their children lists are complete.
        for (pi, part) in model.parts.iter().enumerate() {
            if !is_mount[pi] {
                continue;
            }
            let children = std::mem::take(&mut mount_children[pi]);
            top.push(self.node(json!({
                "name": part.name,
                "translation": part.origin.unwrap_or([0.0; 3]),
                "children": children,
            })));
        }
        top
    }

    /// One glTF mesh for a part's best LOD — one primitive per shader
    /// group, with `recenter` baked in as the game does. `None` when the
    /// part has no triangles.
    fn part_mesh(
        &mut self,
        model: &VehicleModel,
        paint: usize,
        part: &ModelPart,
        kind: MatKind,
    ) -> Option<usize> {
        let groups = part.best_nonempty_lod().or_else(|| part.best_lod())?;
        let recenter = part.recenter.unwrap_or([0.0; 3]);
        let mut primitives = Vec::new();
        for g in groups.iter().filter(|g| !g.indices.is_empty()) {
            let uvs = if kind == MatKind::Flare {
                flare_uvs(&g.positions)
            } else {
                g.uvs.clone()
            };
            let material = self.group_material(model, paint, g.shader_offset, kind);
            primitives.push(self.primitive(g, recenter, &uvs, material));
        }
        if primitives.is_empty() {
            return None;
        }
        self.meshes
            .push(json!({ "name": part.name, "primitives": primitives }));
        self.stats.meshes += 1;
        Some(self.meshes.len() - 1)
    }

    fn primitive(
        &mut self,
        g: &MeshGroup,
        recenter: [f32; 3],
        uvs: &[[f32; 2]],
        material: usize,
    ) -> Value {
        let positions: Vec<[f32; 3]> = g.positions.iter().map(|p| sub(*p, recenter)).collect();
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for p in &positions {
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
        let normals: Vec<[f32; 3]> = if g.normals.len() == g.positions.len() {
            g.normals.iter().map(|n| unit(*n)).collect()
        } else {
            vec![[0.0, 1.0, 0.0]; g.positions.len()]
        };
        // Retail flare quads carry uninitialised UVs; anything non-finite
        // is invalid glTF.
        let uvs: Vec<[f32; 2]> = uvs
            .iter()
            .map(|uv| {
                if uv.iter().all(|c| c.is_finite()) {
                    *uv
                } else {
                    [0.0, 0.0]
                }
            })
            .collect();
        let count = positions.len();
        let pos = self.accessor(
            floats(&positions),
            count,
            "VEC3",
            5126,
            Some((min, max)),
            34962,
        );
        let nrm = self.accessor(floats(&normals), count, "VEC3", 5126, None, 34962);
        let tex = self.accessor(floats(&uvs), count, "VEC2", 5126, None, 34962);
        let idx_bytes: Vec<u8> = g.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let idx = self.accessor(idx_bytes, g.indices.len(), "SCALAR", 5125, None, 34963);
        self.stats.triangles += g.indices.len() / 3;
        json!({
            "attributes": { "POSITION": pos, "NORMAL": nrm, "TEXCOORD_0": tex },
            "indices": idx,
            "material": material,
        })
    }

    fn accessor(
        &mut self,
        bytes: Vec<u8>,
        count: usize,
        ty: &str,
        component: u32,
        bounds: Option<([f32; 3], [f32; 3])>,
        target: u32,
    ) -> usize {
        let view = self.buffer_view(&bytes, Some(target));
        let mut acc = json!({
            "bufferView": view,
            "componentType": component,
            "count": count,
            "type": ty,
        });
        if let Some((min, max)) = bounds {
            acc["min"] = json!(min);
            acc["max"] = json!(max);
        }
        self.accessors.push(acc);
        self.accessors.len() - 1
    }

    fn buffer_view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut view = json!({
            "buffer": 0,
            "byteOffset": self.bin.len(),
            "byteLength": bytes.len(),
        });
        if let Some(t) = target {
            view["target"] = json!(t);
        }
        self.bin.extend_from_slice(bytes);
        self.buffer_views.push(view);
        self.buffer_views.len() - 1
    }

    /// `car_visual::group_material` / `flare_material` for one group.
    fn group_material(
        &mut self,
        model: &VehicleModel,
        paint: usize,
        offset: usize,
        kind: MatKind,
    ) -> usize {
        let shader = model
            .shaders
            .get(paint * model.shaders_per_paint_job + offset);
        match (kind, shader) {
            // An out-of-range slot binds the plain fallback, glow or not.
            (MatKind::Solid | MatKind::Glow, None) => self.fallback_material(),
            (MatKind::Solid, Some(s)) => self.shader_material(s),
            (MatKind::Glow, Some(s)) => {
                let tint = Some([s.emissive[0], s.emissive[1], s.emissive[2]])
                    .filter(|e| e.iter().any(|c| *c > 0.0))
                    .unwrap_or([1.0; 3]);
                // The glow keeps `shader_material`'s texture (clean stem
                // resolution included) and swaps everything else.
                let stem = self.clean_texture_stem(&s.texture);
                self.additive_material(&stem, tint)
            }
            (MatKind::Flare, s) => {
                let tint = s.map_or([1.0; 3], |s| {
                    let c = if s.diffuse[3] < 0.01 {
                        s.emissive
                    } else {
                        s.diffuse
                    };
                    if c[..3].iter().any(|v| *v > 0.0) {
                        [c[0], c[1], c[2]]
                    } else {
                        [1.0; 3]
                    }
                });
                let texture = s
                    .map(|s| s.texture.to_ascii_lowercase())
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| FLARE_TEXTURE.to_string());
                self.additive_material(&texture, tint)
            }
        }
    }

    fn fallback_material(&mut self) -> usize {
        if let Some(i) = self.fallback {
            return i;
        }
        let i = self.push_material(json!({
            "name": "fallback",
            "pbrMetallicRoughness": {
                "baseColorFactor": rgba_linear(FALLBACK_SRGB, 1.0),
                "metallicFactor": 0.0,
                "roughnessFactor": 0.9,
            },
        }));
        self.fallback = Some(i);
        i
    }

    /// `MaterialCache::shader_material`: the clean-stem texture (or the
    /// fallback), then `adjust_material`'s diffuse tint and blend.
    fn shader_material(&mut self, s: &PkgShader) -> usize {
        let stem = self.clean_texture_stem(&s.texture);
        let key = format!("solid:{stem}:{:?}", s.diffuse);
        if let Some(&i) = self.material_index.get(&key) {
            return i;
        }
        let texture = self.texture(&stem, false);
        let (mut color, roughness, mut alpha_mode, name) = match texture {
            Some((_, has_alpha)) => (
                [1.0; 4],
                0.95,
                if has_alpha { "MASK" } else { "OPAQUE" },
                stem.clone(),
            ),
            None => (
                rgba_linear(FALLBACK_SRGB, 1.0),
                0.9,
                "OPAQUE",
                "fallback".to_string(),
            ),
        };
        let needs_blend = s.diffuse[3] < 0.99;
        let tinted = s.diffuse[..3].iter().any(|c| (c - 1.0).abs() > 0.01);
        if needs_blend || tinted {
            color = rgba_linear([s.diffuse[0], s.diffuse[1], s.diffuse[2]], s.diffuse[3]);
        }
        if needs_blend {
            alpha_mode = "BLEND";
        }
        let mut pbr = json!({
            "baseColorFactor": color,
            "metallicFactor": 0.0,
            "roughnessFactor": roughness,
        });
        if let Some((t, _)) = texture {
            pbr["baseColorTexture"] = json!({ "index": t });
        }
        let mut mat = json!({ "name": name, "pbrMetallicRoughness": pbr, "alphaMode": alpha_mode });
        if alpha_mode == "MASK" {
            mat["alphaCutoff"] = json!(0.5);
        }
        let i = self.push_material(mat);
        self.material_index.insert(key, i);
        i
    }

    /// An unlit, double-sided, additive-looking sprite material.
    fn additive_material(&mut self, stem: &str, tint: [f32; 3]) -> usize {
        let key = format!("additive:{stem}:{tint:?}");
        if let Some(&i) = self.material_index.get(&key) {
            return i;
        }
        let texture = self.texture(stem, true);
        let mut pbr = json!({
            "baseColorFactor": rgba_linear(tint, 1.0),
            "metallicFactor": 0.0,
            "roughnessFactor": 1.0,
        });
        if let Some((t, _)) = texture {
            pbr["baseColorTexture"] = json!({ "index": t });
        }
        let i = self.push_material(json!({
            "name": format!("{stem}_glow"),
            "pbrMetallicRoughness": pbr,
            "alphaMode": if texture.is_some() { "BLEND" } else { "OPAQUE" },
            "doubleSided": true,
            "extensions": { "KHR_materials_unlit": {} },
        }));
        self.material_index.insert(key, i);
        i
    }

    fn push_material(&mut self, mat: Value) -> usize {
        self.materials.push(mat);
        self.materials.len() - 1
    }

    /// `MaterialCache::clean_texture_stem`: `<stem>_dmg` binds `<stem>`
    /// when `texture/<stem>` resolves, else the authored name stands.
    fn clean_texture_stem(&self, name: &str) -> String {
        let lower = name.to_ascii_lowercase();
        match lower.strip_suffix("_dmg") {
            Some(stem)
                if self
                    .vfs
                    .resolve_preferred(&format!("texture/{stem}"), TEXTURE_EXTS)
                    .is_some() =>
            {
                stem.to_string()
            }
            _ => lower,
        }
    }

    /// The glTF texture for `stem` (and whether it carries alpha), or
    /// `None` when the stem is empty or does not load.
    fn texture(&mut self, stem: &str, additive: bool) -> Option<(usize, bool)> {
        if stem.is_empty() {
            return None;
        }
        if !self.decoded.contains_key(stem) {
            let decoded = load_texture(self.vfs, stem)
                .or_else(|| load_texture(self.vfs, &format!("{stem}-0001")));
            if decoded.is_none() {
                self.stats.missing_textures.insert(stem.to_string());
            }
            self.decoded.insert(stem.to_string(), decoded);
        }
        let key = (stem.to_string(), additive);
        let has_alpha = self.decoded[stem].as_ref()?.has_alpha;
        if let Some(&t) = self.texture_index.get(&key) {
            return Some((t, has_alpha));
        }
        let d = self.decoded[stem].as_ref()?;
        let (clamp_u, clamp_v) = (d.clamp_u, d.clamp_v);
        let png = if additive {
            encode_png(d.width, d.height, &additive_rgba(&d.rgba))
        } else {
            encode_png(d.width, d.height, &d.rgba)
        };
        let view = self.buffer_view(&png, None);
        self.images.push(json!({
            "name": if additive { format!("{stem}_glow") } else { stem.to_string() },
            "mimeType": "image/png",
            "bufferView": view,
        }));
        let image = self.images.len() - 1;
        let sampler = self.sampler(clamp_u, clamp_v);
        self.textures
            .push(json!({ "source": image, "sampler": sampler }));
        let t = self.textures.len() - 1;
        self.texture_index.insert(key, t);
        Some((t, has_alpha))
    }

    fn sampler(&mut self, clamp_u: bool, clamp_v: bool) -> usize {
        if let Some(&s) = self.sampler_index.get(&(clamp_u, clamp_v)) {
            return s;
        }
        const REPEAT: u32 = 10497;
        const CLAMP: u32 = 33071;
        self.samplers.push(json!({
            "magFilter": 9729,
            "minFilter": 9987,
            "wrapS": if clamp_u { CLAMP } else { REPEAT },
            "wrapT": if clamp_v { CLAMP } else { REPEAT },
        }));
        let s = self.samplers.len() - 1;
        self.sampler_index.insert((clamp_u, clamp_v), s);
        s
    }

    fn finish(mut self, root: usize) -> (Vec<u8>, ExportStats) {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut doc = json!({
            "asset": { "version": "2.0", "generator": "mm2-inspect export-glb" },
            "scene": 0,
            "scenes": [{ "nodes": [root] }],
            "nodes": self.nodes,
            "meshes": self.meshes,
            "materials": self.materials,
            "accessors": self.accessors,
            "bufferViews": self.buffer_views,
            "buffers": [{ "byteLength": self.bin.len() }],
        });
        if !self.images.is_empty() {
            doc["images"] = json!(self.images);
            doc["textures"] = json!(self.textures);
            doc["samplers"] = json!(self.samplers);
        }
        let unlit = doc["materials"]
            .as_array()
            .is_some_and(|m| m.iter().any(|m| m.get("extensions").is_some()));
        if unlit {
            doc["extensionsUsed"] = json!(["KHR_materials_unlit"]);
        }
        let mut stats = self.stats;
        stats.nodes = doc["nodes"].as_array().map_or(0, Vec::len);
        stats.materials = doc["materials"].as_array().map_or(0, Vec::len);
        stats.images = self.images.len();
        (glb_container(&doc.to_string(), &self.bin), stats)
    }
}

/// Wrap a JSON document and its binary buffer in the GLB container.
fn glb_container(json: &str, bin: &[u8]) -> Vec<u8> {
    let mut json = json.as_bytes().to_vec();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(bin);
    out
}

/// Resolve and decode `texture/<stem>` with the game's extension order.
/// TEX decodes through `mm2_formats`; PNG/TGA through `image`. KTX2 is
/// GPU-compressed and not decoded here — reported as missing.
fn load_texture(vfs: &Vfs, stem: &str) -> Option<Decoded> {
    let resolved = vfs.resolve_preferred(&format!("texture/{stem}"), TEXTURE_EXTS)?;
    let bytes = vfs.read(&resolved).ok()?;
    let ext = resolved.logical.rsplit('.').next().unwrap_or_default();
    let (width, height, rgba, clamp_u, clamp_v) = match ext {
        "tex" => {
            let tex = TexFile::parse(&bytes).ok()?;
            let rgba = tex.decode_rgba(0)?;
            let bits = tex.header.bits;
            (
                tex.header.width as u32,
                tex.header.height as u32,
                rgba,
                bits & 0x01 != 0,
                bits & 0x1_0000 != 0,
            )
        }
        "png" | "tga" => {
            let format = if ext == "png" {
                image::ImageFormat::Png
            } else {
                image::ImageFormat::Tga
            };
            let img = image::load_from_memory_with_format(&bytes, format)
                .ok()?
                .to_rgba8();
            (img.width(), img.height(), img.into_raw(), false, false)
        }
        _ => return None,
    };
    let has_alpha = rgba.as_chunks::<4>().0.iter().any(|px| px[3] < 250);
    Some(Decoded {
        width,
        height,
        rgba,
        has_alpha,
        clamp_u,
        clamp_v,
    })
}

/// Re-encode a texture the game adds to the frame (`AlphaMode::Add`,
/// premultiplied: `src.rgb × src.a + dst`) as a straight-alpha texture
/// that blends to the same colour: alpha = the strongest premultiplied
/// channel, colour = premultiplied colour over that alpha.
fn additive_rgba(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, a]| {
            let pre = [r, g, b].map(|c| c as u32 * a as u32 / 255);
            let out_a = pre.into_iter().max().unwrap_or(0);
            if out_a == 0 {
                return [0, 0, 0, 0];
            }
            let [r, g, b] = pre.map(|c| (c * 255 / out_a) as u8);
            [r, g, b, out_a as u8]
        })
        .collect()
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .expect("decoded size matches its pixels")
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("PNG encoding into memory cannot fail");
    out.into_inner()
}

/// `car_visual::flare_uvs`: the glow sprite spread corner to corner over
/// the quad's extent in the car's XY plane.
fn flare_uvs(positions: &[[f32; 3]]) -> Vec<[f32; 2]> {
    let span = |axis: usize| {
        let (lo, hi) = positions.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
            (lo.min(p[axis]), hi.max(p[axis]))
        });
        (lo, (hi - lo).max(f32::EPSILON))
    };
    let (x0, xs) = span(0);
    let (y0, ys) = span(1);
    positions
        .iter()
        .map(|p| [(p[0] - x0) / xs, 1.0 - (p[1] - y0) / ys])
        .collect()
}

fn with_mesh(mut node: Value, mesh: Option<usize>) -> Value {
    if let Some(m) = mesh {
        node["mesh"] = json!(m);
    }
    node
}

fn floats<const N: usize>(v: &[[f32; N]]) -> Vec<u8> {
    v.iter().flatten().flat_map(|f| f.to_le_bytes()).collect()
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// glTF requires unit normals; a degenerate one points up, like a
/// missing one in the loader.
fn unit(n: [f32; 3]) -> [f32; 3] {
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len.is_finite() && len > 1e-6 {
        [n[0] / len, n[1] / len, n[2] / len]
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// The game builds colours with `Color::srgb`; glTF factors are linear.
fn rgba_linear(srgb: [f32; 3], alpha: f32) -> [f32; 4] {
    let lin = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    [lin(srgb[0]), lin(srgb[1]), lin(srgb[2]), alpha]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn additive_keeps_the_added_colour_over_black() {
        // Half-bright red at full alpha adds (128, 0, 0); blended over
        // black the re-encoded texel must give the same.
        let out = additive_rgba(&[128, 0, 0, 255, 0, 0, 0, 255]);
        let [r, g, b, a] = [out[0], out[1], out[2], out[3]];
        assert_eq!(r as u32 * a as u32 / 255, 128);
        assert_eq!((g, b), (0, 0));
        assert_eq!(&out[4..], &[0, 0, 0, 0]);
    }

    #[test]
    fn container_is_aligned_and_sized() {
        let glb = glb_container("{}", &[1, 2, 3, 4]);
        assert_eq!(&glb[0..4], b"glTF");
        assert_eq!(
            u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize,
            glb.len()
        );
        assert_eq!(u32::from_le_bytes(glb[12..16].try_into().unwrap()), 4);
        assert_eq!(&glb[20..24], b"{}  ");
    }
}
