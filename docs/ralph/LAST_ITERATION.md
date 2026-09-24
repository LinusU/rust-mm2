# Last iteration — F15-B.6: navigation-backed `.opp` route densification

Task slice on `ralph/night` (baseline `2070fb2`; previous external review
of the F15-B.5 doc-repair passed with no blocking findings). Selected the
highest-value ready work: the `sf circuit:0` p3→p4 descent wall F15-B.5
recorded as its open remainder — it stalled both the routed scripted
player (`cp=2/9`, 21 recoveries) and the AI field (`opp_rec=5`).

## Root cause — route fidelity, not physics

F15-B.5's record guessed "steep-slope/collider fidelity". Instrumented
probing disproved that:

- The `.opp` p3→p4 straight line crosses Telegraph Hill *through* two
  rows of bungalows: raycast profiles show a ~7 m step up onto roof
  terraces at y≈40.5/46.4 while the adjacent street runs at y≈33 and
  the bottom street at y≈18.9. A 33 m/s jump attempt still wedged on a
  prop wall — the line is undrivable at any speed.
- A direct physics probe drove the real street (BAI road 340, an
  L-shaped descent around the block) at ~40 m/s with no drama.
- Every retail `.opp` is sparse (8–20 anchors/lap, legs of 40–200 m) —
  the polyline is course *intent*, not a literal centreline.

So the fix derives the driven line from the authored road graph between
anchors rather than chasing the straight polyline.

## What changed

`crates/mm2_game/src/nav.rs`:

- `lane_hits()` — every eligible lane snap, nearest-first;
  `nearest_lane()` now picks its head.
- `route_candidates()` + shared `route_search()` — multi-entry A*:
  every lane within `ENTRY_TOLERANCE` (12 m) of the nearest snap seeds
  its arc at snap cost, so both directions of a carriageway compete
  without neighbouring roads pretending to be the endpoint.
  `forward_on_arc()` rejects a same-arc goal lying *behind* the start
  in travel direction. On the real leg the search resolves a single
  258 m `road 340 Backward` arc vs the old single-snap 699 m uphill
  detour.
- `route_path()` — samples routed arcs in travel direction via
  `transfer_lane` rank preservation + `crossing_path` junction chords.
- `densify_route()` — a leg leaving `ROUTE_CORRIDOR` (14 m) of every
  vehicle lane is re-pathed at `DENSIFY_STEP` (8 m); authored anchors
  stay verbatim (gate binding/checkpoint semantics unchanged); closed
  routes densify the wrap leg and re-emit the head anchor; unroutable
  legs keep the authored line rather than inventing geometry.

`crates/mm2_app/src/opponents.rs` / `session.rs`:

- `OpponentDriver.route` carries the derived driving line; `spec` stays
  authored verbatim. `driving_route()` applies `densify_route` with
  `RouteOptions::default` — the event aimap's `[Exceptions]` ambient
  closures are deliberately *not* consumed (they forbid ambient traffic
  on a road; they don't remove a road from a race course —
  implementation choice).
- `load_session_world` builds the city's `NavGraph` once per event
  (~2 ms measured on sf.bai) and densifies the bot's `ScriptedRoute`
  plus every opponent route; a failed/absent graph leaves routes
  verbatim.

## Verification

- Tests +7 (`crates/mm2_game/tests/nav.rs` 25→32): goalward-direction
  entry where single-snap `route()` dead-ends, stacked-level snap
  preference, lane-walk sampling incl. junction chords, corridor-leg
  verbatim, off-road re-path, closed-wrap re-emission, unroutable-leg
  verbatim. One test-constructor field added in
  `crates/mm2_app/tests/opponents.rs`.
- Retail (`fnv1a64:e91e6cd4b2ae30d9`, headless):
  `sf circuit:0 --bot --frames 9000` → `status=pass cp=4/9 lap=2/3
  pos=1/5 opp_rec=4` — the player completes lap 1 (pre-slice:
  `cp=2/9` + 21 recoveries wedged on the hill). An instrumented run
  under the candidate code showed all four opponents clearing gates
  (2/4/3/2) vs the old shared wall.
- London regression: `london circuit:0 --bot --frames 1500` →
  `status=pass cp=3/6 pos=5/8 opp_rec=1` — one gate behind the
  reviewer's pre-slice `cp=4/6 pos=1/8`; a densified-line pacing
  delta, not a stall. At 9000f one opponent finished (`opp=1/7`).
- Gates (this tree): `cargo fmt --all -- --check` clean;
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  clean; `cargo test --workspace` — 67 suites, 0 failures.

## Not done / open

- F15-B parent's remaining items stand: AC06 measured-difficulty leg,
  representative avoidance matrix, authored-tail columns
  (`weirdPathfinding`/`distancePadding`/`cornerBrakingThreshold`),
  `avoidOpponents` polarity.
- The scripted re-anchor disclosure gap noted by review stands:
  `p_rec=`-style field still absent (re-anchors are `info!` +
  `ScriptedRoute::reanchors` only).
- GPU/rendered output not exercised — headless physics evidence only.
- Whether retail opponents actually route on the BAI network between
  `.opp` anchors is unverified (plausible inference from the sparse
  data + authored road graph; the densified line is our implementation
  choice, recorded as such).
