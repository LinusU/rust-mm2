# Vehicle coverage matrix

F02-C's published account of what the stock roster actually does under
the project's own instruments — measured, not claimed. Every retail
number below comes off the fingerprinted install
(`fnv1a64:e91e6cd4b2ae30d9`, read-only); catalog/dynamic legs were run
on 2026-09-22, the render captures and the per-car override sweep on
2026-09-24; the synthetic legs are the `mm2_vehicle` / `mm2_app` test
suites.

## Instruments

```sh
mm2-inspect cars <install>                      # roster vs EXPECTED_STOCK_ROSTER
mm2-inspect validate-cars <install> [--all]     # metadata, tuning, model, rig, collider, paints
mm2-inspect handling <install> [--strict]       # analytic metrics per car
cargo run -p mm2_app --example drive_probe -- <install>             # accel + cornering
cargo run -p mm2_app --example drive_probe -- <install> --controls  # launch/brake/reverse/reset
cargo run -p mm2_app --example drive_probe -- <install> --drop      # per-car landing
cargo run -p mm2_app --example drive_probe -- <install> --city <c> --clearance  # spawn/reset/trailer on real roads
cargo run -p mm2_app --example drive_probe -- <install> <id> --dump-config <file>
cargo run -p mm2_app --example drive_probe -- <install> <id> --config <file>    # override through the --vehicle-config path
mm2 --mm2-path <install> --city sf --car <id> --headless --frames 120
```

`drive_probe` runs the production `vehicle_bundle` + `VehiclePlugin`
systems — the same systems gameplay runs — so its numbers are
simulation evidence, not parser evidence. `--drop`/`--controls`/`--city`
run on a flat slab or real city colliders respectively; `--clearance`
spawns every ready car (plus its trailer rig through the production
`spawn_trailer` path) at the probe's city road midpoint, settles until
still, then exercises `ResetVehicle` back to spawn. The `--car`
headless records are the full app session on real SF geometry.

## The denominator (F02-AC01)

`VehicleCatalog::scan` discovers **29** vehicle ids. The expected stock
roster is **21** and all 21 resolve `ready` — none missing, none
degraded. The remaining 8 are unlisted extras the install ships
incomplete; they stay in the denominator and fail validation with an
explicit reason each (`validate-cars --all`):

| id | why it cannot drive |
| --- | --- |
| vpdb731 | metadata + tuning + bounds missing |
| vpeagle | model + bounds + wheel transforms missing |
| vpftruck | metadata missing; `vehCarSim.Engine` lacks `IdleRPM` |
| vplafrance | metadata + tuning missing |
| vpvw_cup | metadata + tuning missing |
| vpvw_dune | tuning missing |
| vpvwcup_angel | metadata + model + wheel transforms missing; `WheelFront` lacks `TireDragCoefLong` |
| vpvwdune | metadata + tuning missing |

`validate-cars` on the expected roster: **21/21 ok**, warnings on 3
(`vpcentury` — `vehTrailer` hitch offsets absent so conversion derives
a fallback, its `TWHL0`/`TWHL1` parts are decorative 5 cm detail
excluded from the physics rig, and its `whl4`/`whl5` tandem-axle parts
are visual back-back followers of `whl2`/`whl3`; `vpmoonrover` — the
same `whl4`/`whl5` follower pair; `vpford` — metadata declares 4
paints, the model has 5, so 4/5 validated). The follower warnings are
the retail `vehCarSim`'s own architecture: it carries exactly four
`vehWheel` slots and draws `whl4`/`whl5` as `whl2`/`whl3` plus a stored
offset — verified in mm2hook's `vehCarSim::Init`/`Draw` sources.
`validate-cars --strict` exits 2 on those warnings;
`handling --strict` exits 0 — every ready car sits inside the arcade
envelope.

## Audit table

