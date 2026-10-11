# City handling calibration: keyboard routes and skid audio

**Measured routes complete, 11 October 2026.** All eight native replays and
fresh original-game recordings have been reviewed for physical contamination.
This document records their measured agreement and remaining discrepancies;
it does not certify the whole roster, city driving, collisions or native
audio output as equivalent. The evidence package is available below; quality
gates have passed; residual city trajectory drift remains unresolved.

The canonical eight-case table below uses ordinary native runs **after the
source-initialized MPH conversion constant correction**. The earlier baseline
and its controlled diagnostics remain separately labeled in the residual
discussion. The source-fed kernel audits retain their explicitly recorded
input/provenance constants; they do not certify closed-loop trajectories.

## Scope and reproducibility

The comparison uses retail `Midtown2.exe` under BottleShip and the native
production physics on actual San Francisco/London PSDL collision geometry. The
native harness calls `city::load_city` and `VehiclePlugin`; it includes
production authored road materials, ground internal-edge handling, body
contacts and static INST props. It does not implement traffic/pedestrians,
dormant-prop activation/breakaway, recovery or rendering. A route contacting
such a prop cannot establish comparable source/native behavior through this
harness. See [the harness contract](../../../tools/handling/city-traces.md)
and [the implementation](../../../crates/mm2_app/examples/city_handling_trace.rs).

Each route contains 1,800 recorded ticks at 60 Hz, after 600 handbrake-held
settlement ticks. Source captures may reuse a warm loaded engine; a 600-tick
settlement is not proof of a cold start. Record the actual settled RPM/gear
and cold/warm provenance (unknown if unavailable), rather than inferring
startup history from the route. The native harness starts its independent
engine state cold, so compare the measured settled states.

The frozen JSON carries per-car keyboard events: arrows for
steering/throttle/brake and Space for handbrake. Held buttons persist until
their release event. Offline native waypoint planning was used only to author
candidate schedules; final native and original runs replay the same explicit
events without a waypoint controller. A clean native replay must remain finite
and avoid props, buildings and body-contact impulses. All-contact sidecars
include speculative/negative-penetration contacts: a zero positive-depth
aggregate alone does not establish that a route is clear of obstacles.

The initial pose in a route is the **model origin**, followed by yaw in
degrees about positive Y. The source instrumentation converts it to COM with
the rotated `−CenterOfGravity`; inserting model coordinates directly into the
inertial coordinate system changes the initial suspension state. The
original's room must also match the verified road polygon. Native collision
queries resolve the geometry spatially.

The historical source JSON field `forward` stores **AGE row 2/back**
(`vehCarSim+0x84`), not the vehicle's forward direction. Vehicle forward is
its negative. Body torque reconstruction uses the recorded vector unchanged as
the back axis; substituting its negative reverses the axle torque axis.

Original row `n=0` is settled pre-input. Original row `n=k+1` aligns with
native CSV `frame=k`, both post-step. Comparisons use absolute COM in world
axes; a relative-origin plot can conceal an initial placement error. Native
CSVs retain basis/angular velocity, wheel forces/bristles/ground flags,
contact normals/materials, implicit suspension matrices, delayed body impulses
and push corrections. No numeric values are rounded for packaging.

## Verified byte decoding

**verified_original.** Recorder steering is `trunc(s*127)` as a signed byte;
pedals use `trunc(v*255)` as unsigned bytes. The getter decodes by multiplying
with a stored **f32 reciprocal**, rather than floating-point division:

| Getter operation | Source instruction | Constant address | f32 bits |
| --- | --- | --- | --- |
| Steering byte × reciprocal of 127 | `0x4ccc4f`, `FMULS` | `0x5af484` | `0x3c010204` |
| Pedal byte × reciprocal of 255 | `0x4ccbf1`, `FMULS` | `0x5af480` | `0x3b808081` |

All 3,602 sampled original steering floats matched reciprocal multiplication
bit for bit; division differed by one ULP in 244 samples. The distinction is
small but matters when a later tire/contact threshold is sensitive to its
history. The port's input helpers now preserve this operation; explicit bit
assertions for nine steering bytes and three pedal bytes distinguish it from
division. See [player input](05-player-input.md#byte-quantisation) and [the
native helpers](../../../crates/mm2_vehicle/src/player_input.rs). Addresses
refer to the researched executable; the recorded London capture's SHA256 is
`93afb6c00be3d3b12a6e5d88d8e4f711a13f5a4100dbdfab77943c30083f98a2`. They must
not be transplanted to a different executable without verification.

## Verified skid quantity, classes and gain

**verified_original.** `vehSurfaceAudio` at `0x4de2b0`/`0x4de3e0` takes
maximum wheel `+0x220` (`slipVisual`) across all four wheels. Tire-force
utilization is a different quantity: a sticking tire can have high force
demand while the source skid quantity remains zero. The arcade fallback
retains its designed utilization proxy. This changes the quantity carried by
native skid contact metadata, with the existing wire class/slip fields and
layout retained.

