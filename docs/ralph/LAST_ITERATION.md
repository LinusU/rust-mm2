# Last implementation iteration

- Task ID and title: F10-B.1 — ambient-traffic obstruction response
  (forward corridor + bounded follow/stop), plus the F10-A.2 external
  review's same-subsystem non-blocking repairs. The review passed
  F10-A.2 with no blocking findings; this slice takes the highest-value
  F10-B leg and folds in its four noted subsystem gaps.
- Starting commit: `a035b38c33b66700239b0f7e5da221654aadba62` on
  `ralph/night`; tree was clean.

## What changed

- `mm2_game::traffic` — `draw_spawn` now takes `&SpawnPolicy` and
  places inside the `[min_player_distance, recycle_distance]` annulus:
  `SpawnDraw::InsideBubble` became `OutOfBand` covering both bounds, so
  a draw can never land past the radius the recycler would collect it
  at immediately. New `FollowPolicy` (designed values — the original
  braking model is unverified, UNK-12), `corridor_gap` (nearest blocker
  inside a forward corridor: XZ-projected heading, lateral
  half-width, vertical tolerance, speed-scaled reach) and
  `follow_speed` (road limit on a clear corridor, desired speed capped
  so the car rolls up to `follow_gap`, instant stop inside
  `panic_gap`, `turn_speed` cap across intersections).
- `mm2_app::traffic` — `drive_ambient` builds a blocker list from
  every `Player` participant (local driver and AI opponents alike)
  plus every other ambient car, senses the corridor per car, applies
  `follow_speed`, and counts held cars into the new
  `AmbientTraffic::queued`. `maintain_ambient` now runs under the same
  `Countdown|Playing` phase gate as the driver — a paused session no
  longer recycles/respawns behind the overlay. `AmbientTraffic::issues`
  are warn-logged at load instead of counted silently.
- `mm2_app::traffic` bug found by the new pause test — the kinematic
  velocity was derived from the measured position delta across the
  physics step; Avian integrates kinematic bodies from
  `LinearVelocity`, so the delta fed back on itself and diverged (cars
  reached ~km/s once a pause stopped the position overwrite).
  `LinearVelocity` now carries the intended surface velocity
  `tangent * speed` — contacts resolve against a real velocity.
- `mm2_content::opponents` — `opponent_roster` split so
  `opponent_roster_from_aimap` builds the roster from an already
  resolved+parsed aimap; `event_race_setup` calls `event_aimap` once
  and shares the record between the roster and the ambient setup
  (the double parse the review noted is gone).
- `mm2_app::smoke` — the headless `traf=` record gained `q=` (queued
  cars held behind a blocker at the final tick).
- Tests — `mm2_game`: annulus bounds (`plan_never_spawns_beyond_the_
  recycle_radius`), corridor nearest-in-band selection, and the
  follow law's brake-to-gap/resume. `mm2_app`: a parked participant
  holds a follower at a bounded gap and clearing releases it
  (sparse `[Density] 0.0` install — the full-density fixture
  saturates its ~100 m of lanes so the corridor stays legitimately
  occupied), two followers queue behind a blocker reporting
  `queued >= 2`, and `maintain_ambient` freezes during `Paused`.
  The synthetic PSDL ground was widened to span the fixture's lanes
  and the player's quarantine spawn — the player previously fell
  through the world, which the annulus bound correctly exposed as
  population drain.

## Evidence

- `cargo test -p mm2_game --test traffic` — 16 pass.
- `cargo test -p mm2_app --test traffic` — 7 pass on the synthetic
  install through the real `load_session_world`.
- Retail headless smoke:
  `mm2 --mm2-path <retail> --city sf --headless --frames 600` →
  `status=pass … traf=16/16 sp=26 rec=10 dead=0 uns=0 q=0` — vs
  `sp=140 rec=124` before the annulus bound: the spawn-past-recycle
  churn is gone. `ambient traffic loaded density=0.5 target=16
  spawned=12 eligible=1212 issues=22` — the 22 issues (unroutable
  roads) now log as warnings; 4 initial draws landed out-of-band and
  the maintainer refilled to target. `q=0` at the final tick — no
  car held at the sample instant; the queued counter is exercised
  synthetically.
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets --all-features
  -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, 49 suites, 0 failures.

## Still open

- F10-AC02/AC03 remain unmet: no intersection controller, signals,
  right-of-way, stuck recovery or dynamic collision fidelity. The
  follow law is a local bounded brake — a kinematic follower stops
  behind a blocker and waits; it never passes, changes lane or
  recovers. Junction turns transfer onto the next lane without an
  occupied-space check, so cars can materialise inside a queue (they
  then hold safely — `along <= 0` blockers are skipped only for the
  car itself).
- Spawn-vs-spawn overlap is still unchecked (F10-AC04's "reject
  occupied space" leg) — `draw_spawn` enforces the annulus only.
- `FollowPolicy` constants, `SpawnPolicy` bounds and the density
  precedence chain are designed values (UNK-12) — original braking /
  follow behaviour is unverified.
- No rendered/GPU check of traffic on either city; london not run
  this iteration. A saturated tiny network legitimately queues —
  `q=` on retail sf read 0 at the sample tick, so the hold behaviour
  is proven synthetically only.
- `maintain_ambient` reads the first `PlayerVehicle` position —
  remote-player bubbles and per-player populations are F10-B+ scope.
