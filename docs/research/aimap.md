# AIMAP override files

Per-city and per-event AI-map configuration (`city/<name>.aimap`,
`race/<city>/<stem>.aimap`, `race/<city>/<stem>.aimap_p`). They carry
the ambient/police/opponent settings a session applies on top of the
BAI road network: default speed limit, per-road exceptions, cop and
opponent spawns, ambient vehicle roster, drive-on-left, ped models.

Measured on all 209 retail files (2026-09-20): 108 `.aimap` +
99 `.aimap_p` under `race/{london,sf}/`, plus `city/london.aimap` and
`city/sf.aimap`. The `_p` variant's role is now evidenced for the
checkpoint events (2026-09-20, F13-A): `<stem>.aimap` binds the
Amateur actor roster and `<stem>.aimap_p` the Professional one —
`[Opponent]`/`[Police]` counts match the corresponding
`mmracedata.csv` parameter block on 23 of 24 checkpoint events
(london race0: aimap 4 `vpcoop` vs amateur Opponents 4, aimap_p 6
`vpcoop2k` vs professional 6). The one mismatch is `sf/race0`:
the amateur block asks for 7 opponents while `race0.aimap` wires 6 —
a seventh `race0-a-6.opp` route file ships but is unreferenced
(authored inconsistency, reported not repaired). Opponent `.opp`
refs inside aimaps follow the same split (`-a-`/`p-` name suffixes).
The grammar is identical; whether `_p` also carries a multiplayer
meaning is separate (ledger MP-9).

## Grammar (measured, no shipped documentation)

- CRLF line endings; `#`-prefixed lines are comments anywhere; blank
  lines ignored. `#[Section]` is therefore a comment, not a section.
- `[Name]` on its own line starts a section. Order varies; most
  sections are optional; none repeat on retail.
