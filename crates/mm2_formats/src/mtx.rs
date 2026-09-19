//! Parser for MM2 `.mtx` part-transform files (`geometry/<car>_<part>.mtx`).
//!
//! These are 48-byte binary files holding four little-endian `f32` triples:
//!
//! ```text
//! [0..3)   bounding-box minimum of the part in car space
//! [3..6)   bounding-box maximum of the part in car space
//! [6..9)   pivot/rotation centre (usually zero for wheels)
//! [9..12)  part origin in car space (wheel centre, part attach point)
//! ```
//!
//! Verified against the stock installation: `whlN` origins give wheel
//! centres (front axle is local `-Z`), and `headlight0`, `breakN`, `srnN`,
//! `trailer_hitch` origins give attach points for geometry authored around
//! its own local origin.

use std::fmt;

/// One 48-byte part transform record.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mtx {
    /// Part bounding-box minimum in car space.
    pub bounds_min: [f32; 3],
    /// Part bounding-box maximum in car space.
    pub bounds_max: [f32; 3],
    /// Pivot/rotation centre (zero for most parts).
    pub pivot: [f32; 3],
    /// Part origin in car space.
    pub origin: [f32; 3],
}

/// `.mtx` parse error.
#[derive(Debug)]
pub struct MtxError(pub String);

impl fmt::Display for MtxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MtxError {}

impl Mtx {
    pub const BYTE_LEN: usize = 48;

    pub fn parse(data: &[u8]) -> Result<Self, MtxError> {
        if data.len() < Self::BYTE_LEN {
            return Err(MtxError(format!(
                "mtx file too short: {} bytes (need {})",
                data.len(),
                Self::BYTE_LEN
            )));
        }
        let f =
            |i: usize| -> f32 { f32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap()) };
        Ok(Mtx {
            bounds_min: [f(0), f(1), f(2)],
            bounds_max: [f(3), f(4), f(5)],
            pivot: [f(6), f(7), f(8)],
            origin: [f(9), f(10), f(11)],
        })
    }

    /// Wheel radius implied by the part bounds (half the vertical extent of
    /// the bounding box measured relative to `origin.y`). For wheels authored
    /// around their centre this is simply half the bbox height.
    pub fn wheel_radius(&self) -> f32 {
        (self.bounds_max[1] - self.bounds_min[1]).abs() * 0.5
    }

    /// Wheel width implied by the part bounds (x extent of the bounding box).
    pub fn wheel_width(&self) -> f32 {
        (self.bounds_max[0] - self.bounds_min[0]).abs()
    }
}
