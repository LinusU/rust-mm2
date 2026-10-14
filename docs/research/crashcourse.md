# Crash Course data audit (F21-A.1) and runtime recovery (F21-C.2)

Measured on the retail install (`fnv1a64:e91e6cd4b2ae30d9`,
read-only) with `mm2-inspect crash-course <install>`. This document
records what the files actually contain; every classification that is
not directly evidenced is marked *inferred*. The last major section,
"Recovered runtime (F21-C.2)", records what `Midtown2.exe` does with
those files — the `Event` dispatch, the sub-event record and every
family's pass/fail rule — and supersedes the "inferred" qualifiers
the earlier sections carry for them.

## File layout

Each race city carries one authored lesson table plus per-lesson
record sets:

- `race/<city>/mmcrashdata.csv` — 13 rows (the lesson index), in the
  authored sequence `lesson1–3, midtrm1, lesson4–6, midtrm2,
  lesson7–9, midtrm3, final13` (CC-2). Each row carries the standard
  Amateur/Professional `RaceParams` pair — on retail only
  `TimeofDay`/`Weather` vary between lessons and between
  difficulties (Professional rows frequently author a *different*
  tod/weather than the Amateur row — e.g. london `crash9` am 3/0 vs
  pro 2/2). `Opponents`/`Cops`/`NumLaps`/`TimeLimit` are all 0 in
  every retail row: lesson limits live in the sub-event tables
  instead.
- `race/<city>/crash<N>data.csv` and `crash<N>data_p.csv` — the
  lesson's sub-event tables. `_p` rows author tighter time limits
  (london `crash0` 26 s → 25 s, sf `crash5` 180 s → 150 s) or higher
  `AmbDensity` (london `crash5` 0.05 → 0.30) and, in one case, a
  *different waypoint file* (sf `crash9` pro links
  `reverse180_p.csv`). The `_p` = Professional reading is inferred
  from the shared `.aimap_p`/`race<N>_p` convention and the
  harder-authored values; it is not documented anywhere.
- `race/<city>/crash<N>.aimap` / `crash<N>.aimap_p` — the lesson's
  actor wiring (police spawns, opponent lead cars, road exceptions,
  chase distance).
- Linked waypoint CSVs (`longjump.csv`, `frogger0waypoints.csv`,
  `final1.csv`…) referenced by each sub-event row's `Filename`
  column.
- `<object>_crash<N>.pathset` override records
  (`london_parkedcar_crash0`, `london_bridge_crash3`,
  `london_ferry_crash9`…), re-attributed to the lesson by the
  `<object>_<event>` suffix convention (`pathset.md`) — inferred
  attribution, the classification lands them under their own stems.
- `race/<city>/<city>_rewards.csv` — `crash,<index>` rows bind the
  midterm/final rewards (CC-6).

## `crash<N>data.csv` row shape

```
Filename,Event,Checkpoints,TimeLimit,AmbDensity,<tail...>
```

Header anomalies are authored, not corruption: `AmbDenisty` is
misspelled on some tables, `Filename` is unnamed on others (london
`crash8`), and the tail column names drift (`Misc`, `cornerspeed`,
`chkflags`, `numopp`, `extra`, `etra`, blank). The parser keeps the
header cells verbatim (`CrashDataFile::columns`) — they are the only
in-file evidence for the tail meanings — and preserves the tail
values raw (`CrashDataRow::extra`).

Observed correlations, with the executable's reading
("Recovered runtime" below) where it settles them. `tail[k]` is the
k-th value after `AmbDensity` (file column k+5); header names drift
per file, so a name is only claimed where the file that sets the
value also authors one.

- `tail[0]` (`Misc` on london `crash1`, sf `crash2`/`crash12`;
  `cornerspeed` on london `crash6`): nonzero only on `Event 4` rows —
  40 on london's pair (`crash1` `corner`, `crash3` `exam1_1`), 50 on
  all three sf `Event 4` rows (`crash2` `corner0waypoints`, `crash3`
  `exam1_1`, `crash12` `final`). **Verified (CC-9):** the cornering
  lesson's minimum speed in mph, read as a float; a value below 1.0
  means "unset" and the executable uses 50.
