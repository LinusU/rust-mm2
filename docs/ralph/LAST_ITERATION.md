# Last implementation iteration

- Task ID and title: F10-B.2 — authored junction rules: the BAI
  `vehicleRule` road-end codes gate lane transfers (traffic-light
  phases, stop-sign FCFS queues, `AlwaysStop`), plus the
  occupied-space check at junction transfer. F10-B.1 passed external
  review with no blocking findings; this slice takes the controller
  leg of F10-AC02 and the junction leg of F10-AC04.
- Starting commit: `fadb615ad6fd04855a99baf288fac700a0865862` on
  `ralph/night`; tree was clean.

## What changed

- `mm2_game::traffic` — new `JunctionPolicy` (designed values — the
  original's signal period, stop dwell, stop-line placement and entry
  clearance are unverified, UNK-12), `JunctionGate`, `Junctions` and
  `junction_speed`. `Junctions::approach` resolves a lane's
  travel-arc exit to `(junction, road, rule)`; `signal_members`
  builds each junction's light-controlled member set from the
  authored counterclockwise road order. The gate: `NeverStop`/
  unruled/dead ends open, `AlwaysStop` never opens, `TrafficLight`
  opens only while the car's road holds the deterministic member
  phase (green + all-red clearance, `PHASE_SPREAD` desync per
  junction), `StopSign` registers a standing car into a per-junction
  FCFS queue and opens once the head's dwell elapsed. `junction_speed`
  brakes a closed-gate car down a `decel`-limited ramp toward the
  stop line and never accelerates. `depart`/`retain` drop cars on
  transfer or despawn so a stale entity never holds a queue.
- `mm2_app::traffic` — `drive_ambient` advances the controller clock
  per tick, sheds queue entries for recycled cars, and per car:
  corridor follow first, then the gate for its lane end. A closed
  gate brakes to the stop line (`stop_inset` 2.5 m before the lane
  end — the nose stays out of the box) and clamps the cursor there;
  an open gate transfers normally. A `Turned` step now samples the
  landing and reverts to the lane end if a live car or participant
  sits within `enter_clearance` (6 m) — cars never materialise
  inside a junction queue. Dead ends, unsampleable lanes and
  non-finite poses all `depart` before despawn; `maintain_ambient`
  departs recycled cars. `AmbientTraffic::junction_held` counts cars
  standing at closed gates; `jq=` joins `q=` in the `traf=` smoke
  record.
- Fixture — the synthetic two-road BAI authors `vehicleRule` codes
  per connected end (`bai_with_rules(r0_end, r1_start)`); the default
  keeps `NeverStop` so existing free-flow tests are unchanged.
- Retail census recorded in `docs/research/bai.md`: approach-end
  rules are SF 449 light / 138 never / 30 stop across 212
  vehicle-approached junctions (82 uniformly lit, 92 mixed, 0
  all-stop) and London 369 / 214 / 23 across 254 (76 lit, 102 mixed)
  — mixed-rule junctions are the norm, so each approach gates on its
  own end's rule. UNK-12's runtime note updated.

## Evidence

- `cargo test -p mm2_game --test traffic` — 20 pass (+4: gate binds
  the authored rule incl. `AlwaysStop`/unconnected, stop-sign FCFS
  admission after dwell, light cycles one member at a time with an
  all-red slice, `junction_speed` ramp never accelerates).
- `cargo test -p mm2_app --test traffic` — 12 pass (+5 through the
  real `load_session_world`: stop-sign serialises competing
  approaches in arrival order with both cars standing at their lines;
  light transfers only on the car's own green; a red-window spawn
  holds at the line and releases on green; `AlwaysStop` never
  releases while the `NeverStop` approach flows; an occupied exit
  lane holds the transfer at the lane end and freeing it releases).
- Retail headless smoke, both cities:
  - `--city sf --frames 600` → `status=pass … traf=16/16 sp=23
    rec=7 dead=0 uns=0 q=0 jq=5` — 5 cars held at authored gates on
    real SF data at the sampled tick.
  - `--city london --frames 600` → `status=pass … traf=16/16 sp=21
    rec=5 dead=0 uns=0 q=0 jq=3` — 3 held on London's left-hand
    network. Fewer respawns than last iteration (sp 26→23 / rec 10→7
    on sf): held cars wait at lights instead of flowing into the
    recycler.
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets --all-features
  -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, 49 suites, 0 failures.

## Still open

- Signal/dwell/stop-line/entry-clearance values are designed —
  original junction timing unverified (UNK-12). Stop-sign FCFS and
  one-road-at-a-time lights follow the *documented* rule semantics;
  whether the original consumes them identically is unverified.
- `TrafficLight` approach ends also author `trafficLightOrigin`/
  `trafficLightAxis` — parsed but not yet rendered as signal props.
- A car mid-approach when its phase flips red freezes wherever it
  stands (possibly just past the line) — bounded, safe, but the
  original's clear-the-box behaviour is unverified.
- F10-AC02 remainder: right-of-way between *vehicles* inside the box
  is occupancy-only — no yielding to crossing traffic, no
  queue-through-intersection priority beyond the landing check.
- F10-AC04 remainder: spawn-vs-spawn overlap still unchecked —
  `draw_spawn` enforces the annulus only.
- F10-AC03 unmet: kinematic followers stop short; no dynamic
  collision fidelity, lane-change passing, or stuck recovery beyond
  the recycle bound.
- No rendered/GPU check of junction behaviour (headless only);
  multiplayer union-of-interest bubbles still open.
