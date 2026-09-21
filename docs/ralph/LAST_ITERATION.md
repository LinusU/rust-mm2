# Last implementation iteration

- Task ID and title: F15-A.3 review repair — an authored `_strtpnts`
  `a = 0` is *no heading* (the `.opp` `brake == 0` convention applied
  to start grids): `RaceStart.yaw_deg` → `Option<f32>`, and a `None`
  slot derives a course facing instead of spawning backward.
- Starting commit: `36b05304816083af2cf9ee65f37355eb582417b1` on
  `ralph/night` — the iteration-10 candidate the external review
  rejected on one blocking finding.
- Retail install: `/Users/linus/coding/rust-mm2/retail`
  (`fnv1a64:e91e6cd4b2ae30d9`).

## The blocker being repaired

External review (iteration-10 feedback, `review.verdict: fail`):
`cir6_strtpnts` authors `a = 0` on all three rows — the lone all-zero
grid on retail — yet `authored_start_slots` kept it verbatim, so
`session.rs`/`spawn_pose` spawned the player and the two grid-slot
opponents at yaw 0 = −Z. The `.opp` routes bound to `circuit:6` stage
~177–183° (vehicle-yaw +Z) and the driving line passes within ~6 m of
the grid heading +Z; the reviewer's live run had the hold-driver go
`z=-402 → z=-483`, 81 m backward into the start-line area, and the
grid-slot opponents U-turned at green. The reviewer's own convention
already treats `.opp brake == 0` as "no authored heading"; the same
rule was missing on the `_strtpnts` side.

## Root cause

`authored_start_slots` mapped `p.angle_deg` verbatim into
`RaceStart.yaw_deg: f32` — a scalar contract with no way to say
"nothing authored". Every nonzero grid on retail (8/9 files) carries
a real heading, so the convention fix was correct for them; the zero
column needed the same zero-means-unset reading `start_heading_deg`
applies to `.opp` row-0 `brake`.

## What changed

- `mm2_game::race::RaceStart.yaw_deg` is now `Option<f32>` — `Some`
  is an authored (or producer-derived) vehicle-yaw heading, `None`
  means the record supplied none. Doc updated with the measured
  `cir6` case.
- `mm2_game::race::RaceDefinition::course_yaw(from)` — new shared
  helper: the course's opening direction as a vehicle-yaw radians
  heading toward the first trigger ≥2 m away in XZ (the same facing
  the no-grid producer fallback derives from the row0→row1 tangent —
  `checkpoints[0]` is its far end under both rules). `None` on
  degenerate data.
- `mm2_content::race_def::authored_start_slots` maps `a = 0` →
  `yaw_deg: None`; nonzero stays verbatim; the designed no-grid
  fallback slot stores `Some`.
- `mm2_app::session` player spawn: `yaw_deg` → `course_yaw` → the
  world roam yaw, in that order — matching the reviewer's suggested
  "derived/course facing" leg.
- `mm2_app::opponents::spawn_pose` restructured so a `None` grid slot
  falls through to the same chain a route-anchor spawn uses: route
  staged heading → first-leg direction → player's yaw — the
  reviewer's suggested opponent leg. Position still follows the slot.
- `mm2_formats::waypoints::StartPoint` doc notes authored 0 = no
  heading (parser keeps raw, as before).

## Tests

- `mm2_content/tests/race_def.rs` — `zero_strtpnts_angle_is_no_
  authored_heading`: a cir6-shaped all-zero grid produces `None`
  slots while a nonzero row in the same file still binds verbatim.
  Existing strtpnts/tangent tests updated for `Option` (15 total).
- `mm2_app/tests/event.rs` — `zero_strtpnts_yaw_falls_back_to_the_
  course` (production path): an all-zero grid on a +X course spawns
  the player on the authored slot facing +X toward gate 0 — the old
  code faced −Z (13 total).
- `mm2_app/tests/opponents.rs` — `spawn_pose_on_a_headless_grid_slot_
  uses_the_route_facing`: `None` slot keeps its position and takes
  the route's staged heading; no staged heading → first leg; no route
  → player's yaw (20 total).
- `mm2_game/tests/race.rs` — `course_yaw_faces_the_first_real_
  trigger`: first-trigger facing, on-slot trigger skip, degenerate →
  `None` (21 total).

## Commands actually run and results

- `cargo fmt --all -- --check` PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` PASS.
- `cargo test --locked --workspace` — all 38 groups ok, 0 failures.
- Retail evidence — `fnv1a64:e91e6cd4b2ae30d9`, deterministic
  headless (`./target/debug/mm2 --mm2-path …/retail`):
  - `sf circuit:6 --headless --frames 900` (hold driver — the
    reviewer's reproduction command): spawn `(-1478,·,-402)` →
    `final=(-1646,38.6,-393)`, `cp=1/9`, `pos=1/7` — drives down-course
    −X through gate 0 toward the next gate. The rejected candidate
    drove `z=-402 → -483` (81 m backward, −Z); the course direction is
    now correct.
  - `sf circuit:6 --headless --bot --frames 5400`: `pos=2/7`,
    `final=(-1759,·,-307)` — the scripted player cleared gate 0 and is
    chasing gate 1 with one opponent ahead; the field makes progress
    instead of U-turning off the grid. (No pre-fix `circuit:6` bot
    baseline exists — the reviewer measured the defect live.)
  - `sf circuit:1 --headless --frames 900` (regression check):
    `final=(-507,18.9,-52)` — bit-identical to the verified
    iteration-10 record; the nonzero-heading path is untouched.
  - `mm2-inspect dump race/sf/cir6_strtpnts` confirms all three rows
    author `a=0.0`; `circuit6-a-0.opp` row 0 stages at `(-1474,-474)`
    heading `177.0` and drives +Z through the grid before turning −X.
- Evidence classification: code gates + synthetic integration tests +
  deterministic headless retail runs. No GPU/rendered/audio evidence;
  no original-executable comparison exists.

## Ledger / research updates

- `docs/original-rules.md` WPT-4: the `a = 0` unset rule added to the
  measured conventions (`Option` contract + per-consumer fallbacks).
- `docs/research/aimap.md`: `_strtpnts` side of the zero-means-unset
  convention recorded with the `cir6` measurements.
- `docs/ralph/PLAN.md`: F15-A.3 row gains the review-repair note.

## Still open

- The `cir<N>` → `circuit<N>` same-index alias (WPT-3, pre-existing):
  the review's positional+heading matching contradicts it for several
  grids (cir2↔circuit0, cir4↔circuit5, cir5↔circuit8, cir8↔circuit4;
  cir9 ambiguous). Wrong-event grid *positions* were already consumed
  before F15-A.3; the alias needs its own measurement/ledger update —
  left open as its own slice, not silently repaired here.
- What triggers `race/sf/race5-a-{5,6,7}`'s mid-file staging row.
- Whether the original consumes `.opp` staged poses vs `_strtpnts`
  slots per participant (UNK-17), and whether waypoint `a` enforces
  gate direction at runtime (UNK-16 remainder).
- `london circuit:0` opponent parity (-1 finisher vs baseline) — race
  dynamics, not a verified regression; a representative seeded matrix
  remains an F15-B open item.
- F15-B remainder: difficulty/param-tail model (UNK-11), catch-up,
  AC06 measured difficulty effects.
