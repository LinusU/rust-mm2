# Race coverage matrix

F13-C.1's published account of what the authored Checkpoint catalog
actually does under the production headless runtime — measured, not
claimed. Every number below comes off the fingerprinted retail install
(`fnv1a64:e91e6cd4b2ae30d9`, read-only) at commit `1791df0`; the
audits and the runtime matrix were run on 2026-09-24 on Apple M1 /
Metal. The synthetic legs are the `mm2_app` / `mm2_game` test suites.

These records prove the engine executes the authored data — roster,
course, results, recovery — end to end. They do **not** prove
original-game fidelity: no reference retail timing, placing rule, or
AI behavior has been compared. Where a field's semantics are designed
rather than verified-original, the rules ledger
(`docs/original-rules.md`) says so.

## Instruments

```sh
mm2-inspect events <install>        # catalog denominator, per-row status, references, rewards, gates
mm2-inspect race-defs <install>     # race-definition build per event × difficulty
mm2-inspect opponents <install> [--strict]   # roster wiring vs .opp route records
mm2-inspect event <install> --city <c> --event checkpoint:<row>   # single-event deps

# production runtime leg — full session: world load, authored roster,
# countdown, checkpoint progress, results, restart machinery
mm2 --mm2-path <install> --city <london|sf> --event checkpoint:<i> \
    [--bot] --headless --frames 12000
```

`--bot` drives the scripted route-following driver (F15-B.5);
without it the `Hold` driver runs (settles ≤2 s, then holds full
throttle — it drives blind, it is *not* a parked car). Each run is
12000 app updates ≈ 197 s of simulation at the 120 Hz physics step.
Both legs ran at the default Amateur difficulty; Professional rosters
are audited structurally but not driven in this matrix.

The smoke record fields used below: `phase` (session state at frame
cap), `cp=cleared/total` (local participant), `results` (result-ledger
entries this generation), `outcome`/`place` (local participant's
result only — commit `1791df0` fixed a fallback that could surface
another participant's result here), `opp=resolved/spawned` (opponents
with a terminal `Finished`/`TimedOut` state), `omax` (deepest gate
count reached by any opponent, read from the `opps=` rows), `rs`
(session restarts — each restarts the whole generation: countdown,
grid, opponent progress), `e`/`r` in the opponent detail (three-point
escapes / bounded re-anchor teleports).

## The denominator (F13-AC01/AC06)

`mmracedata.csv` authors **12 Checkpoint rows per city — 24 total**,
all `ready` (no row diagnostics, no failed references). They sit in a
90-event catalog: 20 Blitz rows (F12 scope), 20 Circuit rows (F14),
26 Crash Course rows (**unsupported — F21 scope; excluded here, not
dropped**). The audits:

- `events`: 45 rows/city, 0 diagnostics; availability gates and
  authored rewards resolve (milestone chain race3-5 ← {0,1,2}, etc.).
- `race-defs`: 64 definitions built per city, **0 failed builds** —
  every Checkpoint row produces an amateur and a professional
  definition (8-gate+finish shape varies per row).
- `opponents`: 271 (London) + 246 (SF) opponent slots wired to routes,
  0 failed builds. `opponents --strict` exits 2 on **79 authored
  anomalies**, kept visible rather than filtered:
  - 77 orphan `.opp` route records wired to no opponent (retail ships
    unused routes across checkpoint/circuit/crash rows);
  - `sf/race0` amateur: `.aimap` wires 6 opponents but the roster
    table authors 7 (`6opp/7tbl` — one authored slot has no route);
  - `race/sf/stunt0.aimap`: 1 dead route ref in an extra (non-catalog)
    file.

## Runtime matrix — 24 events × 2 drivers

All runs at commit `1791df0`, Amateur, `--frames 12000`.
`P-F` = local participant finish (`place` in the ledger), `oF` =
opponents reaching `Finished` with a recorded result. `omax` as above;
`rs` = restarts.

### London (`world=city/london.psdl`)

