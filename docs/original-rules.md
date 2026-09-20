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

## Blitz

| ID | Rule | Class | Source |
| --- | --- | --- | --- |
| BLZ-1 | Solo race against a countdown timer: clear all checkpoints in any order and reach the finish before time runs out. | documented | help:Blitz Race |
| BLZ-2 | No opponents and no police in Blitz events. | verified_original | data:`mmblitzdata.csv` — Opponents=0, Cops=0 in all 40 rows |
| BLZ-3 | Per-event time limits are authored (25-120 amateur, 18-103 professional on retail). Unit unverified (UNK-4). | verified_original | data:`mmblitzdata.csv` TimeLimit |
| BLZ-4 | Blitz has ambient traffic and pedestrians; per-event densities are authored (am 0.0-0.7/0.1-0.4; pro 0.2-0.9/0.1-0.5). | verified_original | data:`mmblitzdata.csv` Ambient/Peds |

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
| MP-9 | MP-specific data is thin: `.aimap_p` variants (99), `copchase`/`multicop`/`*_p` records exist but no C&R-specific data discovered. | verified_original | `mm2-inspect inventory` multiplayer family |

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

## Deliberate rust-mm2 departures (designed, not original claims)

| ID | Policy | Source |
| --- | --- | --- |
| DSN-1 | Handling assists (steering slip, levelling, gearbox) intentionally deviate from retail numbers; per-field provenance is in the `ConversionReport`. | docs/vehicle-handling.md |
| DSN-2 | Rust/Bevy/Avian stack, wgpu rasterized rendering — a modern engine, not a Direct3D emulation; original renderer options (CTL-4) inform features, not implementation. | PROJECT.md, docs/architecture.md |
| DSN-3 | Modern engine-to-engine multiplayer only; no DirectPlay/Zone/serial/modem compatibility. | PROJECT.md |
| DSN-4 | VFS reads installs read-only with deterministic mod overrides. | PROJECT.md, docs/modding.md |
| DSN-5 | Shared race runtime defaults not pinned by authored data: checkpoint vertical band ±8 m, direction-check flag off, 3 s start countdown at 120 Hz. Provisional until real event evidence exists. | `mm2_game::race` constants + docs |

## Open questions (unknown until evidenced)

| ID | Question |
| --- | --- |
| UNK-1 | Exact enum maps for `CarType`, `TimeofDay`, `Weather`, `Difficulty` columns (0-3 seen; no key shipped). |
| UNK-2 | Whether `.aimap` files beyond the `mm*data.csv` rosters (e.g. london race12-13, blitz10-12, circuit10-11, sf r0) are selectable events, variants or leftovers. Both cities' `circuit11` lack `.aimap` entirely. |
| UNK-3 | How `vpmoonrover` is unlocked/selected in the original (undocumented; likely cheat code — no verified source). |
| UNK-4 | Blitz `TimeLimit` unit (seconds assumed, unverified); the constant 50/40 `TimeLimit` on checkpoint/circuit rows is likely unused — unconfirmed. |
| UNK-5 | `NumLaps` nonzero on non-circuit tables — likely an unused shared column; unconfirmed. |
| UNK-6 | `.info` `Flags`/`UnlockFlags` bit meanings and whether `UnlockScore` gates via Pro points. |
| UNK-7 | Whether race names live in `mmlang.dll` string tables (`.info` descriptions exist for cars; race Description fields are `none`/lesson ids). |
| UNK-8 | Pro-points scoring formula (inputs documented; weights unknown). |
| UNK-9 | Police pursuit AI, sight model, spawn/de-spawn rules beyond "fixed spots" + "line of sight". |
| UNK-10 | C&R gold spawn/hideout positions, scoring values, drop mechanics, respawn timing. |
| UNK-11 | Opponent AI route choice/difficulty model; `.opp` record semantics (612 files, unparsed). |
| UNK-12 | Traffic/pedestrian ambient models (lanes, lights, panic reactions); `.bai`/`.pathset` semantics unparsed. |
| UNK-13 | Exact damage accumulation model and breakaway-part rules (DMG-3 documented only as a feature claim). |
| UNK-14 | Crash Course pass/fail criteria per lesson (time? gates? stunts scored how?). |
| UNK-15 | Whether pedestrians can be struck and what the consequence is. |

## Gaps in this ledger

- `MM2HELP.HLP` topics on joystick/gamepad/wheel button maps and
  serial/modem setup are documented but not yet transcribed into rules;
  they matter for F23, not yet needed.
- No observable-behavior verification yet (no original executable run,
  no packet capture): everything marked *documented* is doc-trust only.
- Race *names* shown in the UI are unverified (UNK-7) — the CSV
  `Description` column is `none` for all race events.
