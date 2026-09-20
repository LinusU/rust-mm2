# AIMAP override files

Per-city and per-event AI-map configuration (`city/<name>.aimap`,
`race/<city>/<stem>.aimap`, `race/<city>/<stem>.aimap_p`). They carry
the ambient/police/opponent settings a session applies on top of the
BAI road network: default speed limit, per-road exceptions, cop and
opponent spawns, ambient vehicle roster, drive-on-left, ped models.

Measured on all 209 retail files (2026-09-20): 108 `.aimap` +
99 `.aimap_p` under `race/{london,sf}/`, plus `city/london.aimap` and
`city/sf.aimap`. The `_p` variant's role is unverified (Amateur vs
Professional vs multiplayer — see `mm2_formats::racefiles`, ledger
MP-9); the grammar is identical.

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
- Police/opponent tail columns, `[Traffic Lights]` shape, `[Hookmen]`
  rows, `_p` role: **unknown**, preserved raw.
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
