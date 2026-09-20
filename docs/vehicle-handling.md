# Vehicle handling

How MM2 tuning data becomes a drivable car, and which parts of that are
faithful to the retail data versus deliberately not.

The conversion lives in exactly one place, `mm2_content::convert` — a
parser crate never produces runtime handling, and `mm2_app` never patches
it after the fact. Every value the converter emits is tagged in a
`ConversionReport` as **imported**, **derived**, **adapted**, **defaulted**
or **unsupported**, and that audit trail is the authority on any single
field:

```sh
cargo run -p mm2_inspect -- car <install> vpbug --json | jq .conversion
```

What follows is the shape of the mapping and, more usefully, the reasoning
behind the adaptations.

## Conventions

- Vehicle space is an **identity** map of MM2 coordinates: both use `-Z`
  forward, so no axis is mirrored for vehicles. (The city importer *does*
  mirror Z; vehicles do not.)
- Metres, kilograms, seconds, radians, newtons.
- The front axle is at the smallest `z`.
- "Ground" means the plane the contact patches settle on once the springs
  have taken the car's weight — not the model origin. `HandlingMetrics`
  solves for it; nothing should assume it is `y = 0`.

## What comes across directly

| MM2 source | Runtime | Notes |
| --- | --- | --- |
| `Mass` | `mass` | kg, verbatim |
| `InertiaBox` | `inertia` | box dimensions → principal tensor |
| `CenterOfGravity` | `center_of_mass` | offset from the bound centre |
| `Engine.MaxHorsePower` / `OptRPM` | `engine.max_power_w` / `peak_power_rpm` | 745.7 W/hp |
| `Engine.IdleRPM` / `MaxRPM` | `engine.idle_rpm` / `redline_rpm` | verbatim |
| `Trans.Low`/`High`/`Reverse`/`GearBias` | `transmission.gear_ratios` | per-band top speeds → ratios at `OptRPM` |
| `Trans.GearChangeTime` | `transmission.shift_time` | seconds, capped — see below |
| `DrivetrainType`, `Axle*.TorqueCoef` | `wheels[].driven`, `drive_share` | |
| `Wheel*.BrakeCoef` / `HandbrakeCoef` | `wheels[].brake_bias` / `handbrake_coef` | normalised against the axle sum |
| `Wheel*.SteeringLimit` | `steering.low_speed_max_angle`, `steer_scale` | |
| `Wheel*.StaticFric` / `SlidingFric` | `tires.lateral_grip`, `slide_fraction` | scaled, see below |
| `Wheel*.SuspensionExtent` + `Limit` | `suspension.travel` | |
| `Aero.Drag` / `Down` | `aero.*` | × frontal area × air density |
| `bound/<id>_bound.bnd` | `collider_points`, `striker_points` | hull verts, underside reshaped for world contact; unmodified copy kept as the prop-strike surface |
| `geometry/<id>_whlN.mtx` | `wheels[].position`, `radius` | hardpoints raised for sag |

## The adaptations, and why

MM2's numbers were written for MM2's solver. Fed straight into a rigid-body
engine with raycast suspension, several of them describe a car that cannot
be driven. Each adaptation below is a deliberate departure, not an
approximation of something we failed to decode.

The claims here are measurements, from two instruments. `mm2-inspect
handling` solves what a config *implies* — rollover margin, ride height,
suspension frequency and damping:

```sh
cargo run -p mm2_inspect -- handling <install>          # every ready car
cargo run -p mm2_inspect -- handling <install> vpbug    # one car
cargo run -p mm2_inspect -- handling <install> --strict # nonzero on problems
```

`drive_probe` runs the simulation and reports what the car *does* — how
evenly it accelerates through its gears, how hard it can actually corner
at a given speed, and, over real city geometry, every point where its body
touched the world:

```sh
cargo run -p mm2_app --example drive_probe -- <install>
cargo run -p mm2_app --example drive_probe -- <install> vppanoz --city sf
```

Use the second whenever a handling claim depends on what the solver does
rather than on what the numbers say. Two of the adaptations below were
found only by running it.

### Roll resistance

**Every stock car tips before it slides.** Retail geometry carries the mass
about as high as the track is wide — the Beetle's static rollover threshold
is 0.98 g, the London Cab's 0.63 — while `StaticFric 3.0` maps to tires
worth 1.65 g. The whole roster audited between 0.28 and 0.86 on
`rollover_margin`, meaning a merely committed corner ends with the car on
its roof.

Tire grip acts at the contact patch, a full centre-of-mass height below the
mass it is turning, and that lever is what does it. Real suspension resists
it geometrically through the roll centre; `assists.roll_resistance` is the
arcade form of the same idea, raising where lateral force is applied toward
the centre of mass. Raising it *along the contact normal* leaves the yaw
moment untouched — the car corners exactly as hard, it just stays down —
and measuring against the contact normal rather than world up keeps banked
road correct.

Imported cars get `0.85`, which puts the roster at 1.87–5.75.

### Grip-limited steering

Stock steering locks ask for cornering no tire can deliver: at its
high-speed limit the Mini Cooper's lock demands 33 g and the double-decker
bus 64 g. An angle that far past grip does not corner harder, it saturates
the front tires and ploughs, so full lock at speed gave no feedback at all
until the car let go.

`steering.grip_limit` caps the commanded angle at the angle whose
steady-state demand stays inside that multiple of the tires' lateral limit.
Inverting the bicycle model gives a lock falling off as `1/v²` — the shape
real speed-sensitive steering has — derived per car from its own wheelbase
and grip rather than authored. A floor (`MIN_STEER_LOCK`) keeps some
authority at any speed.

