# City special content (`mm2-inspect specials`)

F28-A coverage report. It inventories, per city, the moving and
interactive content that generic static geometry and ambient traffic do
not cover — from the installation, not from a list of remembered
landmarks — and says which of it has a runtime.

```sh
mm2-inspect specials <install> [--city london|sf] [--strict]
```

## What it measures

| Row | Source | Runtime |
| --- | --- | --- |
| drawbridge leaves | `race/<city>/<city>_bridge[_<stem>].pathset` | `mm2_app::drawbridge` (WLD-26) |
| sailboats / ferries / Underground | `…_{sailboat,ferry,train}[_<stem>].pathset` | `mm2_app::movers` (WLD-28) |
| parked-car strips | `…_parkedcar[_<stem>].pathset` | `mm2_app::city::spawn_parked_cars` |
| water / recovery rooms | `city/<city>.water` | `mm2_app::water`, `mm2_app::recovery` |
| rail curves | `city/<city>.bai` tram/train curve counts | measured only |
| cable car | `va_cablecar_f` assets + `city/<city>.bai` tram curves + executable evidence | `mm2_app::cablecar` — partial: 4 cars on 2 circuits in SF, none in London; audio, networking and the init gate not reproduced; cable cars, ambient cars and participants are obstacles to each other (UNK-44) |

Every family file is found by name (default, `_<event stem>` overlays and
backup variants), parsed by the production `Pathset` parser, and each
path's model is resolved the way its manager resolves it (path name, else
the family default). An overlay is *reachable* when a catalogued event of
the city has that stem (`EventCatalog`); the report shows unreachable
leftovers but they cannot fail `--strict`. The expected default files are
`mm2_content::EXPECTED_SPECIAL_PATHSETS` (9 on retail); a city without
one — San Francisco has no Underground — is listed as absent, not
invented.

## Retail result (this checkout, `--strict`, exit 0)

- Expected default pathsets 9/9; 20 of 21 family files loadable.
  The one that is not is `london_bridge_blitz10` (truncated, and London's
  blitz table ends at `blitz9`, so nothing selects it — `drawbridge.md`).
  Two other overlays are unreachable but intact: `london_bridge_multi`,
  `london_parkedcar_test`.
- London: 4 bridge paths, 16 sailboat, 6 ferry (+6 per Crash Course 9/11/12),
  8 train lines, 91 parked-car strips; water level −3.8, 3 room refs.
  San Francisco: 1 bridge path, 16 sailboat (4 fall back to the default
  model), 1 ferry, 55 parked-car strips; water level −1.9, 3 room refs.
- Every path's model resolves. London's train paths are named for their
  lines, none a model, so all eight use `va_ug_l`.
- Rail curves: London 28 train curves on 28 of 540 roads, no tram; San
  Francisco 42 tram curves on 21 of 379 roads, no train.

## The cable car

`Midtown2.exe` creates cable cars during AI-map initialisation
(`"AIMAP.Init: Create the cable cars."` is pushed at `0x5358cf`; the
`va_cablecar_f` model name follows at `0x5359b5`; the manager's
`"Returning a NULL CableCar. Idx: %d"` accessor is at `0x534a3b`). The
model, its bound and its `aivehicledata` ship, as do the `cablecar*` and
`streetcable` audio clips, yet no pathset names a cable car and the model
is in neither city's ambient roster. Everything below marked
*(verified_original)* was read from the disassembly; `mm2-inspect
specials` shows the retail counts.

### Where the cars start (verified_original)

The init walks every BAI intersection and calls `0x54a200(intersection)`,
which counts the intersection's listed roads that carry tram rails on
the side facing away from it (`0x549960`: a nonzero per-side tram-curve
pointer; the train-rail twin at `0x549990`/`0x54a290` serves the next
init step, `"AIMAP.Init: Create the subways."`). Exactly one such road makes
the intersection a **tram-line terminus** and yields `(road id, ±1
direction)` — `+1` when the road starts at this intersection, else `-1`;
zero or two-plus yield nothing. The init allocates one `0x184`-byte
cable-car object per terminus and constructs it (`0x53f7c0`) from the
model name, its index, that road id (`+0xd0`) and the direction (`+0xd2`).
`Bai::tram_termini`/`tram_start` implement the selection. Retail: San
Francisco has **4** termini (21 tram roads, none one-sided), London **0**
— so London has no cable cars. The step only runs when a flag in the
init's parameter block (`+0x40`) is set; what sets it (per city, per game
type, or a detail option) was not traced, so whether every San Francisco
session spawns the four is **unknown** — the port always spawns them in
local sessions.

