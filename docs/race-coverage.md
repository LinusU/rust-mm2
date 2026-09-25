# Race coverage matrix

F13-C.1's published account of what the authored Checkpoint catalog
actually does under the production headless runtime — measured, not
claimed — plus F13-C.2's stationary-control and first Professional
legs, F13-C.4's complete Professional matrix, F13-C.5's
Professional hold legs and F13-C.6's deep-course extended-budget
probe — plus the Circuit (F14) Amateur matrix with its two
post-repair reruns and the Professional legs, in the section at the
bottom.
Every number below comes off the fingerprinted retail install
(`fnv1a64:e91e6cd4b2ae30d9`, read-only): the audits and the scripted/
hold matrix at commit `1791df0`, the parked legs and first
professional legs at commit `a7d2797`, the full Professional matrix
at commit `11af1ea`, the Professional hold legs at commit `7fb6cf6`
and the deep-budget legs at commit `feccdc4` (both docs-only over
`11af1ea` — identical code); all runs on 2026-09-24
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
mm2-inspect event <install> --all [--city <c>] [--strict]         # the deep check on every row

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
`/tmp/mm2-pro-hold/`, C.6 in `/tmp/mm2-pro-deep/` (each leg log stamps
the code commit it ran).

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
- `event --all` (F11-C.2, this slice's commit): the per-event deep
  check run on **all 90 cataloged rows** — 45/city, every one `ready`,
  0 incomplete, 0 failed records, 0 failed `RaceDefinition`/
  `OpponentRoster` builds at either difficulty. `--strict` exits 2 on
  **96 authored anomalies**, none new in kind: the same 77 orphan
  `.opp` routes and the `sf/race0` 6-vs-7 mismatch, plus 18
  record-level diagnostics the catalog-wide audits count but don't
  attribute per row — the `AmbDenisty` header misspelling on 8
  `crash*Ndata{,_p}.csv` files, a missing `Filename` header label on
  6 of them (london `crash8` + sf `crash4`/`crash9` pairs), and 4
  short rows (8 fields of 9) skipped in london's `exam1_1.csv` — a
  `crash3` (midterm) waypoint file whose skipped tail rows are
  authored data, disclosed not repaired.

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
| 2 | playing 2/9 r1 1/7 | –, rs0, omax9 |
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
hill; bounded, no spikes). Ten legs banked at least one gate before
wrecking (cp ≤2); sf-8's `cp=1/8` at `moved=37 m` reads like the
parked legs' disclosed opponent-shove crossings rather than driven
progress — the other nine moved 105–589 m.

## Deep-course extended-budget legs (F13-C.6)

The matrices' disclosed residual was that the deep London/SF
non-finishes could be *budget*-bound: on 11 events the parked-Pro
field's leader never passed 3 gates inside the 197 s budget. This leg
re-runs exactly those events — london-3/4/5/6/9/10 and
sf-6/7/9/10/11 — at `--frames 24000` (~394 s simulated, 2× the
matrix budget), `--parked --pro`, commit `feccdc4` (docs-only over
`7fb6cf6` — code-identical to `11af1ea`, stamped in every leg log).
**11/11 `status=pass`, rc 0.** Raw logs + `results.txt` local at
`/tmp/mm2-pro-deep/`.

`omax` is the deepest gate any opponent banked; `Σc` sums every
opponent's banked gates; `rec` is the field's re-anchor teleport count
(`opp_rec=` — escapes are the separate per-opponent `e` field in the
`opps=` rows, not part of this sum). The 197 s column is the C.4 parked
leg.

| event | omax 197→394 s | Σc 197→394 s | rec 197→394 s |
| --- | --- | --- | --- |
| london-3 | 2 → 3 | 9 → 11 | 17 → 36 |
| london-4 | 2 → 5 | 2 → 5 | 35 → 82 |
| london-5 | 2 → 2 | 4 → 4 | 26 → 41 |
| london-6 | 2 → 3 | 10 → 13 | 21 → 44 |
| london-9 | 3 → 3 | 16 → 17 | 20 → 55 |
| london-10 | 3 → 4 | 12 → 18 | 13 → 37 |
| sf-6 | 3 → 6 | 12 → 16 | 5 → 18 |
| sf-7 | 2 → 3 | 12 → 16 | 10 → 30 |
| sf-9 | 2 → 2 | 9 → 9 | 18 → 60 |
| sf-10 | 3 → 3 | 18 → 18 | 8 → 15 |
| sf-11 | 3 → 3 | 17 → 17 | 7 → 12 |

