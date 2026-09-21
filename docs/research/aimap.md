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
      numbers on 536 rows (first is a 0.75–1.0 skill-like value) or 1
      number on 1 row (`race/sf/stunt0.aimap`).
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
- Police/opponent tail columns, `[Traffic Lights]` shape, `[Hookmen]`
  rows: **unknown**, preserved raw.
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
