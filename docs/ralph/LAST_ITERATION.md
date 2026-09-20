# Last implementation iteration

- Task ID and title: F13-B.1 — authoritative standings (finish
  ordering) and placing presentation. First slice of F13-B ("Add
  participant progress, rank/result presentation and opponent
  integration hooks"), the next task named by the selection policy
  after F14-A.1 passed external review (`561b8b7`).
- Starting commit and resulting commits: started at
  `561b8b76c19dd227288179f6f129b9f29a862932` (the externally checked
  F14-A.1 handoff; branch `ralph/night`, clean tree).
- Why this slice: the F14-A remainder is either blocked on F15/F10
  consumers (event `.aimap` exception/density scoping) or bot-limited
  (AC06 representative playtests), and F11-C is mostly satisfied by
  the existing audits/tests. F13-B's rank leg was ready: the spec
  demands an explicit tie resolution (F14 required-behavior 3, F11
  edge cases), the HUD literally noted "placing waits on F13-B", and
  F16 progression will need a "won?" predicate over authoritative
  results.
- Retail install: `/Users/linus/coding/rust-mm2/retail`
  (`fnv1a64:e91e6cd4b2ae30d9`), all runs `--headless` on this
  machine's dev-profile binary.

## What changed

- **`mm2_game::result`: `ResultLedger::standings()`/`place_of()`.**
  The ledger's recorded results now have a defined finishing order —
  the authoritative standings a results screen or progression ranks
  by (DSN-12, designed — no verified original placing rule exists):
  `Finished` outranks `TimedOut` regardless of times, `Finished`
  orders by the recorded `race_ticks`, and equal ticks (a same-tick
  finish or two expiries on the shared deadline) order by `PlayerId`
  — a deterministic tie-break independent of recording/query order.
  A participant with no recorded result is unplaced, not ranked
  last; a participant with several results places by the best.
- **`mm2_app::main` (HUD): place on the Results line.** The
  `Finished` outcome now reads `FINISHED {ord}[ of {n}] {time}s`
  from the standings (ordinal-only for a lone participant);
  `TimedOut` keeps `OUT OF TIME` (a DNF banner, not an ordinal).
  The "placing waits on F13-B" comment is resolved.
- **`mm2_app::smoke`: `place=` in the headless record.** The
  `outcome=` field now picks the *local participant's* result by id
  (not an arbitrary first record) and appends `place={n}` whenever
  the result has a standing — multi-participant headless runs become
  self-describing.
- **`docs/original-rules.md`:** DSN-12 added (standings policy);
  DSN-11's "placing is F13-B scope" wording updated.
- **Doc fix (review nit):** london circuit authored laps are
  `3am/4pro` except c1 *and c2* `2/2`, c9 `3/2` — re-verified with
  `race-defs --table circuit`; PLAN.md corrected in both places it
  claimed only c1 was 2/2. The 60 000-frame finish attempt left
  running last iteration was never recorded and is treated as
  unrecorded (not evidence).

## Tests

- `mm2_game/tests/contracts.rs` (+1):
  `standings_order_by_finish_then_participant` — results recorded out
  of finishing order still rank by `race_ticks`; a same-tick tie
  orders by `PlayerId`; `TimedOut` ranks below every finish;
  unrecorded participants are unplaced.
- `mm2_app/tests/race.rs` (+1):
  `ordered_multi_lap_participants_stay_independent_and_order` — two
  participants (remote + local) driven through a 2-lap Ordered course
  via the production `advance_race` path: each wraps its own
  `next`/`lap` (the remote's lap-1 wrap leaves the local at
  `next=1, lap=0`), the remote's resolution records without ending
  the local race (`Running`/`Playing`), the local's later finish
  ends the session at `Results`, and the ledger's standings order
  them by recorded clock (`place_of` 1 and 2). One-position-per-gate
  drive path so each segment clears exactly one gate (resting inside
  a radius would let the next segment clear the wrapped sequence —
  the swept contract's intended multi-crossing).

## Commands actually run and results

- `cargo fmt --all -- --check` PASS; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` PASS;
  `cargo test --locked --workspace` PASS (all 32 groups incl.
  doc-tests, 0 failures).
- `mm2 --mm2-path <retail> --city london --event blitz:0 --headless
  --bot --frames 2000` → `status=pass`, `phase=results
  race=Complete cp=3/3 results=1 tl=7.6s outcome=finished place=1` —
  the standings field end-to-end on real authored content.
- `mm2-inspect race-defs <retail> --city london --table circuit` —
  10/10 rows build; confirmed c1/c2 both `2/2` (the review's doc
  correction).

## What this proves / does not prove

- Proves: the finish ordering is a defined contract property —
  deterministic, recording-order-independent, with an explicit tie
  policy (F14 req. 3); multi-lap multi-participant progress is
  independent and yields a consistent ordering through the
  production driver (F13-AC03/F14-AC03 legs, test level); `place=`
  reaches the smoke record on a real authored event.
- Does not prove: live position/leaderboard while racing (F14-B);
  opponent-driven standings (F15 — standings are exercised with
  synthetic remote participants, no AI exists); a full results
  screen (F17); any original placing rule — the ordering is
  designed (DSN-12), not verified_original.
- Acceptance IDs: advances F13-AC03 (independent progress +
  consistent ordering) and F14-AC03's ordering leg; F13-AC04 needs
  F15 opponents, F14-AC04/AC05/AC06 stay open.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
