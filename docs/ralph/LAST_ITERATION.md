# Last iteration — F07-B.4: surface skid/rolling loop voices

Iteration 40 on `ralph/night` (baseline `21cc9f8`, the F07-B.3
impact-voices leg — externally checked clean). Selected the next
F07-B remainder slice: connect the authored `default_surface*.csv`
tables to wheel-contact/slip telemetry with bounded skid and rolling
loop playback (spec req 3, F07-AC03 legs). F07 is *not* complete —
ambient engines, clutch, sirens, scrape semantics and audible capture
remain open.

## What landed

- `mm2_formats::cardata` — exported `is_sample_sentinel` (the same
  `NOSOUND`/`ENDOFDATA`/… test `wave_names` applies); the `SurfaceTable`
  doc comment corrected: the schema split is per **variant**, not per
  side — dry/wet files on both sides band `min slippage,max slippage`
  under a `max speed` window, ice files on both sides use the
  12-column divisor layout with `min speed,max speed` bands (verified
  by direct `mm2aud.ar` extraction; AUD-6 and the research doc
  corrected — an earlier reading had it per side).
- `mm2_game::audio` — `SkidUnit::{Slippage,Speed}` (the `skid wave`
  header's unit column decides the trigger), `RollingSpec::mix`
  (authored `min,max surface volume`/`pitch` interpolated over
  `0..max speed`, reverse reads `|speed|`, non-finite silences),
  `SkidSpec::pick` (first covering `min,max` band interpolates
  `min,max skid volume` inside it; below every band = authored
  silence), `SurfaceSpec::from_entry` (rolling resolves only on the
  canonical `max speed` schema — divisor rows return `None` rather
  than a guessed formula, same policy as `EngineLoopSpec`; each half
  independently `None` on sentinel/missing/unclassifiable fields),
  `tire_slippage` (designed trigger quantity — `max(|traction_demand|,
  |slip_angle|/peak_slip_angle)` clamped 0..1; the arcade tire model
  has no wheel-speed state, so this is the closest "past the tire's
  limit" measure the sim publishes — UNK-25).
- `mm2_content::SurfaceTables::sound_index` — `SurfaceMaterial` → the
  material's authored `sound` field → a positional table row
  (`Authored(i)` reads def *i*, `Unspecified` reads `_default`;
  missing/out-of-range/`None`-valued → `None`). Designed binding:
  `sound` is the only selector the authored data names (UNK-25).
- `mm2_app::audio` — session `SurfaceAudio` resource
  (`aud/cardata/player/default_surfacedry.csv`; absent/malformed warns
  and inserts nothing — the weather→variant binding is unverified so
  dry is the sole runtime table), `SurfaceRig`/`SurfaceVoice`/
  `SurfaceRole` components, `VoiceKind::{Skid,Rolling}`, and
  `surface_voices` after `session::drive_session` in both schedules:
  per grounded wheel the contact entity's `SurfaceMaterial` resolves a
  row; `slippage` rows read `tire_slippage`, `speed` rows read
  `|vel_long|`; the loudest covering skid band and loudest rolling
  loop (`|forward_speed|` ≥ `ROLL_MIN_SPEED` 0.5) win. Loop voices
  spawn lazily per committed entry and idle at volume 0 (the engine
  rig's no-churn policy); a different winning entry rebuilds only
  after holding `SURFACE_DWELL` 4 frames. Bounded `MAX_SKID_VOICES` 4
  bands per entry, `MAX_SURFACE_RIGS` 16 cars (refusals muted-marked,
  counted once). Children of the car + `SessionEntity`-swept; spatial
  for non-local cars, non-spatial for the player (DSN-37). Headless
  reports `aud=` `/<skids>k/<rolling>g` gauges when nonzero.
- `mm2_app::session` — `SurfaceAudio::load` on world build, removed at
  teardown; `mm2_app::main`/`smoke` schedule the system.

A held brake at rest and airborne wheels resolve nothing — AC03's
legs are silent by construction (no demand → no covering band; no
contact → no row), not by a special-case gate.

## Tests

`mm2_game` +7 — canonical schema resolves both halves; divisor schema
skips rolling but keeps speed bands; unclassifiable skid header → no
skid half; band selection + sub-floor authored silence; gain
interpolation inside the covering band; rolling volume/pitch ramp +
reverse-speed `|v|` + non-finite; `tire_slippage` utilization/saturate/
brake-at-rest/non-finite legs.

`mm2_app` (`tests/audio.rs`, +7 → 36 total) — sliding wheel voices the
covering band's own wave at the interpolated gain (`Loop`, child of
car, non-spatial local); AC03: grounded wheels at rest on grass and
airborne full-lock wheels spawn nothing and build no rig; moving car
rolls the authored loop (local non-spatial, AI spatial emitter, mix
ramped by speed); road→grass switch keeps the road band through the
dwell then rebuilds on the grass entry (rolling loop joins);
unresolvable material index and absent table stay silent with no
fabricated row; `MAX_SURFACE_RIGS` 16 caps 20 resolving cars (4 muted,
counted once); teardown sweeps the voices.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--locked --workspace` green — 68 suites, 1081 tests, 0 failures.
`Cargo.lock` unchanged.

## Retail evidence

```
headless:  mm2 --mm2-path retail --event circuit:0 --city london
           --bot --headless --frames 3000
           → status=pass, impacts=212,
             aud=0h/70v/0s/18l/17a/8r/12i/8k/1g+125d
```

`default_surfacedry.csv` resolved and parsed on the real install (no
warnings); the surface rigs produced 40 voices across the 8-car field
(70v total = 18 engine loops + 12 impacts + 40 surface) with `8k`
skid-band and `1g` rolling voices audible at the report tick — the
authored `sound`-class → row → wave chain live on retail data.
Headless `0s` is honest: no output device attaches; sink attach is
windowed/F07-C scope.

## Not done / open

- F07-B continues: ambient-traffic engines (`AmbientEngine` grammar),
  clutch sample trigger, siren programs, sustained-scrape semantics
  (AC04's second leg), AC02's scripted drive sequence, AC05 audible
  capture, weather→{dry,wet,ice} variant binding (dry bound
  unconditionally).
- Designed readings under UNK-25/DSN-39: the material `sound`→row
  binding, the `tire_slippage` skid quantity, `|vel_long|` for
  speed-banded rows, the rolling `|forward_speed|` mix, dry-as-default
  variant, `ROLL_MIN_SPEED`/`SURFACE_DWELL`/voice bounds.
- The ice divisor-schema rolling formula is unresolved — no
  `max speed` window exists to interpolate, so those rows roll silent
  rather than guessing (their `min speed` skid bands do resolve).
- No audible A/B against the original; headless `0s` reports no output
  device honestly (F07-C scope).
