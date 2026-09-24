# Race coverage matrix

F13-C.1's published account of what the authored Checkpoint catalog
actually does under the production headless runtime — measured, not
claimed — plus F13-C.2's stationary-control and Professional legs.
Every number below comes off the fingerprinted retail install
(`fnv1a64:e91e6cd4b2ae30d9`, read-only): the audits and the scripted/
hold matrix at commit `1791df0`, the parked/professional legs at
commit `a7d2797`; all runs on 2026-09-24 on Apple M1 / Metal. The
synthetic legs are the `mm2_app` / `mm2_game` test suites.

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
    [--bot | --parked] [--pro] --headless --frames 12000
```

`--bot` drives the scripted route-following driver (F15-B.5);
`--parked` holds the handbrake all session — the stationary control
(F13-C.2), measuring what the opponents do with no competing local
driver; with neither flag the `Hold` driver runs (settles ≤2 s, then
holds full throttle — it drives blind, it is *not* parked).
`--pro` selects the event's authored Professional parameter block and
aimap variant (RACE-11); the default is Amateur. Each run is 12000
app updates ≈ 197 s of simulation at the 120 Hz physics step.
The scripted/hold legs below ran Amateur; the parked legs are Amateur
and the `pro-bot` legs Professional as labelled.

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

**Wall-clock + physics caveat (sf-8) — resolved (F13-C.3).** The
scripted sf-8 leg originally passed but needed ~24 min wall and
recorded the matrix's worst `dropped=53955` plus a transient
`peak=17070 m/s` spike. Reproduction with instrumentation showed a
physics defect, not mere load: fragment angular kicks (tiny cuboid
inertia → huge ω) inflated the next contact's approach speed via the
ω×r term, and the banger transfer launched each successive
generation of fragments at that magnitude — ~4×10⁶ m/s within ~10
frames. Hypervelocity fragments tunneled city-wide, breaking 3915
props and flooding the solver; fragment blowback produced the car's
17070 m/s spike and a below-world fail. Banger bodies now carry
linear/angular speed caps and the activation severity/launch are
clamped. Re-run: **~70 s wall**, `status=pass`, `peak=30.1 m/s`,
`dropped=0`, 3 props broken, race progressing `cp=4/8 pos=1/7`. The
caps are an implementation choice — authored records carry no speed
limit. Its hold leg restart-looping (rs=13, countdown) is unchanged —
the fast blind start wrecks before steering matters.

## Stationary-control and Professional legs (F13-C.2)

Two gaps the matrix left open: the `Hold` leg drives blind at full
throttle (no true stationary control), and Professional rosters were
audited but never driven. F13-C.2 adds the `--parked` driver — the
local participant holds its handbrake on the grid for the whole
session — and ran both new legs on the retail install at commit
`a7d2797`, `--frames 12000`.

**Parked control (Amateur)** — the events where opponents finished
in the C.1 matrix, re-run with the local participant stationary. The
parked car never drives itself (`peak` ≤4.7 m/s is impact shove, not
drive — the field physically pushes it; `moved` 1–8 m is the same
contact displacement plus the spawn settle):

| event | local cp / moved | results | opponents resolved |
| --- | --- | --- | --- |
| london-0 | 0/5, 2 m | 3 | 3/4 finished |
| london-2 | 0/4, 5 m | 0 | 0/7 (omax 4) |
| sf-0 | 1/6, 8 m | 4 | 4/6 finished |
| sf-2 | 0/9, 4 m | 1 | 1/6 finished |
| sf-3 | 0/6, 2 m | 2 | 2/5 finished |
| sf-4 | 0/7, 1 m | 1 | 1/5 finished |
| sf-5 | 0/6, 6 m | 0 | 0/5 (omax 5) |

All 7 `status=pass`, `pos=` last on every leg, `dup=0`. The control
sharpens AC04: **opponents finish with ledger results while the local
participant contributes nothing** — 11 finishes across 5 events,
including sf-3 where the scripted leg's local win had masked the
field. The one `cp=1/6` (sf-0) is the parked car being shoved through
a checkpoint trigger by opponent contact — a pushed crossing is a
real crossing, so the count is honest, not a driver artifact.

**Professional scripted legs** — the first Pro runtime rows:

| event | outcome | field | conditions |
| --- | --- | --- | --- |
| sf-0 | `finished place=6/7`, 5 opp finished | vpcaddie+vpbug — Pro wires `6opp 6rt` cleanly (the `6opp/7tbl` mismatch is amateur-only) | lt01 cloudy-morning |
| sf-3 | `finished place=1/7`, 0/6 resolved | vpvwcup/vpbullet/vpauditt | lt06 foggy-noon |
| london-0 | playing at cap, `cp=3/5`, `rs=2` | vpcoop2k ×6 | lt01 cloudy-morning |

Pro measurably selects different authored content — different rosters
(sf-3's amateur `vpbug`-heavy field becomes `vpvwcup`/`vpauditt`),
different environment presets (sf-0 lt00→lt01, sf-3 lt05→lt06), and
on sf-0 a field that beats the scripted driver (place 6 of 7 vs the
amateur field the same driver matched). london-0's two restarts are
the scripted driver wrecking under authored damage → `RestartEvent`,
not a session defect.

## Anomalies kept visible

- **Physics step drops under contention:** `dropped=` counts physics
  substeps skipped under load — hold-sf-4 55917, hold-london-9 53155,
  bot-sf-4 16348 (the player *still won* that one), bot-sf-7 13535,
  hold-london-5 4914. Simulation stays finite and correct; the drops
  mean effective sim rate sagged below 120 Hz on those runs. The
  worst offender, bot-sf-8's 53955 + `peak=17070 m/s`, was a real
  hypervelocity-fragment defect — diagnosed and bounded in F13-C.3;
  it no longer appears on re-run.
- **End-of-run grounding:** `wheels=0/4` on hold-london-4 and
  hold-sf-4 (car ended propped/airborne), 2/4 bot-sf-2, 3/4
  hold-sf-7 — disclosed poses, not crashers.
- **London late-catalog grind:** even uncontaminated, london 4-11
  opponents top out at 2-5 gates in ~197 s with heavy escape/re-anchor
  use (hold-london-4: 137 escapes / 31 reanchors across 7 cars) — the
  deep-city courses beat the opponent controller's current skill.
  Whether that matches retail difficulty is **unknown** — a F15-B
  residual, not excused here.
- **Roster mismatch (authored):** sf-0 amateur spawns 6 opponents
  though the table authors 7 (`6opp/7tbl`) — disclosed above, kept in
  the denominator.

## Evidence → acceptance mapping

- **F13-AC01** (discoverable + deps load): `events` 24/24 ready,
  `race-defs` 48/48 built, `opponents` all rosters wired; every row
  ran the production session to `status=pass`. Structural + Amateur
  runtime legs done; **Professional runtime legs started (3/24 events
  driven scripted at Pro — sf-0/sf-3/london-0 above), full Pro matrix
  open.**
- **F13-AC02** (ordering/crossing rules): AnyOrder freedom verified
  (CHK-1); 36 race tests cover high-speed/wrong-height/repeat
  crossings, teleport/reset segment invalidation, timeout edge ticks.
- **F13-AC03** (independent progress): per-participant `opps=` rows
  diverge live; `ordered_multi_lap_participants_stay_independent`,
  `a_non_local_resolution_does_not_end_the_local_race` cover it —
  sharpened by the parked legs: the local participant's progress and
  the field's results are fully independent (a stationary local earns
  nothing while opponents resolve).
- **F13-AC04** (opponents start/progress/finish): **demonstrated,
  control leg included** — opponent finishes on 7 amateur events
  (london-0/2, sf-0/2/3/4/5), and 11 finishes across 5 events under a
  *stationary* local participant; results compare against real
  participants (`pos=` vs full field, parked = last).
- **F13-AC05** (restart/recovery don't bypass): restart rebuilds the
  generation (25-cycle soak, `dup=0`, results scoped per generation);
  synthetic `restart_removes_the_race_resource`,
  `restart_from_results_rebegins_the_session`, teleport/reset invalid
  crossing tests; recovery-penalty fidelity remains a F05/F15
  research item.
- **F13-AC06** (catalog matrix): **this document** — 24/24 rows
  attempted, 48/48 legs `status=pass` (sf-8's ~24-min wall-clock
  outlier was a physics defect, bounded in F13-C.3 — ~70 s on re-run),
  finishes and stalls reported per event per driver; plus the C.2
  parked-control legs and the first Pro rows.

## Residuals / honest limits

- Professional coverage is partial: 3 scripted Pro legs (sf-0/sf-3,
  london-0) — the remaining 21 events' Pro rosters are structurally
  audited, not driven; no Pro hold/parked legs yet.
- sf-8's ~24-min wall-clock outlier was a hypervelocity-fragment
  cascade — diagnosed and bounded in F13-C.3 (banger speed caps +
  clamped activation severity; re-run ~70 s, `dropped=0`,
  `peak=30.1 m/s`). The scripted driver's pace there (cp=4/8 at cap)
  is a driving-quality question, not a physics defect.
- The parked control is stationary, not physics-frozen — the field
  shoves it (sf-0 `cp=1/6` on contact push-through; `moved` up to
  8 m). That is correct swept-trigger behavior, disclosed.
- ~100-200 s/event budget leaves genuinely-long races unresolved;
  opponent non-finishes on deep courses are budget- and
  controller-skill-bound, not proven impossible.
- Blitz (F12), Circuit (F14), Crash Course (F21) rows are audited for
  structure only; their runtime matrices belong to those features.
- No original-fidelity claim: engine self-metrics only. Difficulty,
  pacing and AI competence vs retail are unverified.