The source front-wheel material classes survive a missed contact ray (wheel
`+0x268`). `SurfaceChanged` at `0x4de520` ignores the rear wheels: retain the
shared class while either front still reports it; after both leave, select
wheel 0's class (`0x4de354..0x4de378`). An invalid table index falls back to
class zero. Sampling only the final state of several physics steps can miss an
intermediate shared-class commitment, so the native snapshot advances class
retention on the physics clock.

The loader stores `MaxSkidVolume−MinSkidVolume` at data `+0x28`
(`0x4e0378..0x4e0384`). The mixers use the single global ramp
`v=min+q*(max−min)` (`0x4dfe06..0x4dfe17`, `0x4e0006..0x4e0017`), not a ramp
restarted at each band's minimum. Every inclusive covering band is visited
(`0x4dfe00`/`0x4e0000`): adjacent bands can both play at a shared boundary.
Zero skid quantity produces no native skid request. The source feeds slip
quantity even when a table header describes min/max “speed”.

The source node setter `0x5a27e0` clamps normalized volume to `[0,1]`. The
DirectSound callback `0x5a4260` then applies:

```
db_hundredths = trunc((v − 1) * 10000)
linear_amplitude = 10 ** (db_hundredths / 2000)
```

Thus normalized volume 0.88 is −12 dB, about 0.25118864 linear amplitude.
Sending 0.88 directly to Bevy would produce a substantially louder signal. The
native requested mix uses the converted amplitude; user master/effects bus
gain is applied once at voice creation and on later sink updates.

**Measured phase.** Source DirectSound playing windows occur one 60 Hz tick
after the sampled current wheel skid quantity. Native skid snapshots read the
previous simulation tick before the next `PhysicsStepSystems::First`,
including class selection; the render mixer consumes that snapshot. Remote
replicas consume already delayed authority contact metadata and do not
introduce another delay. Ordinary headless audio sidecars record requested
production selection/gain/pitch without an `AudioSink` or output device. Those
sidecars establish requested playback, not backend onset or audible playback
quality. The optional live-backend runs below measure sink state separately.

The fresh London Beetle comparison contains six source backend bursts. All six
match native requested onset **and stop** to zero ticks (one tick is 1/60 s),
with no unmatched windows and no active-band mismatch across 1,800 compared
frames. Same-band amplitude MAE is 2.16×10⁻⁶ and maximum error 9.49×10⁻⁵; the
native gain-law check has maximum error 7.32×10⁻⁹. These are measured
source-backend-versus-native-request results. The native backend was evaluated
separately in the following runs.

## Live native audio backend checks

The optional real-time harness uses the production Bevy audio plugin and
records attached `AudioSink` state as well as requested mixes. After the
stop/reset correction, two **pre-MPH-correction baseline** live runs match
the original DirectSound positive-gain windows exactly. These backend checks
are distinct from the current requested-mix comparisons below:

| Case | Positive-gain windows | Positive-gain frames | Start/stop error | Active-band mismatches |
| --- | --- | --- | --- | --- |
| SF Mustang Fastback | 14 | 38 | 0 ticks | 0 of 1,800 frames |
| London Beetle, including handbrake | 6 | 44 | 0 ticks | 0 of 1,800 frames |

Each positive-gain native frame has an attached, nonempty, unpaused sink.
There are no unmatched windows. Sink gain equals the requested native gain in
these samples; the remaining source/native amplitude differences are those
already present in the wheel skid quantity. These results are from
`sf-vpbullet-live-reset-comparison.json` and
`london-vpbug-live-reset-comparison.json`. They establish actual backend state
and playback scheduling. Device samples were **not recorded**, so they do not
establish the sound reaching the speakers, perceptual quality or equivalence
of the audible waveform.

The source stops a released voice and resets its buffer cursor with
`Stop`/`SetCurrentPosition(0)` in `0x5a4210`, `0x5a4235` and `0x5a4245`.
`NativeSkidPlayback` now performs the same release/reset transition. The
looped Rodio buffered decoder refuses seeking, so the native implementation
stops and removes the old sink, then queues a fresh sink at sample zero. An
inactive band queues paused; a global pause preserves the active voice's
position rather than resetting it. The production post-update ordering applies
voice gain and pause/reset before Bevy queues playback.

All eight ordinary native physics CSVs remain byte-identical after this audio
lifecycle change. This is an audio correction, with no measured change to
those physics replays.

## Fresh-load and contamination lessons

**Measured capture failure.** Loading a new cruise can restore original
traffic/cops defaults even after the menu density controls were changed. Fast
or held UI clicks can also repeat and select a different vehicle. A loaded car
fingerprint, rendered menu preview or menu density value alone does not
establish the active cruise configuration. Fresh source capture must verify
the selected live tuning and city, settle the loaded car, and check traffic
controls at the launch boundary. Actor/body impulse telemetry remains
necessary to detect contamination after launch.

