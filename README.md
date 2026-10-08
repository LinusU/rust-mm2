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
- Drawbridges: Tower Bridge, both Waterloo crossings and the Tower of
  London gate open and close on the retail cycle (per-event bridge
  files honoured); SF's Chinatown gate stands.
- Kerbside parked cars along the authored parked-car paths of both
  cities, knockable like other bangers.
- Moving scenery: tugs, water taxis, sailboards and ducks, the car
  ferries, and London's Underground trains shuttling through their
  tunnels.
- Their sounds: the drawbridge motor and bell, ferry engines and horns,
  and the Tube's rumble (heard only underground), with the original's
  range and distance falloff.
- San Francisco's four cable cars: one per tram-line terminus, each
  driving its line out and back at the original's recovered speed
  controller and stopping at red lights (silent; ambient cars and the
  player are not yet obstacles to it).
- City ambience: gulls and tug horns along the Thames and the Tube
  stations' announcements in London; gulls, buoy bells, sea lions,
  horns and the cable-car slot in San Francisco.

## Build & run

Requires a current stable Rust toolchain. Platform status, native
dependencies, release builds without private paths, asset discovery and
user-data locations are in [docs/building.md](docs/building.md).

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
| C            | cycle chase / cockpit / free camera |
| Backspace    | toggle the rear-view mirror strip |
| I            | toggle opponent indicators (in a race) |
| H            | toggle the driving HUD (map, indicators, dash, race cues) |
| X / Z        | cycle nav-arrow target (in a race) |
| F1           | toggle physics debug gizmos   |
| F4           | restart the session           |
| Esc          | quit (teardown → menu, or exit when launched direct) |
| Cmd/Ctrl+P   | save a screenshot to `screenshots/` |
| WASD+mouse   | fly (in free-camera mode)     |

The HUD ends with the active camera's pose as `cam x,y,z,yaw,pitch` — the
value `--cam` accepts — and screenshots carry it in their file name, so any
view can be reproduced exactly.

