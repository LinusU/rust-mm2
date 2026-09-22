# Last iteration — opponent catch-up assist (F15-B.4)

Iteration 38 on `ralph/night`, continuing from `68a7b99` (the
externally checked F05-B.8 doc-repair candidate — review verdict
**pass**). Task id: `F15-B.4` — the catch-up-assistance leg of
F15-B's difficulty-effects scope (F15 spec req 4: difficulty must
affect documented/tunable behavior; AC06: "difficulty changes have
measured effects, with any catch-up assistance disclosed and tested";
the spec permits rubber-banding only when it is "explicit,
observable, scoped by rules and not falsely recorded as physical
racing").

## Slice choice

Of the F15-B remainder, catch-up was the actionable piece: the
spec's difficulty-effects requirement names it, `RaceProgress`
already carries enough authoritative state to measure a deficit,
`OpponentDriver` already carries authored tuning plus a disclosed
assist counter (`reanchors`), and the smoke record already reports
opponent fields conditionally. The other named remainders stay
gated: `weirdPathfinding`/`distancePadding`/`cornerBrakingThreshold`
consumption waits on semantics verification, `avoidOpponents`
polarity is open (inert meanwhile), and AC06's measured-difficulty
evidence leg needs the controlled amateur/pro comparison the
parameter tail only partially enables.

Whether the original rubber-bands trailing opponents at all is
unverified (UNK-11) — nothing recovered pins down original catch-up
semantics — so the entire policy is designed and recorded as
DSN-27, not an original-behavior claim.

## What changed

- `crates/mm2_game/src/race.rs` (+ `lib.rs` exports): the pure
  catch-up contract.
  - `CatchUpPolicy` — `deficit_full` 2.0 gates, `assist_max` 0.25;
    both implementation choices, `pub` for test/evidence binding.
  - `mean_gate_spacing` — mean authored spacing between consecutive
    checkpoint centres (`None` under two gates / no finite leg).
  - `course_progress` — a participant's continuous course position
    in gate units: banked gates (`lap × gates + next` `Ordered`,
    cleared count `AnyOrder`) plus the covered fraction of the leg
    toward the same objective `live_order` tie-breaks on
    (`checkpoints[next]` / `navigation_target`'s nearest remaining
    gate or armed finish). Non-finite position, out-of-range `next`,
    missing objective or dead `leg_ref` degrade to the banked count.
  - `catch_up_factor` — 0 at/ahead of the lead, linear to
    `assist_max` at `deficit_full`, NaN/garbage-safe.
- `crates/mm2_app/src/opponents.rs`: `opponent_drive` resolves the
  leader as the best `course_progress` across every
  progress-carrying participant (p1 gains `Option<&RaceProgress>`)
  — the human included — over one shared leg scale (`mean_gate_spacing`,
  designed fallback `CATCH_UP_LEG_REF` = 80 m). A trailing AI
  driver's demand ceiling lifts one-directionally —
  `throttle_cap + assist` bounded 1.0, `corner_speed × (1 + assist)`
  — on a per-frame tuning copy, never mutating the authored values.
  `OpponentDriver` gains `catch_up_policy` and `catch_up` (the live
  factor, zeroed on every non-driving path including the re-anchor
  branch — kept distinct from `reanchors`: a lifted demand is not a
  recovery). The player carries no `OpponentDriver` and is never a
  recipient; nobody is ever slowed below authored tuning; progress
  is still earned through the same swept-trigger validation.
- `crates/mm2_app/src/smoke.rs`: `cu=<n>` counts drivers with
  `catch_up > 0`, emitted on activity only like `opp_rec=` —
  unassisted runs stay bit-identical.

## Tests

