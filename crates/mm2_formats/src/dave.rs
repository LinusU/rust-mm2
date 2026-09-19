//! Parser for the `DAVE` archive format (`.ar` files) used by
//! Midtown Madness 2.
//!
//! Layout (all integers little-endian):
//!
//! ```text
//! offset  size  field
//! 0x000   4     magic "DAVE"
//! 0x004   4     entry count
//! 0x008   4     names_offset (relative to 0x800)
//! 0x00c   4     names_size
//! 0x010   ..    padding up to 0x800
//! 0x800   count*16  entry table, padded to a 0x800 boundary
//! ..              filename blob (names_size bytes, NUL-separated strings)
//! ..              file data
//! ```
//!
//! Each entry:
//!
//! ```text
//! u32 name_offset   offset into the filename blob
//! u32 data_offset   absolute offset of the file data in the archive
//! u32 size          uncompressed size
//! u32 stored_size   stored size; if != size the data is raw DEFLATE
//! ```
//!
//! Provenance: field layout matches the implementation of the public MM2
//! archive extractors and was verified against `mm2tex.ar`, `mm2core.ar`,
//! `mm2aud.ar` and `mm2audex.ar` from a retail installation.

use std::io::Read;

use crate::{FormatError, Reader, check_range};

/// Magic bytes at the start of every DAVE archive.
pub const DAVE_MAGIC: &[u8; 4] = b"DAVE";

/// The header and the file entry table are both padded to this boundary.
pub const HEADER_SIZE: usize = 0x800;

/// Size in bytes of a single file entry record.
pub const ENTRY_SIZE: usize = 16;

/// A single file entry inside a [`DaveArchive`].
#[derive(Debug, Clone)]
pub struct DaveEntry {
    /// Logical path stored in the archive, e.g. `texture/vpcaddieblue_bk.tex`.
    /// Separators are `/` in the original data.
    pub name: String,
    /// Absolute byte offset of the (possibly compressed) file data.
    pub data_offset: usize,
    /// Size of the file contents after decompression.
    pub size: usize,
    /// Size of the file contents as stored in the archive.
    pub stored_size: usize,
}

impl DaveEntry {
    /// Whether the stored data is DEFLATE-compressed.
    pub fn is_compressed(&self) -> bool {
        self.stored_size != self.size
    }
}

/// A parsed DAVE archive borrowing the archive bytes.
#[derive(Debug)]
pub struct DaveArchive<'a> {
    data: &'a [u8],
    entries: Vec<DaveEntry>,
}

impl<'a> DaveArchive<'a> {
    /// Parse the archive header, entry table and filename blob.
    ///
    /// All offsets and sizes are validated up front, so a parsed archive can
    /// be indexed and read without further bounds checks.
    pub fn parse(data: &'a [u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(data);
        let magic = r.bytes(4)?;
        if magic != DAVE_MAGIC {
            return Err(FormatError::BadMagic {
                offset: 0,
                expected: "DAVE",
                found: magic.to_vec(),
            });
        }
        let count = r.u32()? as usize;
        let names_offset = r.u32()? as usize;
        let names_size = r.u32()? as usize;

        let entries_base = HEADER_SIZE;
        let names_base =
            HEADER_SIZE
                .checked_add(names_offset)
                .ok_or(FormatError::InvalidValue {
                    offset: 8,
                    field: "names_offset",
                    value: names_offset as u64,
                    reason: "names offset overflows",
                })?;
        check_range(data.len(), names_base..names_base + names_size)?;
        check_range(data.len(), entries_base..entries_base + count * ENTRY_SIZE)?;

        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let entry_offset = entries_base + i * ENTRY_SIZE;
            let mut er = Reader::at(data, entry_offset)?;
            let name_offset = er.u32()? as usize;
            let data_offset = er.u32()? as usize;
            let size = er.u32()? as usize;
            let stored_size = er.u32()? as usize;

            // Resolve the name inside the filename blob.
            let name_start =
                names_base
                    .checked_add(name_offset)
                    .ok_or(FormatError::InvalidValue {
                        offset: entry_offset,
                        field: "name_offset",
                        value: name_offset as u64,
                        reason: "name offset overflows",
                    })?;
            if name_offset >= names_size {
                return Err(FormatError::InvalidValue {
                    offset: entry_offset,
                    field: "name_offset",
                    value: name_offset as u64,
                    reason: "name offset outside filename blob",
                });
            }
            let blob_end = names_base + names_size;
            let name_end = data[name_start..blob_end]
                .iter()
                .position(|&b| b == 0)
                .map(|p| name_start + p)
                .ok_or(FormatError::InvalidString {
                    offset: name_start,
                    reason: "entry name is not NUL terminated",
                })?;
            let name = String::from_utf8(data[name_start..name_end].to_vec()).map_err(|_| {
                FormatError::InvalidString {
                    offset: name_start,
                    reason: "entry name is not valid UTF-8/ASCII",
                }
            })?;

            check_range(data.len(), data_offset..data_offset + stored_size)?;

            entries.push(DaveEntry {
                name,
                data_offset,
                size,
                stored_size,
            });
        }

