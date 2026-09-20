# Roadside-prop rule tables (`propdefs.csv`, `proprules.csv`, `props.csv`)

The third placement mechanism after INST and pathsets: each PSDL room
carries a `prop_rule` byte (parsed as `Psdl::prop_rules`), and these
CSV tables turn that byte into the props stamped along the room's
perimeter — street lights, trees, parking meters, mailboxes, benches,
trash cans, phone booths. This is the sidewalk dressing that lines
every street block.

Measured on all 16 retail files (2026-09-20):

- `city/<city>/propdefs.csv` — named prop prototypes (london 16, sf 33).
- `city/<city>/proprules.csv` — `n{NN}left`/`n{NN}right` rows listing
  propdef names (london 16 numbers × 2 sides, sf 20 × 2).
- `city/<city>/props.csv` — `Group,Name` membership list; retail uses
  only the `Races` group (london 19, sf 16 props).
- `city/props.csv`, `city/london/props.csv.txt`,
  `city/sf/propdefs02.csv.txt`, `city/phys/*`, `city/sf/bak/*` —
  top-level/dev/snapshot copies audited the same way.
- `geometry/props.csv` — a *different* table sharing the basename:
  per-PKG LOD triangle counts (`Name,H Tris,M Tris,L Tris,VL Tris`, 8
  `va_*` traffic vehicles on retail).

## Grammar (verified against every retail file)

`propdefs.csv`: header `name,start,distance,maxUse,minLerp,maxLerp,`
`file1,file2,file3,file4` (trailing empty columns common), then rows
`name,int,int,int,float,float[,pkg…]`. The file columns hold 1–4 PKG
basenames; empty cells are dropped.

`proprules.csv`: header `rulename,prop1,…,prop8`, then rows
`rule,propdef…`. Every retail rule name fits `n{NN}left`/`n{NN}right`.

`props.csv`: header `Group,Name`, then `group,pkg-basename` rows. The
`city/phys/` dev copy mislabels the header `name,start` while writing
the same `group,name` rows — the parser tolerates any ≥2-column header
and preserves the labels.

`geometry/props.csv`: `Name,H Tris,M Tris,L Tris,VL Tris`, PKG
filenames *with* the `.pkg` extension. The four counts are not ordered
by magnitude on retail (`va_bus_f.pkg` = 68/178/78/12), so they are
preserved as authored columns, not reinterpreted as sorted LODs.

## The `prop_rule` byte ↔ rule-number link

`Psdl::prop_rules` distributes exactly over the defined rule numbers:

- london: bytes 1–16 used (415 rule-bearing rooms of 1342; 927 rooms
  carry 0 = no roadside props) ↔ rules `n01`–`n16` all defined.
- sf: bytes 1–20 (397 rule-bearing of 1172) ↔ `n01`–`n20`; `n14` is
  defined but referenced by no room.
- Both cities have **one room carrying byte 205** with no `n205` rules
  — an authored anomaly, reported as an audit issue (same class as
  `sfai.bai`'s room-ref-0).

So byte `N` selects the `n{NN}left`/`n{NN}right` row pair.

## PSDL prop-path geometry (measured on retail, 2026-09-20)

The `Psdl::paths` records carry the rooms the rules apply to. Measured
on `city/{sf,london}.psdl`:

- A `RoomPath` is one road run between two junction crossings.
  `road_rooms` lists the road rooms it passes through; consecutive
  entries share a *direct* road-to-road boundary — a junction sits
  between two paths, never inside one (sf: 360 single-room, 19
  multi-room paths; london: 500/40).
- `start_crossroads`/`end_crossroads` name the two **curb** vertices
  of the junction crossing at each path end — the pair is adjacent on
  the room perimeter.
- A crossing occupies four consecutive perimeter points
  `[outer, curb, curb, outer]` (outer = building-line corner, curb =
  road-edge corner). At a direct road-to-road boundary the curb pair
  is the *widest* consecutive pair of perimeter points marked with
  the neighbouring room's id (the curb gap spans the road width;
  the corner–curb gap only the sidewalk).
- The two perimeter arcs between the entry and exit crossings are the
  sidewalk building lines; each pairs with the curb segment joining
  the crossings' curbs on that side. Arc bulge over the straight curb
  averages ≈9 m (sf) / ≈7 m (london) — consistent with sidewalk
  corner returns, not a second roadway.
- Some sf `road_rooms` entries hold out-of-range values (65 0xx) — a
  different encoded record kind, not room refs (109 entries across 23
  sf paths, 0 on london). The walk counts them as bad refs rather
  than interpreting them.