- `tail[1]`: 1 on exactly four rows — london `crash4` `map` and sf
  `crash4` `oneeighty`, both difficulties (`[0,1,0,0,0,0]`). **Read
  (CC-8):** the executable passes it to the course loader as a flag
  that lands in bit 0 of the course's field `+0x20`; the code that
  tests the bit sits next to the gate-marker show/hide calls other
  races use, so it is most likely "show every gate marker, not just
  the next one" — inferred from that pairing, the draw path was not
  traced. It has no effect on pass/fail.
- `tail[2]` (`numopp` per london `crash6`'s header): 1 on every
  `Event 2` (follow) row *and* on both sf `Event 8` (stop) rows —
  `crash6` `stop`, `crash7` `exam1_2`. **Verified (CC-8):** the number
  of lead cars the sub-event takes from the opponent array; the lead of
  sub-event *n* is the array slot after the `numopp` of all earlier
  sub-events (`0x416910`). Every lesson containing a `numopp`=1 row
  wires exactly one `[Opponent]` lead car (the sf stop rows wire
  `vpford`); london `crash3` and `crash11` wire an opponent with no
  `numopp` row (their sub-events are e4/e7), which the executable
  never asks for.
- `tail[3]` (unnamed on every file that sets it): 1 on three sf rows —
  `crash10` `follow` amateur only (the `_p` row authors 0) and
  `crash11` `exam1_3` on both difficulties. **Not read:** the loader
  stops after `tail[2]` (CC-8), so `tail[3]` and later columns are
  dead data.
- `tail[4]`/`tail[5]` are 0 on every retail row and are not read.

## The `Event` column — decode

The integer correlates with the waypoint file's lesson family
across all 26 retail lesson tables. The family *names* below are a
measured correlation (mm2hook recovers no crash-course object); the
dispatch itself is read from the executable — see "Recovered runtime"
for which handler each code selects and what it requires.

| code | retail filenames | inferred family |
|-----:|------------------|-----------------|
| 0 | `longjump`, `precisionjump` | jump |
| 2 | `follow`, london `exam2_2`/`final1`, sf `exam1_3` | follow a lead car |
| 3 | `copchase`, sf `exam2_2` | cop chase |
| 4 | `corner`, `corner0waypoints`, `exam1_1`, sf `final` | cornering |
| 5 | `frogger0waypoints`, `safe` | cross traffic |
| 7 | `slalom`, `oneeighty`, `reverse180`, london `exam1_2`/`exam1_3`/`final2` | maneuver |
| 8 | `stop`, sf `exam1_2` | braking |
| 9 | london `map` | map reading |

Codes 1 and 6 are authored nowhere on retail. The same filename can
carry different codes per city (london `exam1_2` = 7 maneuver vs sf
`exam1_2` = 8 stop; london `exam1_3` = 7 vs sf `exam1_3` = 2) — the
code belongs to the row, not the filename.

## Aimap wiring (retail)

- **Follow lessons** (Event 2) wire one `[Opponent]` lead car:
  london `vpcab` on `follow-0.opp`/`crash3-0.opp`/`exam1_2.opp`/
  `final.opp`; sf `vpford`/`vpbullet` on `race0-a-6.opp`,
  `Follow-1.opp` (mixed-case on disk — VFS-normalized),
  `exam1_3.opp`. SF `crash6` (stop) also wires a `vpford` opponent,
  consistent with its `numopp` tail.
- **Cop-chase lessons** (Event 3) wire `[Police]` spawns: london
  `crash10` 14 police over `vpauditt`/`vpdb7`/`vpmustang99`;
  london `crash11` 5; sf `crash5` 14 `vpcop` plus the only authored
  `[CopChaseDistance]` (150); sf `crash7` 6 `vpcop`.
  Launch fields the lineup (F21-B.18): headless retail runs
  (`--event crash:<row>`, both difficulties) report `pol=N/N` with N =
  14 (london crash10), 5 (london crash11), 14 (sf crash5), 6 (sf
  crash7); a cop sees the standing player and begins a pursuit in
  crash5/crash11, the others stay out of sight at the start. What
  makes a cop-chase lesson pass or fail is recovered (CC-9): reach the
  finish with no cop in sight.
- **Lead cars** are fielded (F21-B.19): `lesson_opponent_roster`
  turns each `[Opponent]` row into a lead car on its authored vehicle
  and `.opp` route, as a non-participant (no `RaceProgress`) staged at
  the route's row 0. Nine lessons wire one — london `crash3` (also
  reused by `crash11`), `crash6`, `crash7`, `crash12`; sf `crash6`
  (`vpford` on `race0-a-6.opp`, not the `stop-1.opp` its own records
  carry — reported as an unreferenced record), `crash7`, `crash10`
  (`Follow-1.opp`, mixed-case on disk), `crash11` — and a route the
  lesson's own records do not carry resolves through the VFS
  (`crash3-0.opp` for london `crash11`, `race0-a-6.opp`, `Follow-1.opp`).
  Retail: 18/18 lesson×difficulty wirings field a car with a drivable
  route (opt-in sweep), and headless runs (`--event crash:<row>`, 900
  frames, both difficulties) report `lead=<slot>:<vehicle>/<m>m` with
  122–343 m driven. The lead car drives for the whole session, not per
  leg, and drives its route regardless of the player; what a follow
  or stop lesson asks of the player relative to it is recovered (CC-9):
  stay within 100 m and finish within 10 m (follow), wreck it before
  it arrives (stop).
