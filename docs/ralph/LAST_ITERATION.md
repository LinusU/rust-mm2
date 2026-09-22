# Last implementation iteration

- Task ID and title: F10-B.4 — spawn occupied-space rejection
  (F10-AC04's spawn leg; spec req 2's "cleared lanes" and req 6's
  "avoid spawning cars inside the player"). Selected per the
  selection policy's F10-B remainder: `draw_spawn` only tested the
  player-distance annulus, so two planned directives could stack on
  one spot and a maintainer refill could materialise inside a live
  ambient car or a participant — opponents were never covered by the
  player bubble at all.
- Starting commit: `7e7ea4c17be1b87c5860e3678054a6c7f77698c4` on
  `ralph/night`; tree was clean, previous external review verdict
  pass (F10-B.3), so this is feature work, not a repair.

## What changed

- `mm2_game::traffic` — `SpawnPolicy` gains a designed
  tangent-aligned exclusion box: `spawn_clearance` 5 m longitudinal
  (about a car length — the materialising hull never overlaps),
  `spawn_half_width` 2 m lateral (a car in the neighbouring lane
  legitimately shares the road — an euclidean check would wrongly
  veto adjacent-lane spawns), `spawn_max_rise` 3 m (an overpass does
  not occupy). All designed values — the original's spawn-overlap
  behaviour is unverified (UNK-12). New `spawn_occupied` evaluates
  the box with the same tangent projection `corridor_gap` uses; a
  degenerate tangent occupies nothing.
- `draw_spawn` takes `occupied: &[[f32; 3]]` and returns the new
  `SpawnDraw::Occupied` when the sampled box touches a point —
  before the class pick, so a rejected spot never burns a roster
  draw.
- `plan_ambient` accumulates each placed directive's position into
  the occupied set, so two planned cars keep `spawn_clearance` on a
  shared lane; rejections exhaust `placement_attempts` and land in
  `dropped` like annulus misses.
- `mm2_app::traffic` — `maintain_ambient` builds the occupied set
  from ambient cars surviving the recycle test plus every `Player`
  participant (the same contract the obstruction sense reads —
  local driver and AI opponents alike) and feeds each same-tick
  placement into it, since Commands-deferred spawns are invisible
  to the query. `AmbientTraffic::policy` is `pub` for test/evidence
  binding, matching the `stuck_policy` precedent.

## Evidence

- `cargo test -p mm2_game --test traffic` — 25 pass (+4:
  `spawn_occupied_boxes_the_sample_tangent` — ahead/behind/lateral/
  overpass/degenerate legs; `draw_spawn_rejects_occupied_space` —
  fixture-wide box vetoes 8 draws, the same stream places under the
  default box; `plan_keeps_same_lane_spawns_clear_of_each_other`;
  `plan_drops_directives_past_a_lane_s_capacity` — a saturated lane
  drops its overflow into `dropped`).
- `cargo test -p mm2_app --test traffic` — 17 pass (+2:
  `respawns_reject_space_a_live_car_occupies` and
  `respawns_reject_space_a_participant_occupies` — each vetoes every
  refill under a fixture-wide box (`spawned` frozen over 120
  updates), then the default box refills again as the control that
  the respawner itself still works).
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets --all-features
  -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, 49 suites, 0 failures.
- Retail headless smoke (install `fnv1a64:e91e6cd4b2ae30d9`):
  - `--city sf --frames 600` → `status=pass … traf=16/16 sp=23
    rec=7 dead=0 uns=0 q=0 jq=5 stuck=0` — record unchanged.
  - `--city london --frames 600` → `status=pass … traf=16/16 sp=21
    rec=5 dead=0 uns=0 q=0 jq=3 stuck=0` — record unchanged.
  - The initial plan now drops saturated draws honestly: sf spawns
    12/16 at load, london 14/16 — the maintainer refills both to
    target, exactly the designed behaviour.
- One existing test adjusted for the new contract:
  `plan_skips_non_finite_lane_geometry` bound its target (32 on two
  100 m lanes) above the lanes' occupancy capacity; it now plans 12
  so the geometry-rejection leg — not spawn saturation — is what the
  test measures.

## Still open

- Exclusion-box constants are designed — original spawn-overlap
  behaviour unverified (UNK-12). Static props/geometry are not
  occupancy inputs (a spawn could still clip a prop).
- F10-AC04's remaining leg: "preserve active interactions near any
  player" is covered for the single local player by the bubble, but
  multiplayer union-of-interest bubbles are F10-C scope.
- F10-AC02 remainder: no yielding to crossing traffic inside the
  box; F10-AC03: kinematic followers stop short, no collision
  fidelity or lane-change passing. Signal-prop rendering,
  rendered/GPU junction checks, original signal/spawn timing all
  open.
