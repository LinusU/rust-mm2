# Last iteration — F02-C.5: per-car render leg + override matrix + AC06 re-run

Task slice on `ralph/night` (baseline `8bae423`, the reviewed
F15-B.11 commit). Selected the F02-C remainder — the two named open
legs in `docs/vehicle-coverage.md` (per-car rendered-output evidence
for F02-AC03, per-car override causality for F02-AC04) plus the AC06
startup-path re-run — the highest-priority ready slice, with GPU
available on this machine and all instruments already in-tree.

## What changed

`crates/mm2_app/src/smoke.rs` + `crates/mm2_app/src/main.rs`:

- **Stale-screenshot false pass fixed.** `smoke_test`'s capture-wait
  treated any non-empty file at the `--screenshot` target as this
  run's write — a reused path reported `status=pass` on the previous
  image while the fresh save was still in flight (observed live while
  staging the render leg: `pass` with stale byte count, then
  "Failed to send screenshot: sending on a closed channel" during
  teardown). New `smoke::clear_stale_screenshot` removes the target
  before the `Screenshot` request — `NotFound` is the expected case,
  any other error fails the run rather than risking a stale pass.
  Regression test `a_stale_capture_is_cleared_before_the_new_request`
  (existing file cleared; fresh target ok; uncleanable target errors).

## Verification

Evidence on the fingerprinted retail install
(`fnv1a64:e91e6cd4b2ae30d9`, read-only), Apple M1 / Metal, this
commit + the patch above, 2026-09-24:

- **Render leg (F02-AC03):** all 21 ready cars captured through the
  production app — one pinned waterfront spawn, one fixed free cam:
  `mm2 --mm2-path <install> --city sf --car <id>
  --spawn=-141.9,1.5,-608.5,115 --cam=-144.9,7.0,-588.5,-8.5,-15
  --frames 120 --screenshot screenshots/f02c-render/<id>.png`.
  **21/21 `smoke=visual status=pass`**, every PNG inspected — the
  named car renders recognisably at default paint on real SF
  geometry; `vpcentury`/`vpsemi` draw hitched trailers; `vpmoonrover`
  shows its authored nose-up stance; `vpcoop`/`vpcoop2k` and
  `vpbug`/`vpvwcup` are distinct models, not re-textures. Captures
  local under `screenshots/f02c-render/` (gitignored — original
  content stays out of the public tree).
- **Override matrix (F02-AC04):** per car, the dumped `VehicleConfig`
  TOML edited twice through the same `apply_handling_override` the
  app's `--vehicle-config` uses — `peak_torque_nm`+`max_power_w`
  halved → accel probe; every `longitudinal_grip` halved →
  `--controls` brake leg. **Every car's measurement moves on both
  legs.** Saturated cells recorded, not argued away:
  `vpbullet`/`vppanoz` launches are traction-limited (0-100 parity —
  `--trace` diverges mid-launch: 47.8→38.9 m/s at t=8, 72.5→64.6 at
  t=14); seven cars' top speeds are rev-limited not power-limited
  (<1% delta); `vpbus` no longer reaches 100 km/h under the halved
  engine (−24% top); `vpmoonrover`'s top collapses 12.9→1.6 m/s (its
  stop leg is unsuitable — brakes from ~1 m/s); `vpcentury` stops
  *shorter* under halved grip (24.2→13.7 m; 0.75× lands at 11.2 m —
  non-monotonic wheel-lock interplay, mechanism unverified, flagged
  for the handling owner — no retuning done).
- **Baseline drift disclosed + corrected:** the dynamic table's
  `vpcentury`/`vpmoonrover` stop cells predated F02-C.3's 6→4 wheel
  rig — re-measured at 2.3 s/24.2 m and 0.1 s/0.1 m; all other cells
  unchanged.
- **AC06 re-run:** `sf` + `london` headless `status=pass` (120
  updates each); unknown `--car` exits 2 with `unknown vehicle id
  "nonexistent" (see --list-cars)`; the shipped `checker-override`
  mod mounts at VFS priority 300 and renders — a dev-world capture
  shows its magenta checkerboard road.
- Gates: `cargo fmt --all -- --check` clean; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` clean;
  `cargo test --locked --workspace` green (incl. the new smoke test).

## Not done / open

- F02-C residual disclosures (in `docs/vehicle-coverage.md`):
  left/right + teleport verbs measured only at the shared synthetic
  level; non-default paints not rendered (21 default-paint captures,
  not all ~90 variants); `vpcentury`'s non-monotonic brake leg needs
  a mechanism answer from the handling owner.
- Render evidence is one fixed spawn/camera on SF — no London render
  matrix, no night/weather variants, no moving-car capture.
- Override numbers are this engine's instruments, not original-game
  comparisons — no original-binary reference exists on this machine.
