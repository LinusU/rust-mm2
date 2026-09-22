# Vehicle coverage matrix

F02-C's published account of what the stock roster actually does under
the project's own instruments — measured, not claimed. Every retail
number below comes off the fingerprinted install
(`fnv1a64:e91e6cd4b2ae30d9`, read-only) in one pass on 2026-09-22; the
synthetic legs are the `mm2_vehicle` / `mm2_app` test suites. Rendering
per car is not claimed — see "Open legs".

## Instruments

```sh
mm2-inspect cars <install>                      # roster vs EXPECTED_STOCK_ROSTER
mm2-inspect validate-cars <install> [--all]     # metadata, tuning, model, rig, collider, paints
mm2-inspect handling <install> [--strict]       # analytic metrics per car
cargo run -p mm2_app --example drive_probe -- <install>             # accel + cornering
cargo run -p mm2_app --example drive_probe -- <install> --controls  # launch/brake/reverse/reset
mm2 --mm2-path <install> --city sf --car <id> --headless --frames 120
```

`drive_probe` runs the production `vehicle_bundle` + `VehiclePlugin`
systems on flat ground — the same systems gameplay runs — so its
numbers are simulation evidence, not parser evidence. The `--car`
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

`validate-cars` on the expected roster: **21/21 ok**, warnings on 2
(`vpcentury` — `vehTrailer` hitch offsets absent, conversion derives a
fallback; `vpford` — metadata declares 4 paints, the model has 5, so
4/5 validated). `validate-cars --strict` exits 2 on those warnings;
`handling --strict` exits 0 — every ready car sits inside the arcade
envelope.

## Audit table

Per `cars` + `validate-cars` + `handling` (`margin` = rollover margin
after the roll-resistance assist; see `docs/vehicle-handling.md`):

| id | name | paints | wheels | mass kg | margin | audit |
| --- | --- | --- | --- | --- | --- | --- |
| vp4x4 | Light Tactical Vehicle | 5/5 | 4 | 2500 | 5.11 | ok |
| vpauditt | Audi TT | 5/5 | 4 | 1300 | 3.60 | ok |
| vpbug | VW New Beetle | 4/4 | 4 | 1000 | 3.98 | ok |
| vpbullet | Ford Mustang Fastback | 5/5 | 4 | 1300 | 4.01 | ok |
| vpbus | City Bus | 4/4 | 4 | 5000 | 5.11 | ok |
| vpcab | London Cab | 2/2 | 4 | 1000 | 3.07 | ok |
| vpcaddie | Cadillac Eldorado | 4/4 | 4 | 1300 | 4.68 | ok |
| vpcentury | Freightliner Century | 4/4 | 6 | 3500 | 1.87 | ok (+2 warn) |
| vpcoop | Mini Cooper Classic | 5/5 | 4 | 800 | 3.22 | ok |
| vpcoop2k | NEW MINI COOPER | 5/5 | 4 | 800 | 3.22 | ok |
| vpcop | Ford Mustang Cruiser | 2/2 | 4 | 1300 | 4.40 | ok |
| vpdb7 | Aston Martin DB7 Vantage | 5/5 | 4 | 1573 | 3.87 | ok |
| vpddbus | Double-Decker Bus | 4/4 | 4 | 4915 | 2.23 | ok |
| vpdune | VW New Beetle Dune | 5/5 | 4 | 1000 | 3.44 | ok |
| vpford | Ford F-350 | 4/5 | 4 | 2500 | 3.59 | ok (+1 warn) |
| vpmoonrover | Moon Rover | 1/1 | 6 | 2500 | 3.93 | ok |
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
| vpcentury | 4.7 s | 54.0 | 1.4 | 10.3 | -9.1 | ok | pass 6/6 |
| vpcoop | 6.7 s | 52.8 | 1.1 | 8.2 | -13.6 | ok | pass 4/4 |
| vpcoop2k | 6.7 s | 70.4 | 1.1 | 7.9 | -13.6 | ok | pass 4/4 |
| vpcop | 2.9 s | 74.7 | 1.2 | 8.9 | -15.8 | ok | pass 4/4 |
| vpdb7 | 5.1 s | 80.2 | 1.1 | 8.5 | -15.9 | ok | pass 4/4 |
| vpddbus | 9.4 s | 46.7 | 1.2 | 8.6 | -13.6 | ok | pass 4/4 |
| vpdune | 5.7 s | 69.6 | 1.4 | 7.9 | -13.8 | ok | pass 4/4 |
| vpford | 4.4 s | 47.7 | 1.2 | 9.0 | -15.8 | ok | pass 4/4 |
| vpmoonrover | n/a | 16.9 | 0.6 | 1.4 | -16.0 | FAIL(drive) | pass 6/6 |
| vpmustang99 | 3.0 s | 61.4 | 1.1 | 8.2 | -15.7 | ok | pass 4/4 |
| vppanoz | 5.2 s | 80.2 | 1.0 | 7.8 | -18.2 | ok | pass 4/4 |
| vppanozgt | 2.8 s | 124.0 | 1.2 | 8.8 | -22.5 | ok | pass 4/4 |
| vpsemi | 5.8 s | 40.4 | 1.6 | 11.7 | -9.0 | ok | pass 4/4 |
| vpvwcup | 5.0 s | 80.0 | 1.0 | 7.8 | -13.6 | ok | pass 4/4 |

`app smoke` = `status=pass`, `wheels=N/N`, finite pose after 120
headless frames on `city/sf.psdl` (hold-throttle driver). Every ready
car loads through the full app path and stays finite.

## Findings

- **vpmoonrover's launch is weak and wanders.** It reaches only
  6.0 m/s in the 10 s launch window (the one `drive` leg failure), no
  0-100 time, 131° of heading drift in the accel probe. Its
  stop/reverse/reset/finite legs all pass and it loads cleanly in the
  app. Whether the wander is authored behaviour for the unlisted moon
  buggy or a rig defect is **open** — recorded, not repaired or hidden.
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
  probe (0.26–1.49 g measured across 10–40 m/s) and left/right +
  landing + teleport legs only at the shared synthetic level
  (`mm2_vehicle::drive` tests). A per-car landing leg is not run.
- **F02-AC03, render leg.** Every declared paint resolves through the
  pipeline (the `paints` column); a rendered frame per stock car is not
  yet captured on this machine's GPU.
- **F02-AC04, override causality.** Surface-grip modifier causality is
  proven synthetically (`surface.rs` tests); a per-car power/mass
  override measurement through `--vehicle-config` is not yet run.
- **F02-AC05, spawn clearance.** `vpmoonrover` (6-wheel), `vpsemi` /
  `vpcentury` / `vpddbus` (heavy/tall) reset cleanly on flat ground;
  spawn-without-penetration evidence on real road geometry and trailer
  articulation checks are not run here.
- **F02-AC06, startup paths.** Both-city launches and invalid-car CLI
  errors are covered by existing tests; not re-run in this matrix.
