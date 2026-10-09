# Powertrain: engine, gearbox, drivetrains, differential, brakes

**verified_original** throughout (retail `Midtown2.exe`, Kuna decompile
cross-checked with the disassembly), unless a line says otherwise.
Conventions in [README.md](README.md).

A car has one `vehEngine` (`+0x25c`), one `vehTransmission` (`+0x2e0`)
and three `vehDrivetrain`s: the primary `Drivetrain` (`+0x3d4`) and two
`Freetrain`s (`+0x420`, `+0x46c`). A drivetrain is a single spinning
shaft shared by the wheels attached to it; a freetrain is a drivetrain
with no engine and one wheel.

## Which wheels go where — `ReconfigureDrivetrain` (`0x4cc0a0`)

`DrivetrainType` (file; constructor default 0):

| Type | Primary drivetrain (engine) | Freetrain L | Freetrain R |
| --- | --- | --- | --- |
| 0 — rear drive | `whl2`, `whl3` | `whl0` | `whl1` |
| 1 — front drive | `whl0`, `whl1` | `whl2` | `whl3` |
| 2 — four-wheel | all four (`whl0..3`, in that order) | — | — |

The wheels are asNode children of their train, so each train updates
its own wheels. The car's child order — which is the per-step update
order — is: `Engine`, `Transmission`, `Aero`, then `FreetrainL`,
`FreetrainR`, `Drivetrain` (types 0/1) or just `Drivetrain` (type 2),
then `AxleFront`, `AxleBack`, then the ten visual suspension pieces.
The `Freetrain` file block is loaded into `FreetrainL` and copied to
`FreetrainR` (`0x4d9dd0`: `AngInertia`, `BrakeDynamicCoef`,
`BrakeStaticCoef`).

## Engine (`vehEngine`, `0x84` bytes)

### Fields and file tokens (`FileIO` `0x4d9230`)

| Off | Token / name | Default | Notes |
| --- | --- | --- | --- |
| `0x18` | `MaxHorsePower` | 200 | |
| `0x1c` | `IdleRPM` | 750 | |
| `0x20` | `OptRPM` | 5000 | peak-power rpm |
| `0x24` | `MaxRPM` | 8000 | hard limiter |
| `0x28` | `GCL` | 0.25 | gear-change lag, s |
| `0x2c` | powerScale | 1.0 | multiplies `MaxHorsePower`; never written by the vehicle code |
| `0x30` | throttleCap | 1.0 | the AI writes its `maxThrottle` here; read only by the stuck tests (04, 06) |
| `0x34` | `AngInertia` | 1.0 | engine inertia, kg·m² |
| `0x38`,`0x3c`,`0x40` | ωmax, ωopt, ωidle | | rad/s (`rpm · 2π/60`) |
| `0x44` | 1/(ωmax−ωopt)² | | |
| `0x48` | P | | `powerScale·HP·746 / ωopt³` |
| `0x4c`,`0x50` | A, B | | `(√5 ± 1)/2` |
| `0x54` | rpmAtShift | | |
| `0x58` | gclTimer | | |
| `0x5c` | inGearChange | | |
| `0x60` | Throttle | 0 | input, 0..1 |
| `0x64` | ωe | | engine speed, rad/s |
| `0x68` | RPM | | displayed (and used by the auto box) |
| `0x6c` | hp | | display: `T·ωe/746` |
| `0x70` | T | | engine torque this step, N·m |

`1 hp = 746 W` (`0x5b2028`).

### Torque curve

`CalcTorqueAtFullThrottle(ω)` (`0x4d8e10`):

```
Topt = powerScale · HP · 746 / ωopt          # torque at peak power
if ω <= ωopt:   T = P·(A·ωopt − ω)·(B·ωopt + ω)
elif ω <= ωmax: T = P·(ωmax − ω)·(A·ωopt − ω)·(B·ωopt + ω)·(ω + ωmax − 2·ωopt) / (ωmax − ωopt)²
else:           T = 0
```

Because `A·B = 1` and `(A−1)(B+1) = 1`, `T(0) = T(ωopt) = Topt`; the
curve below `ωopt` is a parabola peaking at `ωopt/2` with `1.25·Topt`;
power `T·ω` peaks at exactly `MaxHorsePower` at `OptRPM`; above it the
quartic falls to zero at `MaxRPM`.

