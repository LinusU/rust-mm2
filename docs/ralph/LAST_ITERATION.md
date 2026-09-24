# Last iteration — F10-B.11: same-tick junction-entry arbitration + C.6 review-finding repairs

Task slice on `ralph/night` (baseline `e3e2875`, the externally
checked F13-C.6 commit). Selected the F10-B "queue-through-
intersection priority" remainder's same-tick half — plus three
C.6-review doc repairs (committed separately as `e72304f`).

## What changed

The junction box-yield's occupancy report (`blockers`/`bound_for`)
is a frame-start snapshot: a second eligible car evaluated *after*
another car's crossing commit in the same drive tick read the box
empty, and both took the interior at once. The window is real
wherever two gated approaches admit simultaneously — the parallel
lanes of a green member road are the sharpest case.

- `mm2_game::traffic::Junctions` gains a per-tick
  `entered: BTreeSet<u16>`. `gate` reads
  `occupied = box_occupied || entered.contains(ix)` on exactly the
  two paths a gated rule can open (green member, FCFS head) —
  `NeverStop`/unruled ends keep authored free flow and never
  consult the record. `commit(ix)` claims the junction,
  `release(ix)` undoes a rolled-back transfer, `entered(ix)`
  exposes the state to tests/diagnostics, and `advance_tick`
  clears the set once the next tick's physical box-yield sees the
  committed car.
- `mm2_app::traffic::drive_ambient`: on `LaneAdvance::Entered` the
  junction index comes off the previous lane; the existing landing
  occupancy check runs first — a rejected landing reverts the
  cursor, zeroes speed, decrements `crossings`, *and* releases the
  claim; a passing landing commits the claim, then departs the
  FCFS slot as before.

This is a designed approximation, disclosed: the claim serializes
the box at tick granularity — a car already rolling when the gate
closes stands at its lane end mid-green rather than sharing the
box. That is consistent with the existing per-car box
serialization (a one-tick-earlier arriver already won this), not a
claim about original arbitration (UNK-12 stays open).

## Tests

- `mm2_game` +2: `a_committed_entry_holds_the_box_for_the_rest_of_
  the_tick` (commit closes the now-front FCFS car on an empty
  snapshot; `release` re-admits immediately; `advance_tick` sheds
  the record) and `a_committed_entry_closes_a_green_approach_and_
  not_free_flow` (a commit closes a green approach mid-phase while
  a `NeverStop` end stays open).
- `mm2_app` +1: `a_same_tick_second_arrival_waits_for_the_
  committed_crossing` — two cars on parallel lanes of one signal
  member reach the lane end in the same tick (identical speed
  profiles make the same-tick arrival deterministic); at most one
  committed crossing is ever active and both eventually take the
  junction. **Verified to fail without the record** (`shared=2` —
  both cars in the box at once). The BAI fixture gained a
  parameterized lane-offset variant (`bai_with_lane_offsets`) so a
  road can carry two driving lanes; the existing single-lane bytes
  are unchanged.

## Gates + retail sanity

`cargo fmt --all -- --check` clean, `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean,
`cargo test --locked --workspace` green (all suites, 0 failures;
mm2_game traffic 39, mm2_app traffic 29).

Retail install `fnv1a64:e91e6cd4b2ae30d9` (read-only), Apple M1,
`--headless --frames 3000` against the B.9 baseline:

- sf: `traf=16/16 sp=43 rec=27 dead=0 uns=0 q=0 jq=3 stuck=0 crx=59
  jmp=0 kn=1 sig=647 sigd=3` — bit-identical to B.9.
- london: `traf=16/16 sp=22 rec=6 dead=0 uns=0 q=0 jq=3 stuck=0
  crx=66 jmp=0 sig=828` — vs B.9's `jq=2 crx=68`: the expected
  deterministic signature of a same-tick pair now serializing (one
  more held-approach observation, two fewer in-window commits).
  No new stuck/dead/dead-end counters; `jmp=0` in both cities.

## Review-finding repairs (commit `e72304f`)

All three C.6-review findings repaired, each verified against the
raw `/tmp/mm2-pro-deep/` records:

- The published aggregate `160 → 380 (+138%)` did not reconcile —
  the raw `opp_rec` sums and the doc's own table give
  `180 → 430 (+139%)`. Fixed in `race-coverage.md`,
  `LAST_ITERATION.md`, and `PLAN.md`.
- The `rec`/`opp_rec` gloss claimed "escape+re-anchor count" —
  `smoke.rs` sums only `driver.reanchors`; escapes are the
  separate per-opponent `e` field. Gloss corrected.
- The anomalies bullet named only london-3's `moved=110 m` as
  exceeding the Pro matrix's ≤29 m bound; london-5 (58 m) and
  london-9 (30 m) exceed it too — all three now named.

## Not done / open

- Same-tick claims are tick-granular serialization — a designed
  approximation; original junction arbitration, timings and
  queueing discipline remain unverified (UNK-12).
- F10-B stays `active`: collision fidelity vs AC03's full
  checklist (player-hit feel, damage), original spawn/junction
  timing and interior-crossing geometry, signal-prop model
  fidelity, and the F10-AC evidence legs remain.
- Box occupancy is still point-based (hull extents excluded);
  props/static geometry don't occupy the box.
- No rendered/manual playtest evidence this slice — headless and
  synthetic coverage only.
