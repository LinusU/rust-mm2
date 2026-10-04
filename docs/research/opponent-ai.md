# Race opponents (`aiVehicleOpponent` / `aiVehiclePhysics`)

How the original drives its race opponents. Everything below is
**verified_original**, read from the retail `Midtown2.exe`
(`llvm-objdump`, image base `0x400000`, file offset = `VA − 0x400000`
for `.rdata`/`.data`), unless a line says otherwise. Field offsets are
on the opponent object (`aiVehicleOpponent`, `0x9794` bytes in the
`aiMap` array, which begins with an `aiVehiclePhysics`); `carsim+N` is the opponent's own
`vehCarSim`.

The short version: an opponent is an ordinary simulated car driven by
an input-writing controller. It follows a chain of adjacent
intersections road by road, plans a 40-point path along the BAI road
geometry with an arc through every bend, detours around obstacles,
steers proportionally at that path and brakes from a fixed grip
budget. It does not rubber-band. It does reach into its own physics
state in a few places to stay upright and to recover (§ Physics
interventions).

## The car

- `aiVehicleOpponent::Init` (`0x53d040`) hands the `[Opponent]` row's
  geo name to `aiVehiclePhysics::Init` (`0x5591a0`), which passes it
  unchanged to `vehCar::Init` (`0x42be00`) → `vehCarSim::Init`
  (`0x4cbb70`), whose loader reads `tune/vehicle/<name>.vehcarsim`
  (directory method `0x4cbae0`, generic loader `0x4a1110`). The extra
  argument is the paint variant.
- **The opponent drives the player's tuning.** No `opp` token exists
  anywhere in the executable, and the retail aimap rows name plain
  geo ids (`vpcoop2k`, `vpdb7` …). The 23 `tune/vehicle/*_opp.vehcarsim`
  files (RACE-13) are not read by the shipped game.
- Every frame `aiVehiclePhysics::Update` copies the authored
  `maxThrottle` into `carsim+0x28c` (`0x55a6c6`), the throttle the
  controller drives at.
- Inputs go through the same slots the player's do — the network
  input decoder `0x43cc80` writes them: `carsim+0x1554` steering
  (±1), `carsim+0x2bc` throttle, `carsim+0x154c` brake,
  `carsim+0x1550` handbrake.
- The opponent's physics is the full car simulation at every distance.
  Only collision is level-of-detail (`0x53d3b4`): when the nearest
  player is within 200 m (`d² < 40000`) the car is declared to
  `dgPhysManager::DeclareMover` (`0x468360`) at level 3 (neighbouring
  rooms join its collision set), otherwise at level 2.

## The route (`.opp`)

`0x53d040` reads the `.opp` CSV:

| Row | Meaning |
| --- | --- |
| 0 | Start pose: `x y z` and the mislabelled `brake` column as a heading, `× 0.0174444` (3.14/180) into `carsim+0x250` |
| 1 … n−2 | Each point's PSDL room is looked up (`[0x654d7c]` vtable `+0x1c`); the room must hold a type-3 map component — an intersection — whose id becomes the next route entry. Otherwise `"ERROR: Opp: %d, Point - %d, Is not a intersection."` |
| n−1 | The finish position (`+0x9774`) |

The other `.opp` columns are not read.

`aiMap::FindRoadBetween` (`0x53a660`) returns the one road two
intersections share, or nothing — so consecutive route points must be
**adjacent** intersections. The original does no pathfinding between
`.opp` points; the authored file is the whole road-by-road route.

The retail files are written that way (**measured** 2026-10-04,
nearest BAI intersection centre per interior row): london 7770 of 7862
interior rows lie within 15 m of an intersection centre and 7229 of
7538 consecutive pairs are adjacent or the same intersection; sf 4414
of 4801 and 4248 of 4513. The 40–200 m spacing between rows is simply
one city block per row. The remaining pairs are mostly where the
nearest-centre approximation picks the wrong one of two close
junctions; whether any authored route really skips an intersection is
not checked.

Progress (`0x55c2d0`): each frame `aiMap` locates the car on a road or
an intersection (`0x537a80`). Entering the next route intersection
advances the index; at the end of the list a circuit (game type 3,
`[0x6b1e98]`) wraps to index 1 for the next lap, up to the race's lap
count (`[0x6b1e9a]`, `RegisterRoute` argument 5 — 1 outside circuits).
The current road and the next two (`+0x26c/+0x270/+0x274`, with
direction flags at `+0x268..`) are what the planner looks along.

## Authored parameters

The `[Opponent]` row parser (`0x555154`) is
`sscanf("%s %s %f %d %f %f %d %d %d %d %d %f")` into a `0x58`-byte
record whose fields are preset first (the defaults below).
`aiVehicleOpponent::Update` (`0x53d490`) passes them to
`aiVehiclePhysics::RegisterRoute` (`0x559660`), and nothing writes
them afterwards.

