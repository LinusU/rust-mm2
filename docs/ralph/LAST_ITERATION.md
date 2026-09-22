# Last iteration — F18-A.2 preset→Bevy lighting binding + session-legal selection

Iteration 44 on `ralph/night`, continuing from `f3af7cc` (the F18-A.1
evidence-repair candidate — external review verdict **pass**, with
non-blocking notes about `.lt` dispatch, saturating `ambient_packed`
and unsupported-extra handling). TASKS.json offered the F18-A
remainder; this slice binds the measured `.ltNN` presets to Bevy
lighting through the production session path, adds session-legal
weather/time-of-day selection, and keeps every unverified semantic
explicit. F18-A stays **active** — `.sky` dome, fog, `.cpvs` PVS,
precipitation/wetness/audio, menu pickers and condition replication
are untouched (UNK-24, F18-B/C scope).

## What changed

- `mm2_formats::lighting`: `LightSpec::to_light_dir()` /
  `travel_dir()` — the R4-recovered `setLightDirectionInv` convention
  (to-light = `(−cos h·cos p, −sin p, −sin h·cos p)`; Bevy's
  `DirectionalLight` forward gets the negated travel direction).
  Authored radians/colours are used verbatim — including
  `rainy-night`'s pitch +1.4 key, which correctly shines from below
  the horizon and contributes nothing upward.
- `mm2_game::race::effective_conditions(config, event)` — the single
  shared resolver: authored `EventParams::conditions` win while an
  event runs (RACE-2), `SessionConfig::conditions` is the cruise/dev
  fallback. Exported through `mm2_game::lib` for every future consumer
  (densities, precipitation).
- `mm2_app::environment` (new): `spawn_environment` runs inside
  `load_session_world` *after* event resolution, picks
  `city/<stem>.ltNN` on the measured `NN = tod*4 + weather` grid
  (WLD-21), spawns three `DirectionalLight`s (authored colours
  verbatim; key-only shadows — designed) and a `GlobalAmbientLight`
  from the packed BGRA ambient. A missing/unparseable preset spawns
  the pre-preset fixed rig and sets `EnvironmentReport.fallback` — an
  explicit F18-AC06 diagnostic, never a silent default. Report +
  entities are session-scoped (`SessionEntity`-stamped; report removed
  in `drive_session`'s teardown arm). `session.rs` lost the hardcoded
  city light block, which survives verbatim as the fallback rig.
- Designed scales, disclosed in `environment.md` + ledger DSN-28:
  uniform 15 000 lux per directional (authored colour carries relative
  weight, like the original's per-channel diffuse contribution) and
  `GlobalAmbientLight` brightness 2 000 anchored so a typical authored
  day ambient lands near the previous fixed ambient.
- CLI: `--weather`/`--time-of-day` accept the authored 0-3 selectors;
  out-of-range exits 2 (`invalid --weather: weather selector 4 is
  outside 0-3`). Both flags are excluded from `menu_mode` like every
  session-shaping flag, and `--event` + either flag warns they only
  feed the cruise fallback (authored conditions still win). Menu
  weather/time pickers remain unimplemented (F17-A remainder).
- Smoke: `env=ltNN(<name>|fallback)` on city-world records (absent on
  dev world so those stay bit-identical).

## Verification (this tree)

- `cargo fmt --all -- --check` — pass (after `cargo fmt --all`).
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — pass (two lints fixed during the iteration:
  `field_reassign_with_default` in the new `mm2_game` test,
  `clone_on_copy` in `tests/environment.rs`).
- `cargo test --locked --workspace` — pass, 64 suites / 0 failures.
- Tests +6: `lighting.rs::recovered_light_direction` (noon source
  overhead, travel = negation, zero-angle, unit length, positive pitch
  below horizon); `mm2_game/tests/race.rs::
  effective_conditions_prefers_the_authored_event`;
  `mm2_app/tests/environment.rs` ×4 over a synthetic one-room city —
  configured (2,0) binds `lt08` with authored colours/direction/
  ambient, missing preset reports `ltNN(fallback)` + pre-preset rig,
  authored event (1,2) binds `lt06` over configured (3,3),
  off-schema `SepiaTone` counts as an issue not a fallback.

## Retail evidence (fingerprinted install)

- Headless `smoke=` records through the real path: `sf --frames 60`
  → `env=lt00(clear-morning)`; `sf --time-of-day 3 --weather 3` →
  `env=lt15(rainy-night)`; `london --time-of-day 1 --weather 1` →
  `env=lt05(cloudy-noon)` (`status=pass`, `wheels=4/4`).
- Rendered captures on Metal/Apple M1 at frozen
  `--cam=-747.5,42.4,275.0,179,-15`: `sf.lt00`/`lt06`/`lt15` produce
  visibly different lighting (`status=pass`, PNGs inspected locally —
  not committed).
- Invalid selector: `--weather 4` → `invalid --weather` + exit 2.
- Honest caveat recorded in `environment.md`: `rainy-night` renders
  pastel-bright rather than dark — authored pastel fills +
  grey-80 ambient + the below-horizon key, with no fog yet (`.ltNN`
  authors no fog parameters; the fog/darkness semantics stay UNK-24).
  That is authored data plus a deferred leg, not a selection bug.

## Ledger / docs

- `docs/research/environment.md`: new "Runtime consumption (F18-A.2)"
  section (binding contract, designed scales, evidence, deferred
  scope).
- `docs/original-rules.md`: new DSN-28 (binding slice); WLD-21 gains a
  consumption cross-ref; UNK-24 narrowed — `.ltNN` binding exists,
  application-intensity semantics and everything else stay open.
- `docs/ralph/PLAN.md`: F18-A.2 row added; F18-A row + header updated.

## Not done / blockers

- `.sky` dome geometry/floats, any fog parameter, `.cpvs` variant
  selection + PVS culling, `.ldef`/`.pvshist`/`.lmap`/`.water`
  consumers, precipitation/wetness-traction/audio, authoritative
  condition replication (F18 req 5), menu weather/time controls.
- No original-executable comparison of the rendered presets (the
  retail binary is not runnable here); direction convention rests on
  R4's recovered formula + authored-value sanity, not side-by-side
  capture.