Per `cars` + `validate-cars` + `handling` (`margin` = rollover margin
after the roll-resistance assist; see `docs/vehicle-handling.md`;
`wheels` = visual wheel parts → physics wheels — the retail carsim
simulates at most four `vehWheel` slots, so a 6-wheel model's extra
`whlN` parts are visual followers):

| id | name | paints | wheels | mass kg | margin | audit |
| --- | --- | --- | --- | --- | --- | --- |
| vp4x4 | Light Tactical Vehicle | 5/5 | 4 | 2500 | 5.11 | ok |
| vpauditt | Audi TT | 5/5 | 4 | 1300 | 3.60 | ok |
| vpbug | VW New Beetle | 4/4 | 4 | 1000 | 3.98 | ok |
| vpbullet | Ford Mustang Fastback | 5/5 | 4 | 1300 | 4.01 | ok |
| vpbus | City Bus | 4/4 | 4 | 5000 | 5.11 | ok |
| vpcab | London Cab | 2/2 | 4 | 1000 | 3.07 | ok |
| vpcaddie | Cadillac Eldorado | 4/4 | 4 | 1300 | 4.68 | ok |
| vpcentury | Freightliner Century | 4/4 | 6→4 | 3500 | 1.87 | ok (+6 warn) |
| vpcoop | Mini Cooper Classic | 5/5 | 4 | 800 | 3.22 | ok |
| vpcoop2k | NEW MINI COOPER | 5/5 | 4 | 800 | 3.22 | ok |
| vpcop | Ford Mustang Cruiser | 2/2 | 4 | 1300 | 4.40 | ok |
| vpdb7 | Aston Martin DB7 Vantage | 5/5 | 4 | 1573 | 3.87 | ok |
| vpddbus | Double-Decker Bus | 4/4 | 4 | 4915 | 2.23 | ok |
| vpdune | VW New Beetle Dune | 5/5 | 4 | 1000 | 3.44 | ok |
| vpford | Ford F-350 | 4/5 | 4 | 2500 | 3.59 | ok (+1 warn) |
| vpmoonrover | Moon Rover | 1/1 | 6→4 | 2500 | 3.97 | ok (+2 warn) |
| vpmustang99 | Ford Mustang GT | 4/4 | 4 | 1300 | 4.40 | ok |
| vppanoz | Panoz Roadster | 5/5 | 4 | 1300 | 4.28 | ok |
| vppanozgt | Panoz GTR-1 | 5/5 | 4 | 1200 | 5.75 | ok |
| vpsemi | American LaFrance Fire Truck | 4/4 | 4 | 3500 | 4.59 | ok |
| vpvwcup | VW New Beetle RSi | 7/7 | 4 | 1000 | 4.61 | ok |

## Dynamic table

`drive_probe` acceleration/cornering (25 s flat-ground launch +
steady-state full-lock cornering) and `--controls` (launch to 15 m/s,
full brake to stop, held-brake reverse, `ResetVehicle`, finiteness —
each leg through the production systems):

