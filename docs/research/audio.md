# Audio formats (`aud/**`)

Measured on the retail install (VFS fingerprint `fnv1a64:e91e6cd4b2ae30d9`,
2026-09-24) with `mm2-inspect audio <install>` — **3 256 files** under
`aud/`, all of them now classified and the two content-bearing families
fully decoded:

```
2503 waves, 103 tables, 552 deferred text csv, 0 binary csv,
3 extras (.bat), 0 failures, 0 issues, 35 quirks, 1 finding, 1 dead ref
```

F07-A.2 adds the first runtime consumer: the local vehicle's authored
horn (`Horn wave name` row) resolves through `aud22`-preferred stem
lookup, decodes through the bounded PCM parser, and plays through Bevy's
mixer on ENTER (documented default, CTL-1). Voices are session-owned,
bounded, despawned on completion and paused with the session. Whether
the original holds the horn for the keypress duration or retriggers it
is still unverified — the press fires one clip, a designed choice.

F07-B.1 adds the engine rig: every `Engine wave name` row resolving the
canonical fade-window schema spawns a looping voice parented to the
player car, re-mixed each frame off `VehicleState.rpm` — the sim's
drivetrain-derived engine RPM (gear- and direction-aware). The designed
formula reads the fade-in window as a `0→1` envelope ramp, fade-out as
`1→0`, volume interpolating `Min Volume→Max Volume` over the combined
envelope (a loop outside its band is silent, not parked at its authored
minimum) and speed interpolating `Min Pitch→Max Pitch` across the pitch
window — all inferred, not recovered (UNK-25/DSN-36). Divisor-schema
rows carry no RPM windows and are skipped with a counted warning.

F07-B.2 extends the rig to every `VehicleAudio` car: opponents now
spawn with the component (the opponent-side cardata record
`load_opponent` already resolved, same absence policy as the player)
and each builds its `PlaybackMode::Loop` voices, bounded by
`MAX_ENGINE_RIGS` 16 per session. Non-player loops are
`PlaybackSettings::spatial` emitters heard through a single
`SpatialListener` that `audio_listener` keeps on the active `Camera3d`
(chase↔free moves the ear the same frame); the player's own rig stays
non-spatial — the local car anchors the mix. Rodio's spatial panner is
inverse-square attenuation plus left/right ear pan; the designed
`ENGINE_SPATIAL_SCALE` 0.25 puts a 4 m opponent at ~full authored
volume, a 5–15 m pack clearly audible and a 50 m straggler near
silence — the original's attenuation model and listener placement are
unrecovered (UNK-25). Retail `london circuit:0 --bot --headless
--frames 3000`: `aud=0h/18v/0s/18l/17a/8r` — 8 rigs, 18 loops, 17
audible mid-drive, honest 0 sinks headless.

F07-B.3 adds the impact consumer: the session loads the player-side
`default_impacts.csv` into an `ImpactAudio` resource (absent/malformed
→ no resource, warned once, never fabricated) and `impact_voices`
consumes the deduplicated `ImpactEvent` stream. Each *vehicle*
participant of an event earns one bounded one-shot
(`MAX_IMPACT_VOICES` 12, `PlaybackMode::Despawn`, `SessionEntity`
lifecycle) — a remote participant's voice belongs to its own client, a
non-vehicle participant earns none. The struck side's
`dgBangerData.AudioId` selects the category `ID` (anything unmatched —
which is every retail record, all `AudioId` 0 — reads the id-0 `WALL`
catch-all; the binding is designed, UNK-25), `severity × striker mass`
picks the `min force,max force` band (the same impulse estimate the
knock pipeline weighs — whether the original weighs this quantity is
unverified), `frequency` weights the covering samples and the authored
`min,max volume` draws the gain. Below every band is authored silence,
not an error. Non-local voices are spatial emitters at the impact
point under `ENGINE_SPATIAL_SCALE`; the local player's hits stay
non-spatial (DSN-37). `aud=` gains `<impacts>i` when nonzero.

