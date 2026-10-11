# Shared city keyboard traces

`city_handling_trace` replays keyboard events on the production SF or London
collision scene. It calls `mm2_app::city::load_city` and `VehiclePlugin`;
road splitting, internal-edge correction, authored materials, chassis contacts,
and wheel physics use the playable implementation.

Build only the example while a playable `mm2` binary is in use:

```sh
RUSTC_WRAPPER= cargo build -p mm2_app --example city_handling_trace
target/debug/examples/city_handling_trace /path/to/retail route.json native.csv
```

The route contract is:

```json
{
  "car": "vpbug",
  "city": "sf",
  "initial_room": 163,
  "initial_model_pose": [-86.0, 1.0, -330.0, 180.0],
  "frames": 1800,
  "settle_frames": 600,
  "schedule": [
    [0, [[38, true]]],
    [300, [[39, true]]],
    [330, [[32, true]]],
    [348, [[32, false]]],
    [375, [[39, false], [37, true]]],
    [435, [[37, false]]],
    [1500, [[38, false], [40, true]]]
  ]
}
```

These coordinates illustrate the format; they are not a validated original
comparison route. The pose is the **model origin**, followed by yaw in degrees
around positive Y. Original recording code must convert model origin to COM
using the car's rotated `−CenterOfGravity` before writing the inertial-CS pose.
The required `initial_room` integer records the original's verified room;
the native solver finds collision geometry spatially.

Events are strictly increasing frame numbers with virtual-key changes. Arrow
keys are left37, throttle38, right39, brake40; handbrake is space32. Buttons
remain held until changed. The car settles for exactly600 physics ticks by
default, with handbrake held and all arrows released. Route buttons start
released; frame0 applies its changes and advances the first recorded tick.
`frames:1800` records1800 ticks (30 seconds). No traffic actors are spawned.

CSV `x/y/z` are displacement of COM from the settled start. `com_*` and
`model_*` are absolute world coordinates. Original sample `n0` is pre-input;
native CSV frame0 aligns with original `n1`, native frame1 with original `n2`.
The native file includes wheel forces, bristles, slip visuals, contact points,
normals, grip, body-contact aggregates, delayed impulses, push corrections,
COM basis/velocity, and implicit matrices. Matrix entries use column order.
`native.csv.provenance.json` records the route, effective vehicle config,
settled model/COM poses and basis, and loader room/prop counts.

The loader includes authored INST placements. This harness does not run the
application's dormant-prop activation, breakaway, traffic, pedestrian,
recovery, audio-playback, or rendering systems. A route hitting a dormant prop
therefore requires separate app-level verification. Body-contact summaries
come from the production contact graph; they do not reconstruct original
per-contact records.

`native.csv.contacts.json` preserves every native contact pair involving the
car, including speculative points with negative penetration. Entries identify
collider entity/name, Banger phase, authored surface material and raw source
contact material, together with point normal, penetration, relative normal
speed and solver impulse. A zero positive-penetration aggregate alone does
not prove a route is clear of props: speculative constraints can stop a car.

For offline route authoring only, set `MM2_PLAN` to a JSON file containing
`{"points":[[x,z],...],"speed":16}`. The example follows that polyline with
ordinary discrete keyboard keys and writes `native.csv.route.json` containing
the resulting explicit schedule. Optional `"handbrake":[320,338]` holds the
handbrake over that half-open frame interval. Replay the emitted route with
`MM2_PLAN` unset before accepting it for an original comparison. The original
must receive the same frozen keyboard schedule; it never follows waypoints.
The planner changes input only and uses the production physics unchanged.

`native.csv.audio.json` samples production surface-audio selection after every
recorded update. The example loads `SurfaceTables`, clear-weather
`SurfaceAudio` for the selected car, the VFS `WaveBank`, and decoded
`PcmAudio` assets, then runs `surface_voices` and `level_new_voices` with the
car marked `PlayerVehicle`. Each frame records `SurfaceContact`, its child
`SurfaceVoice` roles/current requested mix, `AudioPlayer` handle and decoded
clip duration, and `PlaybackSettings`. The current mix is authoritative for
headless requested gain/pitch; settings describe voice creation and bus
scaling, because no live sink exists to apply later changes. `AudioPlugin`
is deliberately absent: these records measure requested selection/playback
metadata, not backend playback, device output or recorded sound.

