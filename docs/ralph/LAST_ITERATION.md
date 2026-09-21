# Last implementation iteration

- Task ID and title: review repair (`update_checkpoint_markers`
  ambiguous participant pick) + F15-B.1 — opponent traffic
  avoidance/overtake: live-participant corridor sensing, committed
  pass side, following brake, bounded response to uncompletable
  passes.
- Starting commit: `f4fe9bbe94d253da5da44e70990ff2f239cce6e7`
  (externally checked F15-A.2; branch `ralph/night`).
- Why this slice: the external review's only finding was the marker
  ambiguity — repaired first per policy. For feature work the
  selection policy named F15-B among the ready slices; the
  avoidance/overtake leg is its smallest coherent piece and directly
  advances F15-AC03 (blocked roads → bounded response, not
  indefinite stationary cars). The difficulty/param-tail model
  (UNK-11) stays open — no semantics are invented for it.
- Retail install: `/Users/linus/coding/rust-mm2/retail`
  (`fnv1a64:e91e6cd4b2ae30d9`).

## What changed

- **`race.rs` review repair**: `update_checkpoint_markers` queried
  `(&Player, &RaceProgress)` and took `iter().next()` — correct only
  by archetype-iteration luck once opponents also carried both
  components. It now selects `PlayerControl::Local`, the same
  disambiguation `update_race_warning` already uses; markers
  represent the local driver's view.
- **`opponents.rs` avoidance** (designed controller — no claim of
  original AI behavior, which remains UNK-11):
  - `Traffic`/`Blocker`: every other participant (player included)
    resolved into the driver's frame from live `Position`/`Rotation`/
    `VehicleState` — real physics obstacles, no map data.
  - `nearest_blocker`: nearest car inside a forward corridor
    (`BLOCK_HALF_WIDTH` 2.4 m lane, `reach = 14 m + 1.4·speed`).
  - Pass engagement: a *standing* blocker (`< CRAWL_SPEED` 3 m/s)
    anywhere in the corridor, or a *moving* one only on genuine
    closure (`> FOLLOW_RELEASE` 2 m/s). A matched-pace car is a
    queue to sit in — with a field sharing `.opp` lines, always-on
    offset aims weave the whole field off-line all race (measured:
    first version dropped `london circuit:0` to `opp=0/7`).
  - Commit: `pass_side` ±1 from `pick_pass_side` (open side of an
    offset blocker → route side for a centred one → default), then
    a `PASS_SCAN` room weighting over every nearby car flips it if
    the picked lane is the busier one. Held for the pass so
    alternating blockers cannot flicker it.
  - Aim: `pos + fwd·PASS_LOOKAHEAD + route-lateral·PASS_OFFSET` —
    a short-range point down the offset lane (a shifted distant
    anchor is a ~4° wiggle, not a lane change); the lateral derives
    from the route leg each frame so the offset lane bends with
    the road (a world-fixed vector aimed cars across corners).
  - `held_blocker`: the pass holds across a wider window
    (`PASS_WIDE`) until the blocker is `PASS_BEHIND` behind — no
    cut-back across its nose; `PASS_RELEASE` bounds the linger.
  - `apply_gap_brake`: moving blocker inside the comfort gap →
    soft adaptive-cruise brake even at matched pace (a queue keeps
    its gaps instead of riding bumpers — measured: matched-pace
    bumper-riding drove impacts up); standing blocker → brake only
    on a real approach so crawl-pace steering can still complete
    the drive-around; `PANIC_GAP` → hard brake on active closure.
  - Bounded response (AC03): `PASS_STALL` frames (6 s) without
    `PASS_STALL_DIST` (8 m) of displacement abandons the pass and
    bans that blocker for `PASS_BAN` (5 s) — *fully* transparent
    (no aim, no brake) so the route line can push or slip past;
    the clean pass retries after. Displacement, not speed, is the
    stall signal — recovery shuffles oscillate through 2 m/s and
    reset a velocity check. Static walls/props stay with
    `ScriptedBot`'s existing bounded recovery — no second
    geometry-avoidance system.