The earlier London Beetle recording collided at sample 408 near COM
`[-241.574,-0.022,708.308]`, with body impulse approximately
`[3017,246,-1516]` while the native route was clear. Before that collision,
position error was at most 5.24 cm. That run cannot establish later handling
error. A London Panoz recording also contains contamination at sample 358; that
recording is rejected, and its fresh replacement is compared below. Do not
splice a clean prefix into a claimed clean 30-second trajectory. Capture
settings and rejected runs remain provenance, rather than excuses to discard a
genuine clean handling discrepancy.

## Eight frozen routes

The requested Fastback is **`vpbullet`**; `vpford` is the F350 and is not one
of these eight cases. Native case IDs and route identities are:

| City | Beetle | Panoz Roadster | Mustang Fastback | Mini Cooper |
| --- | --- | --- | --- | --- |
| San Francisco | `sf-vpbug` | `sf-vppanoz` | `sf-vpbullet` | `sf-vpcoop` |
| London | `london-vpbug-hb` | `london-vppanoz-hb` | `london-vpbullet-hb` | `london-vpcoop-hb` |

SF starts at model `[-86,1,-330]`, yaw 180°, room 163. It travels north
through the waterfront streets and curving promenade, roughly through
`[-69,-120]`, `[-48,-70]`, `[-9,-5]`, then toward `[36,80]`. It combines
sustained speed, gentle corners, throttle lifts, braking bursts and
countersteering. Beetle, Panoz and Fastback SF schedules do not use handbrake.
The final Mini route keeps the first 1,400 ticks of its waterfront drive, then
releases throttle/steering and holds service brake plus handbrake; service
brake releases at 1,490 while handbrake remains held. This avoids the dormant
palm proximity of the superseded post-correction route. All routes still record
1,800 ticks. This is not a claim of a sharp 90° intersection test.

London starts at model `[-231,1,650]`, yaw 180°, room 791. It runs north
toward `[-231,708]`, west through `[-347,732]`, southwest toward `[-443,703]`,
then south past `[-471,627]` toward `[-450,567]`. All four frozen London
schedules hold handbrake on frames 320..337, releasing at 338 during the first
corner. Other event timings differ per car because the schedules were authored
to clear the same street geometry.

London's modest street slide is distinct from the earlier controlled-flat
powered handbrake flick/countersteer tests. During native route authoring,
peak body slips were approximately Beetle 12.47°, Panoz 10.95°, Fastback 5.93°
and Mini 8.59°. It does not exercise the full 50°–60° slide range of the
earlier flat powerslide captures. See the controlled evidence under
[tools/handling/evidence](../../../tools/handling/evidence/).

Frozen route **raw-byte** SHA256 identities (formatting is part of the hash):

```
sf-vpbug              6cf4736dd16d6499154f8a737835de9e454803e8893bcd9351d8816269630b1c
sf-vppanoz            9786e1545157352ae5a67b4096d1a87db5a86e026ae89da96dc387aa0062c4c2
sf-vpbullet           a8a2705aca8820b36c41df73ef99c821f5e70beff6e89ccf97e6e139ad1d0548
sf-vpcoop             891e9c3e6a93b29f6fbb179e6c7a8c43cda0d59a8dffe8e702b6a08206a0d0e7
london-vpbug-hb       d75386e162d1d138dfb424c5955bebf7883802020c57e543c5d2b13520b5f2cf
london-vppanoz-hb     62f78cee7580e99898bc1b7998dfce7d4c40c86168f61ac5190840183d272f8f
london-vpbullet-hb    27d9372fb91169c9f3980c9c73f063e0c6d35c1c2ae391ebc255f42f9ecb6e4b
london-vpcoop-hb      4f1487e7b16245642c73ff345e5c4219b8be1cb7c408dd55e2f9f00436155dd8
```

## Earlier baseline and residual handling diagnosis

The following table and discussion preserve the **pre-MPH-correction
ordinary native baseline**. Seven source routes/captures are unchanged;
the current SF Mini safe-stop route and source replace its earlier baseline. Endpoint,
substep and partial-state diagnostics below were compared against this
baseline, so their numbers must not be mixed with the current table.

| Case | Maximum position error (m) | Maximum heading error (°) | Speed RMS error (m/s) | Steering-byte mismatches | Source skid bursts / native requested match |
| --- | ---: | ---: | ---: | ---: | --- |
| SF Beetle | 0.31969 | 0.28509 | 0.01698 | 0 | 2, all starts/stops exact |
| SF Panoz Roadster | 0.004137 | 0.000889 | 0.0001005 | 0 | 2, all starts/stops exact |
| SF Mustang Fastback | 9.228 | 2.327 | 0.0199 | 0 | 14, all starts/stops exact |
| SF Mini Cooper | 0.55301 | 0.29497 | 0.015311 | 0 | 1, starts/stops exact |
| London Beetle, HB | 10.07179 | 2.94881 | 0.0213185 | 4 | 6, all starts/stops exact |
| London Panoz Roadster, HB | 4.739 | 2.0023 | 0.01643 | 3 | 3, all starts/stops exact; gain difference remains |
| London Mustang Fastback, HB | 0.518639 | 0.38726 | 0.011886 | 0 | 26, 25 exact and 1 unmatched source burst |
| London Mini Cooper, HB | 1.13014 | 0.42226 | 0.015587 | 7 | 1, starts/stops exact; HB corner silent in both |