- `tests/race.rs` +5 (26 total): `mean_gate_spacing` mean/degenerate
  legs; ordered `course_progress` banking lap×gates+next plus the
  leg fraction incl. a leader/follower deficit measure; any-order
  cleared-count + nearest-objective fraction; degenerate inputs
  (out-of-range `next`, non-finite position, dead `leg_ref`, empty
  definition) stay finite; `catch_up_factor` ramp, saturation, and
  garbage legs.
- `tests/opponents.rs` +1 (32 total): production-path A/B — a
  parked player teleported through the shared `advance` validation
  leads at ~3.3 gate units; both trailing opponents report
  `catch_up > 0` bounded by `assist_max` and their `VehicleInput`
  exceeds the authored `throttle_cap` (impossible without the lift);
  leaders and the countdown-locked field report `catch_up == 0`;
  `reanchors` stays 0; no progress is granted; the player is never
  a recipient.

## Gates

- `cargo fmt --all -- --check` — pass.
- `cargo clippy --locked --workspace --all-targets --all-features
  -- -D warnings` — pass.
- `cargo test --locked --workspace` — pass (63 suites, 0 failures).

## Evidence (retail `fnv1a64:e91e6cd4b2ae30d9`, dev build at `68a7b99+diff`)

- `london --event circuit:0 --bot --headless --frames 2700`:
  `status=pass race=Running lap=2/3 cp=2/6 results=0 pos=1/8
  opp=0/7 opp_rec=1 cu=7` — the bot leads, all 7 opponents trail
  and are assisted; `opp_rec=1` (vpcoop re-anchor) stays a distinct
  counter.
- `sf --event checkpoint:0 --bot --headless --frames 1500` (mid-race):
  `race=Running cp=3/6 results=0 pos=5/7 opp=0/6 cu=5` — 5 of 6
  opponents trail the leading opponent and are assisted; the leader
  gets nothing (one-directional).
- `sf --car vpbug --bot --headless --frames 600`: `status=pass
  impacts=12 dmg=3a/0d/0r rej=3 dup=0 vsk=6a/0d/0r spk=6b/20e/18x`
  — bit-identical to the F05-B.8 review record.
- `london --bot --headless --frames 600`: `status=pass impacts=7
  dmg=1a/0d/0r rej=4 dup=0 vsk=5a/0d/0r spk=5b/23e/23x` —
  bit-identical. No `opp=`/`cu=` fields on roster-free runs.

`cu=` is evidence of the *designed* policy firing in the production
path — not retail fidelity, and not a measured-difficulty A/B
(AC06's remaining leg).

## Disclosures and gaps

- `cu=` reports the *live* assist: it zeroes when the session leaves
  `Playing` (resolved/held drivers get zeroed input), so
  end-of-race records show none. It is deliberately not a
  cumulative counter — the `driver.catch_up` component is the
  per-frame observable; a cumulative assist-seconds metric is a
  possible later refinement.
- `sf checkpoint:0`'s end-of-run `status=fail "fell through the
  world"` is the pre-existing altitude-threshold trip: the car is
  grounded (`wheels=4/4`, `cp=3/6`, still racing) but the course
  descends >25 m below the spawn altitude. Same failure class noted
  in iteration 11's records — unrelated to this slice (the player
  is never assisted; the check compares final altitude to spawn).
- F15-B's parent stays open: AC06's measured amateur-vs-pro
  difficulty A/B, representative avoidance matrix, the remaining
  authored-tail columns once semantics verify, `avoidOpponents`
  polarity (decoded, inert).
- Catch-up applies to AI opponents only and on authority-local
  `opponent_drive`; remote participants (F25+) are untouched.

Files: `crates/mm2_game/src/race.rs`, `crates/mm2_game/src/lib.rs`,
`crates/mm2_game/tests/race.rs`, `crates/mm2_app/src/opponents.rs`,
`crates/mm2_app/src/smoke.rs`, `crates/mm2_app/tests/opponents.rs`,
`docs/original-rules.md`, `docs/ralph/PLAN.md`,
`docs/ralph/LAST_ITERATION.md`.