| id | 0-100 km/h | top m/s | stop s | brake m | rev m/s | controls | app smoke |
| --- | --- | --- | --- | --- | --- | --- | --- |
| vp4x4 | 4.9 s | 47.7 | 2.1 | 15.9 | -15.9 | ok | pass 4/4 |
| vpauditt | 3.0 s | 67.1 | 1.4 | 10.3 | -15.8 | ok | pass 4/4 |
| vpbug | 5.4 s | 59.1 | 1.1 | 8.2 | -13.6 | ok | pass 4/4 |
| vpbullet | 4.4 s | 61.5 | 1.5 | 12.6 | -13.6 | ok | pass 4/4 |
| vpbus | 14.1 s | 29.0 | 1.8 | 13.7 | -13.5 | ok | pass 4/4 |
| vpcab | 5.7 s | 62.4 | 2.0 | 13.7 | -10.5 | ok | pass 4/4 |
| vpcaddie | 3.1 s | 61.5 | 1.8 | 13.6 | -15.8 | ok | pass 4/4 |
| vpcentury | 6.9 s | 54.0 | 2.3 | 24.2 | -9.3 | ok | pass 4/4 |
| vpcoop | 6.7 s | 52.8 | 1.1 | 8.2 | -13.6 | ok | pass 4/4 |
| vpcoop2k | 6.7 s | 70.4 | 1.1 | 7.9 | -13.6 | ok | pass 4/4 |
| vpcop | 2.9 s | 74.7 | 1.2 | 8.9 | -15.8 | ok | pass 4/4 |
| vpdb7 | 5.1 s | 80.2 | 1.1 | 8.5 | -15.9 | ok | pass 4/4 |
| vpddbus | 9.4 s | 46.7 | 1.2 | 8.6 | -13.6 | ok | pass 4/4 |
| vpdune | 5.7 s | 69.6 | 1.4 | 7.9 | -13.8 | ok | pass 4/4 |
| vpford | 4.4 s | 47.7 | 1.2 | 9.0 | -15.8 | ok | pass 4/4 |
| vpmoonrover | n/a | 12.9 | 0.1 | 0.1 | -16.0 | FAIL(drive) | pass 4/4 |
| vpmustang99 | 3.0 s | 61.4 | 1.1 | 8.2 | -15.7 | ok | pass 4/4 |
| vppanoz | 5.2 s | 80.2 | 1.0 | 7.8 | -18.2 | ok | pass 4/4 |
| vppanozgt | 2.8 s | 124.0 | 1.2 | 8.8 | -22.5 | ok | pass 4/4 |
| vpsemi | 5.8 s | 40.4 | 1.6 | 11.7 | -9.0 | ok | pass 4/4 |
| vpvwcup | 5.0 s | 80.0 | 1.0 | 7.8 | -13.6 | ok | pass 4/4 |

`app smoke` = `status=pass`, `wheels=N/N`, finite pose after 120
headless frames on `city/sf.psdl` (hold-throttle driver). Every ready
car loads through the full app path and stays finite. The
`vpcentury`/`vpmoonrover` stop cells were re-measured on 2026-09-24 —
the earlier values predated F02-C.3's 6→4 physics-wheel rig, which
moved their braking onto four wheels.

## Landing and spawn clearance

`--drop` (flat slab, released airborne, settle measured from first
wheel contact): **21/21 ok** — airborne 0.88–0.92 s, peak sink
8.5–8.8 m/s, settled upright (`up` = 1.00) in 0.90–1.43 s, drove away
(5.2–21.9 m), finite throughout.

`--clearance` on real road geometry (`city/sf.psdl` and
`city/london.psdl`, probe's road-midpoint spawn, settled until still,
then `ResetVehicle` and re-settled): **20/21 ok on both cities** —
every wheel grounded, zero hull contacts at spawn and reset. The two
trailer rigs report tractor and trailer separately: `vpcentury`
4/4 + trailer 4/4, `vpsemi` 4/4 + trailer 4/4, hitch-anchor gap
0.00 m after spawn and reset on both. The one failure is
`vpmoonrover`, which now rests on its authored statics: its
`CenterOfGravity` z-offset (+0.4 m) puts the centre of mass behind the
physics rear axle, so the nose lifts, the tail hull rests on the road
(penetration 0.000 — touching, not interpenetrating), and the front
wheels hang at droop. That is the retail four-wheel rig's own
equilibrium, not a spawn defect — see the moon-rover finding below.

## Rendered-output evidence (F02-AC03)

Per-car GPU captures, 2026-09-24, on the fingerprinted install —
Apple M1 / Metal, windowed run, SF roam, default paint. Every car was
pinned to the same flat waterfront spawn with the same fixed free
camera, so the frames are directly comparable:

```sh
mm2 --mm2-path <install> --city sf --car <id> \
    --spawn=-141.9,1.5,-608.5,115 \
    --cam=-144.9,7.0,-588.5,-8.5,-15 \
    --frames 120 --screenshot screenshots/f02c-render/<id>.png
```

