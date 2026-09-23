//! Parser for MM2 `PSD0` city geometry files.
//!
//! A PSDL file contains a vertex pool, a height pool, a texture-name table,
//! a list of rooms (perimeter + attribute stream), room flags, prop-rule
//! indices, terrain bounds and prop paths.
//!
//! Provenance: `angel-file-formats/Midtown Madness 2/PSDL.md` and
//! `Room_attributes.md`, verified against `city/london.psdl` and
//! `city/sf.psdl` from a retail installation (all 1341 rooms of London parse
//! without remainder).
//!
//! Attribute word layout (verified, differs from the older interpretation in
//! `PSDL.md`): the low byte packs `subtype` (bits 0-2), `type` (bits 3-6)
//! and `last-attribute` (bit 7); the high byte is zero.

use crate::{FormatError, Reader};

/// PSDL file magic.
pub const PSDL_MAGIC: &[u8; 4] = b"PSD0";

/// A perimeter point: a vertex index plus the index of a neighbouring room
/// (`0` = no connection, otherwise room index + 1).
#[derive(Debug, Clone, Copy)]
pub struct PerimeterPoint {
    /// Index into the vertex pool.
    pub vertex: u16,
    /// Connecting room index + 1, or 0 for none.
    pub room: u16,
}

/// Room attribute types observed/documented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeType {
    /// 0x00 — road strip with sidewalks (4 vertices per cross-section).
    RoadWithSidewalks,
    /// 0x01 — sidewalk strip (2 vertices per cross-section).
    SidewalkStrip,
    /// 0x02 — road/walkway without sidewalks (2 vertices per cross-section).
    RoadNoSidewalks,
    /// 0x03 — sliver (slanted-bottom facade piece).
    Sliver,
    /// 0x04 — crosswalk rectangle.
    Crosswalk,
    /// 0x05 — road triangle fan.
    RoadFan,
    /// 0x06 — generic triangle fan (ground surfaces).
    Fan,
    /// 0x07 — facade collision bound.
    FacadeBound,
    /// 0x08 — divided road (6 vertices per cross-section).
    DividedRoad,
    /// 0x09 — tunnel/railing parameters.
    Tunnel,
    /// 0x0a — texture reference applying to subsequent attributes.
    TextureRef,
    /// 0x0b — facade (textured building side).
    Facade,
    /// 0x0c — roof triangle fan with height override.
    RoofFan,
    /// Any other type value.
    Unknown(u8),
}

impl AttributeType {
    fn from_raw(raw: u8) -> Self {
        match raw {
            0x00 => Self::RoadWithSidewalks,
            0x01 => Self::SidewalkStrip,
            0x02 => Self::RoadNoSidewalks,
            0x03 => Self::Sliver,
            0x04 => Self::Crosswalk,
            0x05 => Self::RoadFan,
            0x06 => Self::Fan,
            0x07 => Self::FacadeBound,
            0x08 => Self::DividedRoad,
            0x09 => Self::Tunnel,
            0x0a => Self::TextureRef,
            0x0b => Self::Facade,
            0x0c => Self::RoofFan,
            other => Self::Unknown(other),
        }
    }
}

/// A decoded room attribute. `refs` holds the trailing index data; its
/// meaning depends on the attribute type (vertex indices, height indices,
/// counts or packed parameter fields).
#[derive(Debug, Clone)]
pub struct RoomAttribute {
    /// Whether this was marked as the room's final rendered attribute.
    pub last: bool,
    /// Attribute type.
    pub kind: AttributeType,
    /// Raw subtype field.
    pub subtype: u8,
    /// Data shorts following the attribute word.
    pub data: Vec<u16>,
}

/// Decode a `TextureRef` attribute to its [`Psdl::textures`] index:
/// `data + 256 * subtype - 1` (the subtype carries the index's high
/// byte — the retail loader masks the attribute word's low 3 bits as
/// the subtype). Raw 0 is the suppression sentinel, so it and an empty
/// `data` yield `None`.
///
/// The retail city loader reads exactly this field of a room's *first*
/// attribute to classify the room's surface (water marking); non-first
/// attributes are bound to the geometry that follows them.
pub fn texture_ref_index(attr: &RoomAttribute) -> Option<usize> {
    let raw = attr.data.first().copied().unwrap_or(0) as usize + (attr.subtype as usize) * 256;
    raw.checked_sub(1)
}

/// A single room (city block analogue) of the PSDL.
#[derive(Debug, Clone)]
pub struct PsdlRoom {
    /// Perimeter points surrounding the room.
    pub perimeter: Vec<PerimeterPoint>,
    /// Decoded attributes, in file order.
    pub attributes: Vec<RoomAttribute>,
    /// Trailing words that could not be decoded because an attribute of
    /// unknown size was encountered.
    pub unparsed_attributes: Vec<u16>,
}

