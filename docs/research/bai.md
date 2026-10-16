# BAI ambient navigation

Per-city directed road network for ambient traffic and pedestrians
(`city/<name>.bai`, `CAI1` container). Three sections: roads (directed
strips with per-side lane/sidewalk/rail curves), intersections (a PSDL
room plus the connected roads, counterclockwise), and per-room "AI
bubble" culling lists naming the roads simulated while the player
occupies each room.

## Format (documented in angel-file-formats/Midtown Madness 2/BAI.md, verified on retail)

- `char[4]` `"CAI1"`, `u16` nIntersections, `u16` nRoads
- `roads[nRoads]`, `intersections[nIntersections]`, `culling`
- Road: `u16 id`, `u16 nSections`, `u16 flags`, `u16 nRooms` +
  `u16 rooms[n]` (room index + 1), `f32 halfWidth`, `f32 baseSpeed`,
  `RoadSide right`, `RoadSide left`, then **six parallel arrays** of
  `nSections` entries: `f32 distance`, `vec3 origin`, `vec3 xAxis`,
  `vec3 yAxis`, `vec3 zAxis`, `vec3 tangent`, then `RoadEnd end`,
  `RoadEnd start` (end is stored first).
- RoadSide: `u16 nLanes`, `u16 nTrams`, `u16 nTrains`, `u16 nSidewalks`,
  `u16 ambientTypes`, then — **measured order, differs from the doc** —
  `f32 laneDistances[nLanes+nSidewalks][nSections]` (cumulative distance
  along each curve), `f32 edgeDistances[nLanes+nSidewalks]` (outer-edge
  distance from road centre), `u8 misc[40]` (mostly `0xCD` fill; bytes
  4–8 hold a float matching `halfWidth` on retail — undocumented),
  `vec3 laneVertices[nLanes+nSidewalks][nSections]`,
  `vec3 tramVertices[nTrams][nSections]`,
  `vec3 trainVertices[nTrains][nSections]`,
  `vec3 sidewalkInner[nSections]`, `vec3 sidewalkOuter[nSections]`.
  The doc lists `edgeDistances` before the distance matrix; on retail the
  matrix is a per-curve monotone cumulative sequence and the trailing
  values are per-curve edge distances — verified by byte-exact parses of
  `london.bai`, `london_bak.bai`, `sf.bai`, `sf_bak.bai`, `sfai.bai`.