With the verified reciprocal input decode, the fresh clean London Beetle
30-second replay still has maximum COM position error **10.07179 m**, maximum
heading error **2.94881°**, and speed RMS error **0.0213185 m/s**. Position
RMS is 5.72706 m and heading RMS 2.07761°. Grounded-wheel count agrees
throughout. Exact source skid windows coexist with this heading/trajectory
drift; audio agreement is not a substitute for handling agreement.

Four steering bytes differ across the run. A private diagnostic that forced
matching measured bytes did not eliminate the drift; it is not an accepted
route or a replacement for the keyboard comparison. Read-only wheel-force
inspection finds small lateral-velocity/bristle differences before steering
starts: at source sample 26, front lateral velocity is about +0.000124 m/s
versus native −0.000316 m/s, with corresponding bristle/force differences.
Those differences cross near-zero hold/relax boundaries. This is evidence of a
sensitive trajectory history, not proof of an incorrect tire law. Heading
difference tracks accumulated angular-velocity difference; no independent
orientation-integrator defect has been established here. No arbitrary tire,
damping or steering coefficient adjustment is justified by these observations.
This residual remains an open fidelity limitation.

A second clean comparison, SF Mustang Fastback (`vpbullet`), also has
sustained heading drift without a handbrake event: speed RMS 0.0199 m/s, slip
RMS 0.056°, maximum position error 9.228 m and maximum heading error 2.327°.
All 1,801 source samples have zero body impulse/push, the steering bytes
match, and both cars retain four grounded wheels. This independently
reproduces drift in another car and city; it warrants checking for a
systematic handling discrepancy rather than attributing all drift to roundoff.

The [fresh original-to-original SF Fastback repeat](evidence/city-keyboard-2026-10-11/original-repeat/manifest.json)
reproduces all 1,801
recorded physical states exactly: COM position, linear/angular velocity,
body basis, wheel diagnostics, RPM, gear and steering bytes. Absolute
recorder counters differ, but there is zero position, speed or heading
error between the two original runs. Original run-to-run variability does
not explain this measured native Fastback discrepancy.

A read-only phase audit localizes most of the offset to frames 600–1000: the
integrated native-minus-source yaw-rate difference is approximately −2.10°. A
fitted rate ratio is about 1.00043 on frames 200–400, 0.99494 on 400–600,
0.91022 on 600–800 and 0.89125 on 800–1000. That is transient-sensitive, not
evidence for a uniform steering multiplier.

Reconstructing every source contact lateral axis from its source body basis,
the ported steering/Ackermann formula and source ground normal agrees within
0.000006° across all 1,801 samples. An independent double arithmetic replay of
the port's tire equations, fed **source** velocities, loads/friction and
previous source bristles, reproduces 7,200 source wheel updates with maximum
bristle error 2.27×10⁻⁸ m and force RMS 0.00663 N. The maximum force error is
0.795 N at one near-threshold sample; the next largest is 0.002714 N. This is
a scalar-law diagnostic, not a bit-exact x87 reimplementation. It excludes
large coefficient/formula errors in those tested laws while leaving initial
conditions, arithmetic/order and the coupled body/suspension integration under
investigation. Source/native front force transients still differ by hundreds
to thousands of newtons once their kinematic histories diverge.

The [actual Rust source-state audit](evidence/city-keyboard-2026-10-11/source-kernel-audit/manifest.json)
provides a tighter check than the double-precision scalar replay. It calls
production `original::tyre` using each previous **source** bristle state and
the current source wheel velocities/load, with native authored constants.
Across 7,200 SF Fastback wheel updates, force error is RMS 0.00013283 N and
maximum 0.0029297 N; bristle error is RMS 7.672e−10 m and maximum 2.980e−8 m.
Skid-quantity error is RMS 7.024e−10 and maximum 5.960e−8. At source samples
74–75, near the first native divergence, forces match or differ by less
than 0.000245 N and bristles by at most 3.638e−12 m. This finds no material
Rust tire-branch discrepancy when given the original kinematic state.
Rolling speed is reconstructed from rounded `vFwd−vSlip`; raw source wheel
spin is not substituted, and surface drag is outside this dry-road audit.

The same executable calls actual `original::step_train` and the engine
curve at 1,746 eligible source states. It requires an unchanged forward
gear, two consecutive displayed-RPM checks of the unblended attached phase,
engine speed above twice idle and below the limiter, and no brake/handbrake.
With native authored constants, shaft-speed error is RMS 6.520e−7 rad/s and
maximum 3.815e−6 rad/s; source samples 74 and 75 match bit-exactly. Using
captured source ratios and full-throttle curve constants reduces RMS to
3.028e−7 rad/s, retaining the same maximum and exact samples 74–75.
Differential bias is reconstructed from rounded wheel velocity/radius fields;
the latter pass retains native engine inertia/coast and train constants and
evaluates the captured curve primitive explicitly. Neither is a complete
hidden-state or bit-exact x87 drivetrain replay. The observed native shaft
history difference does not indicate a `step_train` defect at identical
source inputs.