`CalcTorqueAtZeroThrottle(ω)` (`0x4d8e90`) — engine braking and idle:

```
T0 = 0.75 · Topt · (ωidle − ω) / (ωopt − ωidle)
```

zero at idle, pushing up below it, braking linearly above it
(`−0.75·Topt` at `OptRPM`).

`CalcTorque(throttle) = throttle·T(ω) + (1 − throttle)·T0(ω)`
(`0x4d8ec0`), evaluated at the engine's current `ωe`.

### `vehEngine::Update` (`0x4d8f20`)

```
if trans.gearChanged and not inGearChange:
    rpmAtShift = RPM ; gclTimer = GCL ; inGearChange = 1
T = CalcTorque(Throttle)
ratio = trans.currentRatio
# automatic clutch
if ratio == 0 or ωe < ωidle:          detach the primary drivetrain (if attached)
elif ωe > 2·ωidle and not attached:   attach it
if not attached:                        # free-revving
    ωe = clamp(ωe + dt·T/AngInertia, 0, ωmax)
RPM = ωe · 60/2π
if inGearChange:
    if gclTimer <= 0:
        inGearChange = 0 ; trans.gearChanged = 0 ; ωe = RPM·2π/60
    else:
        T = 0                                           # no drive during the shift
        RPM = (rpmAtShift·gclTimer + (GCL − gclTimer)·RPM)/GCL   # displayed blend
        gclTimer −= dt
hp = T·ωe/746
```

