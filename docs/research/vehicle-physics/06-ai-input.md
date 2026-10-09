# AI drivers: how opponents, police and traffic drive

**verified_original** throughout (retail `Midtown2.exe`, Kuna decompile
cross-checked with the disassembly), unless a line says otherwise.
Route planning is in [`docs/research/opponent-ai.md`](../opponent-ai.md);
this document covers the **control layer** — how the AI turns its plan
into the same four inputs a player produces, and what it does to its
car beyond that. Conventions in [README.md](README.md).

**Address note.** mm2hook's shift varies by region here: `0x240` in
`aiVehiclePhysics` (mm2hook `DriveRoute` `0x55A8F0` → ours `0x55a6b0`),
`0x280` in the ambient goals, `0x20` for police and race opponents,
`0x10` elsewhere. Addresses here are ours.

## Same car, same physics

`aiVehiclePhysics::Init` (`0x5591a0`) builds an ordinary `vehCar`
(`0x42be00`) from the player's `tune/vehicle/<car>.vehcarsim` — same
`vehCarSim`, wheels, `vehGyro`, `vehStuck`, `vehCarDamage`. There is no
AI branch anywhere in `vehCar::Update` or `vehCarSim::Update`. AI cars
keep the transmission constructor's **automatic** box. So every
mechanism in 01–04 — 2 g, the tyre model, the drivetrain, the aero
damping and the gyro assists — applies to AI cars exactly as to the
player.

The controller writes four fields once per frame, before physics
(`mmGame::Update` updates its child nodes, then `dgPhysManager`):

| Field | Written as |
| --- | --- |
| `carsim+0x1554` Steering | raw, no smoothing or rate limit |
| `carsim+0x154c` Brake | 0..1 |
| `carsim+0x1550` Handbrake | 0 or 1, state 0 only |
| `carsim+0x2bc` engine Throttle | `maxThrottle` or 0 |

It also writes `carsim+0x28c` (`maxThrottle`) for the stuck tests and
`carsim+0x153c` `CarFrictionHandling` (below). None of the human
pipeline of [05](05-player-input.md) — quantisation, ramps,
auto-reverse, the stopped handbrake — applies.

## The controller — `aiVehiclePhysics::Update` (`0x55a6b0`)

```
carsim.maxThrottleField = maxThrottle
if damage > maxDamage:                                   # totalled
    inputs = 0 ; linearMomentum *= 0.95  (every frame)
    circuit races: repaired in place after 5 s
    return
switch state: 0 drive, 1 backup, 2 off the road network, 3 stop
```

### State 0, drive (`0x55ad10`)

```
if not drivable: if front-left wheel grounded: steer 0, brake 0, throttle 1   # revs on the grid
                 return
aiStuck.update()                                                       # below
if vehStuck.state == 2: state = backup ; linear & angular momentum = 0 ; return
if aiStuck.state == 2:  steer = +1 ; throttle = 1 ; brake = 0 ; vehStuck.state = 0 ; return
plan path ; speed control (sets brake, throttle)                        # opponent-ai.md
d = target − pos                                                         # target: next node of the chosen path
bearing = atan2(d·right, d·forward)            (horizontal)
raw     = bearing · 1.33 · 1.428                (= 1.8992 → full lock at 30.2°)
steer   = clamp(raw, ±1)
handbrake = (Speed > 30 m/s and |raw| > 1) ? 1 : 0
CarFrictionHandling = touchedPlayerLastStep ? 2.0 : 1.0
```

Near a finish that is already behind the nose (within 25 m), the car
brakes fully and keeps going straight.

**Speed control** is binary: throttle `maxThrottle` with brake 0, or
throttle 0 with brake `clamp(v·(v − vc)/(23.76·d), 0, 1)` once that
demand exceeds `cornerBrakingThreshold` (default 0.7); `vc =
sqrt(23.76·R)·cornerSpeedMultiplier`. Details and the bend sources are
in `opponent-ai.md`. **Every frame it brakes for a bend, the car's
angular momentum is multiplied by 0.85.**

### State 1, backup (`0x55b220`, entry `0x55b1c0`)

```
on entry: frames = 0 ; target = path node ; if gear != 0: gear = reverse
bearing = atan2(d·right, d·forward)       (3D)
if |bearing| <= 0.1:  steer = 0 ; rotate the body about its up axis by bearing ; leave
else:                 steer = clamp(−2.857·bearing, ±1)
                      if frames > 65: leave  else: throttle = 0.85, brake = 0, frames++
leave: state = drive (or off-network) ; vehStuck.reset ; both momenta ×0.25 ; throttle 0 ; brake 1
```

### State 2, off the network / State 3, stop

State 2 steers at the next route intersection with gain 1.33 clamped to
±0.75 (no 1.428), speed control as above. State 3 (fallen below
y = −200, or a cop whose perp escaped) steers at the end point with gain
1.33, brake 1, throttle 0.

## Getting unstuck

Two detectors, both armed only after a **collision**:

- **`vehStuck`** (the car's own, 04): the vehicle collision callback
  (`0x4cb340` → `0x4d60b0`) arms it at the impact point. The AI
  overrides its thresholds at init — `TimeThresh 0.5 s` (police 0.75),
  `PosThresh 1 m`, `Rotation 0`. Still within 1 m after the threshold
  with throttle above 0.75 of `maxThrottle` and |steer| > 0.5 → state 2
  → the controller **backs up**. With no wheel on the ground it flips
  the car upright instead.
- **`aiStuck`** (`ai+0x280`, `0x56f820`): after the same arming, within
  0.6 m for 0.3 s with throttle above 0.75 of the cap and |steer| < 0.5
  (pinned head-on) → full right lock, full throttle, and the body is
  yawed in place at 1 rad/s until it has moved 1 m.

(`opponent-ai.md` attributed backup to the second detector; the first
is the one that triggers it.)

## Physics interventions — the complete list

| Where | When | Effect |
| --- | --- | --- |
| `0x55b420`/`0x55b5f0`/`0x55a330` | each frame braking for a bend, kink, junction or finish | angular momentum ×0.85 |
| `0x55ad3b` | entering backup | linear and angular momentum = 0 |
| `0x55b370` | leaving backup | both ×0.25, brake 1 |
| `0x55b2db` | leaving backup aligned | body rotated onto the target (≤ 0.1 rad) |
| `0x55a757` | totalled | linear momentum ×0.95 per frame |
| `0x56f9bc` | `aiStuck` | body yawed 1 rad/s in place |
| `0x55b14e` | every frame | `CarFrictionHandling = 2.0` while touching the player |
| `0x53dc76` | police, every frame | linear momentum ×1.03 while throttle is 1 and Speed < 50 m/s |

**`CarFrictionHandling = 2` halves the car's grip.** The flag is bit
`0x8000` of the car instance, cleared for every mover at the start of
`dgPhysManager::Update` (`0x46894a`) and set in `CollideInstances` on
the other car when one of a colliding pair is the player. The wheel
divides surface friction by it whenever the friction is below 1
([02](02-wheel-suspension-tire.md#grounded)) — and tarmac is 0.9 — so
**an AI car in contact with the player loses half its grip** on every
ordinary surface. This is why opponents and cops can be shoved off
their line.

No other velocity writes, mass changes or grip multipliers exist in the
AI, and **race opponents have no rubber-banding**. The only speed
boost is the police one: ×1.03 linear momentum per frame (at 60 fps
an extra ≈ 1.8·v m/s² — 18 m/s² at 10 m/s) whenever the cop is
flat out below 50 m/s. It is per frame, not per second.

## Police (`aiPoliceOfficer`, Update `0x53dc50`)

The same controller, re-routed every frame to the perpetrator's
current pose (behaviours from the race data: Follow, Ram, Push, Block):

| Behaviour | targetSpeed | finishRadius | cornerMult |
| --- | --- | --- | --- |
| Follow | perp speed + distance − 12.5 | 5 | **2.0** |
| Ram | perp speed + 15 | 0 | 1.0 |
| Push | perp speed (+25 unless within 2 m behind), target ±3 m to the perp's side, alternating | 0 | 1.0 |
| BlockWait | `Mirror` (`0x55a330`): match the perp's *heading*, brake to perp speed − 3 | — | — |

All use `maxThrottle 1`, `cornerBrakingThreshold 0.7`, look-ahead 75.
Behaviour is forced to Follow when the perp is in reverse, slower than
10 m/s, or the cop is backing up. Cops beyond 250 m while idle are not
stepped at all (**inferred** from `DeclareMover`).

## Ambient traffic

**Ambient cars are not physical while they drive.** `aiVehicleAmbient`
(`0x5515e0`) moves along a lane rail by a scalar speed:

```
accel = VehicleAccelFactor                               # 5–8 m/s² per car (random)
if a leader is within reaction distance:
    vL = clamp(leader.speed − 2.5, 0, 999)
    accel = (vL² − v²) / (2·(gap − (SeparationDist + leader.back + my.front)))   # SeparationDist 0.5–3 m
v += dt·accel   (clamped to the target without overshoot)
target = road speed limit + ExceedLimit (+8, +6, +4, +2, +0 m/s cycling per car) (+5 per remaining lane on freeways)
pose = rail point at the travelled distance, pitched/rolled by four corner raycasts
```

**On any contact with a moving body** (`CollideInstances`, `0x469610`)
the instance is attached to one of **32** pooled `aiVehicleActive`
bodies (`0x553200`): a `phInertialCS` with `InitBoxMass` from the
traffic data, initial velocity = its rail speed along its heading, and
four simple spring wheels (preload `mass·19.6/4`, damping on a rate
clamped to ±3 m/s, a capped tyre-displacement friction; **inferred**
detail) — not a `vehCarSim` and not a banger. When the body comes to
rest on ground flat within `normal.y ≥ 0.9` it rejoins its rail;
otherwise it stays a wreck.

## Network cars

Remote players are full `vehCar`s simulated locally from packets that
carry the same byte-encoded inputs as a human's, plus periodic hard
sets of velocity and momentum.