### Which road comes next (verified_original)

On reaching the end of a road the car asks `0x53fcb0` for the next one.
At the intersection it arrives at, the function counts the listed roads
whose forward tram-curve pointer is nonzero (`0x549960(+1)` for every
road, whichever way it points):

| count | next road |
| --- | --- |
| 1 (this road alone: a terminus) | the same road, the other way — the car turns round |
| 2 | the other tram road: the first tram road after this road's own slot in the intersection's list, wrapping |
| 4 | straight across: the second tram road after this road's own slot |
| 3, 5+ | none — the function reports failure |

The next road's direction is `+1` when its start intersection is the one
just reached, else `-1`. `Bai::tram_hop` is that function;
`Bai::tram_circuit` walks it until the car is back where it began. Retail
San Francisco has only counts 1, 2 and 4 (17 two-road joints, 4 termini
and one four-way crossing at intersection 153, where the two lines
cross), so every car's walk is deterministic and closes: **two circuits**
— 26 legs (2363 m of tram curve, 3037 m driven with the junction hops) and
16 legs (1502 m, 1908 m) — each shared by the cars from its two ends.

### How a cable car drives (verified_original, except where marked)

The object is an AI vehicle following the tram curve of its road
(`+0xdc` forward from point 0, `+0x78` backward from the last point),
between roads along a spline built from the end of one curve to the start
of the next (`0x558750`). Its speed controller (`0x540560`, run from the
update at `0x53fbb0`) uses `dt` = the frame time and these constants:

- cruise `0x5d6d5c` = **15 m/s** (target speed `15.0001`); look-ahead
  `0x5d6d58` = **25 m**;
- own acceleration `0x9c` = `1.5 + 2·rand()/32768` m/s², drawn once in
  the constructor;
- *distance remaining* (`0x540b70`) = road curve length − distance
  travelled − `+0x4c` (a model-derived offset, sign not pinned down), or
  `9999` while between roads;
- **obstacle** (`0x540790`, only on a road): any other AI vehicle on the
  road's lists or in the global list within a 30 m × 2 m probe ahead.
  Found, and not yet braking: decelerate at `v²/(2·(gap − 2.5))` to a stop
  2.5 m behind it (target speed 0); while braking the rate is kept;
- **end of road** (remaining within 0–25 m): ask `0x540a00` whether it may
  go on. Yes (and the answer sticks until the next road): keep or regain
  cruise. No: brake at `v²/(2·(remaining − 0.25))` to stop 0.25 m short,
  stand (speed snapped to zero below 0.2 m/s within 0.5 m), and ask again
  every frame. `0x540a00` returns yes outright if the road has a "no
  control" flag (`+0x160`), no if `+0x162`; otherwise by the road end's
  rule — `NeverStop`: yes; `TrafficLight`: the signal state for that road
  is green; `StopSign`: the queue logic at `0x54a3a0`/`0x54a4c0` (stand
  below 0.5 m/s within 1.5 m, wait for its turn); `AlwaysStop`: an error
  message and no;
- elsewhere: accelerate to cruise;
- after the integration the speed is clamped onto the target (snap within
  0.05 of it while decelerating, or beyond it while accelerating) and the
  car moves `speed·dt`.

Between roads nothing brakes the car, including at a terminus, where the
turnaround hop is its only reversal. When the car's model is not active
and its road is outside the player's AI bubble (`0x537f10`) the update
resets the model instead of moving it — whether cars far from the player
keep their place or restart is **unknown**; the port runs every car on
the session clock.

### What the port does (`mm2_app::cablecar`, `mm2_game::cablecar`)

