# Last iteration — F05-B.9: impact texel damage

Iteration 57 on `ralph/night`. Selected the F05-B remainder's texel
damage leg — the direct follow-on of F02-C.4's `_dmg`↔clean pairing,
which bound the clean skin but left `ApplyDamage`/`Reset`
unimplemented. mm2hook recovers more of `fxTexelDamage` than the docs
recorded (`src/modules/effects/texeldamage.{h,cpp}` +
`vehicle/carmodel.{h,cpp}`), so the triangle/radius/barycentric
mechanics are recovered; only the splat itself
(`ApplyBirdPoopDamage`, a binary call) stays unrecovered → designed
(DSN-32).

## What changed

- `mm2_game::texel` (new): `TexelDamageMesh`/`TexelDamageTri`/
  `TexelSplat` — `splats(point, radius, rng)` is the recovered
  `ApplyDamage` loop: every tri with a vertex within
  `TextelDamageRadius` of the car-space impact point earns one splat
  at a random barycentric UV (three `frand()` draws normalized by
  their sum; degenerate sums skipped). `TexelDamagePolicy` +
  `splat_blit` is the designed splat — a 48 px probability-dithered
  disc (`1 − d/r` + 0.1 edge floor, following mm2hook's debug
  reimplementation) copying `_dmg` texels onto the per-vehicle
  `current` clone, propagated down the shared mip chain and clipped
  to bounds.
- `mm2_app::texel_fx` (new): `TexelSlot` mirrors the recovered
  `DamageTextures[]`/`CurrentShaders` triple (`clean`/`damage`/
  `current` + cloned material); `TexelDamageRig` component carries
  the authored radius, paired slots, the body-only car-space tri soup
  and a per-vehicle `NavRng` seeded by object id (same domain as
  smoke/sparks — recorded impacts replay identically, F05 req 6).
  `apply_texel_damage` runs in `FixedLast` after
  `apply_impact_damage`, reads the same deduplicated `ImpactEvent`
  stream, same `is_playing`/authority/`Remote` gates, world point →
  car space through the vehicle transform.
- `MaterialCache::texel_binding` (city.rs): pairs a clean shader stem
  with `<stem>_dmg` when it resolves, keeps an authored `_dmg` leg as
  its own damage texture, and on an orphan `_dmg` (no clean stem)
  reuses the bound texture for both sides — `gfxGetTexture` caches by
  name in retail, so the same handle serves both rather than
  double-loading. Clones `clean` into the writable `current`,
  clones the material, binds `current`; non-4bpp formats warn and
  stay unpaired.
- `car_visual::spawn_vehicle_model` gains a
  `texel_damage: Option<(&VehCarDamage, u64)>` leg: `PartRole::Body`
  groups route through `TexelDamageBuilder::bind_group` (paired slot
  → per-vehicle material + recorded tris; unpaired → shared
  material), and `finish` inserts the rig only when at least one slot
  paired — matching the authored-presence policy of
  `VehicleDamage`/`VehicleSmoke`/`VehicleSparks`. Player and AI
  opponents pass `Some`; ambient traffic and trailers pass `None`.
- `damage::resolve_disabled` gained a `TexelRepair` SystemParam
  (bundled to stay under the fn-system param limit); its three
  `damage.reset()` repair sites — Cruise free reset, Circuit penalty
  reset, AI reset — call `texel.reset(entity)`, re-blitting clean
  over the clone. Plain reset/stuck/recovery is not a repair and
  leaves the skin stamped (same rule as breakaway parts).
- `TexelDamageReport` resource → `txl=<impacts>i/<splats>s/<resets>r`
  in the headless record, emitted only on activity so impact-free
  runs stay bit-identical; reset during `drive_session` teardown.
  Registered in both the windowed app and the headless smoke app.

## Verification (this tree)

- `cargo fmt --all -- --check`, `cargo clippy --workspace
  --all-targets --all-features -- -D warnings`, `cargo test
  --workspace` — all pass (the new `TexelDamageReport` param on
  `resolve_disabled`/`drive_session` required adding the resource to
  17 existing test harnesses).
- Tests added: 8 `mm2_game` (radius filter, barycentric UV,
  determinism, degenerate inputs, blit disc/floor/clipping/mips),
  3 `mm2_app` (body-only rig build + per-vehicle texture/material
  independence, impact stamps cloned pixels, `resolve_disabled`
  repair restores clean pixels + counts), 1 city unit (clean/damage
  pairing, orphan `_dmg` handle reuse, per-vehicle clones).
- Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`):
  - sf cruise vpbug `--frames 1800`:
    `impacts=42 dmg=10a/0d/0r spk=15b/105e/105x txl=15i/2543s/0r`
  - sf `circuit:0 --bot --frames 12000`:
    `dmg=237a/4d/3r … txl=401i/64364s/3r` — the reset counter tracks
    the three repairs exactly; every player impact (237 applied + 164
    damage-threshold-rejected) stamped, consistent with the retail
    `ImpactCB` feeding the table regardless of damage gating.
- Rendered (local captures, not committed): vpbug
  `--spawn=-1319.5,80,214` drops ~11 m nose-first —
  `/tmp/txl-front.png` (front view) shows stamped cracks/dents on
  hood, windshield and lamps while `/tmp/txl-dbg2.png` (same run,
  rear view) stays clean — localized stamping, not a texture swap.
  A light settle (`--spawn=-1319.5,69,214`,
  `/tmp/txl-dbg-clean.png`) stamps the rear.

## Corrections

- The prior iteration's "22 of 25 parseable `vp*.pkg`" denominator is
  corrected to 22 of 27 base files (excluding `_dash`/`_trailer`), and
  vpdb731 + vpvw_dune join the no-`_dmg` exception list
  (docs/research/pkg.md, DMG-9).

## Not done / open

- `ApplyBirdPoopDamage`'s original splat shape stays unrecovered —
  the radial blit is designed (DSN-32, UNK-13). The `ImpactsTable`'s
  fill rules (which participant feed it reads, queue depth) are
  likewise unrecovered; the implementation feeds the deduplicated
  `ImpactEvent` stream directly.
- A rendered post-repair capture was not staged (repair is proven by
  the `txl` reset counter tracking `dmg` repairs 1:1 plus the
  pixel-restore test).
- LOD note carried forward from F02-C.4: retail `Init` pairs only the
  high-LOD body's shader slots; only the best LOD renders today, so
  the uniform substitution is unobservable — revisit if LOD switching
  lands.
