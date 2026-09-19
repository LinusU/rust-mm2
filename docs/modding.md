# Modding

Mods are plain directories. Each mod is a self-contained tree whose folder
structure maps directly onto *logical* asset paths — the same paths the
original game data uses.

## Layout

```
mods/
  my-hd-pack/
    mod.toml                     # required manifest
    texture/
      london_road.png            # overrides logical "texture/london_road.*"
    geometry/
      vpbug.glb                  # overrides logical "geometry/vpbug.*"
```

`mod.toml`:

```toml
[mod]
id = "my-hd-pack"                # required, unique
name = "My HD Pack"
version = "0.1.0"
# optional, informational only for now:
author = "you"
description = "..."
```

`mod.toml` itself is never exposed as an asset.

## Loading

```sh
cargo run -- --mm2-path "/path/to/MM2" --mods ./mods
```

Every subdirectory of the mods dir that contains a `mod.toml` is mounted, in
sorted directory order. Mods listed later win over earlier ones on conflict.

## Resolution rules

Every lookup goes through the VFS with two independent orderings:

1. **Source priority** (always wins first):

   ```
   mods (300+)  >  override dirs (200)  >  loose install files (100)  >  .ar archives (0)
   ```

   A mod file beats the original archive file even if the extensions differ —
   `texture/foo.png` in a mod overrides `texture/foo.tex` in `mm2tex.ar`.

2. **Extension preference** (within equal priority): callers request a
   logical stem with a preference list, e.g.
   `resolve_preferred("texture/foo", ["png", "ktx2", "tex"])`. Inside one
   source, the first listed existing extension is chosen.

So the practical rule for mod authors: **name your file with the logical path
of the asset you want to replace, using any supported modern extension.**
You never need to know which archive or physical file shipped the original.

## Logical paths

- Use `/` separators; matching is ASCII case-insensitive (the original data
  is Windows-era).
- Paths are normalized; `..`, absolute paths and drive-relative escapes are
  rejected — a mod cannot read outside the virtual root.
- Top-level folders mirror the game's layout: `texture/`, `geometry/`,
  `city/`, `audio/`, etc. The inspector lists everything currently mounted:

  ```sh
  cargo run -p mm2_inspect -- list "/path/to/MM2" texture/london
  ```

## What can be replaced today

- **Textures**: TEX originals can be overridden by `png`/`ktx2` where the
  caller uses `resolve_preferred` (the city importer does). A TEX file can
  also be supplied directly if you want byte-identical replacement.
- **Geometry/props**: PKG overrides by same logical path; modern formats
  (`.glb`) are resolved by the preference list — glTF importing is a planned
  importer, not yet implemented.
- **City data**: `city/*.psdl`, `*.inst` can be overridden with the same
  format.

Anything without a dedicated importer yet still resolves correctly through
the VFS — the limitation is on the import side, not resolution.

## Debugging

- Mount logs print each source, its entry count and priority at startup.
- `mm2-inspect resolve` (via the VFS) reports which physical source wins a
  logical path, including archive offsets.
- Conflicts are silent by design (deterministic last-wins); use
  `mm2-inspect list` + resolve to verify which file is live.
