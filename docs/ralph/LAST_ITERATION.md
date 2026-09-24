# Last iteration — F07-B.3: impact one-shot voices

Iteration 39 on `ralph/night` (baseline `eec3f62`, the F07-B.2
opponent-rig/spatial-listener leg — externally checked clean).
Selected the next F07-B remainder slice: consume the deduplicated
`ImpactEvent` stream through the authored `default_impacts.csv` table.
F07 is *not* complete — skids, ambient engines, clutch, sirens, scrape
semantics and audible capture remain open.

## What landed

- `mm2_game::banger::BangerDefinition.audio_id` — the authored
  `AudioId` column carried verbatim (it was parsed and dropped before).
  0 on every retail record, so retail props all read the id-0 `WALL`
  category — the binding is unverified (UNK-25), kept so impact audio
  and mods see the authored value.
- `mm2_game::audio` — `impact_category(table, audio_id)` resolves the
  authored `ID` category with an id-0 fallback (`None` when the table
  has no id-0 either — a data failure, not a guess), and
  `pick_impact(category, force, rng)` filters `min,max force` bands,
  weights covering samples by `frequency` (all-zero → uniform draw so
  authored silence is never manufactured) and draws the gain inside
  `min,max volume` (non-finite bounds read as their counterpart, both
  bad → 1.0, clamped ≥ 0). A force below every band returns `None` —
  authored silence, not an error.
- `mm2_app::audio::ImpactAudio` — session resource: parsed
  `ImpactTable` + generation-seeded `NavRng`. `load_session_world`
  resolves `aud/cardata/player/default_impacts.csv` through the VFS;
  absent/malformed warns once and inserts nothing (same absence policy
  as every authored record). Removed by `drive_session` teardown. The
  player-side file is the binding — the opponent file authors `WALL`
  bands two orders of magnitude smaller in-column, an authored
  inconsistency the runtime does not normalize.
- `mm2_app::audio::impact_voices` — runs after `session::drive_session`
  in both schedules (same despawn ordering as the rigs). Per event:
  stale generations skipped; non-`Playing` drains without emitting.
  Each *vehicle* participant earns one voice — remote participants
  skipped (their authority's client owns the sound, same skip every
  F05 consumer applies), non-vehicle participants earn none. Force =
  `severity × striker mass` — `ComputedMass` → `Mass` → config mass,
  each source validated before falling through (a not-yet-computed
  `Mass(0)` can't collapse the pick to a 1 kg touch — caught by the
  test suite, fixed this iteration). Voices are bounded
  `MAX_IMPACT_VOICES` 12 one-shots (`PlaybackMode::Despawn`,
  `SessionEntity`-swept), spatial emitters at `ImpactEvent.point`
  under `ENGINE_SPATIAL_SCALE` for everyone but the local player
  (non-spatial, DSN-37 anchor). `AudioReport.impacts` counts spawned
  voices; `aud=` gains `<impacts>i` only when nonzero — impact-free
  records stay bit-identical.

All three semantic picks (category binding, force quantity, per-side
emission) are designed readings classified under UNK-25 — the only
selectors the authored data itself names.

## Tests

`mm2_game` +4 — `AudioId`→category selection + id-0 fallback + no-id-0
`None`; force-band selection + sub-floor authored silence; frequency
weighting (zero-frequency row never wins while a weighted row stands;
all-zero weights draw uniformly); volume draw inside the authored
range + non-finite bound sanitization.

`mm2_app` (`tests/audio.rs`, +9 → 29 total) — wall impact picks the
authored band, `Despawn` mode, authored volume range, non-spatial
local voice at the impact point; 30 m/s picks the huge band's own
sample; sub-floor touch is silent; struck prop's `AudioId` 7 selects
its category (and only the car side voices); a two-car impact voices
each side at its own impulse (local non-spatial, AI spatial);
remote/non-vehicle/stale-generation events produce nothing;
`MAX_IMPACT_VOICES` 12 caps a 20-event pile-up with 8 counted drops;
absent table degrades to silence; teardown sweeps the voice.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--locked --workspace` green — 68 suites, 1067 tests, 0 failures.
`Cargo.lock` unchanged.

## Retail evidence

```
headless:  mm2 --mm2-path retail --event circuit:0 --city london
           --bot --headless --frames 3000
           → status=pass, impacts=212,
             aud=0h/30v/0s/18l/17a/8r/12i+125d
```

`fnv1a64:e91e6cd4b2ae30d9`. The bot race produced 212 deduplicated
impact events; the voice bound held 12 live one-shots (`12i`, part of
`30v` = 18 loops + 12 impacts) and counted 125 bound-drops (`+125d`) —
the bound and drop counter working under real load, not a cap never
reached. Headless `0s` is honest: no output device attaches, so
`PlaybackMode::Despawn` voices never self-clean and the bound simply
stays saturated — a windowed run is where clip-end despawn and sink
attach are exercised (F07-C scope).

## Not done / open

- F07-B continues: surface skid/rolling bands (two schemas disagree on
  the trigger column), ambient-traffic engines (`AmbientEngine`
  grammar), clutch sample trigger, siren programs, sustained-scrape
  semantics (the dedup cooldown bounds repeats but AC04's
  sustained-scrape leg is not separately evidenced), AC02's scripted
  drive sequence, AC05 audible capture.
- The category binding (`AudioId`→`ID`), force quantity
  (`severity × mass`) and per-side emission rule are designed readings
  — the original's are unrecovered (UNK-25, DSN-38 discloses each).
- No audible A/B against the original; headless `0s` reports no output
  device honestly (F07-C scope).