A connected gamepad works too: left stick steers, right trigger throttles,
left trigger brakes, South handbrakes. The pad also mirrors the in-session
keys (a designed map — the original's pad layout is unrecovered): right
stick click cycles the camera, West toggles the cockpit, East the mirror,
Y/North resets, left stick click honks, Select cycles the map view, the
dpad drives HUD up / indicators down / map zoom left / map rotate right,
the bumpers cycle the nav-arrow target, and the right stick glances in
the cockpit. The fullscreen pause map (`Q`), headlights (`L`), the
F-keys and the fly-camera controls stay keyboard-only.

### With an MM2 installation

```sh
cargo run -- --mm2-path "/path/to/Midtown Madness 2"
```

With no session-shaping flag the app opens the menu front-end
with original blue/amber city artwork, driver cards, framed buttons and a
loading splash. The garage renders the focused vehicle and paint in a rotating
3D showroom using the same imported meshes and textures as driving. Keyboard,
gamepad and mouse share navigation; Previous/Next buttons browse longer lists.
For reproducible screenshots, use `--menu --menu-screen garage --frames 90
--screenshot garage.png` (screens: `root`, `profiles`, `races`, `garage`, `options`).

The menu supports
(F17-A.1): Cruise and authored-event pickers over the real city/event
catalogs, the garage (reward-locked cars and paints are listed but
refuse with their reason), the driver-profile screen (select, create
through a typed-name entry screen, `X`/`Delete` deletes behind a
confirmation screen), the difficulty toggle, the graphics Options
screen, and a disabled-with-reason row for the not-yet-built
Multiplayer screen. Menu
controls: ↑/↓ or W/S move, ←/→ or A/D adjust, Enter/Space select,
Esc/Backspace back (quit at the root), X/Delete delete; on the name
field, type to edit, Backspace erases, Enter creates, Esc cancels; on
a gamepad the dpad/left stick navigates, South selects, East backs,
West deletes. Quitting a menu-launched session returns to the menu.

Any session-shaping flag skips the menu and boots straight in:

```sh
cargo run -- --mm2-path "/path/to/Midtown Madness 2" --city london
# optionally:
#   --city london|sf                 (menu pick when omitted)
#   --event <table>:<index>          e.g. blitz:0, checkpoint:3, circuit:1
#   --car <id|name> [--paint <n>]    stock/modded vehicle + paint index
#   --mods <mods dir>
#   --vehicle-config <toml>          (e.g. examples/vehicles/dev-car.toml)
```

`--event` loads an authored race from the city's event tables: the session
enters countdown on the event's start grid, checkpoint/finish markers are
placed from the authored waypoint rows, and crossing them advances shared
race progress. Crash Course events are parsed but not yet playable.

`--cnr ffa|cops|robbers` (with `--cnr-gold`, `--cnr-limit`) plays the
Cops & Robbers gold match over a city: hideout, bank and gold markers on
the authored site pool, a delivery scoring 100. Alone it is a one-seat
match; with `--host` the mode is advertised and closed to late joins.
A top-right readout shows the clock or limit, your side and points, and
where the gold is (the `H` key hides it with the rest of the HUD). The
main menu's `Cops & Robbers` row offers it too: pick a city (listed with
its reason when it cannot seed a round), cycle the game, gold weight and
limit, then `Start match`. When a one-seat match is decided (point or
time limit) a match-over screen shows the verdict and standings and
offers `Play again` (a fresh match) or `Continue to menu`; a hosted or
joined match shows the same screen to the host and every client but
offers only `Back to lobby` (the lobby's `Start` begins the next match).
There is no commentary yet.

Driver profiles live in the OS user-data directory (never the install):
`--profile <id|name>` binds one, `--new-profile <name>` creates and binds
one (`--sandbox` makes it a dev identity ineligible for progression,
`--pro` fixes its rank), `--profile-dir <dir>` relocates the store, and
`--no-profile` opts out. Without a flag, interactive runs bind whichever
profile was last selected. A bound profile restores your last
vehicle/paint and driver rank — `--car`, `--paint`, `--pro` still
override — and the session's selections are saved back when a session
starts. Smoke runs never touch profiles unless explicitly asked.

The app mounts every `.ar` archive found in the install directory plus loose
files, then loads `city/<name>.psdl` through the VFS. A `--city` the VFS
cannot provide is a hard failure — it never falls back to the dev world.

### Smoke tests (evidence commands)

```sh
# headless physics smoke — no window or GPU needed; settles, then drives:
cargo run -- --dev-world --headless [--frames N]            # default 600
cargo run -- --mm2-path <dir> --city sf --headless          # real city collision
cargo run -- --mm2-path <dir> --city london --event blitz:0 --headless  # authored race
cargo run -- --mm2-path <dir> --city london --event blitz:0 --headless --bot
#   --bot steers at the live race objective (nav target / next ordered
#   gate) instead of holding throttle straight — event completions go
#   through the real checkpoint/finish/result path. Works windowed too.
cargo run -- --mm2-path <dir> --city london --event blitz:0 --headless --parked
#   --parked holds the handbrake all session — the stationary control
#   leg: what the opponents do with no competing local driver.
cargo run -- --mm2-path <dir> --dev-world --headless --ram --frames 600
#   --ram holds throttle and steers at the nearest other vehicle (F25-C):
#   a driven car-to-car collision for multi-process runs against
#   --parked neighbours. An evidence driver; works windowed too.
cargo run -- --mm2-path <dir> --dev-world --car vpbug --headless --seq --frames 2200
#   --seq runs the staged idle → accelerate → coast → brake → reverse
#   audio evidence program (F07-AC02): the record's seq= field reports
#   the per-stage rpm/gear/speed, loudest engine-loop mix and clutch
#   one-shot counts. Works windowed too.
cargo run -- --mm2-path <dir> --city sf --car vpbug --headless --reset-at 1100
#   --reset-at <ticks> fires the `R` reset bundle (player + trailers)
#   once at that session tick — the scheduled reset leg for
#   frozen-input captures (record-ineligible).
#   --cam-cycle-at <ticks> walks the documented `C` view chain once at
#   its tick — the scheduled camera-transition leg (render-only).

# visual smoke — real render path, windowed; the screenshot is awaited
# (pass is reported only once the file actually lands):
cargo run -- --dev-world --frames 90 --screenshot out.png
```

Every smoke prints `mm2-smoke commit=<sha>` then one record:
`smoke=<headless-physics|visual> world=<world> status=<status> <metrics>`.

`status` distinguishes outcomes that must never be conflated:

| status        | meaning                                            | exit |
|---------------|----------------------------------------------------|------|
| `pass`        | ran, criteria held                                 | 0    |
| `fail`        | ran and failed (bad/missing resource, NaN, …)      | 3    |
| `unavailable` | capability absent (no MM2 data, no display/GPU)    | 4    |

Usage errors (bad flags, unloadable `--car`/`--vehicle-config`) exit 2.

### Frame-time profiling

A stutter report needs a number per frame, not an average. Play (or let
`--bot` drive) the release build with `--perf-log`:

```sh
cargo run --release -- --mm2-path <dir> --city london --event blitz:2 \
    --car vpauditt --perf-log frames.csv [--bot --frames 2400] [--no-vsync]
```

On exit it writes one CSV row per frame and prints a percentile summary.
Each frame's wall time is split into `fixed` (the 120 Hz `FixedMain`
loop: Avian and every fixed-step system), `update` (the rest of the main
schedule) and `render` (the render world, GPU and present wait — vsync
lives here), plus `fixed_steps` and Avian's summed `broad_ms`,
`narrow_ms`, `solver_ms` and `contacts`. Read it like this:

