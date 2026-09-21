# Last implementation iteration

- Task ID and title: F06-B.2 — the remaining authored-physicals leg:
  `drag` into the real tire path, `elasticity` into collider contact
  restitution, plus F06-AC03's runtime texture-swap invariance test.
- Starting commit: `7bfbba8d17ab4ffcb412d98a24ec24e0e4209dce`
  (externally checked F06-B traction leg; branch `ralph/night`).
- Why this slice: the plan's selection policy listed the F06-B
  remainder (`elasticity`/`drag` consumers, consumer consistency) as
  a next candidate. These are the last authored scalar fields with a
  feasible runtime consumer — `sound`/`effect`/`width`/`height`/
  `depth`/`ptx*` need F07 audio/particle systems and stay open, as do
  AC05's audio/dust consistency and AC06's network authority (no such
  consumers exist to be consistent with).
- Retail install: `/Users/linus/coding/rust-mm2/retail`
  (`fnv1a64:e91e6cd4b2ae30d9`).

## What changed

- **`mm2_vehicle::surface::TireSurface`** gains `drag: f32` — the
  surface's wading-resistance coefficient (retail: only `water` 0.119
  and `deepwater` 0.5 carry it). Neutral is `0.0`.
- **`mm2_vehicle::vehicle_simulation`:** each grounded wheel resolves
  `drag` off the same `TireSurface` its grip comes from (unmarked
  collider → 0) and applies `-v_plane × drag × load` at the contact
  point — a viscous resistance opposing motion in the contact plane.
  Kept **outside** the friction ellipse: it is fluid resistance on
  the wheel, not a tire force. `WheelState.surface_drag` reports the
  coefficient per wheel (0 airborne/unmarked), parallel to
  `surface_grip`.
- **`mm2_content::surface`:** `tire_surface` fills `drag` from the
  def's `drag` **raw** (`_default` authors 0.0 — no divisor exists);
  missing/negative/non-finite → 0.0. New `contact_restitution(i)`
  maps the def's `elasticity` into `0..MAX_SURFACE_RESTITUTION` (0.1 —
  the same conservative cap `convert` applies to `BoundElasticity`:
  MM2's elasticity drove its own impact solver, so it is scaled, not
  applied verbatim); `restitution_for` maps `Authored(i)` →
  `Some`, `Unspecified` → `None` (unmarked colliders keep Avian's
  default). *Implementation choices under UNK-23, not recovered
  original formulas.*
- **`mm2_app::city::load_city`:** the collider spawn loop now inserts
  `Restitution::new(...)` beside `TireSurface` on every
  named-material collider. Collider `Friction` deliberately stays at
  Avian's default — authored `friction` is a tire-grip coefficient
  and applying it to chassis/prop contact would fight the documented
  `MAX_COLLIDER_FRICTION` scrape policy.

## Tests

- `mm2_content/tests/surface.rs` — +2: raw `drag` pass-through with
  negative/out-of-range neutrality; `contact_restitution` scaling
  (`0.9→0.09`, `0.19→0.019`, `0.5→0.05`), invalid/out-of-range → 0.0,
  `Unspecified` → `None`.
- `mm2_vehicle/tests/surface.rs` — +2 on real Avian physics: per-wheel
  `surface_drag` over a marked/unmarked seam; `a_wading_surface_…` —
  a `deepwater`-strength slab halves a launch vs dry, and stamping the
  component mid-drive decelerates a coasting car below half its entry
  speed with `grip` untouched (viscous resistance, not a traction
  change).
- `mm2_app/tests/surface.rs` — +2 through `load_city`: colliders carry
  scaled `Restitution` of their material (unmapped region carries
  none); F06-AC03 — a higher-priority VFS mount replacing the texture
  *file* leaves `SurfaceMaterial`/`TireSurface`/`Restitution`
  identical at every probed point (classification walks the PSDL
  texture *name*, never the image).

## Commands actually run and results

- `cargo fmt --all -- --check` PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` PASS.
- `cargo test --locked --workspace` — all groups, 0 failures.
- `mm2 --mm2-path <retail> --city london --headless` — `status=pass`,
  `peak=29.0m/s moved=84m`: land baseline unchanged.
- `mm2 --mm2-path <retail> --city sf --headless` — `status=pass`,
  `peak=37.2m/s moved=171m`: unchanged.
- `mm2 --mm2-path <retail> --city london --headless --traction 0.5` —
  `traction=0.5`, `peak=22.3m/s`: env override still composes.
- `mm2 --mm2-path <retail> --city london --spawn=-80,2,805,0
  --headless` — `status=pass`, final y=−4.0 (on the Thames
  `deepwater` colliders, rooms ~337–364 of `london.psdl`, measured via
  `emit_psdl` + `load_surface_tables` probe): full throttle reaches
  only `peak=1.1m/s moved=11m` in 10 s — authored `drag` bogs the car
  down on real content.
- Banger re-takes (the added restitution is a real new input on
  named-material streets — deltas recorded in
  `docs/research/banger.md`, not regressions): sf `vpddbus
  --spawn=-141.9,1.5,-608.5,115 --frames 5000` → `bng_ev=0a/2s/1b`
  (was `0a/5s/2b` — the site sits on cobblestone/grass colliders);
  london `vpbug --spawn=802,6,-905,180 --frames 1500` →
  `bng_ev=3a/3s/0b` (same 3 activations, bollards now settle — was
  `3a/0s`).

## What this proves / does not prove

- Proves: every authored scalar physical field (`friction`,
  `elasticity`, `drag`) now reaches a real runtime consumer — tire
  grip, collider restitution, and per-wheel wading resistance —
  through the same texture→csv→mtl classification; water measurably
  slogs a car on real retail content while dry surfaces are
  untouched; a cosmetic texture-file override cannot move physics
  identity (F06-AC03's test leg).
- Does not prove: the original's combination formulas — the
  `_default`-divisor, the viscous `drag × load` model and the ×0.1
  restitution cap are all implementation choices (UNK-23); `sound`,
  `effect`, `width`/`height`/`depth`, `ptx*` still have no consumer;
  no audio/dust consumer exists to check AC05 consistency against
  (F07 scope); no networked session for AC06 (F24 scope); whether
  wheels/bodies striking water should also get buoyancy/submersion
  (`depth` field) is unmodelled.
- Acceptance IDs: F06-AC03 advanced (runtime swap-invariance test now
  exists); AC02 strengthened (a third measurable surface difference —
  drag — on real content); AC04 unchanged; AC05 partially (tire path
  consistent; audio/dust consumers absent); AC06 open. F06 stays
  `active`.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
