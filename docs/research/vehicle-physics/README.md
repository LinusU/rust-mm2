# How Midtown Madness 2 drives — the original vehicle physics

A complete reading of the retail `Midtown2.exe` vehicle simulation:
every force on a car, every variable it uses, and how keyboard, pad,
wheel and AI inputs become those forces. Written so the steering,
handling and feel can be reproduced exactly.

## Method and confidence

- The retail executable (`retail/Midtown2.exe`, x86-32, image base
  `0x400000`) was decompiled in full with
  [Kuna](https://github.com/noelo-Lab/kuna) v1.755 (9 075 functions).
  Every formula here was read from that output, and every branch whose
  meaning depends on an x87 comparison was re-read in the disassembly.
- Names come from [mm2hook](https://github.com/Fireboyd78/mm2hook)
  (Fireboyd78's and Dummiesman's forks) and, for the engine's shared
  classes, [Open1560](https://github.com/0x1F9F1/Open1560). **Our
  exe's addresses differ from mm2hook's by a per-region shift**: the
  vehicle and physics code sits `0x10` below (`vehWheel::Update`
  mm2hook `0x4D34E0`, ours `0x4d34d0`), `mmInput` `0x20` below,
  `aiVehiclePhysics` `0x240` below, `mmPlayer` unshifted, and most data
  `0x1000` below (`datTimeManager::Seconds` `0x5CE820` → `0x5cd820`).
  Every address in these documents is ours and was checked by content.
- Field names in mm2hook are partly guesses; where the code says
  otherwise these documents use the code's meaning (e.g. the
  drivetrain's `AngInertia` is a stiffness, the wheel's
  "SuspensionRestingPosition" is a progressivity term).
- Class labels follow `docs/original-rules.md`: **verified_original**
  (read in the code), **inferred** (reasoned, with the reason given),
  **unknown**. Unless a paragraph says otherwise, it is
  verified_original.
- Nothing was measured by running the original; the claims are about
  what the code computes.

## Documents

| File | Contents |
| --- | --- |
| [01-rigid-body.md](01-rigid-body.md) | The body integrator (`phInertialCS`), the time step, gravity, implicit springs, collisions and impact parameters |
| [02-wheel-suspension-tire.md](02-wheel-suspension-tire.md) | Ground probe, suspension, the stick–slip tyre model, friction curve and circle, surfaces |
| [03-powertrain.md](03-powertrain.md) | Engine torque curve, auto-clutch, gearbox and shift logic, drivetrain spin integration, the kinematic differential, brakes |
| [04-chassis-and-assists.md](04-chassis-and-assists.md) | `vehCarSim::Update` (how inputs reach the wheels), speed-sensitive steering, aero rotational damping, the **gyro assists**, anti-roll, unsticking |
| [05-player-input.md](05-player-input.md) | Keyboard, mouse, pad, wheel → throttle/brake/steer/handbrake/gear |
| [06-ai-input.md](06-ai-input.md) | How opponents, police and traffic drive the same car model |
| [07-tuning-reference.md](07-tuning-reference.md) | Every `.vehcarsim`/`.vehgyro`/`.vehstuck` token: field, default, effect; the retail roster; derived numbers |
| [08-replication-recipe.md](08-replication-recipe.md) | The whole thing as one per-step pseudocode, ready to port |
| [09-differences-from-rust-mm2.md](09-differences-from-rust-mm2.md) | Snapshot comparison with rust-mm2 at `c3e1223`, and a suggested porting order |

## Conventions

- **Axes** (model and vehicle space): `+X` right, `+Y` up, `+Z` back —
  forward is `−Z`. AGE matrices are row-major with row vectors
  (`world = local · M`): rows 0–2 are the X, Y, Z axes, row 3 the
  position.
- **Units**: metres, kilograms, seconds, radians, newtons. Engine speeds
  are authored in rpm and used in rad/s; gearbox speeds are authored in
  **mph**.
- **Steering input `+1` is full right**; positive wheel angle is a left
  turn, so the wheel code negates it.
- **Wheel spin**: rolling forward is **negative** `ω` (rotation about
  `+X`).
- `whl0` front-left, `whl1` front-right, `whl2` rear-left, `whl3`
  rear-right.
- `dt` is `datTimeManager::Seconds` (`0x5cd820`) and `invdt` its
  inverse (`0x5cd828`); see [01-rigid-body.md](01-rigid-body.md) for how
  they are set.

## Gravity is 19.6 m/s²

The single most important number. `vehCar::Update` (`0x42c680`) begins
by adding `mass · −19.6` to the body force (`0x46a110`, reading the
read-only global `0x5c5c1c`); the same helper drives bangers
(`dgBangerActive::Update`, `0x440040`), trailers and a few others. The
wheels' static loads are computed from the same constant, so the whole
car model is built around **2 g**:

- every tyre force scales with a normal load twice the real one, and
  the authored friction coefficients are large (`StaticFric` 3.0 ×
  tarmac 0.9 = 2.7) — together about 5 real g of grip;
- springs carry twice the weight over the same travel, so they are
  twice as stiff for the same sag;
- a car falls with twice a real car's acceleration and its body
  pitches and settles faster, a large part of why MM2 cars feel planted
  and "heavy" over crests.

A re-implementation that keeps the authored numbers but uses 9.81
halves every tyre's grip and every spring's rate relative to the mass
it carries.

## The per-step pipeline

Per simulation step, for each car (`vehCar::Update`, `0x42c680`):

```
body.force += mass · (0, −19.6, 0)                       # gravity
vehCarSim::Update                                        # 04
    Speed = |v · forward|
    steer = SSS(Speed) · SteeringInput                   # SSS off in retail
    front wheels  SetInputs(steer,  Brake,  max(−Handbrake, 0))
    rear wheels   SetInputs(−steer, Brake*, Handbrake split by steer)
    body.integrate()                                     # 01 — uses forces from the previous step + gravity
    WorldMatrix = body.matrix shifted by CenterOfGravity
    Engine.Update        torque from throttle, auto-clutch, shift torque cut      # 03
    Transmission.Update  automatic up/down shifts                                 # 03
    Aero.Update          angular damping, drag, downforce → body                  # 04
    for each train (freetrains, then the drivetrain):                             # 03
        brake + engine + last step's tyre reaction torques
        for each wheel: ray, suspension, surface, contact velocities              # 02
        limited-slip bias; integrate shaft spin implicitly; engine limiter
        for each wheel: Update — stick–slip tyre forces + suspension → body       # 02
    Axles (anti-roll, 0 in retail), visual suspension
vehGyro::Update      drift / handbrake-spin yaw torques, air levelling            # 04
vehStuck::Update     unstick / flip                                               # 04
splash, particles, damage
```

The car's forces are therefore computed **after** the body has been
moved this step and integrated at the start of the next — a one-step
lag between the state the tyres see and the motion they cause, and a
second one between a tyre's longitudinal force and the drivetrain
torque that answers it.

## What makes it feel like MM2

In rough order of importance, the mechanisms a re-implementation must
reproduce (each links to its section):

1. **2 g gravity with large μ** — above.
2. **The tyre is a stick–slip spring per axis**, not a slip-angle curve
   alone. While its displacement is inside `μ·N/k` the tyre is a stiff
   spring–damper (static friction: no creep, no drift on slopes, instant
   response); past it, the force is `μ(slip)·N` with μ from a parabola
   that peaks at `OptimumSlipPercent` and falls to `SlidingFric`. A
   friction circle couples the two axes. ([02](02-wheel-suspension-tire.md))
3. **The gyro "Drift" assist** adds yaw acceleration
   `Drift · steer·|steer| · |ω_driven|` into the turn whenever all four
   wheels are down — proportional to the driven wheels' spin, so it
   grows with speed and works during wheelspin (doughnuts). On a Beetle
   at 30 m/s it is ~18 rad/s² at full lock. ([04](04-chassis-and-assists.md#vehgyro-0x4d5bf0--the-handling-assists))
4. **The handbrake-spin assist** (`Spin180`/`Reverse180`) adds yaw
   `k · steer · |ω_driven|` while the handbrake is held; the handbrake
   itself only brakes the rear wheels, and relieves the outside one.
5. **Aero rotational damping** (`AngCDamp`, `AngVelDamp`,
   `AngVel2Damp`): per-axis constant, linear and quadratic angular
   deceleration, scaled by inertia, which kills rotation as soon as
   nothing drives it. ([04](04-chassis-and-assists.md#vehaero-0x4d9350-fields-0x180x4c))
6. **Slow, damped wheel spin** — the drivetrain integrates
   `Δω = dt·τ/(I + dt·AngInertia)` with `AngInertia` 2 000–30 000, so
   wheels spin up and down over tenths of a second: handbrake slides
   persist after release, burnouts build, lock-ups recover slowly.
   ([03](03-powertrain.md#drivetrain-spin--vehdrivetrainupdate-0x4d9e80))
7. **Engine and box**: a √5 torque curve with peak power exactly at
   `OptRPM`, an auto-clutch (no stalls; launches at 2× idle), a hard
   rev limiter that limits road speed per gear, `GCL` seconds of zero
   torque per shift, power-optimal automatic shift points.
   ([03](03-powertrain.md))
8. **Steering geometry**: the authored lock, Ackermann via
   `SteeringOffset`, a small rear counter-steer on some cars, and *no*
   speed-sensitive reduction in the physics (`SSSThreshold` is 0 on
   every retail car — what the input layer does is in
   [05](05-player-input.md)).
9. **Brakes near the lock limit** (`1.2·BrakeCoef` ≈ `SlidingFric/StaticFric × surface`)
   and a constant 50 N·m drag per drive train instead of rolling
   resistance.
10. **Everything acts at the contact patch** — tyre forces roll and
    pitch the body about its centre of mass with the full height as
    lever; nothing raises the roll centre.
11. **The step is the frame** — a variable `dt` (one step of 1/60 s at
    60 fps, split into at most three steps below ~34 fps), with forces
    lagging the integration by one step and several per-step terms, so
    the feel depends on frame rate (the designers presumably tuned at
    the 30–85 fps of the day — **inferred**). ([01](01-rigid-body.md#the-time-step))
12. **No body damping** in the integrator; the only rotational damping
    is `vehAero`'s, the only clamps 500 m/s and 4π rad/s per axis.
13. **Keyboard steering is a per-car, speed-dependent rate ramp**, not a
    lock limit: `tune/<car>.asnode` sets how fast the wheel turns
    (Beetle: ~0.4 s to full lock at rest, ~1.3 s at 40 m/s), it springs
    back at 5/s, the output is shaped `^1.2`, and the full authored lock
    is available at any speed. ([05](05-player-input.md#steering))
14. **Small rules around stopping**: below 4 mph off the throttle the
    handbrake is held for you; holding the brake ≥ 80% below 5 m/s
    swaps the pedals into reverse; brake + throttle at a standstill
    releases the rear brakes for a burnout. ([05](05-player-input.md), [04](04-chassis-and-assists.md#inputs-to-the-wheels))
15. **AI cars drive the same model**, with no rubber-banding, but lose
    half their grip while touching the player, damp their own rotation
    while braking, and cops get a ×1.03-per-frame boost when flat out.
    ([06](06-ai-input.md))
