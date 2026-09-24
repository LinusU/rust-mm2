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
is still unverified — the press fires one clip, a designed choice. The
engine/impact/surface/siren families remain unparsed at runtime and
DirectMusic is recognized but not decoded (F08). Everything below is
measured data structure; runtime semantics are unverified unless noted.

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
retail record, so the category↔prop binding is by name category at best
— mechanism unverified.

### Surface tables — `cardata/{player,opponent}/default_surface{dry,ice,wet}.csv`

`Tunnel sound index` (0 on dry/wet, **5** on ice in both dirs — meaning
unverified), then one entry per surface index (positional; which index
maps to which material is unverified):

- Opponent schema (9 columns): `max speed`, rolling volume/pitch bands,
  skid volume band, `num skid samples`; skid bands keyed
  `min slippage,max slippage`.
- Player schema (12 columns): adds `vol divisor`/`pitch divisor` fields
  and a `for tunnels` flag; skid bands keyed `min speed,max speed` — a
  different unit than the opponent files claim for the same slot
  (authored inconsistency, preserved).

`ENDOFDATA` terminator. Dry/wet tables carry a `skidflagstone` reference
that resolves to **no** wave stem — the one dead reference on retail.

### Siren programs — `cardata/{player,opponent}/*policesiren.csv`

`Explosion wave name` row (player files only), then `Sample name`
sequences of `play time,next index` steps — a tiny state machine
chaining siren loops; retail repeats the header before *every* step.
sf: 4 sequences/16 steps; london: 2/2.

### Ambient engines/horns — `cardata/ambient/*_{engine,horn}.csv`

Ambient traffic audio: one engine sample with speed bands
(`min speed,max speed` → volume/pitch ranges), horn clips with honk
sequences (`num honks` + per-honk durations). `default_engine`/`_horn`
plus per-ambient-type files (`va_bus_f_*`, …).

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

- Engine table application: which RPM/quantity drives the fade windows
  (vehicle `tune` RPM vs wheel speed vs throttle), and what the
  `Volume divisor`/`Pitch divisor`/`vol inverse RPM` schema computes.
- `Tunnel sound index` semantics (0 vs 5, why only ice differs).
- Skid-band trigger unit — the two schemas claim `slippage` and `speed`
  for the same table slot.
- `default_impacts` force→sample selection and the category↔banger
  binding (`AudioId` is 0 everywhere).
- `flags` word on the horn row (always 0 on retail); also whether the
  original holds the horn for the press duration or retriggers it —
  the runtime fires one authored clip per press as a designed policy.
- Whether `aud11` variants ever serve non-speech references (the
  runtime `WaveBank` prefers `aud22` on a stem tie — designed choice).
- Siren `next index` wrap/entry semantics beyond the obvious chain.
- DirectMusic segment/style/band playback (F08) and the `csv_files`
  cue tables.
- `spchdata`/`creaturedata` grammars (F08).
