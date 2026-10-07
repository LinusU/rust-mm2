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
# mods also work in the dev world without an installation:
cargo run -- --dev-world --mods examples/mods
```

Every subdirectory of the mods dir that contains a `mod.toml` is mounted, in
sorted directory order. Mods listed later win over earlier ones on conflict.
A mod `id` names exactly one mod: if two directories declare the same id (a
copied or renamed mod folder), mounting fails with an error naming both
directories rather than merging them — remove or re-id one.
Restart-based loading only — hot reload is not implemented.

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
- Symlinks inside a mounted directory (files, directories, or `mod.toml`
  itself) are never followed, so link cycles and links out of the mod are
  inert. A loose file over 256 MiB (the archive-member bound,
  `dave::MAX_ENTRY_SIZE`) is listed but refuses to read with a typed
  `TooLarge` error; a `mod.toml` over 1 MiB, or one that is not a regular
  file, fails the mount. Mods have no dependency declarations, so there is
  no mod-to-mod reference cycle to check; model/prop reference cycles stay
  with the format parsers.
- Top-level folders mirror the game's layout: `texture/`, `geometry/`,
  `city/`, `audio/`, etc. The inspector lists everything currently mounted:

  ```sh
  cargo run -p mm2_inspect -- list "/path/to/MM2" texture/london
  ```

## What can be replaced today

- **Textures**: TEX originals can be overridden by `png`, `tga` or `ktx2`
  where the caller uses `resolve_preferred` (the city importer and the dev
  world do). KTX2 support is limited to the compressed formats compiled
  into the build and supported by the GPU — an unrecognized encoding is a
  decode failure, not silent success. A TEX file can also be supplied
  directly for byte-identical replacement.
- **Geometry/props**: PKG overrides by same logical path; modern formats
  (`.glb`) are resolved by the preference list — glTF importing is a planned
  importer, not yet implemented.
- **City data**: `city/*.psdl`, `*.inst` can be overridden with the same
  format.

Anything without a dedicated importer yet still resolves correctly through
the VFS — the limitation is on the import side, not resolution.

## Debugging

- Mount logs print each source, its entry count and priority at startup.
- `mm2-inspect resolve` reports which physical source wins a logical path,
  including archive offsets and the mod label when applicable.
- `mm2-inspect lookup <stem>` explains a texture lookup: every extension
  tried, where each was found, and the winning source with its priority —
  the fastest way to see whether an override is live.
- Conflicts resolve deterministically (higher priority tier wins; at equal
  priority the later mount wins; mods are stacked in sorted directory
  order, so the alphabetically last mod wins). They are explained, not
  silent:
  - `mm2-inspect resolve <install> <path>` prints the winner, every
    shadowed source in rank order (mod id or archive/directory, priority,
    mount number) and why the winner won (`Vfs::explain`).
  - `mm2-inspect --mods <dir> conflicts <install>` lists every logical path
    more than one source provides plus a per-pair summary — `override`
    (a mod replacing original content) or `CONFLICT` (two mods); `--strict`
    exits non-zero on a mod-against-mod conflict, `--prefix` narrows the
    listing.
  - `mm2_assets::mount_mods` — used by the game, `mm2-host`, `mm2-join` and
    the inspector alike — logs the same summary at mount: `warn` for a mod
    conflict, `info` for a mod overriding original content.
  Use `mm2-inspect list` + `lookup` to verify which file is live.

## Portable cities with multiple PSDL parts

A custom city may provide a sibling `city/<name>.chunks` text file. Its first
non-comment line is `MM2_CHUNKS 1`; subsequent lines are logical VFS paths to
additional PSDL files, for example `city/mycity.parts/east.psdl`. All parts
use the same metre coordinates and materials resolved through the VFS. The
primary file retains its default spawn. Each part owns an independent vertex
pool, allowing maps larger than PSDL's 16-bit vertex indices.

The loader validates every listed file before spawning geometry and reports
missing or malformed parts as fatal. Paths must be relative, unique, traversal
free, and must not list the primary file. At most 128 additional parts are
accepted. Nested manifests and CPVS visibility files are rejected: cross-part
visibility is not yet supported. Room IDs are offset into one unique city
namespace; per-part `.water` files retain their own water bounds and levels.
Collision surfaces and city diagnostic counts include every part.

This optional companion format is a rust-mm2 extension, not a claim of retail
MM2 support. A single PSDL without this companion keeps the existing behavior.
