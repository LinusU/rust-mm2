# Pathset placement files (`*.pathset`, `PTH1`)

Pathsets are MM2's second placement mechanism after INST. Where INST
positions individual monuments, a pathset stamps a series of copies of
one prop PKG or decal texture along a named point sequence — rows of
trees, street lights, barricades, skid marks, crosswalk decals.

Measured on all 101 retail `.pathset` files (2026-09-20):

- `city/<city>/props.pathset` + `decals*.pathset` — the ambient city
  dressing (london 87/84–99 paths, sf 144/52).
- `city/<city>/audio_pathsets/*.pathset` — ambient-sound source paths;
  names are `PATHnn` labels, not assets.
- `city/phys/{props,decals}.pathset`, `city/race0.pathset`,
  `city/sf/backup.pathset`, `city/sf/bak/*` — dev/test/snapshot sets.
- `race/<city>/*.pathset` — per-event placements: circuit/crash course
  barricades (`sp_barricadeconc[lr]_f`), parked cars (`PATHnn` routes
  + `sp_pcar*`), and the animated-object paths for the London
  bridges/ferry/train/sailboat and SF ferry/sailboat (`giz_*` names).

## Grammar (R3-documented, confirmed byte-exact on retail)

```text
"PTH1" u32 path_count u32 current_path
per path: char[32] name (NUL-padded, may fill all 32 bytes)
          u32 point_count u32 selection
          point_count * (u32 attributes + f32 x + f32 y + f32 z)
          u8 kind u8 spacing u8 pad[2]
```

- `kind` (R3-documented): 0 = one object per point; 1 = point *pairs*
  — position plus a second point whose offset sets yaw about Y;
  2 = line strip, objects stamped along each segment at `spacing`.
- `spacing` is authored in units of 1/4 metre (`spacing_code / 4`).
  Measured on retail: 0 m (562 paths — densest possible, e.g. every
  point), 5 m (1906 — the common value), up to 60 m. R3 notes spacing
  is ignored for kinds 0/1; retail still carries 5 m on all kind-1
  paths.
- Path *names* are the asset basename: `geometry/<name>.pkg` for
  props, `texture/<name>.*` for decals (VFS lookup is
  case-insensitive — `r4i_railsX_f` resolves to `r4i_railsx_f.tex`).
  `PATHnn` names are internal route labels, never asset refs.

## Preserved-but-uninterpreted fields (UNK-20)

- Per-point `attributes` word — undocumented; recurring values
  (`0x1b5`, `0xf0`, `0x3b09`…) do not decode to anything verified.
  Kept raw.
- `current_path` (file) / `selection` (path) — inferred dev-tool
  cursors: `selection` is always `< point_count` or equal, and
  `current_path` is a valid path index on every retail file (usually
  the last). Kept raw; `validate()` reports an out-of-range cursor.
- `PREFIX:` name decorations — `OPEN:`/`open:`/`inactive:` appear on
  `giz_*` bridge/gate props (`london_bridge_circuit0.pathset` carries
  `OPEN:giz_bridge01_l`), inferred to be event-state variants; the
  audit strips the prefix for asset resolution.
- Other authored prefixes seen: `sp_`/`np_`/`op_`/`wp_`/`cp_`/`xcp_`/
  `giz_`/`prop_`/`decal_`/`trackdecal_`/`r4i_`/`r_` — naming
  conventions, not verified semantics.

## Audit results (`mm2-inspect pathset`, retail 2026-09-20)

- 101 files discovered = expected denominator; **98 parse**, 2692
  paths / 13185 points; kinds {0: 137, 1: 122, 2: 2433}. All
  successfully parsed paths use documented kinds.
- **3 authored junk files fail**: `race/london/blitz10.pathset` (58
  bytes, truncated mid-header), `race/london/blitz11.pathset` (663
  bytes — path 1 declares `0x40000` points), and
  `race/london/london_bridge_blitz10.pathset` (326 bytes, truncated
  mid-points). Reported, not repaired (UNK-20).
- **66 empty paths** (0 points, all three kinds) are normal authored
  data — `decal_zigzag_l`, `r4i_rails_f`, `sp_cleat_f`… an empty path
  stamps nothing; `validate()` does not flag them.
