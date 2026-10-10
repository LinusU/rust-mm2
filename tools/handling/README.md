# Original handling measurements

These tools exercise the production vehicle model and the retail game. They
record evidence rather than tune arbitrary constants to an expected answer.
No retail assets belong in this directory.

## Native trace

Build the instrument once, then record CSV at 60 Hz:

```sh
cargo build -p mm2_app --example handling_trace
target/debug/examples/handling_trace /path/to/install vpbullet launch 15 1.0 > native.csv
```

Cars use their imported tuning, human auto-reverse, keyboard steering ramp,
signed-byte quantization and the production Avian vehicle plugin. The body
settles for ten seconds on a flat slab. The optional final argument selects
its material friction (default `0.9`); the generated original course uses
`1.0`, so pass that value for course comparisons. Positions record the
center of mass relative to its initial position. An optional final
`settle_frames` argument overrides the default 600 physics ticks. Use 780
when reproducing a Beetle baseline that has already idled for 180 ticks
before a further 600-tick original warm-up. Each row is the
state **after** applying that frame's input and advancing one step. Scenarios:

| Scenario | Inputs |
| --- | --- |
| `launch` | Full throttle throughout |
| `coast` | Full throttle for five seconds, then release |
| `brake` | Full throttle for five seconds, then full brake |
| `turn` | Full throttle; right steering from five through seven seconds |
| `handbrake` | Full throttle for five seconds; release throttle and apply right steering plus handbrake for two seconds |
| `powerslide` | Full throttle throughout; right at frame300, handbrake flick330–347, release handbrake348, left countersteer375–434, neutral435 onward |

## Original trace

Use the retail `Midtown2.exe` in an already running BottleShip guest. The trial
executable uses different addresses. The instrument obtains the player/car
pointer from the live `mmPlayer::Update` breakpoint; it does not hardcode a
heap address. Reset the cruise with the desired car and ensure it is upright,
stationary and grounded before recording.

```sh
BOTTLESHIP_ROOT=/path/to/bottleship \
ORIGINAL_CAR_ID=vpbullet ORIGINAL_SCENE=verified-flat-course \
ORIGINAL_FIXTURE_SHA256=<verified-psdl-hash> \
bun tools/handling/capture_original.ts turn original.json
```

The recorder fixes the original frame step to 1/60 second, hooks the keyboard
poll, settles for 600 frames, then records 901 frames with the same input
schedule. It rejects a moving, tilted or ungrounded initial state. It saves
live mass/engine/wheel tuning, per-wheel friction and contact normals alongside
the trajectory. Hooks, keys and clock settings are restored afterward. The
emulator remains paused so it does not drive away unattended.

On the generated course, the original can assign a reserved room (`0`) to its
spawn. `dgPhysManager::DeclareMover` at `0x468360` rejects an instance whose
signed 16-bit room at `lvlInstance + 6` is zero, so merely moving the body does
not enable physics. `ORIGINAL_POSE=1200,1,1200,0` explicitly places the car at
x/y/z with yaw in degrees and assigns its cached instance to main room 1.
Use this only on the verified generated course. The normal settlement and
contact checks still apply; this option does not manufacture grounded data.

`ORIGINAL_BASELINE=save` snapshots the stationary simulator at the first
accepted sample. Subsequent captures can use `ORIGINAL_BASELINE=restore` to
restore it in the same guest, followed by another 600-frame settlement and
validation. This rejects different guest processes, car IDs, or player/car
pointers. A pose override and
a saved-baseline restore cannot be combined. `ORIGINAL_SETTLE_FRAMES` can extend
the default 600-frame warmup (minimum 180); captures record the chosen value.
Start a new guest/car in forward first gear; holding the brake after stopping can activate original auto-reverse.

An entered car ID or scene label is metadata, not independent proof. Check the
live tuning against the intended car and the contacts against the course.
The recorder verifies retail executable code signatures and its SHA-256 before
using fixed addresses, and records that fingerprint with the measurement.
Original traffic, props, grade changes and collisions invalidate affected
frames. The `speed` field in the original is stale at this sampling boundary;
the comparator uses signed body velocity instead.

## Flat PSDL course

Generate a self-authored road plane referencing an installed road texture:

```sh
python3 tools/handling/make_flat_track.py /tmp/track/city/sf.psdl --rooms 1172 --size 8000
python3 tools/handling/patch_dave_member.py /path/to/install/mm2core.ar \
  city/sf.psdl /tmp/track/city/sf.psdl /tmp/calibration-mm2core.ar
```

The helper writes a separate archive copy and refuses to modify its source.
Only the member payload and its two size fields change; every other byte is
verified unchanged. The original resolves the city from its archive before
loose files, so installing only a loose PSDL does not select the course.
Use the patched archive in an isolated guest copy and restart the executable
so its cached archive table is rebuilt. Restore the original guest archive
after the experiment. Do not replace the user's retail installation.

The main road occupies 80% of the world width when preserving multiple rooms;
disjoint one-metre pads in the outer ring retain companion-file room indices
without overflowing the original's spatial buckets. These rooms do not remove
the companion files' props, pedestrians, traffic or cops. Disable those actors or
use a verified isolated position. Confirm the runtime contact normals and
height before accepting a run as a flat-course measurement.

## Compare

```sh
python3 tools/handling/compare_traces.py original.json native.csv
python3 tools/handling/compare_traces.py original.json native.csv --last-frame 240
```

Original rows are sampled before their scheduled inputs; native rows are
post-step. The default alignment therefore compares original frame `n` with
native row `n-1`. Positions are transformed into the original initial car axes,
yaw errors wrap at ±π, and speed is projected onto the car's forward direction.
The report gives RMS and maximum differences in metres, m/s, radians and RPM.
It also compares signed sideways velocity, body slip angle, yaw rate and
quantized input bytes. Body slip is measured from horizontal COM velocity
against the body's forward/right axes; below 1 m/s its angle is excluded
because a stationary velocity has no heading. `--first-frame` selects a phase
without altering alignment. Powerslide reports separately measure the flick,
powered sustain, countersteering and recovery. They count actual sliding at
15 degrees or more and at least 5 m/s, rather than treating a handbrake input
as proof that a powered slide occurred.
Inspect the whole trace and exclude contaminated windows explicitly; small
launch errors alone do not prove matching cornering, impacts or every car.

The current scope supersedes the specs' earlier preference for deliberately
enhanced handling. The owner requested original steering and physics fidelity;
the specifications themselves remain unchanged.