For native cars the recorded `mix_volume` is **linear amplitude** after the
source DirectSound attenuation conversion: normalized gain `v` becomes
`trunc((v - 1) * 10000)` hundredths of a decibel, then amplitude
`10^(attenuation / 2000)`. Thus normalized gain0.88 requests amplitude
approximately0.25119, rather than0.88. Compare like units with original audio
telemetry; clip selection and normalized source gain remain distinct facts.

`MM2_MEASURED_STEERING=/path/to/original.json` is an explicitly diagnostic
mode. It still runs the normal frozen keyboard schedule and steering filter,
then replaces only final `VehicleInput.steering` with the signed byte in measured original row
`frame + 1`, reconstructed using the verified retail `f32(1/127)` multiplication. Source car identity, city,60Hz, completion, full route equality,
route-file SHA-256 (using `shasum`), sequential rows and signed-byte steering
quantization must match. It cannot combine with waypoint planning. Output
provenance marks the override; never use this mode as ordinary keyboard-input
fidelity evidence. It isolates physics evolution from measured one-byte input
threshold differences.

The harness registers production `NativeSkidTimingPlugin`: at the start of
each fixed physics tick, it snapshots native wheel slip and retained front
contacts from the preceding simulation tick. Surface-audio Update consumes
that snapshot, matching the measured original skid-request phase. Rolling
and arcade audio timing are unchanged. CSV wheel fields still report the
current completed tick, so their maximum `slip_visual` is one tick ahead of
the native skid request in `.audio.json`.

Packaging reviewed city evidence
-------------------------------

`package_city_evidence.py` stores full numeric source recordings, native CSV,
requested-audio metadata and all-contact diagnostics with lossless gzip. It
copies the frozen route, native provenance and comparator scripts, then generates
world-coordinate handling and actual-source-backend audio comparisons. The
manifest records raw and compressed SHA256 hashes and sizes. JSON floats and CSV
numbers are preserved byte for byte, including full diagnostics; they are never
rounded or resampled. gzip has a fixed timestamp. Retail assets are not copied.

Pass an explicit plan, with paths relative to that plan or absolute:

```json
{
  "provenance": {
    "source_capture": "State the executable/archive identity and capture conditions",
    "native_capture": "State the code revision/build and any local changes"
  },
  "cases": [
    {
      "id": "london-vpbug-hb",
      "status": "reviewed_clean",
      "review_note": "State how traffic, props and source collision telemetry were reviewed",
      "original": "original-london-vpbug-hb-fresh.json",
      "native": "london-vpbug-final-city-native.csv",
      "route": "london-vpbug-hb-frozen-route.json"
    }
  ]
}
```

```sh
python3 tools/handling/package_city_evidence.py plan.json new-evidence-directory
```

Final packaging requires all four requested cars in both cities. Use
`--allow-partial` for a development package; its manifest states that scope.
`reviewed_clean` is an explicit contamination review, not an assertion that
handling differences pass a threshold. Never assign it to a known traffic/prop
collision run. The script verifies car/city identities, complete sequential
frames, 60Hz, identical route data and the original's frozen route hash; it
rejects measured-steering diagnostics, native prop/body contacts and nonfinite
numbers. Native provenance must explicitly record `diagnostic_waypoint_plan:
null`; planner runs and older records without that field are rejected. It
cannot determine whether unrecorded original actors contaminated a
capture: that review remains necessary. Output directories must be new.
Native sidecars default to CSV filename plus `.audio.json`, `.contacts.json` and
`.provenance.json`; optional plan keys `audio`, `contacts`, `provenance` override
those paths. Both comparators can read `.json.gz` and `.csv.gz` directly. Native
sound evidence remains requested production metadata unless live sink evidence
is supplied separately. Optional `backend_native`, `backend_audio`, and
`backend_provenance` paths must be supplied together. The live CSV must be
byte-identical to the ordinary native replay; the physics provenance must
match, and the live audio must observe attached sinks without failed/dropped
voices. Packaging preserves the live audio sidecar and generates a separate
`backend-audio-comparison.json`. It does not claim recorded device output.

