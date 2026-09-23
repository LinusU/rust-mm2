# Last iteration — repair: F15-B.5 external-review doc findings

Doc-drift repair on `ralph/night` (baseline `df5d6c9`; external review of
the F15-B.5 candidate **failed** on two false statements in the committed
record — both doc-only). No implementation change: the review confirmed
the slice's code, tests and retail reproduction as sound, so the smallest
real defect was the record itself.

## Root cause

Iteration 16's committed record misreported two facts:

1. **Wrong test count.** `PLAN.md`'s F15-B.5 row and `LAST_ITERATION.md`
   claimed `Tests +5` in `crates/mm2_app/tests/bot.rs`; the diff added 11
   `#[test]` fns (7 → 18 — the "18 total" figure was right). Verified:
   `git show f79f48f:crates/mm2_app/tests/bot.rs | grep -c '#\[test\]'`
   → 7, working tree → 18.
2. **Misdescribed bind condition.** `LAST_ITERATION.md` said
   `load_session_world` binds `ScriptedRoute` "when `ScriptedDrive` is
   present" and the PLAN row said "when `--bot` runs". The code at
   `crates/mm2_app/src/session.rs:984` inserts it unconditionally in
   every event session whose roster resolved a route — no `ScriptedDrive`
   check exists; it is dormant without `--bot` because `scripted_drive`
   is resource-gated, exactly as the code comment already stated. Chose
   the doc correction over gating the insert (review offered either; the
   unconditional bind is harmless and the comment was already truthful).

## What changed

- `docs/ralph/PLAN.md` F15-B.5 row: `Tests +5` → `Tests +11`
  (`tests/bot.rs` 7→18); bind description corrected to "every event
  session whose roster resolved a route — dormant without `--bot`".
- `docs/ralph/PLAN.md` next-slice preamble: this repair recorded.
- This file rewritten for the repair iteration.

## Verification

- Facts re-verified against the tree: the `session.rs` insert site has no
  `ScriptedDrive` check; the 7→18 test growth confirmed by the commands
  above.
- Gates (2026-09-24, this tree; doc-only diff, run anyway for a clean
  candidate record): `cargo fmt --all -- --check` clean;
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  clean; `cargo test --workspace` — 67 suites, 0 failures.

## Not done / open

- F15-B.5's technical remainder stands unchanged: the `p3→p4` hillside
  descent on `sf circuit:0` is not drivable — the car slides ~15 m off
  the line's crown into a parallel ditch and grinds at ~1–2 m/s through
  disclosed re-anchors; the AI field shares the wall (`opp=0/4
  opp_rec=5`). Next blocker is steep-slope/collider fidelity
  (F14-B/F15-B physics or city-collider task), not routing.
- Review parity note (non-blocking): scripted-player re-anchors are
  disclosed via `info!` + `ScriptedRoute::reanchors` only — the smoke
  record has no field like opponent `opp_rec=`. A `p_rec=`-style field
  would be a small ready follow-up.
- Iteration-16 dev-trace claims (per-gate `offd<5`, landing `offd≈1`,
  the ditch-grind description) remain post-hoc unverifiable — they came
  from per-tick instrumentation; the end-of-run record and the 6
  disclosed re-anchor log lines are consistent but do not prove the
  intermediate values.
