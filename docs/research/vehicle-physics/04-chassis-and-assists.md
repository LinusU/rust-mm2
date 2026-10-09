# Chassis: inputs to the wheels, aero damping, gyro assists, anti-roll, unsticking

**verified_original** throughout (retail `Midtown2.exe`, Kuna decompile
cross-checked with the disassembly), unless a line says otherwise.
Conventions in [README.md](README.md).

This covers `vehCarSim` itself (`0x1560` bytes) and the small
components that act on the body directly: `vehAero`, `vehGyro`,
`vehAxle`, `vehStuck`. These are where most of MM2's "arcade" handling
lives — in particular the **gyro**, which turns the car with the
throttle and the handbrake, and the **aero angular damping**, which
kills rotation.

## `vehCarSim` fields

| Off | Token / name | Default | Notes |
| --- | --- | --- | --- |
| `0x18` | body (`phInertialCS`, `0x1b4`) | | the rigid body; position = centre of mass |
| `0x1cc`/`0x1d0` | collider / level instance | | |
| `0x1d4` | WorldMatrix | | model-origin matrix (below) |
| `0x204` | `CenterOfGravity` | 0 | model space |
| `0x210` | ResetPosition | | |
| `0x228` | `InertiaBox` | (2, 1, 3) | → `InitBoxMass` |
| `0x234` | WheelCount | 4 | |
| `0x238` | `BoundFriction` | 0.3 | body–world contact friction |
| `0x23c` | `BoundElasticity` | 0.2 | body–world restitution |
| `0x244` | `Mass` | 2000 | kg |
| `0x248` | Speed | | `|forward velocity|`, m/s |
| `0x24c` | SpeedMPH | | `Speed · 2.236025` |
| `0x250` | ResetRotation | | |
| `0x254` | `DrivetrainType` | 0 | 0 RWD, 1 FWD, 2 4WD |
| `0x25c`.. | Engine, Transmission, Drivetrain, Freetrains, 4 Wheels, 2 Axles, 10 suspension visuals, Aero | | |
| `0x153c` | `CarFrictionHandling` | 1.0 | low-grip surface remap (02); the AI sets 2.0 while touching the player (06) |
| `0x1544` | (unknown) | 1.0 | |
| `0x154c` | Brake input | | 0..1 |
| `0x1550` | Handbrake input | | 0..1 (negative = front handbrake, below) |
| `0x1554` | Steering input | | −1..1, **+1 = right** |
| `0x1558` | `SSSValue` | 1.0 | |
| `0x155c` | `SSSThreshold` | 0 | m/s; 0 disables speed-sensitive steering |

`Init` (`0x4cbb70`) also sets the body's **maximum angular speed to
4π rad/s per axis** (`ICS+0x30`, `0x41490fdb`).

The world matrix (`0x4cbf40`) is the body matrix with its origin moved
by the rotated `CenterOfGravity`: `WorldMatrix.pos = body.pos +
CoG.x·row0 + CoG.y·row1 + CoG.z·row2`. The model origin is the body's
centre of mass **plus** `CoG`, so in model space the mass sits at
`−CenterOfGravity` (as `docs/vehicle-handling.md` already concluded
from the load split).

## The per-step routine — `vehCarSim::Update` (`0x4cc8d0`)