- All tuning constants are designed controller values, disclosed as
  such; nothing claims retail-game constants.

## Tests

- `mm2_app/tests/opponents.rs` — 16 total (+5): `pick_pass_side`
  open/route-side/default picks, corridor-only sensing (adjacent
  lane, behind, out-of-reach excluded), gap-brake bands (outside
  gap, hard close, matched-pace station-keeping, moving-blocker
  queue brake, crawl-pace no-brake), `blocked_route_drives_around_
  the_parked_car` — a parked participant on the route is driven
  around through `load_session_world` → `advance_race` with zero
  blocker contacts and all gates cleared — and
  `checkpoint_markers_track_the_local_participant` (review
  regression).

## Commands actually run and results

- `cargo fmt --all -- --check` PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` PASS.
- `cargo test --locked --workspace` — all groups ok, 0 failures.
- `cargo test -p mm2_app --test opponents` — 16/16 ok.
- Retail evidence, both this build and a `f4fe9bb` baseline
  worktree run this iteration (deterministic headless, same
  commands):
  - `sf checkpoint:0 --bot --frames 5400`: `opp=3/6` place 4,
    impacts 195 — baseline `opp=3/6` place 4, impacts 208
    (parity, fewer impacts).
  - `london circuit:0 --bot --frames 14400`: `opp=3/7` place 4,
    impacts 408 — baseline `opp=2/7` place 3, impacts 367. First
    retail circuit opponent evidence at all (AC02's circuit leg
    was a noted gap); three opponents complete 3 laps × 6 gates
    through shared `advance_race` validation.
  - `sf checkpoint:0` hold-driver `--frames 5400` (player becomes
    a mid-course obstacle, full window for opponents): `opp=4/6`
    impacts 165 — baseline `opp=4/6` impacts 232.
  - `london circuit:0` hold-driver `--frames 14400`: `opp=3/7` —
    baseline `opp=4/7`.
  - `london checkpoint:0 --bot --frames 5400`: `opp=1/4` place 2 —
    baseline `opp=3/4`. Confounded the other way here: the baseline
    player never resolved inside the window (`cp=3/5`, full Playing
    time for opponents) while this build's player finished at ~75 s
    and ended their clock; the unresolved three were at c2–c5, not
    stalled.
- Process of getting here matters for the record: the first
  avoidance cut measured `opp=0/7` on the circuit (always-on offset
  aim wove the pack; matched-pace bumper-riding raised impacts).
  The corridor narrowing, moving/standing split, occupancy scan,
  route-relative lane and transparency-ban are what bring the
  numbers above — evidence-tuned, not assumed.

## What this proves / does not prove

- Proves: opponents sense live participants and alter their driving —
  brake for closing traffic, commit and hold a pass side, drive
  around parked cars, queue at matched pace instead of weaving —
  through the same `VehicleInput` path and physics; on retail data
  the field matches or beats the no-avoidance baseline on finishers
  with materially fewer impacts on open courses; circuit opponents
  now have retail completion evidence; a failed pass is bounded
  (stall → transparent push-through window → retry), not an
  indefinite hold.
- Does not prove: exact original AI behavior (designed controller);
  difficulty/param-tail semantics (UNK-11, untouched); that
  avoidance never hurts pace — on the tightest narrow-street
  circuit a mid-pack knot can circulate at crawl for tens of
  seconds inside stall/ban cycles (baseline instead stranded cars
  permanently: honest different failure profile, hold-driver leg
  `3/7` vs `4/7` records the remaining deficit); the `opp=`
  denominator still counts spawned cars, not authored slots.
- Acceptance IDs: F15-AC02 circuit leg now has retail evidence;
  AC03 advanced (drive-around + bounded pass abandonment +
  queue/brake behavior — an observed *recovery* event from a
  wall/prop trap is still unevidenced); AC05 still needs a
  seeded soak matrix; AC06 untouched (difficulty). F15-B stays
  `active` pending external review.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
