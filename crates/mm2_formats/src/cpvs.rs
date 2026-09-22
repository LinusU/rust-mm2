//! Parser for MM2 `.cpvs` potentially-visible-set files and the `.pvshist`
//! per-room visibility histograms that accompany them.
//!
//! Binary layout (`angel-file-formats/Midtown Madness 2/CPVS.md`, corrected
//! against the reference decoder and measured on `city/london.cpvs` /
//! `city/sf.cpvs`):
//!
//! ```text
//! char[4]   magic = "PVS0"
//! u32       index_count        // number of index slots; index[0] is
//!                              // implicitly 0 and is NOT stored
//! u32       indices[index_count - 1]
//! u8[rest]  payload            // RLE-compressed visibility lists
//! ```
//!
//! There are `index_count - 1` lists; list `i` covers
//! `payload[indices[i]..indices[i + 1]]` (the last list runs to EOF).
//! Retail list count equals the PSDL room count + 1: London 1343 indices →
//! 1342 lists for rooms 0..=1341 (list 0 is empty — PSDL room 0 is
//! reserved).
//!
//! List RLE: a control byte `n`; `n >= 0x80` copies the next `n - 127` bytes
//! verbatim, `n < 0x80` repeats the next byte `n` times. (The community
//! doc's "+1" on the fill count and its "first two bits reserved" claim are
//! both wrong: measured on retail, room `r` occupies bits `2*(r % 4)` of
//! byte `r / 4` — 1340/1341 London rooms mark themselves visible under this
//! mapping vs 888 under the doc's offset — and every nonzero code is `0b11`,
//! never the documented `01`/`10`. MM2Hook's `cityLevel::IsRoomVisible`
//! reads the same slot arithmetic.)
//!
//! Decompressed lists are variable length and end at the last nonzero byte;
//! trailing invisible rooms are implicit.

use crate::{FormatError, Reader};

/// `.cpvs` magic.
pub const CPVS_MAGIC: &[u8; 4] = b"PVS0";

const MAX_INDEX_COUNT: usize = 1 << 20;
/// Decompressed-list size cap. Retail max is 336 bytes
/// (`ceil(1342 / 4)`); the cap leaves generous headroom while keeping a
/// hostile count from allocating unbounded output.
pub const MAX_LIST_BYTES: usize = 8192;

/// A parsed `.cpvs` file.
#[derive(Debug, Clone)]
pub struct Cpvs {
    /// List offsets into [`Cpvs::payload`], `indices[0]` = 0 implicit,
    /// followed by the `index_count - 1` stored values.
    pub indices: Vec<u32>,
    /// RLE-compressed list data.
    pub payload: Vec<u8>,
}

