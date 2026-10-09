# Where rust-mm2 differs from the original (snapshot)

A comparison of the recovered original (01–08) with rust-mm2's vehicle
model **as of commit `c3e1223` (2026-10-08)** — `crates/mm2_vehicle`,
`crates/mm2_content/src/convert.rs`, `crates/mm2_app` input and AI.
It goes stale as the code changes; the original's side does not.
"Ours" values were read from the code, not measured.

Ordered roughly by how much each difference should change the feel.

| # | Mechanism | Original | Ours (`c3e1223`) | Expected effect |
| --- | --- | --- | --- | --- |
| 1 | **Gravity** | 19.6 m/s² for every body; wheel loads built on it ([01](01-rigid-body.md#gravity)) | 9.81 (`mm2_app/src/main.rs:1498`) | Half the normal load → half the tyre force per unit μ, softer-feeling springs, floaty jumps |
| 2 | **Grip level** | `μ = StaticFric × surface friction` (2.7 on tarmac), on loads at 2 g → ~53 m/s² | lateral `StaticFric × 0.9` × surface normalised to `_default` (→ 2.7), longitudinal `× 0.6`, load sensitivity 0.3, all at 9.81 (`convert.rs:49-50`, `sim.rs:230`) | Roughly half the lateral and a third of the longitudinal capacity |
| 3 | **Tyre model** | Per-axis stick–slip springs (`k = 2L/DispLimit`, damped), limit `μ(s)·N` from a parabola in slip *ratio* (`tan` of slip angle; wheel slip ratio), friction circle with sliding-direction rule ([02](02-wheel-suspension-tire.md)) | Piecewise-linear slip-*angle* curve, `OptimumSlipPercent` read as radians; longitudinal force = requested force capped by a demand curve; circle on lateral grip only; `TireDisp*`/`TireDamp*` unused | Different transient response (no relaxation-length lag, no static hold), different post-peak fall-off, different combined slip |
| 4 | **Wheel spin** | Each drive train integrates its own `ω` (`Δω = dt·τ/(I + dt·AngInertia)`), tyre long force from `ω·r + v`; locked wheels, wheelspin and slow recovery are emergent ([03](03-powertrain.md#drivetrain-spin--vehdrivetrainupdate-0x4d9e80)) | No wheel angular state; drive/brake are forces at the patch; spin is visual (`systems.rs:601`) | No real burnouts, lock-ups or lingering handbrake slides |
| 5 | **Gyro: Drift** | Yaw torque `I_y·Drift·s|s|·ω_drive` every step with all wheels down — turns the car in with the driven wheels' spin ([04](04-chassis-and-assists.md#vehgyro-0x4d5bf0--the-handling-assists)) | Reduces the yaw damper's slip term (`systems.rs:625-629`) | Ours lacks MM2's strong throttle-on turn-in and doughnuts |
| 6 | **Gyro: Spin180/Reverse180** | Yaw torque `I_y·k·s·ω_drive` while the handbrake is held (forward/back gains) | A latched yaw-rate floor written to `angular_velocity.y` (`systems.rs:672-775`) | Different handbrake-turn shape; ours can't overshoot or scale with speed the same way |
| 7 | **Rotational damping** | `vehAero` per car axis: constant (`AngCDamp`), linear, quadratic (`AngVel2Damp`) angular deceleration ×inertia, never reversing spin; no other body damping | `AngularDamping(0.02)`, a yaw damper `2.0·m·wb²/12` (world Y), slide recovery, countersteer; the authored `AngCDamp`/`AngVel*Damp` unused (`lib.rs:134-135`, `systems.rs:625-647`) | Different spin-out and recovery character; pitch/roll damping absent in ours |
| 8 | **Steering input (keyboard)** | Per-car `tune/<car>.asnode` rate ramp: `DeltaOut` (2.5→0.8/s on the Beetle, by speed), `DeltaIn` 5/s back, output `^1.2`; full authored lock at any speed ([05](05-player-input.md#steering)) | Binary keys; sim-side `|x|^1.4`, rate 4 rad/s out / 7 back, lock `×1.5`, grip-limited cap `∝ 1/v²` (`systems.rs:127-157`, `convert.rs:125`) | Ours caps lock at speed; the original slows the *rate* instead and keeps the lock |
| 9 | **Steering geometry** | Ackermann from `SteeringOffset`; rear wheels **counter**-steer by their own limit (Beetle/Dune/VW Cup 0.03, TT 0.04); pivot on the wheel's inner edge | Same angle on every steered wheel; `SteeringOffset` reduces high-speed lock; rear steer in phase (`convert.rs:643-659`) | Ours makes the rear-steer cars more stable, the original more agile |
| 10 | **Suspension** | Sag = `SuspensionExtent` at 19.6; `SuspensionLimit` = bump stop with impulse + push-out; ζ ≈ 4.43·`DampCoef` (0.44 typical, **0.04** on the Cab and police car); force at the contact point along the normal; implicit integration | Sag `0.35·(Extent+Limit)` at 9.81, Limit as extra travel, ζ remapped to 0.55–1.0, force raised `0.3·r` (`convert.rs:446-475`, `systems.rs:211-262`) | Different ride height, pitch/roll under load, and the bouncy Cab/police character is gone |
| 11 | **Roll/pitch lever** | Tyre forces at the contact patch, full CoM height as lever | `roll_resistance 0.95`, per-car `pitch_resistance` raise the force points toward the CoM | Ours rolls and squats far less |
| 12 | **Brakes and handbrake** | Per-wheel torque `BrakeCoef·StaticFric·r·L`, ×1.2 static; foot brakes near the lock limit; handbrake rear only, outside wheel relieved by steer, rear-right `HandbrakeCoef` stuck at 1.0 | Forces, `max_brake_force = m·g·max(0.55·μ·1.2, 0.8)` split by normalised `BrakeCoef`; handbrake force plus lateral grip ×(1 − 0.6·hb) (`systems.rs:519-529`) | Different stopping and very different handbrake behaviour |
| 13 | **Engine and box** | √5 torque curve, peak power at `OptRPM`; auto-clutch (detach < idle, attach > 2×idle); engine inertia reflected `g²·I`; rev limiter caps road speed per gear; `GCL` s of zero torque; shift points from equal-power bisection with biases; efficiency 1.0; engine braking `0.75·T_opt` at `OptRPM` | Piecewise-linear curve peaking at 0.72·OptRPM; RPM from wheel speed, no clutch or inertia; efficiency 0.85; shifts at `OptRPM` / `0.5·OptRPM`; shift ≤ 0.35 s with a 0.25 torque floor; engine braking `0.35·T_opt` (`sim.rs:96-147`, `systems.rs:265-335`) | Different launch, pull through gears and lift-off decel |
| 14 | **Assists that don't exist in the original** | — (no traction control, ABS, stability control, slide recovery, countersteer; airborne levelling only while braking and authored 0) | Traction control 0.85, slide recovery 2.5, countersteer 0.3, air control 8.0, grip cap | Ours is far more forgiving; the original lets the car slide and relies on gyro + aero damping |
| 15 | **Differential** | Kinematic limited slip: wheel speeds `ω·b`, `ω/b`, `b` ≤ 1.25 at rest → 1.03 above 50 rad/s | Static torque split | Little at speed (both nearly spool-like); matters for wheelspin out of corners |
| 16 | **Auto-reverse and stops** | Below 5 m/s with brake > 0.8 and throttle < 0.1 → reverse with swapped pedals; stopped-hold handbrake below 4 mph off-throttle | Reverse at ≤ 0.25 m/s with brake > 0.05; `BrakeCarry` option | Original reverses earlier and never creeps |
| 17 | **Body collisions** | Friction `BoundFriction × world` (e.g. 0.5×0.9), restitution `BoundElasticity × world` (e.g. 0.5×0.9 = 0.45), impulses + max-merged pushes | Friction ≤ 0.3, restitution ≤ 0.1 (`convert.rs:881-883`) | Original wall hits bounce noticeably more |
| 18 | **Clamps** | Body angular speed ≤ 4π rad/s per axis; ≤ 500 m/s | None on player/AI cars | Only visible in big crashes |
| 19 | **Time step** | The frame (one 1/60 s step at 60 fps), forces lag the integration by one step | Fixed 120 Hz with Avian substeps; forces computed once per step | Per-step terms (drivetrain stiffness, diff smoothing, bump stop) need retuning at 120 Hz |
| 20 | **AI** | Same car model; no rubber-banding; grip halved while touching the player; ×0.85 angular momentum while braking; police ×1.03 momentum boost | Catch-up throttle/corner boost, re-anchor teleport, no contact grip halving (`opponents.rs:1586-1824`) | Ours' opponents are harder to shove and can catch up artificially |

## Suggested order for a faithful port

Each step is testable on its own against the formulas in 02–04:

1. Gravity 19.6 and the original's static loads, spring rate
   `L/Extent`, `SuspensionLimit` as a bump stop, damping `2·sqrt(k·L)·ζ'`.
   Drop `roll_resistance`/`pitch_resistance` (and `clear_underside`
   should be re-checked against the original's contact-point forces).
2. Wheel angular state per drive train with the implicit
   `I + dt·AngInertia` step, the 50 N·m drag, per-wheel brake torques.
3. The stick–slip tyre (displacements, `μ(s)` parabola, the circle rule).
4. `vehAero` angular damping in place of the yaw damper, slide recovery,
   countersteer and the global angular damping.
5. The gyro as torques.
6. The engine curve, clutch, inertia coupling and gearbox.
7. Input: the `.asnode` ramps, auto-reverse and stopped-hold rules;
   remove the grip cap and lock scaling.
8. AI: contact grip halving, the ×0.85/×1.03 momentum terms; remove
   catch-up.

Keep the 120 Hz step only if the per-step terms are rescaled to give
the original's 60 Hz behaviour (e.g. `AngInertia·dt` at 1/60).
