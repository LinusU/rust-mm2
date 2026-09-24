# Last iteration — F15-B.8: documented tail order + avoidOpponents/look-ahead

Task slice on `ralph/night` (baseline `f599ef1`; the F15-B.7 external
review passed with no blocking findings). Selected the F15-B remainder's
authored-tail item: the `[Opponent]` parameter tail's flag-column order
was an inference that two independent sources now contradict — the
community format reference (angel-file-formats `AIMAP.md`, R3) publishes
all ten columns in order, and mm2hook's recovered
`aiVehiclePhysics::RegisterRoute` signature lists its flag arguments in
exactly that order. This slice corrects the mapping and consumes two
more authored fields.

## What changed

`crates/mm2_game/src/opponent.rs`:

- `OpponentDriveParams` re-decoded to the documented order: `unused_flag`
  ← col 1, `distance_padding` ← col 2, `corner_brake` ← col 3,
  `avoid_traffic/props/players/opponents` ← cols 4–7,
  `weird_pathfinding` ← col 8. The superseded inference had the flag
  block one column late — it made `unused` a 43%-set flag on ordinary
  rows and `avoidOpponents` ≈ universal-0; both anomalies resolve under
  the documented order (re-measured on 536 retail rows: `avoidPlayers=0`
  on 75%, `avoidOpponents=0` on 59%, `weirdPathfinding` set on exactly
  the 3 london `crash6`/`race12` rows that also set `unused_flag`).
- Raw `params` kept verbatim; short rows still decode trailing fields
  as `None`.

`crates/mm2_app/src/opponents.rs`:

- `OpponentDriver.avoid_opponents` — authored col 7, default on.
  `senses()` now gates AI participants by it (human participants stay
  under `avoid_players`). An unsensed class is fully transparent — no
  pass, no brake, no ban.
- `OpponentDriver.look_ahead` — authored col 2 (R3's "Look Ahead
  Distance", `RegisterRoute`'s `someDistancePadding`), default 75.0.
  Replaces the speed-scaled corridor reach (`BLOCK_NEAR + v·BLOCK_LEAD`)
  — a designed reading of the documented name; the original's exact use
  is unrecovered (UNK-11).
- `unkFlag`, `cornerBrakingThreshold`, `weirdPathfinding` stay bound
  but unconsumed — R3's brake-demand-floor description needs a
  continuous brake demand our binary corner-brake does not have.

Docs: `docs/research/aimap.md` tail section rewritten (documented order,
per-column retail distributions, consumption); `docs/original-rules.md`
RACE-14 re-classed inferred → documented, RACE-12/UNK-11 wording updated.

## Verification

- Tests (`cargo test --locked --workspace` — all suites green):
  - `mm2_game/tests/opponent.rs` — the decode test asserts the corrected
    positions on a retail `circuit0` row plus a verbatim london `crash6`
    follow-car row separating `unused_flag` (col 1) from
    `weird_pathfinding` (col 8); `stunt0`'s single-value row still leaves
    the tail absent.
  - `mm2_app/tests/opponents.rs` — `authored_avoid_flags_gate_the_corridor`
    (all four sense-mask combinations against human + AI blockers),
    `authored_tail_binds_the_driver_tuning` (production spawn binds both
    avoid flags and per-driver look-ahead),
    `authored_avoid_opponents_decides_the_parked_ai` (a route-less roster
    slot parked mid-lane: authored-1 slips around, authored-0 collides
    head-on) and `authored_look_ahead_sets_the_sensing_distance` (a
    blocker ~90 m ahead: authored-150 commits a pass, authored-30 does
    not react inside the window).
- Retail A/B (`fnv1a64:e91e6cd4b2ae30d9`, `circuit:0 --bot --headless
  --frames 1500`, same command before/after):
  - sf — headline unchanged (`cp=2/9 lap=1/3 pos=4/5 opp=0/4 cu=3
    moved=467m final=(-2154,15.1,-32)`); impacts 58→55; opponent stuck
    peaks 158/98/137/74 → 104/109/110/74. The authored `0 1 0 1 0` rows
    now make the field transparent to the player (avoidPlayers col 6 =
    0) while still sensing each other (col 7 = 1).
  - london — impacts 149→84, `cp=3/6→4/6 pos=5/8→2/8`, `opp_rec=1 cu=6`
    unchanged. The B.6-recorded pacing regression is recovered: the
    fixed authored 50 m reach brakes earlier than the old speed-scaled
    14 + 1.4·v corridor at low speed.
- Gates: `cargo fmt --all -- --check` clean; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` clean;
  `cargo test --locked --workspace` — 0 failures.

## Not done / open

- F15-B parent's remaining items stand: AC06 measured-difficulty leg,
  representative avoidance matrix, `unkFlag`/`cornerBrakingThreshold`/
  `weirdPathfinding` consumption (semantics still unverified).
- The column *order* is documented; the original's runtime *semantics*
  are not — `look_ahead` as corridor reach and both avoid gates are
  designed readings of documented names (UNK-11), not verified original
  behavior.
- GPU/rendered output not exercised — headless physics evidence only.
- Whether retail opponents actually route on the BAI network between
  `.opp` anchors remains an unverified inference (the densified line is
  our implementation choice, recorded as such).
