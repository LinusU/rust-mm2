# Last iteration — F15-B.5: scripted-player authored-route guidance + bounded re-anchor

Repair/feature iteration on `ralph/night` (baseline f79f48f, external
review of F00-C.2 passed). Selected the named remainder that review
surfaced: the scripted `--bot` driver stalled at `cp=2/9` on retail
`sf circuit:0` because it steered gate-to-gate in a straight line and
cut off the elevated road's edge on the descent.

## The defect chain (three layers, peeled in order)

1. **Direct gate aim leaves the road.** From gate 1 the straight line
   to gate 2 cut off the elevated freeway (route at z≈−70, gate aim
   pulled to z≈−52) and the car fell ~20 m under the gate it needed.
2. **Raw-anchor chasing turns too late.** `.opp` anchors sit 40–200 m
   apart; aiming at the next anchor snapped the bearing only after its
   perpendicular plane passed — the car reached the descent still
   flying at 30 m/s and left the ramp transition airborne.
3. **Fall loop + invalid re-anchor landing.** Off the ramp the car
   penned in a hillside pocket, then fell through a collider gap the
   interpolated `.opp` line crosses; the generic recovery respawned it
   at the last dry pose — the same lip — forever (`rcv=206`).

## What changed

- `mm2_app::opponents`: `point_reached`, `route_is_closed`,
  `REANCHOR_DIST`, `REANCHOR_FRAMES`, `SPAWN_LIFT` are now `pub(crate)`
  — the shared route helpers the player bot reuses.
- `mm2_app::scripted`:
  - `ScriptedRoute` — session-owned component on the player carrying the
    borrowed `OpponentRoute`, chase index and the recovery counters.
  - `pick_bot_route` — resolved route staged nearest the player spawn
    (skips unresolved/empty). Implementation choice: retail assigns
    routes to AI opponents, never to the player; this is the evidence
    driver borrowing a wired line as a guide.
  - `route_aim` — chases anchors bounded at the objective's nearest
    route anchor (an off-height car can't run past an uncleared gate
    and chase forever), detects gate-behind on open and closed routes,
    and aims at a 40 m polyline lookahead (`lookahead_aim`) so bends
    are entered before the apex. Gates still bank exclusively through
    the race's swept triggers.
  - `route_distance` — 3-D clearance to the polyline; a car under an
    elevated leg reads off-route.
  - Three bounded re-anchor arms sharing one `ResetVehicle` teleport:
    displacement (900 f without 8 m — penned/beached), off-route
    (300 f beyond 12 m of the line — a fall loop's bubble never fills),
    and recovery-count (≥3 water/OOB `RecoveryEvent`s since the last
    banked gate — `RaceProgress::crossings` is the progress stamp —
    while still off the line). The walk-back's `blocked` closure now
    also raycasts down 10 m (`GROUND_PROBE`) through `SpatialQuery` and
    rejects poses with no collider beneath — the authored line is
    car-height sampling, not a ground promise. `reanchors` discloses
    each assist.
- `mm2_app::session`: `load_session_world` binds `ScriptedRoute` on the
  player when `ScriptedDrive` is present and the roster resolved a
  route.

## Tests

+5 in `crates/mm2_app/tests/bot.rs` (18 total): route-aim
chase/lookahead-bend/objective-cap/gate-behind/closed-wrap/empty,
`route_distance` 3-D, `pick_bot_route` nearest-staging, a routed
detour integration (260 m off the gate line → real `Finished`), the
penned-car re-anchor (banks nothing, resumes to a real finish), and
the recovery-count arm (two falls don't arm it, the third re-anchors
onto the line).

## Verification (this tree)

- `cargo test -p mm2_app --test bot` — 18/18.
- Retail (`fnv1a64:e91e6cd4b2ae30d9`):
  `mm2 --mm2-path <retail> --city sf --event circuit:0 --bot --headless
  --frames 9000` → `status=pass race=Running cp=2/9 lap=1/3 pos=4/5
  opp=0/4 opp_rec=5 rcv=0w/21f/21r wheels=4/4 dropped=0
  final=(-2026,29.8,-51)`. Per-tick trace during development showed
  gates 1→2 banked *on the line* (offd < 5 through both), the descent
  entry grounded, and every re-anchor landing on a supported pose
  (offd ≈ 1 immediately after). Before the slice the same run looped
  `rcv=206` falls through a void to `final y≈−11`.
- Gates (2026-09-23, this tree): `cargo fmt --all -- --check` clean;
  `cargo clippy --workspace --all-targets --all-features -- -D
  warnings` clean; `cargo test --workspace` — 67 suites, 0 failures.

## Not done / open

- The remaining stall is **terrain drivability, not routing**: on the
  `p3→p4` descent (the .opp line cuts across the steep block between
  x≈−1972 and −2155, z −70→+52) the car slides ~15 m south of the
  line's crown into a parallel ditch and grinds at ~1–2 m/s; bounded
  re-anchors (6, disclosed) land on supported line poses but the car
  re-enters the same pocket. The authored `.opp` carries no usable
  brake/speed hints (the `brake` column is the mislabelled heading;
  target speeds all zero). Opponents share the failure (`opp=0/4`,
  `opp_rec=5`) — so the next blocker is steep-hillside drivability
  under our physics/collider import (possibly a terrace/trench the
  retail import produced), an F14-B/F15-B physics or city-collider
  task, not a bot-routing one.
- Re-anchors are bounded and disclosed but can repeat against a
  genuinely undrivable segment — an assist, not a progress guarantee
  (same caveat F15-B.3 recorded for opponents).
- The `--bot` driver remains an evidence tool; no retail analog is
  claimed for route borrowing, lookahead distance, or the
  recovery-count bound.
