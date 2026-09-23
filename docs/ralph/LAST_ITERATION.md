# Last iteration — F18-A.4: `.sky` dome consumption

Iteration 53 on `ralph/night`. Selected the remaining `.sky` leg of
F18-A (environment/weather authored content): the city's sky dome was
parsed since F18-A.1 but never consumed — the renderer drew a solid
clear colour. `mm2_formats::sky` already parsed the record, the dome
pkg's 16 paint jobs measure as the same `tod*4 + weather` slot grid
the `.ltNN` preset and `_fog.csv` row bind (WLD-21/22/24), and the
existing PKG→Bevy path plus `SessionEntity` ownership covered the rest
— a clean small slice.

## What changed

- `mm2_app::city::pkg_to_parts` gained a `paint` parameter selecting
  the section shader as `paint * shaders_per_paint_job + offset` (the
  same convention `car_visual` uses for vehicle paints); prop callers
  stay on 0. New crate-visible `pkg_paint_parts` returns one paint
  job's render parts without the prop collider/`BREAK<NN>` fragments —
  the dome is render-only.
- `mm2_app::environment::spawn_sky_dome` runs in
  `load_session_world` beside `spawn_environment` at the resolved
  effective slot (authored-event and `SessionCustomization`
  precedence apply identically). `city/<stem>.sky` →
  `geometry/<model>.pkg` through the VFS → paint job `slot % jobs`
  → render parts. The dome spawns `SessionEntity`-stamped, unlit +
  `fog_enabled = false` + `cull_mode = None` + `NotShadowCaster` (the
  authored texture is the sky's final colour), scaled from its
  measured ~43 m extent to a designed 900 m radius.
- `drive_sky_dome` (Update, after the camera systems) re-centres the
  dome on the active camera in XZ at the authored `HatYOffset` and
  advances the authored `RotationRate` (read as radians/second).
- Diagnostics: `EnvironmentReport.sky: SkyReport` records path /
  model / paint / texture / issue count / `absent` reason
  (`missing`, `unparseable`, `degenerate`, `model unavailable`,
  `model unparseable`, `empty`) — never a fabricated dome
  (F18-AC06). A missing dome *texture* warns and draws the shared
  fallback material (the prop policy). `smoke_detail()` gains
  ` sky=<model>:<texture>` / ` sky=none`.

The `.sky` float readings are designed, not recovered (UNK-24):
`HatYOffset` as world-space dome height, `YMultiplier` as vertical
squash, `RotationRate` as radians/second, camera-centring as the
horizon policy, 900 m as the backdrop radius (inside the 1 000 m far
plane, past every authored fog end).

## Verification (this tree)

- `cargo fmt --all -- --check` — pass.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — pass.
- `cargo test --locked --workspace` — pass, all suites green; +8
  `tests/environment.rs` cases (16 total): authored bind at slot 7 →
  paint 1 → `dome_1` (scale, authored fields, session stamp,
  unlit/fog-exempt/double-sided material), camera-follow + rotation
  step, and every absent leg (missing/unparseable `.sky`,
  unresolvable model, <16-job wrap, missing texture → fallback,
  degenerate transform).
- Retail (`fnv1a64:e91e6cd4b2ae30d9`, `--headless --frames 120`):
  london `env=lt00(clear-morning) fog=220-320
  sky=sky_dome_l:skylondon_ca_l`; sf `env=lt00(clear-morning)
  fog=650-1000 sky=sky_dome:sky_ca_f` — each slot-0 paint job binds
  its authored texture through the real path; startup WARN count
  unchanged (1, the known `p_parkmeter_f.tex` mip warning).
- Rendered captures (Metal/Apple M1, frozen `--cam` pitched up):
  `/tmp/mm2-sky-sf-up.png` and `/tmp/mm2-sky-london.png` — the
  authored cloud/gradient dome textures draw visibly on both cities,
  unlit and unfogged; PNGs inspected.

## Not done / blockers

- The dome transform/rotation semantics are designed readings —
  `HatYOffset`/`YMultiplier`/`RotationRate` composition, the rotation
  units, dome scale and the camera-centring horizon policy are all
  unverified against the original (UNK-24 stays open).
- Whether the original lights or fogs its dome is unrecovered; bound
  unlit + fog-exempt by design.
- Not covered: per-slot visual distinction across all 16 presets
  (only slot 0 captured), dome rotation/scale vs retail screenshots,
  `.cpvs`/`.lmap`/`.ldef` consumption, precipitation, wetness,
  weather audio, condition replication — F18-A remainder +
  F18-B/C.
- Carried: UNK-22 (banger activation quantity), the named
  `sp_tree1_s` retail shatter not yet staged, `vpmoonrover` launch
  wander (open `drive` finding), vehicle handling remains
  operator-owned.