A separate source body-torque decomposition uses the captured contact
normals/arms, normal load plus implicit `ks*xdot*dt` predictor, lateral and
longitudinal forces, and authored aero, axle and gyro laws. With source
`forward` interpreted correctly as AGE **back**, it reproduces source
`torque+torque2` over all 1,801 samples with per-world-axis RMS residuals
`[0.001120,0.000460,0.000668]` N·m and maximum vector error 0.003908 N·m. All
source wheels are grounded, `N=Fs`, and body impulses/pushes are zero in this
capture, so the decomposition's grounded/no-bump gates are valid. No fitted
coefficient is needed. This verifies the tested torque laws and coefficient
values against the active original scene; it provides no evidence for a
gyro/aero/axle runtime coefficient or axle-sign discrepancy. It does not
verify the native trajectory's coupled integration merely by matching the
source force evaluation at captured source states.

A source-state integration replay also reconstructs `A`, `B`, `C` from each
wheel's captured `D`, normal and contact arm, then applies the [documented
coupled suspension solve](01-rigid-body.md) to the captured source
force/torque and current velocity. Over 1,800 source-to-source transitions,
per-world-axis velocity RMS residual is `[1.268e−7,5.930e−9,2.816e−7]` m/s;
angular-velocity RMS residual is `[8.093e−10,1.613e−9,7.650e−10]` rad/s.
Maximum component errors are 9.309e−7 m/s and 1.180e−8 rad/s respectively. An
independent plain-Python Gaussian-elimination implementation agrees with the
NumPy result, without using the native trajectory as an input.

The dependency-free [source integration audit](../../../tools/handling/audit_source_integration.py)
makes this check reproducible:

```sh
python3 tools/handling/audit_source_integration.py \
  original-sf-vpbullet-clear.json \
  sf-vpbullet-final-city-native.csv.provenance.json \
  --output sf-vpbullet-source-integration.json
```

Both inputs may be `.json` or `.json.gz`. Standard output contains the
summary; `--output` additionally saves all per-transition residuals. The
tool rejects incomplete captures, mismatched car/city identities, clocks
other than a constant 60 Hz step, invalid mass/inertia/gravity, airborne
wheels, bump-stop states and any recorded body impulse or push correction.
It does not silently skip those frames or fit coefficients to them.

Re-running the packaged audit reproduces the Fastback residuals above over
1,800 transitions. The clean SF Panoz capture also passes the guards:
velocity RMS residuals are `[9.907e−8,5.042e−9,2.731e−7]` m/s and angular
RMS residuals `[1.503e−10,2.394e−9,1.274e−10]` rad/s. Maximum component
errors are 4.786e−7 m/s and 1.477e−8 rad/s respectively. These are
source-to-source equation checks, using the native provenance only for
its authored mass, inertia and gravity; they do not compare native states.

The Rust audit additionally evaluates the production f32 `Mat3` Jacobian,
cofactor inverse and coupled expressions at the captured source states.
Using raw AGE rows for source inertia, all 1,800 next forward/Z velocity
components are bit-exact. Velocity RMS residuals per axis are
`[2.615e−12,6.675e−9,0]` m/s, with maximum component 1.176e−8 m/s.
Angular-velocity RMS residuals are `[1.015e−9,1.931e−9,8.469e−10]` rad/s,
with maximum component 2.980e−8 rad/s. Of 5,400 components, 3,605 velocity
and 3,147 angular-velocity components are bit-exact. ULP distances near
zero can be large despite these tiny absolute errors.

Projecting only the captured inertia basis to a normalized quaternion raises
angular RMS residuals to `[2.218e−8,8.419e−9,2.123e−8]` rad/s and maximum
component to 1.583e−7 rad/s. Geometry/Jacobians remain source-prescribed in
that variant, so it is an inertia-only diagnostic. These results strongly
exclude a material cofactor-inverse or coupled-equation defect at the tested
states, but do not reproduce the native pose/contact/bristle feedback loop.
The several-degree clean city trajectory discrepancies remain unresolved.

The supplemental manifest preserves the executable identity, native
provenance before the MPH correction, captured derived constants, tool and
kernel-source snapshots, compact summaries and losslessly gzipped full
per-row results with raw/stored SHA256 hashes. Reproduce the Rust check with:

```sh
RUSTC_WRAPPER= cargo build -p mm2_app --example source_tyre_audit
target/debug/examples/source_tyre_audit \
  original-sf-vpbullet-clear.json native.provenance.json output.json \
  original-derived-constants.json
```

Only this unique diagnostic example was built; no playable binary or quality
gates were run for the audit. The existing city audio and powerslide results
remain separately measured evidence, with their stated discrepancies.

This verifies the grounded coupled equations, gravity term, Jacobian assembly
and documented positive-sign back-substitution at these captured source
states. The comparison is double arithmetic against f32 samples; it is not a
bit-exact native/x87 arithmetic comparison or a closed-loop trajectory test.
It does not test native collision geometry, source body orientation
integration, airborne momentum/clamps, mixed-frame timing, initial hidden
caches or the sensitivity of accumulated bristle history. The source next
position also agrees with `p+dt*v_next` within expected absolute-coordinate
f32 rounding (maximum component 1.526e−5 m), with no body push in this case.

