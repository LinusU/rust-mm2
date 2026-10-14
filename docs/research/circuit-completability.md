# Circuit completability audit (F14-AC01 / AC06)

Evidence level: original-content validation (retail install, headless, no
GPU). Not a playtest, and not an original-fidelity comparison: the Ordered
lap model for opponents stays UNK-11 (DSN-45 is designed).

## Method

`scripts/circuit-audit.sh` runs each of the 20 retail circuits (sf and
london, `circuit:0`..`9`) at the Amateur block with its authored roster, a
`--parked` player (so only the opponents race), 36 000 frames = 597 s of
sim. Run 2026-10-14 on `main` at `fd480fc`, debug build. `opp=a/b` is
opponents finished of fielded; the `opps=` columns are in
`crates/mm2_app/src/smoke.rs` (`<slot>:<vehicle>/<cleared>c/<lap>l[/F]`,
then escapes `e`, re-anchors `r`, stuck frames `w`, route clears `d`).

## AC01 — loads with its own lap/route configuration

All 20 load and field their own roster: `mm2-inspect race-defs` shows 20/20
`ok` for both difficulties with per-circuit lap counts (2–4), gate counts
(6–23), slot counts and opponent counts, and every record above had
`status=pass`. No circuit falls back to a shared layout. (Mid-race
behaviour is a separate question, below.)

## AC06 — completability in 597 s of sim

| Result | Circuits |
|---|---|
| every opponent finished | sf 0, 1, 3, 4; london 0, 4, 6 (7/20) |
| some finished, some not | sf 2 (3/6), 5 (6/7), 6 (4/6), 7 (1/6); london 1 (5/6), 2 (2/6), 3 (5/6), 5 (3/4) |
| none finished | sf 8 (0/4), 9 (0/6); london 7, 8, 9 (0/6) |

The task's original list (london 2/5/6/9, sf 7/9) is stale in one place:
london 6 now completes. london 7 and 8 and sf 2/5/6/8 are additional.

Two longer runs (108 000 frames, 1797 s sim) separate "slow" from "stalled":

- sf 2: still 4/6 at 1797 s; slot 2 (`vpcaddie`) sits at 12 of 13 gates in
  lap 3 with 143 escapes and 36 re-anchors, slot 0 (`vpford`) never leaves
  lap 2 start. That is a stall, not a slow lap.
- sf 9: 1/6 at 1797 s with 114 km driven; five opponents are on lap 2 with
  4–13 of 22 gates and 2–6 re-anchors each. Not a plain timeout either.

The other 11 incomplete circuits were also started at 1797 s but were
killed by the 25-minute wall limit before printing a record, so they are
**not** classified as stalled versus slow here.

## What is not yet known

No per-cause diagnosis (route data versus controller) was done for any
incomplete circuit; the counters only show where opponents bunch up (high
re-anchor counts with the `900w` stuck cap). Per-cause tasks are filed
rather than guessed. The re-anchor recovery was not widened.

Not covered: the Professional block, a driving player, the lap model's
original fidelity (UNK-11), and F14-AC02/03/05 (unit-level, tested
elsewhere).
