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
# optional claim: what the mod changes ("cosmetic" or "gameplay"); see below
effect = "cosmetic"
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
directories rather than merging them — remove or re-id one. Any failure in
the scan (duplicate id, bad or oversize manifest) mounts none of the mods:
the game starts with the install alone and the menu, lobby and handshake
report no mods, matching what is actually mounted (`mm2-host`, `mm2-join` and
`mm2-inspect` exit non-zero instead).
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
  no mod-to-mod reference cycle to check. A PKG's `xref` chunk names other
  models, but the importer only keeps those names (`VehicleModel::xrefs`)
  and never opens them, so a PKG that references itself or a pair that
  reference each other loads once and cannot recurse
  (`mm2_app/tests/reference_cycles.rs`); an importer that starts following
  xrefs must bring its own cycle check. Nested text formats (`.skel`, tune
  blocks) have fixed depth bounds, and a `.chunks` manifest part may not be
  a manifest.
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

## Gameplay versus cosmetic mods

What a mod changes decides what it costs (F29 req 5). `mm2_content::
fingerprint::is_gameplay_path` is the one classifier: `tune/`, `bound/`,
`geometry/`, `race/`, `anim/`, `players/` and every `city/` file except
skies, lighting and visibility sets feed the simulation or its rules;
`texture/`, `aud/`, `jpg/`, `city/*.sky|ldef|lmap|ltNN|cpvs|pvs|pvshist` and
unprefixed loose files are cosmetic. A mod is judged by the files it
**wins** — a gameplay file a later mod replaces is credited to that later
mod, and a mod that wins nothing changes nothing.

- A **gameplay** mod moves the multiplayer gameplay fingerprint (peers must
  run the same mods to join) and makes the run **record-ineligible**: no
  records, no unlocks (`Ineligible::ModContent`).
- A **cosmetic-only** mod (a repaint, a sound swap, menu art) moves neither;
  its runs record and unlock exactly like stock.
- This is a path classification, not a content diff: a gameplay-family file
  rewritten with identical bytes still counts as gameplay (conservative).
  The split is an implementation choice naming the families *this engine's*
  simulation reads, not an original rule; a new consumer with gameplay
  effect (e.g. an audio cue with a gameplay trigger) must move its family
  into the gameplay set.
- A session whose mods nobody classified fails safe: `SessionConfig::
  mods_cosmetic_only` defaults to `false`, so `mods_active` alone keeps the
  run record-ineligible.

`mm2-inspect --mods <dir> mods <install>` prints each mod's verdict, counts
and first gameplay path; `--expect-cosmetic` exits non-zero if any mod
changes gameplay (for a pack advertised as cosmetic). The game logs the same
verdict per mod at startup.

### Declaring the effect

A manifest may state `effect = "cosmetic"` or `effect = "gameplay"`. It is a
claim to check, never an input: the verdict above always comes from the files
the mod wins, so a mod cannot launder a tuning edit by calling it cosmetic.
A claim the files contradict — `cosmetic` on a mod that wins a gameplay path,
or `gameplay` on one that wins none (empty, or shadowed by a later mod) — is a
`warn` in the game's log and a `MISMATCH` line in `mm2-inspect mods`, which
then exits 2. Any other spelling is a manifest error (a typo must not read as
"no claim"); omitting the field is fine. Every mod under `examples/mods`
declares one, and `mm2_inspect`'s `the_shipped_example_mods_declare_an_effect_their_files_confirm`
fails if an example's claim stops matching its files.

## Replacement is a remount (cache identity)

Replacing content is restart-based: the game mounts the install and the
mods once, wraps the `Vfs` in the immutable `Mm2Vfs` resource and never
remounts it, so every cache (`MaterialCache`, `PropCache`, `BangerDefs`)
borrows that one VFS and keys by normalized logical path — Rust's borrow
rules already forbid a mount while they exist. Hot reload is not supported.

