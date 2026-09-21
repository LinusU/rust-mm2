# Last implementation iteration

- Task ID and title: F15-B.3 — bounded opponent re-anchor recovery:
  the disclosed last-resort teleport for opponents the escape/pass
  machinery cannot free (F15-AC03's "blocked indefinitely" leg and
  AC04's "no checkpoints from teleports" leg).
- Starting commit: `a203f83304908ab4d5c5d204a65a72eea68a1e98` on
  `ralph/night` — the iteration-12 candidate the external review
  passed (F15-B.2).
- Retail install: `/Users/linus/coding/rust-mm2/retail`
  (`fnv1a64:e91e6cd4b2ae30d9`).

## What changed

- `mm2_app::opponents` — `OpponentDriver` gains `stuck_pos`,
  `stuck_frames`, `reanchors`. While a route target exists the
  authority counts frames spent within `REANCHOR_DIST` (8 m) of the
  window anchor — **displacement, not grounded speed**: a penned car
  can roll forever inside its bubble, and an ungrounded or
  hull-beached car never reaches `scripted_input`'s grounded-gated
  stuck counter at all, so both classes sat forever before this
  slice. At `REANCHOR_FRAMES` (900 ≈ 15 s at 60 Hz — the
  reverse-and-turn escapes and the pass stall/ban cycle get their
  turns first) the driver writes a `ResetVehicle` for itself.
- The teleport is the production path, not a second mechanism:
  `vehicle_reset` writes position/upright yaw/zero motion and marks
  the entity `Teleported`, and `reanchor_teleported_participants`
  (FixedLast, before `advance_race`) breaks the swept segment — so
  the jump cannot bank a checkpoint crossing (AC04).
- `reanchor_pose` picks the landing: project the car onto the route
  leg it was chasing (`driver.next`, closed-route wrap leg for
  `next == 0`, open-route leg 0 approach), walk *backward* along the
  authored polyline `REANCHOR_BACK` (4 m) for clearance and further
  in the same stride while the candidate sits inside a trigger the
  participant has not cleared (built from `RaceProgress::remaining`
  over the live `RaceDefinition`), bounded by `REANCHOR_WALK` (60 m)
  and the open-route start. The landing is upright, faces down-leg,
  gets the spawn's hull clearance, and `driver.next` recomputes via
  `initial_route_index`; recovery/pass/stall/stuck state resets.
- Observable and scoped: `info!` logs each assist with vehicle id +
  running count, `OpponentDriver::reanchors` accumulates them, and
  `smoke.rs` appends `opp_rec=<n>` to the existing `opp=` record only
  when at least one fired (records without re-anchors stay
  byte-identical). Authority-only — a predicted client's
  `opponent_drive` resets the window instead of teleporting. Ledger
  entry **DSN-14** — a designed anti-standstill policy, no verified
  original recovery rule exists.

## Tests (`tests/opponents.rs` 24 → 31)

- `reanchor_pose_projects_onto_the_chased_leg` — projection + 4 m
  step-back + down-leg facing.
- `reanchor_pose_walks_back_out_of_uncleared_triggers` — a landing
  inside a pending gate keeps walking across a leg boundary until
  clear; a *cleared* gate does not extend the walk.
- `reanchor_pose_wraps_closed_and_clamps_open` — `next == 0` on a
  closed route walks the wrap leg; an open route clamps at its start
  even when every candidate is blocked.
- `reanchor_pose_handles_degenerate_routes` — empty/single-point.
- `permanently_stuck_opponent_reanchors_and_resumes` — end to end:
  `vpt` is walled into an off-lane pocket during the countdown via
  the `Teleported` contract itself; zero gates banked while penned,
  the bounded window fires, the car lands on the route clear of all
  pending triggers (cleared stays 0), upright, marker consumed — then
  re-drives the course to a real `Finished` through `advance_race`.
- `reanchor_dispatches_through_the_production_reset_path` — a spent
  budget fires exactly once, resets pass/recovery state, restarts the
  window.
- `progressing_opponents_never_reanchor` — 1200 updates of free
  driving, `reanchors == 0` for the whole field.

## Commands actually run and results

- `cargo fmt --all -- --check` PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` PASS.
- `cargo test --locked --workspace` — all groups ok, 0 failures
  (opponents 31, race 33+21, bot 7, session 13, event 13, vehicle 21
  + drive 15 + surface 8, banger 17, contracts 6, import 10, nav 7,
  mm2_game opponent 4, mm2_inspect 5, mm2_app lib 25).
- Retail evidence — `fnv1a64:e91e6cd4b2ae30d9`, deterministic
  headless (`./target/debug/mm2 --mm2-path …/retail`):
  - `sf circuit:1 --headless --frames 900` (hold driver):
    `final=(-507,18.9,-52)` `opp=0/7`, no `opp_rec` — bit-identical
    to the verified record; the dormant path changes nothing.
  - `sf checkpoint:0 --headless --frames 5400`: `opp=5/6 opp_rec=2`
    — `vpbug` re-anchored twice mid-race (each logged) and the field
    resolved one more car than the F15-B.2 baseline's `opp=4/6`
    (dynamics data, not a controlled A/B). `status=fail "fell
    through the world"` is the documented pre-existing hold-driver
    artifact (PLAN baseline table — the idle car ends >25 m below
    spawn altitude), unchanged by this slice.
  - `sf circuit:1 --headless --bot --frames 14400`:
    `opp=0/7 opp_rec=15` — the assist fired 15 times across the field
    (`vpbug` ×5, `vpcoop`, …) on genuinely stationary cars, each
    ~15 s after its last 8 m of progress; no opponent finishes 3 laps
    inside the 240 s cap either way. Player bot reached `cp=9/10`
    (baseline `cp=4/10` — the field no longer parks as obstacles on
    the line; dynamics data).
- Evidence classification: code gates + synthetic production tests +
  deterministic headless retail runs. No GPU/rendered/audio evidence;
  no original-executable comparison — whether retail opponents
  re-anchor at all is unmeasured (DSN-14 is a designed policy).

## Ledger / research updates

- `docs/original-rules.md` — **DSN-14** new (designed departure:
  bounded opponent re-anchor, disclosed + counted + smoke-surfaced).
- `docs/ralph/PLAN.md` — F15-B.2 row added (was narrative-only;
  external-review bookkeeping nit), F15-B.3 recorded, F15-B parent's
  Remaining updated.

## Still open

- Catch-up/rubber-band semantics — unimplemented, needs an explicit
  rules decision before any assistance is designed (F15 spec req 9).
- AC06 measured difficulty effects — still confounded by authored
  vehicle/route differences; no controlled retail A/B exists.
- `avoidOpponents` polarity; `weirdPathfinding`/`distancePadding`/
  `cornerBrakingThreshold` consumption once semantics verify (UNK-11).
- Re-anchoring into the same difficult pocket repeats every ~15 s —
  bounded and disclosed, not a progress guarantee; field pace on the
  tightest circuits stays an F15-B/F15-C open item.
- AC05 fixed-seed soak of finish/DNF/stuck outcomes — `reanchors`
  is now the recorded per-car recovery channel.
- `cir<N>` alias (WPT-3), UNK-17 grid-slot mapping — own slices.