Result: **21/21 `smoke=visual status=pass`**, each PNG inspected — the
named car renders recognisably at its default paint on real SF
geometry. Both trailer rigs (`vpcentury`, `vpsemi`) draw the hitched
trailer; `vpmoonrover` renders its authored nose-up stance; the two
Beetle-based cars (`vpbug`/`vpvwcup`) and the two Minis
(`vpcoop`/`vpcoop2k`) are visually distinct models, not the same mesh
re-textured. Captures stay local under `screenshots/f02c-render/`
(gitignored — original content is not committed).

## Override causality (F02-AC04)

`--dump-config` writes the imported `VehicleConfig` TOML; `--config`
re-loads an edited file through the same `apply_handling_override`
the app's `--vehicle-config` uses (wheel positions/radii and collision
geometry stay pinned to the imported rig; a wheel-count change is
rejected — vpbug + the 6-wheel `vpmoonrover` tune errors
"override defines 6 wheels but the selected vehicle's rig has 4").

Per-car sweep, 2026-09-24 (same install, this commit): each car's
dumped config was edited twice — `peak_torque_nm` **and** `max_power_w`
halved together (`engine ×0.5`, so the cap reaches the whole rev band,
not only the power-limited top), and every `longitudinal_grip` field
(the four wheel entries + the global `[tires]` value) halved
(`grip ×0.5`) — then measured through the accel probe and the
`--controls` brake leg. `n/r` = did not reach 100 km/h inside the
25 s window.

| id | 0-100 base→eng | top base→eng m/s | stop base→grip s | dist base→grip m |
| --- | --- | --- | --- | --- |
| vp4x4 | 4.9→8.5 | 47.7→44.4 | 2.1→3.0 | 15.9→22.0 |
| vpauditt | 3.0→4.3 | 67.1→66.8 | 1.4→2.3 | 10.3→17.0 |
| vpbug | 5.4→7.5 | 59.1→44.9 | 1.1→1.9 | 8.2→14.5 |
| vpbullet | 4.4→4.3 | 61.5→61.2 | 1.5→2.4 | 12.6→18.2 |
| vpbus | 14.1→n/r | 29.0→22.1 | 1.8→3.4 | 13.7→25.3 |
| vpcab | 5.7→6.1 | 62.4→51.3 | 2.0→2.7 | 13.7→20.4 |
| vpcaddie | 3.1→4.4 | 61.5→61.2 | 1.8→2.2 | 13.6→17.0 |
| vpcentury | 6.9→8.8 | 54.0→53.8 | 2.3→1.8 | 24.2→13.7 |
| vpcoop | 6.7→7.4 | 52.8→50.7 | 1.1→1.8 | 8.2→13.6 |
| vpcoop2k | 6.7→7.0 | 70.4→52.2 | 1.1→1.8 | 7.9→13.5 |
| vpcop | 2.9→3.4 | 74.7→74.4 | 1.2→1.9 | 8.9→14.0 |
| vpdb7 | 5.1→5.9 | 80.2→62.1 | 1.1→2.1 | 8.5→16.0 |
| vpddbus | 9.4→17.7 | 46.7→33.7 | 1.2→1.8 | 8.6→13.6 |
| vpdune | 5.7→6.1 | 69.6→53.5 | 1.4→2.1 | 7.9→16.0 |
| vpford | 4.4→8.5 | 47.7→45.3 | 1.2→1.9 | 9.0→14.1 |
| vpmoonrover | n/r→n/r | 12.9→1.6 | 0.1→0.2 | 0.1→0.2 |
| vpmustang99 | 3.0→4.6 | 61.4→61.1 | 1.1→1.9 | 8.2→14.1 |
| vppanoz | 5.2→5.3 | 80.2→77.8 | 1.0→2.0 | 7.8→14.8 |
| vppanozgt | 2.8→3.2 | 124.0→99.4 | 1.2→2.3 | 8.8→17.4 |
| vpsemi | 5.8→10.6 | 40.4→31.1 | 1.6→2.6 | 11.7→19.5 |
| vpvwcup | 5.0→5.2 | 80.0→60.4 | 1.0→1.9 | 7.8→14.3 |

