# Last iteration — F07-B.8: weather-bound surface-table variant

Iteration 45 on `ralph/night` (baseline `4b3b164`, F07-B.7 authored
siren programs — external verify + review green). One coherent slice
of the F07-B remainder: the session's effective weather now selects
the `default_surface{dry,wet}.csv` variant `SurfaceAudio` binds.

## Task selection

No failing gate or review finding to repair. F07-B's remaining
candidates were sustained-scrape semantics, the weather→surface-variant
binding, and the scripted/audible evidence legs. The variant binding
won on fresh evidence: `strings` on the retail exe carries
`%s_surfacedry`/`%s_surfacewet` plus `default_surface{dry,wet}` — a
per-name probe ahead of a shared default — and **no** `surfaceice`
string anywhere, so the shipped ice tables are dead authored data
under every recovered binding (AUD-11). Sustained scrape has no
authored scrape sample to bind (it would be an entirely designed
policy), so the weather binding was the evidence-backed slice.

## What landed

- `mm2_game::audio` — `SurfaceVariant::{Dry, Wet}` +
  `for_weather`: `rainy` (selector 3) → `Wet`, every other authored
  selector → `Dry`. Designed reading (DSN-43): the exe proves a
  runtime two-variant selection exists but not which state selects
  wet (UNK-25); `rainy` is the only authored weather that wets the
  road.
- `mm2_app::audio` — `SurfaceAudio::load(vfs, weather, vehicle)`:
  probes `aud/cardata/player/<vehicle-id>_surface<variant>.csv` ahead
  of `default_surface<variant>.csv` (the exe's `%s_` format; the
  catalog-id stem is inferred — stock ships no per-vehicle file, so
  an absent candidate probes silently rather than warning). A
  present-but-malformed or wrong-kind candidate warns and falls
  through to the `default_`; a `default_` failure warns and yields no
  resource. No cross-variant substitution — a rainy session with no
  wet table gets no surface audio rather than a mislabeled dry one.
  The resource now carries the bound `variant`/`path` for
  diagnostics.
- `mm2_app::session` — the effective conditions resolve once
  (`effective_conditions`: customization > authored event params >
  configured defaults — the same `SessionConditions` the `env=ltNN`
  lighting and fog bindings consume) and feed both the environment
  block and `SurfaceAudio::load`, alongside the catalog vehicle id.
- `mm2_app::smoke` — ` surf=wet` detail when a wet table is bound;
  dry and absent sessions emit nothing (bit-identical record).

## Docs

`docs/original-rules.md` — AUD-11 (exe surface-variant strings, the
`%s_`-probe shape, `surfaceice` dead-data finding), DSN-43 (the
designed rainy→wet binding, probe order, no-substitution policy,
`surf=wet` marker), DSN-39's tail updated (variant binding now
landed), UNK-25's surface clause rewritten (selection state still
unrecovered; designed binding disclosed). `docs/research/audio.md` —
F07-B.8 runtime paragraph; the surface-tables section records the
exe-string finding; the open-items line drops the variant binding
(sustained scrape remains).

## Evidence

Synthetic tests (device-independent): +1 `mm2_game` unit test (all
four authored selectors map — only 3 → `Wet`), +5 `mm2_app`
integration tests (rainy binds the wet table end-to-end — the wet
rolling/skid samples resolve, not the dry row's; selectors 0–2 bind
dry; a per-vehicle `<id>_surfacewet.csv` wins over the default;
a malformed per-vehicle file falls through to the default; a
dry-only tree under `rainy` yields no table — no cross-variant
substitute). Audio suite now 69 tests green.

Retail (`fnv1a64:e91e6cd4b2ae30d9`, `--city sf --headless --frames 60`):

- `--weather 0`: `env=lt00(clear-morning)`, no `surf=` marker —
  `default_surfacedry.csv` bound; no probe warn (the absent
  `vpbug_surfacedry.csv` candidate is silent).
- `--weather 3`: `env=lt03(rainy-morning)`, `surf=wet` present —
  `default_surfacewet.csv` bound off the same effective-conditions
  slot the lighting preset consumed.

`0s` honestly reports no output device headless; no audible A/B
against the original was performed — that remains F07-AC05 scope.

## Classification

A **designed/inferred runtime policy** over verified authored data:
the two exe variant strings, the `default_` fallback names and the
`surfaceice` absence are retail-verified (AUD-11); rainy→wet and the
vehicle-id probe stem are designed/inferred readings recorded under
DSN-43/UNK-25. The original's actual selection state (weather field
vs computed wetness) stays open.

## Remaining open items

- F07-B continues: sustained-scrape semantics (AC04 second leg),
  AC02 scripted drive-sequence evidence, AC05 audible capture.
- Surface unknowns now under UNK-25: the original's variant-selection
  state, the `%s` stem's exact content, `Tunnel sound index` (0 vs
  5), `for tunnels`, the divisor-schema rolling formula.
- Siren unknowns unchanged under UNK-25: trigger semantics, step
  pick, `Explosion sample` consumer, termination, `flags` bits 1/2/8.
- Headless `0s` reports no output device honestly; no windowed
  sink-attach leg was run this iteration.