An exact 180° initial-yaw diagnostic only slightly changes the Fastback result
(maximum position 9.013 m versus 9.228 m, heading 2.298° versus 2.327°); it
does not explain the dominant drift.

A subsequent private diagnostic partially restores the settled source
state: pose, velocity/angular velocity, wheel bristles/compression, pending
forces and reconstructed suspension matrices. It retains native powertrain
timers, shaft bias and other hidden caches. Converting the source basis to
a normalized quaternion also cannot preserve raw AGE matrix drift. This is
therefore a partial source-state seed, not a fully matched initial state.

| Case | Ordinary replay: maximum position / heading error | Partially seeded replay: maximum position / heading error |
| --- | --- | --- |
| SF Mustang Fastback | 9.228 m / 2.327° | 8.51417 m / 2.16494° |
| London Beetle | 10.0718 m / 2.949° | 9.95774 m / 3.232° |
| SF Panoz | 0.00414 m / 0.000889° | 0.061517 m / 0.008569° |

The seeded London case has 14 steering-byte differences, versus four in the
ordinary replay. Seeding does not eliminate the larger clean drift and
worsens the already tight Panoz result. These outputs remain diagnostic;
the packaging script rejects their provenance as ordinary native evidence.
They provide no justification for changing production coefficients. The
larger clean trajectory discrepancy remains unresolved.

An independent pose-endpoint audit compares each trajectory with its own
post-step angular velocity, rather than substituting original velocities
into native output. The recovered source rotates each raw AGE axis once by
Rodrigues' formula, without re-orthonormalizing. Avian composes six scaled-axis
quaternions and normalizes the resulting rotation; its final writeback also
converts the accumulated COM displacement through the model origin.

For SF Fastback, the per-component full-step axis residual is RMS 2.034e−8
in source versus 4.071e−7 in native, with maximum 1.456e−7 versus 1.095e−6.
Basis orthogonality residual reaches 1.654e−5 in source, versus 1.528e−6
in native. The full-step COM update residual is RMS 2.735e−6 m in source
versus 4.930e−6 m in native, with maximum 1.525e−5 m versus 4.127e−5 m.
The accumulated signed yaw endpoint residual is only −0.0001356° in
source and −0.0002545° in native over 30 seconds. Those direct orientation
rounding residuals do not explain the multi-degree heading difference;
any relevant effect must act indirectly through the coupled force history.
These establish different finite-precision pose operations, not that those
small differences cause the larger closed-loop drift. A private SF Fastback
endpoint experiment keeps six collision substeps but replaces selected final
pose updates before force evaluation:

| Endpoint diagnostic | Maximum position error (m) | Maximum heading error (°) |
| --- | ---: | ---: |
| Ordinary replay | 9.228 | 2.327 |
| COM once per tick | 9.668 | 2.396 |
| Quaternion rotation once per tick | 5.098 | 1.220 |
| Both once per tick | 5.604 | 1.376 |

The rotation-only experiment has mixed results across other cars/cities:

| Case | Ordinary maximum position / heading error | Rotation-once maximum position / heading error |
| --- | --- | --- |
| SF Panoz | 0.004137 m / 0.000889° | 0.0575 m / 0.00833° |
| London Beetle | 10.07179 m / 2.94881° | 10.048 m / 3.328° |
| London Panoz | 4.739 m / 2.0023° | 3.077 m / 1.303° |

Rotation arithmetic affects the closed-loop result, but this is not a
uniform improvement. It worsens the very tight SF Panoz result and London
Beetle heading while improving the two larger Panoz/Fastback discrepancies.
No production pose change or coefficient adjustment follows from these
diagnostics.

The source routine `0x4bce20` constructs an axis-angle matrix via
`0x4bd1b0`, then multiplies the existing raw matrix via `0x4bc960`.
The constructor uses x87 `FCOS` at `0x4bd1b7` and `FSIN` at `0x4bd1bf`,
with no lookup table or internal small-angle approximation. It stores sine
and `1−cos` as f32, retains cosine on the floating-point stack, stores each
constructed matrix element as f32, then multiplies and stores the resulting
axis entries without normalization. The caller skips rotation when squared
net turn is at most `1e−15`. Computing cosine in f32 before subtracting it
from one changes these storage stages for tiny angles.

A raw-matrix-history diagnostic can retain those axis-update stages and
project its result to a normalized quaternion for Avian. Such a mode is
only partial: projecting before force evaluation discards the raw AGE
matrix's axis-length and orthogonality drift. A full raw-basis diagnostic
would also need the wheel geometry, COM, inertial and aero calculations to
consume that matrix separately from the collider/render quaternion.

