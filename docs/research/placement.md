# Residual sunk placements outside the prop-rule channel

Triage of the `mm2-inspect placement` *sunk* findings (operator report 4
item 4, F03-C.3; task OPR-4.4b). After the kerb-lift fix the prop-rule
channel stamps `sunk 0/5 118` on retail London; the other two channels
(`city/<c>.inst`, `city/<c>/props.pathset`) still report sunk stamps.
Both channels are **verbatim authored positions** — nothing is derived
or lifted by the loader — so a sunk finding there is either authored
data or a defect in the audit's reference surface, never a stamping
bug. Nothing here changes any Y; no constant is applied.

## How the numbers were taken

`mm2-inspect placement $MM2_RETAIL` prints only the 24 deepest hits per
city. The full lists below come from the same run with
`MAX_HITS_SHOWN` raised locally (not committed), then grouped by
channel × band × name × room. A hit is a walkable surface standing
0.05–1.0 m above the stamp base, more than 0.05 m inside its XZ
footprint (`SUNK_EPS` / `SUNK_MAX` / `SUNK_DEPTH_EPS`).

| city   | INST | pathset | note |
|--------|-----:|--------:|------|
| london | 94 (Driving 2, FloorFan 92) | 46 (FloorFan 20, SidewalkTop 26) | the task text's "54" no longer reproduces |
| sf     | 72 (Driving 1, FloorFan 68, SidewalkTop 3) | 37 (Driving 2, FloorFan 17, SidewalkTop 18) | |

## INST (verbatim building transforms)

- **London FloorFan, 92 hits.** The covering surfaces are flat
  `Fan` floors at exactly y = 0.0 (63 hits) and y = 5.0 (18 hits); the
  building origins sit 0–0.3 m below them (58 hits in 0.1–0.2 m, 28
  under 0.1 m). Runs of terrace houses share one value (room 9: six
  `kw_hous01lb_stuc_wht01_1s_4_l` at y = 4.81 under the 5.0 floor, each
  0.19; room 30: `kw_hous01rb_*` at y = −0.19 under 0.0). Consistent,
  repeated, sub-0.3 m offsets across a row are authored: building
  origins follow the terrain by row while the lot-floor fan is a flat
  datum, and a building's base is below its floor by construction.
  *Classification: authored foundation depth, not a datum error.*
  The one cluster that does not fit is **room 166**:
  `ww_hous1_brick_red_2s_12_l` at y = −1.00 and two
  `ww_hous1_stone_tan_b6_12_l` at y = −0.69, 2.4–4.5 m inside a flat
  y = 0 fan (the only INST hits over 0.5 m in London).
  *Unknown: the retail mesh base is not measured here, so whether
  these are deliberately deep foundations is not established.*
- **London Driving, 2 hits** (`kw_storefront*`, `cw_b*_store*`, rooms
  1021 / 1176): 0.06–0.12 m under a walkway/road-fan surface, ≥1 m
  inside it. Storefront origins on pedestrianised streets; same
  magnitude as the floor-fan noise. *Authored, benign.*
- **SF FloorFan, 68 hits, 43 rooms.** Penetrations spread 0.05–1.0 m
  (median 0.27) and the covering surface heights are all distinct (no
  shared datum like London's 0.0/5.0), on hillside rooms. This is the
  pattern of a sloped lot-floor fan sampled at a building's origin
  while the building spans the slope. *Inferred, not measured:* the
  slope-vs-footprint test is not in the audit. Names are
  `*_canneryshop*`, `sw_b*_tft*`, `gw_b*_bung*`, `sw_coit_*` — building
  shells, none street furniture.
- **SF SidewalkTop, 3 hits** (`pl_ggbridge_{marin,ftpnt}_upper01_f`,
  rooms 562/563/570, 0.5–0.71 m): Golden Gate bridge-tower pieces over
  a derived sidewalk top (the audit's kerb-foot chain lifted by
  `SIDEWALK_KERB_LIFT`), not over an authored surface; a tower base
  standing in the deck is expected. *Authored (bridge structure).*
- **SF Driving, 1 hit** (`nw_b*_canneryshop*`, room 1118, 0.07 m):
  noise.

## Pathset (verbatim prop rows)

- **London SidewalkTop, 26 hits** — 18 are `sp_lightthames_l` in rooms
  856/858 (Thames embankment): y ≈ 7.92 against a 8.0 sidewalk top,
  0.06–0.09 m; four outliers at y = 7.65–7.84 (0.14–0.32 m).
  *Authored: the poles are planted ~8 cm below the embankment top, the
  outliers are per-row authoring jitter.* The remaining 8
  (`sp_lightpark`, `sp_pilaster`, `sp_cone`, `sp_dumpstr`) are 0.05–0.21 m.
  Not the kerb defect: the kerb burial was 0.135 m on *prop-rule*
  stamps whose Y the loader computes; these Y values are file data.
- **London FloorFan, 20 hits**: bollards, cones, trees at 0.06–0.16 m
  under plaza fans, three trees (`sp_tree1_s`) deeper inside the fan
  (11–30 m) at ≤0.52 m. *Authored plaza dressing; tree roots below the
  floor are invisible.*
- **SF SidewalkTop, 18 hits**, with one real cluster: pathset **136**
  (`sp_lightstreet_rt_f`, rooms 567/568, Golden Gate approach). The
  four poles are at y = 34.31, 34.82, 35.33, 35.84 along z — a straight
  1.17 % authored grade — while the covering surface rises faster
  (penetration 0.23, 0.45, 0.68, 0.90). The pole line and the PSDL
  deck disagree in *slope*, which is an authored inconsistency between
  two retail files (the poles are authored on a constant grade, the
  deck is not), or the derived `SidewalkTop` is steeper than the real
  deck there. *Unknown which; the only SF cluster over 0.5 m.* Path 133
  (same bridge, 0.05–0.09 m) and the rest (`sp_dumpstr`, `sp_cone`,
  `sp_crashbarrelgroup`, `sp_hillwarn`; 0.05–0.27 m, ≤0.5 m deep) are
  authored noise.
- **SF Driving, 2 hits** (`cp_banrred_*`, 0.08–0.12 m): banners
  pivoted on the carriageway (see the audit's header); benign.
- **SF FloorFan, 17 hits**: 12 `sp_tree*_s`, 4 `sp_barricadewood_f`,
  one `sp_hotdogcart_f` (0.67 m, 6.4 m inside a sloped fan, room 195).
  Trees are planted into slopes; the cart is the single outlier.

## Result

No cluster is a loader defect and none justifies touching a Y. Of 249
sunk stamps (London 140, SF 109): the large majority are authored
sub-0.3 m offsets (foundations, planted bases, roots, jitter). The
residue that is **not** explained, and is left open:

1. London room 166 terraced houses (−0.69 / −1.00 m under a flat floor).
2. SF pathset 136 (Golden Gate approach lamps, 0.23–0.90 m, slope
   mismatch between the pole line and the deck).
3. SF INST FloorFan hillside buildings (> 0.5 m: rooms 27, 147, 193,
   125, 59, 111, 24, 17) — slope hypothesis unmeasured.
4. `sp_hotdogcart_f` pathset 0:1, SF (0.67 m).

Resolving 1 and 3 needs the building meshes' lowest local Y against the
fan slope over the footprint; 2 needs the deck surface compared with the
authored pole line. The audit's `sunk` counter is a measured finding,
not a failure (`--strict` ignores it).