**The stalls are predominantly controller-bound, not budget-bound.**
Doubling the simulated budget produced zero additional finishes
(`opp=0` everywhere, `results=0`) while the field's re-anchor count
went 180 → 430 (+139%) against summed gate clears 121 → 144 (+19%) —
the extra 197 s bought recovery churn, not progress. The leader
advanced on 6 of 11 events (london-4's 2→5 of 6 the largest move) and
plateaued on 5. sf-6 is the one near-finish: `vpauditt` banked all 6
checkpoints — the finish trigger arms at full clearance (RACE-7) —
and was still racing toward it at cap. Budget is a real but secondary
factor; the binding constraint on the deep courses is opponent driving
competence (escape/re-anchor churn), which is the F15-B residual, not
something a longer run fixes.

**Parked-control anomalies scale with budget, unchanged in kind.**
london-4's disclosed punt loop continued at the same rate (209 → 461
falls + recoveries, `rcv=0w/461f/461r`, `wheels=4/4`); the same
spawn-edge fall loop now shows on london-5 (`rcv=0w/348f/348r`, was
122f at 197 s), sf-9 (43 → 172f) and sf-11 (49f+7w → 304f+7w) — all
present at the lower budget at proportionally lower counts, the parked
car being a standing target for field contact near spawn. london-3's
parked car took `cp=1/6` at `moved=110 m` — the disclosed
opponent-shove mechanism (sf-0/sf-8) driven further by sustained
contact — plus 17 water recoveries (`rcv=17w`) and 10 ambient deaths
in the same spawn-adjacent pileup. Physics health: `dropped` ≤218
(london-3's pileup; ≤80 sf-9, else 0), `peak` ≤12.8 m/s — a shoved
parked car, no spikes.

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
  shoves it (sf-0 `cp=1/6` on contact push-through; `moved` 1–8 m on
  the amateur legs, ≤29 m at Pro, and 30–110 m on three doubled-budget
  legs — london-3's 110 m shove plus london-5 (58 m) and london-9
  (30 m), where the disclosed spawn-edge fall loops also displace the
  end pose — all contact/recovery displacement, `peak` ≤12.8 m/s).
  That is correct
  swept-trigger behavior, disclosed.
- The 197 s/event matrix budget was probed at 394 s on the 11
  deepest-stalling events (F13-C.6): zero added finishes, field
  re-anchors +139% against +19% banked gates — the deep-course
  non-finishes are predominantly controller-skill-bound (the F15-B
  residual), not merely budget-bound. Longer budgets remain untested
  beyond 394 s and original-fidelity pacing is unverified.
- Blitz (F12) and Crash Course (F21) rows are audited for structure
  only; their runtime matrices belong to those features. Circuit (F14)
  coverage is the section below.
- No original-fidelity claim: engine self-metrics only. Difficulty,
  pacing and AI competence vs retail are unverified.

# Circuit (F14) matrix — F14-A.2 Amateur legs + F14-A.4 Professional legs

## Denominator and instruments

The authored Circuit table is **10 events per city**
(`circuit0..9` in both `race/london/` and `race/sf/`, every row
`ready` under the production `CatalogEvent → RaceDefinition` builder at
Amateur and Professional; `circuit10`/`circuit11` exist on disk as
extra stems — circuit11 has `.opp`/`.pathset` but no `.aimap` — and are
kept visible as uncataloged extras, outside this denominator).

- Driver legs: scripted (`--bot`) and the parked control (`--parked`),
  Amateur only, `--frames 12000` (~197 s at 60 Hz).
- Command shape: `mm2 --mm2-path <install> --city <c> --event
  circuit:<i> --<driver> --headless --frames 12000`.
- Content: retail install `fnv1a64:e91e6cd4b2ae30d9`, Apple M1.
- Two runs exist. **v1** (`/tmp/mm2-circuit-matrix/`) ran at commit
  `0fe4787` and surfaced the stall signature. **v2**
  (`/tmp/mm2-circuit-matrix-v2/`) ran the same binary work-tree rebuilt
  with the F14-A.2 `densify_route` gate-coverage repair — every v2 log
  still stamps `commit=0fe478719d…` because the fix was uncommitted at
  run time; the code under test is `0fe4787` + the uncommitted repair.
  All times wall-clock on a shared M1; `status=pass` means the smoke
  record completed cleanly, **not** that the race was completable.

## What the matrix shows

40/40 legs `rc=0`, `status=pass`, `dup=0` at both versions. The
pre-fix analysis read **uniform field-wide plateaus at a fixed gate
index** on 7 of 20 events — every opponent clearing exactly the same
number of gates is a geometry/binding signature, not driving skill.
Two distinct root causes:

1. **`densify_route` re-paths could drop gate coverage** (london-6,
   london-9, sf-9): where a long authored `.opp` leg left the road
   corridor the nav re-path produced a drivable detour that no longer
   crossed a checkpoint cylinder the authored segment crossed
   (london-6 gate 0: authored line crosses at 4.7 m inside r10 on a
   flyover whose banger-dressed ramp has no routable BAI lanes; the
   re-path detoured around the block and missed by ~198 m). With the
   F14-A.2 repair, a re-path that abandons authored gate coverage is
   rejected per leg; the verbatim segment stands instead.
2. **Authored `.opp` lines themselves miss gate cylinders**
   (london-2, london-4, london-5, sf-7): verbatim misses of 11–120 m
   against radii of 7–19, uniform across every `-a-*`/`-p-*` route of
   the event's roster. Physical `Checkpoint::crossed` cannot fire on a
   line that never enters the cylinder — so either the original bound
   AI Ordered progress to route position rather than trigger sweeps,
   or it crossed these gates by wander. That is an **unverified
   original rule** (UNK-11 class). Iteration 54 (F14-A.3) implemented
   a designed route-bound binding for it — see the v3 rerun below —
   while the original's own accounting stays unknown.

## Post-route-credit rerun (v3 — F14-A.3)

**v3** (`/tmp/mm2-circuit-matrix-v3/`) reran the four authored-miss
events × {`--bot`, `--parked`} at Amateur `--frames 12000` on the same
retail install with DSN-45 live — each Ordered opponent carries a
`RouteGateLine` binding every gate to its closest-approach arc on the
*driven* route, and `advance_race` banks the next required gate once
the driver's corridor-checked high-water arc passes the bound (player
and `AnyOrder` progress stay trigger-bound; route-derived clears count
separately as `/Nd`). All 8 legs `rc=0 status=pass`.