Called from `vehCar::Update` (`0x42c680`) right after gravity
(`mass · −19.6` into the body's force).

```
Speed    = |body.velocity · WorldMatrix.row2|          # forward speed magnitude
SpeedMPH = Speed · 2.236025
steer    = SSS(Speed) · SteeringInput                  # 0x4cc880, below

# front wheels
hbFront = Handbrake < 0 ? −Handbrake : 0
whl0.SetInputs(steer, Brake, hbFront)
whl1.SetInputs(steer, Brake, hbFront)

# rear wheels
hb = max(Handbrake, 0)
brakeRear = Brake
if Brake > 0.95 and engine.Throttle > 0.95 and Speed < 1.0:
    brakeRear = 0                                      # brake-stand: burn out on the spot
whl2.SetInputs(−steer, brakeRear, hb · (SteeringInput > 0 ? 1 − SteeringInput : 1))
whl3.SetInputs(−steer, brakeRear, hb · (SteeringInput < 0 ? 1 + SteeringInput : 1))

body.integrate()            # 0x477de0 — see 01-rigid-body.md
WorldMatrix = body.matrix ⊕ CoG                        # 0x4cbf40
update children: Engine, Transmission, Aero, trains (→ wheels), Axles, visuals
```

### Inputs to the wheels

- **Speed-sensitive steering** (`0x4cc880`):
  `SSS(v) = 1` if `SSSThreshold == 0`; otherwise
  `1 + (v/SSSThreshold)·(SSSValue − 1)` below the threshold and
  `SSSValue` above it — a linear fade of the lock from full at rest to
  `SSSValue`× at `SSSThreshold` m/s. **Every retail car authors
  `SSSThreshold 0`** (only the unused `vpvwcup_angel` sets 0.4 / 30), so
  the retail game applies no speed-sensitive steering in the physics.
  (The input layer's own steering filter is in
  [05-player-input.md](05-player-input.md).)
- **Rear wheels steer opposite** (`−steer`) through their own
  `SteeringLimit`. Most cars author 0; `vpbug`, `vpdune`, `vpvwcup`
  (0.03) and `vpauditt` (0.04) counter-steer the rear by up to ~2°,
  which sharpens turn-in.
- **Handbrake goes to the rear wheels only**, and the wheel on the
  *outside* of the steer is relieved: steering right (`+`), the
  rear-left gets `(1 − steer)` of the handbrake. Full lock right leaves
  only the rear-right (inside) wheel braked.
- A **negative handbrake input** brakes the *front* wheels instead
  (`−Handbrake`). Nothing in the player path is known to produce one
  (see 05/06).
- **Brake-stand**: with brake and throttle both above 0.95 below 1 m/s
  the rear wheels' foot brake is released, so a rear- or four-wheel
  drive car spins its rears in place (a front-drive car's driven
  wheels stay braked).

## `vehAero` (`0x4d9350`, fields `0x18..0x4c`)

| Off | Token | Default |
| --- | --- | --- |
| `0x18` | enabled | 1 |
| `0x20` | `AngCDamp` (vec3) | 0 |
| `0x2c` | `AngVelDamp` (vec3) | 0 |
| `0x38` | `AngVel2Damp` (vec3) | 0 |
| `0x44` | `Drag` | 0 |
| `0x48` | `Down` | 0 |

```
ωl = Rᵀ·body.ω                                   # angular velocity in car axes (x pitch, y yaw, z roll)
for axis a in x,y,z:
    α[a] = −sign(ωl[a])·AngCDamp[a] − ωl[a]·AngVelDamp[a] − |ωl[a]|·ωl[a]·AngVel2Damp[a]
    if |α[a]|·dt > |ωl[a]|: α[a] = −ωl[a]·invdt          # never reverses the spin in one step
    if |body.ω_world[a]| < 1: α[a] *= |body.ω_world[a]|  # fade all three terms near rest (world-axis components!)
body.torque += R·(α ⊙ body.angularInertia)
body.force  += −Speed·Drag · body.velocity
body.force  += −Speed²·Down · WorldMatrix.row1
```

- **The damping terms are angular accelerations** (rad/s²), turned
  into torque by the body's own inertia, so they mean the same on every
  car. Retail values are big: the Beetle decelerates its yaw by
  `1.4 + 2.0·ω²` rad/s² — a 1 rad/s yaw loses 3.4 rad/s² of it; the DB7
  authors 4 rad/s² of constant yaw damping, the Panoz GTR-1 a quadratic
  roll term of 6 (`AngVel2Damp.z`). This is the main reason an MM2 car's
  rotation dies the moment the tyres stop driving it.
- The low-speed fade multiplies by the **world**-axis component of the
  body's angular velocity, not the car-axis one used everywhere else in
  the routine — an original quirk.
- `Drag` is in kg/m: `F = Drag·|v_fwd|·v`. The Beetle's 0.5 is 800 N at
  40 m/s; many fast cars author 0 (no aero drag at all). `Down` presses
  along the car's down axis with `v_fwd²`.

## `vehGyro` (`0x4d5bf0`) — the handling assists

Loaded from `tune/vehicle/<car>.vehgyro` (`FileIO` `0x4d5ed0`):

| Off | Token | Retail range |
| --- | --- | --- |
| `0x1c` | `Drift` | 0–0.727 (Beetle 0.2) |
| `0x20` | `Spin180` | 0–1.61 (Beetle 0.8) |
| `0x24` | `Reverse180` | 0–6.14 (Beetle 2.617) |
| `0x28` | `Pitch` | 0 everywhere |
| `0x2c` | `Roll` | 0 everywhere |

Gates are asNode flag bits: `0x20000` (drift) is set by the
constructor and stays on; `vehCar::Update` sets `0x10000` while
`|Handbrake| > 0.01` and `0x40000` while `|Brake| > 0.01`.

Let `ωd` be the **primary drivetrain's** spin (`carSim+0x40c`,
negative rolling forward), `I = body.angularInertia`, `up/right/back`
the car's axes, and `allDown = (OnGround() / WheelCount)` evaluated in
**integer** arithmetic — 1 only when all four wheels touched the ground
this step, 0 otherwise.