| event | scripted: phase, cp, results, opp | scripted: P-F, rs, omax | hold: phase, cp, results, opp | hold: P-F, rs, omax |
| --- | --- | --- | --- | --- |
| 0 | results 5/5 r3 2/4 | 3rd, rs0, omax5 | playing 1/5 r2 2/4 | –, rs0, omax5 |
| 1 | playing 3/7 r0 0/6 | –, rs6, omax3 | playing 1/7 r0 0/6 | –, rs0, omax7 |
| 2 | playing 4/4 r0 0/7 | –, rs0, omax4 | playing 0/4 r2 2/7 | –, rs0, omax4 |
| 3 | playing 2/6 r0 0/7 | –, rs1, omax2 | playing 1/6 r0 0/7 | –, rs0, omax2 |
| 4 | playing 0/6 r0 0/7 | –, rs1, omax0 | playing 0/6 r0 0/7 | –, rs0, omax3 |
| 5 | playing 0/3 r0 0/7 | –, rs5, omax0 | playing 0/3 r0 0/7 | –, rs0, omax3 |
| 6 | **countdown** 0/8 r0 0/7 | –, rs9, omax0 | playing 0/8 r0 0/7 | –, rs0, omax3 |
| 7 | playing 1/8 r0 0/6 | –, rs1, omax1 | playing 0/8 r0 0/6 | –, rs0, omax5 |
| 8 | **countdown** 0/9 r0 0/6 | –, rs9, omax0 | playing 1/9 r0 0/6 | –, rs0, omax5 |
| 9 | playing 0/5 r0 0/7 | –, rs3, omax2 | playing 1/5 r0 0/7 | –, rs0, omax3 |
| 10 | playing 2/8 r0 0/6 | –, rs2, omax2 | playing 0/8 r0 0/6 | –, rs25, omax0 |
| 11 | playing 5/6 r0 0/6 | –, rs0, omax3 | playing 1/6 r0 0/6 | –, rs0, omax3 |

### San Francisco (`world=city/sf.psdl`)

| event | scripted: phase, cp, results, opp | scripted: P-F, rs, omax | hold: phase, cp, results, opp | hold: P-F, rs, omax |
| --- | --- | --- | --- | --- |
| 0 | playing 2/6 r4 4/6 | –, rs0, omax6 | playing 3/6 r3 3/6 | –, rs1, omax6 |
| 1 | playing 2/8 r0 0/6 | –, rs0, omax7 | playing 2/8 r0 0/6 | –, rs0, omax7 |
| 2 | playing 0/9 r0 0/6 | –, rs1, omax1 | playing 2/9 r2 2/6 | –, rs0, omax9 |
| 3 | results 6/6 r1 0/5 | **1st**, rs0, omax6 | playing 2/6 r1 1/5 | –, rs0, omax6 |
| 4 | results 7/7 r1 0/5 | **1st**, rs0, omax7 | playing 2/7 r1 1/5 | –, rs0, omax7 |
| 5 | playing 0/6 r0 0/5 | –, rs1, omax0 | playing 0/6 r1 1/5 | –, rs0, omax6 |
| 6 | playing 3/6 r0 0/4 | –, rs0, omax3 | playing 1/6 r0 0/4 | –, rs0, omax3 |
| 7 | playing 2/4 r0 0/6 | –, rs1, omax2 | playing 0/4 r0 0/6 | –, rs0, omax3 |
| 8 | playing 3/8 r0 0/6 | –, rs1, omax4 | **countdown** 0/8 r0 0/6 | –, rs13, omax0 |
| 9 | playing 0/10 r0 0/6 | –, rs1, omax0 | playing 0/10 r0 0/6 | –, rs0, omax2 |
| 10 | playing 0/5 r0 0/6 | –, rs5, omax0 | playing 1/5 r0 0/6 | –, rs0, omax3 |
| 11 | playing 2/11 r0 0/6 | –, rs1, omax2 | playing 0/11 r0 0/6 | –, rs0, omax3 |

## What the matrix shows

**Every row loads and runs.** 48/48 legs reached `status=pass` — the
world, authored roster, countdown, checkpoint tracking and HUD run on
every catalog row with no load failure, panic, or smoke sanity-trip.

**Opponents race.** Opponents spawn from the authored roster, take the
start, and clear gates in every uninterrupted generation — 20 of 24
events show opponents reaching ≥2 gates under the hold leg, and
**7 distinct events produced opponent finishes with ledger results**
(18 total): london-0 (2+2 across legs), london-2 (2), sf-0 (4+3),
sf-2 (2), sf-3 (1), sf-4 (1), sf-5 (1). Player finishes: 3 — london-0
place 3 behind two opponents, sf-3 and sf-4 place 1. When the local
participant resolves first the session moves to `Results` (DSN-11),
so `opp=0/5` on sf-3/4 is the session ending mid-race, not stalled
opponents — the same events' hold legs show opponents at omax 6-7.