Read: every ready car's measurement moves under both overrides — the
override channel reaches the live simulation on the whole roster, not
just on `vpbug` where the earlier pass stopped. Cells that saturate,
recorded rather than argued away:

- **Launch traction-limited.** `vpbullet`'s 0-100 sits at parity
  (4.4→4.3): its engine already exceeds what the tires put down, so
  halving output briefly *improves* the run before the unsaturated
  midrange falls behind — the `--trace` comparison diverges from
  t≈3 s (47.8 vs 38.9 m/s at t=8). The same masking keeps `vppanoz`'s
  0-100 near parity while its trace diverges from t≈4 s (72.5 vs
  64.6 m/s at t=14).
- **Top speed rev-limited, not power-limited.** `vpauditt`,
  `vpbullet`, `vpcaddie`, `vpcentury`, `vpcop`, `vpmustang99` and
  `vppanoz` lose under 1% of top speed — they hit the limiter below
  the speed half the power can still reach. Their engine-leg delta
  shows in the 0-100/trace numbers instead.
- **`vpbus`** no longer reaches 100 km/h in the window (n/r); its
  delta is the −24% top speed.
- **`vpmoonrover`** never launches (authored nose-up equilibrium —
  findings below); the halved engine collapses its top to 1.6 m/s.
  Its stop leg is unsuitable either way: it brakes from ~1 m/s, so
  0.1→0.2 m is noise, not a measurement.
- **`vpcentury` stops *shorter* under halved grip** (24.2→13.7 m; a
  0.75× point lands at 11.2 m — non-monotonic). The measurement moves
  so the override is causal, but the direction is anomalous: the
  truck brakes inside the wheel-lock regime, where the grip scale
  changes which slip state the solver settles into rather than
  scaling deceleration linearly. Mechanism unverified — flagged for
  the handling owner, not retuned here.

Single-car rows from the earlier pass, kept for the record (vpbug,
same install): `longitudinal_grip` halved → 0-100 5.4→9.4 s
(traction-limited); `mass` doubled → 0-100 5.4→23.6 s (suspension
bottoms out — expected, not a defect).

## Findings

- **A reused `--screenshot` path could pass on stale pixels —
  fixed.** `smoke_test`'s capture-wait accepted any non-empty file at
  the target, so a second run to the same path reported
  `status=pass` on the previous image while the fresh save was still
  in flight — found while staging the render leg above. The runner
  now clears the target before requesting the capture (only this
  run's write can satisfy `landed`) and fails explicitly when the
  target cannot be cleared. Regression test:
  `smoke::tests::a_stale_capture_is_cleared_before_the_new_request`.
- **vpmoonrover's brokenness is authored, not a rig defect —
  resolved.** The model carries six wheel parts, but the retail
  `vehCarSim` has exactly four `vehWheel` slots: mm2hook's
  `vehCarSim::Init` binds `whl0`–`whl3` and records
  `BackBackLeft/RightWheelPosDiff = whl4/whl5 pivot − whl2/whl3
  centre`, and `vehCarModel::Draw` renders `whl4`/`whl5` as the
  reference wheel's matrix plus that offset — visual followers, no
  force. We previously simulated all six as independent corners, a
  statically-indeterminate rig that porpoised and wandered (131°
  drift). Now `build_model` flags `whl` index ≥ 4 as `!simulated`
  followers of `whl(N−2)` (regression test
  `wheels_beyond_four_are_back_back_followers`), `assemble` emits only
  the four physics wheels, and the follower mounts copy the reference
  wheel's live droop/steer/spin plus the authored offset. What remains
  is authored character on a cut test vehicle: `CenterOfGravity`
  (+0.4 m rearward of the bound centre) sits behind the physics rear
  axle, so the rover rests nose-up on its tail (the `--clearance`
  failure above) and launches weakly (~1 m/s in the window —
  `FAIL(drive)` on `--controls` stands). Community documentation
  reports the retail original misbehaves too — "the rear end
  levitates", which matches: the follower `whl4`/`whl5` copy `whl2/3`'s
  compression at a station where the pitched tail sits lower, so the
  rear wheels visibly float. `vpcentury` (the other 6-wheel model)
  keeps the same rule: its tandem axle is a follower pair; it passes
  every leg, with a launch wheelie transient (0-100 now 6.9 s) before
  settling dead straight at 54 m/s.
- **Reverse was unbounded until this iteration.** The drivetrain
  torqued through `reverse_ratio` but the upshift selector and RPM
  tracking ran on the forward gearbox, so a held brake accelerated the
  car backwards to its forward top speed (vpbug measured -45.6 m/s).
  Reverse is now modelled as the single band the authored data
  describes: RPM tracks the reverse ratio and the band-top limiter cuts
  drive at the `OptRPM` point — the `Trans.Reverse` top speed itself.
  Measured tops now sit at each car's authored reverse (vpbug -13.6 vs
  implied ~13.4, vppanozgt -22.5 vs ~22.6). Regression test:
  `mm2_vehicle::drive::reverse_speed_stays_bounded_at_the_reverse_gear`.
