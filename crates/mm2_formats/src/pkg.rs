//! Parser for MM2 `PKG` object geometry files (`PKG2`/`PKG3` containers).
//!
//! A PKG is a sequence of named "FILE" chunks. Geometry chunks end in `VL`,
//! `L`, `M` or `H` (LOD suffix), `shaders` holds materials, `offset` a single
//! translation, and `xref` references other PKGs. Unknown chunks are kept as
//! [`PkgChunk::Raw`] so nothing is silently dropped.
//!
//! Provenance: `angel-file-formats/Midtown Madness 2/PKG.md`, verified against
//! `geometry/vp4x4.pkg` from a retail `mm2core.ar`.

use crate::{FormatError, Reader};

/// DirectX flexible-vertex-format bit: position (`XYZ`).
pub const FVF_XYZ: u32 = 0x002;
/// DirectX FVF bit: pre-transformed position (`XYZRHW`).
pub const FVF_XYZRHW: u32 = 0x004;
/// DirectX FVF bit: normal vector.
pub const FVF_NORMAL: u32 = 0x010;
/// DirectX FVF bit: diffuse vertex colour.
pub const FVF_DIFFUSE: u32 = 0x040;
/// DirectX FVF bit: specular vertex colour.
pub const FVF_SPECULAR: u32 = 0x080;
/// DirectX FVF mask for the texture-coordinate count field.
pub const FVF_TEXCOUNT_MASK: u32 = 0x0f00;

/// Primitive type value used for triangle lists.
pub const PRIMTYPE_TRIANGLES: i32 = 3;

fn tex_count(fvf: u32) -> u32 {
    (fvf & FVF_TEXCOUNT_MASK) >> 8
}

/// One vertex of a [`PkgStrip`]. Presence of components is controlled by the
/// containing geometry's FVF flags.
#[derive(Debug, Clone)]
pub struct PkgVertex {
    /// Vertex position.
    pub position: [f32; 3],
    /// Vertex normal, if `FVF_NORMAL` was set.
    pub normal: Option<[f32; 3]>,
    /// Diffuse vertex colour (packed RGBA), if `FVF_DIFFUSE` was set.
    pub diffuse: Option<u32>,
    /// Specular vertex colour (packed RGBA), if `FVF_SPECULAR` was set.
    pub specular: Option<u32>,
    /// Texture coordinate sets (usually zero or one).
    pub tex_coords: Vec<[f32; 2]>,
}

/// A strip of vertices plus the indices that build primitives from them.
#[derive(Debug, Clone)]
pub struct PkgStrip {
    /// Primitive type; only [`PRIMTYPE_TRIANGLES`] has been observed.
    pub prim_type: i32,
    /// Vertex buffer of this strip.
    pub vertices: Vec<PkgVertex>,
    /// Indices into `vertices`.
    pub indices: Vec<u16>,
}

/// A group of strips sharing one shader.
#[derive(Debug, Clone)]
pub struct PkgSection {
    /// Unknown flags, preserved (always 0 in observed MM2 files).
    pub flags: u16,
    /// Index into the shader list of the active paint job.
    pub shader_offset: i32,
    /// Strips making up this section.
    pub strips: Vec<PkgStrip>,
}

/// A geometry chunk of a PKG file.
#[derive(Debug, Clone)]
pub struct PkgGeometry {
    /// FVF flags describing the vertex layout (kept verbatim).
    pub fvf: u32,
    /// Declared total vertex count.
    pub total_vertices: u32,
    /// Declared total index count.
    pub total_indices: u32,
    /// Duplicate of the section count stored in the file, preserved verbatim.
    pub sections_duplicate: u32,
    /// Parsed sections.
    pub sections: Vec<PkgSection>,
}

/// A single shader (material) definition.
#[derive(Debug, Clone)]
pub struct PkgShader {
    /// Texture name (base name without extension) or empty.
    pub texture: String,
    /// Diffuse colour.
    pub diffuse: [f32; 4],
    /// Ambient colour.
    pub ambient: [f32; 4],
    /// Specular colour (float shaders only).
    pub specular: Option<[f32; 4]>,
    /// Emissive colour.
    pub emissive: [f32; 4],
    /// Specular shininess exponent.
    pub shininess: f32,
}

/// The `shaders` chunk of a PKG file.
#[derive(Debug, Clone)]
pub struct PkgShaders {
    /// Number of paint jobs.
    pub paint_jobs: u32,
    /// Shaders per paint job.
    pub shaders_per_paint_job: u32,
    /// `paint_jobs * shaders_per_paint_job` shader records.
    pub shaders: Vec<PkgShader>,
    /// Raw shader-type word, preserved.
    pub shader_type: u32,
}

