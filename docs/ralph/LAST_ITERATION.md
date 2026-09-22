# Last iteration — F02-C.1 roster coverage matrix + reverse-band repair

Iteration 41 on `ralph/night`, continuing from `9b87fbd` (the F00-C.1
teardown-window repair — external review verdict **pass**). This
iteration picks up F02-C, the highest-priority queued task whose
dependencies are met (priority 2): run the complete roster/paint/
handling matrix and publish honest original-vs-synthetic coverage.

## What changed

Three production-side changes plus the published matrix:

- `crates/mm2_app/examples/drive_probe.rs` — new `--controls` mode.
  Per ready car: launch to 15 m/s, full brake to a stop (time +
  distance), held-brake reverse (deepest signed speed +
  `DriveDirection::Reverse` latch), `ResetVehicle` teleport/pose/
  `Teleported` check, and a finiteness leg — every leg through the
  production `vehicle_bundle` + `VehiclePlugin` systems, the same sim
  gameplay runs. Numbers print per car with a per-leg `FAIL(...)`
  verdict; the documented accel/corner table is unchanged.
- `crates/mm2_vehicle/src/systems.rs` + `sim.rs` — **reverse was
  unbounded**, caught by the new matrix's first run (vpbug measured
  -45.6 m/s backing up). The drivetrain torqued through
  `reverse_ratio` but `select_gear` and the RPM tracker ran on the
  forward gearbox, so a held brake upshifted through every forward
  gear in reverse. Now: no gear selection while reversing, wheel-
  implied RPM tracks `reverse_ratio` (new `sim::engine_rpm_at_ratio`),
  and a band-top limiter cuts reverse drive at `upshift_rpm` — the
  `OptRPM` convention `Trans.Reverse` is authored under (`None` →
  `0.92 × redline`, matching `select_gear`'s own default).
- `docs/vehicle-handling.md` — new "Reverse" paragraph recording the
  single-band model and the repair.
- `docs/vehicle-coverage.md` — new published matrix: the 29-id
  denominator, per-car audit table (paints/wheels/mass/margin),
  per-car dynamic table (0-100, top, stop, brake distance, reverse,
  controls verdict, app smoke), findings, and the explicit open legs.

## Root cause classification of the repair

Implementation defect (physics), not a tuning or original-rule issue:
the reverse branch was added with the correct ratio but the shared
gear-selection/RPM bookkeeping was never told the gearbox no longer
existed. `Forward` driving paths are byte-identical — the new code is
gated on `reversing` only. Sessions whose scripted drivers reverse
(bot stuck-escape, recovery re-anchors that back up) may record
different trajectories — bounded now, and the records stay honest.

## Tests

- `mm2_vehicle::drive::reverse_speed_stays_bounded_at_the_reverse_gear`
  (+1): holds the brake 30 s on the dev config; asserts
  `DriveDirection::Reverse` latches, the car backs up past -2 m/s, and
  the speed stays within 5% of the band top implied by
  `upshift_rpm`/`reverse_ratio`/`final_drive`/wheel radius — the old
  code overshoots it ~2x inside 5 s.
- The `--controls` probe is the per-car instrument; its roster run is
  the recorded evidence (below). The example carries no `#[cfg(test)]`
  — the shared legs (brake-to-stop, reverse latch, reset teleport,
  finiteness) already have synthetic coverage in `drive.rs`.

## Gates

- `cargo fmt --all -- --check` — pass.
- `cargo clippy --locked -p mm2_vehicle -p mm2_app --all-targets
  --all-features -- -D warnings` — pass (workspace run below).
- `cargo test -q -p mm2_vehicle` — pass (incl. new test).
- Full `cargo test --locked --workspace` — recorded in the run log;
  expected unchanged outside mm2_vehicle.

## Evidence (retail `fnv1a64:e91e6cd4b2ae30d9`, dev build, 2026-09-22)

- `mm2-inspect cars`: 29 ids discovered, 21 expected stock all `ready`,
  8 unlisted extras `incomplete` with named gaps (denominator intact).
- `mm2-inspect validate-cars`: 21/21 `ok`; warnings on `vpcentury`
  (trailer hitch offsets absent → derived fallback) and `vpford`
  (4 declared vs 5 model paints → 4/5). `--all`: the 8 extras FAIL with
  explicit reasons. `--strict` exits 2 (warnings count).
- `mm2-inspect handling --strict`: exit 0 — all 21 inside the arcade
  envelope (margin 1.87–5.75).
- `drive_probe` accel/corner roster run: 0-100 in 2.8–14.1 s, tops
  16.9–124 m/s, cornering 0.26–1.49 g; `vpmoonrover` NaN 0-100,
  131° drift (pre-existing).
- `drive_probe --controls` roster run: **20/21 all legs ok** —
  stops in 1.0–2.1 s over 7.8–15.9 m, reverse tops -9.0 to -22.5 m/s
  at each car's authored band top (vpbug -13.6 vs implied ~13.4,
  vppanozgt -22.5 vs ~22.6), every reset teleports with `Teleported`
  and cleared motion, every run finite. `vpmoonrover` FAIL(drive):
  6.0 m/s in the 10 s launch — wandering moon-buggy handling, recorded
  open (stop/reverse/reset/finite all pass).
- `mm2 --car <id> --city sf --headless --frames 120` × 21: all
  `status=pass`, `wheels=N/N`, finite final pose through the real app
  path. `vpcab`/`vpcop` each recorded one ambient impact+damage+stuck
  episode during the hold — ambient behaviour, not a failure.

## Remaining gaps (open, recorded in docs/vehicle-coverage.md)

- Per-car rendered frame (F02-AC03 GPU leg) not captured.
- AC04 power/mass override measurement through `--vehicle-config` not
  run (synthetic traction causality exists).
- AC05 spawn-clearance on real geometry + trailer articulation checks
  not run; `vpmoonrover` launch finding unresolved (authored quirk vs
  rig defect — unverified).
- Per-car landing leg not run (landing covered synthetically only).
- `vpmoonrover`'s `drive` leg is the only matrix failure; it stays in
  the denominator, not worked around.

Files: `crates/mm2_app/examples/drive_probe.rs`,
`crates/mm2_vehicle/src/{systems.rs,sim.rs}`,
`crates/mm2_vehicle/tests/drive.rs`, `docs/vehicle-coverage.md`,
`docs/vehicle-handling.md`, `docs/ralph/{PLAN.md,LAST_ITERATION.md}`.