| Col | Field | Default | Stored | Consumed by |
| --- | --- | --- | --- | --- |
| 0 | `maxThrottle` | 1.0 | `+0x9760` | the throttle whenever the car is not braking |
| 1 | `unkFlag` | 0 | `+0x9758` | no reader found |
| 2 | look-ahead (m) | **50** | `+0x976c` | obstacles farther along the path than this are ignored (`0x55ddc0`) |
| 3 | `cornerBrakingThreshold` | 0.7 | `+0x9768` | the brake demand a corner must exceed before the car brakes |
| 4 | `avoidTraffic` | 1 | `+0x975a` | path sweep tests ambient cars |
| 5 | `avoidProps` | 1 | `+0x975b` | path sweep tests props within 500 m |
| 6 | `avoidPlayers` | 1 | `+0x975c` | path sweep tests players |
| 7 | `avoidOpponents` | 1 | `+0x975d` | path sweep tests other opponents |
| 8 | `weirdPathfinding` | 0 | `+0x9759` | candidate-path preference (§ Path planning) |
| 9 | `cornerSpeedMultiplier` | **1.0** | `+0x9764` | multiplies every corner **speed** |

The parser's defaults differ from the ones the community headers give
`RegisterRoute` (75 and 2.0 for cols 2 and 9). Every retail race row
authors all ten values, so the defaults never decide a retail race car
(only `race/sf/stunt0.aimap` authors a short row).

## Path planning (every frame)

`0x55be60` → `0x55d7b0` → `0x55dad0` builds the car's path: up to 40
points of `0x24` bytes each, forward along the current and next two
roads.

- **Straights** follow the road's BAI section frames (`0x561890`).
- **Bends** are precomputed per road at load (`0x546360`), from the
  heading change at each section. A section turning more than 0.7 rad
  is a bend; when the next section lies within 10 m along the road
  (`0x547320`, the cumulative-distance difference) the two turns are
  summed. Two gentler turns within 10 m whose sum passes 0.7 rad are a
  bend too. Each becomes a `0x44`-byte corner record: section index,
  signed turn, inside side and an apex — where the inner edge lines,
  pulled 1.5 m (`1.5 ×` the lateral axis) into the road, meet; a lone
  sharp section takes its edge vertex instead.
- **Per car, per frame** the radius through each bend (`0x546bc0`):
  `w` is the car's lateral distance from the apex, clamped to
  `[3, 2·width − 1.5]`, and
  `R = w / (1 − sin((π − |θ|)/2))` — the arc tangent to both legs
  that uses the room the car actually has. Its tangent length is
  `R·cos(…)`, and the entry and exit points follow. The path then
  runs the arc (`0x55ea10`).
- **Junction turns** (`0x55f920`) use the same construction across
  the intersection: turn angle at `+0x96a0`, radius `+0x9730`,
  tangent length `+0x9728`.
- **Obstacles** (`0x566ff0`): each new segment is swept, padded by
  the car's half width (`+0x9750`), against the classes the four
  avoid flags enable. The first hit within the look-ahead (col 2)
  starts a recursive detour search (`0x55de80`/`0x55e290`): detour
  points either side of the obstacle at its radius + 2 m, within
  ±π/2 of travel, each re-swept, at most 10 candidates (`+0x9644`).
  `0x55d980` picks the shortest unblocked candidate — with
  `weirdPathfinding` set, candidates the search marked are preferred
  first.

## Control

**Steering** (`0x55ad10`): the bearing from the car's nose to point 2
of the chosen path, `steer = clamp(1.33 × 1.428 × bearing, −1, 1)` —
full lock at 30°. Above 30 m/s with the raw command past ±1 the
handbrake goes on.

**Speed** (`0x55b420`, `0x55b5f0`): throttle is `maxThrottle` and the
brake 0, unless a bend binds. For every bend on the next three roads
and the next two junction turns:

```
vc     = sqrt(1.2 · 19.8 · R) · cornerSpeedMultiplier
demand = v · (v − vc) / (1.2 · 19.8 · d)        (v > vc; d = distance to the bend)
```

When `demand > cornerBrakingThreshold` the car brakes at
`min(demand, 1)` with the throttle off, and damps its own yaw (below).
A kink of more than 0.7 rad between the next points of the chosen
path — the main path is candidate 0 when nothing blocks it — is
treated the same way with `R = 10 · tan((π − θ)/2)` (`0x55b420`). There is no partial throttle and no
coasting band: the car is either at `maxThrottle` or braking.

`1.2 · 19.8 = 23.76 m/s²` is the controller's grip budget for every
car. MM2's world gravity is **19.6 m/s²** (`0x5c5c1c`, applied as
`mass × −19.6` at `0x46a125`), so it reads as μ 1.2 at 2 g.

**Finishing**: within 70.7 m of the finish with the route and laps
complete, the car brakes to stop at the finish point, and holds the
brake inside 2.5 m.

## States (`0x55a6b0`, `+0x27c`)

