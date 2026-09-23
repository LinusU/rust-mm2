# Last iteration — F02-C.4: `_dmg` damage-texture pairing (vpbug defect)

Iteration 56 on `ralph/night`. Selected operator report 2 item 3 — the
long-standing `vpbug` defect: rear windscreen half garbage pixels with a
hard vertical seam, rear-right quarter smeared/crumpled, present since
first vehicle rendering.

## Root cause

Not a parsing or TEX-decode bug. `vpbug.pkg` authors the body in two
halves along the `x=0` centre split:

- `BODY_H` sections 0/1/3 — left half (`x<0`), bound to
  `vpbugyellow_{sd,ft,bk}`.
- `BODY_H` sections 4/5/6 — the **entire right half** (`x>0`), bound to
  `vpbugyellow_{sd,ft,bk}_dmg` — the authored *damage-state* textures
  (cracked glass, dented panels).

We bound every shader texture verbatim, so every clean car drew its
damage skin on the damage-region geometry. The defect is fleet-wide:
22 of 25 parseable retail `vp*.pkg` ship `_dmg`-bound `BODY` sections
(vpbus largest at 12 sections/6 slots; vpcop, vpmoonrover, vpmustang99
carry none). vpbug's was simply the most visible.

MM2Hook's recovered `fxTexelDamage` — a `vehCarModel` member fed by
`vehCarDamage`'s `TextelDamageRadius`/`ImpactsTable` (DMG-5) — documents
the retail rule (`Init`): a shader whose texture name ends in `_dmg`
(`strrchr` last-underscore, case-insensitive) is the damaged variant;
the undamaged car binds the clean stem via `gfxGetTexture`, the `_dmg`
original is stashed per shader slot in `DamageTextures[]` for
per-impact texel blits onto a cloned texture, and a failed clean lookup
keeps the `_dmg` name. The pairing is symmetric — a clean `<stem>`
shader pairs `<stem>_dmg` when it exists — so one `_dmg` file serves
both halves of a symmetric body.

## What changed

- `mm2_app::city::MaterialCache::shader_material` (the single
  vehicle-shader material path — `car_visual::group_material` covers
  player, opponents, traffic and trailers via `spawn_vehicle_model`)
  resolves `clean_texture_stem`: `<stem>_dmg` → `texture/<stem>` when it
  resolves through the VFS (mods included), else the authored name.
  Prop PKGs in `pkg_to_parts` keep verbatim binding — `fxTexelDamage`
  is a `vehCarModel` mechanism, not a prop one.
- Regression test `city::tests::shader_material_binds_the_clean_stem_of_a_dmg_texture`
  covers all three legs: clean stem substitution, orphan `_dmg`
  fallback, and the authored-name missing-texture report.
- `docs/research/pkg.md` records the pairing convention;
  `docs/research/damage.md`'s `TextelDamageRadius` note gains the
  recovered `Init` leg; ledger entry DMG-9.

## Verification (this tree)

- `cargo test -p mm2_app --lib` — new test passes.
- Retail (`fnv1a64:e91e6cd4b2ae30d9`), identical chase-view captures
  (`--city sf --frames 90`):
  - `vpbug` paint 0: `/tmp/vpbug-rear.png` (before — cracked right
    windscreen, dented right quarter, hard seam) vs
    `/tmp/vpbug-fixed.png` (after — uniform clean yellow).
  - `vpbug` paint 1 (blue): `/tmp/vpbug-paint1.png` — clean.
  - `vpbus` paint 0: `/tmp/vpbus-fixed.png` — clean (largest `_dmg`
    binding: 6 slots across BODY_H/M).
  - macOS arm64 / Apple Silicon GPU, Bevy 0.19.

## Not done / blockers

- The impact-time damage blit itself (`ApplyDamage`: `DamageTris`
  barycentric texel lookup + radial blit onto a cloned texture,
  `Reset`'s clean restore) stays unrecovered — F05-B scope. Only the
  clean-state binding is implemented.
- Retail `Init` builds the pairing only over shader slots the
  **high-LOD** body uses; lower-LOD-only `_dmg` slots (vppanozgt
  BODY_M/L) would keep `_dmg` textures in the original. We substitute
  uniformly — we render only the best LOD today, so no observable
  difference; if LOD switching lands, revisit.
- Per-car rendered-paint captures (F02-AC03) and the per-car override
  matrix remain open; this fix's captures cover vpbug ×2 paints + vpbus
  only.
