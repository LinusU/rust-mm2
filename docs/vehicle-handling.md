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

The original game's own vehicle model — 19.6 m/s² gravity, the
stick–slip tyre, the drivetrain spin integration, the aero rotational
damping, the gyro assists, the input ramps — is documented in
[research/vehicle-physics/](research/vehicle-physics/README.md), with a
comparison against this implementation in
[09-differences-from-rust-mm2.md](research/vehicle-physics/09-differences-from-rust-mm2.md).
Where this document calls an original mechanism unrecovered or
designed, check there first.

## Current imported-car simulation

Four-wheel imported cars now use `VehicleConfig.original`, populated from
retail tuning. The synthetic dev car and passive trailers retain their
existing configurable model. Imported cars use 19.6 m/s² gravity, authored
centre of mass (`-CenterOfGravity` on every axis), the square-root-five
engine curve, clutch and shaft inertia, per-wheel brake torques,
stick–slip tyre displacements, rear counter-steering, aero angular damping
and gyro yaw torques. Authored axle anti-roll springs and dampers act after
the wheel update, including the original tyre-squash displacement. Live F350
slalom measurements exposed this previously omitted force; axle `TorqueCoef`
is not a drivetrain torque split. Keyboard steering uses each car's `.asnode` rate
ramp and signed-byte quantization; full authored wheel lock stays
available at every speed.
Speed-sensitive human steering preserves the recorder/player cache order:
its parameters use body speed from two fixed steps earlier, as measured
in the original Beetle powerslide.

The application and trajectory probe run at 60 Hz. Vehicle forces from
tick k are cached for tick k+1. Suspension rank-one Jacobians are solved
as a coupled linear/angular velocity increment applied once before Avian
advances the body. Original cars bypass Avian's velocity integrator. Their
four-wheel configuration retains the authored collision hull, including its
underside, and the steering pivot uses the full measured wheel-mesh width
(previously imported as a half width); radius remains a half extent.

The chase camera samples the player's current interpolated root `Transform`
during `Update`. Its `GlobalTransform` has not yet propagated at that point;
sampling it produced visible car/camera jitter in the rendered SF Beetle
launch despite smooth interpolation. The corrected camera shares the body's
render pose. City height measurements and before/after camera timing are in
[the SF investigation](../tools/handling/evidence/sf-beetle-README.md).

Marked static-world contacts use the recovered source effective-mass response:
contact impulses and angular impulses wait for the next body integration,
while penetration pushes change position without adding body velocity.
Wheel point velocities account for the preceding positional push. Hull
`BoundFriction` and `BoundElasticity` remain raw; `OriginalContactMaterial`
carries raw world coefficients. Pair friction is their product and pair
elasticity is `min(product, 1)`. These products also replace Avian's material
combination for pairs of original hulls. Generic dynamic props and unmarked
terrain retain their existing response.

Avian supplies collision geometry and pose integration. For the marked static
contacts, manifold points are temporarily removed before constraint preparation
to suppress Avian's impulse solve, then restored after the solver and before
sleeping. The original response runs after vehicle force evaluation. Its
swept midpoint contact reconstruction is **inferred** from planar measurements:
the swept hit must belong to the same world collider and have a compatible
normal (dot product at least 0.999), unless the solid ray starts inside that collider; otherwise the manifold point is used.
This does not establish exact original collision detection on arbitrary
geometry. The six Avian solver substeps remain enabled for other contacts.
Formula fidelity alone does not establish a match to a driven retail trajectory.

Modern traction control, countersteering, slide recovery, roll/pitch
cancellation and airborne levelling are disabled for imported cars.
Optional automatic flip recovery remains a user setting. The generic
config's engine, tyres and transmission summarize the original model for
AI, telemetry and tooling; `original` carries the actual simulation
parameters. Explicit original tuning takes precedence in overrides;
legacy scalar overrides are propagated when representable.

Record deterministic keyboard-target inputs on a flat half-space with:

```sh
cargo run -p mm2_app --example handling_trace -- <install> vpmustang99 launch 15
cargo run -p mm2_app --example handling_trace -- <install> vpbug turn 15
cargo run -p mm2_app --example handling_trace -- <install> vpbullet powerslide 15 1.0
```