| event | leg | v2 plateau | v3 outcome |
|---|---|---|---|
| london circuit2 | bot + parked | 0c all-six | 0c all-six, **0 route clears** — gate 0's route bind sits ~700 m of driven arc in, and the field's permanent spawn pile-up never gets there (`opp_rec` 28–33, stuck peaks 900w, parked `rcv=203w`) — traversal-bound, not progress-bound |
| london circuit4 | bot | 9c omax | field 3–7c, one `/1d` — the missed g0/g9 banks by arc on the leader |
| london circuit4 | parked | 9c omax | **two opponents complete lap 0** (`0c/2l`, `/2d` each) — first opponent lap completions on an authored-miss event |
| london circuit5 | bot + parked | 2c all-four | 2c all-four, **0 route clears** — the field stalls short of gate 2's ~1050 m bind (heavy escape churn); traversal-bound |
| sf circuit7 | bot + parked | 5c all-six | **one opponent completes lap 0** (`2c`/`5c` on `2l`, `/10d`); the pack holds the v2 5c plateau at gate 5's ~985 m bind — the leader crosses, the rest do not reach it |

Read: the binding produces honest progress wherever the driven arc
reaches it — three opponents banked lap-0 completions across london-4
and sf-7 that physical triggers alone could never produce (the missed
gates clear by route arc; `/Nd` counts only route-derived clears — the
other gates cleared by real crossings as cars wandered into
cylinders). The two unchanged events are honest traversal residuals:
`0d` means the high-water arc never reached the first missed gate's
bound — the model earns by driving and cannot invent progress for a
field that churns at the start. `results=0` on every leg — no
inflated finishes. Whether the original accounts AI Ordered progress
this way remains UNK-11; this is the designed reading.