- RoadEnd: `u32 intersection`, `u16 0xCDCD fill`, `u16 vehicleRule`,
  `u16 unknown`, `u32 intersectionRoadIndex` (index into the named
  intersection's road list, or `0xCDCDCDCD` when unconnected),
  `vec3 trafficLightOrigin`, `vec3 trafficLightAxis`.
- Intersection: `u16 id`, `u16 room` (index + 1), `vec3 center`,
  `u16 nRoads`, `u32 roads[n]` counterclockwise.
- Culling: `u32 nRooms` (= PSDL room count + 1), then per room a large-
  bubble list and a small-bubble list of `u16` road indices; list `[0]`
  is empty on retail, matching the doc's "ignore the first list" note.

## Verified on retail (2026-09-20, `mm2-inspect bai`)

- `city/london.bai`: 540 roads, 328 intersections, culling 1342 rooms;
  all room refs within `city/london.psdl` (1341 rooms). `city/sf.bai`:
  379 roads, 214 intersections, culling 1172 rooms vs 1171-room PSDL.
- Ids are sequential with file order on retail, so intersection/road
  references read equally well as indices or ids; we treat them as
  indices and the ambiguity is harmless on stock data.
- Every connected end's `intersectionRoadIndex` points back at its own
  road (757/757 SF, 1077/1077 London), and every intersection road
  reference names a road that references the intersection back
  (758/758, 1080/1080) — `Bai::validate` checks both directions.
- Observed value ranges: 4–86 sections/road, ≤5 lane+sidewalk
  curves/side, ≤1 rail curve/side, `ambientTypes` ∈ {0,1,2,3},
  `vehicleRule` ∈ {0,1,3} (AlwaysStop=2 unused on retail), flags ∈
  {0,1,2,5,8,9,10} (documented bits only).
- `sfai.bai` (dev/test map) parses but has one authored anomaly:
  intersection 0 references room 0 — reported as an issue, not repaired.
- `vehicleRule` census on *approach* ends (arc exits into a junction,
  2026-09-21): SF authors 449 `TrafficLight` / 138 `NeverStop` / 30
  `StopSign` approaches across 212 junctions receiving vehicle arcs —
  82 uniformly lit, 38 uniformly free-flow, 92 mixed, none uniformly
  stop-signed. London authors 369 / 214 / 23 across 254 — 76 lit, 76
  free-flow, 102 mixed. Mixed-rule junctions are the norm, so each
  approach must gate on its own end's rule; `AlwaysStop` (=2) is
  authored nowhere.
- `trafficLightOrigin`/`trafficLightAxis` census (2026-09-22,
  `mm2-inspect bai`): nonzero origins on 650 connected ends in SF
  (533 light-ruled, 117 on other-rule ends) and 828 in London (519 /
  309), zero non-finite and zero on unconnected ends in either city.
  Origins sit at road level on the kerb/sidewalk beside the approach
  — the authored anchor the original's signal unit stands on.
  Y range −22.1..17.8 London, −0.2..109.8 SF; XZ distance to the
  authored junction centre ≤ 39.7 m on London while SF authors 3
  wild outliers (686.8 m max — treated as junk by the runtime's
  60 m sanity bound, counted not hidden). Only 522 SF / 440 London
  lit ends are the *downstream* end of a routable vehicle arc: 96 /
  166 sit on one-way upstream ends and 32 / 222 on roads with no
  vehicle arcs at all — so arc-only traversal misses roughly half of
  London's authored heads. `trafficLightAxis` is the cross-road unit
  vector described under *Traffic-signal units and the axis convention*
  below; the runtime preserves the pair verbatim, and the original's
  signal-unit *draw* (which variant, pole/arm placement) stays
  unverified; its state timing is now recovered — see *Junction signal
  timing and ambient population* below (UNK-12).
- `london_sup.bai`/`sf_sup.bai` share the `CAI1` magic but do not fit
  this layout under either field reading — reported `unsupported`; their
  record structure is uninvestigated.
- Lane vertex heights are **not** the PSDL road surface (measured
  2026-10-03, `city/sf`: ambient cars posed on lane curves, ground
  ray-cast under them over ~36k car-ticks on the hills around
  (−1150, 55, 370)). Mean offset is ~0, but the linear curve between
  sections sits from 1.87 m above to 1.42 m below the road over
  crests and dips, and 7.6 % of samples are more than 0.3 m off. The
  curves are good for routing and plan position; anything resting on
  the road has to take its height from the PSDL geometry
  (`mm2_app::traffic::drape_ambient` does).

## Confidence

- Container/road/intersection/culling layout: **documented + measured**
  byte-exact on five retail files.
- Lane-side block order: **measured** (doc order produces non-monotone
  lane distances and trailing edge values in the wrong place).
- `misc[40]` contents, `RoadEnd.unknown1`, the `fill0`/`END_FILL` fill
  values: **unknown**, preserved raw.
- Semantics (how the runtime consumes lanes, lights, bubbles, ambient
  types): **partly recovered** — signal phase timing, the `vehicleRule`
  gate and the per-room cull-list population rule are read from
  `Midtown2.exe` (see *Junction signal timing and ambient population*);
  the rest stays at R3's documented claims, runtime behavior unverified
  — see `docs/original-rules.md` UNK-12.

## Implementation

`mm2_formats::bai` — `Bai { roads, intersections, culling }` with typed
helpers (`ambient_type`, `vehicle_rule`, `is_connected`, `FLAG_*` /
`KNOWN_FLAGS`) and `Bai::validate()` reporting `BaiIssue` diagnostics.
`mm2-inspect bai <install>` audits every discovered `city/*.bai`,
cross-checks room refs against the same-stem PSDL, and `--strict` exits
nonzero on missing expected files or any issue.

## Tram curve storage (F28-A, measured 2026-10-08)

On all 21 retail `sf.bai` roads that carry trams (one curve per side),
the right-side tram curve sits at `+x` of the section frame and is stored
**end→start**, the left-side curve sits at `−x` and is stored
**start→end** — against the lane curves' "right with the sections"
convention above, though the lane curves' own per-curve storage order is
likewise an authoring artefact. `Bai::tram_curve` therefore orients a
tram curve by geometry (first vertex nearer the section the leg starts
from) rather than trusting side or storage order.

## Traffic-signal units and the axis convention (F10-B.7 evidence, 2026-10-12)

The authored `trafficLightOrigin`/`trafficLightAxis` pair was previously
read as an anchor plus an unexplained orientation. `mm2-inspect bai`
(`axis:` line) measures the convention on retail: the stored axis is
`trafficLightOrigin` **plus a horizontal unit vector across the
carriageway**.

- `‖axis − origin‖` is 1 within f32 noise (max |len − 1| 5.7e-5 London,
  8.9e-5 SF, 1.1e-4 on the dev `city/sfai.bai` — printed by the `axis:`
  line, so the number is re-measurable, not quoted) and `axis.y ==
  origin.y` on every end that carries a direction.
- The vector is within 15° of the end section's `x_axis` in either
  direction on **all 828** London ends with a nonzero origin and **646 of
  SF's 650**; four SF ends author `axis == origin`, a direction-less pair
  the census counts, not hides. Section `x_axis` is the cross-road
  direction (`tangent × up` on essentially every section, above), so the
  field is a cross-carriageway direction, not the head's facing (which is
  along the travel tangent).
