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
- `london_sup.bai`/`sf_sup.bai` share the `CAI1` magic but do not fit
  this layout under either field reading — reported `unsupported`; their
  record structure is uninvestigated.

## Confidence

- Container/road/intersection/culling layout: **documented + measured**
  byte-exact on five retail files.
- Lane-side block order: **measured** (doc order produces non-monotone
  lane distances and trailing edge values in the wrong place).
- `misc[40]` contents, `RoadEnd.unknown1`, the `fill0`/`END_FILL` fill
  values: **unknown**, preserved raw.
- Semantics (how the runtime consumes lanes, lights, bubbles, ambient
  types): **documented claims in R3**, runtime behavior unverified —
  see `docs/original-rules.md` UNK-12.

## Implementation

`mm2_formats::bai` — `Bai { roads, intersections, culling }` with typed
helpers (`ambient_type`, `vehicle_rule`, `is_connected`, `FLAG_*` /
`KNOWN_FLAGS`) and `Bai::validate()` reporting `BaiIssue` diagnostics.
`mm2-inspect bai <install>` audits every discovered `city/*.bai`,
cross-checks room refs against the same-stem PSDL, and `--strict` exits
nonzero on missing expected files or any issue.

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

`mm2_game::nav` turns `Bai` into an immutable `NavGraph`: directed
arcs, lane sampling, full-3D `nearest_lane` (bridge decks do not snap
to ground lanes through horizontal proximity; PSDL-room hints
disambiguate stacked geometry), legal exits per lane position,
seeded exit choice, bounded deterministic A* routing with specific
failure reasons, per-consumer `RouteCursor`s and a bounded
`reachable_arcs` census walk. `mm2_content::nav`
loads `city/<name>.bai` through the VFS and distils `city/<name>.aimap`
into `NavOverrides` (zero-density `[Exceptions]` close roads to ambient
routing; `[Speed Limit]` overrides base speeds). `mm2-inspect nav
<install>` audits the graph per city with `--route from:to` road-index
probes (honouring aimap closures), the `--turns` ccw-delta/geometry
reconciliation, `--aimap <logical>` to substitute an event aimap's
overrides, `--routes n` for the directed-reachability census plus `n`
seeded probes (chain-consistency and closed-road-traversal checks),
and `--strict`. The app's `--nav`/`--nav-route` flags
draw the graph over the imported city through gizmos (F09-AC04).
