# Last iteration — F07-B.1: RPM-driven engine loop voices

New-task iteration on `ralph/night` (baseline `cd4dd48`, the reviewed
F07-A.2 horn leg — external gates + incremental review passed with no
blocking findings). Selected **F07-B.1** — the first consumer of F07-B's
task text ("wire authored engine/impact/surface/horn bindings to live
gameplay telemetry"). The engine loops are the highest-value leg: the
authored fade-window schema is fully understood and `VehicleState.rpm`
already models the drivetrain quantity the table is shaped around.
Impacts, surfaces, sirens, spatial/opponent audio stay open under
UNK-25 → later F07-B slices.

## What landed

- `mm2_game::audio` — `EngineLoopSpec` resolves an `Engine wave name`
  row's canonical fade-window columns (`fade in start/end RPM`,
  `fade out start/end RPM`, `Min/Max Volume`, `Min/Max Pitch`,
  `Pitch shift start/end RPM`). Rows in the divisor schemas (`Volume
  divisor`, `vol inverse RPM`, …) carry no RPM windows → `None`,
  never guessed. `EngineLoopSpec::mix(rpm)` → `EngineMix` — the
  designed reading (DSN-36, inferred — UNK-25): fade-in window ramps
  the envelope `0→1`, fade-out ramps `1→0`, volume interpolates
  `Min→Max` over the combined envelope (a loop outside its band is
  silent, not parked at its authored minimum), speed interpolates
  `Min→Max Pitch` across the pitch window. Degenerate windows are
  step functions (vpbug's `VWHIGH` `fade out 15000,15000` = hard cut);
  non-finite RPM → silent.
- `mm2_app::audio` — `EngineVoice`/`EngineRig` + two systems:
  - `engine_rigs` — one `PlaybackMode::Loop` voice per
    resolvable+decodable row, spawned as a `ChildOf` the player car
    (dies with it; sits at its transform for the future spatial mix),
    `SessionEntity`-stamped, bounded `MAX_ENGINE_VOICES` 8, excess rows
    dropped. Unresolvable rows and missing/failed waves increment
    `AudioReport.failed`; the `EngineRig` marker makes the build
    one-shot — a missing wave does not churn retry attempts.
  - `engine_drive` — recomputes `mix(rpm)` every frame and applies
    `set_volume`/`set_speed` to the sink when attached (headless: the
    voice exists with no sink — `aud=` reports honestly). Phase-ungated
    so countdown/results keep sounding; `Paused` sinks are held by the
    existing `sync_audio_pause`.
- `aud=<h>/<v>/<s>` extended to `aud=<h>/<v>/<s>/<loops>l/<audible>a` —
  engine loops spawned vs currently audible by computed mix.
- Production (`main.rs`) and headless (`smoke.rs`) schedules both run
  `(engine_rigs, engine_drive).chain()` after the horn systems.
- Docs: DSN-36 in `docs/original-rules.md`, UNK-25 narrowed (designed
  engine reading recorded; divisor schema, clutch trigger, and all
  other application semantics stay open), `docs/research/audio.md`
  runtime note + open-semantics entry.

Player-only and non-spatial: the local car sits at the listener.
Opponent/ambient `VehicleAudio` components already carry the data; their
rigs + `PlaybackSettings::spatial` + camera listener are future work.

## Retail evidence

```
headless parked:  mm2 --mm2-path retail --city sf --headless --parked
                  --frames 300
                  → aud=0h/4v/0s/4l/3a   (4 authored loops, 3 audible
                    at ~900 rpm idle — VWHIGH below its band; 0 sinks
                    honest, no device)
headless driven:  mm2 --mm2-path retail --city sf --headless --bot
                  --frames 1200
                  → aud=0h/4v/0s/4l/3a   (rig bounded through a full
                    scripted drive)
windowed:         mm2 --mm2-path retail --city sf --frames 120
                  → aud=0h/4v/4s/4l/3a   (all four loops attached real
                    mixer sinks on this machine)
```

vpbug's authored `VWIDLE`/`VWDRIVE`/`VWMID`/`VWHIGH` resolved through
the production VFS (`aud/aud22/engines/*.22k.wav`) and decoded via the
bounded parser — no fabricated bindings, no device required.

## Tests

- `mm2_game::audio` (+6) — canonical retail row resolves; idle band
  (900 rpm → VWIDLE alone audible); rising-RPM crossfade (VWIDLE fades,
  VWMID rises, pitch ramps); degenerate fade-out window is a hard cut;
  divisor-schema row → `None`; non-finite RPM/values → silent, never
  panic.
- `mm2_app` (`tests/audio.rs`, +6 → 15 total) — rig spawns one loop
  per row as children of the car; RPM re-mix changes volume/speed;
  missing wave + unresolvable row count in `AudioReport.failed` with
  the rig still built once; `MAX_ENGINE_VOICES` bound; car without
  authored audio builds nothing; session teardown cascades the voices.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--locked --workspace` green — 68 suites, 1049 tests, 0 failures.
`Cargo.lock` unchanged.

## Not done / open

- F07-B continues: opponent/ambient engine rigs + spatial mix +
  listener, impact/scrape voices off the deduplicated `ImpactEvent`
  stream (`default_impacts` force→sample binding unknown), surface
  skid/rolling bands (two schemas disagree on the trigger column),
  stationary-brake/airborne-wheel negatives, clutch sample trigger,
  siren programs.
- The engine mix formula is a designed reading (DSN-36) — the
  original's fade/pitch math and driving quantity are unverified under
  UNK-25; whether it consumes sim RPM, throttle, or a load-weighted
  blend is not recovered.
- No audible A/B against the original; sink-attach proves the
  decode→mixer path on this machine only (F07-C scope).
- `spchdata`/`creaturedata` + DirectMusic remain F08.