A third clean city case, SF Beetle without a handbrake event, stays much
closer over the same 30-second duration: maximum position error 0.31969 m,
maximum heading error 0.28509°, position RMS 0.12410 m, heading RMS 0.08319°,
and speed RMS 0.01698 m/s. This provides a counterexample to a universal
multi-degree city turning bias. Differences among car/input histories remain
relevant, and the larger clean London Beetle/Fastback drift is still retained
as an unresolved fidelity limitation.

The clean SF Panoz Roadster is tighter still: maximum position error 0.004137
m, maximum heading error 1.551×10⁻⁵ rad (0.000889°), and speed RMS 0.0001005
m/s, with no steering-byte mismatches. Its two source skid bursts also match
native requested starts/stops exactly, with no band mismatch across 1,800
samples and same-band amplitude error 7.32×10⁻⁹. These same city/harness
conditions provide strong counterevidence to a global multi-degree bias from
Avian's six pose/contact substeps. They do not remove the larger
Beetle/Fastback limitations or establish behavior for unmeasured wheel
unloading, impacts and difficult surface transitions.

A private one-substep diagnostic reduces some of the larger position and
heading errors, but does not eliminate them:

| Case | Six substeps: maximum position / heading error | One substep: maximum position / heading error |
| --- | --- | --- |
| SF Mustang Fastback | 9.228 m / 2.327° | 4.14 m / 1.210° |
| London Beetle | 10.072 m / 2.949° | 8.197 m / 2.795° |

These diagnostic outputs are not accepted native route evidence. The
near-exact SF Panoz result under six substeps is a counterexample to a
universal substep bias. Changing the production substep count globally is not
justified by these two improvements, especially without verifying contact
stability and the other vehicle/input histories.

The clean SF Mini Cooper comparison has maximum position error 0.55301 m,
maximum heading error 0.29497° and speed RMS error 0.015311 m/s, with no
steering-byte mismatches. An independent review found zero body impulse,
angular impulse and push/turn corrections across all 1,801 source samples; all
four source wheels remain grounded. Its sole skid burst, frames 24–27 (stop at
frame 28), matches native requested onset and stop exactly, with no
active-band mismatch across 1,800 compared frames.

The clean London Mini Cooper handbrake route has maximum position error
1.13014 m, maximum heading error 0.42226°, speed RMS error 0.015587 m/s,
and body-slip RMS error 0.0259° (maximum 0.3296°). Seven steering bytes
differ. Independent review found zero body impulse, angular impulse and
push/turn corrections in every one of the 1,801 source samples, with all
four wheels grounded throughout.

Its sole source skid-audio burst is also frames 24–27 (stop at frame 28),
matching native requested onset and stop with zero ticks of error and no
active-band mismatch across 1,800 samples. **The handbrake corner is silent
in both the original and native requests.** During the held-handbrake
interval, frames 320–337, maximum absolute body slip at speeds of at least
5 m/s is 8.7616° in the source and 8.5883° in native, both at aligned
frame 333; actual body speeds there are 8.7773 and 8.7843 m/s respectively.
These angles use body lateral velocity versus physical forward velocity,
not the wheel `slipVisual` used by the audio law. This verifies that a
modest body slide or a held handbrake does not necessarily produce source
skid audio.

## Current city comparison results: source-initialized MPH correction

All eight ordinary native runs below use the corrected source-initialized
MPH constant, with the same frozen routes and reviewed source recordings.
Independent source review found zero body impulse, angular impulse and
push/turn corrections throughout the clean captures. The correction changes
some trajectories materially through later state history; it does not remove
the unresolved city handling discrepancies.

| Case | Maximum position error (m) | Maximum heading error (°) | Speed RMS error (m/s) | Steering-byte mismatches | Source skid bursts / native requested match |
| --- | ---: | ---: | ---: | ---: | --- |
| SF Beetle | 0.31969 | 0.28510 | 0.016978 | 0 | 2, both starts/stops exact |
| SF Panoz Roadster | 0.004891 | 0.000892 | 0.0001009 | 0 | 2, both starts/stops exact |
| SF Mustang Fastback | 7.85705 | 1.99324 | 0.024567 | 0 | 14, 13 exact; one stop 1 tick early |
| SF Mini Cooper, safe stop | 1.68512 | 0.83380 | 0.017099 | 0 | 3, all starts/stops exact |
| London Beetle, HB | 10.07179 | 2.94881 | 0.0213185 | 4 | 6, all starts/stops exact |
| London Panoz Roadster, HB | 4.02189 | 1.10151 | 0.012082 | 3 | 3, all starts/stops exact; gain difference remains |
| London Mustang Fastback, HB | 0.519327 | 0.49508 | 0.028411 | 0 | 26, 22 exact; timing/band differences detailed below |
| London Mini Cooper, HB | 1.37461 | 0.36789 | 0.008514 | 4 | 1, starts/stops exact; HB corner silent in both |

There are **57 source windows and 57 native requested windows**. Same-band
matching pairs 55 windows; **52 pairs have exact onset and stop**. The six
non-Fastback cases account for 17 windows, all exact. SF Fastback has one
band-2 window `[625,628)` whose native stop is at 627, one tick early.