/// A cross-reference to another PKG (e.g. a breakable part).
#[derive(Debug, Clone)]
pub struct PkgXref {
    /// X axis of the reference frame.
    pub x_axis: [f32; 3],
    /// Y axis of the reference frame.
    pub y_axis: [f32; 3],
    /// Z axis of the reference frame.
    pub z_axis: [f32; 3],
    /// Origin of the reference frame.
    pub origin: [f32; 3],
    /// Referenced PKG name.
    pub name: String,
}

/// The decoded payload of a `FILE` chunk.
#[derive(Debug, Clone)]
pub enum PkgChunk {
    /// Geometry LOD chunk.
    Geometry(PkgGeometry),
    /// Shader/material chunk.
    Shaders(PkgShaders),
    /// Single translation applied to the whole model.
    Offset([f32; 3]),
    /// References to other PKG files.
    Xref(Vec<PkgXref>),
    /// Unrecognised chunk, preserved verbatim.
    Raw(Vec<u8>),
}

/// A named `FILE` chunk.
#[derive(Debug, Clone)]
pub struct PkgFile {
    /// Chunk name, e.g. `BODY_H` or `shaders`.
    pub name: String,
    /// Parsed payload.
    pub data: PkgChunk,
}

/// A parsed PKG file.
#[derive(Debug, Clone)]
pub struct Pkg {
    /// Container variant as written in the file (`PKG2` or `PKG3`).
    pub version: [u8; 4],
    /// Chunks in file order.
    pub files: Vec<PkgFile>,
}

impl Pkg {
    /// Parse a PKG file from bytes.
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(data);
        let magic = r.bytes(4)?;
        let is_pkg3 = match magic {
            b"PKG3" => true,
            b"PKG2" => false,
            _ => {
                return Err(FormatError::BadMagic {
                    offset: 0,
                    expected: "PKG2|PKG3",
                    found: magic.to_vec(),
                });
            }
        };

        let mut files = Vec::new();
        while r.remaining() >= 4 {
            // Some retail PKGs end with zero padding rather than a chunk.
            if r.rest().iter().all(|&b| b == 0) {
                break;
            }
            let offset = r.pos();
            let tag = r.bytes(4)?;
            if tag != b"FILE" {
                return Err(FormatError::BadMagic {
                    offset,
                    expected: "FILE",
                    found: tag.to_vec(),
                });
            }
            let name = r.lp_string()?;
            let chunk_data = if is_pkg3 {
                // PKG3 stores the payload size; parse inside a bounded slice.
                let len = r.u32()? as usize;
                if len > r.remaining() {
                    // A small number of retail PKGs are corrupt (documented by
                    // the community). Preserve the remainder and stop.
                    tracing::warn!(
                        chunk = %name,
                        offset,
                        len,
                        remaining = r.remaining(),
                        "PKG chunk length exceeds file; stopping parse"
                    );
                    let raw = r.bytes(r.remaining())?.to_vec();
                    files.push(PkgFile {
                        name,
                        data: PkgChunk::Raw(raw),
                    });
                    break;
                }
                let payload = r.bytes(len)?;
                let mut cr = Reader::new(payload);
                let chunk = parse_chunk(&name, &mut cr, payload)?;
                if cr.remaining() != 0 {
                    // Parsed less than declared; keep going anyway since the
                    // next chunk is located through `len`.
                    tracing::debug!(
                        chunk = %name,
                        leftover = cr.remaining(),
                        "PKG chunk not fully consumed"
                    );
                }
                chunk
            } else {
                // PKG2 has no length field; parse known chunks in place and
                // treat the remainder as one raw chunk on unknown names.
                parse_chunk_stream(&name, &mut r)?
            };
            files.push(PkgFile { name, data: chunk_data });
        }

        Ok(Self {
            version: [magic[0], magic[1], magic[2], magic[3]],
            files,
        })
    }

    /// All geometry chunks, in file order.
    pub fn geometries(&self) -> impl Iterator<Item = (&str, &PkgGeometry)> {
        self.files.iter().filter_map(|f| match &f.data {
            PkgChunk::Geometry(g) => Some((f.name.as_str(), g)),
            _ => None,
        })
    }

    /// The shader chunk, if present.
    pub fn shaders(&self) -> Option<&PkgShaders> {
        self.files.iter().find_map(|f| match &f.data {
            PkgChunk::Shaders(s) => Some(s),
            _ => None,
        })
    }
}