- One kinematic body per terminus (`va_cablecar_f`, local-authority city
  sessions only), on the circuit its terminus' walk gives it; the two
  cars of a line share one route and start at their own ends.
- The route is each leg's tram curve, oriented to the leg's direction by
  the road's geometry, joined by smooth hops, sampled at 1 m
  (`CableRoute`). *Implementation choice*: the original's spline through
  the junction, and the Hermite it draws roads with, are not reproduced.
- The controller (`CableMotion`) is the one above, step for step. The
  road-end answer is the ambient-traffic junction controller
  (`Junctions::gate_approach`, the same gate lane followers use — its
  signal timing is itself a designed value, UNK-12), or open when the
  session has no ambient traffic. The car ahead is another cable car on
  the same circuit.
- **Which side of a road a car runs on is a choice**: forward (with the
  sections) takes the right-side tram curve, backward the left, so cars
  keep to the right. The file does not say: on all 21 retail tram
  roads the right curve is stored end→start and the left start→end (the
  same authoring artefact as the lane curves, `bai.md`), and which
  in-memory pointer (`+0xdc`/`+0x78`) is which parser side was not
  mapped. The two curves are 2–4 m apart.
- Evidence: unit tests for the walk, the route and every controller
  branch; `mm2_app` tests drive the production system in Avian and, on
  retail (`MM2_RETAIL`), run every car round its circuit (worst step
  0.125 m at 120 Hz, worst heading change 0.056 rad per step) and stop it
  on red at the end of each of the 84 legs. A windowed run on San
  Francisco (`--frames`, ambient traffic on) shows the four cars set off,
  reach 15 m/s, stop 0.3 m short of the line on red and go on green; a
  screenshot shows the model on the rails facing its direction of travel.

Obstacles beyond other cable cars (iteration 21): the sensor reads "any
other AI vehicle … within a 30 m × 2 m probe ahead", so ambient cars are
obstacles by the original's own rule. The port also counts every
`Player` participant (the driver and AI opponents) — an *enhanced
policy*, since whether the original's sensor sees the player's car is not
recovered, and a kinematic tram that shoves the player is not acceptable
either way. `CableRoute::blocker_gap` samples the route (1 m grain) from
the nose out to 30 m and takes, per car body, the sample nearest its
centre: it blocks when that sample is within the car's half-width + 1 m
horizontally (retail model: 1.32 m → 2.32 m; the original's probe width is
read as 2 m, the port's is a choice) and 2.5 m vertically (an overpass
does not stop a tram under it), and the controller sees the distance to
the centre less 2 m (a car's half-length). The corridor follows the
curve, so a car parked beside a bend is not ahead. Unchanged from the
original: no braking while in the junction between roads, constant-rate
braking to 2.5 m.

Ambient cars brake for a cable car (iteration 22). The original's probe
is symmetric — a cable car is one of "any other AI vehicle" to an
ambient car — so `drive_ambient` adds each tram as three centres to its
corridor blockers (`RoadObstacle`: the origin, plus a point 2 m inside the
nose and the tail, since the sense reads centres and a car's own
half-length is 2 m). An ambient car then queues behind a tram under the
same follow law as behind a car and stops at least a car's length short
of the body, tail first or head-on. The tram's extent is the nose
(`aabb.max.z`) taken as symmetric; the retail origin-to-tail distance is
not measured. A tram in the junction box also counts as an occupant of
it for the box yield. Synthetic lane only (no windowed run with retail
traffic).

Still unrecovered or unreproduced: audio triggers (`cablecar`,
`cablecarstart/stop`, `cablecarbell*`, `streetcable`), the `+0x40` gate,
the AI-bubble behaviour above, the `+0x4c` offset's sign (the port stops the nose at the line), and the
StopSign queue and `+0x160`/`+0x162` flag semantics on roads where they
matter (no retail tram road ends in a stop sign; all but one end under a
traffic light, the other `NeverStop`).

Out of scope here: the object-audio tables (`drawbridge`, `ferry`,
`subwaycar`, `trolleycable`, …) are sound emitters covered by
`movers.md` § Object audio, and the static `giz_*` landmark models are
ordinary placed props.