- The **sign** is mixed, and the census reports the two signs apart
  (`axis:` line): `+x` 415 / `−x` 413 London, `+x` 328 / `−x` 318 SF — so
  a mixed sign is averaged away nowhere. It tracks neither the junction
  centre, the road centre nor the end, so which kerb holds the pole and
  which way the head faces stay unverified. The dev map `city/sfai.bai`
  authors 813 axes (813 unit, 811 horizontal, 796 within 15°, split
  407/389) — 17 off-convention, reported by the same census.
  `city/london_bak.bai`/`sf_bak.bai` match their live files.

**The signal-unit geometry ships**, contrary to the earlier "textures
only" reading. `Midtown2.exe` holds the two model-name strings
`sp_traflitsingle_f` (single) and `sp_traflitdual_f` (dual) twice
(0x5c503c/0x5c5050 and 0x5d79a8/0x5d79bc), beside the `[Traffic Lights]`
section string (0x5d7a9c, image base 0x400000) — read as that section's
single/dual defaults. Their one-line
aimap section `<single> <dual>` overrides them per map:
`city/london.aimap` sets both to `sp_traflitsingle_ped_l`, SF authors
none. Five `.pkg`s resolve through the VFS
(`geometry/sp_traflitsingle_{f,l,corner_l,ped_l}.pkg`,
`geometry/sp_traflitdual_f.pkg`), with bounds for the two defaults
(`bound/sp_traflit{single_f,dual_f}_BOUND.bnd`) and a
`tune/banger/sp_traflit*.dgBangerData` record for each — the original
signal heads are breakable props. Their PKG chunks carry the lamp states
as authored geometry, not a recoloured material:
`{RED,YELLOW,GREEN}GLOW{DAY,NIGHT}_H` on all five and
`{WALK,NOWALK}_{DAY,NIGHT}_H` on `sp_traflitsingle_f`,
`sp_traflitsingle_ped_l` and `sp_traflitdual_f` (the `_l` and
`_corner_l` singles ship no pedestrian states). `mm2_app::traffic` draws
that geometry at each origin (pole base on the origin, DAY lamp chunks
toggled per aspect); which variant each head gets and which way it faces
remain unrecovered, so the runtime picks single/dual by approach-lane
count and faces the lamps along the road away from the junction
(designed). The four `sp_traflit*` rows in `city/phys/props.pathset` (a
dev set no session loads) are a demo layout — the nearest SF authored
origin is 33 m away, the farthest 56 m (horizontal) — not the placement
source.

**No junction or spawn timing is authored.** Every BAI `RoadEnd.unknown1`
is 0 (1080 London, 758 SF — the `ends:` line), the intersection record is
only `id`/`room`/`centre`/counterclockwise road list, and no `.aimap`
section holds a phase or interval; the culling lists select *which* roads
are simulated per room, not *when* (they are the population rule — see
*Junction signal timing and ambient population* below). The original's
phase length, all-red clearance, stop dwell and population rule are
executable-only; they are now recovered from `Midtown2.exe` and written
up in the next section (UNK-12).

## Junction signal timing and ambient population (F10-SIGNAL-TIMING, recovered 2026-10-15)

Neither timing nor population distance is authored anywhere, so these
values exist only in `Midtown2.exe`. They were read from the disassembly
(image base `0x400000`, x86-32 PE32) and the Kuna v1.833 decompiler;
every constant below is cited by address and re-readable with
`llvm-objdump -d -M intel --start-address=<addr> Midtown2.exe`.

### The signal controller

