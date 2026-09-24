# Race coverage matrix

F13-C.1's published account of what the authored Checkpoint catalog
actually does under the production headless runtime — measured, not
claimed — plus F13-C.2's stationary-control and first Professional
legs, F13-C.4's complete Professional matrix and F13-C.5's
Professional hold legs.
Every number below comes off the fingerprinted retail install
(`fnv1a64:e91e6cd4b2ae30d9`, read-only): the audits and the scripted/
hold matrix at commit `1791df0`, the parked legs and first
professional legs at commit `a7d2797`, the full Professional matrix
at commit `11af1ea`, the Professional hold legs at commit `7fb6cf6`
(docs-only over `11af1ea` — identical code); all runs on 2026-09-24
on Apple M1 / Metal. The
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
The Amateur legs are the C.1 scripted/hold matrix and the C.2 parked
control; every `pro-*` leg below — the C.2 scripted legs, the C.4
scripted/parked matrix and the C.5 hold legs — ran `--pro`.
Raw per-leg logs plus `results.txt` stay local, uncommitted per the
large-capture rule: C.4 in `/tmp/mm2-pro-matrix/`, C.5 in
`/tmp/mm2-pro-hold/` (each leg log stamps the code commit it ran).

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

**Professional scripted legs** — the first Pro runtime rows (commit
`a7d2797`; superseded as evidence by the uniform C.4 matrix below —
the banger-cap fix in between legitimately moved trajectories):

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

## Professional runtime matrix — 24 events × 2 drivers (F13-C.4)

All 48 legs at commit `11af1ea`, `--pro`, `--frames 12000`, on the
same retail install: every event scripted (`--bot`) **and** parked
(`--parked` — the stationary control). `P-F` = local finish, `oF` =
opponents reaching `Finished` with a ledger result, `omax` = deepest
gate any opponent reached. **48/48 `status=pass`, rc 0.**

### London (`world=city/london.psdl`, Professional)

| event | scripted: phase, cp, results, opp | scripted: P-F, rs, omax | parked: cp, results, opp | parked: moved, omax |
| --- | --- | --- | --- | --- |
| 0 | results 5/5 r4 3/6 | **4th**, rs0, omax5 | 0/5 r5 5/6 | 2 m, omax5 |
| 1 | playing 0/7 r0 0/6 | –, rs5, omax0 | 0/7 r0 0/6 | 11 m, omax7 |
| 2 | results 4/4 r1 0/7 | **1st**, rs0, omax3 | 0/4 r2 2/7 | 5 m, omax4 |
| 3 | playing 1/6 r0 0/7 | –, rs8, omax1 | 0/6 r0 0/7 | 19 m, omax2 |
| 4 | playing 0/6 r0 0/7 | –, rs2, omax0 | 0/6 r0 0/7 | 5 m, omax2 |
| 5 | playing 0/3 r0 0/7 | –, rs1, omax0 | 0/3 r0 0/7 | 9 m, omax2 |
| 6 | playing 0/8 r0 0/7 | –, rs5, omax1 | 0/8 r0 0/7 | 4 m, omax2 |
| 7 | playing 1/8 r0 0/6 | –, rs0, omax5 | 0/8 r0 0/6 | 10 m, omax5 |
| 8 | playing 2/9 r0 0/6 | –, rs1, omax4 | 0/9 r0 0/6 | 29 m, omax4 |
| 9 | playing 0/5 r0 0/7 | –, rs3, omax1 | 0/5 r0 0/7 | 3 m, omax3 |
| 10 | playing 2/8 r0 0/6 | –, rs2, omax2 | 0/8 r0 0/6 | 3 m, omax3 |
| 11 | playing 4/6 r0 0/6 | –, rs0, omax3 | 0/6 r0 0/6 | 5 m, omax4 |

### San Francisco (`world=city/sf.psdl`, Professional)

| event | scripted: phase, cp, results, opp | scripted: P-F, rs, omax | parked: cp, results, opp | parked: moved, omax |
| --- | --- | --- | --- | --- |
| 0 | playing 2/6 r0 0/6 | –, rs1, omax2 | 1/6 r4 4/6 | 8 m, omax6 |
| 1 | playing 2/8 r0 0/6 | –, rs1, omax2 | 0/8 r0 0/6 | 3 m, omax7 |
| 2 | playing 1/9 r0 0/7 | –, rs1, omax2 | 0/9 r1 1/7 | 1 m, omax9 |
| 3 | results 6/6 r1 0/6 | **1st**, rs0, omax5 | 0/6 r1 1/6 | 9 m, omax6 |
| 4 | playing 5/7 r0 0/5 | –, rs1, omax5 | 0/7 r0 0/5 | 14 m, omax7 |
| 5 | playing 2/6 r0 0/5 | –, rs1, omax2 | 0/6 r0 0/5 | 6 m, omax5 |
| 6 | playing 3/6 r0 0/4 | –, rs0, omax3 | 0/6 r0 0/4 | 5 m, omax3 |
| 7 | playing 2/4 r0 0/6 | –, rs0, omax2 | 0/4 r0 0/6 | 3 m, omax2 |
| 8 | playing 6/8 r0 0/6 | –, rs0, omax5 | 1/8 r0 0/6 | 22 m, omax5 |
| 9 | playing 0/10 r0 0/6 | –, rs1, omax0 | 0/10 r0 0/6 | 6 m, omax2 |
| 10 | **countdown** 0/5 r0 0/6 | –, rs6, omax0 | 0/5 r0 0/6 | 3 m, omax3 |
| 11 | playing 2/11 r0 0/6 | –, rs1, omax2 | 0/11 r0 0/6 | 9 m, omax3 |