- **Exceptions**: four sf lessons — `crash1` (slalom), `crash2`
  (corner), `crash4` (oneeighty) and `crash12` (final) — wire an
  identical ten-road `[Exceptions]` block (roads 10–19, `1.0 35.0`)
  in both difficulty aimaps. No other lesson in either city authors
  any, and no event-family pattern explains the set (sf `crash3`'s
  cornering `exam1_1` and `crash9`'s `reverse180` wire none). A
  shared cordon around a reused course block is the plausible reading
  — inferred.

## Retail audit result (2026-09-26)

`mm2-inspect crash-course <retail>` — both cities 13/13 lessons
`ready`, 0 incomplete, 0 unresolved links, 0 dead `.opp` wires, 0
wired vehicle ids outside the catalog, rewards bound at `crash3/7/
11/12` matching CC-6. Extras unclaimed by the `_crash<N>`
attribution: london 26, sf 23 — counted and printed, not filtered.

## Runnable legs (F21-B.1)

`mm2_content::lesson_legs` builds each sub-event into a gate-run
`RaceDefinition` (DSN-72, designed). Measured on retail: 26 lessons ×
2 difficulties build 52/52. Waypoint rows per leg (start + gates):
london `longjump` 5, `corner` 24, `exam1_1` 15, `final2` 20, `safe` 19,
`map` 9, `precisionjump` 9, `copchase` 7; the two-row files (`slalom`,
`stop`, `follow`, `exam1_2`, `exam2_2`, `final1`, `reverse180`) are a
start pose and one far pose; sf `copchase` 24, `exam1_1`/`final` 18,
`slalom` 11, `corner0waypoints` 10. The ordering of rows as gates is
now read from the executable (CC-8: row 0 is the start gate, the rest
ordered; jump legs are any-order with the last row last); what each
family *requires* beyond reaching the gates is "Family rules" below.

## Recovered runtime (F21-C.2)

Method and confidence. Static reading of `retail/Midtown2.exe`
(x86-32, image base `0x400000`, the build `docs/research/vehicle-
physics/README.md` describes; addresses are this build's) with
`objdump -d -M intel`, anchored on the `crash%ddata` /
`crash%ddata_p` format strings (`0x5c2f5c`/`0x5c2f68`, referenced from
the table loader at `0x4164af`). No decompiler output was used and
nothing was run under a debugger or against the retail game — every
statement is a reading of the instructions, with x87 comparisons
re-derived from the `fnstsw`/`test ah` masks. Message texts come from
the string table in `mmlang.dll` (resource type 6). Class and field
names are ours; the executable has no symbols. Unless a paragraph says
*inferred* it is **verified_original** (read in the code).

### The lesson object

One object runs a lesson (constructor `0x415b10`, destructor
`0x415bd0`). Fields used here:

| Off | Meaning |
| --- | --- |
| `+0x48` | the player's car |
| `+0x76cc` | the sub-event's **course** (a checkpoint set, loaded by `0x416320`) |
| `+0x76e8` | ten 60-byte sub-event records (below) |
| `+0x7940` / `+0x7944` | index of the sub-event in play / number of sub-events |
| `+0x270` | lesson state, `0`–`6` (below); `+0x408` its timer, seconds |
| `+0x795c` | corner minimum speed, mph (`50.0` from the constructor) |
| `+0x7960` | corner grace seconds (`1.0`, constructor `0x415ba9`, never rewritten) |
| `+0x7964` / `+0x7968` | corner time-below-speed / reached-speed flag |
| `+0x7958` | stop lead's `MaxDamage`, `150000.0` (constructor `0x415bb3`) |

### The sub-event table loader (`0x416080`) — CC-8

Reads `race/<city>/crash<N>data` (`crash%ddata`) or `…data_p` when the
global `0x6b064c` is nonzero. That global is the difficulty argument of
the lesson's profile calls (`0x41a4c0`) — so `_p` = Professional is now
read, not just inferred from the harder values. Each line is split on
the file's delimiters into one **60-byte record** (`+0x76e8 + 60·i`;
the constructor allocates ten, the loader does not bounds-check):

| Rec off | Column | Type | Used for |
| --- | --- | --- | --- |
| `+0x00` | `Event` | int | the handler dispatch |
| `+0x04` | `Checkpoints` | int | nonzero: load the course (`0x416320`); zero skips the load and clears the course's bit 0 |
| `+0x08` | `TimeLimit` | float, s | sub-event timer (`0x415dc0`) |
| `+0x0c` | `AmbDensity` | float | clamped to `[0,1]`, ×0.2, stored in `0x6b1e2c` (consumer not traced); the raw value goes to `0x6b05f8` |
| `+0x10` | `tail[0]` | float | corner minimum speed, mph |
| `+0x14` | `tail[1]` | int | course flag (see the tail notes) |
| `+0x18` | `tail[2]` | int | lead cars taken from the opponent array (`numopp`) |
| `+0x1c` | `Filename` | 32 bytes | the waypoint CSV stem |

Columns after `tail[2]` are never read. The first line is consumed as
the header, so the authored header names are cosmetic.

### The course (`0x416320`, `0x4346f0`, checkpoint update `0x4352a0`)

The waypoint file `race/<city>/<Filename>.csv` becomes the course: each
row is a gate (position, the fourth column the radius, default 15 when
zero) and the row count is the gate count. `Event 0` loads it as
course **mode 4**, every other event as **mode 5**:

- **Mode 5 (ordered)** — every frame the car's last segment is tested
  against the *current* gate only (`0x43e200`); a hit advances the
  current gate and a passed counter; the course completes when the
  counter equals the gate count. The counter and current gate are
  reset to `1` (`0x4358d2`, `0x4357e9`): **row 0 is the start gate,
  counted as passed, and the gates are rows `1..`** in authored order.
  This confirms the reading DSN-72 took on inference.
- **Mode 4 (jump)** — the same walk as the blitz mode (mode 2): the
  car's position is looked up against *all* gates (`0x435f50` returns
  the gate containing it); a hit on any gate but row 0 and the last is
  marked passed (`0x4360b0`) whatever its order, and a hit on the last
  row only completes the course when the passed counter has reached
  `count-1`, i.e. every middle gate is already hit. Jump lessons are
  therefore any-order gates with the last row as the finish. (Mode 4
  shares its branch with mode 2; the repeat-hit guard inside
  `0x435f50` was not traced.)

