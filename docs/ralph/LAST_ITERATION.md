# Last iteration — authored engine-smoke visual tier

Iteration 34 on `ralph/night`, continuing from `0831d73` (the
externally checked F05-B.5 water/OOB candidate — review verdict
pass). Task id: `F05-B.6` — the smoke leg of F05-B's remaining
"visual tiers" work (F05 spec req 4 / F05-AC02): authored emission
pivots and the embedded `vehCarDamage` particle spec rendered as
engine smoke while the vehicle is damaged.

## Slice choice

Of the F05-B remainder — visual tiers (smoke, `TextelDamageRadius`,
pivot gates, sparks), damage-driven detachment-if-original,
impairment, C&R healing, replication — the smoke tier is the
highest-value ready slice: mm2hook's recovered `vehCarDamage`
struct names the embedded spec `EngineSmokeRule` and carries the
exact gate fields the authored records ship (`SmokeOffset`/
`SmokeOffset2`, `DoublePivot`, `MirrorPivot`, `m_CurrentPivot`), so
a large authored surface can be consumed faithfully while the
genuinely-unrecovered `Update()` cadence stays a disclosed designed
policy (DSN-24). Texel damage (`ImpactsTable`/`fxTexelDamage`) and
`asLineSparks` need the contact-point feed that doesn't exist yet —
deferred. Impairment is next-slice material: MM2Hook's `mm2.ini`
`PhysicalEngineDamage` option documents the original coupling
("damage affects engine torque … when the engine spews smoke" ⇒
less acceleration/top speed), now recorded as a lead in
`docs/research/damage.md`; its shape is still unrecovered.

## What changed

- `mm2_game::effects` (new contract module):
  - `ParticleSpec` — every `DamageEffect` field verbatim so
    consumers bind authored values. Consumed this slice:
    `PositionVar`, `Velocity`/`VelocityVar`, `Life`/`LifeVar`,
    `Radius`/`RadiusVar`, `Drag`/`DragVar`, `DRadius`/`DRadiusVar`,
    `DAlpha`/`DAlphaVar`, `Gravity`, `TexFrameStart`/`TexFrameEnd`,
    `Color` (packed ARGB — retail decodes as `0xF6000000`-class
    near-opaque black). Carried unconsumed and documented:
    `Position`, `Mass`, `Damp`, `DRotation`, `InitialBlast`,
    `SpewRate`/`SpewTimeLimit` (0 on all retail damage records — the
    designed policy owns cadence), `BirthFlags`, `Height`,
    `Intensity`.
  - `SmokePolicy` — designed (DSN-24): emission is the damaged-tier
    signal, 0 below `MedDamage` ramping 4 → 24 puffs/s at
    `MaxDamage`; `max_live` 48/vehicle; `atlas_tiles` 2.
  - `VehicleSmoke` component — authored pivot gate: `SmokeOffset`
    always; `MirrorPivot != 0` derives a second pivot mirrored about
    x (designed reading; all 7 retail values are 0); else non-zero
    `SmokeOffset2` is the second pivot. `DoublePivot != 0` emits
    every pivot per burst (vpddbus/vppanoz/vppanozgt — all carry
    non-zero second pivots, measured); single-pivot rigs alternate
    via the `m_CurrentPivot` cursor. Fractional burst accumulator +
    per-emitter `NavRng` seeded by the vehicle's object id —
    emission replays identically (replicable by construction).
  - `SmokePuff` component — the entity is the particle; `advance`
    integrates designed field readings (gravity as signed +Y rise,
    exponential `Drag`, `DRadius` growth, byte-space `DAlpha` fade
    of the `Color` alpha byte) and expires on authored `Life`.
