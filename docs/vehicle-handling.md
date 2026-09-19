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
| `Trans.GearChangeTime` | `transmission.shift_time` | seconds |
| `DrivetrainType`, `Axle*.TorqueCoef` | `wheels[].driven`, `drive_share` | |
| `Wheel*.BrakeCoef` / `HandbrakeCoef` | `wheels[].brake_bias` / `handbrake_coef` | normalised against the axle sum |
| `Wheel*.SteeringLimit` | `steering.low_speed_max_angle`, `steer_scale` | |
| `Wheel*.StaticFric` / `SlidingFric` | `tires.lateral_grip`, `slide_fraction` | scaled, see below |
| `Wheel*.SuspensionExtent` + `Limit` | `suspension.travel` | |
| `Aero.Drag` / `Down` | `aero.*` | × frontal area × air density |
| `bound/<id>_bound.bnd` | `collider_points` | hull verts, underside reshaped |
| `geometry/<id>_whlN.mtx` | `wheels[].position`, `radius` | hardpoints raised for sag |

## The adaptations, and why

MM2's numbers were written for MM2's solver. Fed straight into a rigid-body
engine with raycast suspension, several of them describe a car that cannot
be driven. Each adaptation below is a deliberate departure, not an
approximation of something we failed to decode.

The claims here are measurements. `mm2-inspect handling` audits the whole
roster against them:

```sh
cargo run -p mm2_inspect -- handling <install>          # every ready car
cargo run -p mm2_inspect -- handling <install> vpbug    # one car
cargo run -p mm2_inspect -- handling <install> --strict # nonzero on problems
```

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

The countersteer assist is applied *after* this cap on purpose: a car
already sideways is not the steady-state corner the cap models, and
catching it needs more lock than that corner would.

Imported cars get `1.3` — enough over the limit to provoke a slide.

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
