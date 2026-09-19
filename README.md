# rust-mm2

An open-source game engine/game in Rust inspired by *Midtown Madness 2* — an
"ultimate edition" style spiritual successor that loads content from a
**legally owned** MM2 installation rather than shipping any copyrighted data.

**This repository contains no MM2 assets.** To see original game content you
must point the engine at your own installation.

Long-term goals:

- Recreate London and San Francisco, vehicles, races, traffic, props,
  pedestrians, audio and UI from the user's own MM2 data.
- A modern, substantially improved arcade driving model (think
  *Midtown Madness* meets *Burnout Paradise* / *Forza Horizon* with assists).
- Mods as first-class citizens: any original asset can be overridden by a
  modern-format replacement (PNG/KTX2 for TEX, glTF for PKG, …) without
  touching the original files.
- Native Windows, macOS and Linux.
- Modern Bevy/wgpu rendering — deliberately *not* a D3D7 clone and *not* an
  executable-compatible reimplementation.

## Status

Vertical-slice stage. What works today:

- DAVE archive mounting + priority-aware VFS with mod overrides, provenance
  and pinned resolutions; one shared mount policy for the game and
  `mm2-inspect`.
- Parsers: DAVE, TEX (all mip levels), PKG2/PKG3, PSD0 (PSDL), INST.
- `mm2-inspect` CLI for scanning, lookup explanation and parse diagnostics
  (`scan`/`list`/`resolve`/`lookup`/`tex`/`pkg`/`psdl`, `--mods`, strict
  mode).
- Synthetic dev world with a fully simulated four-wheel Avian vehicle,
  chase/free cameras, reset, physics debug gizmos and a HUD
  (speed / gear / RPM / grounded wheels). Vehicle tuning loads from an
  optional TOML file (`examples/vehicles/dev-car.toml`).
- London: all 1341 rooms import with zero rejected/malformed attributes —
  per-room textured meshes, per-room static colliders, facade-bound walls,
  ~2000 INST/PKG props with collision, and a spawn on verified road
  geometry. San Francisco parses and emits identically.

## Build & run

Requires a current stable Rust toolchain.

```sh
cargo build
```

### Development world (no MM2 data needed)

```sh
cargo run -- --dev-world
```

Controls:

| Input        | Action                        |
|--------------|-------------------------------|
| W / ↑        | throttle                      |
| S / ↓        | brake / reverse               |
| A,D / ←,→    | steer                         |
| Space        | handbrake                     |
| R            | reset vehicle to spawn        |
| C            | toggle chase / free camera    |
| F1           | toggle physics debug gizmos   |
| Cmd/Ctrl+P   | save a screenshot to `screenshots/` |
| WASD+mouse   | fly (in free-camera mode)     |

The HUD ends with the active camera's pose as `cam x,y,z,yaw,pitch` — the
value `--cam` accepts — and screenshots carry it in their file name, so any
view can be reproduced exactly.

A connected gamepad works too: left stick steers, right trigger throttles,
left trigger brakes.

### With an MM2 installation

```sh
cargo run -- --mm2-path "/path/to/Midtown Madness 2"
# optionally:
#   --city london|sf                 (default: london)
#   --mods <mods dir>
#   --vehicle-config <toml>          (e.g. examples/vehicles/dev-car.toml)
#   --frames N --screenshot out.png  (smoke test: run N frames, capture, exit)
```

The app mounts every `.ar` archive found in the install directory plus loose
files, then loads `city/<name>.psdl` through the VFS.

### Texture override demo

```sh
cargo run -- --dev-world                        # base texture
cargo run -- --dev-world --mods examples/mods   # checker override visible
```

`examples/mods/checker-override` replaces the dev world's ground texture
through the same VFS path the city uses.

### Inspection tool

```sh
cargo run -p mm2_inspect -- scan    "/path/to/MM2" [--strict]
cargo run -p mm2_inspect -- list    "/path/to/MM2" [prefix]
cargo run -p mm2_inspect -- resolve "/path/to/MM2" texture/foo.tex
cargo run -p mm2_inspect -- lookup  "/path/to/MM2" texture/foo   # extension/source explanation
cargo run -p mm2_inspect -- tex     "/path/to/MM2" texture/foo.tex
cargo run -p mm2_inspect -- pkg     "/path/to/MM2" geometry/vp4x4.pkg
cargo run -p mm2_inspect -- psdl    "/path/to/MM2" city/london.psdl
# all commands accept --mods <dir>, mounted exactly as in the game
```

## Workspace layout

```
crates/
  mm2_formats/  pure-Rust parsers for MM2 binary formats (no Bevy/Avian)
  mm2_assets/   VFS: sources, priorities, mods, logical-path resolution
  mm2_vehicle/  arcade vehicle sim on Avian (config-driven, engine-agnostic math)
  mm2_game/     small game-domain state (world mode, markers)
  mm2_app/      Bevy executable: bootstrap, input, cameras, city import
tools/
  mm2_inspect/  CLI inspector — uses the same crates as the game
```

See [docs/architecture.md](docs/architecture.md) for the dependency rules and
the logical-asset pipeline, [docs/modding.md](docs/modding.md) for mod
authoring, and [docs/research/](docs/research/) for format notes.

## Mods (summary)

```
mods/example-hd-pack/
    mod.toml
    texture/foo.png      # overrides texture/foo.* from any lower source
    geometry/car.glb     # future formats resolve before original ones
```

A mod is a directory with a `mod.toml`; anything inside maps to logical paths
by directory structure. Higher-priority sources always beat lower ones;
within one source, modern extensions are preferred over original ones.

## Quality gates

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

## Legal

*Midtown Madness 2* is © Microsoft/Angel Studios. This project is an
unofficial, non-commercial reimplementation effort; it ships no copyrighted
material and requires users to supply their own game data.