That cap must include **the slip the front tires need**, not just the
geometric angle. A tire makes force by slipping, so the front wheels have
to be turned past the path the car is taking before they pull at all;
capping them at the geometric angle leaves a car whose fronts want more
slip than its rears turning the wheel for nothing. Sorting the roster by
front minus rear `OptimumSlipPercent` sorts it exactly by how badly it
steered: the London Cab (0.40 against 0.14) managed 3 deg/s of yaw at
30 m/s and the DB7 (0.20/0.08) 8, while the Mini and Panoz, whose axles
want the same slip, were unaffected. With the understeer term added the
Cab reaches 22 and the DB7 27, and the cars that were already fine do not
move.

The countersteer assist is applied *after* this cap on purpose: a car
already sideways is not the steady-state corner the cap models, and
catching it needs more lock than that corner would.

Imported cars get `1.3` — enough over the limit to provoke a slide.

### Gear changes

`GearChangeTime` is authored at 0.8-1.0 s. Cutting drive for that long is
what a real clutch does, but five of them between rest and top speed turn
acceleration into a staircase — the probe measured the F-350 *losing*
speed during its worst half-second and spending 7.5 s of a run gaining
nothing.

Drive torque now dips to `SHIFT_TORQUE_FLOOR` and ramps back across the
change rather than switching off, and the authored time is capped at
`MAX_SHIFT_TIME`. The shift is still there to feel; the car never stops
pulling.

### Levelling in the air

Not an import either, and the fix for a symptom that looks like something
else entirely. A car cresting a rise at speed leaves it pitched nose-down
and holds that attitude all the way to the ground, so the front of the
hull arrives ahead of the wheels, digs into the road and stops the car
dead. It reads as catching on a seam, and no amount of ground clearance
helps, because the nose is pointed at the road.

`assists.air_control` is the natural frequency in rad/s of a critically
damped return of the car's up axis to vertical — `8.0` means level in
about half a second. It scales by each car's own pitch and roll inertia,
so that figure means the same thing on a Mini and on a fire truck, and its
torque axis is horizontal by construction, so a deliberate spin is left
alone.

### Suspension damping

Damping is set as a fraction of critical, and critical damping depends on
the corner's **mass**. `SuspensionDampCoef` is authored between about 0.01
(London Cab) and 0.1 (most cars); mapped straight through, the low end
lands near ζ = 0.1 and the car pogos off every road seam.

The authored value still orders the roster, but inside a band
(`DAMPING_RATIO_MIN`..`MAX`) that keeps all of it drivable.

### Collider underside

An MM2 car bound is the real body shell: a flat floor slung between axles
that sit well inside it, 0.13–0.25 m off the road. The wheels ride over a
road seam or the crown of an intersection without noticing, and then the
body pan catches on it — the Mustang GT cleared an 8° crest.

Each hull vertex is lifted to whatever height its position demands: beyond
an axle, the ramp that overhang has to clear; between the axles, the crest
rising from the nearer axle; everywhere, at least `MIN_GROUND_CLEARANCE`.
The **visual** body is untouched, so a car still looks slammed.

The reshape is confined to world contact. MM2's prop collision is
bound-vs-bound (the `phBound` every `dgBangerData` record and every car
carries), so the lifted floor would let a tall vehicle ride over a
kerb-height prop the authored shell would have touched — a double-decker
over a cone. The unmodified bound is therefore kept on the entity as a
`StrikeBound` and dormant bangers are activated through a shape-overlap
test against it, while roads and bodies still only ever meet the
snag-safe hull.

### Bodywork contact

- `BoundElasticity` (0.3–0.5 on stock cars) drives MM2's own car-versus-car
  impact solver. As a rigid-body coefficient of restitution it means the
  chassis trampolines off every kerb, so it is scaled into `0..0.1`.
- `BoundFriction` runs up to 0.8. As a friction coefficient the panel bites
  a wall rather than sliding along it, which spins the car — capped at 0.3.
  Tires still do all the gripping.

### Grip scale

`StaticFric` runs from 1.2 (City Bus) to 3.0 (most cars), which at the top
end is not a friction coefficient any tire has. It is scaled by
`GRIP_LAT_SCALE` / `GRIP_LONG_SCALE` into a 0.9–1.7 g range, preserving the
ordering between cars.

### Engine curve

MM2 authors power, not torque: `MaxHorsePower` at `OptRPM`. The torque peak
placed at `0.72 × OptRPM` and `1.15 ×` the power-peak torque gives the
curve a plausible shape. Nothing in the retail data constrains it.

### Self-righting

Not from MM2 data at all. A car on its roof has no wheel touching anything,
so no tire or spring force can reach it and the drive is simply over.
`assists.self_right_delay` seconds after it comes to rest inverted, the car
is set upright on its heading and dropped back on the surface beneath it.

## Deliberately unsupported

- `Aero.AngCDamp` / `AngVelDamp` / `AngVel2Damp` — MM2's per-axis angular
  damping. The assist policy (`yaw_stability`, `roll_resistance`,
  `air_control`) covers the same ground with parameters whose meaning we can
  state; the axis order of these fields is also not established.
- `SSSValue` / `SSSThreshold` / `CarFrictionHandling` — legacy steering and
  friction switches.
- `Wheel*.CamberLimit` / `WobbleLimit` / `TireDisp*` / `TireDamp*` — camber,
  wobble and slip-displacement internals with no analog in the tire model.

## Overriding handling

Nothing above is baked in. A TOML file replaces or patches the imported
config:

```sh
cargo run -p mm2_app --bin mm2 -- --mm2-path <install> --vehicle-config car.toml
```

`examples/vehicles/dev-car.toml` is the default config serialised in full,
which is the easiest starting point — and `cargo run -p mm2_vehicle --example
dump_default_config` regenerates it.
