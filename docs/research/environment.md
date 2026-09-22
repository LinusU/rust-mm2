# Weather and environment formats

The stock install's weather/environment content family: the sky dome
definition, the 16-preset time×weather lighting grid, ambient-light
definitions, the room-visibility (PVS) tables and their runtime history,
the water plane, and the per-room light map. All files live under `city/`.

Measured on the retail install (2026-09-22) by `mm2-inspect weather`:
102 environment files discovered, 102 parsed, 0 failures, 0 issues —
the audit's expected denominator is the 21 per-city files
(`<stem>.sky`, `.lt00`–`.lt15`, `.cpvs`, `.pvshist`, `.water`, `.lmap`
= 1 + 16 + 4) × 2 cities plus the 32 shared `amb_*` `.ldef` files = 74
expected; the other 28 files are audited extras (23 `.cpvs` variants,
three named `.ldef`s, `city/phys/j01.sky`, `sf082100.pvshist`).

Sources: R3 = community format docs (`angel-file-formats`), R4 = MM2Hook
recovered structures (`lvlSky`, `cityTimeWeatherLighting`,
`IsRoomVisible`).

## `.sky` — sky dome definition (verified on retail)

One line of text: `<model> <f1> <f2> <f3>`.

| file | model | f1 | f2 | f3 |
| --- | --- | --- | --- | --- |
| `city/london.sky` | `sky_dome_l` | 0 | 0.95 | 0.005 |
| `city/sf.sky` | `sky_dome` | 0 | 0.95 | 0.005 |
| `city/phys/j01.sky` (extra) | `sky_dawn` | 30 | 0.95 | 0.005 |

- `model` resolves to `geometry/<model>.pkg` — both dome meshes ship;
  `j01.sky`'s `sky_dawn` resolves to `geometry/sky_dawn.pkg`.
- R3 documents the file as the dome object + its position relative to
  the PSDL. R4's recovered `lvlSky` maps the three floats to
  `HatYOffset` / `YMultiplier` / `RotationRate` (the parser uses those
  names — recovered, not documented).
- Parser: `mm2_formats::sky::SkyDef` — exact 4-token arity, finite
  floats validated.

## `.ltNN` — time×weather lighting presets (verified on retail)

`city/<stem>.lt00` … `.lt15` — sixteen text records per city in the
shared tune/block grammar (`type: a`, one root block). Each record is
MM2Hook's recovered `cityTimeWeatherLighting` (R4): a `Key` (sun) light
and `Fill1`/`Fill2` fills, each `Heading`/`Pitch` (radians) + `Color`
RGB, plus a packed `Ambient` colour (BGRA-packed i32, alpha opaque on
retail).

The `NN` index is the authored grid slot, measured on all 32 records:

```text
NN = tod * 4 + weather
tod:     0=morning 1=noon 2=evening 3=night
weather: 0=clear   1=cloudy 2=foggy  3=rainy
```

Every retail record's block name (`<weather>-<tod>`) classifies back to
its own file slot — the audit enforces this; the file index (not the
name) is the position `timeOfDay`/`weather` select. This resolves the
`mm*data.csv` `Weather`/`TimeofDay` column order half of UNK-1.

Retail values (both cities author different sets — these are the
authored originals, not dev presets): heading −3.14..3.14, pitch
−1.4..1.4, colours 0.0..1.0 channels, ambients 0xFF02020A..0xFF646464
packed. Example `sf.lt00` (`clear-morning`): key h2.20/p−0.20
rgb(0.9,0.9,0.8), ambient 0xFF1E1E32.

Parser: `mm2_formats::lighting::LightingPreset` —
`classify()`/`index()`/`ambient_rgba()`/`validate()`; `WeatherKind`,
`TimeOfDay`, `preset_index`, `LIGHTING_PRESET_COUNT = 16`.

## `.ldef` — ambient light definitions (verified, semantics unrecovered)

35 files: the shared `city/amb_<w><t>_<v>.ldef` grid (w ∈ {c,f,p,r},
t ∈ {a,d,m,n}, v ∈ {f,l} — 32 files, letter semantics inferred:
w ≈ clear/foggy/partly-cloudy/rainy, t/v unverified) plus three named
extras (`london_clearmorn`, `sf_clearmorn`, `sf_clearnoon`).

Text: first line is a bake-source path on the original dev machine
(`\\taxi\projects\madness2\art\...\*.tif` — provenance only, never
resolves to shipped content and must not be redistributed as an asset),
then integer rows — two signed pairs on every retail file:

- `amb_*_f` files: `-2300 2600` / `500 -1800`
- `amb_*_l` files: `-1500 1250` / `1500 -1250`
- named extras: `-2300 2600` / `500 -1800`

Measured naming alignment (inferred pairing, audit-checked): every
`amb_<grid>.ldef` has a `texture/sky_<grid>.tex` counterpart — 32/32 on
retail. The integer pairs are preserved verbatim; a milli-radian
heading/pitch reading is plausible but unverified (UNK-24).

Parser: `mm2_formats::ldef::Ldef` (`source_ref`, `rows`,
`texture_stem()`).

