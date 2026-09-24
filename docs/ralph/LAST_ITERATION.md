# Last iteration — F15-B.10: representative avoidance matrix + occupancy-aware re-anchoring

Task slice on `ralph/night` (baseline `5275afb`; the F15-B.9 external
review passed with no blocking findings). Selected the F15-B remainder's
last non-research-gated item: the spec's edge-case row — "faster car
behind a bus; route junction miss; persistent player obstruction;
overturned opponent; shortcut invalidation; competing recovery
positions." Auditing the suite first: persistent obstruction
(`blocked_route_drives_around_the_parked_car`, `authored_avoid_*`) and
shortcut invalidation (`Teleported` breaks the swept segment — penned/
dispatch tests) were already covered; three scenarios were not, and
"competing recovery positions" was a real gap — `reanchor_pose`'s
`blocked` checked only un-cleared gates, so two cars whose stuck windows
expire on the same frame teleported onto the same projected spot.

## What changed

`crates/mm2_app/src/opponents.rs`:

- `REANCHOR_CLEAR` (8 m XZ) + `REANCHOR_CLEAR_Y` (4 m band): the
  re-anchor `blocked` closure now also rejects candidates within the
  clearance of *any* participant — self included, so a car beached on
  the line does not teleport back onto itself — and of poses another
  re-anchor already claimed this frame (`claimed: Vec<Vec3>` covers the
  stale frame-start `traffic` snapshot). The occupancy walk consumes the
  same `REANCHOR_WALK` 60 m budget — a route jammed solid still lands
  bounded (disclosed). Positional, not avoidance: applies whatever the
  authored avoid flags say.

`crates/mm2_app/tests/opponents.rs` (+4, 40 total):

- `a_faster_car_passes_a_slower_moving_blocker` — a roster slot on the
  same lane staged ahead at authored `maxThrottle` 0.15 vs 1.0: the
  position order swaps on track, the pass leaves the lane line
  (`max_dev > 0.5`), the bus is never sustained-shoved (≤2 impacts) and
  is still `Racing` when the follower finishes.
- `an_overturned_opponent_self_rights_and_resumes` — roof-down flip
  during the countdown → the authored `self_right_delay` flops it
  upright in place well inside the re-anchor window (`reanchors == 0`,
  no gate banked), then a real finish through swept triggers.
- `a_junction_miss_rejoins_the_route` — teleport past a corner on the
  old line's extension → bounded rejoin (the `point_reached` plane rule
  marks the missed anchor — chase forward — or the chase bends back;
  either is a rejoin) → finish earned through the post-leg gates.
- `competing_reanchors_land_clear_of_each_other` — two penned cars,
  same-x projections onto colinear routes: both re-anchor, landings are
  upright, on the line, ≥ `REANCHOR_CLEAR` apart, and bank nothing.

## Verification

- Tests: `cargo test --locked --workspace` — 67 suites green
  (`tests/opponents.rs` 36→40).
- Gates: `cargo fmt --all -- --check` clean; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` clean.
- Retail (`fnv1a64:e91e6cd4b2ae30d9`, `sf circuit:0 --bot --headless
  --frames 9000`): `status=pass cp=4/9 lap=2/3 pos=1/5 opp=0/4
  opp_rec=5 cu=4` — the F15-B.7/9 progress envelope holds; re-anchors
  still bounded and disclosed per-slot
  (`opps=0:vpbug/6c/1l/15e/2r/900w,…`).

## Not done / open

- F15-B parent's remaining items are now research-gated only:
  `unkFlag`/`cornerBrakingThreshold`/`weirdPathfinding` consumption
  (runtime semantics unverified, UNK-11) and original-runtime semantics
  of the consumed fields (designed readings meanwhile).
- Newly named gap: the scripted player's re-anchor (`scripted.rs`)
  keeps gate + ground-probe rejection only — participant occupancy is
  not checked there (evidence-driver scope; worth a follow-up slice).
- Whether the original rubber-bands at all stays unverified — the
  bounded assist is a designed policy (DSN-27), disclosed in records.
- GPU/rendered output not exercised — headless physics evidence only.
