//! Reader for the string table in `mmlang.dll`.
//!
//! The game's user-visible text lives in the language DLL as a standard
//! Win32 string table — resource type 6 (`RT_STRING`) of the PE
//! resource tree. Each leaf is a block of 16 strings, stored as a
//! 16-bit length followed by that many UTF-16LE code units (a zero
//! length is an empty slot); block `n` (the resource *name*, 1-based)
//! holds string ids `(n - 1) * 16 ..= (n - 1) * 16 + 15`. The executable
//! asks for a string by that id (`0x534770`).
//!
//! Only the grammar is read here. Which id the game shows where is
//! `mm2_game::lesson_text` (Crash Course) and the other consumers' own
//! business. A retail DLL ships a single language; when a table carries
//! several, the first language of each block wins.

use std::collections::BTreeMap;

use crate::{FormatError, Reader};

const RT_STRING: u32 = 6;
const STRINGS_PER_BLOCK: u32 = 16;
/// A resource tree this deep or wider is not a language DLL's.
const MAX_ENTRIES: u32 = 4096;

/// The decoded string table: id → text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StringTable {
    strings: BTreeMap<u32, String>,
}

impl StringTable {
    /// Parse the `RT_STRING` resources of a PE image.
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(data);
        r.seek(0)?;
        if r.bytes(2)? != b"MZ" {
            return Err(FormatError::BadMagic {
                offset: 0,
                expected: "MZ",
                found: data.iter().take(2).copied().collect(),
            });
        }
        r.seek(0x3c)?;
        let pe = r.u32()? as usize;
        r.seek(pe)?;
        if r.bytes(4)? != b"PE\0\0" {
            return Err(FormatError::BadMagic {
                offset: pe,
                expected: "PE\\0\\0",
                found: data.iter().skip(pe).take(4).copied().collect(),
            });
        }
        r.skip(2)?;
        let sections = r.u16()? as usize;
        r.skip(12)?;
        let optional = r.u16()? as usize;
        r.skip(2)?;
        let optional_start = r.pos();
        // Data directory 2 (resources) sits at +96 of a PE32 optional
        // header.
        r.seek(optional_start + 96 + 2 * 8)?;
        let resource_rva = r.u32()?;
        r.seek(optional_start + optional)?;
        let mut mapped = Vec::with_capacity(sections);
        for _ in 0..sections {
            r.skip(8)?;
            let virtual_size = r.u32()?;
            let virtual_address = r.u32()?;
            let raw_size = r.u32()?;
            let raw_ptr = r.u32()?;
            r.skip(16)?;
            mapped.push((virtual_address, virtual_size.max(raw_size), raw_ptr));
        }
        let rva_to_offset = |rva: u32| -> Result<usize, FormatError> {
            mapped
                .iter()
                .find(|(va, size, _)| rva >= *va && rva - *va < *size)
                .map(|(va, _, raw)| (rva - va) as usize + *raw as usize)
                .ok_or(FormatError::InvalidValue {
                    offset: 0,
                    field: "rva",
                    value: u64::from(rva),
                    reason: "not inside any section",
                })
        };
        if resource_rva == 0 {
            return Ok(Self::default());
        }
        let root = rva_to_offset(resource_rva)?;

        let mut strings = BTreeMap::new();
        for (kind, kind_target) in directory(data, root, 0)? {
            if kind != RT_STRING {
                continue;
            }
            for (block, block_target) in directory(data, root, kind_target)? {
                let Some((_, leaf_target)) =
                    directory(data, root, block_target)?.into_iter().next()
                else {
                    continue;
                };
                let mut leaf = Reader::at(data, root + leaf_target as usize)?;
                let data_rva = leaf.u32()?;
                let size = leaf.u32()? as usize;
                let start = rva_to_offset(data_rva)?;
                let mut s = Reader::at(data, start)?;
                let end = start.checked_add(size).filter(|e| *e <= data.len()).ok_or(
                    FormatError::InvalidValue {
                        offset: start,
                        field: "string block size",
                        value: size as u64,
                        reason: "block extends past end of data",
                    },
                )?;
                let base = block.saturating_sub(1).saturating_mul(STRINGS_PER_BLOCK);
                for slot in 0..STRINGS_PER_BLOCK {
                    if s.pos() >= end {
                        break;
                    }
                    let units = usize::from(s.u16()?);
                    if units == 0 {
                        continue;
                    }
                    let at = s.pos();
                    let wide = (0..units)
                        .map(|_| s.u16())
                        .collect::<Result<Vec<u16>, _>>()?;
                    let text =
                        String::from_utf16(&wide).map_err(|_| FormatError::InvalidString {
                            offset: at,
                            reason: "string is not valid UTF-16",
                        })?;
                    strings.insert(base + slot, text);
                }
            }
        }
        Ok(Self { strings })
    }

    /// The text for string id `id`, if the table has one.
    pub fn get(&self, id: u32) -> Option<&str> {
        self.strings.get(&id).map(String::as_str)
    }

    /// Number of non-empty strings.
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    /// Whether the table holds no strings.
    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }

    /// Every `(id, text)` pair in ascending id order.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &str)> {
        self.strings.iter().map(|(id, s)| (*id, s.as_str()))
    }
}

