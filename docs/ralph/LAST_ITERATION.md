# Last implementation iteration

- Task ID and title: F14-A.1 — circuit/lap binding hardening and
  evidence. First slice of F14-A ("Bind original circuit/lap rules to
  the shared checkpoint runtime"), the next slice named by the
  selection policy after the F04-C.2/-C.3 evidence re-take.
- Starting commit and resulting commits: started at
  `79733f8bfe15a80a42252b1b5eab81a61a1414a5` (the externally checked
  docs-only re-take; branch `ralph/night`, clean tree).
- Why this slice: F13-A's remaining legs are all parked behind other
  features (unlock/progression → F16, destruction penalty → F05,
  tod/weather → F18) and F11-C is mostly satisfied by the existing
  audits/tests, so F14-A was the highest-value ready task. Most of the
  binding already existed (Ordered rule + `NumLaps` + `_strtpnts`
  grids + pathset barricade overlays landed via F11-B.2/F03-B.2), so
  this slice closes the honesty holes a review would flag rather than
  re-implementing.
- Retail install: `/Users/linus/coding/rust-mm2/retail`
  (`fnv1a64:e91e6cd4b2ae30d9`), all runs `--headless` on this
  machine's dev-profile binary.

## What changed

- **`mm2_content::race_def`: authored `NumLaps` is a checked value.**
  Ordered builds previously did `params.num_laps.max(1) as u32` — a
  silent clamp (0/negative → 1 lap) plus a truncating `as` cast
  (`2³²+2` → 2) while every other authored parameter rejects
  out-of-range values as `BadParam`. `NumLaps` now binds like the
  rest: zero, negative and overflowing values are named
  `BadParam { field: "NumLaps" }` build errors. Any-order rows keep
  `laps: 0` regardless of the column — its constant 3/4 there is
  template junk (UNK-5), never validated.
- **`mm2_game::race`: `RaceProgress::advance` is inert outside
  `Racing`.** Feeding positions to an `AwaitingStart` participant
  re-anchors the swept segment without clearing; feeding them to a
  `Finished`/`TimedOut` participant can never re-emit a finish or
  clear more gates. Previously the contract relied on every caller
  gating on the lifecycle — `advance_race` does — but the once-only
  rule (AC04, and F14-AC02's "repeated finish hits do not accumulate
  laps") is now a contract property. The dead `with_next` builder
  (zero callers — a vestige of a gate-ordering idea the producer
  never needed) is removed, and `Ordered`'s doc now states the
  start-lap semantics explicitly: lap 1 begins at countdown release,
  the start-line copy closes each lap, so the line crossing *is* the
  lap boundary.
- **`mm2_app::smoke`: `lap={current}/{laps}` in the headless record**
  for Ordered definitions — a multi-lap `--bot` run is now
  self-describing (`cp=` is per-lap cleared). Any-order records stay
  bit-identical.

## Tests

- `mm2_game/tests/race.rs` (+2): `the_lap_line_only_closes_a_completed_sequence`
  (oscillating on the closing gate without the required sequence
  clears nothing; a lap counts once; post-wrap line hits aim at the
  new lap's first gate; lap 2's line crossing finishes) and
  `a_participant_outside_racing_cannot_advance` (AwaitingStart
  re-anchors; Finished stays inert).
- `mm2_content/tests/race_def.rs` (+1):
  `circuit_num_laps_is_a_checked_authored_value` — `0`/`-2`/
  `5 000 000 000` → `BadParam("NumLaps")`; independent per-difficulty
  blocks (valid amateur, broken professional); `-9` on a checkpoint
  row ignored as template junk.
- Adjusted (honestly, for the new contract guard): the shared-step
  pin in `mm2_app/tests/race.rs` and the marker test in
  `mm2_app/tests/event.rs` now put the participant `Racing` before
  calling `advance` — the same state the driver gates on.

## Commands actually run and results

- `cargo fmt --all -- --check` PASS; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` PASS;
  `cargo test --locked --workspace` PASS (all groups incl. doc-tests).
- `mm2-inspect race-defs <retail> --table circuit` — 10/10 rows per
  city build at both difficulties with distinct authored laps
  (CIR-5): london `3am/4pro` except circuit1 `2/2` and circuit9
  `3/2`; sf `3/4` except circuit8-9 `2/2`. Per-event gate counts
  6–23, `4–7 opp`, `0 cop` (CIR-4), `1–8 slt` (SF `cir<N>_strtpnts`
  on circuit1–8).
- `mm2 --mm2-path <retail> --city london --event circuit:0
  --headless --bot --frames 12000` → `status=pass`,
  `race=Running cp=2/6 lap=2/3` at 23 640 ticks: mid-race lap
  tracking on real authored content.
- Same with `--pro --frames 4000` → `lap=2/4`: the professional
  4-lap block binds end-to-end.
- A `--frames 60000` finish attempt was left running; see PLAN.md
  for its outcome. Note: both runs converge deterministically to the
  same wedged spot on lap 2 (`final=(-413,0.0,-169)`,
  `wheels=3/4`) — the scripted driver grinds the stuck-escape
  against course furniture; bot-limited, not a lap-logic defect
  (the earlier matrix recorded london circuit:0
  `outcome=finished cp=6/6` under the same driver on the same
  install — the escape eventually frees it given enough ticks).

## What this proves / does not prove

- Proves: authored `NumLaps` reaches the runtime definition per
  difficulty (CIR-5 verified data, now honestly validated); the
  closing-gate-only-counts-once and resolved-participant-inertness
  negatives are contract-tested; `lap=` records the ordered progress
  on real retail circuits at both difficulties; CIR-2's closed
  cross-streets remain evidenced by the event pathset barricades
  (181 stamped on circuit0).
- Does not prove: opponent integration (F15 — the authored `opp`
  counts are carried, not consumed); rank/lap-timing presentation
  beyond `cp=`/`lap=` (F14-B); traffic-density effects (no traffic
  system exists — the authored `Ambient=0`/`Peds=0` for circuits is
  carried, CIR-3/CIR-4); event `.aimap` `[Exceptions]` closed-road
  binding into a live session (no consumer yet — opponents' routing
  is F15); direction-of-travel enforcement (`require_direction`
  stays a designed off-by-default, DSN-5).
- Acceptance IDs: advances F14-AC01 (per-event lap/route binding
  evidenced on the full circuit table) and F14-AC02's negative legs
  (repeated finish hits, missing-gate traversal, reset — the last
  already covered); F14-AC03/AC04/AC05/AC06 stay open.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