## Professional legs — 20 events × 2 drivers (F14-A.4)

The same matrix shape at the authored Professional block: every
cataloged event × scripted (`--bot`) + parked control at `--pro`,
`--frames 12000`, commit `e43ee20` (docs-only delta on `4a1db5a`;
all 40 logs stamp it), retail `fnv1a64:e91e6cd4b2ae30d9`, logs in
`/tmp/mm2-circuit-matrix-pro/`. **40/40 `rc=0 status=pass`, `dup=0`
on every leg.** This is also the first catalog-wide run with DSN-45
route credit live — the v3 Amateur rerun covered only the four
authored-miss events, so `/Nd` columns below are the first
route-derived-clear accounting for the other sixteen.

`laps` = opponents that completed ≥1 lap (each `opps` row's `Nl`
display ≥2 — cleared counts are per-lap, so `0c/2l` is a completed
lap 0, not a stall); `d` = route-derived clears summed over the
field; `omax` = deepest gate any opponent reached on the lap in
progress; `res`/`opp` = ledger entries / opponents resolved.

### London (Professional)

| event | scripted: cp, lap, res, opp | laps | omax (d) | parked: laps | omax (d) | note |
|---|---|---|---|---|---|---|
| circuit0 | 1/6 lap3/4 r1 1/7 | **7/7** | 6 (2) | **7/7** | 5 (2) | **the only Pro resolution + the deepest field**: `vpcoop2k` slot 0 finishes all 4 laps (`6c/4l/F`) while the local races on; 16/18 lap completions across the legs, four cars reached the final lap |
| circuit1 | 1/8 lap1/2 r0 0/6 | 3/6 | 7 (9) | 4/6 | 7 (10) | two more opponents one gate from lap 0 (`7c/8`) |
| circuit2 | 0/12 lap1/2 r0 0/6 | 0/6 | 0 (0) | 0/6 | 0 (0) | authored-miss + traversal stall verbatim; Thames churn — bot `rcv=240w/19f`, parked `223w/18f` |
| circuit3 | 1/9 lap2/4 r0 0/6 | 5/6 | 5 (5) | 5/6 | 5 (3) | field laps (one lap each); scripted reached lap 2/4 |
| circuit4 | 7/11 lap1/4 r0 0/5 | 1/5 | 7 (4) | 0/5 | 9 (3) | one lap-0 completion scripted; parked field reaches authored-miss g9 (`9c/2d` on `vpdb7`) — amateur v3's two completions don't recur |
| circuit5 | 2/6 lap1/4 r0 0/4 | 0/4 | 2 (0) | 0/4 | 2 (0) | authored-miss stall at the g2 bind — `0d`, field never arrives |
| circuit6 | 1/14 lap1/4 r0 0/7 | 1/7 | 1 (1) | 0/7 | 1 (0) | gate-1 plateau (six of seven `1c`) + one lap-0 completion scripted; Thames punt collateral now hits the scripted leg too — `rcv=243w`/`239w` |
| circuit7 | 4/10 lap1/4 r0 0/6 | 0/6 | 4 (0) | 0/6 | 3 (0) | field mid-course, all physical |
| circuit8 | 2/12 lap1/4 r0 0/6 | 0/6 | 5 (2) | 0/6 | 5 (2) | local `58f` fall churn on *both* legs (spawn-edge class) |
| circuit9 | 1/22 lap1/2 r0 0/6 | 0/6 | 6 (2) | 0/6 | 3 (1) | plateau persists — traversal residual |

### San Francisco (Professional)

