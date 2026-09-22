# Last implementation iteration

- Task ID and title: F10-B.5 — junction-box yield (the F10-AC02
  right-of-way remainder called out in F10-B.2's open list: the
  authored signal/FCFS rules decide *whose turn* it is, but nothing
  stopped an admitted approach from driving into a physically
  occupied box). Selected per the selection policy's named F10-B
  remainder "box-yield/right-of-way/collision"; the yield leg is
  this slice, collision fidelity stays open.
- Starting commit: `de09b4765d3db6b5d418f083b473e6e8acc23b74` on
  `ralph/night`; tree was clean, previous external review verdict
  pass (F10-B.4), so this is feature work, not a repair.

## What changed

- `mm2_game::traffic` — `JunctionPolicy` gains `box_margin` 3 m
  (padding past the farthest member end so a car that just turned
  keeps occupying until its hull is clear) and `box_max_rise` 3 m
  (an overpass does not occupy). Both designed values — the
  original's box geometry and yield behaviour are unverified
  (UNK-12).
- New `junction_zone` derives an occupancy zone from the authored
  intersection: centre = authored `center` (endpoint centroid when
  that is non-finite), XZ radius = farthest member road's end at
  the junction + `box_margin`. `inside_junction_zone` tests XZ
  radius plus the vertical band.
- `Junctions::gate` takes `box_occupied` and closes an otherwise-
  admitted approach while it holds — the green member of a traffic
  light and the FCFS head of a stop-sign queue both yield. Only the
  two paths where a gated rule can open consult it: `NeverStop`/
  unruled ends keep their documented free flow, `AlwaysStop` was
  closed before and stays closed.
- `mm2_app::traffic` — `drive_ambient` pre-passes a `bound_for`
  map (entity → the junction its lane approaches), then for each
  gated approach reports the zone occupied iff a blocker (every
  ambient car + every `Player` participant — the same snapshot the
  corridor sense already builds) sits inside it and is *not* bound
  for the same junction. That exclusion is the deadlock guard: a
  car waiting at its own stop line can never hold the box, so two
  competing approaches cannot freeze each other. Participants carry
  no junction binding, so a parked player or AI opponent always
  counts.

## Evidence

- `cargo test -p mm2_game --test traffic` — 28 pass (+3:
  `junction_zone_spans_member_ends_with_margin_and_rise` — authored
  centre, offset-centre radius, non-finite-centre centroid
  fallback, vertical rejection, out-of-range junction;
  `a_green_member_yields_while_the_box_is_occupied` — closed while
  occupied, open on clear; `a_stop_sign_head_yields_but_a_never_
  stop_flows` — the FCFS head past its dwell still yields,
  `NeverStop` opens regardless, `AlwaysStop` stays closed). The 19
  pre-existing `gate` callsites gained the sixth argument as
  `false` — semantics unchanged.
- `cargo test -p mm2_app --test traffic` — 19 pass (+2:
  `a_participant_in_the_box_yields_the_green_approach` — a `Player`
  parked inside the zone but outside the corridor/landing checks
  holds a lit approach at its stop line through a green, reports
  `junction_held`, and the car goes after despawn;
  `an_ambient_car_in_the_box_yields_the_stop_sign_head` — a parked
  ambient car bound for a dead end holds the admitted stop-sign
  head at its line, release on despawn).
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets --all-features
  -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, 49 suites, 0 failures.
- Retail headless smoke (install `fnv1a64:e91e6cd4b2ae30d9`):
  - `--city sf --frames 600` → `status=pass … traf=16/16 sp=23
    rec=7 dead=0 uns=0 q=0 jq=5 stuck=0` — record unchanged.
  - `--city london --frames 600` → `status=pass … traf=16/16 sp=20
    rec=4 dead=0 uns=0 q=0 jq=4 stuck=0` — small counter drift vs
    the F10-B.4 record (`sp=21 rec=5 jq=3`): one approach now
    stands at its line an extra green, shifting the recycle count.
    Expected behaviour change, all 16 cars live, `stuck=0`.

## Still open

- Box geometry and yield policy are designed — original
  right-of-way/box dimensions unverified (UNK-12). Occupancy reads
  entity position points, not hulls; static props/world geometry
  are not occupancy inputs.
- Same-tick collision behaviour is approximate — a car already
  committed to a turn is not swept out mid-transfer, and two
  simultaneously released approaches can still converge inside the
  box. F10-AC03 (player-collision fidelity, no lane-change
  passing) stays open.
- F10-AC02 remainder now narrows to queue-through-intersection
  priority fidelity; AC05's fixed-seed soak still owes the new
  behaviour; F10-C multiplayer union-of-interest, signal-prop
  rendering, rendered/GPU junction checks, and original
  signal/spawn timing all open.
