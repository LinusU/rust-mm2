# Last iteration — F13-C.5: Professional hold legs + C.4 review-finding repairs

Task slice on `ralph/night` (baseline `7fb6cf6`, the externally
checked F13-C.4 commit). Selected the named F13-C remainder — the
Professional `hold`-driver legs — completing the Pro runtime matrix
at all three drivers. No production code changed; runtime-evidence
slice plus four review-finding doc repairs (committed separately as
`d19b5ca`).

## What ran

Retail install `fnv1a64:e91e6cd4b2ae30d9` (read-only), Apple M1,
code commit `7fb6cf6` (docs-only over `11af1ea` — identical code,
stamped in every leg log), each leg
`mm2 --mm2-path <install> --city <london|sf> --event checkpoint:<i>
--pro --headless --frames 12000` (no driver flag → `Hold`):

- **24/24 legs `status=pass`, exit 0** — the Professional matrix is
  now complete: 72 legs = 24 events × scripted + parked + hold.
  Full tables in `docs/race-coverage.md` §F13-C.5; raw logs +
  `results.txt` local at `/tmp/mm2-pro-hold/` (large-capture rule).
- `Hold` drives blind at full throttle — a bad participant, not a
  control: it never resolved (no `place`), cleared early gates on
  9 events before wrecking (cp ≤2).
- **12 opponent finishes with ledger results across 5 events while
  the hold car raced and never resolved** (london-0 4/6, sf-0 4/6,
  sf-2 1/7, sf-3 2/6, sf-4 1/5) — vs the amateur hold legs' 12
  across 7 events.
- Restart contamination is worst under the blind driver, as
  expected: london-10 rs25 (matching amateur hold's 25 on the same
  event), london-3 rs17, london-9 rs9 (still `Countdown` at cap),
  london-8 rs8, sf-1 rs9, sf-11 rs6, london-2 rs5. The same events'
  parked legs show the field progressing regardless (omax 2–7).
- london-4 repeats its parked-leg anomaly verbatim: the elevated
  spawn gets punted over the edge and re-anchored
  `rcv=0w/209f/209r`, ending `wheels=4/4` — the loop is the spawn
  ground's, not the driver's.
- Physics health: `dropped` ≤21 (4 legs), `peak` ≤48.6 m/s
  (london-2 — blind full throttle downhill; bounded, no spikes).
- Record honesty: `dup=0` wherever the damage/results pipeline ran;
  london-3/london-9 omit the `dmg=`…`txl=` tail because `impacts=0`
  (activity gating — `results=0`, nothing to duplicate).
  `wheels=0/4` london-9 (never left countdown)/london-11, `3/4`
  london-2 — disclosed end poses.

## Review-finding repairs (commit `d19b5ca`)

All four C.4-review doc findings repaired:

- The parked-control Pro finish count was **13, not 16** — the extra
  3 were london-0's *scripted*-leg opponent finishes while the local
  raced to place 4. Fixed in `race-coverage.md` (C.4 narrative + AC04
  mapping) and the PLAN.md C.4 row, each now spelling out the 13
  parked-only / 16 both-drivers split. This file is rewritten per
  iteration, so the stale 16 in the old handoff is gone with it.
- The amateur-vs-Pro comparison claimed "same rate on london-0/sf-0,
  slower elsewhere" — contradicted on london-2 (0/7 → 2/7, faster)
  and london-0 (3/4 → 5/6). Now a per-event comparison.
- The instruments paragraph still implied all parked legs were
  Amateur — now names which legs ran which difficulty.
- The raw matrix artifact locations were unnamed — the instruments
  paragraph now points at `/tmp/mm2-pro-matrix/` (C.4) and
  `/tmp/mm2-pro-hold/` (C.5).

## Gates

Docs-only iteration — no code edited; the exercised binary is the
externally checked `7fb6cf6` code (identical to `11af1ea`). Gates
re-run on the candidate tree: `cargo fmt --all -- --check` clean,
`cargo clippy --locked --workspace --all-targets --all-features --
-D warnings` clean, `cargo test --locked --workspace` green (all
suites). The 24 retail legs above exercised the production session
path end to end at Professional.

## Not done / open

- Deep London/SF non-finishes remain budget- and controller-skill-
  bound; original-fidelity comparisons (retail difficulty, pacing,
  AI competence) stay unverified — engine self-metrics only.
- F13-C stays `active`; remaining rows per the task table
  (deep-course budgets, fidelity comparison).
