# TEX textures

MM2's texture format. Note: there is **no magic string** — files start
directly with the header. `.tga` files also exist in the archives and are a
different format entirely (the scanner skips them).

## Layout (verified against retail data)

All integers little-endian:

| field      | type | notes                                        |
|------------|------|----------------------------------------------|
| width      | u16  |                                              |
| height     | u16  |                                              |
| pixel_type | u16  | see table below                              |
| mip_count  | u16  | total levels including base                  |
| unknown    | u16  | preserved verbatim                           |
| bits       | u32  | flag bits (`ClampU` = 0x01, `ClampV` = 0x10000, …) |
| palette    |      | BGRA entries — 256 for 8-bit types, 16 for 4-bit |
| pixels     |      | level 0, then each mip at half size          |

## Pixel types

| type | format                     |
|------|----------------------------|
| 1    | P8 — 256-colour palette, alpha ignored |
| 6    | A1R5G5B5                   |
| 8    | I8 greyscale               |
| 10   | A8I8 (Midnight Club 2)     |
| 14   | Pa8 — palette with alpha   |
| 15   | P4 — 16-colour palette     |
| 16   | Pa4 — 16-colour palette + alpha |
| 17   | RGB888                     |
| 18   | RGBA8888                   |
| other| preserved as `Unknown(n)`  |

## Confidence

- Header fields + type numbers: **documented** (angel-file-formats TEX spec)
  and **observed** — thousands of retail textures parse; a 3-level mip chain
  matches reported `mip_count`.
- 4-bit nibble order (high first): **inferred**.
- Exact semantics of every `bits` flag: partially documented; values are
  preserved for future use.

## Implementation

`mm2_formats::tex` — `TexFile::parse`, `levels`, `decode_rgba(level)` →
RGBA8. `mm2_app::city` converts to `bevy::Image`.