/// Whether a chunk name marks a geometry section (`*VL`, `*L`, `*M`, `*H`).
fn is_geometry_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let stem = lower.rsplit(['_', '/']).next().unwrap_or(&lower);
    matches!(stem, "vl" | "l" | "m" | "h")
}

/// Interpret a bounded chunk payload based on its name (PKG3 path).
fn parse_chunk(name: &str, r: &mut Reader<'_>, raw: &[u8]) -> Result<PkgChunk, FormatError> {
    Ok(match classify(name) {
        ChunkKind::Geometry => match parse_geometry(r) {
            Ok(g) => PkgChunk::Geometry(g),
            Err(_) => PkgChunk::Raw(raw.to_vec()),
        },
        ChunkKind::Shaders => PkgChunk::Shaders(parse_shaders(r)?),
        ChunkKind::Offset => PkgChunk::Offset(r.vec3()?),
        ChunkKind::Xref => PkgChunk::Xref(parse_xref(r)?),
        ChunkKind::Unknown => PkgChunk::Raw(raw.to_vec()),
    })
}

enum ChunkKind {
    Geometry,
    Shaders,
    Offset,
    Xref,
    Unknown,
}

fn classify(name: &str) -> ChunkKind {
    let lower = name.to_ascii_lowercase();
    if lower == "shaders" {
        ChunkKind::Shaders
    } else if lower == "offset" {
        ChunkKind::Offset
    } else if lower == "xref" {
        ChunkKind::Xref
    } else if is_geometry_name(name) {
        ChunkKind::Geometry
    } else {
        ChunkKind::Unknown
    }
}

/// PKG2 path: chunks have no length, so unknown payloads cannot be skipped.
/// Known chunks are parsed in place; an unknown chunk swallows the remainder
/// of the file as raw bytes (documented limitation).
fn parse_chunk_stream(name: &str, r: &mut Reader<'_>) -> Result<PkgChunk, FormatError> {
    Ok(match classify(name) {
        ChunkKind::Geometry => PkgChunk::Geometry(parse_geometry(r)?),
        ChunkKind::Shaders => PkgChunk::Shaders(parse_shaders(r)?),
        ChunkKind::Offset => PkgChunk::Offset(r.vec3()?),
        ChunkKind::Xref => PkgChunk::Xref(parse_xref(r)?),
        ChunkKind::Unknown => PkgChunk::Raw(r.bytes(r.remaining())?.to_vec()),
    })
}

fn parse_geometry(r: &mut Reader<'_>) -> Result<PkgGeometry, FormatError> {
    let n_sections = r.u32()?;
    let total_vertices = r.u32()?;
    let total_indices = r.u32()?;
    let sections_duplicate = r.u32()?;
    let fvf = r.u32()?;

    if n_sections > 1024 {
        return Err(FormatError::InvalidValue {
            offset: r.pos() - 20,
            field: "nSections",
            value: n_sections as u64,
            reason: "implausible section count",
        });
    }

    let mut sections = Vec::with_capacity(n_sections as usize);
    for _ in 0..n_sections {
        let n_strips = r.u16()?;
        let flags = r.u16()?;
        let shader_offset = r.i32()?;
        let mut strips = Vec::with_capacity(n_strips as usize);
        for _ in 0..n_strips {
            let prim_type = r.i32()?;
            let n_vertices = r.u32()? as usize;
            if n_vertices > 1 << 22 {
                return Err(FormatError::InvalidValue {
                    offset: r.pos() - 4,
                    field: "nVertices",
                    value: n_vertices as u64,
                    reason: "implausible vertex count",
                });
            }
            let mut vertices = Vec::with_capacity(n_vertices);
            for _ in 0..n_vertices {
                vertices.push(read_vertex(r, fvf)?);
            }
            let n_indices = r.u32()? as usize;
            if n_indices > 1 << 22 {
                return Err(FormatError::InvalidValue {
                    offset: r.pos() - 4,
                    field: "nIndices",
                    value: n_indices as u64,
                    reason: "implausible index count",
                });
            }
            let mut indices = Vec::with_capacity(n_indices);
            for _ in 0..n_indices {
                indices.push(r.u16()?);
            }
            strips.push(PkgStrip {
                prim_type,
                vertices,
                indices,
            });
        }
        sections.push(PkgSection {
            flags,
            shader_offset,
            strips,
        });
    }

    Ok(PkgGeometry {
        fvf,
        total_vertices,
        total_indices,
        sections_duplicate,
        sections,
    })
}