impl Cpvs {
    /// Parse a `.cpvs` file.
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(data);
        let magic = r.bytes(4)?;
        if magic != CPVS_MAGIC {
            return Err(FormatError::BadMagic {
                offset: 0,
                expected: "PVS0",
                found: magic.to_vec(),
            });
        }
        let index_count = r.u32()? as usize;
        if index_count == 0 || index_count > MAX_INDEX_COUNT {
            return Err(FormatError::InvalidValue {
                offset: 4,
                field: "index_count",
                value: index_count as u64,
                reason: "index count is zero or exceeds sanity cap",
            });
        }
        // index[0] is implicit; the stored table must fit the input.
        let stored = index_count - 1;
        if stored > data.len() / 4 {
            return Err(FormatError::InvalidValue {
                offset: 4,
                field: "index_count",
                value: index_count as u64,
                reason: "index table extends past end of data",
            });
        }
        let mut indices = Vec::with_capacity(index_count);
        indices.push(0);
        for _ in 0..stored {
            indices.push(r.u32()?);
        }
        let payload = r.rest().to_vec();
        Ok(Cpvs { indices, payload })
    }

    /// Number of visibility lists (one per PSDL room id, room 0 empty on
    /// retail).
    pub fn list_count(&self) -> usize {
        self.indices.len().saturating_sub(1)
    }

    /// Byte range of list `i` inside [`Cpvs::payload`]; `None` for an
    /// out-of-range list or a non-monotonic/out-of-bounds index.
    fn list_range(&self, i: usize) -> Option<(usize, usize)> {
        if i >= self.list_count() {
            return None;
        }
        let start = *self.indices.get(i)? as usize;
        let end = self
            .indices
            .get(i + 1)
            .map(|&e| e as usize)
            .unwrap_or(self.payload.len());
        if start > end || end > self.payload.len() {
            return None;
        }
        Some((start, end))
    }

    /// Decompress list `i` (2-bit-per-room visibility mask; see module
    /// docs). Errors on out-of-range lists, corrupt indices, truncated RLE
    /// streams or output past [`MAX_LIST_BYTES`].
    pub fn decompress(&self, i: usize) -> Result<Vec<u8>, FormatError> {
        let (mut pos, end) = self.list_range(i).ok_or_else(|| {
            FormatError::parse(0, format!("cpvs list {i} out of range or corrupt index"))
        })?;
        let mut out = Vec::new();
        while pos < end {
            let n = self.payload[pos];
            pos += 1;
            if n >= 0x80 {
                let count = (n - 0x7f) as usize;
                if pos + count > end {
                    return Err(FormatError::parse(
                        pos,
                        format!("cpvs list {i}: literal run of {count} bytes overruns list"),
                    ));
                }
                out.extend_from_slice(&self.payload[pos..pos + count]);
                pos += count;
            } else {
                if pos >= end {
                    return Err(FormatError::parse(
                        pos,
                        format!("cpvs list {i}: fill run missing its byte"),
                    ));
                }
                let value = self.payload[pos];
                pos += 1;
                out.resize(out.len() + n as usize, value);
            }
            if out.len() > MAX_LIST_BYTES {
                return Err(FormatError::parse(
                    pos,
                    format!("cpvs list {i}: decompressed past {MAX_LIST_BYTES} bytes"),
                ));
            }
        }
        Ok(out)
    }

    /// The 2-bit visibility code of `room` in decompressed `list`
    /// (`0b11` = visible, `0b00` = not; `01`/`10` are documented but never
    /// appear on retail). Rooms past the list's stored length are
    /// implicitly `0`.
    pub fn code(list: &[u8], room: usize) -> u8 {
        let byte = list.get(room / 4).copied().unwrap_or(0);
        (byte >> (2 * (room % 4))) & 3
    }

    /// Whether `room` is marked visible in decompressed `list` — nonzero
    /// code, matching the original `IsRoomVisible` predicate.
    pub fn is_visible(list: &[u8], room: usize) -> bool {
        Self::code(list, room) != 0
    }

    /// Decompress list `i` and yield the room ids marked visible.
    pub fn visible_rooms(&self, i: usize) -> Result<Vec<u32>, FormatError> {
        let d = self.decompress(i)?;
        let mut out = Vec::new();
        for room in 0..(d.len() * 4) {
            if Self::is_visible(&d, room) {
                out.push(room as u32);
            }
        }
        Ok(out)
    }

    /// Decompress every list and report findings. Expensive — intended for
    /// audits, not the runtime path.
    pub fn validate(&self) -> Vec<CpvsIssue> {
        let mut issues = Vec::new();
        for w in self.indices.windows(2) {
            if w[0] > w[1] {
                issues.push(CpvsIssue::NonMonotonicIndices);
                break;
            }
        }
        if let Some(&last) = self.indices.last()
            && last as usize > self.payload.len()
        {
            issues.push(CpvsIssue::IndexPastPayload {
                index: last,
                payload_len: self.payload.len(),
            });
        }
        for i in 0..self.list_count() {
            match self.decompress(i) {
                Ok(d) => {
                    for (b, &byte) in d.iter().enumerate() {
                        for s in 0..4 {
                            let c = (byte >> (2 * s)) & 3;
                            if c != 0 && c != 3 {
                                issues.push(CpvsIssue::UnknownCode {
                                    list: i,
                                    byte: b,
                                    code: c,
                                });
                            }
                        }
                    }
                    if !d.is_empty() && !Self::is_visible(&d, i) {
                        issues.push(CpvsIssue::SelfInvisible { list: i });
                    }
                }
                Err(_) => issues.push(CpvsIssue::UndecodableList { list: i }),
            }
        }
        issues
    }
}