- Every rule-bearing room a sane path reaches resolves its crossing
  pair under this model (`no_crossing = 0` on both cities). 52 sf +
  5 london rule-bearing rooms are reached by *no* path — authored
  coverage gaps, kept visible in the walk stats.

## Field semantics — inferred, not verified (UNK-21)

| Column | Retail values | Status |
| --- | --- | --- |
| `start` | 1–20 | implemented as metres along the side's curb segment from its walk-start crossing |
| `distance` | 1–90 | implemented as spacing (m) between successive placements of that def |
| `maxUse` | 1–9999 | implemented as a per-def, per-side, per-room cap (`9999` ≈ unlimited) |
| `minLerp`/`maxLerp` | 0.1–0.5, always equal | implemented as the curb→outer lerp factor, `(min+max)/2` (0.1 ≈ curb-hugging, 0.5 ≈ mid-sidewalk) |
| `file1`–`file4` | 1–4 PKG names | implemented as a variant pick list chosen by a deterministic hash; the original's `RandomSeed` selection is unrecovered |

Implemented walk policy (all inferred — a retail-visual comparison
can still falsify any of them):

- Each side is walked so the road stays on the walker's left: the
  right-of-travel side runs entry→exit, the left-of-travel side runs
  exit→entry, and `start` measures from the crossing its walk begins
  at. `n{NN}left`/`n{NN}right` are assigned by measured lateral sign
  against the travel direction (authored left-handed convention
  `right(d) = (d.z, −d.x)` — a one-line flip if comparison shows the
  labels swapped).
- A stamp faces its walk direction; the app yaws the prop's +X axis
  along it (same convention as directed pathset stamps).

`props.csv` group meaning is likewise unverified: `Races` groups the
props race events place (barricades, cones, parked cars) plus some
generic dressing; whether the original uses the table for anything
beyond bookkeeping is unknown.

## Audit results (`mm2-inspect proprules`, retail 2026-09-20)

- **16/16 files parse** (6 expected + 10 extras), zero unsupported.
- Rule→def references: every `proprules.csv` prop ref resolves to a
  sibling `propdefs.csv` entry — including `city/sf/bak/`'s `speed`
  def, which exists only in the `bak/` table.
- Dead PKG refs (authored anomalies, reported): london `props.csv`
  `sp_bollard_pedsafe_l` (a `sp_bollard_pedsafe.dgbangerdata` exists,
  the PKG does not); `geometry/props.csv` LOD row `va_garbagetruck.pkg`;
  all 41 `city/phys/` `*_m` file refs plus `sp_boxfruit_f`,
  `sp_fruitcard_l`, `sp_plantcard_l` in `phys/props.csv` — the same
  dead names the pathset audit reports for `city/phys/props.pathset`.
- `--strict` exits 2 (48 issues: 41 phys `*_m` file refs + 3 phys
  group entries, `sp_bollard_pedsafe_l`, `va_garbagetruck.pkg`, 2 ×
  rule-205). `--city london` → 4 files / 2 issues; `--city sf` →
  7 / 1.

## Runtime consumption

`mm2_game::props::walk_prop_rules` resolves every `road_rooms` entry
to its crossing runs and emits `PropStamp`s (room, side, def, chosen
variant, authored-space position + walk direction, per-def ordinal);
`load_city` spawns them through the shared `PropCache` with the same
bound/unbound classification as pathset stamps — bound names become
dormant banger entities, the rest ordinary static props.

Retail (2026-09-20, `city {london,sf}` headless + screenshots):

- sf: 345 rooms stamped, **5 002** props — every one bound as a
  dormant banger (consistent with WLD-16: prop-rule files all bind);
  0 unresolved variants, 109 bad `road_rooms` refs counted, 52
  rule-bearing rooms unreached.
- london: 410 rooms stamped, **5 083** props, all bound; 0 bad refs,
  5 unreached.
- Screenshots: lamps line both sidewalks at the authored ~29 m
  staggered spacing, banner arms over the road (sf park road /
  freeway parapets / london Trafalgar Square phone booths, trees,
  bollards). Placement visually consistent; orientation of asymmetric
  props not yet compared against the original frame-by-frame.
- Side note: `texture/p_parkmeter_f.tex` declares 7 mips on a 32×32 —
  the TEX decoder now clamps `mip_level_count` to the size-supported
  maximum and warns (was a hard wgpu validation error once prop-rule
  props pulled it in).

Still unverified (UNK-21, narrowed): the original's left/right label
assignment, whether `start`/`distance`/`maxUse` scope is per room or
per path, the variant-pick rule, `minLerp`≠`maxLerp` behaviour, prop
yaw convention, the encoded non-room `road_rooms` record kind, and
the `props.csv` `Races` group's consumer.
