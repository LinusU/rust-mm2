# Last implementation iteration

- Task ID and title: F15-A.3 — authored start headings: `.opp` row-0
  staging heading + `_strtpnts` `a` measured as vehicle yaw; player
  spawn-yaw defect repaired; opponents face their authored heading;
  the initial route chase skips anchors behind the staged facing.
- Starting commit: `09e2b0045d00ca861f37cb322c267f425246278b`
  (externally checked F15-B.1; branch `ralph/night`).
- Why this slice: the F15-A spawn/drive leg left facing provisional
  (UNK-16/17). Measuring all 612 retail `.opp` files against the
  `cir*_strtpnts` grids resolved the convention split — and exposed a
  live defect: `session.rs` read the authored slot angle as a bearing
  and added 180°, so the player spawned facing backward on every
  authored SF circuit grid. One coherent repair covers player spawn,
  opponent facing and the first chase target.
- Retail install: `/Users/linus/coding/rust-mm2/retail`
  (`fnv1a64:e91e6cd4b2ae30d9`).

## What the measurement established

- The `.opp` `brake` header misleads: a nonzero value marks a *staging
  record* whose payload is a heading in degrees — row 0 carries it on
  592/612 retail files; 542 of those agree with the route's course
  direction within ~25°; grid events share one value across their
  routes (`circuit0-*` all read 175.0). `race/sf/race5-a-{5,6,7}` carry
  a second staging row mid-file — what re-stages there is still open.
  Every other `.opp` column authors 0 on retail.
- `_strtpnts` `a` is the same convention: vehicle yaw — forward
  `(−sin a, −cos a)` in XZ, exactly what `Quat::from_rotation_y`
  produces for local −Z forward (`cir1` ≈ +92° faces the −X course).
- The waypoint `a` column is a *course bearing* (`atan2(dx,dz)` along
  the rows) — exactly 180° apart, which is the UNK-16 split, now
  measured rather than suspected. `MIRROR_Z` is `false` — authored
  space is runtime space, no transform accounts for the difference.
- A staged start is not necessarily on the driving line:
  `circuit1-a-0`'s heading runs −X while its row-1 anchor sits +X of
  the spawn — the `.opp` line is a loop the staged start joins mid-leg.

## What changed

- `mm2_game::race::RaceStart.yaw_deg` is now defined as vehicle-yaw
  degrees (doc spell-out; consumers were the bug).
- `mm2_content::race_def`: the no-grid fallback derives
  `atan2(−dx,−dz)` (was `atan2(dx,dz)` — the opposite bearing);
  `_strtpnts` keeps authored `a` verbatim, now documented as yaw.
- `mm2_app::session`: `spawn.yaw = slot.yaw_deg.to_radians()` — the
  old code re-derived a bearing and added 180°, spawning the player
  backward on `_strtpnts` grids.
- `mm2_game::OpponentRoute::start_heading_deg()` returns row-0's
  nonzero staged heading (raw `brake` preserved);
  `OpponentRoutePoint::brake`/`mm2_formats::opp` docs corrected —
  the field is not a speed.
- `mm2_app::opponents::spawn_pose` faces by position source: authored
  grid slot → its `yaw_deg`; route anchor → staged heading when
  authored, else the previous first-leg facing; designed stagger →
  the player's yaw (unchanged).
- `initial_route_index` starts `OpponentDriver.next` at the first
  route anchor ahead of the staged facing and not already reached —
  chasing a behind anchor U-turns the car off its authored heading
  (`circuit1-a-0` shape). Anchors left behind are picked up by the
  closed-route wrap like every other passed point.
- Remaining provisional policies (UNK-17, unchanged): `_strtpnts`
  row 0 = player slot, opponents take `index + 1` or the route's
  staged point; which authored start set the original consumes per
  participant is still unverified.

## Tests

- `mm2_app/tests/opponents.rs` — 19 total (+3):
  `spawn_pose_faces_the_authored_staging_heading` (a 90° staging
  heading beats the +X first leg — discriminates authored facing from
  the old fallback), `initial_chase_index_skips_anchors_behind_the_
  staging` (both directions), `authored_staging_heading_faces_the_
  spawn` (production path: authored −X facing reaches the entity's
  rotation, `driver.next` lands past the behind tail, no drive demand
  reverses it). `spawn_pose_prefers_…` now asserts the slot's authored
  yaw (was the route-leg facing); restart asserts the computed first
  chase index.