/// A room path (prop path) record. Field names follow the doc but are known
/// to be partially wrong; values are preserved verbatim.
#[derive(Debug, Clone)]
pub struct RoomPath {
    /// Undocumented field.
    pub unknown4: u16,
    /// Undocumented field.
    pub unknown5: u16,
    /// Lane densities, `n_forward + n_backward` entries.
    pub density: Vec<f32>,
    /// Undocumented flags.
    pub unknown6: u16,
    /// Vertices of the road part of the start crossing.
    pub start_crossroads: [u16; 4],
    /// Vertices of the road part of the end crossing.
    pub end_crossroads: [u16; 4],
    /// Room indices (+1) of the rooms this path passes through.
    pub road_rooms: Vec<u16>,
}

/// A parsed PSDL file.
#[derive(Debug, Clone)]
pub struct Psdl {
    /// Undocumented header field (always 2 in observed files).
    pub target_size: u32,
    /// Vertex pool referenced by room attributes.
    pub vertices: Vec<[f32; 3]>,
    /// Height pool referenced by facade/roof attributes.
    pub heights: Vec<f32>,
    /// Texture name table (index 0 is unused; names are stored without
    /// extension).
    pub textures: Vec<String>,
    /// Rooms. Note the file stores `n_rooms - 1` room records; index 0 is
    /// reserved by convention.
    pub rooms: Vec<PsdlRoom>,
    /// Per-room flag bytes (see PSDL.md bit table). Bit positions match
    /// mm2hook's recovered `RoomFlags` enum: `UnhitBanger` 0x01,
    /// `Subterranean` 0x02, `Water` 0x04, `Road` 0x08, `Intersection`
    /// 0x10, `SpecialBound` 0x20, `Warp` 0x40, `Instance` 0x80 — on
    /// retail the six `.water`-referenced rooms carry 0x04.
    pub room_flags: Vec<u8>,
    /// Per-room prop-rule indices selecting `n{NN}left`/`n{NN}right`
    /// rows in the city's `proprules.csv` (see
    /// [`crate::proprules::PropRule::rule_key`]); 0 = no roadside
    /// props.
    pub prop_rules: Vec<u8>,
    /// Undocumented count stored next to the room count (junction count?).
    pub junction_count: u32,
    /// City bounding box minimum.
    pub bounds_min: [f32; 3],
    /// City bounding box maximum.
    pub bounds_max: [f32; 3],
    /// Bounding sphere centre.
    pub bounds_center: [f32; 3],
    /// Bounding sphere radius.
    pub bounds_radius: f32,
    /// Prop path definitions.
    pub paths: Vec<RoomPath>,
}

impl Psdl {
    /// Parse a PSDL file from bytes.
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(data);
        let magic = r.bytes(4)?;
        if magic != PSDL_MAGIC {
            return Err(FormatError::BadMagic {
                offset: 0,
                expected: "PSD0",
                found: magic.to_vec(),
            });
        }
        let target_size = r.u32()?;

        let n_vertices = checked_count(r.u32()?, r.pos() - 4, "nVertices", 1 << 22)?;
        let mut vertices = Vec::with_capacity(n_vertices);
        for _ in 0..n_vertices {
            vertices.push(r.vec3()?);
        }

        let n_floats = checked_count(r.u32()?, r.pos() - 4, "nFloats", 1 << 20)?;
        let mut heights = Vec::with_capacity(n_floats);
        for _ in 0..n_floats {
            heights.push(r.f32()?);
        }

        let n_textures = r.u32()?;
        if n_textures == 0 || n_textures > 1 << 16 {
            return Err(FormatError::InvalidValue {
                offset: r.pos() - 4,
                field: "nTextures",
                value: n_textures as u64,
                reason: "implausible texture count",
            });
        }
        let mut textures = Vec::with_capacity(n_textures as usize - 1);
        for _ in 0..n_textures - 1 {
            textures.push(r.lp_string()?);
        }

        let n_rooms = checked_count(r.u32()?, r.pos() - 4, "nRooms", 1 << 20)?;
        let junction_count = r.u32()?;

        let mut rooms = Vec::with_capacity(n_rooms.saturating_sub(1));
        for _ in 0..n_rooms.saturating_sub(1) {
            rooms.push(parse_room(&mut r)?);
        }

        let mut room_flags = Vec::with_capacity(n_rooms);
        for _ in 0..n_rooms {
            room_flags.push(r.u8()?);
        }
        let mut prop_rules = Vec::with_capacity(n_rooms);
        for _ in 0..n_rooms {
            prop_rules.push(r.u8()?);
        }

        let bounds_min = r.vec3()?;
        let bounds_max = r.vec3()?;
        let bounds_center = r.vec3()?;
        let bounds_radius = r.f32()?;

