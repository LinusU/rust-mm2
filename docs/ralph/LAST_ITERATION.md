# Last iteration — F15-B.11: scripted-player re-anchor occupancy parity

Task slice on `ralph/night` (baseline `6ee4e9b`; the F15-B.10 external
review passed with no blocking findings). Selected the F15-B
remainder's only non-research-gated item, named by that review as a
follow-up: the scripted player's re-anchor (`scripted.rs`) kept gate +
ground-probe rejection only — no participant occupancy — so a penned
`--bot` car could teleport onto a participant (opponent or, beached on
the line, itself) and contaminate the evidence the driver exists to
produce. The remaining F15-B items (`unkFlag`/`cornerBrakingThreshold`/
`weirdPathfinding` consumption) are research-gated under UNK-11.

## What changed

`crates/mm2_app/src/opponents.rs`:

- `reanchor_occupied` extracted as the shared occupancy predicate —
  the `REANCHOR_CLEAR` (8 m XZ) / `REANCHOR_CLEAR_Y` (4 m band) test
  `opponent_drive` gained in B.10, now the one definition both
  re-anchor sites run so the two recoveries cannot drift apart.
- `reanchor_pose` doc updated: both callers add the occupancy leg.

`crates/mm2_app/src/scripted.rs`:

- `scripted_drive` gains a `participants` query (`&Position`,
  `With<Player>`) snapshotted frame-start into `occupants` — the same
  set `opponent_drive`'s `traffic` covers (local, remote, AI; the
  scripted car itself included, so a car beached on the line does not
  teleport back onto itself) — plus a `claimed: Vec<Vec3>` for poses
  a same-frame re-anchor already took (a session fields one
  `PlayerVehicle`, but the structure mirrors the opponent loop).
- The re-anchor `blocked` closure adds `|| occupied(p)` beside the
  gate + `GROUND_PROBE` legs; the lift, walk-back budget
  (`REANCHOR_WALK` 60 m — a jammed route still lands bounded),
  `Teleported` dispatch and `reanchors` disclosure are unchanged.

`crates/mm2_app/tests/bot.rs` (+1, 19 total):

- `penned_bot_reanchor_lands_clear_of_a_parked_participant` — pens the
  bot in the off-route pocket, waits for `Racing` + the chase index to
  settle (it only advances, and nothing banks while penned), reproduces
  the walk-back with the gate leg alone to find the pose it would claim
  ((−116,−140) on the test route), parks a `Player`+`Position`
  participant there, and asserts the real landing is on the line,
  upright, ≥ `REANCHOR_CLEAR` off the blocker, and banks nothing.
  Verified non-vacuous: with `occupied` disabled the car lands 0.004 m
  from the blocker — the leg is what spaces the landing.

## Verification

- Tests: `cargo test --locked --workspace` — all suites green
  (`tests/bot.rs` 18→19, `tests/opponents.rs` 40).
- Gates: `cargo fmt --all -- --check` clean; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` clean.
- Retail (`fnv1a64:e91e6cd4b2ae30d9`, `sf circuit:0 --bot --headless
  --frames 9000`): `status=pass cp=4/9 lap=2/3 pos=1/5 opp=0/4
  opp_rec=5 cu=4 p_rec=3r/6e` — the F15-B.7/9/10 envelope holds
  (per-slot `opps=0:vpbug/6c/1l/15e/2r/900w,…` unchanged).

## Not done / open

- F15-B parent's remaining items are research-gated only:
  `unkFlag`/`cornerBrakingThreshold`/`weirdPathfinding` consumption
  (runtime semantics unverified, UNK-11) and original-runtime semantics
  of the consumed fields (designed readings meanwhile).
- The occupancy snapshot is positional — a `Player` without
  `Position` cannot occupy (none is ever spawned without one).
  Ambient traffic is deliberately outside the contract, matching
  `opponent_drive`'s participant-only snapshot.
- Whether the original rubber-bands at all stays unverified — the
  bounded assist is a designed policy (DSN-27), disclosed in records.
- GPU/rendered output not exercised — headless physics evidence only.