The course's completion flag (`+0x34`) is what every handler below
calls "the course completes". `Checkpoints = 0` has no retail row.

### Per-sub-event setup (`0x415dc0`)

Runs when a sub-event loads. It copies `TimeLimit` into a global
(`0x6b07a4`) and the car's timer field (`+0xd2c`; a nonzero global
`0x6b0990` overrides it with an integer — a launch/debug override that
no retail path sets as far as read), then switches on the event:

- `0`, `3`, `5`, `6`, `7`, `9` — the timer is **on** (`car+0xbd8 = 1`).
- `4` — min speed = `tail[0]` if `>= 1.0` else `50.0`; the timer is
  **on only when `TimeLimit > 0`**, so the two untimed retail corner
  rows (`TimeLimit 0`) never expire.
- `8` — timer **on**. The lead-car families (`2`, `8`) also set a field
  (`+0xa0 = 25.0`) and flag bit 0 on the object at lesson `+0x54`
  (`0x415f82`, `0x414bc0`) and clear the car's own HUD flag bits; what
  those drive was not traced.
- `2` — as for `8` but **timer off** (`car+0xbd8 = 0`, `0x41601c`).
  The follow handler never reads the clock, so a follow row's
  `TimeLimit` (36 s, 200 s on retail) does nothing.