/// The entries of the resource directory at `root + rel` as
/// `(id, target)`; a subdirectory's target has its high bit cleared
/// and is relative to `root` like a leaf's. Named entries are skipped.
fn directory(data: &[u8], root: usize, rel: u32) -> Result<Vec<(u32, u32)>, FormatError> {
    let mut r = Reader::at(data, root + (rel & 0x7fff_ffff) as usize)?;
    r.skip(12)?;
    let named = u32::from(r.u16()?);
    let ids = u32::from(r.u16()?);
    if named + ids > MAX_ENTRIES {
        return Err(FormatError::InvalidValue {
            offset: r.pos(),
            field: "resource entry count",
            value: u64::from(named + ids),
            reason: "implausibly many entries",
        });
    }
    let mut out = Vec::new();
    for i in 0..named + ids {
        let id = r.u32()?;
        let target = r.u32()?;
        if i >= named {
            out.push((id, target));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-section PE32 whose `.rsrc` holds string block `block`
    /// (1-based) with `slots`.
    fn synthetic(block: u32, slots: &[(usize, &str)]) -> Vec<u8> {
        let mut payload = Vec::new();
        for slot in 0..16 {
            match slots.iter().find(|(s, _)| *s == slot) {
                Some((_, text)) => {
                    let wide: Vec<u16> = text.encode_utf16().collect();
                    payload.extend((wide.len() as u16).to_le_bytes());
                    for u in wide {
                        payload.extend(u.to_le_bytes());
                    }
                }
                None => payload.extend(0u16.to_le_bytes()),
            }
        }
        const SECTION_RVA: u32 = 0x1000;
        const SECTION_RAW: u32 = 0x200;
        let dir = |named: u16, ids: u16| {
            let mut d = vec![0u8; 12];
            d.extend(named.to_le_bytes());
            d.extend(ids.to_le_bytes());
            d
        };
        let entry = |id: u32, target: u32| {
            let mut e = id.to_le_bytes().to_vec();
            e.extend(target.to_le_bytes());
            e
        };
        let mut rsrc = Vec::new();
        rsrc.extend(dir(0, 1));
        rsrc.extend(entry(RT_STRING, 0x8000_0018));
        rsrc.extend(dir(0, 1)); // at 0x18
        rsrc.extend(entry(block, 0x8000_0030));
        rsrc.extend(dir(0, 1)); // at 0x30
        rsrc.extend(entry(0x409, 0x48));
        // data entry at 0x48
        rsrc.extend((SECTION_RVA + 0x58).to_le_bytes());
        rsrc.extend((payload.len() as u32).to_le_bytes());
        rsrc.extend([0u8; 8]);
        assert_eq!(rsrc.len(), 0x58);
        rsrc.extend(&payload);

        let mut image = vec![0u8; SECTION_RAW as usize];
        image[0..2].copy_from_slice(b"MZ");
        image[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        image[0x80..0x84].copy_from_slice(b"PE\0\0");
        image[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
        let optional_size = 224u16;
        image[0x94..0x96].copy_from_slice(&optional_size.to_le_bytes());
        let optional = 0x98;
        image[optional + 96 + 16..optional + 96 + 20].copy_from_slice(&SECTION_RVA.to_le_bytes());
        let section = optional + usize::from(optional_size);
        image[section..section + 5].copy_from_slice(b".rsrc");
        image[section + 8..section + 12].copy_from_slice(&(rsrc.len() as u32).to_le_bytes());
        image[section + 12..section + 16].copy_from_slice(&SECTION_RVA.to_le_bytes());
        image[section + 16..section + 20].copy_from_slice(&(rsrc.len() as u32).to_le_bytes());
        image[section + 20..section + 24].copy_from_slice(&SECTION_RAW.to_le_bytes());
        image.extend(rsrc);
        image
    }

    #[test]
    fn reads_a_block_and_numbers_the_ids() {
        let table = StringTable::parse(&synthetic(13, &[(0, "alpha"), (3, "Zeta!")])).unwrap();
        assert_eq!(table.get(192), Some("alpha"));
        assert_eq!(table.get(195), Some("Zeta!"));
        assert_eq!(table.get(193), None);
        assert_eq!(table.len(), 2);
        assert_eq!(
            table.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            [192, 195]
        );
    }

    #[test]
    fn rejects_a_non_pe_input() {
        assert!(matches!(
            StringTable::parse(b"not a pe image at all"),
            Err(FormatError::BadMagic { .. })
        ));
    }

    #[test]
    fn rejects_truncation_without_panicking() {
        let image = synthetic(1, &[(0, "x")]);
        for cut in 0..image.len() {
            let _ = StringTable::parse(&image[..cut]);
        }
    }
}