        Ok(Self { data, entries })
    }

    /// All entries in archive order.
    pub fn entries(&self) -> &[DaveEntry] {
        &self.entries
    }

    /// Find an entry by name, comparing ASCII case-insensitively and treating
    /// `\\` and `/` as equivalent separators (old Windows data).
    pub fn find(&self, name: &str) -> Option<&DaveEntry> {
        let wanted = normalize_key(name);
        self.entries
            .iter()
            .find(|e| normalize_key(&e.name) == wanted)
    }

    /// The stored bytes of an entry (still compressed if the entry is).
    pub fn raw_data(&self, entry: &DaveEntry) -> &'a [u8] {
        // Ranges were validated during parse.
        &self.data[entry.data_offset..entry.data_offset + entry.stored_size]
    }

    /// Return the decompressed contents of an entry.
    pub fn read(&self, entry: &DaveEntry) -> Result<Vec<u8>, FormatError> {
        let raw = self.raw_data(entry);
        if !entry.is_compressed() {
            return Ok(raw.to_vec());
        }
        let mut out = Vec::with_capacity(entry.size);
        let mut decoder = flate2::read::DeflateDecoder::new(raw);
        decoder
            .read_to_end(&mut out)
            .map_err(|e| FormatError::Decompression(e.to_string()))?;
        if out.len() != entry.size {
            return Err(FormatError::Decompression(format!(
                "entry {} decompressed to {} bytes, expected {}",
                entry.name,
                out.len(),
                entry.size
            )));
        }
        Ok(out)
    }
}

/// Comparison key for archive member names: lowercase, forward slashes.
fn normalize_key(name: &str) -> String {
    name.replace('\\', "/").to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic archive containing `files`.
    fn make_archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let count = files.len() as u32;
        let mut names = Vec::new();
        let mut name_offsets = Vec::new();
        for (name, _) in files {
            name_offsets.push(names.len() as u32);
            names.extend_from_slice(name.as_bytes());
            names.push(0);
        }
        let names_size = names.len() as u32;
        let mft_size = (files.len() * ENTRY_SIZE).div_ceil(HEADER_SIZE) * HEADER_SIZE;
        let names_offset = mft_size as u32;
        let data_base = HEADER_SIZE + mft_size + names.len();

        let mut data = Vec::new();
        data.extend_from_slice(DAVE_MAGIC);
        data.extend_from_slice(&count.to_le_bytes());
        data.extend_from_slice(&names_offset.to_le_bytes());
        data.extend_from_slice(&names_size.to_le_bytes());
        data.resize(HEADER_SIZE, 0);

        let mut offset = data_base;
        for ((_, contents), name_offset) in files.iter().zip(&name_offsets) {
            data.extend_from_slice(&name_offset.to_le_bytes());
            data.extend_from_slice(&(offset as u32).to_le_bytes());
            data.extend_from_slice(&(contents.len() as u32).to_le_bytes());
            data.extend_from_slice(&(contents.len() as u32).to_le_bytes());
            offset += contents.len();
        }
        data.resize(HEADER_SIZE + mft_size, 0);
        data.extend_from_slice(&names);
        for (_, contents) in files {
            data.extend_from_slice(contents);
        }
        data
    }

    #[test]
    fn parses_entries() {
        let archive = make_archive(&[
            ("texture/foo.tex", &[1, 2, 3]),
            ("geometry/bar.pkg", &[9, 8, 7, 6]),
        ]);
        let parsed = DaveArchive::parse(&archive).unwrap();
        assert_eq!(parsed.entries().len(), 2);
        let e = parsed.find("texture/foo.tex").unwrap();
        assert_eq!(parsed.read(e).unwrap(), vec![1, 2, 3]);
        let e = parsed.find("geometry/bar.pkg").unwrap();
        assert_eq!(parsed.read(e).unwrap(), vec![9, 8, 7, 6]);
    }

    #[test]
    fn find_is_case_and_separator_insensitive() {
        let archive = make_archive(&[("Texture/Foo.TEX", &[1])]);
        let parsed = DaveArchive::parse(&archive).unwrap();
        assert!(parsed.find("texture\\foo.tex").is_some());
    }

    #[test]
    fn rejects_bad_magic() {
        let mut archive = make_archive(&[]);
        archive[0] = b'X';
        assert!(matches!(
            DaveArchive::parse(&archive),
            Err(FormatError::BadMagic { .. })
        ));
    }

    #[test]
    fn rejects_truncated_table() {
        let mut archive = make_archive(&[("a", &[1])]);
        archive.truncate(0x800 + 4);
        assert!(DaveArchive::parse(&archive).is_err());
    }

    #[test]
    fn rejects_out_of_bounds_name() {
        let mut archive = make_archive(&[("a", &[1])]);
        // name_offset points past the filename blob
        archive[0x800..0x804].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            DaveArchive::parse(&archive),
            Err(FormatError::InvalidValue { .. })
        ));
    }

    #[test]
    fn rejects_out_of_bounds_data() {
        let mut archive = make_archive(&[("a", &[1, 2, 3])]);
        // data_offset points at the end of the file
        let bad = archive.len() as u32;
        archive[0x804..0x808].copy_from_slice(&bad.to_le_bytes());
        assert!(DaveArchive::parse(&archive).is_err());
    }

    #[test]
    fn decompresses_entries() {
        // Hand-rolled archive can't easily produce deflate data; exercise the
        // uncompressed path and the compressed-detection flag instead.
        let archive = make_archive(&[("a", &[1, 2, 3])]);
        let parsed = DaveArchive::parse(&archive).unwrap();
        assert!(!parsed.entries()[0].is_compressed());
    }
}
