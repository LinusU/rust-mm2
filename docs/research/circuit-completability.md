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

## Re-measure of london circuit:7/8/9 after the neural driver (#1403)

The same sweep flags (`--headless --parked --no-profile --until-opponents
--frames 43200`, release build, Amateur and `--pro`):

| event | diff | finished | re-anchors | slowest finisher |
|---|---|---|---|---|
| london circuit:7 | Amateur | 6/6 | 47 | 652 s |
| london circuit:7 | Pro | 6/6 | 40 | 583 s |
| london circuit:8 | Amateur | 6/6 | 45 | 561 s |
| london circuit:8 | Pro | 5/6 at 717 s | 48 | 806 s (slot 4) |
| london circuit:9 | Amateur | 6/6 | 1 | 497 s |
| london circuit:9 | Pro | 6/6 | 6 | 294 s |

The old zero-finish stalls are gone: the `.opp` route data and the lap
length are not the cause. The one miss, london circuit:8 Pro, is the
12-minute frame cap, not a stall: slot 4 (`vppanozgt`, 16 re-anchors) was at
7 of 12 checkpoints at 717 s; re-run to 86400 frames (deterministic, same
other results) it finishes at 806 s. Residual signal for the owner (policy
side, not diagnosed here): london circuits 7 and 8 have 40-48 re-anchors
per race, concentrated in the `vpdb7`/`vppanozgt` slots. No code change; the
re-anchor recovery was not widened. Evidence level: synthetic integration
(headless, retail data), no rendered check.

## Re-measure of the remaining London circuits after the neural driver (#1402)