```
# Drift — always on
if Drift > 0:
    s = SSS(Speed) · SteeringInput
    τ = I.y · Drift · |s|·s · allDown · ωd                       # about up
# Handbrake spin
if handbrake held and (Spin180 > 0 or Reverse180 > 0):
    k = (ωd >= 0) ? Reverse180 : Spin180                         # rolling back / stopped vs forward
    τ = I.y · k · SteeringInput · allDown · ωd                   # about up
# Air levelling (only while braking and not all wheels down)
if brake held and (Pitch > 0 or Roll > 0):
    w = Brake² · (1 − allDown)
    τ  = I.x · Pitch · w · R[2][1] · right                       # pitch nose toward level
    τ −= I.z · Roll  · w · R[0][1] · back                        # roll toward level
```

What the first two do, concretely:

- **Drift** adds a yaw acceleration of `Drift·s|s|·|ωd|` *into* the
  turn (negative `ωd` × positive steer gives a clockwise, i.e. rightward,
  yaw). It is proportional to **the driven wheels' spin**, not to the
  car's speed: on the Beetle at 30 m/s (`ωd ≈ −89 rad/s`) full lock adds
  ~18 rad/s² of yaw acceleration — the same order as the ~29 rad/s² its
  two front tyres can produce at their limit (`2·μ·N·a / I_y`) — and a
  car doing a burnout on the spot (wheels spinning, car
  still) *rotates* when steered: MM2's doughnuts. The quadratic `s|s|`
  keeps small corrections gentle.
- **Spin180** (forward) and **Reverse180** (rolling backwards or
  stopped) add `k·s·|ωd|` of yaw while the handbrake is held —
  linear in steer. It adds to the yaw the locked, sliding rear tyres
  allow and is what makes a handbrake turn swing round so fast; the
  larger `Reverse180` makes the J-turn.
- Both need all four wheels on the ground and both use the *primary*
  drivetrain, so a front-drive car's gyro follows its front wheels'
  spin (including wheelspin).
- **Pitch/Roll** levelling is only active in the air, only while the
  brake is held, and every retail record authors 0 — dead in retail.

## `vehAxle` — anti-roll (`0x4d9b10`)

Tokens `TorqueCoef` (`0x94`) and `DampCoef` (`0x98`), `ComputeConstants`
`0x4d9a10`:

```
k = TorqueCoef · body.angularInertia.z
c = 2·sqrt(k · body.angularInertia.z) · DampCoef
Δx = (xL − visL) − (xR − visR)                     # compression difference (visual squash removed)
τ = −(Δx·k + (xdotL − xdotR)·c)  about the body's roll axis (row2)
```