- `fixed_steps > 1` on most frames means physics is not keeping up and
  the fixed clock is catching up; the step count then swings with frame
  time, which is what stutter feels like.
- A slow `narrow_ms` with a high `contacts` count points at collision
  pairs worth removing (see `crates/mm2_app/src/layers.rs`).
- A large `render` with small `fixed`/`update` is GPU- or render-thread-
  bound; `--no-vsync` shows the cost without the display's refresh
  interval hiding it.

Shadows and anti-aliasing are the render costs a player can trade for
frame time: the root **Options** screen sets them (shadows Off/Low/High,
anti-aliasing Off/2x/4x), defaulting to High and 4x — the look the game
shipped with — and saves them to `settings.json` beside the driver
profiles. `--shadows <off|low|high>` and `--msaa <off|2|4>` override
them for a single run without saving, which is how to compare settings
with `--perf-log`. The same screen (and the pause overlay) offers a
**Text size** of 100/125/150%, which scales every menu and HUD element,
and a **Flashing** option (Normal/Reduced) that holds the cop light bar and
the low-time warning steady instead of alternating or pulsing.
**Display** (Windowed / Borderless fullscreen) and **VSync** (On/Off) rows
change the primary window live and persist; borderless never switches the
display's video mode, so there is no mode to get stuck in. `--no-vsync`
still forces vsync off for a run. **Field of view** (Authored / +10° / +20°)
widens the chase and cockpit cameras from each view's own authored
`CameraFOV`; the rear-view mirror keeps its authored view. **Flip recovery**
(Automatic / Manual) lets you turn off the modern self-righting assist for
your own car; the `R` reset and the authored stuck recovery stay.

A settings file that is partly wrong still loads: each invalid field keeps
its default and logs which one, and a file that is not valid JSON is moved
aside to `settings.json.bad` (likewise `controls.json.bad`) instead of
being overwritten by the next save.

Beside the CSV, `frames.csv.report.json` (schema `mm2-perf-report/1`)
makes the numbers reproducible rather than anecdotal: engine commit and
build profile (`debug` here means debug assertions are on — measure a
`--release` build), OS/CPU/logical cores, the GPU adapter and backend the
renderer reported, the content fingerprints (the structural catalog hash
and the gameplay-byte hash with its file/byte counts) and mounted mods,
the scene and settings the run named, and the post-warm-up percentile
timings, plus the live entity and audio-voice counts (sampled every 30
frames; a soak that leaks shows `last` climbing past `first`; the CSV
carries both as `entities` and `voices`). A field the process could not
observe is `null`.

