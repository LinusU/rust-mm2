# Weather and environment formats

The stock install's weather/environment content family: the sky dome
definition, the 16-preset time×weather lighting grid, the per-preset
fog table, ambient-light definitions, the room-visibility (PVS) tables
and their runtime history, the water plane, and the per-room light map.
All files live under `city/`.

Measured on the retail install (2026-09-22) by `mm2-inspect weather`:
105 environment files discovered, 105 parsed, 0 failures, 0 issues —
the audit's expected denominator is the 22 per-city files
(`<stem>.sky`, `.lt00`–`.lt15`, `.cpvs`, `.pvshist`, `.water`, `.lmap`,
`<stem>_fog.csv` = 1 + 16 + 4 + 1) × 2 cities plus the 32 shared
`amb_*` `.ldef` files = 76 expected; the other 29 files are audited
extras (23 `.cpvs` variants, three named `.ldef`s, `sf_fog_orig.csv`,
`city/phys/j01.sky`, `sf082100.pvshist`).

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
- The dome binds at runtime (F18-A.4): `spawn_sky_dome` resolves the
  model through the VFS and draws it — see *Runtime consumption*
  below for the transform/paint-job readings, which are designed
  rather than recovered.

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

## `_fog.csv` — per-preset fog table (verified on retail)

`city/<stem>_fog.csv` — one CSV per stock city: a header row
(`fog red,fog green,fog blue,fog start,fog end,description (ignored)`)
then sixteen rows of `r,g,b,start,end,label`:

```text
250,230,200,650,1000,clear-morning
```

The row index is the same `tod*4 + weather` slot the `.ltNN` grid uses
— measured: every row's label equals the `.ltNN` block name at its
position on both cities (16/16, audit-enforced). The header's own
"description (ignored)" tag confirms the original reads the table
positionally. MM2Hook's recovered `lvlSky` holds the landing site:
`FogColors[16]` / `FogNearClip[16]` / `FogFarClip[16]` indexed by
`TimeWeatherType` — the same 16-slot grid.

`fog start`/`fog end` are the near/far clip distances the fog factor
interpolates between (fixed-function linear fog — the curve shape is
inferred from the recovered field names; the values are authored).
Retail bands differ sharply per city and per preset: london
`clear-morning` fogs 220–320 m while sf's runs 650–1000 m; the foggy
slots are extreme (sf `foggy-morning` 2–100 m, london `foggy-evening`
10–50 m); night slots fade to near-black colours.

`city/sf_fog_orig.csv` is an extra: the same 16-row shape with a
different labelling convention (`clear morning` — spaces, not the
`.ltNN` hyphen form) and markedly wider foggy bands (sf
`foggy-morning` 400–800 m vs the shipped 2–100 m). It reads as an
earlier authored revision kept on disk — audited, never consumed; the
runtime binds only the canonical `<stem>_fog.csv`.

Parser: `mm2_formats::fog::FogTable` (`rows`, `row(slot)`,
`validate()` → `FogIssue`: row count, non-finite/negative values,
out-of-range colour, `end <= start` degenerate band).

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
`visible_rooms`, `validate` → `CpvsIssue`). Runtime consumer:
`mm2_app::pvs` (F18-A.5 — see "Runtime consumption" below).

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
- `<stem>_fog.csv` row *i* label ↔ `<stem>.ltNN` block name at slot
  *i* (the measured positional mapping; extras like `sf_fog_orig.csv`
  are parsed but not cross-checked — their labels use a different
  convention).
- `<stem>.cpvs` `lists == psdl.rooms + 1`; `.lmap` count vs rooms
  (authored-mismatch note); `.pvshist` max room ≤ rooms; `.water` refs
  ≤ rooms — all against `city/<stem>.psdl`.
- Non-`amb_*` `.ldef` bake-source paths reported as provenance notes.

`--strict` exits nonzero on any failure or format-violation issue;
authored anomalies (self-invisible rooms, the sf lmap shortfall, the
0xCDCDCDCD sentinel, dev-path sources) are notes/findings, so a stock
retail install exits 0.

## Runtime consumption (F18-A.2/.3/.4/.5)

