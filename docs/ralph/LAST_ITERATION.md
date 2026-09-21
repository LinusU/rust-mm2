# Last implementation iteration

- Task ID and title: F06-B — the traction leg: authored `friction`
  plus a separate environment modifier into the real tire force path.
- Starting commit: `d292898494a79a2bc2327991c0741db7a1d91c66`
  (externally checked F06-A.2 handoff; branch `ralph/night`).
- Why this slice: F06-A.2 landed classified colliders but nothing
  consumed them physically — the spec's R1–R4 traction leg is the
  smallest real consumer: material grip + a separate environment term
  applied once to delivered tire force (AC02), with the same
  classification feeding wheel telemetry and impact events (AC05's
  tire/impact leg). `elasticity`/`drag`, AC03's texture-swap test,
  audio/dust consumers and AC06's network authority stay open.
- Retail install: `/Users/linus/coding/rust-mm2/retail`
  (`fnv1a64:e91e6cd4b2ae30d9`).

## What changed

- **`mm2_vehicle::surface` (new module):** the physics-side inputs —
  `mm2_vehicle` cannot see `mm2_game::SurfaceMaterial` (dependency
  direction), so it declares what it consumes and the collider
  spawner translates.
  - `TireSurface` (`Component`) — normalized material grip on a
    collider; `1.0` = the reference surface the handling was tuned
    against. A collider without one is the neutral reference.
  - `TireConditions` (`Resource`) — the session environment traction
    term (wetness/ice-style), deliberately separate from the base
    material (spec R2). Default `1.0`; `VehiclePlugin` init_resource
    supplies it pre-session.
- **`mm2_vehicle` force path:** `vehicle_simulation` resolves
  `WheelState.contact_entity → TireSurface` × `TireConditions.traction`
  into one `surface_grip` (clamped finite ≥0) and applies it to the
  lateral force, the longitudinal limit, the traction-control cap and
  the friction ellipse — once, not per consumer (spec R4/R5).
  `WheelState.surface_grip` records the effective term per wheel;
  airborne wheels report the neutral `1.0`.
- **`mm2_content::surface`:** `SurfaceTables::tire_surface(i)` —
  `defs[i].friction` normalized against the `_default` block's
  `friction` (retail `_default` = 0.90 → cobblestone/grass/sand 1.0,
  water ≈0.76, deepwater ≈0.72, wood ≈1.06). Missing/negative/
  non-finite values and out-of-range indices → neutral `1.0`; no
  usable `_default` → authored values raw. `tire_surface_for` maps
  `Authored(i)` → component, `Unspecified` → `None` (same conservative
  policy as the identity layer). *Implementation choice, not a
  verified original scaling — UNK-23.*
- **`mm2_app`:** `emit_psdl`/`load_city` attach `TireSurface` beside
  `SurfaceMaterial` on every collider. `publish_vehicle_telemetry` and
  `collect_impacts` report `TireConditions.traction` in
  `SurfaceState.traction` — wheels and impacts share one modifier.
  `session.rs` stamps `TireConditions` from the session config on
  every load; it is deliberately *not* removed on teardown — it is a
  system input (`Res` every frame) re-stamped per load, so removing
  it only opens a missing-resource panic window (found by the banger
  restart test).
- **`--traction <f>` CLI dev override** (quarantined like `--spawn`/
  `--banger-pool`, `DevOverrides.traction`): finite and ≥0 or usage
  error exit 2; recorded as `traction=<f>` in smoke records when set.
  Not claimed as the original weather system — F18 owns a
  session-legal writer.

## Tests

- `mm2_content/tests/surface.rs` — `tire_surface` normalization:
  `_default` divisor, non-1.0 divisors, missing `_default` raw
  fallback, missing/negative friction neutrality, out-of-range index,
  `Unspecified` → `None`.
- `mm2_vehicle/tests/surface.rs` (new, real Avian): each wheel over a
  two-material seam reports its own collider's grip; the environment
  term multiplies material grip once; it also scales unmarked
  surfaces; an airborne wheel reports neutral; a slippery surface and
  a wet environment each measurably limit delivered drive force.
- `mm2_app/tests/surface.rs` — colliders spawned through `load_city`
  carry the normalized `TireSurface` of their authored material;
  unmapped-name regions carry none.
- `mm2_app/tests/contracts.rs` — per-wheel authored material over
  split ground colliders (AC01's wheel leg), and one
  `TireConditions.traction` reported identically by wheel telemetry
  and impact events (AC05's tire/impact leg).

## Commands actually run and results

- `cargo fmt --all -- --check` PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` PASS.
- `cargo test --locked --workspace` — all groups, 0 failures.
- `mm2 --mm2-path <retail> --city london --headless` — `status=pass`
  (updates=600, peak 29.0 m/s, moved 84 m): baseline unchanged on
  real content.
- `mm2 --mm2-path <retail> --city sf --headless` — `status=pass`
  (peak 37.2 m/s, moved 171 m).
- `mm2 --mm2-path <retail> --city london --headless --traction 0.5` —
  `status=pass`, `traction=0.5` recorded, peak 29.0→22.3 m/s: the
  environment term measurably limits delivered force on real content.
- `--traction nan` / `--traction=-1` → usage error, exit 2.

## What this proves / does not prove

- Proves: authored material friction reaches the real Avian tire
  force path as a per-wheel grip multiplier; a distinct environment
  term composes with it exactly once and measurably changes delivered
  force (AC02's physics leg); wheels report the material under them
  (AC01's wheel leg); telemetry and impacts report one shared
  environment term (AC05's tire/impact leg); physical identity stays
  separate from cosmetic texture grouping (AC03's structural half).
- Does not prove: the original's combination of
  `friction`/`elasticity`/`drag` with tire parameters — the
  `_default`-divisor scaling is an implementation choice (UNK-23);
  `elasticity` and `drag` still have no runtime consumer; no runtime
  texture-swap invariance test (AC03's test leg); no audio/dust
  consumer exists to check consistency against (AC05 remainder); no
  networked session, so authority is unexercised (AC06); the
  `--traction` override is a dev diagnostic, not verified weather
  behavior (F18).
- Acceptance IDs: F06-AC01 advanced (wheel leg now tested), AC02
  advanced (physics leg — synthetic, measurable), AC03 partially
  (structural separation, no swap test), AC05 partially (tire/impact
  consistent), AC06 open. F06 stays `active`.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