Every retail axle authors `TorqueCoef 0`, so there is **no anti-roll
bar in retail**; the axle node only positions the visual axle.

## `vehStuck` (`0x4d6130`) — getting unstuck

Tokens (`FileIO` `0x4d6500`): `Turn` (`0x40`), `Rotation` (`0x44`),
`Translation` (`0x48`), `TimeThresh` (`0x2c`), `PosThresh` (`0x30`),
`MoveThresh` (`0x34`). Defaults: `TimeThresh 0.3`, `PosThresh 1.25`,
`MoveThresh 1.75`, `Turn 1.57`, `Rotation 0.39`, `Translation 0.1`.

States (`0x18`): 0 idle, 1 armed, 2 turn-in-place, 3 impulse, 4 flip.
`0x20` holds the reference position. Each step:

```
if |pos − ref|² > MoveThresh²:  reset (state 0)        # the car got away
state 1 (armed):
    timer += dt
    near = horizontal |pos − ref|² <= PosThresh²
    if near and OnGround()==0 and timer >= TimeThresh and (Translation>0 or Rotation>0):
        state = Rotation > 0 ? 3 : 4                   # wheels off the ground → flip
    elif near and timer >= TimeThresh and tryingToTurn():
        state = 2
    elif |pos − ref|² <= MoveThresh² and timer <= TimeThresh: keep waiting
    else reset
state 2 (turn in place):  while |pos − ref|² <= MoveThresh² and tryingToTurn():
    rotate the body about the world vertical by −dt·Turn·|steer|·steer
    (sign flipped in reverse gear); otherwise reset
state 3 (impulse, unused: Rotation 0 everywhere):
    if body up.y <= 0.7 (tilted past ~45°): lift by |steer|·mass·Translation,
                        twist by steer·mass·Rotation; throttle = 0, brake = 1
state 4 (flip): rebuild an upright matrix from the current heading, raise it by
                Translation, set it, reset.
tryingToTurn() = Throttle > 0.75·throttleCap and |steer| > 0.5             # 0x4d60f0
                  (throttleCap = engine+0x30: 1.0 for the player, maxThrottle for AI)
```

The car's collision callback (`0x4cb340` → `0x4d60b0`) arms it:
state 1, reference = the position at the impact. Retail authors `Turn` 1.57–3.14 rad/s,
`TimeThresh` 1–2 s, `PosThresh 1.25`, `MoveThresh 1.75`,
`Translation 0.1`.

## Impact parameters

`RestoreImpactParams` (`0x4cc040`, from `Reset`) gives the body's bound
the file's `BoundFriction`/`BoundElasticity`. `SetHackedImpactParams`
(`0x4cc070`: friction `2.0`, elasticity `0`, `Brake = 1`) has no callers
or pointers in the shipped exe — dead code. How the bound values enter
the contact solver is in [01-rigid-body.md](01-rigid-body.md#collisions).

## Drivable modes — `vehCar::SetDrivable` (`0x42c2b0`) / `PreUpdate` (`0x42c470`)

`SetDrivable(true)` sets the car's drivable flag (`+0xe8` bit 2), mode 0,
and selects first gear. `SetDrivable(false, mode)` clears the flag and
stores `mode`; modes 1 and 3 also select neutral. While not drivable,
`PreUpdate` overrides the inputs once per frame, before the frame's
physics steps:

| Mode | Effect each step |
| --- | --- |
| 1 | `Brake = 1`, gear forced to neutral — throttle and steering stay live (the engine can be revved, e.g. on a start grid) |
| 2 | `Brake = 1`, `Throttle = 0`, `Steering = 0`, `Handbrake = 0` |
| 3 | as 2 (neutral was selected once, when the mode was set) |

The stuck and splash updates also only run while the car is drivable.

## What runs after the car sim — `vehCar::Update` (`0x42c680`)

```
body.force.y += body.mass · −19.6                     # 0x46a110
carSim.Update()
gyro flags from inputs ; gyro.Update()
stuck.Update()        (if active and the car allows it)
splash (water level vs WorldMatrix.y), wheel particles ×4, room tracking
damage.Update()
```
