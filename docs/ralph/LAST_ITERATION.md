# Last iteration — F02-C.2: landing, clearance and override probe legs

Iteration 54 on `ralph/night`. Selected the F02-C remainder — the
vehicle-coverage matrix (`docs/vehicle-coverage.md`) still owed the
per-car landing leg, real-geometry spawn clearance and trailer
articulation, and `--vehicle-config` override causality. `drive_probe`
already ran the production `vehicle_bundle` + `VehiclePlugin` systems
headlessly, so the legs extended the same harness rather than a new
one.

## What changed

- `drive_probe` gained `--drop`: every ready car released airborne on a
  flat slab — airborne time, peak sink speed, settle-from-first-contact,
  uprightness, drive-away distance and finiteness per car.
- `drive_probe` gained `--city <sf|london> --clearance`: every ready car
  spawns at the probe's road midpoint on real city colliders, settles
  until still (adaptive, capped 15 s — the Moon Rover wallows >6 s off
  the ~1.5 m spawn drop), then `ResetVehicle` back to spawn and
  re-settles. Trailer rigs spawn through the production
  `car_visual::spawn_trailer`, are checked independently, and report the
  hitch-anchor world gap. Failures print the contact names, manifold
  points and per-wheel ground truth.
- `--config <toml>` applies a handling override through
  `mm2_content::assemble::apply_handling_override` — the same path
  `--vehicle-config` uses (wheel positions/radii + collision geometry
  pinned to the imported rig, wheel-count mismatch rejected);
  `--dump-config` writes the imported TOML for editing.
- Clearance found two real trailer defects, both fixed:
  - `vpsemi` tractor/trailer hulls held a standing contact at the
    coincident hitch anchors — the `SphericalJoint` now carries
    `JointCollisionDisabled` (`car_visual::spawn_trailer`).
  - `vpcentury`'s trailer "grounded" only 4/6 wheels: `TWHL0`/`TWHL1`
    are 5 cm detail parts (landing-gear hardware under the wheel
    prefix), not wheels. `build_model` flags trailer wheel parts under
    half the rig's max radius `!simulated`; `load_trailer` filters them
    out of the physics rig (wheel-load split is per real wheel again)
    and their warnings surface through `def.report`; `spawn_vehicle_model`
    parks their visual mounts at the authored origin.
- Tests: `mm2_app::tests/trailer` (joint disables the hitched contact,
  trailer stays attached), two `model.rs` cases (decorative vs real
  trailer wheels), two `apply_handling_override` cases (rig re-pin,
  wheel-count rejection).

## Verification (this tree)

- `cargo fmt --all -- --check` — pass (after `cargo fmt --all`).
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — pass.
- `cargo test --locked --workspace` — pass, all 65 suites green.
- Retail (`fnv1a64:e91e6cd4b2ae30d9`):
  - `--drop`: 21/21 ok — air 0.88–0.92 s, sink 8.5–8.8 m/s, settle
    0.90–1.43 s, `up` 1.00, all drove away, all finite.
  - `--clearance` on sf and london: 21/21 ok both cities — all wheels
    grounded, zero hull contacts at spawn and reset; trailers 4/4 with
    hitch gap 0.00 m.
  - Override causality on vpbug: half `max_power_w` → 0-100 5.4→5.8 s,
    top 59.1→47.0 m/s; half `longitudinal_grip` → 5.4→9.4 s accel and
    `--controls` stop 1.1 s/8.2 m → 1.9 s/14.5 m; 2× mass → 23.6 s
    (suspension bottoms — expected); 6-wheel config on the 4-wheel car
    rejected with an explicit error.
  - `validate-cars`: vpcentury now reports the two decorative `TWHL`
    parts as warnings alongside the known hitch fallbacks.

## Not done / blockers

- Per-car rendered-paint captures (F02-AC03) still not run — needs the
  GPU capture leg on this machine.
- Override causality is demonstrated on vpbug; a per-car override
  matrix is not run.
- `vpmoonrover` launch wander/weak drive leg stays open (recorded).
- Whether the original treats `TWHL0/1`-class parts as wheels is
  unrecovered; the decorative-part exclusion is an implementation
  choice, disclosed in the load warnings.