| event | scripted: cp, lap, res, opp | laps | omax (d) | parked: laps | omax (d) | note |
|---|---|---|---|---|---|---|
| circuit0 | 2/9 lap2/4 r0 0/4 | **4/4** | 3 (8) | 3/4 | 8 (5) | whole field laps scripted; parked `vpcaddie` one gate from lap 0 (`8c/9`) |
| circuit1 | 6/10 lap1/4 r0 0/7 | 5/7 | 4 (51) | 6/7 | 4 (56) | **route-credit extreme**: the field completes lap 0 banked almost entirely by arc (`8–10d` per opponent on a 10-gate course) — the driven path threads almost no cylinders |
| circuit2 | 5/13 lap1/4 r0 0/6 | 0/6 | 10 (0) | 0/6 | 9 (0) | deepest all-physical run: `vpmustang99` `10c/13`, no laps yet |
| circuit3 | 4/11 lap1/4 r0 0/4 | 2/4 | 7 (5) | 2/4 | 8 (5) | field laps mid-course |
| circuit4 | 10/18 lap1/4 r0 0/5 | 0/5 | 15 (5) | 0/5 | 14 (8) | scripted `cp=10/18` — deepest local leg; opponents deep (`15c/18`) but no laps; local `56f`/`58f` fall churn |
| circuit5 | 3/9 lap1/4 r0 0/7 | 2/7 | 8 (10) | 4/7 | 5 (11) | one opponent completes *two* laps parked (`3l`); parked `rcv=222f` — a new spawn-edge fall loop (london-4 amateur class) |
| circuit6 | 1/9 lap1/4 r0 0/6 | 1/6 | 4 (21) | 1/6 | 5 (17) | one `vpdb7` lap-0 banked mostly by arc (`11d` cumulative) |
| circuit7 | 2/11 lap1/4 r0 0/6 | 0/6 | 5 (0) | 0/6 | 5 (0) | pack holds at the g5 bind verbatim — amateur v3's leader completion does not recur; `0d`, traversal-bound |
| circuit8 | 5/23 lap1/2 r0 0/4 | 0/4 | 11 (11) | 0/4 | 10 (17) | scripted `moved=1628 m` on the 23-gate course |
| circuit9 | 4/14 lap1/2 r0 0/6 | 0/6 | 4 (0) | 0/6 | 4 (0) | `4c` all-six plateau persists; only nonzero `dropped` legs (105/112), peak 35.1 m/s |

### What the Pro legs show

