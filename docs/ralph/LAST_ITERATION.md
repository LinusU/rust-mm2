# Last iteration — authored `vehgyro` consumption

Iteration 32 on `ralph/night`, continuing from `d23fdeb` (the
externally checked F05-B.3 repair). Task id: `F05-B.4` — the next
record in the damage family, following the established
consume-authored-records pattern (B.2 `vehstuck`, B.3 `dgbangerdata`).

## Slice choice

The previous candidate is externally clean, so the highest-value
ready slice is the remaining decoded-but-unconsumed damage record:
`.vehgyro` (21 retail records — `Drift`/`Spin180`/`Reverse180` on all,
`Roll`/`Pitch` on 17, all authored 0.0). Evidence base:

- MM2Hook's recovered `vehGyro` (`src/modules/vehicle/gyro.h`):
  same five fields plus `Spinable`/`Driftable`/`Rightable` asNode
  gates mapping {Spin180,Reverse180}/{Drift}/{Pitch,Roll}. Its
  `Update()` is a binary thunk — application semantics unrecovered
  (UNK-13 stays open; the implementation is classified designed).
- MM1's recovered `mmCarSim` keeps a `SpinState` machine → a spin is
  a *latched maneuver*, not a per-frame gate: a 180's forward speed
  collapses to zero at 90°.
- The Crash Course teaches exactly these maneuvers ("Brake Dancing"
  = handbrake 180, "About Face" = reverse 180 / J-turn); community
  documentation reads the hold as dosing the rotation (short tap ≈
  90°, held ≈ 180°).

## What changed

- `mm2_vehicle::config`: `GyroConfig` (spin180/reverse180/drift +
  optional pitch/roll) on `VehicleConfig.gyro: Option<_>` — `None`
  on authored absence, never fabricated; validation requires finite,
  nonnegative rates (pitch/roll keep sign, inert ≤ 0).
- `mm2_vehicle::vehicle`: `GyroSpin` latch (signed rate, rotated,
  age) + `gyro_spins`/`gyro_completed` counters on `VehicleState`.
- `mm2_vehicle::systems` (DSN-22, designed reading): handbrake +
  steering while travelling (>3 m/s) latches a spin — `Spin180`
  forward, `Reverse180` in reverse — and the gyro then writes the
  authored yaw rate directly via `angular_velocity_mut()`. A torque
  servo was tried first and stalls ~140°: the tires' kinetic
  friction correctly resists rotation once the car scrubs its
  speed — the record authors a capability, so the assist delivers
  the rate rather than wrestling grip. The hold doses rotation:
  release ends it partway, ~π counts a completion, an opposite
  flick re-arms, `2.5×` nominal + 1 s age bound prevents indefinite
  servoing. `sim::yaw_damp_factor` extracts the yaw damper's factor
  and relieves its *slip* term by `Drift` (0 = unmodified policy).
  `Pitch`/`Roll` add per-axis critically damped airborne righting
  shaped on `air_control` — inert on the all-zero retail roster.
- `mm2_content`: `ConvertInput.gyro` plumbed from the decoded record
  (assemble already decoded it) into `config.gyro` verbatim;
  negative/non-finite rates warn + clamp to 0 rather than sink the
  load; `report.imported` provenance row for the gyro fields.
- `mm2_app::smoke`: `gyr=N/M` (latched/completed) only when a
  maneuver ran — the scripted driver never handbrakes, so stock
  cruises emit no `gyr=`.

## Tests

- `mm2_vehicle/tests/gyro.rs` (7): no-record car never latches;
  held handbrake+steer completes a ~180 (gyro_completed ≥ 1, real
  heading change); a tap doses a partial spin (spins 1, completed
  0, latch released); reverse travel latches at the authored
  `Reverse180` rate with the right sign; an opposite flick re-arms;
  `Drift` 0.9 vs 0.0 holds a tighter slide through the same
  handbrake input (and never latches below the steer trigger);
  authored pitch/roll levels an airborne tilted car where an
  unaided control does not.
- `mm2_vehicle::sim` unit test: `yaw_damp_factor` — `drift = 0`
  identical policy, monotone relief on the slip term only, clamps
  out-of-range authored values.
- `mm2_content/tests/damage.rs`: verbatim conversion leg (spin/
  drift land, absent Pitch/Roll stay `None`, `vehgyro` provenance
  row); `vpneg`/`vpgyrobad` fixture ids — out-of-range rate warns +
  clamps (decoded record verbatim, config clamped), undecodable
  record drops with a warning and `config.gyro` stays `None`; audit
  denominator now counts 3 strict failures (the two new gyro legs +
  the existing malformed vehcardamage).

## Evidence

- `cargo test --locked -p mm2_vehicle`: 22 lib (incl. new sim test)
  + 15 drive + 7 gyro + 8 surface — all pass.
- `cargo test --locked -p mm2_content --test damage`: 4/4.
- Quality gates: fmt/clippy/`cargo test --workspace` all green,
  760 tests / 0 failures (+9 vs d23fdeb's 751).
- Retail headless smoke on the supplied install
  (`--city sf --headless --bot --frames 600`, vpbug): `status=pass`,
  no `gyr=` (the bot never handbrakes), all counters identical to the
  drift-off A/B run; the live `Drift 0.2` relief moves the cruise
  endpoint ~1 m — authored data taking effect, not a regression.

## Classification / open items

- F05-B.4 is `implemented` (candidate) — pending external gates +
  review.
- Classifications: record decoding/census = verified_original
  (DMG-8); the maneuver application = designed (DSN-22); the
  original's trigger tests and mechanism = UNK-13, still open.
- Honest gaps: no gameplay feel evidence (headless only); no
  rendered proof of a spin; the J-turn trigger combination is the
  designed reading, not verified original.
- Still open in F05-B: visual tiers (smoke pivots,
  `TextelDamageRadius`, `DoublePivot`/`MirrorPivot`), damage-driven
  detachment if original, impairment, water/OOB recovery, C&R
  healing, replication.
