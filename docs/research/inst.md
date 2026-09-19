# INST placements

Per-city object placement data (`city/london.inst`): a flat sequence of
records, each positioning a named PKG prop in the world.

## Record format (documented in angel-file-formats, observed on retail data)

Each record:

- `u16` room the object is attached to
- `u16` modifiers (paint-job selection in the low bits; `0x100` is set on
  most monuments)
- NUL-terminated PKG base name (no extension)
- placement, two kinds:
  - **coordinate** (type bit 7 clear): full basis — images of the unit axes
    plus origin, i.e. `city = origin + M * pkg`
  - **simple** (type bit 7 set): location + a horizontal heading vector
    whose magnitude is the scale. The vector is the image of the PKG's
    local X axis: `(1, 0)` means unrotated (verified: `wl_buckpalace_l`'s
    fence only matches its room perimeter with this reading)

## Confidence

- Structure above: **documented + observed**; `city/london.inst` decodes to
  1,997 placements that spawn at plausible positions.
- `modifiers` bit meanings beyond paint/0x100: **unknown**, preserved.

## Implementation

`mm2_formats::inst` — `InstComponent { room, modifiers, package_name,
placement }`. `mm2_app::city` resolves `package_name` through the VFS
(`geometry/<name>.pkg` or a mod override) and instances the decoded mesh.