### What the Pro matrix shows

**The whole catalog runs at Professional.** 48/48 legs pass with the
authored `.aimap_p` rosters and parameter blocks — no load failure,
panic, or sanity-trip. Every roster resolved and spawned (4–7
opponents per event, authored sizes vary).

**Opponents finish at Pro with the local participant parked.** 13
opponent finishes with ledger results across 5 events while the local
car contributes nothing: london-0 (5/6), london-2 (2/7), sf-0 (4/6),
sf-2 (1/7 — a `vpford` earning all 9 gates), sf-3 (1/6). (16 counting
both driver legs — london-0's scripted leg added 3 opponent finishes
while the local scripted driver raced to place 4; the parked-only
count is 13.) Versus the amateur parked legs, the Pro field finishes
more often on london-0 (5/6 vs 3/4) and london-2 (2/7 vs 0/7),
matches on sf-0 (4/6), sf-2 (1/7 vs 1/6) and sf-5 (0/5), and finishes
less often on sf-3 (1/6 vs 2/5) and sf-4 (0/5 vs 1/5) — authored
tuning and roster differences, measured not fidelity-verified.

**The scripted driver finishes 3 events at Pro**: london-0 `place=4`
behind three opponent finishes, london-2 `place=1`, sf-3 `place=1`.
At amateur the same driver won sf-3/sf-4 and placed 3rd on london-0 —
Pro is measurably harder for it (sf-0's `place=6/7` finish in the
a7d2797 leg became `cp=2/6` at cap here; london-0's at-cap `rs=2` leg
became a `place=4` finish — the F13-C.3 banger caps moved both
trajectories, and both directions are honest evidence of the same
machinery).

**Restart contamination at Pro matches the amateur pattern.** The
scripted driver wreck-loops on london-1 (rs5), london-3 (rs8),
london-6 (rs5) and sf-10 (rs6 — still in `Countdown` at cap, same
class as the amateur sf-8/london-10 hold loops): authored damage →
`RestartEvent` before the scripted line is learned. Every `rs>0` row
is a driver outcome; the parked legs on the same events show the
field progressing regardless (omax 2–7).

**The parked car is an honest stationary control.** `cp=0` on 22 of
24 legs; sf-0 `cp=1/6` and sf-8 `cp=1/8` are the same opponent-shove
through a trigger measured at amateur — real crossings, disclosed.
`peak` ≤10.3 m/s and `moved` ≤29 m are contact displacement, not
drive. london-4's parked leg is the outlier worth naming: the spawn
sits on elevated ground (y≈4.9) and field contact repeatedly punts
the stationary car over the edge — `rcv=209f/209r` shows the recovery
anchor catching every fall and the car ending `wheels=4/4` 5 m from
spawn. The machinery holds; the frequency is the control's own
artifact (a parked car cannot dodge).

**Physics health stays clean under the F13-C.3 caps.** Worst
`dropped=` is 345 (sf-3 scripted); the amateur matrix's 16k–56k drop
legs do not recur at Pro. No velocity spike anywhere: peak ≤34.2 m/s
across all 48 legs.

## Professional hold-driver legs — 24 events (F13-C.5)

The third driver at Pro: `Hold` settles ≤2 s, then holds full
throttle with no steering — it *drives* blind, so it is a field
participant that happens to be a bad driver, not a control. All 24
legs at commit `7fb6cf6` (code-identical to `11af1ea`), `--pro`,
`--frames 12000`. **24/24 `status=pass`, rc 0** — the Professional
matrix is now complete: 72 legs = 24 events × scripted + parked +
hold.

### London (`world=city/london.psdl`, Professional, hold)

| event | phase, cp, results, opp | P-F, rs, omax |
| --- | --- | --- |
| 0 | playing 1/5 r4 4/6 | –, rs0, omax5 |
| 1 | playing 1/7 r0 0/6 | –, rs0, omax7 |
| 2 | playing 0/4 r0 0/7 | –, rs5, omax1 |
| 3 | playing 0/6 r0 0/7 | –, rs17, omax0 |
| 4 | playing 0/6 r0 0/7 | –, rs0, omax5 |
| 5 | playing 0/3 r0 0/7 | –, rs0, omax2 |
| 6 | playing 0/8 r0 0/7 | –, rs0, omax3 |
| 7 | playing 0/8 r0 0/6 | –, rs0, omax5 |
| 8 | playing 1/9 r0 0/6 | –, rs8, omax1 |
| 9 | **countdown** 0/5 r0 0/7 | –, rs9, omax0 |
| 10 | playing 0/8 r0 0/6 | –, rs25, omax0 |
| 11 | playing 1/6 r0 0/6 | –, rs0, omax3 |