F07-B.6 adds the ambient-traffic consumer: `mm2_app::traffic` resolves
each `va_*` class's `aud/cardata/ambient/<id>_engine.csv` — falling
back to `default_engine.csv`, a designed binding (UNK-25; only
`va_bus_f`/`va_ddbus_l`/`va_diesels_s`/`va_smallsuv_s` match a stock
roster id exactly, so `va_sedan_s`/`va_garbagetruck`/`va_pickup_f`'s
tables are dead data under it) — and stamps the resolved
`AmbientEngineSpec` on every spawned car as `AmbientAudio`.
`ambient_engine_rigs` builds one `PlaybackMode::Loop` spatial child
voice per car (`MAX_AMBIENT_VOICES` 32, `AmbientRig` marker so a
failed resolve warns once and a recycled body re-attempts), and
`ambient_engine_drive` re-mixes pitch off the parent's
`LinearVelocity` magnitude through the authored bands — first
covering band in file order wins (the authored `0–500` catch-all
would otherwise shadow the tight bands), an uncovered speed reads the
nearest band's edge. `engine volume` is constant — a stopped car
idles at the same gain. Retail headless: `london --frames 1500` →
`aud=…/20e/16n`, `sf --frames 1500` → `aud=…/34e/16n` (e = voices
spawned over the run — it tracks `sp`, the spawn count; n = loops
audible at record — the live fleet; `0s` stays honest: no output
device headless). Which classes the original binds, what it feeds the
bands and whether it pitches the ambient loop at all are unrecovered
(DSN-41/UNK-25). Sustained-scrape semantics and the
weather→{dry,wet,ice} surface-variant binding remain F07-B/C work,
and DirectMusic is recognized but not decoded (F08).