A traffic-light controller is built per junction by `0x53c020` (vtable
`0x5b4528`), reset by `0x53c440`, and ticked by `0x53c490` (vtable slot
`+0x08`). Its fields:

| Field | Meaning |
| --- | --- |
| `+0x18` | pointer array of member signal (lamp) objects |
| `+0x1c` | the junction's intersection/road-list record |
| `+0x22` u16 | index of the member currently holding the phase |
| `+0x24` u16 | member (lamp) count |
| `+0x26` u16 | control type (0/1/2 — selected once, below) |
| `+0x28` u16 | all-red clearance in progress |
| `+0x2c` f32 | phase length (s) |
| `+0x30` f32 | phase timer (s) |

A lamp object's state word is at `lamp+0x3c`: **3 = green**, **2 =
amber**, **1 = red**, **4/5 = the all-red clearance** (5 during its
second half). Recovered timings (`verified_original`):

- **Phase = 6.0 s.** `0x53c04f` stores the literal `0x40c00000` into
  `+0x2c`. The tick adds a fixed **1/60 s** step (`0x5cd820` =
  `0x3c888889`) to the timer (`0x53c49c`, `0x53c4a3`) and advances when
  the timer passes the phase length, so one phase is 360 ticks.
- **Amber = 3.0 s.** During the phase's last 3 s — the test subtracts
  `dat_5af4e8 = 3.0f` from the phase length at `0x53c61c` — the holding
  lamp switches to amber (`lamp+0x3c = 2`, `0x53c640`).
- **All-red clearance = one whole phase, 6.0 s**, for control types 1
  and 2 only. When the member list wraps, the timer is reset to 0, the
  `+0x28` flag is set and every lamp goes to state 4 (`0x53c4f4`–
  `0x53c53f`); in its last 3 s (`0x53c689`–`0x53c69d`) they go to state
  5; at the next phase boundary the update calls the reset method
  (vtable slot `+0x0c`, `0x53c440`). A type-0 junction wraps straight to
  the next member with no all-red (`0x53c517`).
- The control type is chosen once in `0x53c160`: **type 2** when the
  junction has four roads and the helpers `0x549d70()` and `0x549d40()`
  both return 4; **type 1** when the lamp-member count equals
  `0x549d40()`'s count; otherwise **type 0**. `0x549d40` counts roads
  whose reverse end-ref (`+0x118`) points back at the junction (or whose
  `+0x38` is set), `0x549d70` counts roads carrying a flag bit — the
  exact flag is `inferred`.

The `vehicleRule` code is decoded through the jump table at `0x540b54`
inside `0x540a00`: **0 = StopSign** (`0x540aa3`), **1 = TrafficLight**
(`0x540a64`), **2 = AlwaysStop** (`0x540b27`, which prints an
"ERROR: Yield - Unsuported intersection control type" string — authored
nowhere on retail),
**3 = NeverStop** (`0x540a17`, returns "may proceed" unconditionally).
The green test (`0x540a64`–`0x540aa0`) reads the approach's member index
(`+0x11c` / `+0x140`), then the junction's controller
(`[end+0x118]+0x20`), its lamp array (`+0x18`) and admits the approach
iff the member's lamp state word is exactly 3. `specials.md` already
records this same gate for the cable car.

**A stop sign has no dwell timer.** `0x540aa3` requires the car nearly
stopped — speed `< 0.5` (`dat_5af360 = 0x3f000000`) — and within `1.5 m`
of the line (`dat_5b017c = 0x3fc00000`) before it may enqueue on a
per-road-end first-come-first-served queue (`0x54a3a0` enqueues,
`0x54a4c0` picks the head, called again each tick while waiting). A car
that reaches the line without meeting *both* tests is simply told "no"
that tick; the "dwell" the docs describe is the wait for the queue head,
not a fixed timer. The head is the vehicle with the least per-vehicle
value from vtable `+0x48` (an arrival/fairness value — the accessor is
not pinned).

### Ambient population: bubbles, not rings

There is **no spawn/despawn distance ring**. The AI map manager keeps two
per-room cull-list tables — a large bubble at `+0x174` and a small one at
`+0x178` — each an array of `{ u16 count; u16 *roads }` entries indexed
by room; `0x543dc0` allocates an entry's road list. On a player's room
change the manager calls `0x539db0(old_room, new_room, player)`, which
diffs the two rooms' lists and, per road, despawns cars that left the
bubble (`0x547a30` tests, `0x547be0` pops) and spawns cars on roads that
entered it. So the original's simulated set is exactly the authored
per-room cull list this port already imports (the BAI bubble), not a
radius around the player. Two placement constants sit inside `0x539db0`:

- **Minimum separation = 50 m.** A candidate position is rejected while
  any existing ambient-vehicle body is closer than `sqrt(dat_5aff6c)`;
  the compared quantity is full 3-D squared distance and
  `dat_5aff6c = 2500.0f` (`0x53a2f5`, `0x53a4cf`).
- **Density scale = 0.2.** The map's ambient density is clamped to
  `[0, 1]` (`dat_5af40c = 0.0f`, `dat_5af364 = 1.0f`) and multiplied by
  `dat_5af618 = 0.2f` into the manager field `+0x3c` (`0x53551d`).

The spawn loop also uses `dat_5af418 = 5.0f` (a spacing term) and a
`10.0f` sine amplitude (`dat_5af428`) when it turns "distance into the
road" into a spawn position, and walks the ambient-vehicle array at
`+0x68`/`+0x6c` with a stride of **0x9794 bytes** (`0x25e5` dwords).

### What remains unknown

- Whether the two cull bubbles are the *only* population rule: the room
  diff explains add/remove on a room change, but `0x539db0` also runs
  from non-room-change paths (`0x53b6a4`/`0x53b74f`), so there is a
  second trigger not yet identified.
- The `[Ambient Types/Density]` flag/weight semantics and the pool size
  (WLD-20) — this code sizes the pool but the flag is not decoded.
- The stop-sign queue head's per-vehicle value (vtable `+0x48`).
- Whether an amber/all-red lamp is shown via the state word alone or by
  swapping the authored lamp geometry (the PKG lamp states above).
- The port's `JunctionPolicy` stays a set of designed values; where they
  now coincide with the original (`green_ticks` = 6 s) is deliberate,
  and where they differ (`clear_ticks` = 1 s vs. a full 6 s phase, no
  amber, a 0.75 s stop dwell vs. none, a 2.5 m stop inset vs. 1.5 m) is
  a port choice still to reconcile — filed as a follow-up task.

## Lane direction and the navigation graph (F09-B.1)

Measured on `city/sf.bai` and `city/london.bai` (2026-09-21):

- Section frame `x_axis` equals `tangent × up` on essentially every
  section (SF 1719/1723 agree, 0 opposite; London 2504/2508, 0
  opposite; remainder are degenerate frames), so authored `+x` is the
  geometric right of a vehicle travelling with the sections.
- Right-side lane curves sit at `+x` (SF 1147 +x vs 66 −x; London
  1184 vs 123) and left-side curves at `−x` (SF 1202 −x vs 398 +x;
  London 1322 vs 548). The minority cases are real authored geometry,
  so lane *position* — not side slot — decides driver-relative rank.
