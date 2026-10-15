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

The building-mesh leg comes from `mm2-inspect placement $MM2_RETAIL
--city <c> --sunk-mesh` (added for this task; prints every sunk INST
placement, uncapped). For each it resolves the placed PKG and prints the
mesh's lowest/highest local Y (`local_y`), its world base under the
authored transform (`base_y`), and the covering surface's height range
sampled at the origin and at every mesh vertex's XZ (`surface@footprint`,
with `samples`). INST positions a PKG verbatim, so `base_y` is the mesh's
real base in the world: a mesh authored with its base at `local_y = 0`
has its base exactly on the placement origin.

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
  *Measured (`--sunk-mesh`): all 94 London sunk INST meshes have their
  lowest vertex at `local_y = 0.00`, so the transform origin is the
  facade's base, not a pivot above it, and the covering fan is flat
  (`surface@footprint = [−0.00, −0.00]`), so no slope hides part of the
  offset either. The three bases genuinely stand 0.69/0.69/1.00 m below
  the flat y = 0 lot. Classification: authored base-below-floor
  (verbatim INST) — the same phenomenon as the ≤0.3 m fleet, at ~3× its
  depth — not a mesh-pivot or slope datum error.*
- **London Driving, 2 hits** (`kw_storefront*`, `cw_b*_store*`, rooms
  1021 / 1176): 0.06–0.12 m under a walkway/road-fan surface, ≥1 m
  inside it. Storefront origins on pedestrianised streets; same
  magnitude as the floor-fan noise. *Authored, benign.*