/// Audit findings for a [`Cpvs`].
#[derive(Debug, Clone, PartialEq)]
pub enum CpvsIssue {
    /// The stored index table is not monotonically increasing.
    NonMonotonicIndices,
    /// A stored index points past the payload.
    IndexPastPayload {
        /// Offending index value.
        index: u32,
        /// Payload byte length.
        payload_len: usize,
    },
    /// A list's RLE stream failed to decode.
    UndecodableList {
        /// List index.
        list: usize,
    },
    /// A decompressed list carries a documented-but-unused `01`/`10` code.
    UnknownCode {
        /// List index.
        list: usize,
        /// Byte offset inside the decompressed list.
        byte: usize,
        /// The offending 2-bit value.
        code: u8,
    },
    /// A non-empty list does not mark its own room visible.
    SelfInvisible {
        /// List index (room id).
        list: usize,
    },
}

/// One `.pvshist` row: `from to weight` — room `from` was observed seeing
/// room `to` with the given history weight (255 saturates on retail; small
/// even values appear for rarely-observed pairs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PvsHistRow {
    /// Source room id.
    pub from: u32,
    /// Visible room id.
    pub to: u32,
    /// History weight (255 = strongest on retail).
    pub weight: u32,
}

/// A parsed `.pvshist` file: a whitespace-separated `from to weight`
/// table, one row per observed room pair (measured retail layout — exact
/// runtime semantics are inferred, not documented).
///
/// Retail ships the entry DAVE-compressed inside `mm2core.ar`; that is
/// archive storage, transparently inflated by the asset layer before this
/// parser ever sees the text.
#[derive(Debug, Clone)]
pub struct PvsHist {
    /// Parsed rows, in file order (retail rows sort by `from` then `to`).
    pub rows: Vec<PvsHistRow>,
}

