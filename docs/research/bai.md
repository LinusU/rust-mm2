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
