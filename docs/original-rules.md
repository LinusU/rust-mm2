# Original rules ledger

What the retail game is documented to do, what the shipped data proves,
and what we still do not know. This is the F00-B.2 counterpart to the
content inventory: the inventory counts *what files exist*, this ledger
records *what the original rules are* and how well each is evidenced.

Every fact carries one classification:

- **verified_original** — confirmed against authored data in the local
  retail install or by an existing verified path in this tree.
- **documented** — stated in documentation shipped with the game, but
  not (yet) confirmed against data or observed behavior.
- **inferred** — reasoned from filenames, data patterns or prior
  research; not stated anywhere authoritative.
- **designed** — a deliberate rust-mm2 policy, not an original claim.
- **unknown** — requires research; provisional behavior does not count
  as an answer.

Rules imported from MM1, MM3 or MM2Hook are never assumed; if a fact
comes from community knowledge it says so explicitly and stays at most
*inferred* until verified locally.

## Sources and method

| Source | What it is | How it was read |
| --- | --- | --- |
| `MM2HELP.HLP` | In-game help, 183 topics (cited as `help:<topic>`) | Decompiled locally with helpdeco (GPLv3, tool only — output not committed) |
| `Readme.rtf` | Retail readme, troubleshooting + MP notes (cited `readme:§`) | `textutil` |
| `Booklet.pdf` | Jewelcase booklet, controls/maps (cited `booklet:p`) | `pypdf` text extraction |
| `TROUBLE.RTF` | Crash-recovery troubleshooting doc | `textutil` |
| `data:<path>` | Authored files in the retail install | `mm2-inspect dump` / `inventory` through the read-only VFS |
| code docs | `docs/vehicle-handling.md`, `docs/research/*` | Prior verified/inferred work in this tree |

Retail install: `/Users/linus/coding/rust-mm2/retail`, enumerated
2026-09-20 (catalog fingerprint `fnv1a64:e91e6cd4b2ae30d9` at commit
`156b0cc`). Numbers below are reproducible with
`mm2-inspect inventory <install>` and `mm2-inspect dump <install> <path>`.

## Drivers, profiles and progression

| ID | Rule | Class | Source |
| --- | --- | --- | --- |
| DRV-1 | A driver profile accumulates race history; multiple drivers can share one install. | documented | help:Creating or Loading a Driver; data:`players/` (17 files) |
| DRV-2 | Each driver has a fixed rank: Amateur or Professional. | documented | help:Creating or Loading a Driver |
| DRV-3 | Amateur = longer Blitz times, less traffic, easier unlocks. Professional = shorter Blitz times, more traffic, harder unlocks, earns Pro points. | verified_original | help:Creating a New Driver; data:`mm*data.csv` — the second parameter block per row consistently carries shorter TimeLimits and higher densities/opponent counts |
| DRV-4 | Pro points are awarded on two criteria: finishing position and vehicle driven (slower vehicle, more points). Exact formula unknown (UNK-8). | documented | help:Creating or Loading a Driver |
| DRV-5 | Best results are recorded per driver per race and viewable on Race Records / Driver's Stats, filterable by race type, city, and Amateur Times / Pro Times / Pro Points. | documented | help:Race Records, help:Driver's Stats Screen |
| DRV-6 | Records are kept only for races run under default conditions; customizing laps/checkpoints/opponents excludes that run. | documented | help:Race Records |
| DRV-7 | The last remaining driver cannot be deleted. | documented | help:Main Menu Screen |
| DRV-8 | Quick Race jumps straight into the last race played after vehicle select. | documented | help:Quick Race, help:Main Menu Screen |

## Race modes

