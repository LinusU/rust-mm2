# Retail handling comparison, 2026-10-10

Current expanded measurements and corrections: [expanded verification](expanded-README.md). The baseline results below are historical.

These are measured trajectories and probe results from the production vehicle
model, not targets fed back into its simulation. The implementation uses the
recovered retail formulas and the user's installation tuning.

## Reproduction and provenance

- Engine branch: `codex/original-handling`, based on `64c1be1`.
- Platform: macOS / Apple Silicon; native rendered playtest uses Metal.
- Retail `Midtown2.exe` SHA-256:
  `93afb6c00be3d3b12a6e5d88d8e4f711a13f5a4100dbdfab77943c30083f98a2`.
- Source `mm2core.ar` SHA-256:
  `d4768421c7435faba1e17773a39053227deb87ffbcbba2931cf47b061c21505e`.
- Self-authored course SHA-256:
  `7450117dd967bb39df4b9ea22de07b13b6b2c547a86216d1cb0d6507af23bba5`.
  Generate with `make_flat_track.py --rooms 1172 --size 8000`.
- Original runs use the retail executable in BottleShip, fixed 1/60-second
  step, isolated course position `(1200,1,1200)`, four grounded wheels,
  upward contact normals and measured material friction `1.0`.
- Native runs use `handling_trace <install> <car> <scenario> 15 1.0`,
  60 Hz, ten seconds of settlement (13 seconds for the warm Beetle, whose
  saved original baseline had already idled for three seconds), using the
  production Avian vehicle plugin.
  Human auto-reverse and the imported keyboard steering filter are enabled.
- Original row `n` precedes input; native row `n-1` follows integration.
  The comparator aligns those boundaries, uses signed body velocity rather
  than the original's stale `Speed`, and compares relative center-of-mass
  positions in the initial car frame.

No retail archives, textures, models, saves or process-memory dumps are
included. The JSON contains numeric telemetry and tuning fingerprints.
Original screenshots and the fixture-patched archive remain outside Git.
The user's retail installation was never modified.

## Final code checks

Formatting, strict all-feature Clippy and the full workspace suite pass:
3,065 tests, zero failures, zero ignored tests. `quality-gates.json` records
the exact commands and a digest of the changed Rust source. These final gates
ran after handling calibration. Independent read-only review confirmed the
remote human input and synchronized override validation fixes; no actionable
P0/P1/P2 blocker remains. The original-content limits below still apply.

## Mustang Fastback (`vpbullet`)

Each flat-course run has 901 original samples and 900 aligned comparison
samples over 15 seconds. The turn uses the repeated run with settled idle
RPM; the earlier turn started below idle and is excluded.

| Scenario | Speed RMS / max (m/s) | Planar position RMS / max (m) | Heading RMS / max (degrees) | RPM RMS / max |
| --- | --- | --- | --- | --- |
| Turn | 0.00537 / 0.02594 | 0.675 / 1.662 | 0.193 / 0.279 | 1.33 / 7.81 |
| Brake into reverse | 0.06342 / 0.50830 | 0.127 / 0.326 | 0.126 / 0.282 | 47.67 / 370.12 |
| Handbrake turn | 0.02660 / 0.20318 | 0.135 / 0.186 | 0.645 / 0.864 | 7.24 / 62.34 |

Relative suspension height differs by less than one millimetre at every
sample of these three runs. This measures height changes from the settled
starting point; it does not compare absolute model-origin placement.

Steering bytes agree throughout the Mustang ramp, full-lock interval and
release. In the brake run, both select reverse on sample 425 and first swap
pedals on sample 426. The remaining RPM/speed transient is a one-sample
clutch threshold crossing: on sample 432 the original shaft magnitude is
4.5913 rad/s and the native is 4.4713, on opposite sides of the 4.49863 idle
boundary. Both follow the recovered detach/reattach rules; no coefficient
was altered to hide the branch sensitivity.

The earlier city launch is supporting evidence only. Its saved window stops
at four seconds, before uneven terrain and a later collision. It does not
establish flat-course, cornering or crash fidelity.

## Beetle (`vpbug`)

The warm turn uses identical thirteen-second cold-idle duration in both
simulations. Speed RMS/max is 0.01161/0.03542 m/s, planar position
0.104/0.192 m, heading 0.042/0.265 degrees, and RPM 2.28/7.39.
Relative suspension-height error remains below 0.7 mm.

The matching warm brake-into-reverse run has speed RMS/max
0.00343/0.01975 m/s, planar position 0.023/0.054 m, heading
0.020/0.106 degrees, and RPM 0.78/7.76. It records the full stop and
continued reverse acceleration, with all four wheels grounded throughout.

A separate cold turn begins after three seconds of idle, around 558 RPM.
Its larger launch/clutch transient is preserved: speed RMS/max
0.05818/0.87292 m/s, position 0.369/0.878 m, and RPM 31.56/254.09.
The constructor/reset now faithfully starts actual engine spin at zero
while displaying idle RPM. These cold results are not substituted for the
closer warm comparison.

