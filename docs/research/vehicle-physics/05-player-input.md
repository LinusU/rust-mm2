# Player input: device → throttle, brake, steering, handbrake, gear

**verified_original** throughout (retail `Midtown2.exe`, Kuna decompile
cross-checked with the disassembly), unless a line says otherwise.
Conventions in [README.md](README.md).

**Address note.** The mm2hook shift is not uniform in this part of the
exe: `mmInput` sits `0x20` below mm2hook (`GetThrottle` mm2hook
`0x52D950` → ours `0x52d930`), `mmPlayer` is unshifted
(`FilterSteering` `0x404c90`), `mmGame` is `+0x10`/`+0x20`, and the
vehicle code `−0x10`. Every address here is ours, checked by content.

## The path in one picture

```
devices ── mmInput::Update 0x52c990 ─ ProcessStates 0x52cd30 / PollContinuous 0x52d5f0
              bit mask of pressed bindings + analog axis values
recorder node 0x4069e0 (once per frame)
    steer = GetSteering(mmPlayer::FilterSteering)      0x52de70  (keyboard ramp / pad ramp / analog shaper)
    throttle, brake, handbrake = GetThrottle/GetBrakes/GetHandBrake   0x52d930/0x52d9c0/0x52da50
    QUANTISE: steer → i8 trunc(s·127), others → u8 trunc(v·255)       (also the replay format)
mmGame::ApplyPlayerInput 0x414a10
    decode (/127, /255) → vehCarSim Brake/Handbrake/Steering, engine Throttle ; AUTO-REVERSE
mmPlayer::Update 0x405760
    speed-dependent steering parameters for the next frame ; overrides (stopped handbrake, post-race, dead car)
physics: vehCar::PreUpdate (drivable-mode overrides) → vehCar::Update → vehCarSim::Update (04)
```