The `.ltNN` presets bind to Bevy lighting and the `_fog.csv` row binds
to Bevy fog through the production session path —
`mm2_app::environment::spawn_environment`, called from
`load_session_world` *after* event resolution so an authored event's
`EventParams::conditions` take precedence over the session's
configured ones (RACE-2; the shared resolution point is
`mm2_game::effective_conditions`; a player `SessionCustomization`
pick beats both — DSN-29). `SessionConfig::conditions` is the
cruise/dev fallback, selected session-legally by `--weather` /
`--time-of-day` (the authored 0-3 grid; out-of-range is a usage error,
exit 2 — never a clamp). The menu's options screen picks the same
fields session-legally (F17-A.6).

Binding (per `docs/research/environment.md`'s recovered record):

- `Key`/`Fill1`/`Fill2` → three `DirectionalLight`s. Directions use
  R4's recovered `setLightDirectionInv` convention via
  `LightSpec::to_light_dir`/`travel_dir` — to-light =
  `(−cos h·cos p, −sin p, −sin h·cos p)`, the Bevy forward axis gets
  the negated travel direction. Colours bind verbatim; only the key
  casts shadows (designed — the fills stand in for bounce). The
  unusual authored case is preserved: `rainy-night` keys pitch +1.4,
  so the key shines from below the horizon and contributes nothing to
  upward faces — the fills do the work.
- `Ambient` → `GlobalAmbientLight` colour from the BGRA-packed i32.
- `city/<stem>_fog.csv` row `slot` → `DistanceFog` on both session
  cameras (chase + free): authored RGB → `Color::srgb`, authored
  `fog start`/`fog end` → `FogFalloff::Linear` — an implementation
  mapping of the clip distances onto Bevy's linear falloff (the curve
  shape is inferred; the recovered `lvlSky` names the same near/far
  clips). The fog channel is independent of the lighting one — a
  preset that fell back still binds its fog row.
- Missing/unparseable preset → the pre-preset fixed rig (one
  directional sun + fixed ambient) spawns and
  `EnvironmentReport.fallback` is set — an explicit diagnostic
  (F18-AC06), never a silent default. `EnvironmentReport` is
  session-scoped (removed on teardown); light entities are
  `SessionEntity`-stamped and despawn with the session. Smoke records
  carry `env=ltNN(<name>|fallback)` plus ` fog=<start>-<end>` or
  ` fog=none`.
- Missing/unparseable fog table, a slot with no row, or a row whose
  band cannot interpolate (`end <= start`, non-finite, negative
  start) → no `DistanceFog` is attached and `EnvironmentReport.fog
  .absent` names the reason (`missing`/`unparseable`/`no row for
  slot`/`degenerate`) — never a fabricated default. Table-level
  anomalies stay warnings in `fog.issues`.
- `city/<stem>.sky` → the sky dome (F18-A.4): `spawn_sky_dome`
  resolves `geometry/<model>.pkg` through the VFS and draws it at the
  same effective slot — the dome's authored paint jobs are the same
  `tod*4 + weather` grid (`sky_dome_l` job *i* textures run
  `skylondon_{c,p,f,r}{a,n,d,m}_l` in slot order; `sky_dome` the
  `sky_*_f` equivalents — measured on retail), so paint job
  `slot % jobs` binds the preset's sky texture. The mesh is scaled
  from its measured ~43 m extent to a designed 900 m radius, is
  unlit/double-sided/fog-exempt (the authored texture is the sky's
  final colour), re-centres on the active camera each frame and
  rotates by `rotation_rate × dt` — `drive_sky_dome`. The field
  readings are designed, not recovered (UNK-24): `HatYOffset` as the
  dome's world height, `YMultiplier` as its vertical squash,
  `RotationRate` as radians/second, camera-centring as the horizon
  policy. Missing/unparseable `.sky`, a model the VFS cannot
  provide, a non-finite transform field or an empty mesh → no dome +
  `EnvironmentReport.sky.absent` naming the reason; the smoke field
  shows ` sky=<model>:<texture>` or ` sky=none`. A missing dome
  *texture* warns and draws the shared fallback material (the prop
  policy).
