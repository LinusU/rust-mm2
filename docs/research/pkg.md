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
