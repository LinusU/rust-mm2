//! Parser for MM2 `.lmap` per-room light-map index files.
//!
//! Binary layout (measured on `city/london.lmap` and `city/sf.lmap`):
//!
//! ```text
//! char[4]   magic = "LMP0"
//! u32       count
//! i32       entries[count]
//! ```
//!
//! London carries exactly one entry per PSDL room record (1341); San
//! Francisco carries 1125 against its 1171 records — a short table,
//! reported as a finding rather than a decode failure. Every retail
//! entry but the first is `-267` (`0xFFFFFEF5`); entry 0 is
//! [`LMAP_SENTINEL`] (`0xCDCDCDCD`, the original allocator's
//! uninitialised-heap fill, not an authored value).
//!
//! # Value semantics (recovered, not bound)
//!
//! `cityLevel::Load` in `Midtown2.exe` allocates one record per PSDL
//! room, opens `city/<stem>.lmap`, and rejects the table unless the
//! first dword is `LMP0` and the count equals the room count (it
//! diagnoses `wrong lightmap version` / `room count mismatch`). Each
//! entry is copied into the matching room record's `+0x1c` field; a
//! missing or rejected table leaves every room at `-1` (`0xFFFFFFFF`).
//! The room draw path (`0x4457a0`) decomposes that i32's low three
//! bytes, scales each by `1/255` (`[0x5af480]` = `0.00392157`) and
//! passes the `[r, g, b]` vector to `0x443690`, which multiplies the
//! room's three directional lights' per-channel colours. An entry is
//! therefore a **packed `0xAARRGGBB` per-room light scale** — the
//! `PackColorBGRA` convention the `.ltNN` ambient already uses; the
//! alpha byte is unused. [`light_scale`] unpacks it; the channel order
//! is inferred from that shared packing (in the exe the three bytes
//! scale the three light-colour fields).
//!
//! Retail's `-267` = `0xFFFFFEF5` unpacks to `r=255, g=254, b=245`
//! (scale ≈ `(1.000, 0.996, 0.961)`, a barely-warm full white);
//! `0xFFFFFFFF` is the loader's no-table default. The reading is
//! deliberately *not* bound to rendering: the original applies it per
//! room to that room's own lights, this engine's rig is session-global
//! (DSN-28), and the retail value is uniform — see
//! `docs/research/environment.md`.

use crate::{FormatError, Reader};

/// `.lmap` magic.
pub const LMAP_MAGIC: &[u8; 4] = b"LMP0";

/// The original allocator's uninitialised-heap fill (`0xCDCDCDCD`) that
/// retail `.lmap` entry 0 carries. Preserved verbatim: it is an artifact
/// of the shipped file, not an authored value or a format sentinel.
pub const LMAP_SENTINEL: i32 = -842_150_451;

/// Unpack a recovered `.lmap` entry into its per-channel light scale
/// `[r, g, b]` in `[0, 1]`, each byte scaled by `1/255` (see the module
/// docs). The channel order follows the `0xAARRGGBB` `PackColorBGRA`
/// packing the `.ltNN` ambient uses, so `r = (packed >> 16) & 0xFF`, `g`
/// the next byte and `b` the low byte; the alpha byte is ignored.
pub fn light_scale(packed: i32) -> [f32; 3] {
    let u = packed as u32;
    let byte = |shift: u32| (u >> shift) as u8 as f32 * (1.0 / 255.0);
    [byte(16), byte(8), byte(0)]
}

const MAX_ENTRIES: usize = 1 << 20;

/// A parsed `.lmap` file.
#[derive(Debug, Clone)]
pub struct Lmap {
    /// Per-record entries, verbatim.
    pub entries: Vec<i32>,
}

impl Lmap {
    /// Parse an `.lmap` file. The entry table must fill the file exactly.
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(data);
        let magic = r.bytes(4)?;
        if magic != LMAP_MAGIC {
            return Err(FormatError::BadMagic {
                offset: 0,
                expected: "LMP0",
                found: magic.to_vec(),
            });
        }
        let count = r.u32()? as usize;
        if count > MAX_ENTRIES {
            return Err(FormatError::InvalidValue {
                offset: 4,
                field: "count",
                value: count as u64,
                reason: "entry count exceeds sanity cap",
            });
        }
        if r.remaining() != count * 4 {
            return Err(FormatError::InvalidValue {
                offset: 4,
                field: "count",
                value: count as u64,
                reason: "entry table does not fill the file exactly",
            });
        }
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            entries.push(r.i32()?);
        }
        Ok(Lmap { entries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_retail_shape() {
        let mut d = b"LMP0".to_vec();
        d.extend_from_slice(&3u32.to_le_bytes());
        for v in [LMAP_SENTINEL, -267, -267] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        let l = Lmap::parse(&d).unwrap();
        assert_eq!(l.entries, vec![LMAP_SENTINEL, -267, -267]);
    }

    #[test]
    fn light_scale_unpacks_bgra() {
        // Retail body value `0xFFFFFEF5` → r=255, g=254, b=245.
        let [r, g, b] = light_scale(-267);
        assert!((r - 1.0).abs() < 1e-6);
        assert!((g - 254.0 / 255.0).abs() < 1e-6);
        assert!((b - 245.0 / 255.0).abs() < 1e-6);
        // The sentinel is uniform grey (`0xCDCDCDCD`).
        for c in light_scale(LMAP_SENTINEL) {
            assert!((c - 205.0 / 255.0).abs() < 1e-6);
        }
        // The loader's no-table default is full white.
        assert_eq!(light_scale(-1), [1.0; 3]);
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(Lmap::parse(b"LMP1").is_err());
        // count does not match the table size.
        let mut d = b"LMP0".to_vec();
        d.extend_from_slice(&4u32.to_le_bytes());
        d.extend_from_slice(&(-267i32).to_le_bytes());
        assert!(Lmap::parse(&d).is_err());
        // trailing junk.
        let mut d = b"LMP0".to_vec();
        d.extend_from_slice(&0u32.to_le_bytes());
        d.push(0);
        assert!(Lmap::parse(&d).is_err());
    }
}
