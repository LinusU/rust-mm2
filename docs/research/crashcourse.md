# Crash Course data audit (F21-A.1)

Measured on the retail install (`fnv1a64:e91e6cd4b2ae30d9`,
read-only) with `mm2-inspect crash-course <install>`. This document
records what the files actually contain; every classification that is
not directly evidenced is marked *inferred*.

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

Observed correlations (inferred, not verified). `tail[k]` below is
the k-th value after `AmbDensity` (file column k+5); header names
drift per file, so a name is only claimed where the file that sets
the value also authors one.

- `tail[0]` (`Misc` on london `crash1`, sf `crash2`/`crash12`;
  `cornerspeed` on london `crash6`): nonzero only on `Event 4` rows —
  40 on london's pair (`crash1` `corner`, `crash3` `exam1_1`), 50 on
  all three sf `Event 4` rows (`crash2` `corner0waypoints`, `crash3`
  `exam1_1`, `crash12` `final`). A speed or target for the cornering
  family.
- `tail[1]`: 1 on exactly four rows — london `crash4` `map` and sf
  `crash4` `oneeighty`, both difficulties (`[0,1,0,0,0,0]`). No file
  that sets it names the column (london `crash6`'s header calls this
  position `chkflags` but its own rows carry 0), and the e9/e7
  pairing defeats a family reading — meaning unknown.
- `tail[2]` (`numopp` per london `crash6`'s header): 1 on every
  `Event 2` (follow) row *and* on both sf `Event 8` (stop) rows —
  `crash6` `stop`, `crash7` `exam1_2`. Every lesson containing a
  `numopp`=1 row wires exactly one `[Opponent]` lead car (the sf stop
  rows wire `vpford`). The converse does not hold — london `crash3`
  and `crash11` wire an opponent with no `numopp` row (their
  sub-events are e4/e7) — so the flag marks the sub-event that uses
  the wired car, not the lesson's wiring alone.
- `tail[3]` (unnamed on every file that sets it): 1 on three sf rows
  — `crash10` `follow` amateur only (the `_p` row authors 0) and
  `crash11` `exam1_3` on both difficulties.
- `tail[4]`/`tail[5]` are 0 on every retail row.

## The `Event` column — inferred decode

The integer correlates with the waypoint file's lesson family
across all 26 retail lesson tables. The mapping below is a measured
correlation, not a recovered enum — no documented structure names it
(mm2hook recovers no crash-course object), so the code reports
`LessonObjective` as inferred and keeps unknown codes raw.

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

## Open questions

- The `Event` enum's real semantics (UNK-35): does the runtime
  switch on it for evaluator behaviour, and do codes 1/6 exist in
  any build?
- Tail-column semantics: `Misc`/`cornerspeed` is corner-family-bound
  and `numopp` tracks the sub-events that consume a wired opponent,
  but `tail[1]`/`tail[3]` have no family reading yet.
- Whether `Checkpoints` (1 on every retail row) names waypoint gates
  or something else.
- Instruction/subtitle/audio linkage — no authored reference in
  these tables points at instruction content; where lesson text
  lives is unrecovered.
- Professional `TimeofDay`/`Weather` divergence — deliberate harder
  conditions or revision drift.
