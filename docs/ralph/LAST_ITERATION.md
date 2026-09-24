# Last iteration — F13-C.6: deep-course extended-budget probe + C.5 review-finding repairs

Task slice on `ralph/night` (baseline `b96f5c1`, the externally
checked F13-C.5 commit). Selected the named F13-C remainder — the
deep-course budget question — plus two C.5-review doc repairs
(committed separately as `feccdc4`). No production code changed.

## What ran

Retail install `fnv1a64:e91e6cd4b2ae30d9` (read-only), Apple M1, code
commit `feccdc4` (docs-only over `7fb6cf6` — identical code, stamped
in every leg log). Every event whose C.4 parked-Pro leader topped out
at ≤3 gates inside the ~197 s budget re-ran at 2× budget — 11 legs,
each `mm2 --mm2-path <install> --city <london|sf> --event
checkpoint:<i> --parked --pro --headless --frames 24000` (~394 s sim):

- **11/11 `status=pass`, exit 0.** Full comparison table in
  `docs/race-coverage.md` §F13-C.6; raw logs + `results.txt` local at
  `/tmp/mm2-pro-deep/` (large-capture rule).
- **Zero added finishes** (`opp=0`, `results=0` on all 11 legs) —
  doubling the budget bought recovery churn, not progress: field
  re-anchors 180 → 430 (+139%) against summed banked gates
  121 → 144 (+19%).
- The leader advanced on 6 of 11 events (london-4's 2→5 of 6 the
  largest move) and plateaued on 5 (london-5/9, sf-9/10/11). sf-6's
  `vpauditt` banked all 6 checkpoints — the finish trigger arms at
  full clearance (RACE-7) — and was still racing to it at cap: the
  closest a deep-course opponent has come to resolving.
- **Verdict: predominantly controller-skill-bound, not
  budget-bound.** The deep-course residual's next lever is opponent
  driving competence (F15-B), not longer runs. Budgets beyond ~394 s
  remain untested.
- Parked-control anomalies scale with budget, unchanged in kind:
  london-4's punt loop 209 → 461 falls (`rcv=0w/461f/461r`); the same
  spawn-edge fall loop disclosed on london-5 (122 → 348f), sf-9
  (43 → 172f), sf-11 (49f+7w → 304f+7w). london-3's parked car took
  `cp=1/6` at `moved=110 m` — the disclosed opponent-shove mechanism
  driven further — plus `rcv=17w` water recoveries and `dead=10`
  ambient cars in the spawn-adjacent pileup.
- Physics health: `dropped` ≤218 (london-3's pileup; ≤80 sf-9, else
  0); `peak` ≤12.8 m/s — a shoved parked car, no spikes.

## Review-finding repairs (commit `feccdc4`)

Both C.5-review doc findings repaired:

- `race-coverage.md` C.5's sf-2 hold row was transcribed `cp=1/9`;
  the raw record says `cp=2/9` — fixed (the row's other cells were
  already correct).
- The "cleared early gates on 9 events" claim understated the raw
  logs: 10 hold legs carry `cp>0`. PLAN.md's C.5 row now says 10 and
  discloses sf-8's `cp=1` at `moved=37 m` as reading like the parked
  legs' opponent-shove crossings rather than driven progress (the
  other nine moved 105–589 m); `race-coverage.md`'s C.5 narrative
  gained the same sentence so the count is derivable from the doc.

## Gates

Docs-only iteration — no code edited; the exercised binary is
`feccdc4`, code-identical to the externally checked `7fb6cf6`. Gates
run explicitly on the candidate tree (all three, not just the test
phase): `cargo fmt --all -- --check` clean, `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean,
`cargo test --locked --workspace` green (all suites, 0 failures). The
11 retail legs above exercised the production session path end to end
at 2× the matrix budget.

## Not done / open

- Longer budgets beyond ~394 s remain untested; original-fidelity
  comparisons (retail difficulty, pacing, AI competence) stay
  unverified — engine self-metrics only.
- F13-C stays `active`; its remaining row is the original-fidelity
  comparison.
- The skill-bound verdict points at F15-B's remainders
  (`unkFlag`/`cornerBrakingThreshold`/`weirdPathfinding` — research-
  gated) as the deep-course lever.