The CSV records every simulated frame's pose, velocity, yaw, RPM, gear,
steering and grounded-wheel count. Scenarios include `launch`, `coast`,
`brake`, `turn`, `handbrake`, `powerslide`, `slalom`, `lift_turn` and
`brake_turn`. The last three exercise repeated steering reversals, throttle
lift during a turn, and braking followed by powered countersteering. CSV
orientation bases also let the comparator measure body roll and pitch.
The powerslide keeps full
throttle, flicks the handbrake at 5.5–5.8 seconds, countersteers at
6.25–7.25 seconds and then recovers with neutral steering. The turn and
handbrake scenarios steer from five to seven seconds. The default
settling period is ten seconds. The optional arguments after duration set
surface friction and settlement ticks: use friction `1.0` when comparing
the generated original-game course.
The cold spawn is model pose `(1200, 1, 1200)` with identity rotation,
matching the generated original-course fixture. The half-space replaces a
giant cuboid whose numerical contact geometry could produce spurious oblique
normals on a nominally flat road. Positions are relative center-of-mass
coordinates. This is a measurement instrument,
not a substitute for recorded original-game evidence.

Race/profile ticks now use 60 Hz. Schema-1 best times are migrated to schema 2
on load without rewriting the source file; older builds cannot read schema 2.
Network protocol 25 rejects older peers: version 24 introduced the retail
model and 60 Hz clock; version 25 adds authored axle anti-roll, corrected
wheel width and the source static-world hull response.
An older peer would predict different motion with the same asset fingerprint. Human drivers
also transmit their auto-reverse preference and optional manual gear, so
the host applies the same pedal rules as their local simulation.

## Historical arcade conversion

The following mapping and measurements describe the previous arcade
model. They still explain the generic dev-car path and the legacy config
fields, but no longer describe the forces used by an imported retail car.

## Conventions

- Vehicle space is an **identity** map of MM2 coordinates: both use `-Z`
  forward, so no axis is mirrored for vehicles. (The city importer *does*
  mirror Z; vehicles do not.)
- Metres, kilograms, seconds, radians, newtons.
- The front axle is at the smallest `z`.
- "Ground" means the plane the contact patches settle on once the springs
  have taken the car's weight — not the model origin. `HandlingMetrics`
  solves for it; nothing should assume it is `y = 0`.
