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

## Re-measure after the neural opponent driver (#1400)

The audit above was taken against the old scripted controller. After the
neural driver landed (`docs/neural-opponents.md`), sf circuit:2/5/6/7 were
re-run on main with the sweep's flags (`--headless --parked --no-profile
--until-opponents --frames 43200`, release build, Amateur and `--pro`):

| event | diff | finished | re-anchors | slowest finisher |
|---|---|---|---|---|
| sf circuit:2 | Amateur | 6/6 | 1 | 324 s |
| sf circuit:2 | Pro | 6/6 | 4 | 262 s |
| sf circuit:5 | Amateur | 7/7 | 0 | 135 s |
| sf circuit:5 | Pro | 7/7 | 4 | 201 s |
| sf circuit:6 | Amateur | 6/6 | 15 | 321 s |
| sf circuit:6 | Pro | 6/6 | 70 | 517 s |
| sf circuit:7 | Amateur | 6/6 | 10 | 268 s |
| sf circuit:7 | Pro | 6/6 | 21 | 343 s |

Every opponent completes every gate, so the earlier stalls were the old
controller, not the `.opp` route data. No code change. Residual signal for
the owner (not diagnosed here, policy side): sf circuit:6 has the most
re-anchors (70 on Pro, the `vpdb7` slots at 12-21 each and finishing
last). The
re-anchor recovery was not widened.

## Re-measure of sf circuit:8/9 after the neural driver (#1401)

The same sweep flags as above (`--headless --parked --no-profile
--until-opponents --frames 43200`, release build, Amateur and `--pro`):

| event | diff | finished | re-anchors | slowest finisher |
|---|---|---|---|---|
| sf circuit:8 | Amateur | 4/4 | 3 | 277 s |
| sf circuit:8 | Pro | 4/4 | 0 | 193 s |
| sf circuit:9 | Amateur | 6/6 | 0 | 248 s |
| sf circuit:9 | Pro | 6/6 | 4 | 247 s |

Every opponent finishes on both circuits, so the zero-finish stalls were
the old scripted controller, not
the `.opp` route data or the lap length. No code change. The re-anchor
recovery was not widened. Evidence level: synthetic integration (headless,
retail data), no rendered check.