| ID | Rule | Class | Source |
| --- | --- | --- | --- |
| RACE-1 | Four competitive single-player modes exist — Blitz, Checkpoint, Circuit — plus free-roam Cruise, and the Crash Course schools. | verified_original | help:The Races; data:`race/<city>/` families + `mm*data.csv` |
| RACE-2 | Each event has fixed authored location, checkpoint set, weather, time-of-day and densities. | verified_original | help:Blitz/Checkpoint/Circuit; data:`mm*data.csv` per-row Weather/TimeofDay/Ambient/Peds/Cops |
| RACE-3 | Finishing top-3 as Amateur or 1st as Professional in a Blitz/Checkpoint/Circuit race unlocks customization of that race's weather, time-of-day, traffic/ped/cop density (Circuit: laps + opponents, no pedestrians). | documented | help:Blitz Race, help:Circuit Race, help:Unlocking Races, help:Races Screen |
| RACE-4 | Cruise-mode weather/time-of-day/density options are always available, no unlock needed. | documented | help:Races Screen |
| RACE-5 | Destroying your vehicle in a Blitz or Checkpoint race means restarting from the beginning; in a Circuit race it costs a time penalty and the vehicle resets. | documented | help:Blitz Race, help:Checkpoint Race, help:Circuit Race, help:Durability |
| RACE-6 | The green compass arrow points to the nearest un-cleared checkpoint and turns yellow when that checkpoint is behind you; X/S cycle the arrow through remaining checkpoints. | documented | help:Using Dashboard Instruments, help:General Keyboard Game Controls; booklet:p6 |
| RACE-7 | In Checkpoint races the finish line only appears after every checkpoint is cleared. | documented | help:Checkpoint Race, help:Tips for Winning |
| RACE-8 | Amateur rows: 4-7 opponents in Checkpoint/Circuit, 0 in Blitz; Professional rows: 4-7 (london checkpoint 6-7). | verified_original | data:`mmracedata.csv`, `mmcircuitdata.csv` |
| RACE-9 | Authored per-city selectable-event rosters: 12 Checkpoint, 10 Blitz, 10 Circuit rows in each city's `mm*data.csv`. | verified_original | `mm2-inspect inventory` event-metadata rows (both cities) |
| RACE-10 | More `.aimap` event files ship than the tables list (london race0-13/blitz0-12/circuit0-11, sf race0-11+r0/blitz0-13/circuit0-11 vs 12/10/10 rows). Whether the extra files are selectable events or dev leftovers is unknown (UNK-2). | verified_original | `mm2-inspect inventory` races family vs event-metadata rows |
| RACE-11 | `race/<city>/<stem>.aimap` binds the event's Amateur actor roster and `<stem>.aimap_p` the Professional one: `[Opponent]`/`[Police]` counts equal the corresponding `mmracedata.csv` block's Opponents/Cops on 23/24 checkpoint events. `sf/race0` is an authored anomaly — amateur Opponents=7 while `race0.aimap` wires 6 (`race0-a-6.opp` ships unreferenced). The `mm2-inspect opponents` audit (2026-09-21) re-measures the opponent half of this on *every* table kind: `sf/race0` amateur is still the only wired-vs-table count mismatch — every other event's `[Opponent]` rows match its authored count at both difficulties (Blitz wires 0 rows matching Opponents=0). | verified_original | measured cross-check `mmracedata.csv` vs all 24 checkpoint `race<N>.aimap{,_p}` (2026-09-20); `mm2-inspect opponents` all-table audit (2026-09-21); docs/research/aimap.md |
| RACE-12 | An `[Opponent]` row wires `geo <route>.opp <tail>`: the opponent's vehicle id (amateur rows field base cars — `vpbug`/`vpcoop`/`vpcab`/`vpauditt`…; professional rows shift to harder variants — `vpcoop2k`, `vpvwcup`, `vpdb7`, `vppanoz`, `vppanozgt`), an authored `.opp` driving line, and a numeric tail (10 values on race rows; decoded into mm2hook's recovered driving-parameter vocabulary — inferred, RACE-14/UNK-11). Many events ship spare `.opp` files no `[Opponent]` row references (43 london + 34 sf records, including `blitz3`/`blitz4` route files on 0-opponent events); non-table stems still carry lineups (`race/london/race12.aimap` wires 1; `race/sf/stunt0.aimap` wires 1 whose `opp-c0.2` is a dead ref). Every wired vehicle id resolves to a `ready` vehicle-catalog entry on retail. | verified_original | `mm2-inspect opponents` on retail (2026-09-21): 64 builds/city, 0 failed, 271 london + 246 sf opponents wired, 0 unresolved vehicle ids; docs/research/aimap.md |
| RACE-13 | `tune/vehicle/<id>_opp.vehcarsim` exists for nearly every stock car (23 files on retail) and is a *sparse override*, not a standalone tune: authored values differ from the base file (`vpbug_opp`: inertia box, drivetrain, end-train inertias, horsepower, top speed), but most `_opp` files omit fields the base file carries, and several (`vp4x4_opp`, `vpbus_opp`, `vpcab_opp`…) author transmission data in an alternate schema — `NumGears`/`GearRatios`/`UpshiftRPM`/`DownshiftRPM`/`DownshiftBias` — that the player files' `ManualNumGears`/`Low`/`High` band schema does not use. `vpsemi_opp` and `vpvwcup_opp` are complete under the base schema. Our `load_opponent` merges the `_opp` document over the base tune (authored fields win, the rest inherit) — a designed policy: the files' existence and contents are verified, their exact original consumption is not (UNK-11). | verified_original | `mm2-inspect list`/`dump` field audit of all 23 retail `_opp` files (2026-09-21) |
| RACE-14 | The ten-value `[Opponent]` tail decodes into mm2hook's recovered driving-parameter vocabulary (`OpponentData` / `aiVehiclePhysics::RegisterRoute`, R4): col 0 `maxThrottle` (0.57–1.00), col 1 a rare `weirdPathfinding`/`BadPathfinding` flag, col 2 a distance (50–150), col 3 a ~0.7-centred factor, cols 4–8 flags `unused`/`avoidTraffic`/`avoidProps`/`avoidPlayers`/`avoidOpponents`, col 9 `cornerSpeedMultiplier` (0.89–2.29, professional rows reach 2.0+). The mapping is **inferred** — mm2hook's `OpponentData` field *order* does not match retail distributions positionally, so the assignment orders the same recovered names by what each column's values can be; every column's range matches the corresponding `RegisterRoute` default. Our consumption is partial and disclosed: `maxThrottle` → a throttle ceiling, `cornerSpeedMultiplier` → the corner-brake floor, `avoidPlayers` → gates human participants in the corridor; `avoidOpponents` stays inert (retail ≈ universal 0 — consuming it would blind every stock opponent to the field; the flag's order/polarity is unverified), `avoidTraffic`/`avoidProps` inert (no such runtime classes), the rest bound but unconsumed. | inferred | mm2hook `aiDataTypes.h`/`aiVehiclePhysics.cpp` + retail `mm2-inspect dump` distributions on 536 rows (2026-09-21); docs/research/aimap.md |

## Blitz

| ID | Rule | Class | Source |
| --- | --- | --- | --- |
| BLZ-1 | Solo race against a countdown timer: clear all checkpoints in any order and reach the finish before time runs out. | documented | help:Blitz Race |
| BLZ-2 | No opponents and no police in Blitz events. | verified_original | data:`mmblitzdata.csv` — Opponents=0, Cops=0 in all 40 rows |
| BLZ-3 | Per-event time limits are authored (25-120 amateur, 18-103 professional on retail). Unit unverified (UNK-4). | verified_original | data:`mmblitzdata.csv` TimeLimit |
| BLZ-4 | Blitz has ambient traffic and pedestrians; per-event densities are authored (am 0.0-0.7/0.1-0.4; pro 0.2-0.9/0.1-0.5). | verified_original | data:`mmblitzdata.csv` Ambient/Peds |
| BLZ-5 | Running out of time before finishing fails the run; the run can be restarted. | inferred | follows from BLZ-1's "before time runs out"; help:Blitz Race — exact fail screen unverified |

## Checkpoint

| ID | Rule | Class | Source |
| --- | --- | --- | --- |
| CHK-1 | Race against opponents through all checkpoints in any order; finish line appears last. | documented | help:Checkpoint Race |
| CHK-2 | 12 Checkpoint races per city; only the first three are unlocked initially. | verified_original | help:Checkpoint Race ("all 12", "first three"); data:`mmracedata.csv` = 12 rows/city |
| CHK-3 | Placing top-3 (Amateur) or 1st (Professional) in each race of a set of three unlocks the next set. | documented | help:Checkpoint Race, help:Unlocking Races |
| CHK-4 | Police appear at fixed authored locations per race. | verified_original | help:Tips for Winning ("cops are located in specific places"); data:`mmracedata.csv` Cops 0-4 am / 0-8 pro |
| CHK-5 | Opponents may take different routes; each is shown on the map as a colored triangle. | documented | help:Tips for Winning, help:Displaying a Map of the City |

## Circuit

| ID | Rule | Class | Source |
| --- | --- | --- | --- |
| CIR-1 | Ordered checkpoints define a closed course; laps must beat the opponents. Missed checkpoints must be cleared or the lap does not count. | documented | help:Circuit Race |
| CIR-2 | Cross-streets are closed off; off-road shortcuts through parks etc. still possible. | documented | help:Circuit Race, help:Tips for Winning |
| CIR-3 | No pedestrians in Circuit races. | verified_original | help:Circuit Race; data:`mmcircuitdata.csv` Peds=0 in all rows |
| CIR-4 | No ambient traffic and no police in Circuit races. | verified_original | data:`mmcircuitdata.csv` Ambient=0, Cops=0 in all rows |
| CIR-5 | Lap counts are authored per event (2-3 amateur, 2-4 professional on retail). | verified_original | data:`mmcircuitdata.csv` NumLaps |

## Authored race records (waypoints, start grids)

|| ID | Rule | Class | Source |
|| --- | --- | --- | --- |
|| WPT-1 | `<stem>waypoints.csv` rows carry `x,y,z,a,w`; `w` behaves as the checkpoint trigger radius (half street width — retail values 8-15 m). | inferred | data:`race/*/…waypoints.csv`; used by `mm2_content::race_def` |
|| WPT-2 | Row roles for Blitz/Checkpoint: row 0 is the start line, rows 1..n-1 are any-order checkpoints, the last row is the finish trigger. For Circuit every row after 0 is an ordered gate and the course closes through the start line again. | inferred | consistent across retail rows (e.g. blitz0 = line + 3 gates + finish, matching BLZ-1/CHK-1/CIR-1 docs); measured on all 24 checkpoint waypoint files 2026-09-20 — every file has ≥3 rows and a last row distinct from row 0 (london race0: 7 rows, finish ~840 m from the start; london race9's finish is closest at ~49 m); provisional |
|| WPT-3 | `<stem>_strtpnts` files hold the starting grid (one `x,y,z,a` row per slot). Only SF circuits ship them, under the short stem `cir<N>` — `cir1_strtpnts` … `cir9_strtpnts` — while the event rows are `circuit1`…`circuit9`; the same-index alias is the catalog's inference. | inferred | data:`race/sf/cir*_strtpnts`; `mm2-inspect events` extras listing |
|| WPT-4 | The `a` columns are degrees in two different conventions, measured 2026-09-21: waypoint `a` is a course bearing (`atan2(dx,dz)` along the rows), while `_strtpnts` `a` and the `.opp` row-0 staging field are vehicle yaw — forward `(−sin a, −cos a)`, exactly 180° apart. `RaceStart.yaw_deg` stores vehicle yaw and `Quat::from_rotation_y(yaw_deg.to_radians())` consumes it directly; the no-grid fallback derives it from the row0→row1 tangent. An authored `_strtpnts` `a = 0` is *no heading* — the same zero-means-unset rule the `.opp` staging field uses (measured: `cir6_strtpnts` is the lone all-zero grid while its `.opp` routes stage ~180°, so a verbatim 0 faces the grid backward); `yaw_deg` is `Option` and a `None` consumer derives the course facing (`course_yaw` for the player, the route staged heading/first leg for opponents). Whether the original *enforces* gate direction from waypoint `a` stays open (UNK-16). | inferred | data: all 612 retail `.opp` + `cir*_strtpnts` vs waypoint geometry; `mm2_content::race_def`, `mm2_app::session` |
|| WPT-5 | All waypoint/start coordinates are in world space in the same frame as the PSDL city mesh. | verified_original | retail blitz0 waypoints coincide with the London street mesh the spawned car sits on (screenshot evidence) |

## Cruise

| ID | Rule | Class | Source |
| --- | --- | --- | --- |
| CRZ-1 | Free roam of either city, no clock, no opponents. | documented | help:Cruise, help:Races Screen |
| CRZ-2 | Ambient traffic, pedestrians and police are active; police chase on sight, and "flashy" cars attract more attention. | documented | help:Tips for Winning (Cruise tips) |
| CRZ-3 | Losing a pursuing cop only requires leaving its sight. | documented | help:Tips for Winning |

## Crash Course

| ID | Rule | Class | Source |
| --- | --- | --- | --- |
| CC-1 | Two schools: London Cabbie (East End Cab Company) and SF Stunt Driver (Golden Gate Stunt Driving School). | documented | help:Crash Course, help:Crash Course Screens |
| CC-2 | Each school: 9 lessons, 3 midterms, 1 final — 13 authored events, ordered lesson1-3 → midtrm1 → lesson4-6 → midtrm2 → lesson7-9 → midtrm3 → final. | verified_original | help:Crash Course ("nine lessons", "three lessons in a group"); data:`mmcrashdata.csv` 13 rows in that exact order; `crash0-12` files |
| CC-3 | Passing each group of three lessons unlocks that group's midterm; passing all three midterms unlocks the final. | documented | help:Crash Course |
| CC-4 | Required vehicle per school: Ford Mustang Fastback (SF), London Cab (London). Passed lessons can be replayed in any vehicle. | documented | help:Crash Course |
| CC-5 | "Work Experience" offers Blitz/Checkpoint races from the Crash Course screen. | documented | help:Crash Course |
| CC-6 | Rewards: SF midterms 1/2/3 → RSi Angel Cup paint / Double-Decker paint / Fastback paint, SF final → LTV; London midterms 1/2/3 → RSi Microsoft Cup paint / London Cab paint / Mini Cooper Classic paint, London final → Aston Martin DB7. | documented | help:Crash Course Rewards |

## Vehicle roster and unlocks

| ID | Rule | Class | Source |
| --- | --- | --- | --- |
| VEH-1 | Help lists 20 player vehicles: 12 unlocked + 8 locked. The shipped roster has 21 — `vpmoonrover` ("Moon Rover", 800hp, 1M durability) is undocumented (UNK-3). | verified_original | help:The Vehicles; data:`tune/*.info` + `EXPECTED_STOCK_ROSTER` |
| VEH-2 | Unlocked from the start: New Beetle, '68 Mustang Fastback, City Bus, London Cab, Eldorado, Freightliner Century, Mini Cooper Classic, Mustang Cruiser (police), Double-Decker Bus, F-350, Mustang GT, Panoz Roadster. | documented | help:The Vehicles |
| VEH-3 | Locked-vehicle unlock rules — Audi TT: top-3/1st in half the SF Checkpoint races; Panoz GTR-1: half the London Checkpoint races; Fire Truck: half the SF Blitz; Beetle Dune: half the SF Circuit; Beetle RSi: half the London Circuit; NEW MINI COOPER: half the London Blitz; LTV: finish SF Crash Course; DB7: finish London Crash Course. | documented | help:Unlocking Vehicles |
| VEH-4 | Custom paint unlock rules — TT/Fire Truck/Dune/RSi(first)/NEW MINI: top-3/1st in *all* of the corresponding race set; Fastback/Cab/Mini Classic/DD Bus paints: specific midterms (CC-6); RSi Angel/Microsoft cups: SF/London midterm 1. | documented | help:Unlocking Custom Paint Jobs |
| VEH-5 | `.info` files carry `UnlockScore` (only `vppanozgt` nonzero at 8000), `UnlockFlags`, `Flags`, `ScoringBias` fields. Bit/score semantics are inferred at best — `UnlockScore=8000` likely ties to Pro points but is unverified (UNK-6). | verified_original | data:`tune/vp*.info` |
| VEH-6 | Vehicle stats shown to the player: Horsepower, Top Speed, Durability, Mass. | verified_original | help:Key Properties; data:`tune/*.info` fields |
| VEH-7 | Durability covers collision resistance and rough-terrain abuse; high-clearance trucks survive stairs that wreck low cars. | documented | help:Key Properties |
| VEH-8 | The Police Car is usable in Cops vs. Robbers even while locked, when playing the cop side. | documented | help:Ford Mustang Cruiser |

## Damage and recovery

| ID | Rule | Class | Source |
| --- | --- | --- | --- |
| DMG-1 | A Damage Meter HUD bar runs green → yellow → red; at zero the vehicle stops working. | documented | help:Using Dashboard Instruments |
| DMG-2 | Destruction in Blitz/Checkpoint = restart race; in Circuit = time penalty + reset (RACE-5). | documented | help:Durability |
| DMG-3 | Vehicles have "more detailed damage modeling and breakaway parts". | documented | booklet:p3 |
| DMG-4 | In Cops & Robbers, damaged vehicles not carrying gold heal over time. | documented | help:Cops & Robbers |
| DMG-5 | `.vehcardamage` tune files exist per vehicle; parseable via the generic tune parser. Whether/how the original used them is unverified. | verified_original | data:`tune/vehicle/*.vehcardamage`; PLAN F05-A |

## Police (single player)

| ID | Rule | Class | Source |
| --- | --- | --- | --- |
| COP-1 | Police chase on sight in modes where they are enabled; count/locations are per-event authored data (RACE-2, CHK-4). | verified_original | help:Tips for Winning; data:`mm*data.csv` Cops |
| COP-2 | Escaping = leaving the cop's sight; conspicuous cars get reported ahead and chased on sight elsewhere. | documented | help:Tips for Winning |
| COP-3 | The Police Car (`vpcop`) is a distinct roster entry with `_cop` tuning variants present in data. | verified_original | data:`tune/vpcop.info`, `tune/vehicle/*_cop.*` |
| COP-4 | Pursuit AI, ram tactics, wanted-level model: nothing in shipped docs or obvious data describes it. | unknown | UNK-9 |

## Multiplayer

| ID | Rule | Class | Source |
| --- | --- | --- | --- |
| MP-1 | Up to 8 players total ("seven other people") via MSN Zone, IPX or TCP/IP; serial/modem are 1v1. | documented | help:Types of Multiplayer Connections, help:Multiplayer Screen |
| MP-2 | Max 10 concurrent hosted sessions per LAN. | documented | readme:§7.05 |
| MP-3 | Host controls connection type, password, max players, event, location, conditions, difficulty (from host's driver rank), and which vehicles/checkpoint races are unlocked — the host's record applies to everyone. | documented | help:Hosting vs. Joining |
| MP-4 | Modes: Cruise, Blitz, Checkpoint, Circuit, Cops & Robbers. No ambient traffic, cops or AI opponents in MP races; humans replace AI opponents. | documented | help:Multiplayer Games |
| MP-5 | Race joiners must be in before the host starts; Cruise and C&R allow join/leave at any time. A leaver's vehicle disappears for everyone; host disconnect → a new host is designated. | documented | help:Multiplayer Games |
| MP-6 | No pausing in multiplayer (ESC still opens the menu). | documented | help:Pausing and Resuming, help:Displaying a Map |
| MP-7 | Host can eject players via F6 (in-game player list) or the lobby Eject button. | documented | help:Multiplayer Games, help:Multiplayer Lobby Screen |
| MP-8 | Lobby has chat, per-player vehicle/color, team pick (team modes), Ready (joiner) / Start Server (host), Game Settings box. | documented | help:Multiplayer Lobby Screen |
| MP-9 | MP-specific data is thin: `copchase`/`multicop`/`*_p` records exist but no C&R-specific data discovered. The 99 `.aimap_p` variants once suspected as MP files are evidenced as Professional-difficulty actor rosters instead (RACE-11). | verified_original | `mm2-inspect inventory` multiplayer family; RACE-11 cross-check |

## Cops & Robbers

| ID | Rule | Class | Source |
| --- | --- | --- | --- |
| CNR-1 | A gold bar spawns somewhere in the city; deliver it to your hideout to score, then another appears; carriers can be rammed to drop the gold, which anyone can pick up by driving over it. | documented | help:Cops & Robbers |
| CNR-2 | Three variants: Free-for-all (any unlocked vehicle, own hideout), Cops vs. Robbers (cops = Mustang Cruiser → bank; robbers = Mustang GT → hideout), Robbers vs. Robbers (teams, any unlocked vehicle, several hideouts). | documented | help:Cops & Robbers, help:Host Settings Screen |
| CNR-3 | Host options: location, weather, time-of-day, pedestrian density, match limit (none / time / points), gold mass (weightless / ¼ ton / ½ ton — heavier slows the carrier and holds the gold better). | documented | help:Cops & Robbers, help:Host Settings Screen |
| CNR-4 | Non-carriers heal over time (DMG-4). Results show individual points; team variants also show team totals. | documented | help:Cops & Robbers |
| CNR-5 | Gold/hideout/bank positions, respawn rules, scoring values, drop physics: not in shipped docs or identified data. | unknown | UNK-10 |

## HUD, map and cameras

| ID | Rule | Class | Source |
| --- | --- | --- | --- |
| HUD-1 | Dashboard instruments: speedometer, tachometer (shift near redline on manual), gear display (R N 1 2 3 4…), damage meter, steering bar (mouse control only). | documented | help:Using Dashboard Instruments |
| HUD-2 | Race instruments: green compass arrow (Checkpoint/Blitz), checkpoint list, laps record (Circuit), place indicator, stopwatch (Checkpoint/Circuit), countdown timer (Blitz). | documented | help:Using Dashboard Instruments |
| HUD-3 | Cameras: Chase Near (default), Cockpit, Chase Far on C. W toggles wide-screen; V = "Thrill Cam"; cockpit look left/right/back/forward on numpad 4/6/2/8; BACKSPACE rearview mirror; D dashboard; H HUD; I opponent indicator. | documented | help:Camera Views, booklet:p9 |
| HUD-4 | Map: TAB cycles two smaller map views/off, E zooms, F toggles rotation, Q = full-screen pause map (single player only). Shows player, opponent triangles, un-cleared (bright) / cleared (dark) checkpoints, yellow highlight on arrow target, finish line when unlocked. | documented | help:Displaying a Map of the City |
| HUD-5 | London tunnels are marked on the map; some alleys/shortcuts are not. | documented | help:Tips for Winning |

## Controls and options

| ID | Rule | Class | Source |
| --- | --- | --- | --- |
| CTL-1 | Keyboard defaults: arrows drive (down = brake then auto-reverse when stopped), SPACE handbrake, ENTER horn, T auto/manual, R reverse/drive toggle (only from/to 1st), A/Z shift up/down, C camera, TAB map, Q full map, E zoom, F rotate map, W wide view, H HUD, I opponent arrows, V thrill cam, D dash, BACKSPACE mirror, F1 controls list, F4 restart race, ESC menu, X/S cycle arrow target, 2-5 CD player. | documented | help:General Keyboard Game Controls, booklet:p8-9 |
| CTL-2 | Primary driving controller is selectable: keyboard, mouse (steer by moving, LMB throttle, RMB brake/reverse), joystick, game pad, steering wheel; joystick wins by default if installed, else mouse. | documented | help:Driving with a Mouse/Keyboard/Joystick |
| CTL-3 | Control Options: Auto Reverse, POV-hat look, Force Feedback, controller select, steering sensitivity, dead zone, calibrate, collision/road-force intensity, full control rebinding (with conflict warning), defaults. | documented | help:Control Options Screen |
| CTL-4 | Graphics Options: Textured Sky, Vehicle Reflections, Show Pedestrians, Display device, Renderer (auto-detected; Software Only fallback), Resolution (default 640x480), Visibility + Lighting Quality sliders, Texture Quality + Object Detail (Very High/High/Medium/Low), Cloud Shadows (High/Low/None), defaults. | documented | help:Graphics Options Screen, readme:§5 |
| CTL-5 | Audio Options: Sound FX, Play Commentary, Play Music, City Sounds, device select, Stereo FX (mono/stereo), Sound Quality (High = 16-bit 22kHz 32ch, Medium = 8-bit 22kHz 16ch, Low = 8-bit 11kHz 8ch), FX and Music/City volumes, balance, defaults. | documented | help:Audio Options Screen, help:Adjusting Volume |
| CTL-6 | CD player plays the game CD or any audio CD during play; keys 2/3/4/5. | documented | help:Playing Music While You Race |
| CTL-7 | `-nomipmap` command-line switch exists; Safe Mode / Redetect Video / No Movie / No Sound helper shortcuts ship in the install. | verified_original | readme:§3.19; install dir listing |
| CTL-8 | F4 restarts the current race; ESC opens the In-Game Menu (Quit to Race Menu / Options / Resume / Exit to Windows). | documented | help:Quitting a Race, help:General Keyboard Game Controls |

## Menus and flow

| ID | Rule | Class | Source |
| --- | --- | --- | --- |
| UI-1 | Main Menu: Crash Course, Races, Multiplayer, Quick Race, driver select/create/delete, Driver's Stats, Race Records, Options. | documented | help:Main Menu Screen |
| UI-2 | Races screen gates Time-of-Day/Weather/Ped/Traffic/Cop density (and Circuit laps/opponents) behind top-3 Amateur / 1st Professional finishes — always open in Cruise. | documented | help:Races Screen |
| UI-3 | Select Vehicle shows LOCKED overlays on locked vehicles and locked paint colors, plus transmission choice and the four stats. | documented | help:Select Vehicle Screen |
| UI-4 | Every screen has Options (top-right) and Help ("?"); ESC returns toward Main Menu. | documented | booklet:p2, help:Quick Race |
| UI-5 | A Results screen follows each race/match (placing + total time; C&R adds points). | documented | help:Checkpoint Race, help:Cops & Robbers |

## World and ambient behavior

| ID | Rule | Class | Source |
| --- | --- | --- | --- |
| WLD-1 | Traffic/ped densities are per-event authored fractions (0.0-0.9) and cop counts authored integers, all selectable post-unlock. | verified_original | data:`mm*data.csv` Ambient/Peds/Cops |
| WLD-2 | Pedestrians exist on streets and react audibly (screaming "Maniac!"); can be toggled via Show Pedestrians. | documented | help:Configuring Sound, help:Graphics Options |
| WLD-3 | Documented original defect: vehicles can stick in soft ground under SF dock archways; less likely at speed. | documented | readme:§3.05 |
| WLD-4 | Weather and time-of-day are per-event selectors (0-3 each in data; enum meaning unverified — UNK-1). | verified_original | data:`mm*data.csv` Weather/TimeofDay columns |
| WLD-5 | Sidewalks, alleys, parks, wrong side of road are all drivable shortcuts. | documented | help:Tips for Winning |
| WLD-6 | `.aimap` override grammar is uniform across all 209 retail files: `[Speed Limit]` scalar, counted `[Exceptions]` (road-id/density/speed), `[Police]`/`[Opponent]` spawns, `[Ambient Types/Density]` cumulative weights closing at 1.0, `[Ambients Drive On The Left]` (1 london / 0 sf), ped-model pairs; rarer `[Density]`, `[CopChaseDistance]`, `[AmbientLaneChanges]`, `[Traffic Lights]`, `[Hookmen]`. | verified_original | data: all `city/`+`race/` aimaps, `mm2-inspect aimap`; docs/research/aimap.md |
| WLD-7 | Retail exception density/speed values are all `0.00`/`0` — consistent with R3's claim that event aimaps close course roads to ambient traffic. Runtime consumption of these values is unverified (UNK-12). | inferred | data: every `[Exceptions]` row on retail |
| WLD-8 | Eight london `.aimap` files carry `[Exceptions]` road ids 562–815, outside `city/london.bai`'s 540-road space; SF exceptions are all in range. Different id space or dead authored refs — unknown (UNK-18). | verified_original | `mm2-inspect aimap` cross-check vs `city/london.bai` |
| WLD-9 | BAI section frames carry `x_axis` = `tangent × up` (the geometric right of travel with the sections) on ~all sections of both retail cities; right-side lane curves sit mostly at +x, left-side at −x — exceptions are authored geometry, so lane rank comes from measured offset, not side slot. | verified_original | measured on `city/{sf,london}.bai`; docs/research/bai.md |
| WLD-10 | Ambient turn rules: innermost lane turns toward centre or straight, outermost kerb-side or straight, middle lanes straight; one-way roads may take any outgoing road; freeway ramps may force a right; U-turns never. Intersection road lists are authored counterclockwise. London's left-hand driving is baked into authored BAI data (lane swap + reversed vertex/lane order), not a runtime flag. | documented | Adzima GDMag ambient-AI article; docs/research/bai.md |
| WLD-11 | Each retail city's vehicle road graph is one connected component (SF has exactly one dead end); 166/540 london and 96/379 sf roads are one-way; 176 roads carry no routable vehicle lanes (pedestrian/special/disabled or curve-less). Directed reachability (`mm2-inspect nav --routes`, 2026-09-20): london's graph is strongly connected — all 606 arcs reach all others. SF's is not: beyond the one dead end (road 0 backward), six arcs reach junctions no arm legally departs — authored one-way traps (roads 92/94/97/99 reach only themselves, 102/111 reach 2, road 1 backward reaches 617 of 618) — leaving 4927 ordered arc pairs unreachable (~1.3%); 505/512 seeded route probes succeed, the 7 failures are exactly the trap-region sources. Event `[Exceptions]` bind at routing time: `race/london/blitz0.aimap`'s 8 closures isolate roads 115-/116-/368+/375+ (15016 unreachable pairs, 24/512 probes unreachable — e.g. `270→108` routes 19 steps/1428 m open, `Unreachable` closed) and `race/sf/blitz0.aimap`'s 3 raise unreachable pairs to 7977 (11/512 probes); no probe ever traverses a closed road mid-route. | verified_original | `mm2-inspect nav --routes/--aimap` on retail |
| WLD-12 | `PTH1` pathset grammar is uniform across all 98 parseable retail files: header cursor + named paths of attributed points, kinds 0 single-points / 1 directed-pairs / 2 line-strip, spacing authored in quarter metres (0–60 m range seen; 0 and 5 m dominate). Placement names resolve to `geometry/<n>.pkg` (props) or `texture/<n>.*` (decals); `PATHnn` names are route labels (audio paths, parked-car/ferry/train routes). | verified_original | data: all 101 `.pathset` files, `mm2-inspect pathset`; docs/research/pathset.md |
| WLD-13 | `city/<city>/props.pathset` is the ambient prop-dressing source: london stamps 1188 prop instances from 87 paths (0 unresolved), sf 925 from 113 paths — its remaining 31 paths are `r4i_rails_f` *decal* names living inside the prop file, classified separately not failed. `props.pathset` names are prop PKGs; decal/audio/race pathsets are different consumers. | inferred | `mm2 --city {london,sf}` import reports + rendered stamp rows; docs/research/pathset.md |
| WLD-14 | The PSDL `prop_rule` byte selects a `n{NN}left`/`n{NN}right` row pair in `city/<city>/proprules.csv`: london bytes 1–16 ↔ rules n01–n16, sf 1–20 ↔ n01–n20 (n14 defined but unreferenced); 0 = no roadside props. `propdefs.csv` rows name 1–4 PKG variants plus start/distance/maxUse/minLerp/maxLerp fields; `props.csv` is a `Group,Name` membership list (retail: `Races` only). Each city has exactly one room carrying byte 205 with no matching rules — an authored anomaly, not a parser artifact. | verified_original | `mm2-inspect proprules` on retail; docs/research/proprules.md |
| WLD-15 | `tune/banger/*.dgbangerdata` is one `dgBangerData` record per banger stem (999 on retail, all `type: a`), carrying Size/CG/Mass/Elasticity/Friction/ImpulseLimit2/NumParts plus a `BirthRule` particle spec. The stem→geometry link is by name alone: `geometry/<stem>.pkg` (standalone prop, 216), `geometry/<stem>.mtx` or a matching PKG chunk (named part, 477), `<base>_break<NN>` → the `BREAK<NN>` chunk inside `geometry/<base>.pkg` (254 resolved); `default.dgbangerdata` is the fallback record. On every standalone prop, `NumParts` equals the PKG's distinct `BREAK<NN>` index count (0 mismatches). 47 authored records resolve to no geometry at all. | verified_original | `mm2-inspect banger` on retail; docs/research/banger.md |
| WLD-16 | World bangers bind by placed name (`N` → `tune/banger/<N>.dgbangerdata`) and reach the world through pathset stamping (`props.pathset` 17/17 london, 30/30 sf prop names bound; race overlays all bound) and the prop-rule channel (PSDL `prop_rule` → `proprules.csv` → `propdefs.csv` files, all bound). INST is the static-architecture channel: 0/386 distinct names bound. `*_ai.inst` supplements stamp only bound `sp_stop_f`. Vehicle (`vp*`/`va*`) records bind through the vehicle pipeline — 269/994 records reachable via world placements, 562 vehicle-owned, 163 authored-but-never-placed on retail. | verified_original | `mm2-inspect banger-bind` on retail; docs/research/banger.md |
| WLD-17 | Prop-rule PSDL geometry: a `Psdl::paths` record is one road run between two junction crossings and `road_rooms` chains the road rooms it traverses, consecutive entries sharing direct road-to-road boundaries. `start_crossroads`/`end_crossroads` name the curb vertex pair of the end crossings; a crossing occupies four consecutive perimeter points `[outer, curb, curb, outer]`, and at a direct road-to-road boundary the curb pair is the widest consecutive point pair marked with the neighbouring room id. The two arcs between the entry/exit crossings are the sidewalk building lines. Every rule-bearing room a sane path reaches resolves its crossings under this model (0 misses, both cities); some sf `road_rooms` entries encode non-room values (65 0xx range). | verified_original | measured on `city/{sf,london}.psdl` + `mm2_game::props` runtime walk; docs/research/proprules.md |
| WLD-18 | Decal `LineStrip` paths interleave two authored ribbon edges: even point indices form one edge, odd the other, each (even, odd) pair is a cross-section carrying the strip's width, and consecutive sections join into textured quads. Verified against retail: the pairing produces coherent authored widths — ~1 m zigzag (`decal_zigzag_l`), ~2 m cable-car rail channels (`r4i_rails_f`), ~8 m zebra crossings (`decal_rxwalk03_l`), 15–20 m junction paint (`decal_x_inter_l`) — where a centreline reading yields incoherent alternating ~1 m/~20 m segments; texture contents corroborate the axis (zigzag line oscillates along texture V, zebra bars run along U). Palette alpha in decal P8 textures is authored translucency (unpainted surrounds ≈ 36, painted bars ≈ 240). The exact UV policy (v tiling period vs stretch, u direction) and the original's blend state remain inferred. | verified_original | measured on `city/{sf,london}/decals.pathset` + `texture/decal_*.tex`; docs/research/pathset.md |
| WLD-19 | Surface materials live in one global pair `city/materials.{csv,mtl}` shared by both cities (no per-city copies, no other `.mtl` in the VFS). `materials.csv` maps texture stems to physics-material names (`texture,physics`; 3423 rows: 137 named, 3286 `none` = "no named material"); `materials.mtl` defines 8 `mtl <name> { … }` blocks (`deepwater`, `_default`, `grass`, `water`, `dirt`, `sand`, `cobblestone`, `wood`) each carrying the same ten fields — `elasticity`, `friction`, `effect` (`none` everywhere), `sound` (0/1/2 classes), `drag`, `width`/`height`/`depth`, `ptxindex`, `ptxthreshold`. PSDL texture-table names look up through the map with two normalizations: `<stem>-NNNN` animated-frame entries (`s_thames-0009`) fall back to the base stem (`s_thames` → `deepwater`), and 6 authored blank slots per city expect no coverage. Coverage: london 469 names = 152 named + 308 `none` + 6 blank + 3 unmapped (`sliver`, `sf_win_brickyel01_2s_4_l`, `sf_base_tan09_1s_5_l`); sf 457 = 148 + 301 + 6 + 2 (`sliver`, `gw_stc_offwhite_marswin_f`). Two dead authored refs: `transbay_ramp_f→ash`, `s_grass2mud→mud`; `_default`/`dirt`/`wood` are defined but referenced by no row. `none` does not mean non-solid — collision is a separate attribute. `city/<city>/{floors,walls}.csv` is a different neighborhood-grouping table, not this map. | verified_original | `mm2-inspect materials` on retail; docs/research/materials.md |

## Deliberate rust-mm2 departures (designed, not original claims)

| ID | Policy | Source |
| --- | --- | --- |
| DSN-1 | Handling assists (steering slip, levelling, gearbox) intentionally deviate from retail numbers; per-field provenance is in the `ConversionReport`. | docs/vehicle-handling.md |
| DSN-2 | Rust/Bevy/Avian stack, wgpu rasterized rendering — a modern engine, not a Direct3D emulation; original renderer options (CTL-4) inform features, not implementation. | PROJECT.md, docs/architecture.md |
| DSN-3 | Modern engine-to-engine multiplayer only; no DirectPlay/Zone/serial/modem compatibility. | PROJECT.md |
| DSN-4 | VFS reads installs read-only with deterministic mod overrides. | PROJECT.md, docs/modding.md |
| DSN-5 | Shared race runtime defaults not pinned by authored data: checkpoint vertical band ±8 m, direction-check flag off, 3 s start countdown at 120 Hz. Provisional until real event evidence exists. | `mm2_game::race` constants + docs |
| DSN-6 | Event start without authored `_strtpnts`: the player spawns on the start line facing the row0→row1 tangent — the line is the one point the authored data guarantees is on the course (a 10 m tangent back-off spawned london `blitz6` past the edge of its elevated start deck, over a void; changed 2026-09-20). Checkpoint markers are translucent orange columns, finish a green column — dev-rig visuals, not the original gate rendering. | `mm2_content::race_def`, `mm2_app::race` |
| DSN-7 | Runtime deadline semantics: `TimeLimit` binds as seconds → 120 Hz ticks on Blitz rows only (provisional, UNK-4); the constant values on Checkpoint/Circuit rows stay unbound. The deadline is inclusive — a finish crossing on the expiry tick counts because segments evaluate before the timeout check; expiry records one authoritative `TimedOut` result per unresolved participant. Remaining time displays `m:ss` from the same race clock. | `mm2_content::race_def`, `mm2_game::race`, `mm2_app::race` |
| DSN-8 | Navigation-arrow presentation (RACE-6): a UI needle + diamond tip stands in for the original bitmap arrow (the embedded font is ASCII-only). It is hidden under `Ordered` rules per HUD-2's instrument list, and aims at the armed finish once every gate clears (inferred — RACE-6 names only checkpoints). Cycle keys are X/Z, not the original X/S (CTL-1): `S` is brake under this app's added WASD mapping — an input-map departure, not a rules one. | `mm2_game::race` nav functions, `mm2_app::race` arrow |
| DSN-9 | Low-time warning cue: no documented original rule describes a Blitz low-time warning (HUD-2 lists only the countdown timer among race instruments), so the HUD pulses a `LOW TIME` banner once `time_remaining` reaches 10 s at the race tick rate, alternating bright/dim every 0.5 s of remaining time — derived from the same race ticks that judge the deadline, so it freezes with a pause and clears on resolution. Presentation policy, not an original-behavior claim; an audio cue stays impossible until F07 exists. | `mm2_app::race` `LOW_TIME_TICKS`/`update_race_warning` |
| DSN-10 | Banger runtime slice (F04-A.3 + F04-B.1): bound pathset stamps become one session-owned entity (collider + dormant `Banger` + `ObjectIdentity`/`AuthorityRole`, mesh parts as children) instead of the loose render/collider pair. `activate_bangers` compares `approach_speed × striker_mass` (deepest manifold contact, shared with the impact pipeline) against `ImpulseLimit2` — a provisional stand-in for the unknown original quantity (UNK-22). A qualifying edge flips `RigidBody` to dynamic once, applies one impulse at the striker's approach speed plus a `Size`-derived spin kick, and emits `BangerStateChanged`; a ×32 `BangerPool` (the R4-recovered pool size) reclaims oldest-first; a slept body settles back to `RigidBody::Static` (`dgHitBangerInstance`-like, terminal for the session — the recovered `Timer` despawn is unimplemented). A placement carrying collidable `BREAK<NN>` chunks instead goes `Broken` at the same activation edge (provisional timing, UNK-22): parent collider + mesh children are removed and each piece spawns as its own `Active` fragment body (own `ObjectId`, own `<name>_break<NN>` record or the parent def, convex collider, render children) claiming pool slots — one `BangerStateChanged` per logical break. Predicted sessions never transition banger state. `BirthRule`, audio/flash/decal effects and replication stay deferred; prop-rule placements now stamp through the same dormant-banger path as pathset stamps (WLD-17/UNK-21). | `mm2_game::banger`, `mm2_app::banger`, `docs/research/banger.md` |
| DSN-11 | Results flow (UI-5, documented): the *local* participant's terminal resolution ends the playing session — `advance_race` transitions `Playing → Results` on the same step it records the `Finished`/`TimedOut` result, so the race clock and ledger freeze with the phase. A remote/AI participant resolving while the local driver still races changes nothing (AC03). `RacePhase::Complete` still waits for every participant to resolve; the Results phase surfaces outcome + place + recorded finish time on the HUD line (place per DSN-12) — a full results screen is F17 scope. | `mm2_app::race::advance_race`, `mm2_app::main::update_hud` |
| DSN-12 | Standings ordering (`ResultLedger::standings`): `Finished` outranks `TimedOut` regardless of times, `Finished` orders by the recorded `race_ticks`, and equal ticks order by `PlayerId` — an explicit, deterministic tie-break independent of recording/query order (no verified original placing rule exists; F14 spec demands an explicit tie policy). A participant with no recorded result is unplaced, not ranked last; DNF `TimedOut` ranks below every finish. The HUD shows `FINISHED {ord}[ of {n}]` from this ordering and the smoke record gains `place=`. | `mm2_game::result`, `mm2_app::main::update_hud`, `mm2_app::smoke` |
| DSN-13 | Live running order (`live_order`) — the place indicator's ordering while a race runs (HUD-2 names the instrument; no verified original rule describes it): `Finished` participants lead ordered by their recorded `race_ticks` (same key as DSN-12's standings, so the live order converges to it as everyone resolves), then active participants by progress — `Ordered` counts `(lap, next)`, `AnyOrder` counts cleared gates — progress ties break toward whoever is closer (straight-line XZ) to their *own* current objective (`checkpoints[next]` / `navigation_target`'s nearest remaining gate or armed finish), `TimedOut` trails by `race_ticks`, and every remaining tie orders by `PlayerId`. Straight-line distance is a heuristic, not course distance; results never consume this order — the ledger owns them. The HUD shows `{ord} of {n}` only with ≥2 participants (a place indicator is a competitive instrument); the smoke record reports `pos=` whenever the local participant has a standing. | `mm2_game::race::live_order`, `mm2_app::main::update_hud`, `mm2_app::smoke` |
| DSN-14 | Opponent bounded re-anchor (F15-B.3): an opponent that spends 900 driving frames (~15 s) without 8 m of displacement — penned, hull-beached with unloaded wheels (the scripted stuck detector only counts grounded cars), or knocked off its route — teleports onto the route leg it was chasing through the production `ResetVehicle` path, walked backward along the authored polyline (4 m clearance, further while inside an un-cleared trigger, ≤60 m total). The jump is *disclosed*: `Teleported` breaks the swept segment so it cannot sweep checkpoints, the walk-back keeps the landing out of pending gates, `OpponentDriver::reanchors` counts each assist, and the smoke record surfaces the field total as `opp_rec=`. Authority role only — a predicted client never teleports a participant. No verified original recovery rule exists (retail opponents plausibly despawn/reset; unmeasured), so this is a designed anti-standstill policy, not an original-behavior claim. | `mm2_app::opponents` (`REANCHOR_*`, `reanchor_pose`, `opponent_drive`), `mm2_app::smoke` |
| DSN-15 | Profile store (F16-A.1): our own versioned JSON save format — no compatibility with the retail `players/*.sav`/`*.cfg` binaries is claimed (F16 non-goal). One `<id>.json` per profile under an OS user-data dir (`~/Library/Application Support/rust-mm2/profiles` macOS, `%APPDATA%\rust-mm2\profiles` Windows, `$XDG_DATA_HOME`/`~/.local/share` other Unix); original installs stay read-only. `driver-<n>` ids allocate as max-seen-suffix+1 so a deleted id is never reused; display names may repeat (the id is the identity). Saves are atomic — tmp write + `sync_all`, rotate `.bak`, rename — and `load` falls back to the `.bak` reporting `recovered_from_backup`; corrupt files still `list()` by id and are never deleted by a read. `version` is stamped on every write and any mismatch rejects the file rather than guessing; unknown top-level fields round-trip through a preserved `extra` map. Progress keys are `EventKey{city, table, stem}` — the authored file stem, not the table row index — so a mod inserting a row cannot retarget a saved record. `ProfileKind::Sandbox` gates `records_progress()`, the single check the F16-B reward path must make. DRV-7 is enforced at the store: the last remaining profile cannot be deleted. | `mm2_game::profile` |

## Open questions (unknown until evidenced)

| ID | Question |
| --- | --- |
| UNK-1 | Exact enum maps for `CarType`, `TimeofDay`, `Weather`, `Difficulty` columns (0-3 seen; no key shipped). |
| UNK-2 | Whether `.aimap` files beyond the `mm*data.csv` rosters (e.g. london race12-13, blitz10-12, circuit10-11, sf r0) are selectable events, variants or leftovers. Both cities' `circuit11` lack `.aimap` entirely. |
| UNK-3 | How `vpmoonrover` is unlocked/selected in the original (undocumented; likely cheat code — no verified source). |
| UNK-4 | Blitz `TimeLimit` unit — the runtime binds it as seconds provisionally (DSN-7); the constant 50/40 `TimeLimit` on checkpoint/circuit rows is likely unused — unconfirmed. |
| UNK-5 | `NumLaps` nonzero on non-circuit tables — likely an unused shared column; unconfirmed. |
| UNK-6 | `.info` `Flags`/`UnlockFlags` bit meanings and whether `UnlockScore` gates via Pro points. |
| UNK-7 | Whether race names live in `mmlang.dll` string tables (`.info` descriptions exist for cars; race Description fields are `none`/lesson ids). |
| UNK-8 | Pro-points scoring formula (inputs documented; weights unknown). |
| UNK-9 | Police pursuit AI, sight model, spawn/de-spawn rules beyond "fixed spots" + "line of sight". |
| UNK-10 | C&R gold spawn/hideout positions, scoring values, drop mechanics, respawn timing. |
| UNK-11 | Opponent AI route choice/difficulty model. The roster side is now verified (RACE-11/RACE-12): which aimap variant binds, which vehicle/route each row wires, and how the wired count relates to the table `Opponents` column. Partially measured: the `.opp` `brake` column is mislabelled — a nonzero value marks a staging record whose payload is a heading in vehicle-yaw degrees (row 0 on 592/612 retail files, 542 agreeing with route course direction within ~25°; `race/sf/race5-a-{5,6,7}` carry a second staging row mid-file whose trigger is unknown). Still unverified: the remaining `.opp` columns (`forward/side offset`, `target speed`, `speed/side start` — authored zero on all retail rows, preserved raw; the runtime treats `.opp` as a polyline plus staging), the exact original consumption of the `[Opponent]` parameter tail (decoded into mm2hook's recovered vocabulary as an *inferred* mapping, RACE-14 — including whether `avoidOpponents` ≈ universal-0 on retail means the original never avoids AI or the flag order/polarity is wrong), how the original consumes the driving line vs its own route choice (CHK-5 says opponents take different routes), how the original decodes the `_opp` tuning files' alternate Trans schema and merges them with the base tune (RACE-13 — our sparse-overlay merge is a designed policy), how opponents take grid slots (UNK-17), and what the spare `.opp` files were for. |
| UNK-12 | Traffic/pedestrian ambient models (lanes, lights, panic reactions). `.bai` parses and builds a directed nav graph (`docs/research/bai.md`), `.aimap` overrides parse (`docs/research/aimap.md`) — how the original runtime consumes those remains unverified. `.pathset` placements parse and `city/<city>/{props,decals}.pathset` now stamp ambient prop rows (WLD-13) and decal ribbons (WLD-18); audio/race pathset consumption is still unimplemented. |
| UNK-13 | Exact damage accumulation model and breakaway-part rules (DMG-3 documented only as a feature claim). |
| UNK-14 | Crash Course pass/fail criteria per lesson (time? gates? stunts scored how?). |
| UNK-15 | Whether pedestrians can be struck and what the consequence is. |
| UNK-16 | Narrowed 2026-09-21: the two `a` columns' conventions are now measured (WPT-4) — waypoint `a` is a course bearing, `_strtpnts` `a`/`.opp` staging is vehicle yaw, 180° apart. Still unknown: whether the original enforces gate *direction* (e.g. rejects a backwards crossing) or uses waypoint `a` at runtime at all. |
| UNK-17 | `_strtpnts` row→participant mapping and which authored start set the original consumes per participant: row 0 is treated as the player slot (inferred, WPT-3), opponents take `index + 1` on a grid or the `.opp` row-0 staging point without one — both provisional policies, not verified original behavior. |
| UNK-18 | Which road-id space the over-range london `.aimap` `[Exceptions]` ids (562–815 vs 540 `city/london.bai` roads) address — `london_sup.bai`'s layout is unparsed, so this stays open (WLD-8). |
| UNK-19 | BAI `edgeDistances` semantics — not a monotone outer-edge ordering (profiles like `[7.5, 2.5, 2.5, 7.5]` on one-way sides). The nav graph ranks lanes by measured lateral offset instead (WLD-9). |
| UNK-20 | Pathset per-point `attributes` word, the `OPEN:`/`inactive:`/`open:` name prefixes (event-state decorations on bridge/gate props — inferred), and what the three truncated london files (`blitz10/11`, `london_bridge_blitz10.pathset`) were meant to place. `current_path`/`selection` are inferred dev-tool cursors, preserved raw. Runtime stamping details are likewise unverified: which local axis the directed/line-strip yaw maps (the INST X-axis convention is assumed), whether line-strip spacing restarts per segment or runs continuously (per-segment implemented), whether the final vertex is capped (it is), and what zero spacing means (one prop per vertex implemented). Decal ribbon rendering is likewise unverified: `u` across the pair / `v` along the centre line tiled per `spacing` (5 m default) is inferred from texture contents, palette-alpha blending is inferred from authored alpha values, and whether prop-channel decal names stamp is unknown (the 31 sf `props.pathset` rail copies — 26 byte-identical duplicates — are treated as authoring leftovers). |
| UNK-21 | Prop-rule runtime field semantics (WLD-14/17): the stamping *geometry* is verified — which arcs `n{NN}left`/`n{NN}right` apply to is solved (WLD-17) and the implemented walk resolves every reachable rule room on both cities. Still unknown: whether `start`/`distance`/`maxUse` budget per room-side or per whole path (implemented per room-side), how `file1`–`file4` variants are chosen (deterministic hash implemented; the original's seed is unrecovered), what `minLerp`/`maxLerp` lerp (implemented as curb→outer midpoint; always equal on retail), the original's left/right label convention and prop yaw axis, the encoded non-room `road_rooms` record kind (65 0xx on sf), why 52 sf / 5 london rule-bearing rooms are reached by no path, and what consumes the `props.csv` `Races` group. |
| UNK-22 | Banger runtime semantics (WLD-15/16): what `ImpulseLimit2` is compared against and what crossing it does (break vs. tip vs. nothing) — the implemented `approach_speed × striker_mass` estimate is a provisional stand-in (DSN-10); whether `NumParts` bounds spawned fragments or only echoes the PKG's BREAK count, whether fragments spawn at the activation edge or at a later break threshold (implemented: at activation, one `ImpulseLimit2` gate for both), the `ColliderId`/`AudioId`/`TexNumber` id spaces, `SpinAxis`/`BillFlags`/`Flash`/`YRadius` semantics, the `type: a` tag, when `BirthRule` fires relative to the dormant→active transition, how `default.dgbangerdata` fallback selection works, how a stamped placement acquires `INST_BANGER`, the exact dormant→active→hit/despawn transition conditions, the recovered `Timer` despawn (unimplemented — the slice settles instead) and the ×32 active-pool reclaim order, and whether the 4 fragment records carrying `NumParts>0` mean fragments-of-fragments or inert leftovers. The record↔geometry link, placement binding and R4's recovered class structure are verified; the state machine's thresholds are not. |
| UNK-23 | Surface-material runtime semantics (WLD-19): which consumer performs the texture→`materials.csv`→`materials.mtl` lookup for each surface class (PSDL rooms, PKG bound materials, INST/banger colliders), what material an unmapped texture name gets (`_default` vs none — 5 authored PSDL names and `sliver` have no row), what the two dead refs (`ash`, `mud`) resolve to, the `sound` class table behind values 0/1/2, the `width`/`height`/`depth` volume semantics, the `ptxindex`/`ptxthreshold` particle index space, whether `effect` ever differs from `none`, how `friction`/`elasticity`/`drag` combine with tire parameters in the original force path, and whether the `<stem>-NNNN` frame→base lookup matches original name normalization or the original strips suffixes differently. `dirt`/`wood` definitions exist but no csv row references them — their entry point (bound materials? fallback?) is unverified. Runtime note (F06-A): the implemented importer classifies PSDL colliders by authored material index and gives `none`/blank/unmapped/dead-ref slots `SurfaceMaterial::Unspecified` — a documented conservative *implementation* policy, not an answer to the `_default` question above. Runtime note (F06-B): the tire path consumes `friction` normalized against the `_default` block (`_default` → 1.0; retail water ≈ 0.76, deepwater ≈ 0.72), multiplied by a separate environment term (`TireConditions`, `--traction` dev override pending F18) — an *implementation* scaling, not a recovered original rule. Runtime note (F06-B.2): `drag` is consumed raw as a per-wheel viscous wading resistance (`drag × load` opposing contact-plane motion — only water/deepwater carry it on retail, so a car crawls ~1 m/s on the Thames surface while dry land is unchanged) and `elasticity` is consumed as the collider's Avian restitution scaled ×0.1 (the same conservative cap `convert` applies to `BoundElasticity`) — both *implementation* mappings, not recovered original formulas. `sound`, `effect`, `width`/`height`/`depth` and `ptx*` still have no consumer. |

## Gaps in this ledger

- `MM2HELP.HLP` topics on joystick/gamepad/wheel button maps and
  serial/modem setup are documented but not yet transcribed into rules;
  they matter for F23, not yet needed.
- No observable-behavior verification yet (no original executable run,
  no packet capture): everything marked *documented* is doc-trust only.
- Race *names* shown in the UI are unverified (UNK-7) — the CSV
  `Description` column is `none` for all race events.