fn read_vertex(r: &mut Reader<'_>, fvf: u32) -> Result<PkgVertex, FormatError> {
    let position = r.vec3()?;
    let normal = if fvf & FVF_NORMAL != 0 {
        Some(r.vec3()?)
    } else {
        None
    };
    let diffuse = if fvf & FVF_DIFFUSE != 0 {
        Some(r.u32()?)
    } else {
        None
    };
    let specular = if fvf & FVF_SPECULAR != 0 {
        Some(r.u32()?)
    } else {
        None
    };
    let mut tex_coords = Vec::with_capacity(tex_count(fvf) as usize);
    for _ in 0..tex_count(fvf) {
        tex_coords.push(r.vec2()?);
    }
    Ok(PkgVertex {
        position,
        normal,
        diffuse,
        specular,
        tex_coords,
    })
}

fn parse_shaders(r: &mut Reader<'_>) -> Result<PkgShaders, FormatError> {
    let shader_type = r.u32()?;
    let shaders_per_paint_job = r.u32()?;
    let float_shaders = shader_type & 0x80 == 0;
    let paint_jobs = shader_type & 0x7f;
    let total = paint_jobs as usize * shaders_per_paint_job as usize;
    if total > 4096 {
        return Err(FormatError::InvalidValue {
            offset: r.pos() - 8,
            field: "shaders",
            value: total as u64,
            reason: "implausible shader count",
        });
    }
    let mut shaders = Vec::with_capacity(total);
    for _ in 0..total {
        let texture = r.lp_string()?;
        let shader = if float_shaders {
            let diffuse = read_color4f(r)?;
            let ambient = read_color4f(r)?;
            let specular = read_color4f(r)?;
            let emissive = read_color4f(r)?;
            PkgShader {
                texture,
                diffuse,
                ambient,
                specular: Some(specular),
                emissive,
                shininess: r.f32()?,
            }
        } else {
            let diffuse = read_color4d(r)?;
            let ambient = read_color4d(r)?;
            let emissive = read_color4d(r)?;
            PkgShader {
                texture,
                diffuse,
                ambient,
                specular: None,
                emissive,
                shininess: r.f32()?,
            }
        };
        shaders.push(shader);
    }
    Ok(PkgShaders {
        paint_jobs,
        shaders_per_paint_job,
        shaders,
        shader_type,
    })
}

fn read_color4f(r: &mut Reader<'_>) -> Result<[f32; 4], FormatError> {
    Ok([r.f32()?, r.f32()?, r.f32()?, r.f32()?])
}

fn read_color4d(r: &mut Reader<'_>) -> Result<[f32; 4], FormatError> {
    Ok([
        r.u8()? as f32 / 255.0,
        r.u8()? as f32 / 255.0,
        r.u8()? as f32 / 255.0,
        r.u8()? as f32 / 255.0,
    ])
}

