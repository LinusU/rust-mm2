# Last iteration — repair of the F05-B.3 review blockers

Iteration 31 on `ralph/night`, continuing from `f47dfb1` (F05-B.3
candidate). Task id: `F05-B.3` (review repair — no new feature slice).

## Slice choice

Iteration 30's F05-B.3 candidate failed external review on two
documentation/evidence defects — both in committed text, none in the
implementation:

1. `docs/original-rules.md` DSN-21 recorded the implemented detach
   rule as `severity × other_mass` (the damage accumulator's striker-
   mass estimate), and the UNK-13 row repeated it. The code — and
   `docs/research/damage.md` — implements `severity × part_mass`
   (the part's own record `Mass`; `VehicleBreaks::detachable` calls
   `activates_on(approach_speed * spec.def.mass)`). `other_mass` was
   the rejected mid-iteration draft — under it every panel would shed
   on a ~2 mph touch. The ledger entry was never updated.
2. This file claimed "the fragment visual spawn path is exercised in
   the synthetic suite (mesh children re-spawned under the fragment
   body)". No test attached `Mesh3d`/`MeshMaterial3d` children under
   a `BreakPartVisual` node, so the `render_parts` re-spawn loop in
   `detach_breaks` never executed in tests.

## What changed

- `docs/original-rules.md`: DSN-21 now records `severity ×
  part_mass` — the part's own record `Mass`, explicitly noted as a
  different quantity from the `severity × other_mass` estimate
  `apply_impact_damage` weighs — and that `limit / mass` reads as the
  authored detach speed (≈31.25 / ≈2500 m/s). The UNK-13
  parenthetical repeats the corrected quantity.
- `crates/mm2_app/tests/breakaway.rs`: new test
  `mesh_children_respawn_under_the_fragment` attaches a real
  `Mesh3d`/`MeshMaterial3d` child under the `BreakPartVisual` node,
  detaches the part, and asserts the fragment body carries a fresh
  child entity with the cloned mesh/material handles while the
  hidden node keeps its original — the previously uncovered
  `render_parts` re-spawn loop now runs in the suite, so the claim
  stands as written rather than being corrected away.
- No implementation changes; the reviewed code was correct.

## Evidence

- `cargo test -p mm2_app --test breakaway`: 11/11 — the new
  mesh-children leg plus the previous 10.
- `cargo fmt --all -- --check`: pass. `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings`: pass.
  `cargo test --locked --workspace`: 751 tests, 0 failures.
- Ledger consistency re-checked: `docs/research/damage.md` already
  stated `severity × part_mass`; DSN-21/UNK-13 now agree. No other
  doc repeats the `other_mass` wording for detachment (the damage
  accumulator's own `severity × other_mass` impulse is a different,
  correctly documented quantity).

## Classification / open items

- F05-B.3's state remains `implemented` (candidate): the
  implementation was externally gate-clean and the review's blocking
  findings were evidence defects, now repaired. Re-check pending.
- Still unverified as before: retail detachment observation
  end-to-end (scripted driver never reaches ~31.25 m/s approach
  speed), rendered/GPU proof of a detachment, original threshold
  semantics (UNK-22/UNK-13).
- Still open in F05-B: visual tiers (smoke pivots,
  `TextelDamageRadius`, `DoublePivot`/`MirrorPivot`), damage-driven
  detachment if original, impairment, `vehgyro`, water/OOB
  recovery, C&R healing, replication.