- The Angel Studios GDMag article (Joe Adzima, "Ambient Traffic
  AI in Midtown Madness 2") documents that London's left-hand driving
  is baked into the BAI by the toolchain — right/left lane data
  swapped, vertex order and lane order reversed — so runtime direction
  logic is uniform: right-side curves travel with the sections,
  left-side curves against them. No per-city handedness flag.
- The same article documents the lane-position turn rules: far-left
  lane may turn left or go straight, far-right may turn right or go
  straight, middle lanes go straight, one-way roads may choose any
  outgoing road, freeway ramps can force a right, U-turns never.
  Intersection road lists are authored counterclockwise and the
  original selects exits by index arithmetic — `mm2_game::nav`
  carries that `ccw_delta` and classifies turns geometrically
  (heading change). `mm2-inspect nav --turns` reconciles the two on
  retail (2026-09-20): at 4-ways Δccw=1→right / Δccw=2→straight /
  Δccw=3→left holds for 471/492 London exits (95.7%) and 963/971 SF
  exits (99.2%) — the index scheme and measured geometry agree, so
  the documented convention is consistent with the authored data.
  Other arities show no clean mapping (3-way deltas mix all three
  kinds), matching the note that the scheme is only documented for
  4-ways. The original's *runtime* consumption stays unverified
  (UNK-12).
- `edgeDistances` per curve are **not** a lane ordering: profiles like
  `[7.5, 2.5, 2.5, 7.5]` occur on one-way left sides. Their exact
  meaning stays unknown (UNK-19); lane ranking uses the measured
  lateral offset instead. Sidewalk curves do reliably come last in the
  shared `laneVertices`/`edgeDistances` arrays.
- Built graphs: London = 540 roads (166 one-way) → 606 directed arcs,
  1141 routable vehicle + 1080 sidewalk + 28 rail lanes, 328
  intersections, 0 dead ends, 1 connected component. SF = 379 roads
  (96 one-way) → 618 arcs, 1212 + 758 + 42 lanes, 214 intersections,
  1 dead end, 1 component. 176 roads carry no routable vehicle lanes
  (pedestrian/special/disabled or curve-less) — reported, not
  repaired.
- Directed reachability census (`NavGraph::reachable_arcs`,
  `mm2-inspect nav --routes`, 2026-09-20): London's vehicle graph is
  strongly connected — every arc reaches all 606. SF's is not: road
  0's backward arc ends at the authored dead end, and six more arcs
  reach junctions no other arm legally departs (one-way traps —
  roads 92/94/97/99 reach only themselves, 102/111 reach 2, road 1
  backward reaches 617 of 618). 4927 ordered arc pairs (~1.3%) are
  unreachable; 505/512 seeded `route_roads` probes succeed and the 7
  failures are exactly the trap sources — honest authored
  constraints, not invented connectivity (0 chain violations).
  Event `[Exceptions]` closures bind at route time:
  `race/london/blitz0.aimap` (8 closed roads) raises unreachable
  pairs to 15016 and `race/sf/blitz0.aimap` (3) to 7977, with named
  probes (e.g. london `270→108`: 19 steps open → `Unreachable`
  closed) and zero closed-road traversals.
- Per-curve vertex order is **not uniform** (measured 2026-09-24 on
  `city/sf.bai`): individual lane curves are authored vertex-reversed
  relative to their road's section order, with mixed orders inside a
  single side — e.g. road 113's right side stores lanes 0–2
  end→start while lane 3 and the whole left side run start→end. The
  reversed curves sit at ordinary in-carriageway `+x` offsets beside
  their unreversed siblings, so storage order is an authoring
  artefact, not a direction or carriageway marker; `lateral_offset`
  (zipped to sections) lands at sane values only under the flip.
  `NavGraph::build` normalizes every curve to section order
  (`orient_curve`: flip only when endpoints sit clearly closer to
  the swapped section extremities; the authored distance row is
  reversed and complemented to match). Without the flip, a reversed
  lane's travel-direction *end* lies a full road length from its
  exit junction — the raw `nav --gaps` census reported median
  transfer chords of 92 m (max 1643 m) on SF; after normalization
  the medians are ~20–26 m (a real junction box) with max 87 m.
  London is not exempt: `city/london.bai` road 496's right lane 0
  stores its 16 vertices end→start against the road's 16 sections
  (measured 2026-10-16), so reading the raw order while applying
  section tangents bends that riverside lane out off the bank and
  over the Thames — into the quay wall between it and the water.

`mm2_game::nav` turns `Bai` into an immutable `NavGraph`: directed
arcs, lane sampling, full-3D `nearest_lane` (bridge decks do not snap
to ground lanes through horizontal proximity; PSDL-room hints
disambiguate stacked geometry), legal exits per lane position,
seeded exit choice, bounded deterministic A* routing with specific
failure reasons, per-consumer `RouteCursor`s, a bounded
`reachable_arcs` census walk, and `crossing_path` — a generated cubic
Hermite through each junction interior between an approach lane's
travel-direction end and the exit lane's start (implementation
choice, F10-B.9; BAI carries no authored junction-interior geometry,
so the original's runtime crossing path stays unverified — UNK-12).
`mm2_content::nav`
loads `city/<name>.bai` through the VFS and distils `city/<name>.aimap`
into `NavOverrides` (zero-density `[Exceptions]` close roads to ambient
routing; `[Speed Limit]` overrides base speeds). `mm2-inspect nav
<install>` audits the graph per city with `--route from:to` road-index
probes (honouring aimap closures), the `--turns` ccw-delta/geometry
reconciliation, `--aimap <logical>` to substitute an event aimap's
overrides, `--routes n` for the directed-reachability census plus `n`
seeded probes (chain-consistency and closed-road-traversal checks),
`--gaps` for the lane-transfer gap census (approach-end→exit-start
chords, generated-path lengths, endpoint-to-centre distances), and
`--strict`. The app's `--nav`/`--nav-route` flags
draw the graph over the imported city through gizmos (F09-AC04).