Overload is reported rather than felt. Bevy caps the virtual clock at
`max_delta` (250 ms): a frame longer than that simulates only the cap and
silently discards the rest of its wall time. The CSV's `clamped_ms` column
is that discarded time per frame, and the summary and the report's
`timings.overload` give the frame count, total and worst discarded game
time, the cap, and the most fixed steps any one frame ran to catch up.
Zero `frames_clamped` means no gameplay time was dropped in the measured
window. It is a measurement of the engine's existing behaviour, not a
change to it.

The report is
local evidence — it embeds nothing from the install except hashes and
counts — and a run is only comparable with another that has the same
commit, build, hardware, fingerprints and settings.

For a CPU profile of where inside those stages the time goes, macOS's
`sample <pid> 20 1 -file out.txt` works on the stock release binary
(Instruments is not required).

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
the logical-asset pipeline, [docs/vehicle-handling.md](docs/vehicle-handling.md)
for how MM2 tuning becomes a drivable car,
[docs/modding.md](docs/modding.md) for mod authoring, and
[docs/research/](docs/research/) for format notes.

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

The app integration tests build two executables: `app` for headless app
behavior and `network` for networking and host/join process tests. Test
files are modules registered in `crates/mm2_app/tests/app.rs` or
`crates/mm2_app/tests/network.rs`; add new modules to the appropriate suite.
Run one suite with `cargo test -p mm2_app --test app`, or filter a module
with `cargo test -p mm2_app --test app audio::` (use `--test network net_host::`
for the host process tests).

## Legal

*Midtown Madness 2* is © Microsoft/Angel Studios. This project is an
unofficial, non-commercial reimplementation effort; it ships no copyrighted
material and requires users to supply their own game data.

Headless smoke reports also include `travel=<metres>`, `sim=<seconds>`,
`resets=<count>`, `controls=<throttle steps>t/<brake steps>b/<steer steps>s`
and `finite=<bool>`. Travel is planar solver displacement sampled at 120 Hz;
reset teleports and session-generation changes break the segment. `moved=`
remains displacement from spawn. These metrics describe evidence, not a
claim that the whole course was completed; read `cp=` and the race result.

`--bot --bot-speed 8` limits the evidence driver to approximately 8 m/s
through its ordinary throttle/brake inputs. The optional ceiling preserves
steering, reverse recoveries, vehicle tuning and the default driver behavior.
It works in windowed and headless sessions; useful for narrow custom streets.

`--bot --event <table:row> --bot-route /absolute/path/guide.opp` supplies
an explicit native `.opp` evidence guide. It overrides the roster-derived
player guide without spawning or changing opponents. The guide must contain at
least two points, no malformed rows, finite values in every column, and positive
finite XZ length for each consecutive edge. Invalid input fails before world
spawning. The route guides ordinary steering and throttle/brake inputs; gates,
laps, countdown and time limits still use the event definition. These runs are
record-ineligible, and existing bounded bot recovery behavior still applies.

When a guide is bound, the evidence driver reuses the opponent planner's
speed-scaled route projection, handling-derived steering and curvature braking.
The live player's next checkpoint still bounds the aim, so lookahead never
substitutes route traversal for crossing a gate. Unguided steering and the
existing bounded recovery policy remain unchanged.

Scripted escape and route reanchor traces disclose their tick, position, speed,
heading, route index and live gate. Escape traces also include the current aim;
reanchor traces include the landing point, helping locate source obstacles
without treating a recovered traversal as clean evidence.

An explicit `--bot` also opts into scripted motion during a windowed
`--frames N --screenshot out.png` capture, enabling played lap/finish HUD
images with the real opponent drivers active. Keyboard input remains frozen. Captures without `--bot` preserve their
ordinary static pose policy.

Dense evidence guides use spacing-bounded anchor reach and adjacent segment
projections to retain an upcoming corner in the speed plan until the car
actually turns. Native opponent anchor reach remains unchanged.