- Name cross-check: 120/126 unique asset names resolve; 6 dead refs,
  all in dev/snapshot files — `r_concrete`, `sp_boxfruit_f`,
  `sp_fruitcard_l`, `sp_plantcard_l` (`city/phys/`), `prop_sp_*`
  (`city/race0.pathset`, names `sp_*` without the `prop_` prefix),
  `xcp_banrred_f` (`city/sf/bak/`). Every mainline
  `city/<city>/{props,decals*}.pathset` name resolves.
- `validate()` issues on retail: none — kinds all documented,
  directed paths all even-counted, cursors in range.
- `--strict` exits 2 (3 failures + 6 issues); `--city london`
  → 46 files, 3 failures, 0 issues; `--city sf` → 52 files, 1 issue
  (`xcp_banrred_f` in the `bak/` snapshot).

## Placement-source inventory note (F03-A)

| Family | Supplies | Parser |
| --- | --- | --- |
| `city/<city>.inst` | monuments/landmarks, one per record | `mm2_formats::inst` |
| PSDL room props | embedded per-block geometry | `mm2_formats::psdl` |
| `*/props.pathset`, `*/decals*.pathset` | prop rows, decals | `mm2_formats::pathset` (this doc) |
| `race/<city>/*.pathset` | event barricades, parked cars, animated-object routes | `mm2_formats::pathset` |
| `*/audio_pathsets/*.pathset` | ambient-sound source paths (F07/F08 scope) | `mm2_formats::pathset` |
| `.cpvs`, `.ldef` | block culling / lights — F18 scope, still unparsed | — |
| `race/*.opp`, `*waypoints.csv`, `*_strtpnts` | opponent paths, gates, start grids | F11-A parsers |

## Runtime consumption (implemented for props, 2026-09-20)

`mm2_app::city::load_city` now consumes the `props.pathset` beside the
loaded PSDL (`<dir>/<stem>.psdl` → `<dir>/<stem>/props.pathset`) and
stamps each path's prop PKG through the same `PropCache` INST uses —
shared meshes, materials and one static trimesh collider per instance.
Measured on retail: london 1188 instances / 87 paths, sf 925 / 113
paths — the 31 remaining sf paths are `r4i_rails_f` *decal* names
living inside `props.pathset`, reported as `decal paths`, not
failures. Rendered check: sf's `sp_lightstreet_rt_f` lamp row draws
evenly spaced along its authored strip.

Stamping rules implemented (R3-documented kinds; the micro-semantics
are inferred and tracked under UNK-20):

- `Points`: one unrotated prop per vertex.
- `Directed`: one prop per pair at the first point, yawed about Y so
  local +X runs toward the second (the INST heading convention — R3
  does not name the axis).
- `LineStrip`: each segment fills at `spacing` intervals from its
  start (t = 0, s, 2s, … < len — the shared vertex is stamped by the
  following segment's t = 0), the final vertex caps the row, stamps
  yaw along their segment. Whether the original restarts spacing per
  segment or runs it continuously is unverified.
- Zero spacing on a strip → one unrotated prop per vertex; a lone
  vertex stamps once. Undocumented kinds and odd `Directed` tails
  stamp nothing (`validate()` reports them).

Runtime bounds (implementation choice, not an original rule): the
parser caps path/point counts but coordinates are unbounded, and a
strip expands to `len / spacing` stamps per segment — so the loader
threads a per-file budget of 8192 stamps (~7x the densest retail
expansion, London's 1188) through all paths and counts any suppressed
stamps in `CityReport::pathset_props_capped` rather than truncating
silently. `Pathset::validate()` runs at load and its issues land in
`CityReport::pathset_issues`; non-finite coordinates stamp nothing.

Still unconsumed: `decals*.pathset` (texture stamping — strip width
and orientation semantics unknown), `audio_pathsets/` (`PATHnn` sound
routes, F07/F08), `race/<city>/*.pathset` (event-scoped overlays —
barricades, parked cars, animated `giz_*` object paths; F03-AC04),
and `city/phys/`, `bak/` dev sets. Which pathsets the original loads
per session and whether the stamping micro-rules match its output
remain unverified (UNK-12/UNK-20).
