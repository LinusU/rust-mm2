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
    whose magnitude stretches the model along that X axis **only**; the
    model's height and depth are unscaled (ledger WLD-34). The vector is
    the image of the PKG's local X axis: `(1, 0)` means unrotated
    (verified: `wl_buckpalace_l`'s fence only matches its room perimeter
    with this reading)

## Simple-placement scale (measured 2026-10-09)

The earlier port scaled all three axes by the heading length (the
angel-file-formats wording, "uniform scale"). Retail data says otherwise:
the stretched pieces are flat shop-front / house-base strips (e.g.
`cw_b5_store02_20_l`, 20 × 5 m, zero depth) that fit one wall segment's
length, and the Facade stacked on them starts at `y + model height`
exactly (store at (238.9, 4.8754) → facade bottom 9.8754; at
(246.4, 4.9429), scale 0.68 → 9.9429). Over every stretched simple
placement with a wall piece at its vertex: unscaled height matches 93 of
119 (London) and 23 of 40 (SF); scaled height matches 1 and 0. The
full-basis records of the same families have `|x|` varying and `|y|`,
`|z|` ≈ 1. Depth staying 1 is inferred from the latter.

## Confidence

- Structure above: **documented + observed**; `city/london.inst` decodes to
  1,997 placements that spawn at plausible positions.
- `modifiers` bit meanings beyond paint/0x100: **unknown**, preserved.

## Implementation

`mm2_formats::inst` — `InstComponent { room, modifiers, package_name,
placement }`. `mm2_app::city` resolves `package_name` through the VFS
(`geometry/<name>.pkg` or a mod override) and instances the decoded mesh.
