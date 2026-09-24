# Last iteration — F07-B.6: ambient-traffic engine voices

Iteration 43 on `ralph/night` (baseline `2cc6e8d`, the F07-B.5 doc
repair — external verify + review green). One coherent slice of the
F07-B remainder: the authored `aud/cardata/ambient/*_engine.csv`
tables now drive bounded looping engine voices on ambient traffic.

## Task selection

No failing gate or review finding to repair. TASKS.json's F07-B row
listed ambient-traffic engines as a ready sub-slice with all
dependencies in place — the `AmbientEngine`/`SpeedBand` parsers
already existed (F07-A.1), ambient cars already spawn kinematic
bodies with `LinearVelocity` (F10-B), and the voice/bank/report
machinery was built by F07-A.2/B.1–B.5. Siren programs, sustained
scrape and the weather→surface-variant binding remain.

## What landed

- `mm2_game::audio` — `AmbientEngineSpec::from_table` resolves one
  table's `Engine sample`/`engine volume`/speed bands (sentinel →
  `None` authored silence, non-finite band cells dropped) and
  `mix(speed)` computes the loop's `EngineMix`: **first covering band
  in authored order wins** (retail authors tight piecewise bands
  ahead of a `0–500` catch-all carrying up to ~24.6 max pitch, so
  authored order is the only reading under which the tight bands are
  reachable), an uncovered speed evaluates the nearest band at its
  edge (ties prefer the earlier row), `|velocity|` input, non-finite
  speed silences. `engine volume` is constant — the table authors no
  speed→volume term. `AmbientAudio` is the stamped component.
- `mm2_content::ambient_engine_audio` — resolves
  `aud/cardata/ambient/<id>_engine.csv` then `default_engine.csv`
  (designed binding, UNK-25 — the filenames are the only per-class
  link the data names). `Ok(None)` = authored silence; `Err` =
  malformed resolved file, never a silent substitution.
- `mm2_app::traffic` — `AmbientClass.audio` cached per class
  (parse diagnostics + `validate()` issues warn once per class);
  `spawn_ambient_car` stamps `AmbientAudio` when the class has a
  spec — noteless classes stamp nothing.
- `mm2_app::audio` — `VoiceKind::AmbientEngine`, `AmbientRig` marker
  (one warn per car; dies with the body so a recycled car
  re-attempts), `AmbientEngineVoice` component, and the
  `ambient_engine_rigs`/`ambient_engine_drive` chain scheduled
  `.after(session::drive_session)` in both `main` and headless-smoke
  schedules. One `PlaybackMode::Loop` spatial child voice per car
  (`ENGINE_SPATIAL_SCALE` — ambient cars are never the local anchor),
  `MAX_AMBIENT_VOICES` 32 bound, `SessionEntity`-swept. The drive
  re-mixes off the parent's `LinearVelocity` — lane followers and
  knocked wrecks publish the same component.
- `AudioReport` gains `ambient` (spawned) + `ambient_live` (audible
  gauge); the smoke record's `aud=` gains `/<n>e/<n>n` when nonzero.
- Authored-data finding recorded: only 4 of 7 per-class engine files
  match a stock roster id exactly (`va_bus_f`, `va_ddbus_l`,
  `va_diesels_s`, `va_smallsuv_s`) — `va_sedan_s_engine.csv` has no
  `va_sedan_s` roster id (the roster's is `va_sedans_s`) and
  `va_garbagetruck`/`va_pickup_f` are unrostered in both stock cities,
  so under exact-match resolution those three tables are dead data
  (AUD-9).

## Docs

`docs/original-rules.md` — AUD-9 (ambient table inventory + roster
coverage quirk), DSN-41 (the designed resolution/binding/band
semantics), UNK-25 updated (B.6 joins the implemented-slice
enumeration; the ambient clause records the designed binding while
keeping original class bindings + band quantity unverified).
`docs/research/audio.md` — F07-B.6 runtime paragraph + the ambient
tables section records the naming quirk. `PLAN.md` — F07-B row's
Remaining trimmed (siren/scrape/weather-variant/AC02+AC05 left),
F07-B.6 row added.

## Evidence

Synthetic tests (device-independent): +4 `mm2_game` unit tests
(resolve, authored-order-beats-catch-all + nearest-edge + negative/
non-finite legs, sentinel + bad-cell degradation), +8 `mm2_app`
integration tests (per-class→default→absent resolution, malformed
errors instead of falling back, sentinel silence, spatial-loop spawn
+ report fields, speed-band mix legs, unresolvable-once-per-car,
32-voice fleet bound, session-teardown sweep). Audio suite now 52
tests green.

Retail (`fnv1a64:e91e6cd4b2ae30d9`, `--headless --frames 1500`):

- london cruise: `traf=16/16 sp=20` → `aud=0h/52v/0s/4l/4a/1r/12i/8c/1k/0g/20e/16n+17d`
- sf cruise: `traf=16/16 sp=34 rec=18` → `aud=0h/61v/0s/4l/4a/1r/7i/8c/34e/16n+15d`

`e` tracks `sp` — every spawned ambient car resolved and voiced a
loop (zero `+x` failures); `n` = the live 16-car fleet mixing at
record time; `+d` is the shared bound counter (impact/clutch bound
drops, unchanged semantics); `0s` honestly reports no output device
headless. No audible A/B against the original was performed — that
remains F07-C scope.

## Classification

The slice is a **designed/inferred runtime policy**, not a claim
about the original executable: per-class-file-then-default binding,
`LinearVelocity` m/s input, first-covering-band pitch reading and the
nearest-edge fallback are all recorded as designed under UNK-25/DSN-41.
Whether the original binds classes this way, what quantity it feeds
the speed bands, and whether it pitches the ambient loop at all stay
unverified.

## Remaining open items

- F07-B continues: siren programs, sustained-scrape semantics
  (AC04 second leg), weather→{dry,wet,ice} surface-variant binding,
  AC02 scripted drive-sequence evidence, AC05 audible capture.
- UNK-25 designed readings added here: class→table binding, speed
  quantity/unit, band-order semantics, `MAX_AMBIENT_VOICES` 32.
- Headless `0s` reports no output device honestly; no windowed
  sink-attach leg was run this iteration.
