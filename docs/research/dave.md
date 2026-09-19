# DAVE archives (`*.ar`)

Container format for all retail MM2 data (`mm2core.ar`, `mm2tex.ar`,
`mm2aud.ar`, `mm2audex.ar`).

## Layout (verified against retail data)

| offset | field                        |
|--------|------------------------------|
| 0x00   | magic `"DAVE"`               |
| 0x04   | `u32` entry count            |
| 0x08   | `u32` names-table offset     |
| 0x0c   | `u32` names-table size       |
| 0x800  | MFT: `count` × 16-byte records |

Each MFT record (16 bytes, little-endian):

| offset | field                                   |
|--------|-----------------------------------------|
| +0x00  | `u32` filename offset (into names tbl)  |
| +0x04  | `u32` data offset (absolute file offset)|
| +0x08  | `u32` uncompressed size                 |
| +0x0c  | `u32` compressed size                   |

- Names table: NUL-terminated strings at `names_offset`, total `names_size`.
- Filenames are stored with `\` separators and mixed case.
- When `compressed_size < uncompressed_size`, the stored data is **raw
  DEFLATE** (no zlib/gzip header). Otherwise data is stored verbatim.
- Data blobs appear alignment-padded in the archive.

## Confidence

- Header + record layout: **documented** (Open1560 `stream.cpp`-era
  knowledge, multiple community extractors) and **observed** byte-for-byte
  against the retail install — 13,338 entries across the four archives parse
  and decompress correctly.
- Raw-DEFLATE choice: **observed** (streams inflate only with a raw
  decoder).

## Implementation

`mm2_formats::dave` — `DaveArchive::parse` builds the entry table;
`read(entry)` decompresses on demand via `flate2` (Rust backend).
`mm2_assets` wraps it as `ArchiveSource`.