### San Francisco (`world=city/sf.psdl`, Professional, hold)

| event | phase, cp, results, opp | P-F, rs, omax |
| --- | --- | --- |
| 0 | playing 2/6 r4 4/6 | –, rs0, omax6 |
| 1 | playing 2/8 r0 0/6 | –, rs9, omax2 |
| 2 | playing 1/9 r1 1/7 | –, rs0, omax9 |
| 3 | playing 1/6 r2 2/6 | –, rs0, omax6 |
| 4 | playing 2/7 r1 1/5 | –, rs0, omax7 |
| 5 | playing 0/6 r0 0/5 | –, rs0, omax6 |
| 6 | playing 0/6 r0 0/4 | –, rs0, omax3 |
| 7 | playing 0/4 r0 0/6 | –, rs0, omax2 |
| 8 | playing 1/8 r0 0/6 | –, rs0, omax5 |
| 9 | playing 0/10 r0 0/6 | –, rs0, omax5 |
| 10 | playing 0/5 r0 0/6 | –, rs0, omax3 |
| 11 | playing 0/11 r0 0/6 | –, rs6, omax0 |

### What the hold legs add

**The field finishes around a blind driver too.** 12 opponent
finishes with ledger results across 5 events while the hold car raced
and never resolved (london-0 4/6, sf-0 4/6, sf-2 1/7, sf-3 2/6,
sf-4 1/5). The amateur hold legs produced 12 across 7 events
(london-0 2, london-2 2, sf-0 3, sf-2 2, sf-3 1, sf-4 1, sf-5 1) —
per event the Pro field finished more often on london-0/sf-0/sf-3,
less on london-2/sf-2/sf-5, level on sf-4; authored roster and tuning
differences, measured not fidelity-verified.

**The blind driver wrecks hardest.** Restart counts are the worst of
the three drivers — london-10 rs25 (matching amateur hold's 25 on the
same event), london-3 rs17, london-9 rs9 (still `Countdown` at cap —
same class as amateur sf-8's rs13 and Pro scripted sf-10's rs6),
london-8 rs8, sf-1 rs9, sf-11 rs6. Every one is a driver outcome —
the same events' parked legs show the field progressing regardless.
london-4 repeats its parked-leg anomaly verbatim: the hold car on the
elevated spawn gets punted over the edge and recovered
`rcv=0w/209f/209r`, ending `wheels=4/4` — the loop is the spawn
ground's, not the driver's.

**Record honesty holds at the extremes.** `dup=0` wherever the
results/damage pipeline ran; on london-3/london-9 the whole `dmg=`…
`txl=` tail is absent because `impacts=0` — the activity gating keeps
idle generations bit-identical, and `results=0` means there was
nothing to duplicate. `wheels=0/4` on london-9 (never left countdown)
and london-11, `3/4` on london-2 — disclosed end poses. `dropped`
≤21; `peak` ≤48.6 m/s (london-2 — a blind full-throttle car down a
hill; bounded, no spikes).

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
  runtime legs done; **Professional runtime legs complete at all
  three drivers (72/72 `status=pass` — the C.4 scripted+parked
  matrix plus the C.5 hold legs).**
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
  control leg included at both difficulties** — opponent finishes on
  7 amateur events (london-0/2, sf-0/2/3/4/5), 11 finishes across 5
  events under a *stationary* local participant; at Professional, 13
  opponent finishes across 5 events under the parked control (16
  across both driver legs — london-0's scripted leg added 3) and 3
  local scripted finishes (london-0 place 4, london-2/sf-3 place 1);
  results compare against real participants (`pos=` vs full field,
  parked = last).
- **F13-AC05** (restart/recovery don't bypass): restart rebuilds the
  generation (25-cycle soak, `dup=0`, results scoped per generation);
  synthetic `restart_removes_the_race_resource`,
  `restart_from_results_rebegins_the_session`, teleport/reset invalid
  crossing tests; recovery-penalty fidelity remains a F05/F15
  research item.
- **F13-AC06** (catalog matrix): **this document** — 24/24 rows
  attempted, 48/48 amateur legs `status=pass` (sf-8's ~24-min
  wall-clock outlier was a physics defect, bounded in F13-C.3 — ~70 s
  on re-run), 72/72 professional legs `status=pass` (C.4
  scripted+parked, C.5 hold); finishes
  and stalls reported per event per driver per difficulty.

## Residuals / honest limits

- Professional coverage now spans the full catalog at all three
  drivers (C.4 scripted + parked, C.5 hold — 72/72 pass); the
  scripted legs' divergent outcomes vs the a7d2797 first legs are
  disclosed in the C.4 section.
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
