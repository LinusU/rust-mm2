# PKG geometry (`PKG2` / `PKG3`)

MM2's mesh format: vehicles, props, and building pieces. Files are a
magic-tagged chunk sequence.

## Chunk framing (verified against retail data)

```text
u32 magic ('PKG2' | 'PKG3')
repeat {
    u32  fourcc            # chunk type, e.g. 'FILE','GEOM','SHDR','XREF'
    u32  length            # PKG3: payload bytes ONLY (verified empirically —
                           # the doc's "includes header" note does not hold
                           # for retail PKG3 files)
    u8[] payload           # chunk data
}
```

Chunks seen in retail data: `FILE` (embedded filename), `GEOM` (geometry),
`SHDR`/`SHDS` (shader/material sets), `XREF` (external geometry references —
e.g. wheel models referenced by a car body).

## GEOM payload

- `u16` vertex count, `u16` section count.
- Vertex array — DirectX FVF-style fields; which fields exist is implied by
  the shader/section data: position always; optionally normals,
  diffuse/specular colour, 1–4 UV sets. `PkgVertex` exposes them as Options.
- Per-section: triangle strips (`PkgStrip`), material/shader indices,
  primitive flags. `PkgSection`/`PkgGeometry` retain per-strip data.

## SHDR payload

Shader sets with per-shader texture references (the texture *names* that the
VFS resolver later turns into `texture/<name>.tex` — or a mod override).

Vehicle damage-state texture pairing (recovered via mm2hook's
`fxTexelDamage::Init`, a `vehCarModel` member): a shader texture name ending
in `_dmg` (at the last `_`, case-insensitive) is the damaged variant of the
stem — the undamaged vehicle binds the clean stem when it resolves, and the
`_dmg` texture is stashed per shader slot for impact-time texel blits. A
failed clean lookup keeps the `_dmg` name. Damage-region body sections are
*authored* bound to `_dmg` shaders: `vpbug` BODY_H sections 4–6 are the
car's whole right half (`x>0`) bound to `vpbugyellow_{sd,ft,bk}_dmg`, and 22
of 27 base retail `vp*.pkg` (excluding `_dash`/`_trailer` variants) ship
`_dmg`-bound BODY sections (vpbus the largest at 12 sections/6 slots;
vpcop, vpdb731, vpmoonrover, vpmustang99, vpvw_dune none). The
pairing is symmetric — a clean `<stem>` shader pairs with `<stem>_dmg` as
its damage texture when that file exists — so one `_dmg` file serves both
halves of a symmetric body. `ApplyDamage`'s radius/triangle/barycentric
mechanics (radius = `vehCarDamage`'s `TextelDamageRadius`, DMG-5) are
recovered and implemented (`mm2_game::texel` + `mm2_app::texel_fx`,
F05-B.9); the splat shape itself (`ApplyBirdPoopDamage`, a binary call)
stays unrecovered — the implemented radial blit is designed (DSN-32).
The clean-state binding is `MaterialCache::shader_material` —
vehicle models only; prop PKGs bind names verbatim.

## Confidence

- Chunk framing, FVF vertex fields, strip layout: **documented**
  (angel-file-formats PKG.md + PKGImportExport behaviour) and **observed**
  on retail files.
- `length` = payload-only for PKG3: **observed** (the doc disagrees;
  verified by walking real files end-to-end).
- Some retail PKGs are **corrupt in the shipping data** (bogus chunk length
  mid-file) — confirmed by the external Blender importer's notes. The parser
  tolerates these by stopping at the first implausible chunk and keeping
  what was decoded (`Pkg::chunks` is complete for well-formed files).

## Implementation

`mm2_formats::pkg` — `Pkg::parse` → `PkgFile` (chunks: `PkgGeometry`,
`PkgShaders`, `PkgXref`, `PkgChunk` for unrecognized types, preserved raw).
`mm2_app::city` converts `GEOM` sections to Bevy meshes for INST props.