- `city/<stem>.cpvs` → the authored room-PVS render culling
  (F18-A.5), `mm2_app::pvs`. Retail `cityLevel` decompresses the
  view room's list into a 512-byte buffer and `DrawRooms` gates each
  room draw group on `IsRoomVisible` (`code != 0`); `sm_EnablePVS` is
  the global toggle. Ours: every per-room render mesh is spawned
  `CityRoom`-tagged (authored id = `rooms` index + 1), and
  `apply_city_pvs` re-resolves the source set each frame — every room
  whose authored XZ perimeter contains the active camera *or* the
  player vehicle (retail `sdlPage16::PointInPerimeter` is the same 2-D
  test; `FindRoomId`'s `previousRoom` hint is a search shortcut, not a
  different answer) — and unions their decompressed lists, so a
  stacked/overlapping or camera-lagged pick can only over-show, never
  hide an authored-visible room. Disabled (`--no-pvs`, retail's
  `EnablePVS(false)`), unresolved, or list-less sources bypass
  entirely. A missing/unparseable `.cpvs` yields no `CityPvs` resource
  — unculled, logged, never fabricated. Colliders are physics and
  never culled. Smoke gains ` pvs=<room>r/<hidden>h/<tagged>` (or
  ` pvs=off`); absent without a table. Known authored-table behaviour,
  not a defect: elevated/non-gameplay viewpoints see rooms the table
  marked invisible — identical captures at gameplay height differ only
  by capture noise (AE ~1.5 kpx baseline vs ~28 kpx at a 75 m aerial,
  all in distant skyline rooms).

Designed (not authored) scales: a uniform 15 000 lux illuminance per
directional light — the authored `Color` carries each light's relative
weight, exactly as the original's per-channel diffuse contribution —
and a fixed `GlobalAmbientLight::brightness` of 2 000 anchored so the
typical authored day ambient (~30/255 lum) lands near the previous
fixed ambient (~300 effective). Consequence to keep honest: authored
night ambients are *brighter* than day ones (grey-80 vs 30-blue), so a
night preset still renders lighter than a real night inside the fog's
near band — authored data, not a preset-selection bug. The `.sky` dome
is deliberately fog-exempt (`fog_enabled = false`) — it is the
backdrop the fog fades toward, not a fogged object; whether the
original fogs its dome is unrecovered (UNK-24).

Retail evidence (fingerprinted install, 2026-09-22): `sf.lt00` →
`clear-morning`, `sf.lt06` → `foggy-noon`, `sf.lt15` → `rainy-night`,
`london.lt05` → `cloudy-noon` bound through the real path
(`environment lighting bound` log + `env=` smoke field); rendered
captures at a frozen `--cam` show visibly different lighting per
preset; `--weather 4` exits 2. Fog (same install): headless runs
record `env=lt00(clear-morning) fog=650-1000` on sf and `fog=220-320`
on london — each city's own authored band; a frozen-`--cam` capture
pair at sf `foggy-noon` (authored 10–120 m) vs `clear-noon`
(600–1000 m) shows the skyline dissolving into the authored grey
versus fully resolved. Synthetic tests cover the slot map,
authored-event precedence, the fallback report, validation-issue
counting, the fog row's camera binding, the authored-event fog
precedence, and the missing/degenerate diagnostics. Sky dome (same
install, 2026-09-23): `env=… sky=sky_dome_l:skylondon_ca_l` on london
and `sky=sky_dome:sky_ca_f` on sf — each slot-0 paint job binds its
authored texture; frozen-`--cam` captures pitched up at the dome show
the authored cloud/gradient textures on both cities (Metal/Apple M1,
PNGs inspected).

Still not consumed (UNK-24 stays open): `.cpvs` variant selection
(the numbered `<stem>_N` files — the base table now culls), `.ldef`
rows, `.pvshist` weights, `.lmap` values,
`.water`, precipitation/wetness/audio effects (F18-B/C scope), and
authoritative network replication of conditions (F18 req 5). The
dome's three `.sky` floats are bound under designed readings
(world-height / vertical squash / radians-per-second) — the original's
transform composition and rotation units are unverified. The fog
curve's exact original shape (the linear reading is inferred), any
per-weather `.cpvs` variant switching the recovered `lvlSky` may
drive, and `sf_fog_orig.csv`'s role also stay open.