        let n_paths = checked_count(r.u32()?, r.pos() - 4, "nPaths", 1 << 20)?;
        let mut paths = Vec::with_capacity(n_paths);
        for _ in 0..n_paths {
            paths.push(parse_path(&mut r)?);
        }

        Ok(Self {
            target_size,
            vertices,
            heights,
            textures,
            rooms,
            room_flags,
            prop_rules,
            junction_count,
            bounds_min,
            bounds_max,
            bounds_center,
            bounds_radius,
            paths,
        })
    }
}

fn checked_count(
    count: u32,
    offset: usize,
    field: &'static str,
    max: u32,
) -> Result<usize, FormatError> {
    if count > max {
        return Err(FormatError::InvalidValue {
            offset,
            field,
            value: count as u64,
            reason: "implausible count",
        });
    }
    Ok(count as usize)
}

fn parse_room(r: &mut Reader<'_>) -> Result<PsdlRoom, FormatError> {
    let n_perimeter = r.u32()? as usize;
    let attr_words = r.u32()? as usize;
    if n_perimeter > 1 << 16 || attr_words > 1 << 20 {
        return Err(FormatError::InvalidValue {
            offset: r.pos() - 8,
            field: "room header",
            value: ((n_perimeter as u64) << 32) | attr_words as u64,
            reason: "implausible room size",
        });
    }
    let mut perimeter = Vec::with_capacity(n_perimeter);
    for _ in 0..n_perimeter {
        perimeter.push(PerimeterPoint {
            vertex: r.u16()?,
            room: r.u16()?,
        });
    }
    let attr_bytes = attr_words * 2;
    let raw = r.bytes(attr_bytes)?;
    let (attributes, unparsed_attributes) = decode_attributes(raw);
    Ok(PsdlRoom {
        perimeter,
        attributes,
        unparsed_attributes,
    })
}

/// Decode a room's attribute stream. Stops at the first attribute whose
/// length cannot be determined; the remainder is returned unparsed.
fn decode_attributes(raw: &[u8]) -> (Vec<RoomAttribute>, Vec<u16>) {
    let words: Vec<u16> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .collect();
    let mut attrs = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        let last = word & 0x80 != 0;
        let kind = AttributeType::from_raw(((word >> 3) & 0x0f) as u8);
        let subtype = (word & 0x07) as u8;
        let len = attribute_data_len(kind, subtype, &words, i + 1);
        let Some(len) = len else {
            break;
        };
        if i + 1 + len > words.len() {
            break;
        }
        attrs.push(RoomAttribute {
            last,
            kind,
            subtype,
            data: words[i + 1..i + 1 + len].to_vec(),
        });
        i += 1 + len;
    }
    (attrs, words[i..].to_vec())
}

/// Number of data shorts following an attribute word, or `None` if the size
/// cannot be determined (unknown types or truncated counters).
fn attribute_data_len(
    kind: AttributeType,
    subtype: u8,
    words: &[u16],
    data_start: usize,
) -> Option<usize> {
    let inline = |per_item: usize| -> Option<usize> {
        if subtype == 0 {
            let n = *words.get(data_start)? as usize;
            Some(1 + n * per_item)
        } else {
            Some(subtype as usize * per_item)
        }
    };
    match kind {
        AttributeType::TextureRef => Some(1),
        AttributeType::Fan | AttributeType::RoadFan => {
            // subtype == 0 → count word = nTriangles; else subtype = nTriangles
            if subtype == 0 {
                let n = *words.get(data_start)? as usize;
                Some(1 + n + 2)
            } else {
                Some(subtype as usize + 2)
            }
        }
        AttributeType::RoofFan => {
            // subtype == 0 → count word = nVertices - 1; else subtype = nVertices - 1
            if subtype == 0 {
                let n = *words.get(data_start)? as usize;
                Some(1 + 1 + n + 1)
            } else {
                Some(1 + subtype as usize + 1)
            }
        }
        AttributeType::RoadWithSidewalks => inline(4),
        AttributeType::DividedRoad => inline(6).map(|n| n + 2), // flags/texture/value
        AttributeType::SidewalkStrip | AttributeType::RoadNoSidewalks => inline(2),
        // Fixed-size attributes: subtype equals the number of data shorts.
        AttributeType::Sliver
        | AttributeType::Crosswalk
        | AttributeType::FacadeBound
        | AttributeType::Facade => Some(subtype as usize),
        AttributeType::Tunnel => {
            if subtype == 0 {
                // Junction tunnel: nSize word holds the remaining short count.
                let n = *words.get(data_start)? as usize;
                Some(1 + n)
            } else {
                // Subtype equals the attribute size in shorts after the header.
                Some(subtype as usize)
            }
        }
        AttributeType::Unknown(_) => None,
    }
}

