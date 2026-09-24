# Last iteration — F10-B.11 review repair: per-car same-tick junction claims

Repair iteration on `ralph/night` (baseline `b4e5986`, the F10-B.11
candidate the external review rejected). One blocking finding: the
landing-rollback branch in `drive_ambient` called a junction-keyed
`junctions.release(ix)` on a claim the rolling-back car never held —
`commit` ran only in the landing-accepted branch, so the release
could only no-op or strip a *different* car's same-tick claim.
Reachable on retail data wherever a mixed-rule junction's free-flow
end rolls back over a gated car's live commit in one tick
(london 135/328, sf 78/214 connected junctions mix gated and
NeverStop ends, per the review's probe).

## Root cause

`entered: BTreeSet<u16>` keyed claims by junction alone, and the
rollback released by junction — two cars claiming the same junction
in one tick (a gated commit plus a free-flow commit, which never
consults the record) shared one indistinguishable record.

## Fix

- `mm2_game::traffic`: `entered` is now `BTreeSet<(u16, Entity)>` —
  one claim per `(junction, car)`. `commit(ix, car)` records the
  owner; `release(ix, car)` sheds only the caller's own claim;
  `entered(ix)`/`gate` read "any claim on `ix`" as before.
  `advance_tick` still clears all claims at tick start.
- `mm2_app::traffic`: the commit now runs on `LaneAdvance::Entered`
  *before* the landing-occupancy check — the ordering the doc
  comment already described — so a rejected landing genuinely
  releases *its own* claim rather than reaching for a record it
  never held. Net `entered` state per evaluation is unchanged:
  accepted → claimed, rolled back → unclaimed.

The reviewer's alternative suggestion (`BTreeMap<u16, Entity>`)
would still lose the gated claim when a free-flow car overwrote
then released the single slot; the pair-set removes that residual
case too.

## Tests

- `mm2_game` — `a_committed_entry_holds_the_box_for_the_rest_of_
  the_tick` extended: a second car commits over the live claim, its
  rollback sheds only its own (the box stays closed for the FCFS
  follower), releasing an unheld claim is a no-op, the owner's
  release re-admits, `advance_tick` sheds the record.
- `mm2_app` +1 — `a_rolled_back_free_flow_entry_keeps_the_committed_
  claim`: mixed-rule junction (TrafficLight approach + NeverStop
  approach, 2-lane fixture). A and C ride parallel lanes of the
  green member with identical speed profiles → same-tick lane end;
  B is re-armed a hair short of its NeverStop lane end before every
  update (one fixed drive step per update via
  `TimeUpdateStrategy::ManualDuration(1/120)`), so its blocked
  landing rolls back on whichever drive tick A commits; spawn order
  fixes the in-tick evaluation order A → B → C. B's landing lanes
  are covered by wrecks parked above the zone's vertical band —
  inside `enter_clearance` of the landing points, outside the
  occupancy zone, so the box reads empty to the gated approaches.
  Asserts both gated cars eventually cross and at most one holds a
  committed crossing per tick. **Verified to fail under a
  junction-keyed release** (locally neutered `release` to
  `retain(|&(j, _)| j != ix)` → `shared=2`; restored after).
- Existing `a_same_tick_second_arrival_waits_for_the_committed_
  crossing` still passes unchanged.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` green — 67 suites, 986 tests,
0 failures (mm2_game traffic 39, mm2_app traffic 30).

## Not done / open

- Retail-install re-run of the `--headless --frames 3000` legs not
  performed this iteration (logic-equivalent claim bookkeeping —
  the record's observable set semantics are unchanged for
  single-claim ticks; the app test covers the mixed-rule
  interleaving). The prior candidate's london/sf signatures stand
  as the B.11 baseline; a re-run is worthwhile if the reviewer
  wants fresh retail evidence at the repaired commit.
- Same-tick claims remain a designed tick-granular serialization;
  original junction arbitration stays unverified (UNK-12).
- F10-B stays `active`: collision fidelity vs AC03's checklist,
  original spawn/junction timing and interior-crossing geometry,
  signal-prop model fidelity, F10-AC evidence legs.
- Box occupancy still point-based; props/statics don't occupy.
- No rendered/manual playtest evidence — headless and synthetic
  coverage only.
