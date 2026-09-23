# Last iteration — F00-C.2: authored-floor below-world check in the smoke runner

Repair iteration on `ralph/night` (baseline d7dc8df, external review of
F18-A.7 passed). Selected a defect the previous iteration's evidence run
surfaced, ahead of the queued feature slices per the repair-first
selection policy.

## The defect

`headless_smoke`'s below-world verdict compared the end-of-run pose to
`spawn_pos.y - 25`. That line is wrong on any session that legitimately
descends: retail `sf circuit:0 --bot --headless --frames 6000` failed
`fell through the world` while the car was racing normally — `cp=2/9`,
`wheels=4/4`, `final=(-2107,17.1,-52)` — because the authored course
bottoms at y=15.85, ~28 m under the ~43.5 start grid. London's subway
reaches −22 under street level, the same trap. The false verdict masked
the run's actual outcome (a scripted-driver stall) and would mask any
genuine race evidence on descending courses.

## What changed

- `mm2_app::city`: new `WorldFloor(pub f32)` — the session-scoped
  authored floor, `Psdl::bounds_min.y` when finite; absent on dev worlds
  and non-finite bounds. `LoadedCity` carries it as `floor`.
- `mm2_app::session`: `load_session_world` inserts `WorldFloor` after a
  city load; `despawn_session_entities` removes it on teardown (same
  lifecycle as `CityPvs`/`CityWater`).
- `mm2_app::smoke`: the verdict is `p.y < floor - 25` when a
  `WorldFloor` is bound, `spawn_pos.y - 25` otherwise. SF's authored
  −2.15 yields a −27.15 line; the failing endpoint at y=17.1 is 44 m
  above it. Implementation choice — an evidence-runner threshold, not a
  claimed original rule; the finite-pose and never-grounded checks are
  unchanged.

## Tests

+2 in `crates/mm2_app`:

- `tests/smoke.rs::descending_below_spawn_is_not_a_world_fall` — a
  synthetic one-room city (road at y=0, authored bounds to −60) with a
  dev spawn at y=40: the car drops, grounds, drives off the edge into a
  recovery loop and ends `final=(-0,-0.1,-2)`, ~40 m under spawn — pass
  under the authored floor, fail under the old spawn-relative line.
- `tests/session.rs::world_floor_does_not_leak_across_restart` — a
  planted `WorldFloor` does not survive `Backspace` teardown into a
  dev-world session (the smoke then falls back to spawn-relative).

## Verification (this tree)

- `cargo test -p mm2_app --test smoke --test session` — 19/19 pass.
- Retail (`fnv1a64:e91e6cd4b2ae30d9`):
  `mm2 --mm2-path <retail> --city sf --event circuit:0 --bot --headless
  --frames 6000` → `status=pass updates=6000 race=Running cp=2/9
  lap=1/3 pos=4/5 opp=0/4 opp_rec=5 final=(-2107,17.1,-52)` (was
  `status=fail … fell through the world` at the same endpoint).
- Gates (2026-09-23, this tree): `cargo fmt --all -- --check` clean;
  `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` clean; `cargo test --locked --workspace` — 67 suites, 0
  failures.

## Not done / open

- The `status=pass` is a physics/session-integrity verdict, not a
  circuit-completion claim: the scripted player still stalls between
  gates 1→2 — it leaves the elevated road on the descent (gate 2 at
  y=37.8, car ends at y=17.1 ~155 m past it in x) and loops through 7
  fall/recovery cycles at `cp=2/9` after 6000 frames. Route-following
  on descending/elevated courses is the real F14-B/F15-B remainder this
  repair exposes; `opp=0/4 opp_rec=5` shows the authored opponent field
  running with 5 disclosed re-anchors but no finishers at the cap.
- F15-B.4's record of `sf checkpoint:0` failing `fell through the
  world` was the same defect class — expected to pass on re-run; not
  re-measured this iteration.
- The 25 m margin below the authored floor is an implementation choice;
  no original-game analog is claimed.
