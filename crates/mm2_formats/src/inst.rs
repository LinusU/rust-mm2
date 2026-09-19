//! Parser for MM2 `INST` object-placement files.
//!
//! An INST file is a flat sequence of placement records, each positioning a
//! PKG object in the city. Two record kinds exist:
//!
//! - *coordinate* components (type bit 7 clear): a full basis + origin
//! - *simple* components (type bit 7 set): position + heading + scale
//!
//! Provenance: `angel-file-formats/Midtown Madness 2/INST.md`.

use crate::{FormatError, Reader};

/// A full-basis placement: columns are the images of the unit axes and the
/// origin, i.e. `city = origin + M * pkg`.
#[derive(Debug, Clone)]
pub struct InstCoordinate {
    /// X axis image.
    pub x_axis: [f32; 3],
    /// Y axis image.
    pub y_axis: [f32; 3],
    /// Z axis image.
    pub z_axis: [f32; 3],
    /// Origin.
    pub origin: [f32; 3],
}

/// A compact placement: `location` plus a horizontal direction vector whose
/// magnitude defines the scale.
#[derive(Debug, Clone)]
pub struct InstSimple {
    /// Heading/scale vector X component.
    pub x_delta: f32,
    /// Heading/scale vector Z component.
    pub z_delta: f32,
    /// World position of the object origin.
    pub location: [f32; 3],
}

/// Placement payload.
#[derive(Debug, Clone)]
pub enum InstPlacement {
    /// Full coordinate-system placement.
    Coordinate(InstCoordinate),
    /// Compact placement.
    Simple(InstSimple),
}

/// One INST record.
#[derive(Debug, Clone)]
pub struct InstComponent {
    /// Room this object is attached to.
    pub room: u16,
    /// Modifier flags (paint job selection in the low bits; bit 8 = 0x100 is
    /// set on most monuments).
    pub modifiers: u16,
    /// PKG base name (no extension).
    pub package_name: String,
    /// Placement data.
    pub placement: InstPlacement,
}

/// Parse a whole INST file into its component list.
pub fn parse(data: &[u8]) -> Result<Vec<InstComponent>, FormatError> {
    let mut r = Reader::new(data);
    let mut out = Vec::new();
    while !r.is_empty() {
        let room = r.u16()?;
        let modifiers = r.u16()?;
        let type_byte = r.u8()?;
        let name_len = (type_byte & 0x7f) as usize;
        if name_len == 0 {
            return Err(FormatError::InvalidValue {
                offset: r.pos() - 1,
                field: "type",
                value: type_byte as u64,
                reason: "zero-length package name",
            });
        }
        let name_bytes = r.bytes(name_len)?;
        let package_name = name_bytes
            .split(|&b| b == 0)
            .next()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        let placement = if type_byte & 0x80 == 0 {
            InstPlacement::Coordinate(InstCoordinate {
                x_axis: r.vec3()?,
                y_axis: r.vec3()?,
                z_axis: r.vec3()?,
                origin: r.vec3()?,
            })
        } else {
            InstPlacement::Simple(InstSimple {
                x_delta: r.f32()?,
                z_delta: r.f32()?,
                location: r.vec3()?,
            })
        };
        out.push(InstComponent {
            room,
            modifiers,
            package_name,
            placement,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_coordinate_component() {
        let mut d = Vec::new();
        d.extend_from_slice(&7u16.to_le_bytes()); // room
        d.extend_from_slice(&0x100u16.to_le_bytes()); // modifiers
        let name = b"bigben\0";
        d.push(name.len() as u8); // top bit clear = coordinate
        d.extend_from_slice(name);
        for f in [1.0f32, 0., 0., 0., 1., 0., 0., 0., 1., 10., 20., 30.] {
            d.extend_from_slice(&f.to_le_bytes());
        }
        let comps = parse(&d).unwrap();
        assert_eq!(comps.len(), 1);
        assert_eq!(comps[0].room, 7);
        assert_eq!(comps[0].package_name, "bigben");
        match &comps[0].placement {
            InstPlacement::Coordinate(c) => assert_eq!(c.origin, [10., 20., 30.]),
            _ => panic!("expected coordinate placement"),
        }
    }

    #[test]
    fn parses_simple_component() {
        let mut d = Vec::new();
        d.extend_from_slice(&3u16.to_le_bytes());
        d.extend_from_slice(&0u16.to_le_bytes());
        let name = b"lamp\0";
        d.push(0x80 | name.len() as u8);
        d.extend_from_slice(name);
        d.extend_from_slice(&1.0f32.to_le_bytes());
        d.extend_from_slice(&0.0f32.to_le_bytes());
        for f in [5.0f32, 0., -3.] {
            d.extend_from_slice(&f.to_le_bytes());
        }
        let comps = parse(&d).unwrap();
        match &comps[0].placement {
            InstPlacement::Simple(s) => {
                assert_eq!(s.location, [5., 0., -3.]);
                assert_eq!(s.x_delta, 1.0);
            }
            _ => panic!("expected simple placement"),
        }
    }

    #[test]
    fn rejects_truncated_record() {
        let d = [1u8, 0, 0, 0, 0x85]; // header + name len 5 but no data
        assert!(parse(&d).is_err());
    }
}