- `mm2_app/tests/event.rs` — 12 total (+1):
  `authored_strtpnts_yaw_faces_the_player_spawn` — a retail-style
  90° `_strtpnts` grid puts the player's forward at −X verbatim
  through `load_session_world` (the regression: the old code faced
  +X, backward).
- `mm2_content/tests/race_def.rs` — the derived fallback asserts
  vehicle-yaw convention (`−cos a` down-course); the `_strtpnts` test
  now asserts authored 92° faces −X.

## Commands actually run and results

- `cargo fmt --all -- --check` PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` PASS.
- `cargo test --locked --workspace` — all groups ok, 0 failures
  (38 groups).
- `cargo test -p mm2_app --test opponents --test event -p mm2_content
  --test race_def` — all ok during development.
- Retail evidence — `fnv1a64:e91e6cd4b2ae30d9`, deterministic headless,
  with a same-iteration `09e2b00` baseline worktree (`/tmp`,
  since removed) run for direct comparison:
  - `sf circuit:1 --headless --frames 900` (hold driver — straight-line
    facing probe): final `(-507,·,-52)` from the `cir1_strtpnts` slot
    `(-489,·,-55)` yaw 92.4° — the car drives **−X, the authored
    facing/course direction**. Baseline same command: `(-485,·,-55)` —
    +X, backward off the grid, run this iteration on the old binary.
  - `sf circuit:1 --bot --frames 14400`: `opp=0/7` `cp=4/10` lap 1/3,
    impacts 1049 — baseline `opp=0/7` `cp=4/10` impacts 1075,
    `final` within 2 m. Parity: a 3-lap × 10-gate course the scripted
    driver cannot finish in 240 s — not a stall, and identical on the
    old build.
  - `sf checkpoint:0 --bot --frames 5400`: `opp=6/6` impacts 233 —
    baseline `opp=3/6` impacts 195. All six opponents finish through
    shared `advance_race` validation. Both builds end `status=fail`
    "fell through the world" — the scripted player drives off-course
    after the race resolves; a `--bot` limit on both builds.
  - `london circuit:0 --bot --frames 14400`: `opp=2/7` impacts 576,
    player unresolved at lap 3 — baseline `opp=3/7` impacts 408,
    player finished place 4. One fewer finisher through the
    authored-heading launch (the staged heading aims the pack down
    the p1→p2 leg immediately instead of the old south detour +
    U-turn); disclosed honestly — equal to the pre-F15-B.1 `2/7`.
- Evidence classification: code gates + synthetic integration tests +
  deterministic headless retail runs. No GPU/rendered/audio evidence
  this iteration; no original-executable comparison exists.

## Ledger / research updates

- `docs/original-rules.md`: WPT-4 rewritten — both `a` conventions
  measured (waypoint bearing vs start/staging vehicle yaw, 180°
  apart); UNK-16 narrowed to gate-direction enforcement; UNK-11's
  "all `.opp` columns author zero" misstatement corrected — `brake`
  measured as the staging heading; UNK-17 narrowed to the
  participant↔slot mapping question.
- `docs/research/aimap.md`: `.opp` record section added with the
  612-file measurement (staging rows, shared grid headings,
  mid-leg joins, `race5` second staging row).
- `docs/ralph/PLAN.md`: F15-A.3 row added under F15-A.

## Still open

- What triggers `race/sf/race5-a-{5,6,7}`'s mid-file staging row.
- Whether the original consumes `.opp` staged poses vs `_strtpnts`
  slots per participant (UNK-17), and whether waypoint `a` enforces
  gate direction at runtime (UNK-16 remainder).
- `london circuit:0` opponent parity (-1 finisher vs baseline) — race
  dynamics, not a verified regression; a representative seeded matrix
  remains an F15-B open item.
- F15-B remainder: difficulty/param-tail model (UNK-11), catch-up,
  AC06 measured difficulty effects.