| State | Behaviour |
| --- | --- |
| 0 drive | the planner and control above |
| 1 backup | reverse gear, throttle 0.85, `steer = clamp(−2.857 × bearing)`; ends after 66 frames or once the nose is within 0.1 rad of the target |
| 2 off the road network | steer straight at the target, gain 1.33, clamped ±0.75 |
| 3 stopped | steer at the finish point, brake 1 |

- **Into backup**: the car's stuck detector (`vehCar+0xcc`, state at
  `+0x18`) raises stuck (2). The detector class whose update sits in
  the AI code (`0x56f820`, vtable around `0x5b4c4c`) does so once the
  car has held more than 0.75 of its throttle cap with steering under
  0.5 (`0x56f7b0`) inside a small radius past a time threshold. State
  0 or 2 then zeroes the car's linear and angular momentum and switches
  to backup. Which `vehStuck` fields feed that class's thresholds is
  not recovered.
- **Totalled** (damage past its maximum): inputs zero, linear momentum
  × 0.95 every frame; on a circuit the car is reset after 5 s
  (`0x42c440`), elsewhere it stays out.
- **Fallen through the world** (y < −200, `0x53d652`): the car goes to
  state 3 for the rest of the race (`0x53d690`). There is no respawn.

## Physics interventions

Beyond writing inputs, the controller edits its car's rigid-body state
directly. `carsim+0x54` is linear momentum and `carsim+0x60` angular
momentum — the integrator (`0x478050`) derives velocity from them.

| Where | What |
| --- | --- |
| `0x55b5f0`, `0x55b420` | every frame it brakes for a bend: angular momentum × 0.85 |
| `0x55ad10`, `0x55a9a0` | entering backup: linear and angular momentum zeroed |
| `0x55b370` | leaving backup: both × 0.25, brake 1 |
| `0x55b2db` | leaving backup aligned: the car's matrix is rotated by the remaining bearing (≤ 0.1 rad) |
| `0x56f9d2` | while stuck: the car's matrix is rotated in place at a rate from the stuck record |
| `0x55a785` | totalled: linear momentum × 0.95 per frame |
| `0x55b14e` | each frame: `CarFrictionHandling` (`carsim+0x153c`) set to 2.0 or 1.0 from bit `0x8000` of the car instance's flags; it rescales the wheel's ground-material factor below 1 (`0x4d2ffb`) — off-road surfaces only. What the flag means is unrecovered |

## Finishing: the route, not the checkpoints

`aiVehicleOpponent` answers "finished?" itself (`0x53d6c0`, polled by
the game modes at `0x417b20`, `0x41d470`, `0x41eb90`, `0x419600`). It
is true once the car is on its last lap (`+0x9682 == +0x9684`), its
route index has reached the last intersection (`+0x967a ≥ count − 1`)
and it crosses the finish line's plane (the global finish point and
direction at `0x6b1f90`/`0x6b1f9c`) within 20 m of the finish point —
the side it was on is remembered at `+0x9790` from 30 m out.
**Checkpoints play no part**: an opponent finishes by completing its own
authored course.

The retail routes rely on that (**measured** 2026-10-04 against each
`race<N>waypoints.csv`): 221 of the 335 checkpoint-race `.opp` lines
miss at least one gate cylinder, 133 of them by more than 30 m and 186
gates by more than 100 m (london `race3`'s professional routes pass gate
2 51–59 m off). Under a rule that required every gate those opponents
could never finish.

## No rubber-banding

`maxThrottle`, the multiplier, the threshold and the look-ahead are
written only by `RegisterRoute` (`0x5596a9`–`0x5596d3`), and nothing in
the controller reads another participant's race progress. The only
player-dependent behaviour is the collision level of detail above.

## Confidence

- Everything in the tables and formulas: **verified_original**.
- Not recovered: the exact point spacing of `0x561890`, the scoring
  details of the detour search beyond "shortest unblocked", the
  stuck-detector field layout (which `vehStuck` field is the rotation
  rate), and the meaning of the `CarFrictionHandling` flag.

## Port status

Our opponents (`mm2_app::opponents`, `mm2_app::racing_line`) are a
designed controller: pure pursuit along the `.opp` anchors densified
through the nav graph, a speed plan from the line's curvature and the
car's own handling, wall feelers, a participant corridor, and the
bounded re-anchor teleport. Like the original,
`mm2_content::load_opponent` gives an opponent the player's own tune
and ignores the `_opp` files. Where it still diverges:

- Col 2's default reads 75 and col 9 is a grip scale around 2.0
  (DSN-66); the original defaults are 50 and 1.0, and col 9 scales
  speed.
- The original never teleports a stuck opponent; it backs up, and a
  car that falls out of the world is simply out.
- The original finishes an opponent on its route and the finish line;
  ours makes a checkpoint-race opponent cross every gate (so routes
  that skip one never finish) and credits circuit gates along the
  driven line (DSN-45).