Set `MM2_REALTIME_AUDIO=1` for optional native audio-backend verification.
This registers Bevy's `AudioPlugin` and the production `PcmAudio` source,
uses the default audio output device, and paces every settle and route tick
at 60 Hz. A 600-tick settlement plus 1800-tick route takes about 40 seconds;
slow frames extend elapsed time without dropping physics ticks or inputs.
The default mode keeps fast headless execution and identical physics and
requested-mix frame values.

Real-time `.audio.json` records each attached `AudioSink`'s volume, speed,
paused/empty state, and whether positive unpaused output is requested.
`PlaybackSettings` remain creation metadata; the sink's current values
verify subsequent mixer updates. Production volume/pause systems run in
PostUpdate before Bevy queues playback. This observes the native backend,
but does **not** record device output or prove what was heard through a
speaker. If positive mixes are requested but no sink ever attaches, the
example saves its metadata and exits with an explicit failure instead of
claiming successful backend verification. A route requesting no sound
cannot establish output-device availability.

`MM2_EXACT_INITIAL_YAW=1` is an optional initial-pose sensitivity diagnostic.
It computes the yaw quaternion using double-precision half-angle sine/cosine
before casting components to `f32`, matching the original recorder's
JavaScript arithmetic. At 180 degrees this avoids the small horizontal
bias from first rounding pi to `f32`. Provenance marks
`diagnostic_initial_yaw:true`; this does not represent ordinary native
spawn behavior. With the variable unset, initial yaw computation is unchanged.

`MM2_DIAGNOSTIC_SUBSTEPS=1` overrides Avian's six solver substeps with one
for a sensitivity experiment. Provenance records both the override and the
actual count. This changes solver behavior and is excluded from ordinary
keyboard calibration packages, as are the initial-yaw and measured-steering
diagnostics.

`MM2_DIAGNOSTIC_SETTLED_SOURCE=/path/to/original.json` runs native settlement,
then replaces selected starting state with the original's captured `n0`.
It validates car identity, city, exact route contents/SHA256, 60 Hz,
completeness, and consecutive rows; route planning cannot be combined with it.
The diagnostic currently requires four grounded wheels and no pending
position/rotation correction.

The seed restores COM pose and velocity, angular velocity, wheel compression,
bristles, reaction/spin, pending forces/torques/impulses/push, and suspension
Jacobians reconstructed from captured damping/normal/contact arms. The source
JSON's historically named `forward` column is AGE's **back** axis. Conversion
to a normalized quaternion loses the raw original matrix's slight drift.
Native powertrain state, engine RPM/timers/clutch, drive bias, input speed
caches, collision entity references, and audio class history remain intact;
world angular momentum and individual wheel speeds are derived. Provenance
records `diagnostic_settled_source` with these limitations. This is a partial
state sensitivity experiment, not a bit-identical restore or ordinary native
spawn evidence, and cannot be included in the accepted evidence package.


`MM2_DIAGNOSTIC_ENGINE_SOURCE=/path/to/start-snapshot.json` optionally seeds
selected engine history from a two-row source snapshot. Set
`MM2_DIAGNOSTIC_ENGINE_REFERENCE=/path/to/full-original.json` to the matching
complete recording (or use the settled-source recording). Identity, route hash,
and both physical boundary rows must match; active shifts and attached clutches
are rejected. This remains a partial restore and is excluded from ordinary
calibration packages.

`MM2_DIAGNOSTIC_POSE=com|rotation|both|age` isolates full-step pose accumulation
while retaining the six production contact substeps. The AGE experiment keeps
raw matrix history for its orientation endpoint, then projects it to a
normalized quaternion for native force calculations. It does not reproduce
all original matrix arithmetic. These modes produced mixed results across cars;
none is enabled in production or accepted as ordinary keyboard replay evidence.