Two cosmetic extras: the engine's visual node is rocked by
`0.05·AngInertia·T/T(ωopt)` rad, and **in neutral** (`gear == 1`) a
reaction torque of `AngInertia·T/T(ωopt)` (per axis scaled by the
body's angular inertia) is fed into the body about the crank axis —
the car's long axis for rear/four-wheel drive, its cross axis for
front drive, or the engine node's own axis when the model has one — so
the car rocks when revved standing still.

There is **no stalling**: below idle the clutch simply opens, and it
closes again above twice idle (1500 rpm by default) — a launch is a
1500 rpm clutch drop, synchronised by the inertia term below.

## Transmission (`vehTransmission`, `0xF4` bytes)

### Fields and tokens (`FileIO` `0x4cf730`)

| Off | Token / name | Default |
| --- | --- | --- |
| `0x1c` | gearChanged | |
| `0x20` | IsAutomatic | 1 |
| `0x24` | CurrentGear | 2 (first) |
| `0x28` | timeInGear | |
| `0x2c` | `GearChangeTime` | 0.8 |
| `0x30` | manual ratios [8] | |
| `0x50` | `ManualNumGears` | 7 |
| `0x54` | `AutoNumGears` | 6 |
| `0x58` | auto ratios [8] | |
| `0x78` | upshift rpm [8] | 6000 |
| `0x98` | downshift rpm at full throttle [8] | 2000 |
| `0xb8` | downshift rpm at zero throttle [8] | 2000 |
| `0xd8` | `Reverse` | 20 (mph) |
| `0xdc` | `Low` | 20 (mph) |
| `0xe0` | `High` | 75 (mph) |
| `0xe4` | `UpshiftBias` | 0.05 |
| `0xe8` | `DownshiftBiasMin` | 0.05 |
| `0xec` | `DownshiftBiasMax` | 0.3 |
| `0xf0` | `GearBias` | 0.5 |

Gear index `0` is reverse, `1` neutral, `2..NumGears−1` forward; the
`*NumGears` counts include R and N (a 6-gear automatic has four forward
gears).

### Ratios — `ComputeConstants` (`0x4cf210`)

`GearRatioFromMPH(mph)` (`0x4cf520`) is the overall ratio (engine revs
per wheel rev, final drive included) that puts the engine at `OptRPM`
at `mph`, using the radius of the primary drivetrain's first wheel:

```
ratio(mph) = OptRPM / ((26.8224 · mph) / (2π · r))        # 26.8224 = 1609.344/60
```

For each box (`n` = `AutoNumGears` / `ManualNumGears`):

```
ratio[0] = −ratio(Reverse) ; ratio[1] = 0
ratio[2] = ratio(Low)      ; ratio[n−1] = ratio(High)
N = n − 3 ; q = (ratio[n−1]/ratio[2])^(1/N)
for i in 1..N−1:  ratio[2+i] = ratio[2] · q^(i + GearBias·i·(N − i)/N)
```

`GearBias 0` is a geometric progression; positive values push the
middle gears taller.

**Automatic shift points** (auto box only), for each forward gear `g`
below the top: with `v = ratio[g+1]/ratio[g]`, bisect (to 1 rpm) the
rpm `c ∈ [OptRPM, OptRPM/v]` (the upper end is where the next gear
would sit exactly at `OptRPM`; `MaxRPM` only replaces it in the
impossible case `MaxRPM ≤ v·OptRPM`) at which full-throttle power in
`g` equals full-throttle power after the upshift (`HP(c) = HP(c·v)`,
power being 0 above `MaxRPM`). Then

```
upshift[g]       = (1 + UpshiftBias)      · c
downFull[g+1]    = (1 − DownshiftBiasMin) · c · v
downZero[g+1]    = (1 − DownshiftBiasMax) · c · v
upshift[top]     = MaxRPM
```

### `vehTransmission::Update` (`0x4cf5f0`)

```
if carSim.OnGround() == 0: return            # no shifting (and no timer) while airborne
if IsAutomatic and gear >= 2 and timeInGear > GearChangeTime and not gearChanged:
    if gear < AutoNumGears−1 and RPM > upshift[gear]:  SetGear(gear+1)
    elif gear > 2 and RPM < Throttle·downFull[gear] + (1−Throttle)·downZero[gear]:
        SetGear(gear−1)
timeInGear += dt
```

`SetGear(g)` (`0x4cf6f0`): if `g` differs (and, in automatic,
`g < AutoNumGears`), set `gear = g`, `timeInGear = 0`,
`gearChanged = 1` — which starts the engine's `GCL` torque cut. Player
controls: `Upshift` (`0x4cf560`) in automatic only moves R→N→1st, in
manual any gear up; `Downshift` (`0x4cf5a0`) in automatic jumps any
forward gear to **neutral**, then N→R; `SetForward` (`0x4cf6d0`) goes
to first from R/N; `SetReverse`/`SetNeutral` to 0/1. How the input layer
uses these is in [05-player-input.md](05-player-input.md).

## Drivetrain spin — `vehDrivetrain::Update` (`0x4d9e80`)

### Fields and tokens (`FileIO` `0x4da560`)

| Off | Token / name | Default | Notes |
| --- | --- | --- | --- |
| `0x1c`/`0x20` | engine / trans | | set by Attach, cleared by Detach |
| `0x24` | wheelCount | | |
| `0x28`.. | wheels[4] | | in `AddWheel` order |
| `0x38` | ω | 0 | shaft speed in **wheel** rad/s |
| `0x3c` | bias | 1.0 | left/right speed ratio |
| `0x40` | `AngInertia` | 5000 | **not an inertia** — see below |
| `0x44` | `BrakeDynamicCoef` | 1.0 | |
| `0x48` | `BrakeStaticCoef` | 1.2 | |

Sign convention: `ω < 0` is rolling forward. Torques are accumulated
as `T` that *decelerates* `ω`; the integrator applies `−T`.

### The step

```
# 1. brakes (+ a fixed 50 N·m of drag per drivetrain, 0x5b1d18)
coef = (ω != 0) ? BrakeDynamicCoef : BrakeStaticCoef
B = 50 + Σ wheel.brakeTorque · coef

# 2. engine (only while attached)
if engine:
    g = trans.currentRatio
    T = g·engine.T + g·engine.AngInertia·(g·ω + engine.ωe)·invdt
else:
    T = 0
T −= Σ wheel.τreact                        # FLong·r from the wheels' last Update

# 3. brake application
if ω != 0:
    canStop = |T| <= B
    T += sign(ω)·B
else:                                       # static brake holds up to B
    T = T >= 0 ? max(T − B, 0) : min(T + B, 0)

# 4. effective inertia
I = engine ? g²·engine.AngInertia + 0.02 : carSim.Mass · 0.005

# 5. contact for every wheel (ray, suspension, surface) — see 02
for w in wheels: w.contact(T, ...)

# 6. limited-slip bias (pairs: wheels[0]/[1], [2]/[3])
if wheelCount >= 2 and |ω| >= 0.001:
    maxB = |ω| >= 50 ? 1.03 : (1.25·(50 − |ω|) + 1.03·|ω|)/50
    target = bias + Σpairs (left.τreact − right.τreact) / (AngInertia · ω)
    bias = 0.1·(9·bias + clamp(target, 1/maxB, maxB))
else:
    bias = 1

# 7. integrate (implicit in the "AngInertia" term)
K = AngInertia
ωnew = ω + dt·(−T) / (I + dt·K)
if canStop and sign(ωnew) != sign(ω): ωnew = 0          # brakes stop exactly at zero

# 8. engine coupling and limiter
if engine:
    e = −g·ωnew
    if e < 0:            ωnew = 0                         # never drive the engine backwards
    elif e > engine.ωmax: ωnew = −engine.ωmax/g ; engine.ωe = engine.ωmax
    else:                 engine.ωe = e
ω = ωnew

# 9. hand the speed to the wheels, then update them (tyre forces)
wheels[even].ω = bias·ω ; wheels[odd].ω = ω/bias ; (odd count: last wheel = ω)
for w in wheels: w.Update()
```

What this means for the feel:

- **`AngInertia` is an implicit tyre stiffness, not an inertia.** It
  enters as `I + dt·K`. With retail values (2 000–30 000) and
  `dt = 1/60`, `dt·K` is 33–500 kg·m², an order of magnitude more than
  any real wheel and more than the free wheel's own `I = 0.005·m`. Wheel
  spin therefore changes slowly and smoothly: a locked rear wheel
  released at 30 m/s takes most of a second to roll up again (an
  estimate from the formula at full tyre force, not a measurement), so a
  handbrake slide keeps sliding after the button is let go, and a
  burnout builds rather than flashing up. Because it is multiplied by
  `dt`, the effect is frame-rate dependent.
- **The tyre's reaction torque is one step late** (`τreact` comes from
  the previous `vehWheel::Update`).