The earlier re-measures in this file cover every sf circuit the audit left
incomplete — sf circuit:2/5/6/7 (#1400) and sf circuit:8/9 (#1401). #1402
named the remaining incomplete London circuits, london circuit:1/2/3/5 (5/6,
2/6, 5/6 and 3/4 finished in the table above), so all ten London circuits
were re-measured on `main` at `af2f87b`, release build, with the sweep's
flags (`--headless --parked --no-profile --until-opponents --frames 43200`,
i.e. up to 720 s of sim), both difficulties. Eight of the ten are listed
below; the same sweep also re-ran 7/8/9 as an independent cross-check of
#1403:

| event | diff | finished | re-anchors | slowest finisher |
|---|---|---:|---:|---:|
| london circuit:0 | Amateur | 7/7 | 0 | 111.4 s |
| london circuit:0 | Pro | 7/7 | 0 | 76.2 s |
| london circuit:1 | Amateur | 6/6 | 1 | 169.9 s |
| london circuit:1 | Pro | 6/6 | 0 | 114.0 s |
| london circuit:2 | Amateur | 6/6 | 8 | 354.1 s |
| london circuit:2 | Pro | 6/6 | 30 | 443.5 s |
| london circuit:3 | Amateur | 6/6 | 5 | 278.9 s |
| london circuit:3 | Pro | 6/6 | 6 | 293.6 s |
| london circuit:4 | Amateur | 5/5 | 2 | 209.7 s |
| london circuit:4 | Pro | 5/5 | 36 | 443.7 s |
| london circuit:5 | Amateur | 4/4 | 0 | 218.2 s |
| london circuit:5 | Pro | 4/4 | 9 | 234.0 s |
| london circuit:6 | Amateur | 6/6 | 2 | 291.0 s |
| london circuit:6 | Pro | 7/7 | 15 | 434.9 s |
| london circuit:9 | Amateur | 6/6 | 1 | 496.9 s |
| london circuit:9 | Pro | 6/6 | 6 | 294.3 s |

The same run covered circuit:7/8/9 as an independent cross-check and agrees
with #1403 to within a second: 7 finishes 6/6 and 6/6, 8 finishes 6/6 and 5/6
(slot 4 `vppanozgt` at 7 of 12 gates when the 720 s cap hits; #1403 shows it
finishes at 806 s), 9 finishes 6/6 and 6/6.

Every opponent of all four circuits #1402 named now finishes: london
circuit:1 6/6 at both difficulties, circuit:2 6/6 and 6/6, circuit:3 6/6 and
6/6, circuit:5 4/4 and 4/4, all inside 597 s of sim (the slowest is circuit:2
Professional at 443.5 s). The circuit:0/4/6 rows stay complete and the London
rows above for circuit:7/8/9 are stale.

### Route data versus controller

`mm2_gym audit` is the route-defect screen: it walks every authored line and
reports points it cannot be driven as drawn. Run for both cities on the same
build, it names **no circuit line** — sf's entries are `race4/8/11` and
London's are `race6`, `race8` and `circuit8` only. London circuit:1/2/3/5 have
no entry: their `.opp` lines are drivable as drawn, so their earlier
partial-finish stalls were the old scripted controller, not route data. That
matches the sf conclusion in #1400/#1401. No code change and no route-data fix
were needed for the four circuits this task named; the re-anchor recovery was
not widened.

### Residual signal (policy side, not diagnosed here)

Nothing in the London circuit catalog fails to complete: circuit:8
Professional is the only miss at the 720 s cap and #1403 shows it is slowness
(slot 4 finishes at 806 s when given 86400 frames), not a stall. Two things
are left for the owner. The re-anchor counts are high on circuits 7-8 (40-48 a
race, concentrated in the `vpdb7`/`vppanozgt` slots). And `circuit8` is the one
London circuit whose lines the route-defect screen flags — all six at the same
place, a `wall` at arc 1572 (`at [127.9, -0.1, -238.5]`). Since every
circuit:8 car completes laps, that entry is not a hard block; it is either a
barrier the line can go around or a screen false positive, and is left as a
note rather than a fix. The 12-minute result also disagrees with the aggregate
44/44 race table in [neural-opponents.md](../neural-opponents.md) for
circuit:8 Professional on current `main`, which is the same slowness #1403
established.

## Professional block, time-limit-aware run (#1404, F14-AC06)

`scripts/circuit-audit.sh` now takes a difficulty (`amateur`/`pro`), a
prebuilt binary (`MM2_BIN`) and extra flags (`AUDIT_ARGS`). All 20 circuits
were run on `main` (release build) as
`MM2_BIN=target/release/mm2 AUDIT_ARGS="--no-profile --until-opponents"
scripts/circuit-audit.sh <out> 107820 4 pro`: Professional, parked player,
cap of 107 820 frames = 1797 s of sim (the circuit time limit), four jobs
at a time. `--until-opponents` stops a run once every opponent has
finished, so a row shows when the field was done, not the cap. london 7 and
8 were re-run by hand with the same flags after the first sweep was cut off
by the wall limit.

| event | finished | re-anchors | slowest finisher |
|---|---:|---:|---:|
| sf 0 | 4/4 | 0 | 171.8 s |
| sf 1 | 7/7 | 9 | 315.7 s |
| sf 2 | 6/6 | 4 | 262.0 s |
| sf 3 | 4/4 | 4 | 263.5 s |
| sf 4 | 5/5 | 3 | 299.5 s |
| sf 5 | 7/7 | 4 | 201.1 s |
| sf 6 | 6/6 | 70 | 516.8 s |
| sf 7 | 6/6 | 21 | 342.6 s |
| sf 8 | 4/4 | 0 | 193.4 s |
| sf 9 | 6/6 | 4 | 246.6 s |
| london 0 | 7/7 | 0 | 76.2 s |
| london 1 | 6/6 | 0 | 114.0 s |
| london 2 | 6/6 | 30 | 443.5 s |
| london 3 | 6/6 | 6 | 293.6 s |
| london 4 | 5/5 | 36 | 443.7 s |
| london 5 | 4/4 | 9 | 234.0 s |
| london 6 | 7/7 | 15 | 434.9 s |
| london 7 | 6/6 | 40 | 582.9 s |
| london 8 | 6/6 | 48 | 806.5 s |
| london 9 | 6/6 | 6 | 294.3 s |

20/20 circuits complete on Professional with every opponent finishing, all
inside the 1797 s limit; the slowest is london 8 at 806.5 s (the 720 s
cap earlier is why it showed 5/6 there: slow, not stalled). The "11
incomplete circuits at 1797 s" of the original audit no longer exist: none
is incomplete at either difficulty, so there is nothing left to classify
as slow versus stalled. The `.opp` route data is not the cause of any
earlier stall. Re-anchor counts are the same residual signal noted above
(sf 6, london 7/8); the re-anchor recovery was not widened. Evidence
level: synthetic integration (headless, retail data, parked player); no
driving-player run and no rendered check.
