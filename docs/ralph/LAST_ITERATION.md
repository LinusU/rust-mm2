# Last iteration — F04-C.5 review repair: impulse/energy doc drift

Iteration 51 on `ralph/night`. External review of the F04-C.5 candidate
(`80a6147`) returned one blocking finding: doc drift. The commit changed
what banger activation compares `ImpulseLimit2` against (linear `v·m`
estimate → striker kinetic energy `½·m·v²`), but four cross-references
still asserted the knock/damage linear estimate is the *same* quantity
banger activation uses:

1. `crates/mm2_game/src/traffic.rs` — `KnockPolicy` rustdoc.
2. `crates/mm2_game/src/damage.rs` — `DamageState::apply` rustdoc.
3. `docs/research/damage.md` — the shared-contract paragraph.
4. `docs/original-rules.md` — the UNK-12 row's F10-B.6 runtime note.

## Root cause

Narrow scope during F04-C.5: the implementation commit updated
`contracts.rs`, the `mm2_app` module docs, `docs/research/banger.md` and
the DSN-10/UNK-22 ledger rows, but missed the four places where
*other* features (F10-B.6 knock handover, F05-B.1 damage) describe
their own estimate by reference to banger activation. Verified by
reading every `activates_on`/`impulse_estimate`/`impact_energy` call
site and grepping for residual "shared with banger" claims — the list
above was complete; `mm2_app::traffic`'s module docs already described
the split correctly.

## What changed (docs/rustdoc only — no code)

All four now state the post-F04-C.5 split: the knock handover
(`KnockPolicy::min_impulse`) and damage accumulation
(`severity × striker_mass`) keep the linear estimate; banger activation
gates on `½·m·v²`; what remains shared is the deepest-contact severity
and the `striker_mass` resolution. UNK-22 stays open — the original's
comparison quantity is still unrecovered.

## Verification (this tree)

- `cargo fmt --all -- --check` — pass.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — pass.
- `cargo test --locked --workspace` — pass (doc-only diff; suite
  counts unchanged from the F04-C.5 run, including `tests/banger.rs`
  21/21).

## Not done / blockers

- Carried from F04-C.5: a named `sp_tree1_s` shatter on retail was not
  staged (~12 attempts; straight-line driver cannot reach 22–31 m/s
  before the first trunk) — covered synthetically in both directions.
- UNK-22 stays open: the energy reading is a designed stand-in
  justified by the authored-limit census, not verified original
  behavior.
- Operator report 4 remaining item: 5 (startup warn aggregation).