London Fastback has 24 same-band pairs, 22 exact. The native band-2 stop
for `[770,772)` is one tick early; `[786,787)` occurs a tick later in
native. Source band-0 `[311,312)` and band-2 `[789,790)` are unmatched;
native has unmatched band-1 windows `[772,773)` and `[789,790)` instead.
Thus this case has six active-band-set mismatch frames, including five
frames where the presence of any skid differs. Equal window counts do not
establish equivalent bands or playback timing.

London Panoz same-band amplitude MAE is 0.000156612, with maximum error
0.00532456. London Fastback MAE is 0.000093506, with maximum error
0.00508855. These are comparisons of source DirectSound gain against native
requested gain. The final live-backend replays use the same production code
and byte-identical physics CSVs. Their attached sink gain and active-band
windows match native requests exactly; device output is not recorded. Earlier
pre-correction live-backend checks remain separately versioned.

### Current controlled powerslide regression

The current build also replays the existing original flat-course powerslide
recordings, with their recorded settlement lengths (Beetle/Fastback 600 ticks,
Panoz 180). Each records 900 ticks with full throttle, a handbrake flick and
countersteering. These compare the larger controlled slides independently of
the city routes; they do not measure skid audio. The Mini has its London
handbrake-corner and SF braking measurements, not a new large-angle flat
powerslide recording in this set.

| Car | Maximum heading error (°) | Body-slip RMS error (°) | Speed RMS error (m/s) |
| --- | ---: | ---: | ---: |
| Beetle | 0.06176 | 0.01342 | 0.00328 |
| Panoz Roadster | 0.72778 | 0.03261 | 0.12459 |
| Mustang Fastback | 0.01552 | 0.00203 | 0.03633 |

[Current replay evidence](evidence/city-keyboard-2026-10-11/current-flat-powerslides/manifest.json)
records binary/source hashes and full native CSVs. These regressions do not
remove the larger closed-loop city trajectory discrepancies above.

### Earlier baseline street-slide measurements

The next body-slip table preserves the **pre-MPH-correction native baseline**
used in the earlier powerslide discussion; it is not a post-correction replay
measurement. The source values and frozen handbrake interval are unchanged.

The London handbrake interval is frames 320–337 in every case. Maximum
absolute body slip **during that interval**, considering only body speeds
of at least 5 m/s, is:

| Car | Original body slip (°) | Native body slip (°) | Aligned peak frame |
| --- | ---: | ---: | ---: |
| Beetle | 7.7134 | 8.0408 | 327 |
| Panoz Roadster | 4.9889 | 5.0024 | 320 |
| Mustang Fastback | 4.5863 | 4.6269 | 332 |
| Mini Cooper | 8.7616 | 8.5883 | 333 |

These are held-handbrake street-corner measurements, not maximum slips over
the entire route or replacements for the larger controlled-flat powerslide
cases. Modest source body slip can coexist with no source skid audio.

The London Panoz source-state torque decomposition also reproduces source
`torque+torque2`, including the handbrake interval: per-world-axis RMS
residuals are `[0.000834,0.000427,0.000764]` N·m, with maximum vector error
approximately 0.004214 N·m. The grounded integration audit passes all 1,800
transitions, with maximum velocity and angular-velocity component errors
4.852e−7 m/s and 7.293e−8 rad/s respectively. These source-state checks do
not remove the measured London Panoz native trajectory drift.

## Evidence packaging and acceptance

**Eight-case evidence package:**
[city-keyboard-2026-10-11/manifest.json](evidence/city-keyboard-2026-10-11/manifest.json).
The package contains all eight reviewed source/native cases and preserves
the remaining numerical discrepancies; it is not a claim of perfect handling.

[The packaging script](../../../tools/handling/package_city_evidence.py) requires explicit
`reviewed_clean` notes and all eight cases for a final bundle. It checks
complete sequential source/native frames, car/city/route identity, raw
frozen-route hashes, settlement, finite numbers and native contact
contamination; it cannot infer unrecorded original actor collisions. It
losslessly gzips full source diagnostics, native CSV/audio/contact sidecars,
preserves raw and compressed hashes, copies provenance/routes, and regenerates
gzip-readable comparisons. A partial development bundle is explicitly marked
partial. Numerical comparison errors remain unthresholded: `reviewed_clean`
certifies capture contamination review, not handling fidelity.

## Final build validation

Formatting, Clippy with warnings denied, and the complete workspace tests
passed before the calibration commit. The final test run used four concurrent
test threads and localhost socket access. The outdated native-skid network
fixture was corrected to stage source wheel visual slip; its contact and wire
assertions remain unchanged. The initial subprocess timeout did not recur.
[Validation record](evidence/city-keyboard-2026-10-11/quality-gates.json).

The isolated playable copy is `target/playtest/polish/mm2`, launched by
`target/Rust MM2 Handling Polish.app`. Its 90-frame SF Beetle rendered smoke
check passed. Later Cargo builds do not replace this copy. Neither this build
check nor the clean recording status certifies perfect original handling; the
remaining path drift and Fastback audio differences are reported above.