fn parse_path(r: &mut Reader<'_>) -> Result<RoomPath, FormatError> {
    let unknown4 = r.u16()?;
    let unknown5 = r.u16()?;
    let n_f = r.u8()? as usize;
    let n_b = r.u8()? as usize;
    let mut density = Vec::with_capacity(n_f + n_b);
    for _ in 0..n_f + n_b {
        density.push(r.f32()?);
    }
    let unknown6 = r.u16()?;
    let mut start_crossroads = [0u16; 4];
    let mut end_crossroads = [0u16; 4];
    for v in &mut start_crossroads {
        *v = r.u16()?;
    }
    for v in &mut end_crossroads {
        *v = r.u16()?;
    }
    let n_rooms = r.u8()? as usize;
    let mut road_rooms = Vec::with_capacity(n_rooms);
    for _ in 0..n_rooms {
        road_rooms.push(r.u16()?);
    }
    Ok(RoomPath {
        unknown4,
        unknown5,
        density,
        unknown6,
        start_crossroads,
        end_crossroads,
        road_rooms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribute_word_layout() {
        // 0x5e = last:0 type:0xb subtype:6 (facade) — verified in london.psdl
        let raw = [
            0x5e, 0x00, 1, 0, 2, 0, 5, 0, 2, 0, 0x10, 0, 0x11, 0, // facade
            0x50, 0x00, 0x2a, 0x00, // texture ref, data 0x2a
        ];
        let (attrs, rest) = decode_attributes(&raw);
        assert!(rest.is_empty());
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0].kind, AttributeType::Facade);
        assert_eq!(attrs[0].data.len(), 6);
        assert_eq!(attrs[1].kind, AttributeType::TextureRef);
        assert_eq!(attrs[1].data, vec![0x2a]);
    }

    #[test]
    fn fan_with_explicit_count() {
        // type 0x06 subtype 0 → next word = nTriangles, then nTriangles+2 refs
        let raw = [
            0x30, 0x00, // attr: type 6, subtype 0
            0x02, 0x00, // 2 triangles
            1, 0, 2, 0, 3, 0, 4, 0,
        ];
        let (attrs, rest) = decode_attributes(&raw);
        assert!(rest.is_empty());
        assert_eq!(attrs[0].kind, AttributeType::Fan);
        assert_eq!(attrs[0].subtype, 0);
        assert_eq!(attrs[0].data, vec![2, 1, 2, 3, 4]);
    }

    #[test]
    fn fan_with_inline_count() {
        // type 0x06 subtype 3 → 3 triangles, subtype+2 = 5 vertex refs,
        // no count word.
        let raw = [
            0x33, 0x00, // attr: type 6, subtype 3
            1, 0, 2, 0, 3, 0, 4, 0, 5, 0,
        ];
        let (attrs, rest) = decode_attributes(&raw);
        assert!(rest.is_empty());
        assert_eq!(attrs[0].kind, AttributeType::Fan);
        assert_eq!(attrs[0].subtype, 3);
        assert_eq!(attrs[0].data, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn road_with_explicit_count() {
        // type 0x00 subtype 0 → count word = nSections, then nSections*4 refs
        let raw = [
            0x00, 0x00, // attr: type 0, subtype 0
            0x02, 0x00, // 2 sections
            1, 0, 2, 0, 3, 0, 4, 0, // section 1
            5, 0, 6, 0, 7, 0, 8, 0, // section 2
        ];
        let (attrs, rest) = decode_attributes(&raw);
        assert!(rest.is_empty());
        assert_eq!(attrs[0].kind, AttributeType::RoadWithSidewalks);
        assert_eq!(attrs[0].subtype, 0);
        assert_eq!(attrs[0].data, vec![2, 1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn road_with_inline_count() {
        // type 0x02 subtype 2 → 2 sections of 2 refs, no count word.
        let raw = [
            0x12, 0x00, // attr: type 2, subtype 2
            10, 0, 11, 0, 12, 0, 13, 0,
        ];
        let (attrs, rest) = decode_attributes(&raw);
        assert!(rest.is_empty());
        assert_eq!(attrs[0].kind, AttributeType::RoadNoSidewalks);
        assert_eq!(attrs[0].data, vec![10, 11, 12, 13]);
    }

    #[test]
    fn unknown_attribute_stops_decoding() {
        let raw = [0xf8, 0x00, 1, 0, 2, 0]; // type 0x1f & 0xf = 0xf unknown
        let (attrs, rest) = decode_attributes(&raw);
        assert!(attrs.is_empty());
        assert_eq!(rest.len(), 3);
    }

    #[test]
    fn last_flag_is_preserved() {
        // 0x80 | 0x50 = texture ref marked last
        let raw = [0xd0, 0x00, 5, 0];
        let (attrs, _) = decode_attributes(&raw);
        assert!(attrs[0].last);
    }
}
