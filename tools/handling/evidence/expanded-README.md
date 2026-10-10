# Expanded keyboard handling verification, 2026-10-10

These measurements supersede the earlier baseline for the current physics model. The extension starts from `692fcfb6`; executable, retail archive and self-authored PSDL fingerprints remain those in [README.md](README.md). The user's retail installation was unchanged; [restoration proof](expanded-original-restoration.json) records guest archive, hooks, clocks, selection and renderer restoration followed by a fresh reload.

The expanded set contains 12 distinct car/scenario pairs across six additional cars: Bus, Panoz, Mini, F350, Audi TT and the hidden Moonrover. Together with the earlier Mustang Fastback and Beetle, eight cars have paired original recordings. Thirteen of the 21 native catalog cars remain unpaired. Every expanded original capture has 901 samples, with all 900 aligned samples compared; settling matches 600, 780 or 1,200 actual 60 Hz ticks. Live mass, inertia dimensions, engine tuning and wheel radii verify selection. The first Bus recording's corrected menu label remains explicit rather than hiding the original's silent fallback.

Inputs cover repeated steering reversals, throttle lift/reapplication, braking into a turn followed by powered countersteering, and handbrake flick, powered slide sustain, countersteering and recovery. Steering byte comparisons match on every expanded pair. Pedal bytes are compared only on full-throttle powerslide sequences because the original records effective pedals after assists while native trace pedals are raw keyboard inputs.

## Corrections established by measurement

The F350 exposed missing axle spring/damper torque. Before correction its slalom differed on 139 wheel-contact frames and by up to 35.59 m in position. Live tuning and executable routines confirmed nonzero authored axle coefficients; the previous research claim that all retail axles were inactive was wrong. The model now applies the recovered torque formula, without fitting coefficients to trajectories.

Independent Moonrover wheel diagnostics exposed a second import error: mesh width was divided by two. The original's four widths were approximately 0.388847 m while native widths were half that. Full width restores the tyre force arm and associated suspension/roll response across the roster.

Body contact now uses the original authored hull, raw friction products and capped elasticity products, effective-mass impulse calculation, delayed momentum application and non-kinetic penetration push. Avian generates contact candidates but its generic solver is suppressed for native vehicles against marked static world geometry; contact manifolds are restored before sleeping and impact consumers. Generic dynamic props retain their existing response. Swept contact midpoint reconstruction is inferred from planar original measurements; compatible normals are required except for a solid ray starting inside the contacted collider. This is not proof of identical collision detection at corners or on arbitrary meshes.

The native fixture's previous enormous cuboid also generated spurious oblique floor normals and extreme impulses. Replacing it with an exact halfspace removes that harness artifact. Both fixtures use the same cold spawn, tyre friction 1.0 and separately measured hull/floor material products.

## Final paired trajectories

Errors use the complete 15-second sequences; position is relative center-of-mass trajectory error. Contact mismatches include wheel-count differences, with individual flags available in reports.

| Car / scenario | Speed RMS / max (m/s) | Position RMS / max (m) | Heading RMS / max (degrees) | Contact mismatches / 900 |
| --- | --- | --- | --- | --- |
| vpbus / slalom | 0.00138 / 0.01357 | 0.004 / 0.007 | 0.001 / 0.004 | 0 |
| vpbus / brake_turn | 0.00333 / 0.03433 | 0.032 / 0.087 | 0.029 / 0.059 | 0 |
| vppanoz / slalom | 0.00050 / 0.00670 | 0.031 / 0.072 | 0.007 / 0.010 | 0 |
| vppanoz / powerslide | 0.03670 / 0.50317 | 0.098 / 0.240 | 0.012 / 0.032 | 0 |
| vpcoop / lift_turn | 0.00236 / 0.02177 | 0.016 / 0.041 | 0.010 / 0.029 | 0 |
| vpcoop / slalom | 0.00192 / 0.02270 | 0.033 / 0.076 | 0.015 / 0.018 | 0 |
| vpford / slalom | 0.00548 / 0.02343 | 0.021 / 0.032 | 0.006 / 0.008 | 0 |
| vpauditt / brake_turn | 0.00216 / 0.01315 | 0.214 / 0.606 | 0.112 / 0.174 | 0 |
| vpauditt / powerslide | 0.00191 / 0.02381 | 0.030 / 0.080 | 0.016 / 0.021 | 0 |
| vpmoonrover / lift_turn | 0.05353 / 0.61284 | 0.743 / 1.667 | 0.360 / 0.800 | 4 |
| vpmoonrover / powerslide | 0.01587 / 0.11075 | 0.205 / 0.401 | 0.233 / 0.592 | 1 |
| vpford / powerslide | 0.00287 / 0.03024 | 0.026 / 0.066 | 0.013 / 0.018 | 0 |

[Machine-readable comparisons](expanded-comparisons.json) retain full errors, transitions, contact flags, phase metrics and recovery criteria. The extra Moonrover lift replay after 780 settling ticks is a diagnostic repeat, not a thirteenth distinct pair. Its complete report is `vpmoonrover-flat-lift_turn-diagnostics-comparison.json`. Moonrover is still the largest residual case; these discrepancies remain visible rather than excluded from the denominator. Neither original nor native becomes fully airborne in these paired manoeuvres.

## Powerslide verification

- `vppanoz`: peak countersteer slip 30.008° original / 30.059° native; powered sustain 0.433 / 0.433 s; recovery frame 436 / 436.
- `vpauditt`: peak countersteer slip 48.634° original / 48.664° native; powered sustain 0.450 / 0.450 s; recovery frame 436 / 436.
- `vpmoonrover`: peak countersteer slip 25.311° original / 25.101° native; powered sustain 0.450 / 0.450 s; recovery frame 436 / 436.
- `vpford`: peak countersteer slip 36.201° original / 36.205° native; powered sustain 0.150 / 0.150 s; recovery frame 446 / 446.

Current Mustang and Beetle rechecks are the seven `*-current-comparison.json` reports; older `with-axles` traces document an intermediate state. [Final native roster checks](native-final-roster-rechecks.json) exercise 21 cars × seven sequences = 147 finite 901-frame runs, using production human input. This verifies native stability and does not establish original equivalence for unpaired cars.

Verbose original wheel diagnostics are retained in ignored local storage. Public originals preserve every trajectory frame; capture metadata records both original full-capture and public derivative hashes. Public files contain numeric telemetry and tuning evidence, without retail assets or executable dumps.

Final formatting and strict Clippy passed; the workspace suite passed 3,071 tests in 59 suites with zero failures or ignored tests. [Code gates](expanded-quality-gates.json) record the source digest and exact commands after calibration. General city collision geometry, gamepad handling and all unpaired original cars remain outside this measured equivalence claim.

A [fresh independent review](expanded-independent-review.json) found no actionable P0/P1/P2 blockers after code gates. The rebuilt Metal app was [verified with live keyboard input](expanded-playable-verification.json), including acceleration, steering, handbrake, stable wall contact and reset, then left open at the road spawn. The [playable app](../../../target/Rust%20MM2%20Handling.app) launches San Francisco with the Beetle and `--no-profile`. Arrow keys drive, Space applies the handbrake and R resets.
