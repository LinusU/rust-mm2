# PSDL city geometry (`PSD0`)

One file per city (`city/london.psdl`, `city/sf.psdl`). A PSDL is a shared
vertex/height pool plus per-room attribute streams — the rooms are the
city's spatial cells.

## Structure (verified against retail `london.psdl` — parses to EOF, zero
unparsed words; same for `sf.psdl`)

```text
'PSD0'
u32    target_size            (always 2 in observed files)
u32    nVertices,             nVertices × vec3        (city vertex pool)
u32    nHeights,              nHeights  × f32         (height pool, absolute Y)
u32    nTextures + 1,         nTextures × lp_string   (name table, no extension)
u32    nRooms, u32 junction_count
(nRooms - 1) × room record:                           (index 0 is reserved)
    u32 nPerimeter, u32 attribute_words
    nPerimeter × (u16 vertex, u16 neighbour_room+1)
    attribute_words × u16                             (attribute stream)
nRooms × u8 room_flags
nRooms × u8 prop_rule index
vec3 bounds_min, vec3 bounds_max, vec3 bounds_center, f32 bounds_radius
u32 nPaths, nPaths × path record                      (prop paths; mostly unknown)
```

## Attribute word encoding

Each attribute starts with a **`u16`** word (NOT u32 — an older version of
this document was wrong):

- bits 0–2: subtype
- bits 3–6: type
- bit 7:    last-attribute flag for the room
- bits 8–15: zero in all observed data

For several types `subtype == 0` selects the *counted* form: the first data
word is an element count and the refs follow. Nonzero subtype is the count
itself and there is no count word.

## Attribute layouts (verified against mm2kiwi Block Attributes and retail
data)

Data words after the attribute word; `v` = vertex index, `h` = height index.

| type  | name                 | subtype=0 payload                | subtype=n payload        |
|-------|----------------------|----------------------------------|--------------------------|
| 0x00  | road + sidewalks     | `count`, then count×4 `v`        | n×4 `v`                  |
| 0x01  | sidewalk strip       | `count`, then count×2 `v`        | n×2 `v`                  |
| 0x02  | road / walkway       | `count`, then count×2 `v`        | n×2 `v`                  |
| 0x03  | sliver               | —                                | `top_h, scale_h, v, v`   |
| 0x04  | crosswalk            | —                                | 4 `v`                    |
| 0x05  | road triangle fan    | `nTri`, then nTri+2 `v`          | n+2 `v`                  |
| 0x06  | generic fan          | `nTri`, then nTri+2 `v`          | n+2 `v`                  |
| 0x07  | facade bound         | —                                | `angle, top_h, v, v`     |
| 0x08  | divided road         | `count, packed, value`, +6n `v`  | `packed, value`, +6n `v` |
| 0x09  | tunnel / railing     | `nSize`, then nSize words        | n words                  |
| 0x0a  | texture reference    | —                                | 1 word (+subtype × 256)  |
| 0x0b  | facade               | —                                | `bot_h, top_h, u, v, vl, vr` |
| 0x0c  | roof fan             | `n`, then `h` + n+1 `v`          | `h` + n+1 `v`            |

Notes:

- **Road sections** (`0x00`) are cross-sections `[sw_left, road_left,
  road_right, sw_right]`; sidewalk tops sit 0.15 m above the road verts.
- **TextureRef** (`0x0a`): `index = data + 256×subtype − 1` into the texture
  table; `0` suppresses rendering *and* collision for following attributes.
  Relative texture slots: `n` = road surface, `n+1` = sidewalks.
- **Ground UVs** are not stored; they follow from how the textures are
  authored (verified by dumping retail TEX files). Road textures hold *half*
  a road: `u` runs along the road, `v = 0` is the centre line and `v = 1`
  the kerb (clamped in `v`), so roads and each carriageway of a divided road
  mirror the texture about their midline. Walkway (`0x02`) textures span the
  full width (e.g. `r_sub_l` rails). Sidewalk textures have the kerb stones
  at `v = 0`. Fans are planar-mapped.
- **Intersection texture slots**: `n` = road fans, `n+1` = sidewalk strips
  (`0x01`).
- **Facade** (`0x0b`): the two height indices are *indices into the height
  pool* (absolute Y), not raw metres. The wall is Y-aligned between the two
  base verts; `u`/`v` are texture repeat counts.
- **FacadeBound** (`0x07`): collision-only vertical quad; bottom edge follows
  the authored (possibly slanted) verts, top edge is horizontal at `top_h`.
- **Divided road** (`0x08`): `packed` low byte = `flags<<3 | divider_type`,
  high byte = divider texture index + 1; sections carry 6 verts
  `[sw_l, rl_out, rl_in, rr_in, rr_out, sw_r]`. Divider types: 0 invisible
  (collision bound), 1 flat, 2 elevated (`value` = height), 3 wedge.
- **Sliver** (`0x03`): slanted-bottom facade piece; `top_h` and `scale_h`
  index the height pool (scale values are small, e.g. 0.25).
- **Sidewalk strip** (`0x01`) end caps: a leading pair of equal refs
  (`0,0` or `1,1`) marks a triangular end piece instead of a strip.
- **Unknown types** terminate decoding of the room's stream; the remainder
  is preserved in `PsdlRoom::unparsed_attributes`.

## Empirical observations (retail `london.psdl`)

- ~1341 rooms, ~25k vertices, ~1669 heights, ~76k attributes.
- Perimeter polygons and authored fans are ~98% clockwise in authored
  `(x, z)`; the importer mirrors `z → -z` into Bevy space and flips emitted
  winding to compensate — upward-facing road normals result (~97%).
- Decoded attribute coverage: all bytes accounted for; the only deliberately
  unhandled family is tunnels/railings (`0x09`, ~127 attrs).

## Confidence

- File structure and attribute word layout: **verified** against retail
  London + SF (parse to EOF, zero unparsed words).
- Per-type payload layouts: **verified** via mm2kiwi docs and emission-side
  winding/normal checks on retail data.
- Path records (traffic lanes): **largely unknown** — preserved verbatim.

## Implementation

- `mm2_formats::psdl` — `Psdl::parse` → rooms → `RoomAttribute`s (raw `data`
  always retained; unparsed tails preserved).
- `mm2_app::city` — semantic decoding, per-(room, texture) `Mesh` groups,
  per-room trimesh colliders, facade-bound collision, INST/PKG props and a
  structured `CityReport` distinguishing emitted / suppressed /
  approximated / unsupported / rejected content.
