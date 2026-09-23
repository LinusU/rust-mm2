# Last iteration — F02-C.3: four-wheel physics rig + back-back follower wheels

Iteration 55 on `ralph/night`. Selected the last open F02-C functional
finding — `vpmoonrover`'s launch: weak acceleration, wheelie/porpoising
and ~131° heading drift recorded by the coverage matrix as an open
question between authored behaviour and a rig defect.

## Root cause

mm2hook's `vehCarSim::Init`/`vehCarModel::Draw` sources show the retail
simulator carries **exactly four `vehWheel` slots** (front-left/right,
back-left/right). `whl4`/`whl5` are "back-back" visuals: `Init` stores
`BackBack*WheelPosDiff = whlN pivot − GetWheel(N−2)->GetCenter()`, and
`Draw` renders them as the reference wheel's matrix plus that offset —
no force, no load share. We were simulating all six wheel parts as
independent corners; with the rover's authored `CenterOfGravity`
(+0.4 m rearward, behind the physics rear axle) that made a statically
indeterminate tripod that porpoised under drive torque.

## What changed

- `mm2_content::model::WheelVisual` gains `follows: Option<usize>`;
  `build_model` flags non-trailer `whl` parts with index ≥ 4 as
  `!simulated` followers of `whl(N−2)` with a load warning (`whl4`→`whl2`,
  `whl5`→`whl3` — the retail pairing). Trailer `twhl` parts keep the
  decorative-radius rule; the retail trailer binding beyond four wheels
  is unrecovered (no stock trailer exercises it).
- `mm2_content::assemble` emits only `simulated` wheels into
  `WheelGeom`/`WheelConfig` for the car rig (the trailer loader already
  filtered).
- `mm2_app::car_visual`: `WheelMount` gains `follow_offset`; a follower
  mounts on its reference wheel's physics index and renders at
  `reference position + authored offset − reference droop`, copying
  steer and spin — the same thing the original draws. `usize::MAX`
  parked mounts (decorative parts) are unchanged.
- `drive_probe` gains `--trace` (4 Hz telemetry: speed, vy, height,
  pitch, gear, rpm, up-axis, heading, angular velocity, per-wheel
  grounded/traction/suspension) — the instrument that isolated the
  limit cycle.
- `mm2-inspect car` prints each wheel's role (`simulated` /
  `follows whlN` / `decorative`) and exports `simulated`/`follows` in
  JSON.
- Regression test `model::tests::wheels_beyond_four_are_back_back_followers`.

## Verification (this tree)

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings`, `cargo test --workspace` — see commit
  record; all green.
- Retail (`fnv1a64:e91e6cd4b2ae30d9`):
  - `vpmoonrover`: 4 physics wheels + 2 followers, warnings report the
    pairing. `--drop` ok (settles, drives away, finite). `--controls`
    still `FAIL(drive)` — ~1 m/s launch — and `--clearance` now fails
    on both cities because the car rests nose-up on its tail (hull
    touch, penetration 0.000, fronts at droop). Both are the authored
    statics of the retail four-wheel rig, not defects: the CoM sits
    behind the rear axle so the nose must rise, and the follower wheels
    visibly float at the dropped tail — matching the community-reported
    "rear end levitates" of the original (cut test vehicle, per TCRF).
  - `vpcentury` (the other six-wheel model): `--drop` ok, `--controls`
    ok, `--clearance` ok on sf + london (tractor 4/4, trailer 4/4,
    hitch gap 0.00). Launch shows a wheelie transient (0-100 4.7→6.9 s,
    ~46° heading offset during the hop) then cruises straight at
    54 m/s.
  - `validate-cars` 21/21; roster accel otherwise unchanged.
    `vppanozgt`'s ~160° top-speed drift predates this change (committed
    `.vehgyro` drift-relief; its rig is four wheels throughout — the
    diff provably cannot affect it).

## Not done / blockers

- Per-car rendered-paint captures (F02-AC03) still not run — needs the
  GPU capture leg on this machine.
- Override causality demonstrated on vpbug only; a per-car override
  matrix is not run.
- `vpmoonrover` keeps its `FAIL(drive)`/`FAIL(clearance)` marks as
  honest output of authored statics — no exemption carved out.
- Whether the original suspension can exert pull-down force at droop
  (which would change the tip-over equilibrium) is unrecovered; the
  push-only strut is the conventional reading.
- The trailer `twhl4`/`twhl5` follower pairing exists in mm2hook's
  struct (`TrailerBackBack*PosDiff`) but no stock trailer exercises it;
  trailer wheels beyond four would need the same treatment if a
  trailer ever carried them.