fn parse_xref(r: &mut Reader<'_>) -> Result<Vec<PkgXref>, FormatError> {
    let count = r.u32()? as usize;
    if count > 4096 {
        return Err(FormatError::InvalidValue {
            offset: r.pos() - 4,
            field: "nReferences",
            value: count as u64,
            reason: "implausible xref count",
        });
    }
    let mut refs = Vec::with_capacity(count);
    for _ in 0..count {
        let x_axis = r.vec3()?;
        let y_axis = r.vec3()?;
        let z_axis = r.vec3()?;
        let origin = r.vec3()?;
        let name_bytes = r.bytes(32)?;
        let name = name_bytes
            .split(|&b| b == 0)
            .next()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        refs.push(PkgXref {
            x_axis,
            y_axis,
            z_axis,
            origin,
            name,
        });
    }
    Ok(refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lp(s: &str) -> Vec<u8> {
        let mut v = vec![(s.len() + 1) as u8];
        v.extend_from_slice(s.as_bytes());
        v.push(0);
        v
    }

    fn geometry_chunk(fvf: u32) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&1u32.to_le_bytes()); // nSections
        d.extend_from_slice(&3u32.to_le_bytes()); // nVerticesTot
        d.extend_from_slice(&3u32.to_le_bytes()); // nIndiciesTot
        d.extend_from_slice(&1u32.to_le_bytes()); // nSections2
        d.extend_from_slice(&fvf.to_le_bytes());
        // section
        d.extend_from_slice(&1u16.to_le_bytes()); // nStrips
        d.extend_from_slice(&0u16.to_le_bytes()); // flags
        d.extend_from_slice(&0i32.to_le_bytes()); // shaderOffset
        // strip
        d.extend_from_slice(&3i32.to_le_bytes()); // primType
        d.extend_from_slice(&3u32.to_le_bytes()); // nVertices
        for i in 0..3u32 {
            for f in [i as f32, 0.0, 1.0] {
                d.extend_from_slice(&f.to_le_bytes());
            }
            if fvf & FVF_NORMAL != 0 {
                for f in [0.0f32, 1.0, 0.0] {
                    d.extend_from_slice(&f.to_le_bytes());
                }
            }
            if fvf & FVF_TEXCOUNT_MASK != 0 {
                for f in [0.5f32, 0.5] {
                    d.extend_from_slice(&f.to_le_bytes());
                }
            }
        }
        d.extend_from_slice(&3u32.to_le_bytes()); // nIndices
        for i in 0..3u16 {
            d.extend_from_slice(&i.to_le_bytes());
        }
        d
    }

    fn pkg3(chunks: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut d = b"PKG3".to_vec();
        for (name, data) in chunks {
            d.extend_from_slice(b"FILE");
            let name_bytes = lp(name);
            let len = data.len() as u32;
            d.extend_from_slice(&name_bytes);
            d.extend_from_slice(&len.to_le_bytes());
            d.extend_from_slice(data);
        }
        d
    }

    #[test]
    fn parses_geometry_chunk() {
        let fvf = FVF_XYZ | FVF_NORMAL | 0x100;
        let data = pkg3(&[("BODY_H", geometry_chunk(fvf))]);
        let pkg = Pkg::parse(&data).unwrap();
        let (name, geo) = pkg.geometries().next().unwrap();
        assert_eq!(name, "BODY_H");
        assert_eq!(geo.sections.len(), 1);
        let strip = &geo.sections[0].strips[0];
        assert_eq!(strip.prim_type, PRIMTYPE_TRIANGLES);
        assert_eq!(strip.vertices.len(), 3);
        assert_eq!(strip.vertices[1].position[0], 1.0);
        assert_eq!(strip.indices, vec![0, 1, 2]);
        assert!(strip.vertices[0].normal.is_some());
        assert_eq!(strip.vertices[0].tex_coords.len(), 1);
    }

    #[test]
    fn parses_shaders() {
        let mut s = Vec::new();
        s.extend_from_slice(&0u32.to_le_bytes()); // type: float, 0 paint jobs? use 1
        // patch: paint_jobs = 1
        s[0] = 1;
        s.extend_from_slice(&1u32.to_le_bytes()); // shaders per paint job
        s.extend_from_slice(&lp("mytex"));
        for c in [1.0f32, 0.0, 0.0, 1.0] {
            s.extend_from_slice(&c.to_le_bytes());
        }
        for c in [0.2f32, 0.2, 0.2, 1.0] {
            s.extend_from_slice(&c.to_le_bytes());
        }
        for c in [1.0f32; 4] {
            s.extend_from_slice(&c.to_le_bytes());
        }
        for c in [0.0f32; 4] {
            s.extend_from_slice(&c.to_le_bytes());
        }
        s.extend_from_slice(&8.0f32.to_le_bytes());
        let data = pkg3(&[("shaders", s)]);
        let pkg = Pkg::parse(&data).unwrap();
        let shaders = pkg.shaders().unwrap();
        assert_eq!(shaders.paint_jobs, 1);
        assert_eq!(shaders.shaders[0].texture, "mytex");
        assert_eq!(shaders.shaders[0].diffuse[0], 1.0);
    }

    #[test]
    fn rejects_bad_magic() {
        let data = b"NOPE".to_vec();
        assert!(matches!(
            Pkg::parse(&data),
            Err(FormatError::BadMagic { .. })
        ));
    }

    #[test]
    fn rejects_truncated_chunk() {
        let mut data = pkg3(&[("BODY_H", geometry_chunk(FVF_XYZ))]);
        data.truncate(data.len() - 10);
        assert!(Pkg::parse(&data).is_err());
    }

    #[test]
    fn keeps_unknown_chunks_raw() {
        let data = pkg3(&[("mystery", vec![1, 2, 3])]);
        let pkg = Pkg::parse(&data).unwrap();
        assert!(matches!(pkg.files[0].data, PkgChunk::Raw(_)));
    }
}