- `1` — matches no branch of the setup; `6` shares the timer-on arm.

### The per-frame handler (`0x4169a0`) — CC-8

A jump table on the current record's `Event` (`0x416a2c`):

| `Event` | handler | family |
| ---: | --- | --- |
| 0 | `0x4174d0` | jump |
| 1 | `0x419ca0` | *not decoded — no retail row* |
| 2 | `0x417b20` | follow |
| 3 | `0x416a60` | cop chase |
| 4 | `0x4187b0` | corner |
| 5 | `0x418100` | cross-traffic |
| 6 | `0x417040` | *not decoded — no retail row* |
| 7 | `0x419020` | maneuver |
| 8 | `0x419600` | stop |
| 9 | `0x419020` | map — **the same handler as 7** |

Anything above 9 logs an error (`0x5c2fd8`). So the family names the
earlier sections inferred from file stems are exactly the dispatch
partition, with one refinement: maneuver and map are one rule.

Every handler is the same seven-state machine on `+0x270`: `0`–`2`
the title / "Ready…" / "Set…" sequence (the title is looked up by the
lesson's row), `3` **running**, `4` **failed** (a results prompt),
`5` **wreck recovery** (3 s, then the car is reset in place by
`0x406180` and the state returns to `3`), `6` **passed** (results
prompt). A cleared non-final sub-event loads the next record
(`0x416320`) and re-enters state `0`, so the title and countdown
replay per sub-event; the final sub-event's clear posts the lesson as
passed (`0x41a700(1)`), any failure posts it failed (`0x41a700(0)`).
Everything below is the state-`3` logic.

"Timer expired" is the car's countdown (`0x42e4b0` ≤ 0). "Wrecked"
(`0x406140`) is the car's damage record reading `+0x34 < +0x30`
(damage above the maximum — which field is which is inferred from the
stop handler's use of `MaxDamage`/`MedDamage`). Speeds are mph (the
car's `SpeedMPH`, `|forward speed|·2.236025`), distances metres.

### Family rules — CC-9

All eight rules were read from state `3` of the handler named. The
implementation is `mm2_game::LegRule` (`crates/mm2_game/src/
lesson_rules.rs`); "wreck" below is the `WreckPolicy`.

- **Jump (0), cross-traffic (5), maneuver (7), map (9)** — *the bare
  skeleton*: course completes → cleared; timer expired → "Time's up!";
  wrecked → "Game over!" (fails the leg). Cross-traffic (`0x418404`)
  tests the timer *before* the course, the other three the course first;
  a same-frame tie is the only observable difference and the port takes
  "course first" for all four. Jump's distinctive rule is its gate
  mode (any order, finish last), not its verdict. Map and maneuver
  differ only in setup (HUD/radar flags in `0x415ecd`/`0x415f2f`).
- **Follow (2)** — untimed. Let `d` be the 3-D distance between the
  player and the lead car. *Cleared* when the lead has finished its
  route **or** the course completes, **and** `d < 10 m` ("You Won!").
  *Failed* when `d > 100 m` at any time ("Car escaped!"). A wreck is
  recovered (state 5), not a failure. "The lead has finished its route"
  (`0x53d6c0`): it is on its last route node, within 30 m of the goal
  point (horizontal distance², constant `900`) and — once inside
  20 m (`400`) — has crossed the goal plane (a side test against the
  stored goal direction). The goal point is a global (`0x6b1f90`); which
  route node feeds it was not traced (inferred: the route's end).
- **Cop chase (3)** — when the course completes, *cleared* unless a cop
  is "in sight" (`0x41a1c0`: any entry of the **cop array** — the
  manager's first array, whose out-of-range accessor logs "Returning a
  NULL Cop" (`0x5348d0`), the second being "Opponent" (`0x534920`) —
  whose entry is in the field (`+0x977a` nonzero), with an
  unobstructed ray between it and the player (both lifted 3.5 m) and
  distance `< 200 m`) → "You survived the gauntlet!"; else "Lose your
  pursuers before you finish!" (failed). Timer expired → "Time's up!
  Drive faster to win!" (failed). A wreck is recovered. The ray
  query's result polarity (a hit blocks) is **inferred**.
- **Corner (4)** — in order: course completes → *cleared* unless a
  cop is in sight (the same `0x41a1c0` test; lost with "Lose your
  pursuers…"); timed and expired → failed; **speed rules**, with `v` the
  car's mph and `m` the minimum (`tail[0]`, default 50):
  before the minimum has been *reached*, clearing a gate fails the leg
  ("You need to get up to speed before hitting a checkpoint"), else
  `v > m` marks it reached ("You have reached the minimum speed!");
  once reached, `v > m` clears the below-speed clock, `v < m` runs it
  and more than **1 s** below fails the leg ("You have not maintained
  the minimum speed!", the reached flag cleared); `v == m` does
  nothing. A wreck fails the leg ("Game over!").
- **Stop (8)** — timed. The lead is the opponent array's slot for the
  sub-event; its `MaxDamage` is set to `150000` (and `MedDamage` to
  half) when the race goes (`0x4198a3`, if any opponent is fielded —
  the opponent count `0x6b1e5c`). Order: lead damage at its maximum → *cleared*
  ("You did it!" — the lesson is "Destroy that car!"); lead finished
  its route (`0x53d6c0`) → failed ("Car reached destination" / "You
  lost!"); timer expired → failed; the player's wreck is recovered.
  The stop handler **never reads the course flag**: reaching the far
  gate is not a pass.

String ids (`mmlang.dll`): cop chase 193–198; jump 206–209 and 614/615;
follow 218–223; cross-traffic 225–230; corner 231–240 and 244; maneuver/
map 241–245 and 609; stop 617–622. The lesson titles the table carries
for the same families ("Frequent Flyer" 205, "Straight, With Chaser"
217, "River Dancing" 224, "Turn It Up!" 231, "Bonnie & Collide" 616,
"Artful Dodging" 192) agree with the partition above.

### Per-gate location call-outs (`0x51b950`, `0x51bd90`)

The school-table loader `0x51b680(this, row, city)` picks `ccl`
(london) or `ccs` (else) and reads
`aud\spchdata\<school>\<school><row>.csv`. Only when `row == 4`
(`0x51b736`, `cmp edi, 4`) does it additionally read two fixed-named
tables in the same dir: `cc_cpoint_waveinfo.csv` (`0x51b740`) and
`cc_cpoint_indexinfo.csv` (`0x51b74d`).

`cc_cpoint_waveinfo.csv` is an ordinary cue table whose `CHECKPOINT`
section names the call-out wave pool (`CCLLOCATION,22,0` on retail —
suffixes `1..=22`). `cc_cpoint_indexinfo.csv` (`0x51bd90`) is a third
grammar: a heading line (`SUFIX NUM FOR CPOINTS`) then one signed
integer per line, each read with `strtok(line, ",")` + `atoi` (only the
first field counts) — retail `-1, 15, 3, 6, 5, 10, 12, 13`.

The gate-cleared routine `0x4360b0` increments the passed counter and
calls `0x51b950(this, gate)` with `gate` the course array index
(`[esi+0x24]`) and arg2 `0x3c23d70a` (0.01f). `0x51b950` refuses a
`gate` at or past the list's count or one whose entry is `-1` (silence),
then draws entry `indexinfo[gate]` from the `CHECKPOINT` cue's wave
list: `0x513e00` builds that list for suffixes `add + 1 ..= end`, so
list entry `k` names suffix `add + 1 + k` (`0x513fd0` returns
`[[cue+0x18] + 4*k]`). For London's single `map` lesson (course array
row 0 the start line, rows 1..8 the eight gates) the mapping is
`1→16, 2→4, 3→7, 4→6, 5→11, 6→13, 7→14`; row 8 is past the
eight-entry list and row 0 is the `-1` start line, both silent. Only
London ships the tables (`ccs` ships none), so only this lesson can
speak a call-out.

Unrecovered: the `0.01f` argument's meaning (a scheduled-cue delay or
gain, routed `0x5105e0`→`0x5103d0`), the exact cue timing, and whether
any other sub-event of the lesson fires a call-out.

### What the port does with it

- `mm2_game::LegRule` is the pure rule per family; `LegReport::
  from_verdict` carries its verdict into the sequencer. Positive and
  negative tests for every family are in `crates/mm2_game/tests/
  lesson_rules.rs` (synthetic observations; the constants are the
  executable's).
- `mm2_content::lesson_legs` builds the jump gate mode and attaches
  `LessonLeg::rule`; `LessonObjective::rule_evidence` is now
  `OriginalVerified` for **every** retail family — the skeleton
  families (nothing more to evaluate) and follow, cop chase, corner
  and stop, whose recovered evaluator the live session now feeds
  (F21-C.3). A follow leg builds untimed. Retail coverage: sf 13
  original-verified / 0 synthetic-only / 0 unresolved, london 13 / 0 / 0
  (`mm2-inspect crash-course`).
- **Wired in the session (F21-C.3):** `mm2_app::lesson::drive_lesson`
  builds a `mm2_game::LegObservation` each `FixedLast` tick from the
  running session — the player's speed, gates passed, the race clock
  and the local car's wreck; the nearest `opponents::LeadCar`'s
  squared distance, `route_finished` (`opponents::lead_route_finished`,
  the original's `0x53d6c0`) and damage tier; and a
  `police::cop_sight_clear` pursuer test (both ends lifted 3.5 m, the
  original's `0x41a1c0`; only `GameLayer::World` blocks sight). A
  non-`GateRun` leg is decided by `LegRule::evaluate`, and the session
  no longer ends on the last gate of such a leg until the rule clears
  (`LessonDriver::holds_results`/`is_over`; `advance_race`'s edge now
  also runs in `RacePhase::Complete`); a stop leg's lead takes the
  lesson's `MaxDamage` 150 000 via `lesson::arm_stop_lead_damage`
  (`0x4198a3`); a lead-family leg seen with no lead is a surfaced
  `RuleError`, never a pass.
- **Still not wired:** the lead's *goal direction* for `0x53d6c0`
  (the port infers the final route leg's heading — `docs/
  original-rules.md` DSN-75), and the wreck-recovery (reset in place
  after 3 s) policy for follow, cop chase and stop, which
  `WreckPolicy` reports but nothing yet acts on versus
  `DisabledOutcome::RestartEvent`.
- **Per-gate call-outs (F21-B):** `mm2_game::lesson_callout_tables`
  binds the `cc_cpoint_*` pair for the `row == 4` lesson and
  `mm2_game::callout_suffix(add, indexinfo, row)` maps a cleared course
  row (the definition's checkpoint `j` is course row `j + 1`) to a wave
  suffix, or `None` for the authored silence (the `-1` sentinel, a row
  past the list). `CommentaryAudio` resolves the pair on the first ask
  and `drive_lesson` records each newly cleared gate on the driver,
  which `lesson::lesson_callout_cue` hands to the commentary as
  `Checkpoint`-priority lines. Retail validation: only London's `map`
  lesson resolves the pair and all seven named waves ship
  (`only_the_retail_map_lesson_binds_per_gate_callouts`).

## Open questions

- Events `1` (`0x419ca0`) and `6` (`0x417040`) have dispatch slots but
  no retail row; they were not decoded and map to no objective.
- The polarity of the cop-sight ray query (`0x468e30`) and what the
  cop entry's `+0x977a` count is; the cop array itself is identified by
  its accessor's error string.
- The wreck predicate's two damage fields and whether the original's
  damage saturates at the maximum (the port's `DamageState` does).
- What `0x6b07ac` (a global that suppresses the wreck failure in
  several handlers) and `0x6b0990` (an integer that overrides the
  timer) are; neither is set by any path read.
- What bit 0 of the course flag (`tail[1]`) draws, and the consumer of
  the `AmbDensity` product in `0x6b1e2c`.
- Whether `Checkpoints = 0` (no retail row) is a supported "no gates"
  sub-event or only an unused switch.
- The goal point `0x6b1f90` the lead's arrival test uses: which route
  node writes it.
- Instruction/subtitle linkage — no authored reference in these
  tables points at instruction content; where lesson *text* lives is
  unrecovered. The lesson *messages* are the `mmlang.dll` ids above;
  the *voice* lines are `aud/spchdata/<ccl|ccs>/<school><row>.csv`
  (`DSN-92`); the per-gate call-outs are the `cc_cpoint_*` tables
  covered above. The mid-exam `*firstexam*`/`*thirdexam*` lines are
  authored inside their rows' `PRERACE`/`RESULTSPOOR` sections
  (`ccl3`/`ccl7`/`ccl11`/`ccl12`) and are already bound; only their
  on-screen subtitle text is unrecovered.
- Professional `TimeofDay`/`Weather` divergence — deliberate harder
  conditions or revision drift.