## Powered slides and countersteering

The `powerslide` sequence keeps full throttle throughout. Right steering
starts at frame 300; the handbrake flick lasts 330–347; powered sustain
continues after release 348; left countersteering runs 375–434; steering
returns to neutral 435. Both new original runs begin with verified engine
omega 0, forward first gear, and exactly 600 active settling ticks. The
corresponding native traces use 600 actual ticks, including the Beetle.

Body slip is `atan2(sideways COM velocity, forward COM velocity)`, rather
than a wheel slip field, whose recovered normalized input is saturated.
Both cars slide at more than 15 degrees and at least 5 m/s for all 27 powered
sustain samples after releasing the handbrake (0.45 seconds). Throttle
remains 1.0, handbrake 0.0, and all four wheels stay grounded. Countersteering
begins while both cars are still sliding. Both reverse their yaw rate and
recover under power.

| Car | Original / native peak body slip | Original / native stable recovery frame |
| --- | --- | --- |
| Mustang Fastback | 63.24° / 63.27° | 436 / 436 |
| Beetle | 51.91° / 51.52° | 444 / 444 |

Stable recovery means 30 consecutive samples below 5 degrees of body slip,
at least 5 m/s, after neutral steering. The Mustang is already recovered
when steering returns to neutral; the Beetle reaches that criterion 0.15 s
later. The comparator separately reports flick, sustain, countersteer and
recovery, including lateral speed and yaw rate.

The Beetle measurement exposed a steering-parameter timing error: the
original recorder consumes parameters from the preceding player update,
which reads an older cached car speed. Replaying its measured velocity
through the filter matches every steering byte at a two-frame delay.
The production input path and trace now preserve that cache order. Native
steering mismatches fall from 9 samples to 1; all pedal bytes match. The
remaining single steering byte differs at frame 404 after physical
velocities have already diverged slightly. No tyre coefficient was fitted
to hide that difference.

The full Beetle run has slip-angle RMS/max 0.137°/1.466°, lateral-speed
RMS/max 0.02595/0.27850 m/s and forward-speed RMS/max 0.04596/0.48193 m/s.
Its heading RMS/max is 1.128°/1.621°, with planar-position RMS/max
1.982/5.274 m over 15 seconds. The heading difference accumulates during
countersteering and then remains approximately constant during the long
straight recovery leg. These trajectory limits remain visible in the
saved raw traces and comparison report.

The full Mustang run has slip-angle RMS/max 0.080°/0.914°, lateral-speed
RMS/max 0.02686/0.30614 m/s and forward-speed RMS/max 0.04652/0.39405 m/s.
Its heading RMS/max is 0.079°/0.331°, with planar-position RMS/max
0.200/0.506 m over 15 seconds. All steering and pedal bytes match. The
remaining speed/RPM transients are retained in the report rather than
removed from the comparison.

## Native coverage

- All 21 stock player cars pass the existing direct-input acceleration,
  stopping, reverse, reset and finite-state probe.
- All 21 pass the drop/landing probe and the power/grip override matrix.
- All 21 complete human-input launch, turn and brake-into-reverse runs:
  63/63 finite, full 901-sample traces; every brake run ends driving in
  reverse. `native-human-roster.json` records individual results.
  The full-lock Ford turn briefly lifts one wheel; the Moonrover turn
  and brake runs lose all wheel contacts at some samples. Those events
  are recorded as finite behavior, not verified retail-equivalent handling.
- All 21 cars also complete the powerslide input sequence finitely. The
  same short flick does not initiate a 15-degree powered slide in every
  car; those differences are recorded. The double-decker and Ford lift
  wheels, and the Moonrover briefly loses all contacts.
  `native-powerslide-roster.json` records these unverified retail comparisons.
- Semi/trailer spawn and reset clearance passes, including the hitch gap.
- Both cities complete 600-frame drives with world collisions, finite state
  and four grounded wheels at the end. London includes one recovery reset;
  `native-city-runtime.txt` preserves the complete records.
- A native Beetle window was driven with keyboard throttle and steering;
  a turn and reset were visually checked. A separate deterministic city
  screenshot completed successfully. Captures remain local.

These native roster probes establish stability and per-car behavior, not
matching original trajectories for all 21 cars. They use default paints;
this project does not claim new full-roster paint/render coverage.

## Remaining limits

Flat-road steering, engine, transmission, tyre and suspension behavior are
closely reproduced. Collision contact resolution, pose integration, body
hull underside clearance and trailer articulation still use Avian adapters.
Impacts, kerbs, banking and articulated crash trajectories have not been
matched against controlled retail recordings. Small heading differences
accumulate into metre-scale separation during a long fast turn. This is a
measured close match for the recorded scenarios, not bit-identical physics.