**Restart contamination is real and disclosed.** The scripted/hold
drivers wreck under authored damage → `RestartEvent` rules on several
courses: scripted london-6/8 burned 9 restarts and were still in
countdown at the frame cap; hold london-10 took **25** restarts; sf-8
hold took 13; sf-10 scripted 5. Each restart rebuilds the generation —
countdown, grid, opponent progress all reset — so `cp=0 opp=0` there
is a *driver* outcome, not evidence the opponents cannot progress.
The hold leg (no scripted re-anchors) is the cleaner opponent
read: london-1 opponents reached 7/7 gates vs 3 under the scripted
leg. The loop itself is also positive evidence: 25 clean restart
cycles with `dup=0` — no result duplication, no stale progress
(AC05's restart leg, exercised hard).

**Wall-clock + physics caveat (sf-8).** The scripted sf-8 leg passed
but needed ~24 min wall — the event is CPU-bound (6 opponents +
dense authored traffic) at ~8-12 updates/s, and recorded the
matrix's worst `dropped=53955` plus a transient `peak=17070 m/s`
velocity spike (collision impulse artifact; the final pose stayed
finite and `status=pass` held). Its hold leg restart-looped (rs=13,
countdown) — the fast blind start wrecks before steering matters.

## Anomalies kept visible

- **Physics step drops under contention:** `dropped=` counts physics
  substeps skipped under load — park-sf-4 55917, bot-sf-8 53955,
  park-london-9 53155, bot-sf-4 16348 (the player *still won* that
  one), bot-sf-7 13535, park-london-5 4914. Simulation stays finite
  and correct; the drops mean effective sim rate sagged below 120 Hz
  on those runs. bot-sf-8 also logged a transient `peak=17070 m/s`
  velocity spike (collision-impulse artifact; pose stayed finite).
- **End-of-run grounding:** `wheels=0/4` on park-london-4 and
  park-sf-4 (car ended propped/airborne), 2/4 bot-sf-2, 3/4
  park-sf-7 — disclosed poses, not crashers.
- **London late-catalog grind:** even uncontaminated, london 4-11
  opponents top out at 2-5 gates in ~197 s with heavy escape/re-anchor
  use (park-london-4: 137 escapes / 31 reanchors across 7 cars) — the
  deep-city courses beat the opponent controller's current skill.
  Whether that matches retail difficulty is **unknown** — a F15-B
  residual, not excused here.
- **Roster mismatch (authored):** sf-0 amateur spawns 6 opponents
  though the table authors 7 (`6opp/7tbl`) — disclosed above, kept in
  the denominator.

## Evidence → acceptance mapping

- **F13-AC01** (discoverable + deps load): `events` 24/24 ready,
  `race-defs` 48/48 built, `opponents` all rosters wired; every row
  ran the production session to `status=pass`. **Structural + runtime
  legs done; Professional runtime leg not driven.**
- **F13-AC02** (ordering/crossing rules): AnyOrder freedom verified
  (CHK-1); 36 race tests cover high-speed/wrong-height/repeat
  crossings, teleport/reset segment invalidation, timeout edge ticks.
- **F13-AC03** (independent progress): per-participant `opps=` rows
  diverge live; `ordered_multi_lap_participants_stay_independent`,
  `a_non_local_resolution_does_not_end_the_local_race` cover it.
- **F13-AC04** (opponents start/progress/finish): **demonstrated** —
  opponent finishes on 7 events, progress on all uninterrupted legs,
  results compare against real participants (`pos=` vs full field).
- **F13-AC05** (restart/recovery don't bypass): restart rebuilds the
  generation (25-cycle soak, `dup=0`, results scoped per generation);
  synthetic `restart_removes_the_race_resource`,
  `restart_from_results_rebegins_the_session`, teleport/reset invalid
  crossing tests; recovery-penalty fidelity remains a F05/F15
  research item.
- **F13-AC06** (catalog matrix): **this document** — 24/24 rows
  attempted, 48/48 legs `status=pass` (sf-8 scripted leg a ~24-min
  wall-clock outlier), finishes and stalls reported per event per
  driver.

## Residuals / honest limits

- Amateur difficulty only — Professional rosters are structurally
  audited, not driven.
- sf-8's scripted leg is a wall-clock outlier (~24 min at ~8-12
  updates/s under load) with the matrix's worst dropped-step count —
  performance there is a real question, not yet diagnosed.
- The `Hold` leg is a blind full-throttle driver, not a parked
  control — a true stationary-player isolation leg would need a new
  driver mode.
- ~100-200 s/event budget leaves genuinely-long races unresolved;
  opponent non-finishes on deep courses are budget- and
  controller-skill-bound, not proven impossible.
- Blitz (F12), Circuit (F14), Crash Course (F21) rows are audited for
  structure only; their runtime matrices belong to those features.
- No original-fidelity claim: engine self-metrics only. Difficulty,
  pacing and AI competence vs retail are unverified.