- **Engine inertia is reflected through the gear** (`g²·I_e`); in first
  gear it dominates (`22.8²·1 ≈ 521 kg·m²` on the Beetle) and the
  coupling term `g·I_e·(g·ω + ωe)/dt` synchronises engine and wheels
  after a shift or a clutch close in one step.
- **The "differential" is kinematic.** The two wheels of an axle spin at
  `ω·b` and `ω/b`, `b` creeping toward the torque imbalance at 10% per
  step and capped so the left/right speed ratio `b²` stays within 1.56
  at standstill and 1.06 above 50 rad/s (≈ 17 m/s on a 0.34 m wheel).
  At speed a driven axle is nearly a spool.
- **Rolling resistance** is the constant 50 N·m per train (150 N·m on a
  two-wheel-drive car, 50 on a 4WD); there is no speed-dependent rolling
  drag.
- **No traction control, no ABS.** Wheelspin and lock-up are limited only
  by `AngInertia` and the brake/drive torques.

### The dead piecewise solver

The routine is written as a piecewise-linear solver: each wheel reports
a breakpoint spin rate (`wBreak`, where it would cross its optimum
slip) and a stiffness change, and the loop (up to 20 passes) was meant
to integrate to the nearest breakpoint, switch slopes and continue. In
the shipped code the stiffness outputs are written then overwritten
with 0 by the contact routine, and both breakpoint searches compare the
wrong way (the "nearest above" search starts at `1e9` and keeps only
larger values; the "nearest below" one starts at `−1e10` and keeps only
smaller ones), so no real breakpoint is ever chosen and the loop always
exits after the first pass. Step 7 above is the whole integration.

## Brakes

- Per wheel: `brakeTorque = handbrake·handbrakeMax + brake·brakeMax`
  with `brakeMax = BrakeCoef·StaticFric·r·L` (02).
- Summed per train, multiplied by `BrakeDynamicCoef` while turning or
  `BrakeStaticCoef` (×1.2) when stopped, plus the 50 N·m drag.
- A wheel locks when the brake exceeds what the road can push back:
  with the tyre at `μk·μm·N·r`, the threshold is roughly
  `BrakeCoef·1.2 > SlidingFric·matFric/StaticFric`. Retail
  `BrakeCoef` is mostly 0.4–1.0 (0.06 on the 4×4's front, 1.76 on the
  Eldorado's rear), around that line — foot brakes are close to the
  lock limit — while `HandbrakeCoef 2.0` (and the right side's
  default 1.0, 02) lock comfortably.
- The handbrake only reaches the rear wheels (04), and the foot brake
  is released on the rear wheels during a "brake-stand" (04).