Frame order follows the asNode tree (`0x4016d0`): the recorder is the
parent of the game manager, the game of the player, and the game
manager runs physics after its children. `mmInput::Update` running
earlier in the same frame is **inferred** (its per-frame caller was not
located). All of this runs **once per frame**; the physics may then
take up to three steps with the same inputs
([01](01-rigid-body.md#the-time-step)).

The ramps below integrate with `dt = datTimeManager::Seconds`, the
**uncapped** frame delta clamped to [1e-4, 0.1] — the frame-time cap
that would limit it to 1/60 is switched off by the game's one
`RealTime(0)` call (`0x40170a`).

## Byte quantisation

Every human input reaches the car through one byte (`0x4069e0`, record
at `0x5dfd28 + 16·frame`):

```
steer    : i8  = trunc(steer · 127)      → car: i8 · (1/127)
brake    : u8  = trunc(brake · 255)      → car: u8 · (1/255)
throttle : u8  = trunc(throttle · 255)
handbrake: u8  = trunc(handbrake · 255)
```

Quantisation is toward zero; a held key is exactly 1.0, the keyboard
steering ramp moves in multiples of 1/127. Network cars use the same
encoding.

## `mmInput` — devices and bindings

Configurations (`InputConfiguration`, `0x6b0cf4`): **0 mouse,
1 keyboard, 2 joystick, 3 gamepad, 4 wheel**. Default driving bindings:

| Config | Steering | Throttle | Brake | Handbrake |
| --- | --- | --- | --- | --- |
| 0 mouse | mouse X | left button | right button | Space |
| 1 keyboard | ← / → | ↑ | ↓ | Space |
| 2 joystick | stick X | stick Y forward (analog) | stick Y back (analog) | button 1 |
| 3 gamepad | stick X | button 1 (digital) | button 2 (digital) | button 4 |
| 4 wheel | wheel X | Y up (analog) | Y down (analog) | button 1 |

**Digital throttle/brake/handbrake are hard 0/1** — no ramp, filter or
dead zone (`0x52d930`…`0x52da50`). Analog ones are clamped to [0, 1]
(throttle only from below). With `PedalsSwapped` set (auto-reverse,
below) `GetThrottle` reads the brake binding and `GetBrakes` the
throttle binding.

Joystick and wheel axes are read through DirectInput with range
[−2000, 2000], mapped linearly to [−1, 1]; the options "Controller
Dead Zone" (default 0.1, 0–0.33) is handed to DirectInput as
`trunc(dz·10000)` — the game applies no dead-zone math of its own.
The mouse spans the whole window width: `x = (2·mouseX/width − 1) /
mouseDiv`; Y is unused.

## Steering

`GetSteering` (`0x52de70`): **negative is left**. Keyboard target is
`−1` (left), `+1` (right) or `0`; **left wins** if both are held.

### Keyboard (and gamepad): the rate ramp — `0x52dad0` / `0x52dc60`

```
cur  = state                                       # mmInput+0x19c (pad: +0x1a0, target = raw stick)
rate = (|target| >= |cur| and sign(cur) == sign(target)) ? DeltaOut : DeltaIn
cur moves toward target by rate·dt, without overshoot
state = cur
return sign(cur) · |cur|^Exponent
```

`sign(0) = 0`, so the **first** step off centre uses `DeltaIn`;
afterwards moving outward uses `DeltaOut`, releasing or reversing uses
`DeltaIn`. The exponent shapes the *output*, not the state.

### Speed-dependent parameters — `mmPlayer::Update` (`0x405760`) and `tune/<car>.asnode`

`DeltaOut`, `DeltaIn`, `Exponent` (and the mouse/joystick/wheel
equivalents) are interpolated every frame from the car's
`tune/<car>.asnode` file (`mmPlayer::FileIO`, `0x406320`) by the
car's speed `v = |v·forward|` from the last physics step:

```
SpeedSensitive == 2:  t = clamp(v, SpeedBaseLow, SpeedBaseHi) / (SpeedBaseHi − SpeedBaseLow)
                      (== 0: t = 0 ; == 1: t = 1)
X = X_Lo + (X_Hi − X_Lo)·t
```

**Quirk:** the clamped speed is not offset by `SpeedBaseLow`
(`FDIVRP` at `0x40588f`), so with the retail `5 / 44.6 m/s` the factor
runs from 0.126 at rest to 1.126 above 44.6 m/s — the "Lo" tuning is
never quite reached and the "Hi" tuning is overshot by 12.6%.

Keyboard/gamepad values in the retail `.asnode` files (1/s; `F` =
exponent):

| Car | OutLo | InLo | F Lo | OutHi | InHi | F Hi | Speeds |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `vpmustang99` | 2.5 | 5 | 1.2 | 1.0 | 5 | 1.2 | 5 / 44.6 |
| `vp4x4`, `vpauditt`, `vpbullet`, `vpbus`, `vpford`, `vpmoonrover`, `vppanozgt`, `vpsemi` | 2.573 | 5 | 1.2 | 2.529 | 5 | 1.2 | 5 / 44.6 |
| `vpbug` | 2.573 | 5 | 1.2 | 0.8 | 5 | 1.2 | 5 / 44.6 |
| `vpvwcup` | 2.573 | 5 | 1.2 | 0.5 | 5 | 1.2 | 5 / 44.6 |
| `vpcab` | 1.5 | 5 | 1.2 | 0.5 | 5 | 1.2 | 5 / 44.6 |
| `vpcaddie` | 2.0 | 5 | 1.0 | 1.8 | 5 | 1.1 | 5 / 44.6 |
| `vpcoop` / `vpcoop2k` | 1.065 / 1.0 | 4 | 0.4 | 1.0 | 5 | 2.0 | 5 / 44.6 |
| `vpcop` | 2.0 | 5 | 1.3 | 1.2 | 5 | 1.3 | 5 / 44.6 |
| `vpdb7` | 2.5 | 5 | 1.2 | 2.0 | 5 | 1.2 | 5 / 44.6 |
| `vpddbus` | 2.0 | 2.5 | 2.0 | 1.0 | 1.5 | 1.0 | 5 / **100** |
| `vppanoz` | 1.0 | 5 | 1.2 | 0.3 | 5 | 1.2 | 5 / 44.6 |
| `vpcentury`, `vpdune` (no file — `mmPlayer` defaults) | 3.5 | 2.5 | 2.0 | 2.5 | 1.5 | 1.0 | 5 / 100 |

So **the original's speed-sensitive steering lives here, in the input
layer, as a slower wheel-turning rate** — the lock itself is never
reduced for a keyboard player. On the Beetle the wheel reaches full
lock in ~0.4 s at a standstill and ~1.3 s at 40 m/s, and snaps back to
centre at 5/s at any speed. The `Exponent` (1.2 on most cars) makes
small inputs gentler while the key is first pressed.

### Mouse, joystick, wheel — `mmPlayer::FilterSteering` (`0x404c90`)

The interpolated parameters (same `t`) are a divisor
(`*Sensitivity*` × the options "Steering Sensitivity", 0.5–2.0,
default 1.0), an exponent (`*SteerFilter*`) and, for joystick/wheel,
an approach rate pair:

```
mouse:       out = sign(x)·|x|^mouseExp            (x already divided by mouseDiv)
joy / wheel: a = |x|
             pegged = a > 0.99 ? pegged + dt : 0
             v1 = (div <= 1 or a <= 0.99) ? clamp(x/div, ±1) : sign(x)      # snap to full when at the stop
             if not App or pegged == 0:
                 state = v1 ; out = sign(v1)·(|v1|·div)^exp / div
             else:                                   # ramp while the stick is held at its stop
                 rate = AppApp·pegged + ((|v1| >= |state| and same sign) ? Out : In)
                 state → v1 at rate·dt ; out = sign(state)·|state|^exp
             return clamp(out, ±1)
```

With the Mustang's `JoySensitivity 2.5`, the stick gives up to ~40% of
lock across its travel; only holding it hard against the stop snaps
toward full lock, ramping in over ~0.25 s. Full per-car values for the
mouse/joystick/wheel fields are in the `.asnode` files (tokens:
`MouseSensitivityLow/Hi`, `MouseSteerFilterLow/Hi`,
`JoySensitivityLow/Hi`, `JoySteerFilterLow/Hi`, `JoyApp`,
`JoySteerApproachOut/InLo/Hi`, `JoySteerAppApp`, and the `Wheel*`
equivalents; `mmPlayer` defaults in the constructor `0x4033d0`).

## `mmGame::ApplyPlayerInput` (`0x414a10`) — writing the car, auto-reverse

```
car.Brake     = brake
car.Throttle  = throttle                 (network sessions: capped by a handicap table 1.0/0.9/0.81)
car.Steering  = steer                    (mmPlayer::SetSteering)
car.Handbrake = handbrake
if transmission is automatic and the Auto Reverse option is on (default on):
    if gear >= 2 and Speed < 5.0 and Brake > 0.8 and Throttle < 0.1:
        PedalsSwapped = 1 ; gear = reverse                  # SetReverse
    elif gear == 0 and Throttle < 0.8:                       # the (swapped) reverse pedal let go
        PedalsSwapped = 0 ; gear = first                     # SetForward
```

`Speed` is the unsigned forward speed, so the brake must be held at
≥ 80% below 5 m/s (18 km/h). In reverse the brake key drives the car
backwards; releasing it below 80% drops straight back into first — no
speed condition, no hysteresis. With a manual box, or Auto Reverse
off, the REV key toggles reverse/first directly.

Gear events (`0x414670`): **TRANS** toggles automatic/manual (and
clears the pedal swap); **UPSH**/**DWNS** shift only in manual
(`vehTransmission::Upshift`/`Downshift`, 03); **REV** clears the swap
and sets `gear = (gear != 0) ? 0 : 2` directly.

## `mmPlayer::Update` overrides (`0x405760`, after the above)

```
if post-race:          Throttle = 0 ; Brake = 1 ; Steering = −1     # stop with full left lock
if damage at maximum:  Throttle = 0 ; Steering = 0 ; Brake = 0      # a wrecked car coasts
if SpeedMPH < 4 and Throttle == 0:  Handbrake = 1                   # stopped hold
```

The last one matters for feel: **below 4 mph with no throttle the
handbrake is applied for you** — the car never creeps, rolls back on a
slope, or drifts while the player is off the pedals near a stop. (It
also arms the gyro's handbrake-spin flag, but at that speed the
driven-wheel spin it multiplies is near zero.)

An auto-flip in `mmPlayer` (`0x404920`) is gated on a car field that
nothing in the exe ever sets, so it is inert (**inferred** from an
exhaustive search); the flip recovery that does run is `vehStuck`
([04](04-chassis-and-assists.md#vehstuck-0x4d6130--getting-unstuck)).

## Drivable modes

While the car is not drivable (start countdown, cut-scenes)
`vehCar::PreUpdate` overrides these inputs every physics step — mode 1
keeps steering and throttle live in neutral with the brake on
([04](04-chassis-and-assists.md#drivable-modes--vehcarsetdrivable-0x42c2b0--preupdate-0x42c470)).

## What a human gets that the AI does not

Byte quantisation, the steering ramps/shapers, auto-reverse and the
gear events, the stopped-hold handbrake, the post-race and dead-car
overrides. Everything downstream of `vehCarSim`'s input fields — the
speed-sensitive steering scale, rear counter-steer, the handbrake
split, the brake-stand, Ackermann, the **gyro assists** and the
automatic gearbox — is shared with AI cars. There is no traction
control, ABS or stability control anywhere on the input path.

## Replication recipe

```
per frame:
    params = lerp(asnode Lo, asnode Hi, clamp(speed, Lo, Hi)/(Hi − Lo))     # last frame's speed
    keyboard: target = left ? −1 : right ? +1 : 0
              rate = (|target| >= |s| and sign(s) == sign(target)) ? DeltaOut : DeltaIn
              s = approach(s, target, rate·dt) ; steer = sign(s)·|s|^Exponent
    pads/mouse: FilterSteering as above
    q = (trunc(steer·127), trunc(brake·255), trunc(throttle·255), trunc(handbrake·255))   # swap pedals if PedalsSwapped
    car.{Steering, Brake, Throttle, Handbrake} = q / (127, 255, 255, 255)
    auto-reverse (automatic only) ; overrides (post-race, dead, stopped-hold)
then physics (01–04).
```