- `mm2_app::damage_fx`: `smoke_assets` resolves `texture/fxpt2`
  through `city::load_image` (VFS + mod overrides like every
  texture) — a measured 2×2 puff-tile atlas; `TexFrame` indexes its
  tiles like mm2hook's `asSparkPos::TexCoordOffset` (designed
  binding — the original's texture choice is unrecovered). One
  UV-baked quad per tile + an unlit blended material cloned per puff
  for independent alpha. `drive_smoke` emits per rigged participant
  (`Remote` skipped — its authority renders its own; `is_playing`
  gated), `advance_smoke` integrates, billboards to the active
  camera, writes per-puff alpha and despawns the expired. `SmokeFx`
  is session-scoped (inserted on load, removed on teardown);
  `SmokeFxReport` feeds the `ptx=<e>e/<x>x` smoke field on activity
  only — undamaged runs stay bit-identical.
- Spawn: `VehicleSmoke` attaches to the player vehicle and every AI
  opponent behind the same `def.damage` authored-presence gate as
  `VehicleDamage` — authored absence stays smoke-free, never
  fabricated.

## Tests

- `mm2_game/tests/effects.rs` (11): spec carries authored fields
  verbatim; pivot gate legs (two pivots / zero `SmokeOffset2` /
  `MirrorPivot` wins / `DoublePivot` flag); rate gate + ramp +
  degenerate spec; fractional accumulation + pivot alternation;
  double-pivot emits all; live-bound truncation; deterministic
  seeded puff draws inside authored ±var; atlas-frame clamp;
  integrator readings; alpha byte fade; garbage-dt.
- `mm2_app/tests/damage_fx.rs` (10): damaged car emits at authored
  pivots in car space; intact car emits nothing; repair stops
  emission and puffs expire; authored-life expiry + live pool
  bound; remote participant emits nothing; pause freezes
  emission/integration; missing `SmokeFx` no-ops; session stamp +
  bounded frame; billboard follows the active camera; transform
  and material alpha track the puff.

## Evidence

- `cargo test -p mm2_game --test effects`: 11/11 pass.
- `cargo test -p mm2_app --test damage_fx`: 10/10 pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: clean.
- `cargo fmt --all -- --check` + `cargo test --workspace`: clean —
  61 suites, 0 failures.
- Retail headless smoke on the supplied install
  (`fnv1a64:e91e6cd4b2ae30d9`):
  - Inactive leg (SF scripted cruise, vpbug): `status=pass
    updates=600 ticks=1200 driver=scripted … dmg=3a/0d/0r` — no
    `ptx=` field, run stays bit-identical to the pre-change
    record (damage stays far under vpbug's 150k `MedDamage`).
  - Active leg (SF, vpcoop `--spawn=-1316,60,381,0`, 6000
    frames): `status=pass … impacts=162 dmg=21a/1d/1r brk=6d/6r
    ptx=437e/437x` — the tumble run crossed the Mini's authored
    80k `MedDamage` (once reaching `Disabled`), emitted 437
    puffs through the authored `SmokeOffset`/`SmokeOffset2`
    rig and expired all 437 inside their authored lifetimes.
  - Intermediate runs (vpbug/vpcaddie/vpcoop pen-battering and
    mid-air spawns at 1200–3000 frames) accumulated up to 11
    applied impacts without crossing `MedDamage` — the
    `severity × other_mass` impulse model needs heavy-target
    or high-speed hits, so shallow prop hits alone cannot
    reach the tier. Correctly reported as damage without a
    `ptx=` field.

## Classification / open items

- F05-B.6 is `implemented` (candidate) — pending external gates +
  review.
- Classifications: pivots/spec/texture-tile vocabulary authored;
  emission gate, cadence, mirror reading, per-field integrator
  readings, `fxpt2` binding and pool bound **designed** (DSN-24);
  the original `vehCarDamage::Update()` is a binary thunk — its
  cadence, pivot-switch timing and texture choice stay UNK-13.
- Honest gaps: no rendered/GPU screenshot of the smoke (headless
  proof only — entities/materials are real, nothing rasterizes);
  impairment is documented-only (the `PhysicalEngineDamage` lead),
  not implemented; remote-participant policy is compile-time only —
  no networking exists.
- Still open in F05-B: `TextelDamageRadius`/`ImpactsTable` texel
  damage + `asLineSparks` sparks, damage-driven detachment if
  original, impairment (smoke↔torque coupling lead), C&R healing,
  replication.