The one VFS-derived structure that does *not* borrow is the audio
`WaveBank` (a Bevy resource holding a stem index and decoded handles).
`Vfs::revision()` is its identity check: a counter bumped by every mount and
every rollback (a failed `mount_mods_dir` that undoes its mods still counts
as a change) and never reused. The bank stamps the revision it was indexed
from and `load`/`load_siren` refuse any other revision with an error naming
both, rather than answering from a layout that no longer exists. Any future
resource that owns an index derived from the VFS must do the same. The
revision identifies the *layout* of sources only; `fingerprint::gameplay` is
the byte-level identity of gameplay content.

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
- `mm2-inspect [--mods <dir>] deps <install> <car> [--paint N]
  [--expect-mod <id>]` loads a car through the production loader with the
  VFS tracing every read (`Vfs::trace_reads`), then lists the files the load
  pulled in grouped by the source that served them — `mod <id>` or the
  original install — plus any path read that no source provides. A load
  that fails still prints what it read first. `--expect-mod` exits non-zero
  unless the load read at least one file from that mod, which is how a pack
  proves it is live for a car. The trace covers reads of paths that exist or
  were asked for by name; an optional file the loader only `resolve`s and
  finds absent leaves no entry. Tracing is per call and nests; nothing is
  recorded outside one.
  `mm2-inspect deps <install> --city <stem> --event <table>:<row>` traces an
  authored event instead (the same `<table>:<row>` vocabulary as `event`),
  through the inspector's event audit: the production catalog scan, the row's
  records, and the race-definition and roster builds. The scan reads the whole
  city's event tables and records, so a mod that replaces any of that city's
  race files is credited, not just the named row's; another city's mod is not.
  `mm2 --mm2-path <install> [--mods <dir>] --city <stem> --trace-deps
  [--expect-mod <id>]` is the city-geometry counterpart. It lives in the game
  binary because the city loader builds Bevy meshes and materials: it runs the
  production `load_city` into a throwaway world (no window, no GPU) and prints
  the same per-source report — the PSDL, `.inst`, `.cpvs`/`.water`, prop PKGs,
  textures, the surface tables, pathsets and prop rules the city pulled in. A
  failing load prints what it read first and exits 2, as does an unmet
  `--expect-mod`. It traces the city load only: audio cues are fetched lazily
  per voice, so audio has no `deps` target yet.

## Portable cities with multiple PSDL parts

A custom city may provide a sibling `city/<name>.chunks` text file. Its first
non-comment line is `MM2_CHUNKS 1`; subsequent lines are logical VFS paths to
additional PSDL files, for example `city/mycity.parts/east.psdl`. All parts
use the same metre coordinates and materials resolved through the VFS. The
primary file retains its default spawn. Each part owns an independent vertex
pool, allowing maps larger than PSDL's 16-bit vertex indices.

The loader validates every listed file before spawning geometry and reports
missing or malformed parts as fatal. Paths must be relative, unique, traversal
free, and must not list the primary file; uniqueness is the VFS's, so `City/A.psdl`
repeats `city/a.psdl` and a capitalised spelling of the primary file lists it. At most 128 additional parts are
accepted. Nested manifests and CPVS visibility files are rejected: cross-part
visibility is not yet supported. Room IDs are offset into one unique city
namespace; per-part `.water` files retain their own water bounds and levels.
Every file a city names beside its PSDL (`.chunks`, `.cpvs`, `.inst`, `.water`,
`<name>/props.pathset`, and the environment files `.ltNN`, `_fog.csv` and
`.sky`) is found by the same case-insensitive rule, so a
city spelled `city/MyCity.PSDL` finds `city/MyCity.chunks`; a part may likewise
be listed as `city/parts/East.PSDL`.
Collision surfaces and city diagnostic counts include every part.

This optional companion format is a rust-mm2 extension, not a claim of retail
MM2 support. A single PSDL without this companion keeps the existing behavior.
