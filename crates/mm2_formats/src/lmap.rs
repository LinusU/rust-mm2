//! Parser for MM2 `.lmap` lightmap index files.
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
//! reported as a finding rather than a decode failure.
//!
//! Retail contents are nearly constant: entry 0 is `0xCDCDCDCD` (the
//! classic uninitialised-memory fill the original's allocator stamped)
//! and every other entry is `-267` (`0xFFFFFEF5`). Whatever the per-room
//! slot means (a lightmap texture index/offset is the plausible reading)
//! it is uniform on stock content — preserved verbatim, semantics
//! unverified.

use crate::{FormatError, Reader};

/// `.lmap` magic.
pub const LMAP_MAGIC: &[u8; 4] = b"LMP0";

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
        for v in [-842150451i32, -267, -267] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        let l = Lmap::parse(&d).unwrap();
        assert_eq!(l.entries, vec![-842150451, -267, -267]);
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