## `.cpvs` — room PVS tables (verified on retail)

Binary `PVS0`: `u32 index_count`, `index_count - 1` u32 stored indices,
then one RLE payload. Index 0 is implicit 0; stored index *k* is the
compressed end of list *k*, so `list_count = index_count - 1` and
list *i* = `payload[indices[i]..indices[i+1]]` (last list ends at the
payload end).

RLE (verified against R3 + retail bytes): control < 0x80 = fill run of
`control` copies of the next byte; control ≥ 0x80 = literal run of
`control - 0x7F` bytes. Decompressed output is bounded
(`MAX_LIST_BYTES = 8192`).

Visibility layout (verified against mm2hook `IsRoomVisible` — the
doc-derived initial 2-bit offset yields only 888/1341 London rooms
self-visible and was rejected): room `r` occupies byte `r >> 2`, bits
`2 * (r & 3)`; code `0b11` = visible, `0b00` = hidden; codes `01`/`10`
are documented but never occur on retail. Rooms past a list's stored
length are implicitly 0.

Retail measurements:

| file | lists | nonzero | max bytes | not self-visible |
| --- | --- | --- | --- | --- |
| `london.cpvs` | 1342 | 1341 | 336 | 1 |
| `sf.cpvs` | 1172 | 1171 | 293 | 2 |

`lists == psdl.rooms + 1` holds for both base tables (1342/1341,
1172/1171; list 0 reserved). The 23 `.cpvs` variant extras (11 london
incl. `london_bad`, 12 sf incl. `sf082100`) share the same shape with
differing nonzero/self-visible counts —
the fog/detail-variant hypothesis is measured data, not a recovered
original rule; `london_254`/`sf_00`/`sf_254` author fewer lists than
rooms + 1. A handful of authored rooms (1–14 per file) do not see
themselves — an authored anomaly the audit reports, not an issue.

Parser: `mm2_formats::cpvs::Cpvs` (`decompress`, `code`, `is_visible`,
`visible_rooms`, `validate` → `CpvsIssue`).

## `.pvshist` — PVS history (verified format, inferred semantics)

Whitespace-separated `from to weight` rows (u32), sorted by `from` then
`to`; first/last rows are `<1 1 255>` and `<rooms rooms 255>`.
Retail: london 95,112 rows (max room 1341), sf 105,022 (max 1171),
`sf082100` 105,021. Weights saturate at 255; small even values
(2,4,6,8,…) mark rarely-observed pairs. Rows read as a runtime
visibility history — which room pairs were actually seen, likely used to
refine the static `.cpvs` table — inferred, not documented (UNK-24).

Note: the archive entry is DAVE-compressed inside `mm2core.ar` (the
deflate stream covers only ~311–342 KB of each ~1.3–1.6 MB entry; the
rest is padding). That is archive storage handled by `inflate_entry`,
not part of the file format — the parser takes the inflated text.

Parser: `mm2_formats::cpvs::PvsHist` (`rows: Vec<PvsHistRow>`).

## `.water` — water level + room refs (verified format, inferred semantics)

Text: first non-blank line is the water level (world Y), following
lines are integer references (PSDL block/room ids per R3 — unverified).

- `city/london.water`: level −3.8, refs [345, 351, 356]
- `city/sf.water`: level −1.9, refs [228, 399, 401]

All refs are < the city's PSDL room count. Which consumer reads the
level, and what the refs bound (water rooms? deadly volumes?), is
unverified (UNK-24). Parser: `mm2_formats::water::WaterDef`.

## `.lmap` — per-room light map (verified format, semantics unrecovered)

Binary `LMP0` + `u32 count` + `count` i32 entries, filling the file
exactly. Retail: london 1341 entries (= PSDL rooms), sf 1125 entries
(authored mismatch — 46 short of sf's 1171 rooms; reported, not an
issue). Most values are −267; entry 0 is −842150451 (0xCDCDCDCD — an
authored/uninitialized sentinel, preserved verbatim, documented rather
than normalized). Value semantics are unrecovered (UNK-24).

Parser: `mm2_formats::lmap::Lmap`.

## `mm2-inspect weather`

```text
mm2-inspect weather <install> [--city <stem>] [--strict]
```

Census of every discovered environment file (denominator never
filtered); per-file parse through the production decoders with measured
stats; cross-checks:

- `.sky` dome name → `geometry/<model>.pkg` resolution.
- `amb_<grid>.ldef` ↔ `texture/sky_<grid>.tex` pairing (inferred).
- `.ltNN` block name ↔ file slot, and 16/16 slot coverage per city.
- `<stem>.cpvs` `lists == psdl.rooms + 1`; `.lmap` count vs rooms
  (authored-mismatch note); `.pvshist` max room ≤ rooms; `.water` refs
  ≤ rooms — all against `city/<stem>.psdl`.
- Non-`amb_*` `.ldef` bake-source paths reported as provenance notes.

`--strict` exits nonzero on any failure or format-violation issue;
authored anomalies (self-invisible rooms, the sf lmap shortfall, the
0xCDCDCDCD sentinel, dev-path sources) are notes/findings, so a stock
retail install exits 0.