- **A car has at most four physics wheels.** The retail `vehCarSim`
  carries exactly four `vehWheel` slots (front-left/right,
  back-left/right — verified in mm2hook's `vehCarSim::Init`); wheel parts
  `whl4`/`whl5` are "back-back" visuals the original draws as the
  `whl2`/`whl3` matrix plus a stored offset. `build_model` therefore
  flags `whl` index ≥ 4 as `!simulated` followers of `whl(N−2)`
  (`WheelVisual::follows`); the parts stay in the visual model and copy
  the reference wheel's droop, steer and spin, but add no suspension,
  tire or load-sharing corner. Simulating them independently makes the
  rig statically indeterminate — the Moon Rover tripods and porpoises
  on six independent corners. (Even the back-back offsets are
  mm2hook's own extension: it adds the `BackBack*WheelPosDiff` fields
  to `vehCarSim`, so unhooked retail never drew `whl4`/`whl5` at
  all.) Trailer `twhl` parts use
  the decorative-radius rule instead; the retail trailer wheel binding
  beyond four is unrecovered (mm2hook has `TrailerBackBack*PosDiff`
  fields but no stock trailer exercises them).

## What comes across directly

| MM2 source | Runtime | Notes |
| --- | --- | --- |
| `Mass` | `mass` | kg, verbatim |
| `InertiaBox` | `inertia` | box dimensions → principal tensor |
| `CenterOfGravity` | `center_of_mass` | `-x`/`-z` from the model origin; height adapted, see below |
| `Engine.MaxHorsePower` / `OptRPM` | `engine.max_power_w` / `peak_power_rpm` | 745.7 W/hp |
| `Engine.IdleRPM` / `MaxRPM` | `engine.idle_rpm` / `redline_rpm` | verbatim |
| `Trans.Low`/`High`/`Reverse`/`GearBias` | `transmission.gear_ratios` | per-band top speeds → ratios at `OptRPM` |
| `Trans.GearChangeTime` | `transmission.shift_time` | seconds, capped — see below |
| `DrivetrainType`, `Axle*.TorqueCoef` | `wheels[].driven`, `drive_share` | |
| `Wheel*.BrakeCoef` / `HandbrakeCoef` | `wheels[].brake_bias` / `handbrake_coef` | normalised against the axle sum |
| `Wheel*.SteeringLimit` | `steering.low_speed_max_angle`, `steer_scale` | lock × 1.2, see below |
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
rather than on what the numbers say. Several of the adaptations below
were found only by running it.

### Centre of mass

`CenterOfGravity` is not where the mass is. The original places the
model at the inertial origin *plus* `CenterOfGravity`, so in model space
the centre of mass sits at `-CenterOfGravity` — and its static load
split says so outright: `vehWheel::ComputeConstants` gives each wheel
`|z - CenterOfGravity.z| / 2|z|` of the weight, so a positive `z`
offset loads the front (`-z`) axle. (Read from Dummiesman's
`mmclone_v2` port of the binary, which loads wheel pivots and
`CenterOfGravity` through the same z flip — documented, not decompiled
here.)

We read it the other way round for a long time, adding `z` to the bound
centre. Most cars author a few tenths either way and drove regardless,
but the Moon Rover's `+0.4` put its mass 0.35 m *behind* its rear
axle: it rested on its tail with the front wheels in the air, never
reached 100 km/h and wandered 180° off a straight launch. Plan-view
position (`x`, `z`) now follows the original. On the rest of the
roster, `drive_probe` measured the Mustang Fastback's half-second
stall and the Freightliner's 25° launch wander gone, the double-decker
pulling evenly, the London Cab's one-second stall gone with its 93°
launch wander down to 11° (9.5→4.2 s to 100 km/h), and both Minis
1.6 s quicker. The DB7 is the cost: front-driven with 57% of its
weight now on the rear axle, it is 1.2 s slower to 100 km/h and spins
at full lock from 30 m/s, cornering at 1.2 g there against 1.8 before.

Height stays an adaptation. `-y` from the model origin puts every
stock car's mass below its wheel hubs — the Beetle's 0.1 m off the
road, the London Cab's 0.2 m *under* it — which is how the original
kept cars on their wheels, and nothing else here (roll resistance, the
grip scale, the steering cap) is tuned for it. The height is the bound
centre plus the authored `y` offset, and the roll assist below does
the job the low mass did.

### Roll resistance

**Every stock car tips before it slides.** Retail geometry carries the mass
about as high as the track is wide — the Beetle's static rollover threshold
is 0.98 g, the London Cab's 0.63 — while `StaticFric 3.0` maps to tires
worth 2.25 g. Even on the gentler 1.65 g tires of an earlier grip scale
the whole roster audited between 0.28 and 0.86 on `rollover_margin`,
meaning a merely committed corner ends with the car on its roof.

Tire grip acts at the contact patch, a full centre-of-mass height below the
mass it is turning, and that lever is what does it. Real suspension resists
it geometrically through the roll centre; `assists.roll_resistance` is the
arcade form of the same idea, raising where lateral force is applied toward
the centre of mass. Raising it *along the contact normal* leaves the yaw
moment untouched — the car corners exactly as hard, it just stays down —
and measuring against the contact normal rather than world up keeps banked
road correct.

Imported cars get `0.95`, which puts the roster at 3.42–10.54. It was
`0.85` (1.37–4.21), which is what capped the grip scale below: the
roll arm left at `0.95` is a third of the one at `0.85`, so the same
tires tip at three times the lateral load.

### Pitch resistance

Drive and brake force act at the contact patch too, and pitch the car
on the same lever. On ordinary proportions that is squat and dive worth
keeping, so longitudinal force stays at the patch — except on the Moon
Rover, which carries 0.63 m of centre-of-mass height over a 0.86 m
wheelbase. Even standing on all four wheels it reared up under its own
drive, lifted its front wheels within a second, and inside seven
struck its tail hard enough to be thrown round 180°.

The measure that separates it is the **pitch gradient**, radians of
pitch per g of longitudinal acceleration. Load transfer `m·a·h/L`
compresses each axle by its static sag in proportion to the load it
carries, so the body pitches

```
h / L² · (sag_front / share_front + sag_rear / share_rear)
```

per g. The stock roster runs 0.020 (City Bus) to 0.073 (London Cab);
the rover is 0.576, thirteen times the Beetle's: soft springs under a
wheelbase a third of a car's. `h/L` alone
does not separate it: the cap that brought the rover back that way
would have caught most of the roster too.

`assists.pitch_resistance` is `roll_resistance` for pitch: it raises
the point longitudinal force is applied toward the centre of mass
along the contact normal. The converter sets it just high enough to
bring a car's gradient down to `MAX_PITCH_GRADIENT` (0.08 rad/g), so
that rule alone is `0.0` on every stock car but the rover, which gets
`0.82`. `drive_probe` has the rover at 0–100 km/h in 4.2 s, 47.8 m/s,
4° of drift on a straight launch and 1.66–1.95 g of cornering from
10 to 30 m/s. That is quicker to 100 km/h than the other 2500 kg
vehicles (the Light Tactical Vehicle and F-350 take 4.7 s), and it
corners like a car rather than a truck because its authored
`StaticFric 3.0` is a car's tires.

### Wheelies

The pitch gradient is about soft springs; it says nothing about a car
that is simply tipped back by its own drive. Drive force at the contact
patch pitches the car up about the rear axle, and the front wheels
leave the road at `a = g·b/h` — `b` the centre of mass's distance ahead
of the rear axle, `h` its height. A rear-driven car is traction-limited
to roughly `grip · traction_control · g` with all its weight on the
rear wheels, which is exactly where it stands up. The London Cab
(300 hp in 1000 kg, mass 1.28 m up and only 1.04 m ahead of the rear
axle) did: 26° of pitch and the front wheels in the air through the
first three gears. It was hidden behind its launch spin until the
suspension and grip were retuned.

The converter now also computes the lever that keeps the traction limit
`WHEELIE_MARGIN` (1.2) below that threshold, and cancels the rest of it
with the same `pitch_resistance`, taking the larger of the two rules.
Front-driven cars need nothing — their driven wheels unload first.
It touches the Cab (`0.47`), the Freightliner (`0.36`), the double-
decker (`0.14`) and the Audi TT (`0.13`); every other stock car is
untouched, and the Moon Rover's gradient rule is the larger there.
`drive_probe --trace` has the Cab's launch pitch under 2° and its
0–100 km/h at 3.7 s, down from 4.8 s: it no longer spends its thrust
standing up. Braking is the mirror image (a stoppie, about the front
axle) and is *not* covered here; the roster has not shown one.

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
30 m/s and the DB7 (0.20/0.08) 8.

Front minus rear was still too little. It assumes the rear tires sit at
their own peak, which only a perfectly neutral car at the limit does;
every real corner carries some understeer — load transfer, the drive
force a front-driven axle also carries, the yaw damper — so the fronts
slip more than the rears and the cap stopped them short. Cars whose axles
want the same slip got no allowance at all: the Beetle held full lock at
40 m/s with its fronts at half their peak slip, cornering at 0.72 g on
1.65 g tires, and the Audi TT managed 0.27 g. The allowance is now the
front tires' whole peak slip angle, so they can always reach their grip:
`drive_probe` puts the Beetle at 1.32 g and the TT at 1.32 g at 40 m/s,
the fire truck goes from 0.38 to 0.81, and cars that already cornered
well move by a few hundredths.

The countersteer assist is applied *after* this cap on purpose: a car
already sideways is not the steady-state corner the cap models, and
catching it needs more lock than that corner would.

Imported cars get `1.0`. It was `1.3`, a margin to provoke a slide, but
once the cap also allows the front tires their peak slip that margin
counts twice and full lock — which is all a keyboard ever asks for —
ploughs past the grip; at `1.0` the roster corners a little harder at
every speed the cap governs.

Below the speeds the cap governs, the authored lock is the limit. Stock
`SteeringLimit` runs 0.35–0.6 rad, and a keyboard holds full lock
through every city corner, so that lock is the tightest corner a car
can take. Imported locks are scaled by `1.5` (capped at 0.75 rad); it
was `1.2`, and `1.4` before the tire grip was raised cost eight cars
5% somewhere. `drive_probe --turn` from 30 km/h, the speed of a city
corner, has the Beetle turning 97° in a second against 80°, the
Mini 125° against 112°, the DB7 92° against 75°, and the heavy trucks
and buses 7–10° more. Around 50 km/h it is neutral on most cars and a
few (Mustang Cruiser, Panoz GTR-1, Mini) lose 5–12° of it to
ploughing; from 70 km/h the grip cap governs and nothing changes.
Steady-state cornering at 10–20 m/s moves by ±0.2 g in both
directions, which is the noise of that probe, not a trend.

### Traction control

`assists.traction_control` caps each driven tire's drive at a share of
its grip — and of the grip the car's hardest-cornering tire has left,
`sqrt(1 − u²)` for its lateral utilization `u`, not of its straight-
line limit. Lateral force and drive come out of one friction ellipse,
and a keyboard driver holds the throttle through a corner: capped
against the straight-line limit, the drive spent the grip the steering
needed. Flat out on full lock from 50 km/h the Audi TT turned 41° in
1.5 s against 89° holding speed, and the rear-driven Mustang Cruiser
ran from 50 to 90 km/h through the corner and wide. It is the whole
car's worst tire because a rear-driven car's fronts run out first; the
cap uses last step's value, so it lags a step. It fades in between 2
and 8 m/s — at a crawl a few cm/s of creep reads as a tire at its limit
and the car could not pull away. `drive_probe --turn` measures it: flat
out the Beetle now turns 121° from 50 km/h, the TT 110°, the Cruiser
112°, scrubbing speed instead of running wide.

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

### Reverse

The reverse gear is a single band, exactly like the forward bands the
`Trans.Low`/`High` interpolation produces: its authored top is
`Trans.Reverse`, the speed at which the engine sits at `OptRPM` through
the reverse ratio. There is no next gear to upshift into, so past that
point the drivetrain simply stops pulling — the limiter an upshift
imposes on a forward band. Wheel-implied RPM tracks the reverse ratio,
so the torque curve still tapers toward the cut instead of slamming
into it.

An earlier draft ran the gearbox's upshift selector and forward-ratio
RPM tracking while reversing, which let a held brake walk the car
backwards through every forward gear — the probe measured the Beetle
backing up at 45.6 m/s. The single-band reading bounds every car at its
own authored reverse speed (~13 m/s for the Beetle, ~22 for the GTR-1)
and keeps the roster's authored differences.

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

### Slide recovery

Not an import either. Past about 45° of slide both axles are sliding
about equally hard, and nothing in the tire model turns the body back
toward its path: a handbrake flick held a moment too long ran on into a
full spin however the driver steered out of it.

`assists.slide_recovery` is the natural frequency in rad/s of a spring
turning the body back toward the way it is travelling, acting only on
the slide beyond about 10° and only at forward speed. It never acts
while the handbrake is held — held, the handbrake is a spin; released,
it is a slide to catch. It is soft on purpose: it also pulls the body
back toward its *old* path, so every step firmer costs the turn a flick
makes. `drive_probe --handbrake` measures the trade — from 72 km/h a
0.3 s flick exits 44° off its line with it off, 39° at the imported
`2.5`, 33° at `4.0`.

### Authored gyro maneuvers

Unlike the assists above, `.vehgyro` is authored data — every retail
vehicle carries `Drift`, `Spin180` and `Reverse180` rates (and on 17 of
21 records, `Pitch`/`Roll`, all authored 0.0). `VehicleConfig.gyro`
carries the record verbatim; `None` means no record and no assist.

The consumption is a designed reading (DSN-22). The original's
`Update()` is now recovered and works differently — yaw *torques*
proportional to steering and the driven wheels' spin, not a latched
rate (`research/vehicle-physics/04-chassis-and-assists.md`, PHY-10).
Handbrake plus steering while travelling latches a spin that holds the
yaw rate *at least* at the authored rate for as long as the inputs are
held — a tap spins partway, a held one completes ~180°, `Reverse180`
runs the same maneuver backwards as the J-turn. It is a floor, never a
ceiling: the locked rear tires already rotate most cars faster than
their record, and the Beetle's 0.8 rad/s is just the rate it corners at
on full lock, so pinning the rate there made the handbrake scrub speed
without sliding the car. From 72 km/h a 0.3 s flick now slides the
Beetle out to ~40° and exits ~40° off its line; held 0.6 s it spins
the full 180°. `Drift` relieves the yaw damper's
slip term so a drift-authored car holds its slide; `drift = 0` is the
unmodified policy. `Pitch`/`Roll` would right the car per-axis in the
air like `air_control`, but every retail record authors them at 0.0.

### Suspension damping

Damping is set as a fraction of critical, and critical damping depends on
the corner's **mass**. `SuspensionDampCoef` is authored between about 0.01
(London Cab) and 0.1 (most cars); mapped straight through, the low end
lands near ζ = 0.1 and the car pogos off every road seam.

The authored value still orders the roster, but inside a band
(`DAMPING_RATIO_MIN`..`MAX`) that keeps all of it drivable. The band is
`0.55`–`1.0` of critical; it was `0.40`–`0.90`, which left the body
wallowing after every bump and corner.

The spring is set to carry the static load at `SAG_FRACTION` of its
travel. At `0.35` (it was `0.45`) every car's natural frequency is
13% higher — the roster runs 1.19–2.66 Hz instead of 1.05–2.35 — and
the springs keep more of their travel for bumps; the body pitches and
rolls less under load. `drive_probe` sees one effect it can measure:
the London Cab, whose authored damping is the softest on the roster,
no longer launches into a spin (21.8 s to 100 km/h and 156° of wander
before; 4.8 s and 7°), and the level-drop landing leg passes on every
car. Everything else on the roster's acceleration and cornering table
is within noise, so the rest of this is a claim about how the car
feels, not a measured one.

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
`GRIP_LAT_SCALE` / `GRIP_LONG_SCALE`, preserving the ordering between cars.

The lateral scale is `0.9`, a 1.1–2.7 g range: well past a road tire,
because MM2 cars corner far harder than real ones and the cars were
still hard to hold on a corner at 2.25 g — they ran out of grip before
the driver ran out of road. It was `0.75`, and `0.55` before that.
`0.95` was past what the roll assist could hold at `0.85`; with the
assist at `0.95` every car keeps a `rollover_margin` of 3.42 or more
at `0.9`. `drive_probe` puts the Beetle at 2.36 g at 40 m/s (1.96
before).

### Engine curve

MM2 authors power, not torque: `MaxHorsePower` at `OptRPM`. The torque peak
placed at `0.72 × OptRPM` and `1.15 ×` the power-peak torque gives the
curve a plausible shape. Nothing in the retail data constrains it.

### Self-righting

Not from MM2 data at all. A car on its roof has no wheel touching anything,
so no tire or spring force can reach it and the drive is simply over.
`assists.self_right_delay` seconds after it comes to rest inverted, the car
is set upright on its heading and dropped back on the surface beneath it.

## Remaining gaps

- `CamberLimit` and `WobbleLimit` remain parsed visual tuning; wheel camber
  and wobble are not animated.
- Legacy `SSSValue`, `SSSThreshold` and `CarFrictionHandling` switches are
  retained in parsed data. Human steering uses the recovered `.asnode` law.
- Static-world response uses recovered impulse formulas with Avian collision
  geometry and an inferred swept midpoint reconstruction for compatible planar
  contacts. General impact trajectories, collision detection and dynamic-body
  response have not been established as matching the original solver.
- Passive trailers retain the generic suspension model. A clean spawn/reset
  with an aligned hitch does not establish original trailer dynamics.
- Device mappings, mouse cursor interpretation, AI controllers and optional
  recovery policies remain modern application choices. Stock keyboard
  filtering and automatic pedal swapping use the recovered original rules.

## Overriding handling

A full TOML handling config replaces the imported definition while the
model's wheel positions, radii and collision geometry stay pinned:

```sh
cargo run -p mm2_app --bin mm2 -- --mm2-path <install> --vehicle-config car.toml
```

`examples/vehicles/dev-car.toml` is the default config serialised in full,
which is the easiest starting point — and `cargo run -p mm2_vehicle --example
dump_default_config` regenerates it.

For a retail car, `drive_probe <install> <car> --dump-config /tmp/car.toml`
exports its effective config including `original`. Edit the original fields
for suspension, shaft inertia, gearbox or separate wheel tuning. Common
legacy power, mass, shared grip, brake, steering-lock and aero scalar edits
are propagated when the corresponding original fields are unchanged. Unequal
lateral/longitudinal friction edits are rejected because the retail tyre
uses one friction circle. Omitting `original` from a full config selects the
generic model.