- **The first `--clearance` run found two real trailer defects, now
  fixed.** `vpsemi`'s tractor and trailer hulls held a standing contact
  at the coincident hitch anchors — the spherical joint now carries
  `JointCollisionDisabled` (regression test:
  `mm2_app::tests/trailer`). `vpcentury`'s trailer grounded only 4 of
  6 "wheels": `TWHL0`/`TWHL1` are 5 cm detail parts (landing-gear
  hardware authored under the wheel prefix), not wheels — `build_model`
  flags trailer wheel parts under half the rig's max radius as
  `!simulated`, `load_trailer` excludes them from the physics rig, and
  their visual mounts stay parked at the authored origin.
- **The clearance leg settles adaptively** (until still, capped at
  15 s) rather than reading a transient — a slow wallow does not fail a
  car that reaches equilibrium.
- **Two authored warnings, zero failures.** `vpcentury`'s trailer
  record lacks hitch offsets (conversion derives a fallback) and
  `vpford` declares one paint fewer than its model carries. Both are
  disclosed conversion findings, not silent repairs.
- **Per-vehicle character survives.** 0-100 times span 2.8–14.1 s, top
  speeds 16.9–124 m/s, brake distances 7.8–15.9 m, reverse tops
  -9.0 to -22.5 m/s — the assists homogenise none of it.

## Open legs (not claimed here)

- **F02-AC02, remaining verbs.** Accelerate/brake/reverse/reset are
  covered per car above; steering is covered per car by the cornering
  probe (0.26–1.49 g measured across 10–40 m/s); the per-car landing
  leg is `--drop` above. Left/right and teleport legs run only at the
  shared synthetic level (`mm2_vehicle::drive` tests).
- **F02-AC03, remaining paints.** Every declared paint resolves
  through the pipeline (the `paints` column) and the default paint of
  every ready car is rendered-verified above; captures of the
  *non-default* paints are not run (21 renders, not all ~90 variants).
- **F02-AC06, startup paths — re-run this pass.** Both cities launch
  headless (`status=pass`, 120 updates each, `city/sf.psdl` and
  `city/london.psdl`); an unknown `--car` exits 2 with
  `unknown vehicle id "nonexistent" (see --list-cars)`; the shipped
  `checker-override` mod mounts at VFS priority 300 and renders — a
  dev-world capture shows its magenta checkerboard road, so mod
  content resolves through the real path. Existing synthetic tests
  cover the remainder; the workspace gate is in the iteration report.
