# Last implementation iteration

- Task ID and title: F15-B.2 — the authored `[Opponent]` parameter
  tail: decode it into `OpponentDriveParams` (mm2hook's recovered
  `OpponentData`/`RegisterRoute` vocabulary — inferred mapping) and
  consume the parts that can be supported without overstating
  uncertain original semantics.
- Starting commit: `d64ba5523afc4ef1f311a5c239bc3b50e278eb28` on
  `ralph/night` — the iteration-11 candidate the external review
  passed.
- Retail install: `/Users/linus/coding/rust-mm2/retail`
  (`fnv1a64:e91e6cd4b2ae30d9`).

## What changed

- `mm2_game::opponent` — new `OpponentDriveParams` +
  `OpponentSpec::drive_params()`: the ten-value tail decodes
  positionally into mm2hook's recovered vocabulary (R4): col 0
  `maxThrottle`, col 1 `weirdPathfinding`/`BadPathfinding` flag, col 2
  `someDistancePadding`/`TurnRadius`, col 3
  `cornerBrakingThreshold`/`TurnSpeedMultiplier`, cols 4–8
  `unused`/`avoidTraffic`/`avoidProps`/`avoidPlayers`/
  `avoidOpponents`, col 9 `cornerSpeedMultiplier`. The mapping is
  explicitly *inferred, not original-verified*: mm2hook's own
  `OpponentData` field order does not match retail distributions
  positionally, so the assignment orders the same recovered names by
  what each column's authored values can be — every column's range
  matches the corresponding `RegisterRoute` default (536 rows
  measured). Short rows (`stunt0`'s single value) decode trailing
  fields `None` — authored absence, not zero. Raw params are kept
  verbatim beside the decode.
- `mm2_app::scripted` — new `ScriptedTuning { throttle_cap,
  corner_speed }` overlay + `scripted_input_tuned`: `throttle_cap`
  ceilings every throttle demand (recovery included; a ceiling, not a
  scale), `corner_speed` sets the corner-brake engage speed.
  `ScriptedTuning::DEFAULT` reproduces the pre-tail law bit-for-bit;
  `scripted_input` is now a default-tuned wrapper, so the `--bot`
  evidence driver is untouched.
- `mm2_app::opponents` — `Traffic` gains `control: PlayerControl`;
  `nearest_blocker` takes a sense predicate; `OpponentDriver` binds
  `tuning` + `avoid_players` at spawn from `spec.drive_params()`
  (`None` columns → the `RegisterRoute` defaults = pre-tail
  behavior). `driver.senses` gates human participants on the authored
  `avoidPlayers` flag; the corridor filter, the room scan and the
  held-target path all run through it, so an unsensed participant is
  fully transparent — no blocker, no brake, no pass, no ban.

## Deliberate scope decisions (inferred mapping, honestly partial)

- `avoidOpponents` is **bound but inert**: retail authors it ≈
  universally 0, so consuming it under this inferred mapping would
  blind every stock opponent to the rest of the field — either the
  original genuinely never avoids AI, or the flag's order/polarity
  here is wrong. Held unverified (UNK-11); the corridor senses AI
  unconditionally meanwhile. `avoidPlayers` *is* consumed because it
  carries real authored variance (0 and 1 rows coexist in the same
  file, e.g. `race/sf/race1.aimap_p`), so the gate differentiates
  per-driver.
- `avoidTraffic`/`avoidProps` bound but inert — no ambient-traffic or
  prop runtime classes exist to sense (F10 scope).
- `weirdPathfinding`, `distance_padding`, `corner_brake`,
  `unused_flag` bound but unconsumed — semantics unverified.

## Tests

- `mm2_game/tests/opponent.rs` — `drive_params_decodes_the_authored_
  tail`: a verbatim retail row binds all ten columns; the `stunt0`
  single-value row leaves trailing fields `None` (4 total).
- `mm2_app/tests/bot.rs` — `tuned_input_law_scales_throttle_and_
  corner_floor`: the cap ceilings over-cap bands and leaves under-cap
  bands; a doubled `corner_speed` floor drops the corner brake at
  20 m/s; `ScriptedTuning::DEFAULT` reproduces the untuned law
  bit-for-bit across bearings (7 total).
- `mm2_app/tests/opponents.rs` — `authored_avoid_players_gates_the_
  corridor` (sense predicate + inert `avoidOpponents`),
  `authored_tail_binds_the_driver_tuning` (production spawn binds
  throttle cap, corner floor, avoid flag), `authored_max_throttle_
  measures_on_track` (0.5 vs 1.0 cap twins on identical lanes — the
  uncapped car covers >10 m more in 480 updates),
  `authored_avoid_players_decides_the_parked_player` (sensed leg
  slips past the parked local car without contact; unsensed leg
  collides and shoves it ~67 m down the road). Existing tests updated
  for `Traffic.control`/`nearest_blocker` predicate and the
  retail-realistic flag block on the synthetic roster (24 total).
- `mm2_content/tests/opponents.rs` — unchanged, all 9 pass.

## Commands actually run and results

- `cargo fmt --all -- --check` PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` PASS.
- `cargo test --locked --workspace` — all groups ok, 0 failures
  (opponents 24, bot 7, mm2_game opponent 4, mm2_content opponents 9).
- Retail evidence — `fnv1a64:e91e6cd4b2ae30d9`, deterministic
  headless (`./target/debug/mm2 --mm2-path …/retail`):
  - `sf checkpoint:0 --headless --frames 5400` amateur (vpbug
    0.70–0.75 throttle): `opp=4/6`; `--pro` (0.93–1.00): `opp=4/6`;
    at 3600 updates amateur 4/6 vs pro 3/6 — *dynamics data, not a
    controlled throttle A/B*: the difficulty rosters differ in
    vehicles and routes too, and a throttle ceiling mostly changes
    acceleration, not top speed.
  - `sf circuit:1 --headless --frames 900` (hold driver):
    `final=(-507,18.9,-52)` — bit-identical to the verified record;
    the player path is unaffected by opponent tuning.
  - `sf circuit:6 --headless --bot --frames 5400`: amateur `pos=2/7`
    `final=(-1786,57.1,-313)` (prior verified record pos=2/7
    `final=(-1759,·,-307)` — same standing, position perturbed by the
    now-bound authored corner multipliers 0.98/1.02 + interactions);
    `--pro` `pos=5/7` `moved=98m` — different field composition,
    disclosed as dynamics data.
  - `mm2-inspect dump` re-verified the tail distributions cited
    (536 ten-value rows; `avoidOpponents` ≈ universal 0; pro csm
    reaches 2.29 on `race/sf/race5.aimap_p`).
- Evidence classification: code gates + synthetic integration tests +
  deterministic headless retail runs. No GPU/rendered/audio evidence;
  no original-executable comparison exists — the column mapping and
  consumed semantics remain *inferred*.

## Ledger / research updates

- `docs/original-rules.md` — RACE-14 new (the inferred tail decode +
  partial consumption); RACE-12's tail note now points at it; UNK-11
  narrowed (original consumption still unverified, including the
  `avoidOpponents` polarity question).
- `docs/research/aimap.md` — new *Opponent parameter tail* section:
  the full column table, the `avoidOpponents` caveat, the difficulty
  signal, and what the runtime consumes.
- `docs/ralph/PLAN.md` — F15-B.2 recorded; remainder updated.

## Still open

- `avoidOpponents` order/polarity — retail ≈ universal 0 stays an
  open measurement question (inert meanwhile), not silently consumed.
- Catch-up/rubber-band semantics and AC06's measured difficulty
  effects (F15-B remainder); `weirdPathfinding`/`distancePadding`/
  `cornerBrakingThreshold` consumption once semantics verify.
- The `cir<N>` → `circuit<N>` same-index alias contradiction (WPT-3)
  and UNK-17 grid-slot consumption — unchanged, own slices.
- `london circuit:0` opponent-finisher discrepancy (-1 vs baseline) —
  unchanged dynamics data.
- Amateur-vs-pro pace attribution is confounded by authored vehicle/
  route differences — no controlled retail A/B exists; the synthetic
  production test carries the measured-throttle claim.