impl PvsHist {
    /// Parse a `.pvshist` file. `input` is the file text; every
    /// non-blank row must hold exactly three non-negative integers.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let mut rows = Vec::new();
        for (line_no, line) in input.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut cols = line.split_whitespace();
            let mut col = |name: &str| -> Result<u32, FormatError> {
                let tok = cols.next().ok_or_else(|| {
                    FormatError::parse(line_no, format!("pvshist row missing {name} column"))
                })?;
                tok.parse::<u32>().map_err(|_| {
                    FormatError::parse(
                        line_no,
                        format!("pvshist {name} is not an integer: {tok:?}"),
                    )
                })
            };
            let (from, to, weight) = (col("from")?, col("to")?, col("weight")?);
            if cols.next().is_some() {
                return Err(FormatError::parse(
                    line_no,
                    "pvshist row has more than 3 columns".to_string(),
                ));
            }
            rows.push(PvsHistRow { from, to, weight });
        }
        Ok(PvsHist { rows })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a cpvs file from decompressed lists: all-equal lists encode as
    /// a single fill run, others as one literal run (tests keep them
    /// <= 128 bytes).
    fn build(lists: &[Vec<u8>]) -> Vec<u8> {
        let mut payload = Vec::new();
        let mut ends = Vec::new();
        for l in lists {
            if l.is_empty() {
                // zero payload bytes; the index still records its (empty) end
            } else if l.iter().all(|&b| b == l[0]) {
                payload.push(l.len() as u8); // fill
                payload.push(l[0]);
            } else {
                assert!(l.len() <= 128);
                payload.push(0x80 + (l.len() as u8) - 1);
                payload.extend_from_slice(l);
            }
            ends.push(payload.len() as u32);
        }
        let mut data = b"PVS0".to_vec();
        data.extend_from_slice(&((lists.len() + 1) as u32).to_le_bytes());
        for e in &ends {
            data.extend_from_slice(&e.to_le_bytes());
        }
        data.extend_from_slice(&payload);
        data
    }

    #[test]
    fn round_trip_and_codes() {
        // list0 empty (reserved room), list1 sees rooms 1,2,3 → byte 0xFC,
        // list2 sees rooms 0,1 → byte 0x0F.
        let d = build(&[vec![], vec![0xFC, 0x00], vec![0x0F]]);
        let cpvs = Cpvs::parse(&d).unwrap();
        assert_eq!(cpvs.list_count(), 3);
        assert_eq!(cpvs.decompress(0).unwrap(), Vec::<u8>::new());
        assert_eq!(cpvs.decompress(1).unwrap(), vec![0xFC, 0x00]);
        let l1 = cpvs.decompress(1).unwrap();
        assert!(!Cpvs::is_visible(&l1, 0));
        assert!(Cpvs::is_visible(&l1, 1));
        assert!(Cpvs::is_visible(&l1, 3));
        assert!(!Cpvs::is_visible(&l1, 4));
        assert_eq!(cpvs.visible_rooms(1).unwrap(), vec![1, 2, 3]);
        assert_eq!(cpvs.visible_rooms(2).unwrap(), vec![0, 1]);
    }

    #[test]
    fn fill_run_decodes() {
        let d = build(&[vec![0xAB; 40]]);
        let cpvs = Cpvs::parse(&d).unwrap();
        assert_eq!(cpvs.decompress(0).unwrap(), vec![0xAB; 40]);
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(Cpvs::parse(b"PVS1").is_err());
        assert!(Cpvs::parse(b"PVS0").is_err()); // no count
        // index_count larger than the table can hold.
        let mut d = b"PVS0".to_vec();
        d.extend_from_slice(&1000u32.to_le_bytes());
        assert!(Cpvs::parse(&d).is_err());
        // Truncated literal run.
        let mut d = b"PVS0".to_vec();
        d.extend_from_slice(&2u32.to_le_bytes()); // 2 indices, 1 list
        d.extend_from_slice(&2u32.to_le_bytes()); // stored index[1] = 2
        d.extend_from_slice(&[0x83, 0xAA]); // literal of 4, only 1 byte present
        let cpvs = Cpvs::parse(&d).unwrap();
        assert!(cpvs.decompress(0).is_err());
        // Non-monotonic index table: list 1's range is inverted.
        let mut d = b"PVS0".to_vec();
        d.extend_from_slice(&3u32.to_le_bytes()); // 2 lists
        d.extend_from_slice(&4u32.to_le_bytes()); // index[1] = 4
        d.extend_from_slice(&2u32.to_le_bytes()); // index[2] = 2 < index[1]
        d.extend_from_slice(&[0u8; 8]);
        let cpvs = Cpvs::parse(&d).unwrap();
        assert!(cpvs.decompress(1).is_err());
        // Fill runs past MAX_LIST_BYTES are rejected, not allocated.
        let mut d = b"PVS0".to_vec();
        d.extend_from_slice(&2u32.to_le_bytes());
        let runs = (MAX_LIST_BYTES / 0x7f + 2) as u32;
        d.extend_from_slice(&(runs * 2).to_le_bytes()); // index[1] = payload end
        for _ in 0..runs {
            d.extend_from_slice(&[0x7f, 0xAA]); // fill 127 x 0xAA
        }
        let cpvs = Cpvs::parse(&d).unwrap();
        assert!(cpvs.decompress(0).is_err());
    }

    #[test]
    fn validate_flags_unknown_codes_and_self_invisible() {
        // list1: room1 = code 0b01 (documented-unused); list2: sees nothing
        // including itself.
        let mut d = b"PVS0".to_vec();
        d.extend_from_slice(&4u32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes()); // idx1 = 0 → list0 empty
        d.extend_from_slice(&3u32.to_le_bytes()); // idx2 = 3
        d.extend_from_slice(&6u32.to_le_bytes()); // idx3 = 6
        d.extend_from_slice(&[0x81, 0x04, 0x00]); // list1 literal [0x04,0x00]
        d.extend_from_slice(&[0x81, 0x00, 0x00]); // list2 literal [0x00,0x00]
        let cpvs = Cpvs::parse(&d).unwrap();
        let issues = cpvs.validate();
        assert!(issues.contains(&CpvsIssue::UnknownCode {
            list: 1,
            byte: 0,
            code: 1
        }));
        assert!(issues.contains(&CpvsIssue::SelfInvisible { list: 2 }));
    }

    #[test]
    fn pvshist_rows_validate() {
        let hist = PvsHist::parse("   1    1 255\n   1    2 255\r\n   3 7 8\n\n").unwrap();
        assert_eq!(
            hist.rows,
            vec![
                PvsHistRow {
                    from: 1,
                    to: 1,
                    weight: 255
                },
                PvsHistRow {
                    from: 1,
                    to: 2,
                    weight: 255
                },
                PvsHistRow {
                    from: 3,
                    to: 7,
                    weight: 8
                },
            ]
        );
        assert!(PvsHist::parse("1 2 3 4\n").is_err());
        assert!(PvsHist::parse("1 two 3\n").is_err());
        assert!(PvsHist::parse("1 -2 3\n").is_err());
        assert!(PvsHist::parse("1 2\n").is_err());
    }
}