F07-B.7 adds the siren-program consumer: the session resolves
`aud/cardata/player/<psdl-stem>policesiren.csv` (the city-keyed naming
the exe's `sfpolicesiren`/`londonpolicesiren` strings imply) and the
shared `aud/cardata/opponent/policesiren.csv` into a `SirenAudio`
resource — absent or malformed warns once and leaves that side
programless, never substituted. A car whose horn row carries
`flags & 4` toggles the authored program on each `HornRequest` press
(press-to-toggle is the designed reading; the original's trigger —
hold vs toggle vs pursuit state — is unverified, UNK-25) and never
plays its horn sample. `SirenPlayback` enters at sample 0, draws one
step per entry through the seeded `NavRng`, dwells the authored
`play time` on the session's fixed tick (pause freezes the program)
and follows `next index` to the next *sample* — retail programs all
cycle; a negative/out-of-range target, an empty sample or a
non-finite `dt` ends it, `MAX_SIREN_HOPS` bounds a zero-time chain.
Each activation owns one `PlaybackMode::Loop` voice, respawned on
every authored switch — non-spatial for the local player, spatial
otherwise (DSN-37), `SessionEntity`-swept, `MAX_SIRENS` 8 with `+Nd`
drops. Stems prefer the `aud/*/sirens/` subtree (the exe's
`sirens\%s` scope) then the global bank; a sentinel is authored
silence and an unresolvable stem warns once per activation while the
program walks on. The `Explosion sample` binding is carried with no
consumer (F20). `aud=` gains `/<n>w/<n>y` when nonzero (DSN-42).
Everything below is measured data structure; runtime semantics are
unverified unless noted.

## Waves (`aud/aud11`, `aud/aud22`)

All 2 503 `.wav` files are standard uncompressed RIFF/WAVE PCM, decoded
by `mm2_formats::wav::Wav` (bounded chunk walk, even-byte padding, chunk
order + offsets preserved):

| Format | Files |
| --- | --- |
| 11025 Hz mono 16-bit | 2 112 |
| 22050 Hz mono 16-bit | 375 |
| 48000 Hz mono 16-bit | 13 |
| 11025 Hz stereo | 1 |
| 22050 Hz stereo | 1 |
| 44100 Hz mono | 1 |

240 902 980 bytes of PCM, ≈ 10 235 s total. Layout:

- `aud/aud11/{al1..al6, as1..as5, ccl, ccs}` — 1 831 files, plus 226 at
  the root. Filenames carry a rate suffix (`*.11k.wav`); `al*`/`as*`
  look like speech/voice lines (the `spchdata` CSVs below appear to be
  their cue tables — inferred).
- `aud/aud22/{amb3d, creature3d, engines, horns, impacts, sirens,
  surfaces, suspension}` — 292 files plus 88 at the root, `*.22k.wav`
  rate suffixes. These are the samples the cardata tables reference.

**Name resolution rule (verified against the audit):** cardata sample
references carry *no* rate suffix and no directory — `enginesedan1`
resolves to `aud/aud22/engines/enginesedan1.22k.wav`. The audit's lookup
stem strips `.wav` then a trailing `.<digits>k` suffix, case-folded.
Whether the original ever prefers `aud11` variants for these references
is unverified (the `aud11` tree looks speech-dominated).

## DirectMusic (`aud/dmusic`)

95 RIFF containers, recognized by form word only — not decoded:

| Ext | Form | Files | Role (inferred) |
| --- | --- | --- | --- |
| `.sgt` | `DMSG` | 62 | DirectMusic segments |
| `.sty` | `DMST` | 17 | DirectMusic styles |
| `.dls` | `DLS ` | 14 | Downloadable sound banks |
| `.bnd` | `DMBD` | 2 | DirectMusic bands |

Plus 5 text CSVs under `aud/dmusic/csv_files/` (deferred). Playback is
F08 scope; the audit fails a container whose form word disagrees with
its extension (none do on retail).

## Cardata tables (`aud/cardata/**`, `aud/ambient/**`)

103 CSV tables parse through `mm2_formats::cardata` — path-based
grammar dispatch (`cardata::classify`), recoverable rows preserved as
`TableDiagnostic`s, semantics checked by per-table `validate()`.

### Car audio — `cardata/{player,opponent}/{vp*,default}.csv`

One file per vehicle per side (player + opponent variants), plus a
handful of development leftovers (`copy of *.csv`, `vpbullet.wrk.csv`).
Layout:

```
Horn wave name,Horn volume,flags,Num Engine Samples,clutch wave name,clutch volume
VWHORN,0.95,0,4,REVERSE,0.93
Engine wave name,<columns…>
VWIDLE,0.55,0.835,1,800,2500,7000,0.85,2,1,7000
```

Three authored engine-column layouts ship on retail:

- **Fade-window** (canonical, 10 value columns): `Min Volume`,
  `Max Volume`, `fade in start RPM`, `fade in end RPM`,
  `fade out start RPM`, `fade out end RPM`, `Min Pitch`, `Max Pitch`,
  `Pitch shift start RPM`, `Pitch shift end RPM`. RPM windows describe
  crossfading loops (idle/drive/mid/high); semantics *inferred* from
  names, unverified.
- **`Pitch divisor` variant** (9 columns, `copy of vpford.csv`): the
  same six fade columns then `Min Pitch`, `Max Pitch`, `Pitch divisor`.
- **Compact divisor schema** (7 columns, `copy of default.csv`,
  `copy of vpmustang99.csv`): `Min Volume`, `Max Volume`,
  `Volume divisor`, `Min Pitch`, `Max Pitch`, `Pitch divisor`,
  `vol inverse RPM` — an older development schema.

Values are stored positionally with the header preserved verbatim;
`EngineSample::column(needle)` looks up by normalized column name.
Declared `Num Engine Samples` drifts from the authored row count on ~35
files (players commonly declare 2 but author 4; opponents declare 2–4
but author 1) — an authored quirk, reported not corrected.

### Engine params — `cardata/engineparams{play,opp}.csv`

2 and 3 rows. Every row's name field is a binary blob — MSVC debug fill
(`0xCD`/`0xDD`) plus garbage, 83–97 bytes — followed by nine positional
`%.6f` floats with no header. Preserved verbatim (`name_raw`), reported
as `BinaryNameField` quirks. Column meanings unverified (they share the
engine-sample column count minus one).

### Impact tables — `cardata/{player,opponent}/default_impacts.csv`

`***` section separators (`""""` in the Excel-resaved `copy of` file),
`Banger name,Num samples,ID` category headers, `sample name` rows with
`min volume,max volume,min force,max force,frequency`. 24 categories /
28 samples per file, `ENDOFDATA` terminator. Category names are banger
stems (`WALL`, `LIGHTPOLE`, …); `dgBangerData.AudioId` is 0 on every
retail record, so the runtime binds `AudioId`→category `ID` with an
id-0 (`WALL`) catch-all — a designed reading of the only selector the
data names, unverified (UNK-25). The two files diverge in-column: the
opponent table authors `WALL` bands two orders of magnitude below the
player table's (e.g. `2–500–1500` vs `1000–8000–20000` force ranges) —
an authored inconsistency the runtime does not normalize; the session
reads the player-side file (the local listener's authored mix).

### Surface tables — `cardata/{player,opponent}/default_surface{dry,ice,wet}.csv`

`Tunnel sound index` (0 on dry and on opponent wet, **5** on ice —
and on the player wet file — meaning unverified), then one entry per
surface index (positional; the material `sound` class is the only
authored selector, a designed binding — UNK-25). The schema split is
per **variant**, not per side (verified on both dirs, 2026-09-24):

- Dry/wet schema (9–10 columns): `max speed`, rolling volume/pitch
  windows, skid volume band, `num skid samples` (opponent wet adds
  `for tunnels`); skid bands keyed `min slippage,max slippage`.
- Ice schema (12 columns): `surface vol divisor`/`surface pitch
  divisor`/`skid vol divisor` fields instead of a `max speed` window,
  plus `for tunnels`; skid bands keyed `min speed,max speed` — a
  different trigger unit for the same table slot.

`ENDOFDATA` terminator. Dry/wet tables carry a `skidflagstone` reference
that resolves to **no** wave stem — the one dead reference on retail.

### Siren programs — `cardata/{player,opponent}/*policesiren.csv`

`Explosion sample` row (player files only — no recovered consumer),
then `Sample name` sequences of `play time,next index` steps — a
tiny state machine chaining siren loops; retail repeats the header
before *every* step. Player files are city-keyed
(`sfpolicesiren.csv`/`londonpolicesiren.csv` — both names appear in
the exe alongside the shared opponent `policesiren` and a
`sirens\%s` wave-path format); sf: 4 samples/16 steps, london: 2/2,
opponent: 3 samples looping. `next index` targets are *sample*
positions and every retail value is in range — the authored programs
never terminate. Binding is the horn-row `flags` bitmask: `vpcop`
alone authors `4` (`vpbus` 1, `vpcentury` 2, `vpddbus`/`vpsemi` 8 —
the fire truck's horn slot is literally `FIRETRUCKSIREN`, suggesting
the bits pick the horn control's behavior family; only bit 4 is
bound so far — DSN-42/AUD-10).

### Ambient engines/horns — `cardata/ambient/*_{engine,horn}.csv`

Ambient traffic audio: one engine sample with speed bands
(`min speed,max speed` → `engine min pitch,engine max pitch` ranges;
the tight piecewise bands precede a `0–500` catch-all carrying an
extreme pitch range, ~24.6 on `va_sedan_s`), horn clips with honk
sequences (`num honks` + per-honk durations). `default_engine`/`_horn`
plus per-ambient-type files: 7 engine tables (`va_bus_f`,
`va_ddbus_l`, `va_diesels_s`, `va_garbagetruck`, `va_pickup_f`,
`va_sedan_s`, `va_smallsuv_s`) and 8 horn tables (`va_compact_s` adds
one). Naming quirk: the
roster id is `va_sedans_s` but the authored table is
`va_sedan_s_engine.csv`, and `va_garbagetruck`/`va_pickup_f` are
rostered in neither stock city — under exact-match resolution those
three engine tables are dead data and their classes read the default
(runtime binding: F07-B.6, DSN-41).

### Object audio — `aud/ambient/*.csv` + `cardata/ambient/subwaycar.csv`

Positional ambient emitters: clips with volume/attenuation fields and
`VECTORPOINTS` sections listing world-space emitter positions (up to 99
points — `waves.csv`, `trolleycable.csv`). `drawbridge`, `ferry`,
`birdies`, `buoyseals`, `horns_gulls`, `londonriver`, `tubevoices`,
ped-voice tables (`maleped*`, `femaleped1`).

### Ambient containers — `aud/ambient/*ambientcontainer.csv`

`file names` lists naming the sibling member tables per city —
`sfambientcontainer` → `birdies`, `buoyseals`, `horns_gulls`,
`trolleycable`; `londonambientcontainer` → `londonriver`, `tubevoices`.
The audit verifies every member resolves.

### Shared tables

- `shared/semidata.csv`: `freightreverse` (reverse beeper) +
  `freightairblow` (air brakes) — the shared semi/bus extras.
- `shared/vehtypes.csv`: 3 vehicle-type groups (name lists).
- `player/suspensionaudio.csv`, `player/tirewobble.csv`: single-row
  band tables (suspension thump / wobble samples).

## Cross-checks (retail)

- **160 distinct sample names** referenced by parsed tables; **159
  resolve** to discovered wave stems. The miss is `skidflagstone`
  (surface tables, both sides) — a genuine dead authored reference.
- **Vehicle coverage:** every `tune/vp*.info` roster id has both player
  and opponent cardata. One orphan: `vpcaddie59` cardata exists with no
  tune entry (banger/vehicle assets for `vpcaddie`/`vpcaddie59` do
  exist — reported as a finding, not a failure).
- **Sentinels** treated as no-wave references: `NOSOUND`, `NOTHING`,
  `ENDOFDATA`, `FALSE`, `NONE`, empty.
- **Work files** (`copy of *.csv`, `*.wrk.csv`) parse but are excluded
  from roster coverage sets.

## Deferred (F08 scope)

552 text CSVs stay unparsed: `aud/spchdata/**` (526 — speech cue tables
for the `al*`/`as*`/`cc*` wave trees) and `aud/creaturedata/**` (21 —
ped/ambient voice tables; verified plain text: `Min speed,Max speed,
min time in range,…` headers) plus `aud/dmusic/csv_files` (5). Three
`.bat` work files (`renshit.bat` etc.) are extras.

## Open semantics

- Engine table application: whether the original computes the same
  envelope/pitch formula the runtime's designed reading uses and which
  quantity it consumes (implemented: sim drivetrain RPM — DSN-36); what
  the `Volume divisor`/`Pitch divisor`/`vol inverse RPM` schema
  computes; and what drives the `clutch wave name` binding (a reverse
  loop, a shift blip — unbound).
- `Tunnel sound index` semantics (0 vs 5, why only ice differs).
- Skid-band trigger unit — the two schemas claim `slippage` and `speed`
  for the same table slot.
- `default_impacts` semantics: the runtime binds `AudioId`→`ID` with an
  id-0 fallback and weighs `severity × striker mass` against the force
  bands — both designed readings (the original's selector, force
  quantity and per-side emission rule are unrecovered); the opponent
  file's divergent `WALL` bands have no consumer yet.
- `flags` word on the horn row — bit `4` (`vpcop`) now binds the
  siren program (DSN-42); `1`/`2`/`8` on vpbus/vpcentury/vpddbus/
  vpsemi have no recovered meaning; also whether the original holds
  the horn for the press duration or retriggers it — the runtime
  fires one authored clip per press as a designed policy.
- Whether `aud11` variants ever serve non-speech references (the
  runtime `WaveBank` prefers `aud22` on a stem tie, siren stems the
  `aud/*/sirens/` subtree first — both designed choices).
- Siren runtime semantics: the authored `play time`/`next index`
  chain is consumed, but the original's trigger (press-to-toggle is
  designed), its pick among a sample's authored steps, the
  `Explosion sample` consumer and program termination (all retail
  programs cycle; runtime ends defensively on malformed targets)
  stay unverified.
- DirectMusic segment/style/band playback (F08) and the `csv_files`
  cue tables.
- `spchdata`/`creaturedata` grammars (F08).
