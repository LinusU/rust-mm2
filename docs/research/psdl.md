# PSDL city geometry (`PSD0`)

One file per city (`city/london.psdl`, `city/sf.psdl`). A PSDL is a header
plus per-room attribute streams — the rooms are the city's spatial cells.

## Structure (verified against retail `london.psdl` — parses to EOF, zero
unparsed words)

```text
'PSD0' u32 room_count
[room 0] u32 offset, u32 attribute_count ... per-room directory
per room: attribute word stream
```

### Attribute word encoding (observed — differs from older docs)

Each attribute starts with a `u32` word:

- bits 0–2: subtype
- bits 3–6: type
- bit 7:    last-attribute flag for the room
- bits 8–15: zero in all observed data
- upper `u16`: preserved raw

The attribute's body layout depends on `(type, subtype)`. Size rules were
derived empirically by requiring the whole file to parse to EOF without
leftover words — the resulting rules decode london.psdl (1,341 rooms)
completely.

### Attribute types observed

Roads, sidewalks, fans, facades, tunnels, texture references, divided roads,
roofs, plus unknown types. Each attribute keeps its raw words in
`RoomAttribute::data` so nothing is lost for future RE work.

### Rendering subset currently decoded

- Triangle/fan vertex data and per-attribute texture name references — enough
  for `mm2_app::city` to emit Bevy meshes per room, grouped by texture.

## Confidence

- Header/room directory: **documented + observed**.
- Attribute word bit layout: **observed** (validated on the complete London
  dataset; older community docs describe a different packing that does not
  match retail).
- Per-(type,subtype) body sizes: **observed** for every combination present
  in retail London; unseen combinations may surface in other content and are
  handled conservatively (kept raw, skipped for rendering).
- Semantic meaning of several attribute fields (heights, flags inside
  bodies): **unknown** — preserved verbatim.

## Implementation

`mm2_formats::psdl` — `Psdl::parse` → rooms → `RoomAttribute`s with typed
bodies where known + raw payload always retained.
