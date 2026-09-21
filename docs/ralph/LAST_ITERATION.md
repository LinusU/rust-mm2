# Last implementation iteration

- Task ID and title: F10-A.2 — runtime ambient traffic: the seeded
  spawn plan's consumer. External review #12 passed F10-A.1 and noted
  nothing consumes `plan_ambient`; this slice is the runtime leg the
  plan recorded as remaining.
- Starting commit: `8051e2eb890ea061e8d275d072876895bf8c039d` on
  `ralph/night`; tree was clean.

## What changed

- `mm2_game::traffic` — the plan's inline lane pick refactored into
  `eligible_lanes` (routable arc, finite length/vertices, not closed)
  and `draw_spawn` (one deterministic class+position draw returning
  `Placed`/`InsideBubble`/`Unspawnable`) so the runtime respawner runs
  the same logic the planner used. New `LaneCursor { lane, along }`
  (travel-direction distance) plus `advance_lane_cursor`: walks the
  lane, then at its end picks a seeded legal exit through
  `NavGraph::transfer_lane` (new — `advance_cursor`'s turn math
  extracted), preserving lane rank and skipping closed destination
  roads; `DeadEnd` is an explicit result for runtime despawn.
- `mm2_content` — `opponents::event_aimap` extracted (difficulty-
  selected aimap with the same cross-fallback `opponent_roster` used;
  `EventAimap` records which variant won). `traffic::ambient_setup`
  merges city + event aimap layers: a non-empty event roster replaces
  the city's, `NavOverrides` merge (closed roads unioned, event
  exceptions first so event speed limits win), event `[Speed Limit]`/
  left-driving override the city's. `assemble::ambient_vehicle` loads
  `geometry/<id>.pkg` + `.mtx` + optional `bound/<id>_bound.bnd` —
  `aivehicledata` is deliberately not converted to a `VehicleConfig`
  (it authors no drivetrain).
- `mm2_app::traffic` (new) — `AmbientTraffic` resource (graph,
  merged overrides, roster, eligible lanes, a second `NavRng` stream
  for runtime draws, counters) and `AmbientCar` component.
  `load_ambient_traffic` runs `plan_ambient` and spawns session-owned
  kinematic rigid bodies with bound-convex-hull (or authored `Size`
  box fallback) colliders and the real `va_*` model. `drive_ambient`
  (FixedLast, after the solver) walks each cursor, re-poses from the
  sampled lane, refreshes per-road effective speed on turns and
  despawns dead ends. `maintain_ambient` despawns cars outside the
  recycle bubble and respawns through `draw_spawn` to the density
  target, bounded per tick by `placement_attempts`.
- `session::load_session_world` — `EventSetup` gained the parsed
  event aimap; ambient load runs after the player spawns (its pose is
  the bubble centre); teardown removes the resource. `main.rs` and
  `smoke.rs` register `drive_ambient`/`maintain_ambient` in the
  `FixedLast` chain after `advance_race`; the headless record gained
  `traf={active}/{target} sp=… rec=… dead=… uns=…` — absent on worlds
  with no roster so older records stay bit-identical.

## Evidence

- `cargo test -p mm2_game --test traffic` — 13 pass, incl. new
  `cursor_advances_then_turns_then_dead_ends`,
  `cursor_faces_the_authored_travel_direction`,
  `cursor_never_enters_a_closed_road`,
  `cursor_keeps_its_lane_rank_across_a_turn`.
- `cargo test -p mm2_app --test traffic` — 4 pass on a synthetic
  install (CAI1 two-road chain + PSDL + aimap + `va_*` fixtures)
  through the real `load_session_world`: seeded placement on authored
  lanes outside the bubble and identical replay under the same seed;
  lane-following advances survivors, dead ends despawn and the
  recycler refills to target; an event aimap `[Density] 0.0` authors
  the population off over the city's `0.25` (AC06's consumption leg);
  teardown removes the resource and a restart replans.
- Retail headless smoke:
  `mm2 --mm2-path <retail> --city sf --headless --frames 600` →
  `status=pass … traf=16/16 sp=140 rec=124 dead=0 uns=0` — 1212
  eligible lanes, density 0.5 → target 16, seeded plan placed 16/16,
  the recycler churned distant cars back to the bubble over 10 s
  (`sp=140` counts respawns; `dead=0` on a live graph is expected —
  cars recycle by distance before running out of road).
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --workspace --all-targets --all-features -D warnings`
  — PASS.
- `cargo test --workspace` — PASS, 49 suites, 0 failures.

## Still open

- F10-AC02/AC03 remain unmet: no intersection controller, signals,
  right-of-way, queueing, obstruction response, stuck recovery or
  collision-response fidelity. Ambient cars are kinematic lane
  followers — their hull blocks the player, nothing more.
- Spawn-vs-spawn overlap is still unchecked (F10-AC04's "reject
  occupied space" leg) — `draw_spawn` only enforces the player bubble.
- The recycle policy churns on a large city (140 spawns / 10 s on sf):
  `draw_spawn` places anywhere outside the 60 m bubble including
  beyond the 400 m recycle radius, so far placements recycle
  immediately. Bounded by `placement_attempts` but wasteful — a
  candidate tighten-up (draw inside the recycle annulus) for a later
  slice.
- `SpawnPolicy` bounds and the density precedence chain
  (event aimap → authored table dial → city aimap → config) are
  designed values (UNK-12) — the original layering is unverified.
- `maintain_ambient` reads the first `PlayerVehicle` position —
  remote-player bubbles and per-player populations are F10-B+ scope.
- Remaining non-blocking review notes not taken this round:
  `TrafficAudit::discovered()` counts rostered-but-unresolved ids;
  the two thin city-aimap wrappers could share a helper.
