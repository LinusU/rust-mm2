# Last iteration — F10-B.9 junction-interior crossing paths

Iteration 49 on `ralph/night`, continuing from the externally-passed
F04-C.4 momentum-transfer candidate. Operator report 4's item 1 was next
in the report's own order (items 4 and 2 landed earlier): ambient cars
visibly teleport across intersections — the aggregate smoke counters
could never see it, because a teleporting car still spawns, still moves,
and never registers as stuck.

## Root cause (measured, not guessed)

`advance_lane_cursor` consumed the remaining approach-lane distance and
set `cursor.along = 0` on the exit lane, so `sample_lane` placed the car
on the far side of the junction — the interior distance between the
lane extremities was skipped entirely.

While building the `mm2-inspect nav --gaps` census to measure real
junction gaps, a second latent defect surfaced: several SF lane curves
(roads 111–116 and others, mostly `DIVIDED|FREEWAY`-flagged) are
authored **vertex-reversed** relative to their road's section order —
mixed orders within one side, at ordinary in-carriageway offsets, so
storage order is an authoring artefact, not a direction marker. A
reversed lane's travel-direction *end* sat a full road length from its
exit junction: the raw census read a 92 m median / 1643 m max transfer
chord on SF. `NavGraph::build` now normalizes every curve to section
order (`orient_curve`), and the census reads sane junction boxes:
medians ~20–26 m, max 87 m. Cars on those lanes were also traversing
them geometrically backward before this fix.

## What changed

- `mm2_game::nav`: `CrossingPath` + `NavGraph::crossing_path` — a cubic
  Hermite from the approach lane's travel-direction end pose to the
  exit lane's start pose, chord-scaled tangents, resampled to a dense
  polyline; `None` on coincident endpoints so direct transfers stay
  direct. `orient_curve` normalization at build; shared
  `sample_polyline` extraction.
- `mm2_game::traffic`: `LaneCursor.crossing` (`Crossing` = path +
  covered distance + destination); `LaneAdvance` reworked to
  `Along/Entered/Crossing/Landed/DeadEnd`; `advance_lane_cursor`
  commits a crossing only after the gate admits, traverses it
  incrementally inside the same call, and still reports `Entered` when
  one step spans the whole interior.
- `mm2_app::traffic`: crossing traversal in `drive_ambient` (gate skip
  while crossing, turn-speed cap, landing occupancy check,
  `bound_for` keeps the approach junction so occupancy/right-of-way
  survive the crossing); `crossings`/`jumps` counters and a
  pose-continuity watchdog → `crx=`/`jmp=` smoke fields.
- `mm2-inspect nav --gaps`: per-transfer chord/path-length census,
  endpoint-to-centre distances, distribution buckets, worst outliers.
- App test repaired: `a_traffic_light_admits_only_the_green_member_road`
  now keys "crossed" on the crossing *commit* (`crossing.is_some()`),
  not the lane-id change — the landing can fall in a later signal
  phase.

## Verification (this tree)

- `cargo test --locked --workspace` — pass, all suites 0 failures
  (mm2_game traffic 37, mm2_app traffic 28).
- `cargo fmt --all -- --check` — pass.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — pass.
- New tests: path endpoint continuity + monotonic no-jump samples,
  commit-then-land semantics, whole-interior-in-one-step still reports
  the commit, lane-rank preservation, reversed-vertex normalization,
  and app-level `a_lane_follower_crosses_the_junction_interior_without_a_jump`
  (car observed inside the junction box, max per-update move <1 m,
  `crossings>=1`, `jumps=0`).

## Retail evidence (fingerprinted install `fnv1a64:e91e6cd4b2ae30d9`)

- `mm2 --city sf --headless --frames 3000` → `traf=16/16 sp=43 rec=27
  dead=0 uns=0 q=0 jq=3 stuck=0 crx=59 jmp=0`
- `mm2 --city london --headless --frames 3000` → `traf=16/16 sp=22
  rec=6 dead=0 uns=0 q=0 jq=2 stuck=0 crx=68 jmp=0`

59/68 committed crossings on sf/london with zero continuity-watchdog
violations; spawn/recycle/dead-end/queue counters all healthy.
Headless counters prove motion continuity, not rendered feel — no
windowed/GPU playtest this iteration, no original-executable
comparison (retail binary not runnable here).

## Ledger / docs

- `docs/research/bai.md`: vertex-reversed lane-curve quirk documented
  with the measured before/after census numbers; `crossing_path` and
  `nav --gaps` added to the graph/tooling description.
- `docs/original-rules.md`: UNK-12 extended — F10-B.9 runtime note;
  generated crossing geometry classified designed/implementation,
  original interior paths unrecovered.
- `docs/ralph/PLAN.md`: F10-B.9 task row; report 4 item 1 annotated
  implemented/candidate.

## Not done / blockers

- The original's junction-interior geometry/runtime semantics are
  unrecovered (UNK-12) — the Hermite path is a disclosed
  implementation choice, not a verified original rule.
- No rendered playtest; the watchdog counts discontinuities but visual
  smoothness inside the box is unobserved.
- Operator report 4 remaining items: 3 (trees/large poles
  breakability — research first), 5 (startup warn aggregation).