- Body forms, by section:
  - **Scalar** — one value line: `[Density]` (float), `[Speed Limit]`
    (float; R3 notes it overrides the BAI `baseSpeed` entirely),
    `[Ambients Drive On The Left]` (0/1; 1 london, 0 sf),
    `[CopChaseDistance]` (float; only `race/sf/crash5.aimap{,_p}`),
    `[AmbientLaneChanges]` (0/1; only `race/sf/roam.aimap{,_p}`).
  - **Counted list** — decimal count line then that many rows; every
    retail count matches exactly:
    - `[Exceptions]` — `road-id density speed-limit` (3 fields). All
      retail density/speed values are `0.00`/`0` — the authored meaning
      is "no ambient traffic on this road", consistent with R3's claim
      that race aimaps close course roads.
    - `[Police]` — `geo x y z <tail>`: geo basename (`vpcop`), spawn
      position, then 5 further numbers on 227 rows (first is a heading
      in degrees) or 2 on 10 rows (`race/sf/evade0.aimap`). The file's
      own comment ("Geo File, StartLink, Start Dist, Start Mode, Start
      Lane, Patrol Route") does not match the observed column count;
      tail columns are preserved raw.
    - `[Opponent]` — `geo waypoint-file <tail>`: geo basename
      (`vpcoop`, `vpford`, …), a `*.opp` path record name, then 10
      numbers on 536 rows or 1 number on 1 row
      (`race/sf/stunt0.aimap`). The tail decodes into the driving-
      parameter vocabulary mm2hook recovers — see *Opponent
      parameter tail* below.
    - `[Ambient Types/Density]` — `name cumulative-weight flag`
      (flag 0, omitted on one roambak row). Weights are non-decreasing
      and close at exactly 1.0 on every retail file — a cumulative
      pick table over `va_*` ambient vehicles.
    - `[GoodWeatherPedName / BadWeatherPedName]` — `good-name bad-name`
      ped model pairs (city files only).
    - `[Hookmen]` — counted; only a 0-count instance exists
      (`race/sf/roam.aimap{,_p}`), row shape unknown.
  - **Free-form** — `[Traffic Lights]`: one line of two model names
    (`sp_traflitsingle_ped_l sp_traflitsingle_ped_l`), `city/london.
    aimap` only. Shape unverified, preserved raw.
- Sections this parser does not know are preserved verbatim in
  `Aimap::unknown_sections` and reported by `validate()` — none exist
  on retail (commented-out `[Subway]`/`[Ped Pool]` in london.aimap are
  `#` comments).

## Anomalies found by the audit (`mm2-inspect aimap`)

- **London exception ids exceed `city/london.bai`.** Eight files
  (`blitz0.aimap_p`, `blitz2{,_p}`, `blitz5`, `race0{,_p}`,
  `race1{,_p}`) reference road ids 562–815 while `city/london.bai`
  holds 540 roads with sequential ids 0–539. Every SF exception id is
  in range (0–378). Whether the over-range ids address a different
  road-id space (e.g. the unparsed `london_sup.bai` variant) or are
  authored dead references is unknown — reported, not repaired (UNK-18).
- **`race/sf/stunt0.aimap`** names opponent waypoint file `opp-c0.2`,
  which resolves nowhere in the VFS — a dead authored reference.
- `race/london/roambak.aimap` ambient rows omit the flag column
  (2 fields); accepted with flag = 0.

## Confidence

- Grammar/section vocabulary/counts: **measured** — all 209 files
  parse, every declared count matches, section set enumerated above.
- `_p` role: **measured** for checkpoint events — Amateur vs
  Professional split confirmed by `mmracedata.csv` cross-check
  (23/24; `sf/race0` authored anomaly noted above). Whether `_p`
  files outside the checkpoint roster follow the same rule is
  unverified per-file but consistent with the same convention.
- Opponent tail column *mapping*: **inferred** — mm2hook's recovered
  `OpponentData`/`RegisterRoute` vocabulary plus retail distributions
  (see below); not original-executable verified. Police tail columns,
  `[Traffic Lights]` shape, `[Hookmen]` rows: **unknown**, preserved
  raw.
- Runtime semantics (how the original consumes speed limits, density
  picks, spawns, drive-on-left): **unverified** — ledger UNK-12.

## Implementation

`mm2_formats::aimap` — `Aimap::parse(&str)` (grammar errors →
`FormatError`; malformed rows → `diagnostics` + skip) with typed
records (`RoadException`, `PoliceRecord`, `OpponentRecord`,
`AmbientTypeRow`, `PedNames`) and `Aimap::validate()` reporting
`AimapIssue` (duplicate exception roads, weight range/monotonicity/
closure, non-0/1 flags, negative scalars, uninterpreted sections).
`mm2-inspect aimap <install> [--city] [--strict]` audits every
discovered aimap: expected = `city/<stock>.aimap` + all
`race/<stock>/*.aimap{,_p}`; cross-checks exception road ids against
`city/<city>.bai` and resolves opponent waypoint refs through the VFS.
`.aimap`/`.aimap_p` are recognized `scan` formats.

## Opponent rosters (F15-A.1, 2026-09-21)

`mm2_content::opponents` turns a `CatalogEvent` into an
`mm2_game::opponent::OpponentRoster`: the event stem's `.aimap`
(Amateur) or `.aimap_p` (Professional, with an explicit missing-variant
fallback when only the amateur file ships) provides the `[Opponent]`
rows, each of which wires a vehicle geo id, a `.opp` route record
resolved through the VFS, and the raw parameter tail (first value kept
as `skill`). Route points keep every authored column (`mm2_formats::
opp`); driving semantics stay out of this layer. Roster issues —
count mismatch vs the table's `Opponents` field, a wired route missing
from the VFS, a `-a-`/`-p-` route name that disagrees with the selected
difficulty, and `.opp` records the selected roster references nowhere —
are reported on the roster rather than dropped or repaired, and the
authored slot survives even when its route does not resolve.

`mm2-inspect opponents <install> [--city] [--strict]` runs the same
builder over every cataloged event at both difficulties, cross-checks
wired vehicle ids against `VehicleCatalog`, and lists non-table stems
that still carry `[Opponent]` rows. Measured on retail: 64 builds per
city with 0 failures (26 unsupported — the two Crash Course tables,
which carry no race records), 271 london + 246 sf opponents wired,
0 unresolved vehicle ids. Findings, all authored-data:

- `sf/race0` amateur is the only wired-vs-table count mismatch
  (6 wired vs 7 authored) — the RACE-11 anomaly, now confirmed against
  every table kind, not just checkpoint.
- 43 london + 34 sf `.opp` records are wired by no `[Opponent]` row,
  including `blitz3`/`blitz4` route files on 0-opponent events. Spare
  routes are scoped to the variant that ships them and reported, never
  silently attached.
- `race/london/race12.aimap` (extra stem beyond the 12 table rows)
  wires a 1-opponent lineup; `race/sf/stunt0.aimap` wires 1 opponent
  whose `opp-c0.2` is a dead ref (the anomaly above).
- Professional lineups field harder vehicle variants than the amateur
  files for the same event (`vpcoop`→`vpcoop2k`, plus `vpvwcup`,
  `vpdb7`, `vppanoz`, `vppanozgt` — including the reward-locked Panoz
  GTR-1). All wired ids resolve to `ready` catalog entries.

## Opponent parameter tail (F15-B.2, 2026-09-21 — inferred)

The ten-value `[Opponent]` tail decodes into the driving-behavior
vocabulary mm2hook recovers (`OpponentData`, `aiVehiclePhysics::
RegisterRoute` — documented names, R4). The decode is **inferred,
not original-verified**: mm2hook's own `OpponentData` field order
does not match the retail distributions positionally (its four
floats land where retail authors flags and vice versa), so the
column assignment below orders the *same* recovered field names by
what each column's authored values can actually be. Every assigned
column's retail range matches the corresponding `RegisterRoute`
default, and short rows (`stunt0`'s single value) decode trailing
fields as absent rather than zero.

| col | field | retail range | consumed |
|-----|-------|--------------|----------|
| 0 | `maxThrottle` (default 1.0) | 0.57–1.00 | throttle ceiling on the scripted control law |
| 1 | `weirdPathfinding`/`BadPathfinding` flag | mostly 0; set on london `crash6`/`race12` rows | bound, unconsumed |
| 2 | `someDistancePadding`/`TurnRadius` | 50–150 | bound, unconsumed |
| 3 | `cornerBrakingThreshold`/`TurnSpeedMultiplier` (default 0.7) | 0.07–1.0, centred ~0.7 | bound, unconsumed |
| 4 | `unusedFlag` | 0/1 | bound, unconsumed |
| 5 | `avoidTraffic` | 0/1 | bound, inert — no ambient-traffic class exists |
| 6 | `avoidProps` | 0/1 | bound, inert — the corridor senses participants only |
| 7 | `avoidPlayers` | 0/1, both authored often | gates human participants in the traffic corridor |
| 8 | `avoidOpponents` | ≈ universal 0 | bound, **inert** — see below |
| 9 | `cornerSpeedMultiplier` (default 2.0) | 0.89–2.29 | multiplies the corner-brake engage speed |

The `avoidOpponents` caveat deserves emphasis: retail authors it ≈
universally 0, so consuming the flag as written would make every
stock opponent blind to the rest of the field — either the original
genuinely never avoids AI (possible; stock AI is famously chaotic),
or the flag order/polarity here is wrong. It stays decoded-but-inert
until it can be measured; the corridor senses AI unconditionally.
`avoidPlayers` is consumed because it carries real authored variance
(rows authoring 0 and 1 coexist in the same file — e.g.
`race/sf/race1.aimap_p`), so the gate differentiates per-driver.

Difficulty signal: amateur rows author `maxThrottle` at the low end
more often (sf `race0` amateur vpbug 0.70–0.75 vs professional
0.93–1.00; `circuit6` reverses — amateur 1.00 vs professional
vppanoz/vpdb7 0.81), and professional rows author
`cornerSpeedMultiplier` at the high end (2.0+ on later races —
`race/sf/race5.aimap_p` 2.29, `race/london/race2.aimap_p` 2.08 — vs
the ~1.0 amateur mode). Vehicle variants and routes also
differ per difficulty, so finish-pace differences are *not*
attributable to the tail alone — dynamics data, not a controlled A/B.

Runtime: `mm2_game::OpponentDriveParams` /
`OpponentSpec::drive_params()` decode the tail; `mm2_app::opponents`
binds it per driver at spawn into `ScriptedTuning`
(`throttle_cap`, `corner_speed`) plus the `avoid_players` sense gate.
`None` columns take the `RegisterRoute` defaults — the pre-tail
behavior. Ledger: RACE-14/UNK-11.

## `.opp` route records (measured 2026-09-21)

The `.opp` files themselves are CSVs headed
`x,y,z,brake,forward offset,side offset,target speed,speed start,side
start`. Measured on all 612 retail files: the `brake` header misleads —
a nonzero value marks a *staging record* whose payload is a heading in
vehicle-yaw degrees (forward `(−sin a, −cos a)` in XZ — the same
convention as `_strtpnts`' `a` column, and exactly 180° from the
waypoint `a` course bearing). Row 0 carries it on 592 of 612 files
(grid events share one value across all their routes — e.g. every
`circuit0-*` row 0 reads 175.0), and on 542 files that heading agrees
with the route's own course direction within ~25°. The staged point is
not necessarily *on* the driving line: `circuit1-a-0`'s heading runs
−X while row 1 sits +X of the spawn, so the authored line joins the
course mid-leg — chasing row 1 from the staged pose U-turns the car.
`race/sf/race5-a-{5,6,7}` carry a second staging row mid-file (what
re-stages there is unknown). Every other column authors 0 on retail
and is preserved raw.

The same zero-means-unset rule shows on the `_strtpnts` side:
`cir6_strtpnts` is the only all-zero `a` column on retail — its
position-matched `.opp` routes stage ~177–183° and the driving line
passes within ~6 m of the grid heading +Z, so a verbatim 0 yaw (−Z)
faces the whole grid backward. Producers therefore read `a = 0` as
"no authored heading" and derive a course facing instead of spawning
backward; every nonzero `a` on retail binds verbatim.