- **SF FloorFan, 68 hits, 43 rooms.** Penetrations spread 0.05–1.0 m
  (median 0.27). *Measured (`--sunk-mesh`):* the covering surface's
  height spread across each mesh's own XZ footprint runs 0.00–2.31 m;
  20 of the 68 spread ≥0.5 m and 13 are flat (<0.05 m). Every mesh here
  is authored with its base at `local_y = 0.00` except the deliberate
  undercroft family `nw_b4s_*/nw_b4awn_*` (seven hits, `local_y =
  (−3.00, 4.00)`) and the three Golden Gate tower slabs (`local_y ≈
  −0.03`). The hits the previous pass flagged over 0.5 m read:

  | room | mesh | local_y min | base_y | surface@footprint | spread | reading |
  |-----:|------|------------:|-------:|-------------------|-------:|---------|
  | 27 | `nw_b4_canneryshop01_8_f` | 0.00 | 62.28 | 61.28–63.28 | 2.00 | base crosses a 2 m grade |
  | 147 | `sw_coit_b4r_fff01_12_f` | 0.00 | 39.50 | 39.48–40.42 | 0.94 | base at the grade's low end |
  | 193 | `gw_b4_bung01_v_8_f` | 0.00 | 41.12 | 42.00 | 0.00 | flat fan, 0.88 m below it |
  | 125 | `sw_4_f01_12_f` | 0.00 | 42.80 | 43.64–44.10 | 0.46 | base below the fan's low end |
  | 59 | `nw_b5_canneryshop01_20_f` | 0.00 | 49.79 | 48.95–50.54 | 1.59 | base crosses a 1.6 m grade |
  | 111 | `nw_b4_canneryshop01_12_f` | 0.00 | 63.59 | 62.37–64.33 | 1.96 | base crosses a 2 m grade |
  | 24 | `nw_b4s_canneryshop01_8_f` | −3.00 | 67.52 | 69.39–71.23 | 1.84 | 3 m undercroft under a grade |
  | 17 | `sw_b4_tft01_8_f` | 0.00 | 42.09 | 42.78 | 0.00 | flat fan, 0.69 m below it |

  So the >0.5 m set is **two different authored relationships**: a flat
  base crossing a fan that rises 1–2 m under the footprint (rooms 27, 59,
  111, the sloped sides of 147/125 — SF's hillside grade), or a flat base
  under a fan that is constant to <0.01 m (rooms 193, 17 — the London-166
  shape). Names are `*_canneryshop*`, `sw_b*_tft*`, `gw_b*_bung*`,
  `sw_coit_*` — building shells, none street furniture.
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
  1.17 % grade — while the covering surface rises faster (penetration
  0.23, 0.45, 0.68, 0.90). **Resolved (task #1169): neither the derived
  `SidewalkTop` nor an authored pole line is wrong; the poles are
  interpolated, not authored.** Path 136 is a two-point `LineStrip`
  (kind 2, spacing code 160) from (−1770.0, 38.384, −1591.1) to
  (−1770.0, 33.799, −1199.6): a 392 m chord. Both authored endpoints
  lie on the derived `SidewalkTop` to within 1 mm (38.385 and 33.799);
  every stamp between them is a linear interpolation of those two Ys.
  The deck between them is not straight — sampled every 20 m it climbs
  to a crest of 39.40 m at z ≈ −1531 (room 565, 1.7 m above the chord)
  and falls back; at the audit's four hits (rooms 567/568) it is
  0.23–0.90 m above the chord. The derived surface is faithful to the
  raw vertices: room 567's `RoadWithSidewalks` refs resolve to a road
  edge running y 37.25 (z −1412.8) → 35.25 (z −1294.4), a 1.69 % grade,
  with the sidewalk outer chain at 37.40 → 35.40 (road edge +
  `SIDEWALK_KERB_LIFT` 0.15, parallel to the road). The *Driving* band
  is as far above the chord there, so this is not a sidewalk-lift
  artefact; the line at x = −1770.0 sits on the road/sidewalk seam
  x = −1770.3, which is why the audit files it under SidewalkTop. So
  the pole line is a straight chord through a vertically curved deck.
  Whether the retail game also stamps interior points with this linear
  Y (burying the poles) is not established; the loader's interpolation
  is unchanged and no Y is touched.
  Path 133
  (same bridge, 0.05–0.09 m) and the rest (`sp_dumpstr`, `sp_cone`,
  `sp_crashbarrelgroup`, `sp_hillwarn`; 0.05–0.27 m, ≤0.5 m deep) are
  authored noise.
- **SF Driving, 2 hits** (`cp_banrred_*`, 0.08–0.12 m): banners
  pivoted on the carriageway (see the audit's header); benign.
- **SF FloorFan, 17 hits**: 12 `sp_tree*_s`, 4 `sp_barricadewood_f`,
  one `sp_hotdogcart_f` (0.67 m, 6.4 m inside a sloped fan, room 195).
  Trees are planted into slopes; the cart is the single outlier.
  **Cart measured (task #1169):** it is path 0 point 1, an authored
  `Points` entry at (−1919.85, 40.679, 301.86), not an interpolation.
  The only walkable surface covering it is one room-195 `Fan`
  triangle, (−1965.4, 37.34, 348.1) (−1799.5, 43.00, 295.4)
  (−1955.7, 41.50, 295.4), a 165 m wide sliver whose plane gives
  41.348 m there (cart 0.669 m below). No PSDL vertex lies within
  25 m, so that height is extrapolated from corners 50–120 m away.
  The other 11 of the path's 15 points that have a covering surface
  agree with it to ≤ 4 mm (3 have none). So this one Y is either on
  terrain finer than the fan (a mesh the audit does not read) or an
  authoring slip; the PSDL alone cannot tell them apart. Left open.

## Result

No cluster is a loader defect and none justifies touching a Y. Of 249
sunk stamps (London 140, SF 109): the large majority are authored
sub-0.3 m offsets (foundations, planted bases, roots, jitter). The
building-mesh measurement (`--sunk-mesh`) settles the two INST residues
the previous pass left open:

- Every sunk INST building mesh in both cities has its lowest rendered
  vertex at `local_y = 0.00` (London 94/94; SF 62/72, the rest being the
  deliberate `nw_b4s_*/nw_b4awn_*` 3 m undercroft and the three Golden
  Gate tower slabs at ≈−0.03). INST positions the PKG verbatim, so
  `base_y` is the mesh's real base — there is no mesh-pivot offset that
  could turn a placement convention into a phantom penetration.
- London's covering fans are flat (88 of 92 FloorFan hits level to
  within 0.01 m; the other four spread at most 0.35 m), so room 166's
  0.69/1.00 m is a real, deeper authored base-below-floor beside a flat
  lot — the fleet's phenomenon at ~3× the depth.
- SF's covering fans span 0.00–2.31 m across the footprint (20 of 68
  ≥0.5 m). Most of the >0.5 m set is a flat base crossing a 1–2 m
  authored hillside grade; rooms 193 and 17 alone are the flat
  London-166 shape.

The four residues the previous pass carried, with their status after
this measurement:

1. London room 166 terraced houses (−0.69 / −1.00 m under a flat floor) —
   **resolved**: authored depth (base at the origin, flat fan); a data
   curiosity, not a defect.
2. ~~SF pathset 136~~ — resolved (task #1169): the poles are a linear
   chord between two on-deck endpoints over a crested deck (see above).
3. SF INST FloorFan hillside buildings (> 0.5 m: rooms 27, 147, 193,
   125, 59, 111, 24, 17) — **resolved**: authored hillside grade plus
   two flat-fan cases; no datum error.
4. `sp_hotdogcart_f` pathset 0:1, SF (0.67 m) — measured, still
   unexplained (single authored point; see above).

The audit's `sunk` counter is a measured finding, not a failure
(`--strict` ignores it).

## Golden Gate deck: render mesh is not collision (#1443)

Player report: cars and opponents are launched by something invisible
on the Golden Gate Bridge in the third SF Checkpoint race (`checkpoint:2`).
Reproduced headless (`--event checkpoint:2 --bot`, a per-tick trace of the
player's position, velocity and Avian contacts): at z ≈ −1273 the car
leaves the deck at 4 → 7.8 m/s vertical at 14 m/s forward, in contact
with `prop-pl_ggbridge_sanfran_mid_f-collider` (contact normal straight
down — a downward-facing plane hit from above).

Cause (measured, `tests/golden_gate.rs`): INST props collided with their
best-LOD render triangles, and the `pl_ggbridge_*_mid_f` pieces carry the
deck's underside as `pl_ggbridge_bot_f` planes. They are straight chords
between the piece ends; the PSDL road they sit under is not (a 1.7 m
vertical curve per 392 m, above). Across the whole span the plane stands
0.21–0.25 m *above* the road surface in the lanes and, being a trimesh, is
solid from above: a razor ledge. At the seam where the San Francisco side
piece meets the mid piece (z = −1183) the mid plane starts 0.2–0.3 m above
the road — a 0.25 m step at 32 m/s (`vy` 0.5 → 4.6 m/s, speed 32 → 25).

Fix: a static INST prop uses its authored bound (WLD-37). The bound's
deck is at or under the road everywhere the lanes run (≤ 0.15 m anywhere).
Cross-map: a road-centroid sweep over SF and London (props rising
0.2–3 m over any ground triangle) removes two phantoms —
`pl_ggbridge_sanfran_mid_f` (0.23 m) and London's
`cl_towerbrdge01_rt_l` (0.33 m) — and adds one, `nl_fishwheel_f`'s
authored pedestal column (0.94 m, a bound that is only the column and
footing; the mesh's wheel is no longer solid).

Race sweep (`--event checkpoint:N --bot --headless --frames 7000`, 12 rows
per city, mesh vs bound, frames with a prop contact and vertical speed
above 2.5 m/s, retail 2026-10-14): SF `checkpoint:2` (Golden Hour) 16 → 0
(peak 7.8 → 1.7 m/s, the Marin lip below); London `checkpoint:4` 12 → 0
(`cl_towerbrdge01_rt_l`, peak 6.7 → 0). Worse: London `checkpoint:3`/`10`
0 → 3/2 (Lambeth bridge, peak 3.8 m/s) and `checkpoint:6` 1 → 3 (Waterloo
bridge, peak 4.4 m/s) — the bound's abutment wall tops (y −0.15) meet the
drawbridge leaf deck; filed as #1450. SF `checkpoint:11` is the same in
both (`fl_embarcramp01_f`, 16 vs 15 frames, peak 3.7 m/s). The other 20
rows have no prop launch either way.

Residual, authored (#1451): the Marin side bound's land apron
(`pl_ggbridge_marin_side_f` grass triangles) is flat at y = 30.40. Probed
with downward rays at the three lane x's, every 2 m: the PSDL road is
30.25 up to z -2086, then climbs ~0.015 m/m (30.28 at -2082, 30.40 at
-2074, 30.62 at -2060), so the apron stands 0.09–0.15 m proud of it only
over z -2084..-2080 (~4 m) and the road is above it from z -2078 on; past
the lip the bound drops away (29.6 → 25 m). The render mesh's triangles
dip to 27.8 there instead, so the bound is not a copy of the mesh. The
retail data has nothing that makes the road win: both are authored, the
lip is the bound's own height, and the original's prop-versus-road
precedence is unknown. Left as authored — no offset, no clipping; a car at
12 m/s hops ~0.6 m over a 0.15 m step. `tests/golden_gate.rs`
(`the_marin_end_apron_is_no_more_than_its_authored_lip_over_the_road`)
pins the lip at ≤ 0.15 m and that nothing else stands in the lanes.

## Bridge ends: the bound is flush with the closed leaf (#1450)

#1443's London sweep showed 3–4.4 m/s hops at the Lambeth
(`wl_lambethbrdge_rt_l`) and Waterloo (`cl_waterloobrdge_rt_l`) bridge
ends, and put them down to the bound's abutment wall tops (y −0.15)
meeting the leaf deck. Measured here, that premise does not hold:

- The only bound geometry near the Lambeth hinge is one vertical quad,
  (539.96, 644.15)–(547.38, 621.32), top edge y −0.15, bottom −2.26 —
  the quay face. The leaf hinge (543.67, −0.3, 632.73) sits at its
  midpoint, and the closed leaf's deck is −0.3 + 0.15 = −0.15
  (`drawbridge.md` § Pose). Wall top, deck and the PSDL road at the
  approach (`city-ground-m6-flat`, −0.15) are the same height; the
  wall is flush, not a ridge. The render mesh's lower wall top (−0.42) is
  a mesh-vs-bound difference, not evidence of a deck above the bound.
- `tests/bridge_ends.rs` casts downward rays over every closed London leaf
  pair (axis ±9 m, 6 m onto each approach) and finds no prop collider
  more than 0.2 m above the deck. It passes with the bound and with the
  render mesh, so it guards the flush reading rather than selecting
  between them.
- A per-tick trace of `--event checkpoint:3 --bot --headless` (player
  position, velocity, Avian contacts) puts the hop at 57 s of play: the
  timed leaf (10 s closed, 9.4 s opening at 0.05 rad/s, 38.8 s cycle) is
  ~24° up and the car is climbing it. `vy` is +2.1 m/s on first leaf
  contact and 3.5 → 4.5 m/s while the car is on the raised deck; the wall
  quad shows up as a contact for three frames (+1 m/s). With the Lambeth
  bound swapped for the render mesh the same run hops harder (7.1 m/s at
  22 m/s), so the bound is not the cause: the car drives up a rising leaf.

Nothing was changed in the collision. Open (unknown original semantics):
whether the original's leaves are solid ramps while rising, and whether
its bots time their approach to a closed leaf.

