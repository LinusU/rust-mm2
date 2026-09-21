# Last implementation iteration

- Task ID and title: F10-B.2 review repair — the external review of
  `d055433` (authored junction rules gate lane transfers) found the
  stop-sign FCFS queue deadlocks for exactly the queueing regime it
  exists for: a closed gate's `junction_speed` ramp converges to
  `speed = dist_to_stop / approach_time`, so the driver's
  `ds = min(speed*dt, dist_to_stop)` decays the gap geometrically and
  the f32 cursor asymptotes ~1.1e-4 m short of the line. A car then
  never satisfies `gate()`'s `at_line` (`dist_to_stop <= 0`), never
  registers in the FCFS queue, and waits forever — invisible to
  `jq=`, which used the same `<= 0` test.
- Starting commit: `d055433f488ba55ff60bf2b136ae05d6766b529d` on
  `ralph/night`; tree was clean.

## Root cause

Not a test expectation or missing capability — an implementation
defect in `drive_ambient`'s closed-gate stepping plus an
exact-zero `at_line` definition. Verified independently with an f32
simulation of the per-tick update: resuming from rest at 7/9/14 m of
braking room freezes permanently at `dist_to_stop ≈ 1.14e-4`
(ds ≈ half-ULP of `along ≈ 23.5`, rounds to even); only fast
approaches landed, by clamping onto the line while still moving.

## What changed

- `mm2_game::traffic::JunctionPolicy` — new `stop_line_tolerance`
  (0.1 m, designed like every other constant here): a car that close
  to the stop line counts as standing on it. Documented why "at the
  line" must be a tolerance, not `dist <= 0`.
- `mm2_app::traffic::drive_ambient` — one `at_line` judgement
  (`dist_to_stop <= stop_line_tolerance`) now feeds all three
  consumers the review named: `gate()`'s registration input, the
  `junction_held` counter, and the closed-gate `ds` clamp — inside
  the tolerance the residual is closed outright (bounded by the
  tolerance, never past the line) so the car stands *on* the line
  instead of halving a micrometre gap forever. Open-gate and
  out-of-tolerance stepping are unchanged, so the clamp still never
  lets a closed-gate car past the line.
- `tests/traffic.rs` (mm2_app) — tightened
  `a_stop_sign_serialises_competing_approaches_in_arrival_order`:
  both cars must actually cross (finite tick — `usize::MAX` no
  longer sorts first) and `stood` now requires `along >= 23.0`
  (the line is at 23.5). New regression test
  `a_queued_follower_reaches_the_line_and_takes_its_turn`: the exact
  reported scenario — a follower queued ~7 m behind the stop-sign
  head resumes from rest, must stand at the line, enter the FCFS
  queue after the head departs (`junctions.waiting() >= 1`), and
  take the junction in order. Under the old code B stalls ~1e-4 m
  short and every assertion after `a_crossed` fails.

## Evidence

- f32 simulation of the per-tick update (old vs new rule): resume
  from rest at 7/9/14 m never lands under the old rule (frozen at
  dist ≈ 1.14e-4); under the tolerance all regimes land in bounded
  ticks (528–723 ticks ≈ 4.4–6 s at 120 Hz) and a 15 m/s approach
  still lands via the existing overshoot clamp.
- `cargo test -p mm2_app --test traffic` — 13 pass (+1 regression
  test); `cargo test -p mm2_game --test traffic` — 20 pass.
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets --all-features
  -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, 49 suites, 0 failures.
- Retail headless smoke (real install, this change):
  - `--city sf --frames 600` → `status=pass … traf=16/16 sp=23
    rec=7 dead=0 uns=0 q=0 jq=5`.
  - `--city london --frames 600` → `status=pass … traf=16/16 sp=21
    rec=5 dead=0 uns=0 q=0 jq=3`.
  Held counts match the pre-repair run; the difference is the held
  cars now genuinely stand on the line (registered/queued), which
  only the synthetic queue test can observe end-to-end.

## Still open

- Signal/dwell/stop-line/entry-clearance values remain designed —
  original junction timing unverified (UNK-12); `stop_line_tolerance`
  joins that set.
- A car mid-approach when its phase flips red still freezes wherever
  it stands (possibly just past the line) — bounded, safe, original
  clear-the-box behaviour unverified. Cars landing via the overshoot
  regime still arrive at the line moving ~8–14 m/s and bleed speed
  standing (designed abruptness).
- F10-AC02 remainder: no yielding to crossing traffic inside the
  box; F10-AC04: spawn-vs-spawn overlap unchecked; F10-AC03:
  kinematic followers stop short, no collision fidelity or
  lane-change passing. No rendered/GPU check of junction behaviour
  (headless only); multiplayer union-of-interest bubbles open.