**The whole catalog runs at Professional.** Pro selects the authored
`.aimap_p` rosters and parameter blocks measurably: `diff=professional`
on every leg, distinct opponent lineups (london-0 fields 7 `vpcoop2k`
vs amateur's 7 `vpcoop`; sf-2 runs `vpford`/`vpauditt`/`vpmustang99`),
distinct lap counts (london `lap*/4` vs amateur `*/3` on circuit0/3–8;
`*/2` on circuit1/2/9 — matching the authored `NumLaps` table:
3am/4pro except c1,c2 2/2, c9 3/2; sf `*/4` except c8/c9 `*/2`).

**Multi-lap racing is real at Pro, not just gate churn.** Opponents
complete ≥1 lap on 10 of 20 events — london-0's whole 7-car field
laps (16/18 lap completions across the legs, three cars on the final
lap at cap), sf-0's all-four field laps scripted, sf-1's 5–6 of 7,
sf-5 one car two laps. Per-participant laps advance independently
(spreads like london-0 `4l/4l/4l/3l/4l/2l/2l`), the once-only
`F` resolution mints exactly one ledger result, and a remote finish
ends nothing local (`phase=playing` on london-0 scripted — correct
per F13-A.1). Exactly one `results=` entry across 40 legs — no
inflated finishes. Amateur london-0 resolved 4 opponents per leg at
the same budget: the extra authored lap plus Pro tuning measurably
slows field resolution — authored-difficulty evidence, not a defect.
The scripted driver never finishes a Pro circuit either (best:
london-0 `lap3/4`, sf-4 `cp=10/18`) where amateur produced scripted
wins — Pro is harder for it too, consistent with the checkpoint
matrix.

**Route credit is honest, not generous.** `/Nd` activity appears on
14 of 20 events and stays `0d` exactly where the field never reaches
the first missed bind — london-2 (g0 ~700 m arc), london-5 (g2
~1050 m), sf-7 (g5 ~985 m) — the traversal residuals are unchanged
because the model earns by driving and cannot invent progress. The
extreme is sf-1: not an authored-miss event, yet 5–6 of 7 Pro
opponents complete lap 0 banked `8–10d` of 10 gates each — the field
drives the full course but the cars' actual poses thread almost none
of the cylinders (corner-cutting/avoidance deviations vs the ~7–19 m
radii), so progress is nearly all route-derived. london-4's parked
field reaches authored-miss gate 9 by the same mechanism (`9c/2d`);
sf-6's one lap-0 banked `11d` cumulative.

**The parked control stays honest.** `cp=0` on all 20 parked legs;
`moved` ≤53 m and `peak` ≤15.6 m/s are contact/recovery displacement.
Fall-churn anomalies recur/extend: london-8 `58f` on both legs
(deterministic spawn-edge churn, driver-independent), sf-5 parked
`rcv=222f` (new — the london-4-amateur class of elevated-spawn punt
loops), london-6's Thames punt loop now tags the scripted leg too
(`243w`/`239w` — the restored-course field flows past and through
whatever sits at the start junction). sf-9 logs the matrix's only
`dropped` (105/112) and the worst `peak` 35.1 m/s — disclosed, still
`status=pass`.

## Pre-fix vs post-fix per event

`omax`/`omin` = best/worst opponent gates cleared; `cp` = the local
participant's gates (`x/total`); `res` = results-ledger entries;
`orec` = field re-anchors; `rcv` = local recovery counts (`w`=water,
`f`=fall). Bot legs list the scripted participant's `cp` first.

### London (10 events)

| event | v1 bot cp | v1 omax | v2 bot cp | v2 omax | v2 note |
|---|---|---|---|---|---|
| circuit0 | 1/6 `lap3/3` | 6 + **4 finishes** | 1/6 `lap3/3` | 6 + **4 finishes** | unchanged — the one fully-working Circuit: field resolves, ledger `res=4` both legs |
| circuit1 | 0/8 | 6 | 0/8 | 6 | field reaches gate6, no finish at 12000f; bot 0c |
| circuit2 | 0/12 | **0 all-six** | 0/12 | **0 all-six** | verbatim miss g0 (11.1 m vs r7) — authored-miss class, unchanged; parked `rcv=251w` punt-into-Thames churn persists |
| circuit3 | 3/9 | 8 | 3/9 | 8 | field deep (8/9 gates), no finish at budget |
| circuit4 | 3/11 | 7 | 3/11 | **9** | verbatim misses g0 (16 m) + g9 (32 m); v2 parked field reaches gate9 — plateau deepened to the authored-miss bound |
| circuit5 | 2/6 | **2 all-four** | 2/6 | **2 all-four** | verbatim miss g2 (36.4 m vs r11) — authored-miss class |
| circuit6 | 0/14 | **0 all-six** | 1/14 | **1 all-six** | **the repaired event**: gate 0 now crossed by every opponent; stall moved to gate 1 — traversal churn, not coverage (see below) |
| circuit7 | 2/10 | 3 | 2/10 | 5 | field mid-course both versions |
| circuit8 | 2/12 | 5 | 2/12 | 6 | field mid-course both versions |
| circuit9 | 1/22 | **1 all-six** | 1/22 | **1 all-six** | coverage restored (driven line now reaches every gate) but the 1c plateau persists — physical-traversal residual |

### San Francisco (10 events)

| event | v1 bot cp | v1 omax | v2 bot cp | v2 omax | v2 note |
|---|---|---|---|---|---|
| circuit0 | 7/9 `lap2/3` | 5 | 7/9 `lap2/3` | 6 | field racing deep |
| circuit1 | 2/10 | 4 | 2/10 | 9 | field mid-course |
| circuit2 | 3/13 | 10 | **9/13** | 9 | scripted leg improved markedly (was 3/13) |
| circuit3 | 7/11 | 7 | 7/11 | 9 | field mid-course |
| circuit4 | 4/18 | 9 | 4/18 | 10 | field mid-course |
| circuit5 | 3/9 | 7 | 3/9 | 7 | spread `0–7` — divergent field |
| circuit6 | 2/9 | 6 | 2/9 | 7 | field mid-course |
| circuit7 | 5/11 | **5 all-six** | 5/11 | **5 all-six** | verbatim misses g5 (34 m), g6 (120 m), g7 (63 m) — authored-miss class |
| circuit8 | 3/23 | 5 | 3/23 | 5 | 23-gate course, field at gate ~5 |
| circuit9 | 4/14 | **4 all-six** | 4/14 | **4 all-six** | coverage restored, plateau persists at gate ~4 — traversal residual |

## Anomalies kept visible (Circuit)

- **Post-fix london-6**: all six opponents bank exactly 1c — the
  coverage repair works end-to-end (gate 0 went from un-crossable to
  cleared by the whole field). The new plateau sits at gate 1 with
  heavy escape/re-anchor churn (`orec` 29–30, per-opponent stuck peaks
  445–900 s) and the parked control logs `rcv=294w` — the restored
  course now flows traffic past the parked grid car and punts it into
  the Thames repeatedly (v1: `5w`). A 2400-frame rendered pass shows
  the flyover span over water the course demands; the residual reads
  as traversal difficulty (F15-B controller-skill class), **not** a
  remaining route-coverage defect — the driven line now passes within
  4.7 m of all 14 gates.
- **london-2 parked `rcv=251w/10f`** (v1: 241w): stalled-field
  collateral — the whole field pinned at gate 0 keeps punting the
  stationary car into the river.
- **sf-7 field `rcv` fall churn** (bot `17f`, parked `21f`): elevated
  spawn-area falls on the stalled field, same disclosed class as the
  checkpoint matrix's spawn-edge loops.
- **End-pose `wheels=0/4`** on four scripted legs (london-1/2,
  sf-1/9) and modest `dropped` on two (london-4 159, sf-4 261) —
  disclosed poses and physics-step drops, all `status=pass`.
- **lap counters can exceed cleared gates** (e.g. london-1 `0c/2l`,
  london-0 `1c/3l`): opponent lap accounting advances with route
  progress, not gate clears — consistent with route-derived opponent
  progress being the plausible original model. F14-A.3's `RouteGateLine`
  is that model designed; v3 legs show `2l` rows on london-4/sf-7 whose
  lap-0 gates banked part-physically, part by route arc (`/Nd`).

## Evidence → F14 acceptance

- **F14-AC01** (discoverable + deps load): `race-defs` builds 10/10
  cataloged rows per city at both difficulties; all 20 ran the
  production session to `status=pass` ×2 drivers.
- **F14-AC06** (representative matrix): this section — 20/20 events ×
  {scripted, parked} at Amateur, pre- and post-repair published, plus
  the same 40 legs at Professional (F14-A.4, above) with the authored
  `.aimap_p` rosters/lap counts, the stall classes named, and the
  first Professional opponent finish (london-0). `status=pass` is a
  smoke-record outcome; field plateaus are disclosed per event above,
  not smoothed into a completability claim. Hold-driver legs remain
  open.
- **Residual classes (open, not claimed as done)**:
  (a) authored `.opp` lines missing gate cylinders (london-2/4/5,
  sf-7) — **addressed by DSN-45**: route-bound Ordered AI progress
  implemented in F14-A.3, v3 rerun shows lap-0 opponent completions on
  london-4 and sf-7; london-2/5 remain because their fields never
  reach the bind arcs — which folds them into class (b), and the
  original's own accounting stays UNK-11;
  (b) traversal stalls past restored coverage (london-2 @gate0-bind,
  london-5 @gate2-bind, london-6 @1, london-9 @1, sf-9 @4, sf-7 pack
  @gate5-bind) — opponent controller skill, F15-B class;
  (c) opponent deep-course pace generally — no Circuit finish reached
  except london-0's four.
