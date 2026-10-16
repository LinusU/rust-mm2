# Coverage and gap audit (F31-A.1)

A reconciliation of the original-feature scope (F00–F30) against what the
checkout actually does and what evidence exists for it. It is a *map of
gaps*, not a completion claim: nothing here promotes a feature, and a row
reading "code-checked" says nothing about original-game fidelity.

**Provenance.** Denominators in §2 were measured on 2026-10-07 at commit
`f145ece` against the read-only retail install (fingerprint
`fnv1a64:e91e6cd4b2ae30d9`, Apple M1) with the `mm2-inspect` commands named
beside them. The per-feature rows in §3 are reconciled from
Ralph's task table (`docs/ralph/PLAN.md` as of `18b469d`; the tasks now
live in Rally), the feature specs' acceptance lists and
`docs/original-rules.md`; they were *not* re-executed by this audit, so each
inherits the evidence level its plan row records. Re-run §2's commands to
refresh; update §3 when a feature's evidence changes.

## 1. Evidence levels

| Tag | Meaning | Does not show |
|---|---|---|
| **code** | fmt/clippy/workspace tests pass at a commit | original content loaded, anything audible/visible, feel |
| **synthetic** | self-authored fixtures through the production path | every retail variant |
| **retail-data** | the fingerprinted install was parsed/driven headlessly | a human looked/listened; original fidelity |
| **rendered/played** | a capture or interactive run was observed | universal platform/content support |
| **original-verified** | a rule confirmed against retail data/help/binary (ledger `verified_original`) | anything about a `designed` rule |

Deliberate departures from the original (arcade handling, modern input,
engine-to-engine netcode, designed catch-up/recovery policies) are
*enhanced policy*, listed in `docs/vehicle-handling.md` and as `designed`
rows in `docs/original-rules.md`; they are not gaps.

## 2. Catalog denominators (retail install)

`mm2-inspect inventory <install>` — expected / discovered / accepted /
rejected:

| Family | Expected | Discovered | Accepted | Rejected | Note |
|---|---|---|---|---|---|
| cities | 2 | 5 | 5 | 0 | extras `city`, `sfai`, `variant` |
| player vehicles | 21 | 29 | 21 | 8 | rejects are incomplete spare/variant ids, kept in the denominator (`vpdb731`, `vpeagle`, `vpftruck`, `vplafrance`, `vpvw_cup`, `vpvw_dune`, `vpvwcup_angel`, `vpvwdune`) |
| races | 80 | 111 | 78 | 20 | `circuit11` ×2 partial (no `.aimap`); 18 non-content artefacts |
| crash-course lessons | 42 | 42 | 42 | 0 | structural only — see F21 |
| placement sources | 2 | 13 | 13 | 0 | |
| audio families | 7 | 7 | 7 | 0 | 3256 files classified, 0 failures (`mm2-inspect audio`) |
| pedestrian archetypes | 4 | 5 | 4 | 4 | `pedmodel_wolf` partial; 3 CVS records |
| ambient traffic vehicles | – | 23 | 0 | 1 | `va_garbagetruck` lacks model/AI data |
| breakable props | – | 995 | – | – | no expected count: unverified |

Event catalog (`mm2-inspect events`): 45 selectable rows per city
(12 Checkpoint + 10 Blitz + 10 Circuit + 13 Crash Course) = 90. `race-defs`
builds 64 definitions per city across both difficulties and reports 26
Crash Course (row × difficulty) cases as `unsupported` (they run through the
F21 lesson path, not `RaceDefinition`); `event --all` reports all 45 rows per
city `ready` but strict failures of 53 (london, 33 extra stems not cataloged)
and 43 (sf, 23 extra stems), plus header anomalies such as the retail
`AmbDenisty` typo, kept as diagnostics. "Ready" means *dependencies resolve*;
it is not "playable and verified".

Unverified denominators the inventory deliberately reports as `unverified`
rather than hiding: 111 race records, 157 placement records, 3293 audio files,
42 lessons, 72 pedestrian records, 131 multiplayer variants. The rules
ledger counts 82 `verified_original`, 66 `documented`, 8 `inferred`, 10
`designed` rows and 41 open `UNK-*` entries.

## 3. Feature matrix

`Status` is the plan's own (no row here is *checked* beyond what the plan
says). `Best evidence` is the strongest level any slice reached. Gaps are the
plan's recorded remainder plus acceptance criteria with no direct evidence.

| Feature | Status | Best evidence | Principal open gaps |
|---|---|---|---|
| F00 baseline/audit/harness | checked | retail-data | none for the harness; every later family's *denominator* inherits §2 |
| F01 session + contracts | checked/implemented | synthetic | ownership diagram doc (AC06); menu-driven repeated transitions with a window |
| F02 vehicles/handling | implemented | retail-data (21/21 headless matrix) + rendered (per-car leg) | 8 incomplete extras stay rejected; gap list vs retail numbers owed; GPU render check per car is local-only |
| F03 world placement | implemented | retail-data | sampled original-location comparison; race-cleanup and mod-replacement end to end; UNK-20/21 |
| F04 breakables | implemented | retail-data (strike runs) | break threshold semantics (UNK-22), `NumParts` role, birth-rule effects, natural >32 pool reclaim |
| F05 damage/recovery | checked (A) / active | synthetic + retail-data | damage-driven detachment (UNK-13); recovery on articulated cars; replication |
| F06 surfaces/traction | active | synthetic (six slices, mutation-checked) + retail-data (sf weather × surface in the real force path) + loopback two-process (AC06 authority-owned pin) | UNK-23/39 (original wetness rule unknown — policy is *designed*); `effect` and `width`/`height`/`depth` still have no consumer |
| F07 vehicle/impact audio | active | synthetic + retail-data decode | **no audible capture recorded** (AC05); UNK-25 |
| F08 ambience/commentary/music | active (A, B, C) | retail-data (family matrix, speech-clip census) + synthetic + offline mix capture (ambience/voice/UI) | commentary beyond pre-race weather/time cues, music (DirectMusic: no synth, no capture), 90 authored speech draws with no wave; no listening evidence |
| F09 road network | implemented | retail-data | AC04 sampled-road overlay comparison in both cities is local/visual only; UNK-18/19 |
| F10 ambient traffic | active | retail-data (soak); authored signal census | per-AC tests and evidence levels in `docs/traffic-coverage.md` (AC02 intersection flows, AC04 occupied-space rejection, AC06 race density/speed overrides + restart are synthetic-covered there); right-of-way/state fidelity (UNK-12; signal-unit geometry and the `trafficLightAxis` convention are now measured, docs/research/bai.md), player-hit feel (manual), original population constants |
| F11 race catalog/runtime | checked (A) / active | retail-data (AC01, AC06); synthetic (AC02–05) | per-AC commands, tests and levels in `docs/race-coverage.md` §"Evidence → F11 acceptance"; open: no human playthrough, 26 Crash Course cases per city `unsupported` (F21), retail `event --all --strict` exits 2 on disclosed authored anomalies |
| F12 Blitz | implemented | retail-data (headless 20/20) | no human play of representative events; reward leg (F16); UNK-4 time unit provisional |
| F13 Checkpoint | active | retail-data | original-fidelity comparison (placing, pacing, AI competence) absent; traversal stalls on london-2/5/6/9, sf-7/9 |
| F14 Circuit | implemented | retail-data | catalog audit recorded (`docs/research/circuit-completability.md`, `scripts/circuit-audit.sh`): 20/20 load; the original 597 s scripted sweep left 13/20 short, and the neural-driver re-measure (#1400 sf circuit:2/5/6/7, #1402 all ten London circuits) completes every circuit it re-ran at both difficulties (the only one slower than the 12-minute cap is `london circuit:8` Professional, last car at 806 s — slowness, not a stall); the route-defect screen flags no circuit line except `london circuit:8`; UNK-11/38 |
| F15 opponent AI | active | retail-data (controller read from `Midtown2.exe`) | `cornerBrakingThreshold` consumed (designed demand); `unkFlag` (no reader) and `weirdPathfinding` (no candidate search to reorder) deliberately unconsumed; designed readings stand in |
| F16 profiles/unlocks | active | synthetic + retail-data + two-process restart (AC01: CLI-created profiles, eligible finish in the test process) | AC01 interactive finish in a real process (`--bot` results deliberately ineligible); UNK-6/8 |
| F17 menu/flow | active | synthetic + local render | AC03 keyboard/gamepad + visible-focus evidence; C&R/scoring result variants; no human journey recorded |
| F18 weather/time | active | synthetic + retail-data + rendered (16-slot sf capture matrix) | AC05 audio captures (visual half now covered by the sf weather×time capture matrix); network replication; UNK-24/39/40 — `.ldef`/`.lmap` settled 2026-10-11 (`.ldef` has no exe reference; `.lmap` value recovered but deliberately unbound, see `environment.md`) |
| F19 pedestrians | implemented (A + B + C) | retail-data (parsers, skin/deform domain types, sidewalk-net census, reaction chains, `mm2-inspect peds --strict` coverage: 4/4 expected archetypes supported by the runtime loader) + synthetic runtime (crowd soak, reaction machine) + retail headless crowd soak (`pedestrian_retail.rs`, both cities: 24/24 walkers inside the actor budget, corner hops on real sidewalks, London provoked a wary→dive→rejoin cycle, a second fresh process restored every crowd counter verbatim) + rendered (`--ped-lab` captures: STAND/WALK/ANTIC/mid-dive poses coherent on all four archetypes, fielded crowd on sidewalks) | open: the original's crossing behaviour (UNK-42; crosswalk sites measured and walkable, DSN-106), the original car-sensing rule (UNK-43), pedestrian audio, human play-test judgement (owner); UNK-15/41 |
| F20 police | active | synthetic + retail-data | pursuit rules unknown (UNK-9) — behaviour is *designed*; trigger/loss/outcome matrix and session restrictions |
| F21 Crash Course | active | structural catalog only | **evaluators unrecovered** (UNK-14/35): lesson-by-lesson pass/fail, instruction/voice flow, reward credit; 42 lessons not playable as a course |
| F22 HUD/map/cameras | active | synthetic + retail-data (map coordinates in both cities; roster camera framing) + local render | UNK-26…37 presentation details are inferred; AC05 visual inspection on atypical vehicles (numbers + capture commands in the F22-C hand-off) |
| F23 controls/options | active | synthetic | audio/graphics option *effect* (no output device test), accessibility, hot-plug, wheel/force-feedback unverified |
| F24 multiplayer session | active | synthetic + real loopback processes | LAN/Internet matrix, dead-but-open link, NAT, lobby UI polish; reports must stay loopback-labelled |
| F25 networked driving | active | loopback, impairment matrix measured | sub-epoch drift, full input-replay prediction, LAN/Internet scope, rejoin |
| F26 shared world/races | active | synthetic + loopback | late-joiner bulk ledger, spectator/race-ends-during-late-join, two-process breakdown leg |
| F27 Cops & Robbers | active | synthetic + loopback | what knocks gold loose, pickup radius, respawn, disconnect/out-of-bounds (UNK-10) — stock variants each need status (AC06) |
| F28 city specials | active | retail-data | per-city actor record below; cable car runs partially (4 cars, 2 circuits in SF, signal-gated; audio and init gate unrecovered — UNK-44); `[Hookmen]` semantics (UNK-45); late-join phase (F26) |
| F29 modding | active | synthetic | per-consumer override tests exist for car texture, handling, prop, audio cue (`mod_override.rs`) and race rules — Checkpoint, Blitz and Circuit tables, waypoints, grids, and the `.aimap`/`.opp` roster with a malformed aimap refused (`mod_override_race.rs`); a Crash Course lesson's malformed aimap is refused too (`lesson_launch.rs`); a mod's lesson sub-event table, lead-car aimap and `.opp` route reach `lesson_race_setup`, and an unusable lead-car route is refused (`mod_override_race.rs`, `lesson_launch.rs`); a mod's `tune/<city>.cinfo` renames menu races and classifies cosmetic (`menu.rs`), a malformed one falls back to its own stem labels rather than the shadowed stock names (`menu.rs`); a malformed selected override is refused, not papered over, across car texture (unknown and truncated `.tex`), model (non-`PKG2`/`PKG3` container) and audio cue (`mod_override.rs`); menu art has a VFS consumer (`menu_art_image`; override/revert/malformed/cosmetic in `mod_override.rs`); `mmlang.dll` strings are deliberately not a mod surface (`docs/modding.md`); open: a real-install run beyond `--trace-deps --city` |
| F30 performance/packaging | active | local measurement | release-profile baseline for named scenes; memory/draw-call/voice/bandwidth columns; packaging step, real OS layouts |
| F31 final audit | A.1 this doc; B.1 synthetic single-player journey; B.2 synthetic negative-path journeys; B.3 synthetic lesson/C&R journeys; B.4 multiplayer journey (synthetic fixture, loopback, lossy link); B.5 retail-data single-player journey; B.6 retail-data multiplayer journey (two processes, loopback); C.1 `docs/release-report.md` (final acceptance runs at `d42e6ef`) | synthetic + loopback + retail-data | B.1: `menu::a_named_driver_races_earns_a_reward_and_finds_it_after_a_relaunch` (name a driver → locked paint/race → launch → authored finish → results → restart → menu → relaunch). B.2: `menu::an_unreadable_profile_degrades_visibly_and_other_profiles_survive`, `menu::pausing_or_losing_focus_mid_race_freezes_the_clock_and_awards_nothing`, `menu::a_duplicate_result_does_not_double_award_the_reward` (the game has no auto-pause on focus loss: an unfocused window releases input, Esc-pause freezes the clock). B.4: `net_app::a_multiplayer_journey_survives_a_lossy_link` (host app + two joined client apps, each behind a seeded `ImpairProxy` with 20 % loss, duplication, reorder and jitter: lobby → host edits the mode to a Checkpoint event → race → host finish and deadline `Results` on all three ledgers → host ends the round → rematch with changed difficulty/seed, no reconnect → one client leaves, the other's roster converges, then it leaves). The journey fixed one one-shot message (the lobby's session ad now repeats on the keepalive tick) and found one-shot messages that still needed work, filed rather than routed around: `SetReady`/`SetVehicle` (#1488, made loss-safe by the link's intent repeat, which the test relies on) and `Start`/`Cancel` (#1489, since made loss-safe — the host repeats each for a bounded window, the client ignores a repeat, and the test crosses them on the lossy lane). Synthetic fixture race definition, loopback only. B.3: `menu::a_lesson_fails_retries_passes_and_returns_to_the_menu` (Events → Crash Course → idle lesson times out and fails, nothing credited → *Retry lesson* into a fresh generation → drive the gates → pass saved on the profile → *Continue to menu* → midterm open) and `menu::a_cops_and_robbers_round_is_played_from_the_menu_and_left` (Cops & Robbers → Start match → local car seated → drive to the gold and pick it up → clock out decides the match → *Play again* → quit → menu, match torn down). Both run on self-authored fixtures: the original lesson evaluators (UNK-14/35) and C&R knock-loose/pickup-radius/respawn/out-of-bounds rules (UNK-10) stay open, and no AI cops/robbers exist in the single-player round. B.5: `menu::a_retail_stock_event_drives_from_the_menu_to_a_finished_result` (needs `MM2_RETAIL`; original-content validation, not human play: name a driver → Events → `london` → Blitz → the authored "London's Calling" (`blitz0`) → the session loads the real `city/london.psdl` world and its 3-gate + finish 25 s course → the production scripted evidence driver (the same `scripted::scripted_drive` the `--bot` CLI schedules, at the binary's 60 Hz) races it to a real `Finished` result → Results; the scripted finish is record-ineligible (DSN-16) so the profile is left uncredited and the report says so, and *Restart race* → quit returns to the menu). Command: `MM2_RETAIL=<install> cargo test -p mm2_app --test app -- a_retail_stock_event`. B.6: `net_drive::a_retail_stock_event_journey_runs_from_the_lobby_to_leave` (needs `MM2_RETAIL`; original-content validation over loopback, not human play: `mm2 --host --city london --event blitz:0 --bot` plus one `mm2 --join --parked --until-results` process; the client readies in the lobby, the host starts, both load the real `city/london.psdl` world and the 3-gate `blitz0` course, the host's scripted driver finishes it (`outcome=finished`, `cp=3/3`) and the authority times the parked client out (`outcome=timed-out`); both records show `phase=results`, `mp=gen1` and a recorded ledger (`results=2`), then the client leaves and the host's frame cap ends its run; the real processes use the production `SetReady`/`SetVehicle` path on a clean link, so no #1488 re-issue workaround is needed). Command: `MM2_RETAIL=<install> cargo test -p mm2_app --test network -- a_retail_stock_event_journey`. Open: an *eligible* retail reward leg (a human-driven finish banked to the profile); a lossy-link or two-client retail journey, and a host end-of-round with a client still seated (the host record prints only at process exit) |

### F10 ambient traffic evidence (F10-AC01 / F10-AC05)

Recorded 2026-10-13 with `MM2_RETAIL` set, original-content validation level.
Command (also asserted by `cargo test -p mm2_app --test app traffic_retail`):

```sh
cargo run -p mm2_app --bin mm2 -- --headless --mm2-path "$MM2_RETAIL" --city <london|sf> --frames 3600
```

The smoke record's `tty=<n>/<id>:<count>,...` is the census of every ambient
car spawned in the 60 s run; `hdg=<agreeing>/<audited>` audits each live lane
follower at the end of the run (nose within ~8 degrees of the authored lane or
junction-crossing tangent at its cursor, velocity not against it).

| city | finite | traf | stuck | jmp | distinct types spawned | hdg |
|------|--------|------|-------|-----|------------------------|-----|
| london | true | 16/16 | 0 | 0 | 10 of 12 rostered (`va_2sitersport_l`, `va_compact_s`, `va_cooper_l`, `va_ddbus_l`, `va_euro_l`, `va_eurocargo_l`, `va_largesuv_s`, `va_sedans_s`, `va_smallsuv_s`, `va_ultrasport_l`); sp=21 | 15/15 |
| sf | true | 16/16 | 0 | 0 | 11 of 11 rostered (`va_2siter_s`, `va_bus_f`, `va_compact_s`, `va_diesels_s`, `va_eurovan_f`, `va_largesuv_s`, `va_limo_f`, `va_sedans_s`, `va_smallsuv_s`, `va_taxi_f`, `va_wedelivertruck_f`); sp=82 | 9/9 |

Limits: one fixed seed, 60 s per city. The direction audit compares each car's
pose and velocity with the *graph's* lane tangent (built from the BAI lane point
order), so it catches pose/cursor disagreement, not a misreading of the BAI
itself; the authored-direction convention is `docs/research/bai.md`. Whether
the cars *look* right on the road is not measured. Population constants stay
designed (UNK-12).

### F28 city-special-content evidence (F28-AC01 / F28-AC06)

Recorded 2026-10-15 with `MM2_RETAIL` set, original-content validation
level, `fnv1a64:e91e6cd4b2ae30d9`. Command (exit 0; the same run with
`--strict` also exits 0, i.e. no data failure):

```sh
mm2-inspect specials "$MM2_RETAIL"
```

The tool inventories each city *from the installation* — no actor list is
hard-coded — parses every pathset with the production parser, and resolves
each path's model the way its manager resolves it. `--strict` fails only on
a data failure (a missing expected file, a parse failure, a model that
resolves neither by name nor as the family default, an unreadable
water/BAI record, a cable-car terminus with no route); an unresolved actor
is listed and counted, not failed. Retail summary: **expected default
pathsets 9/9, pathset files loaded 20/21, 0 failure(s), 0 unresolved
actor(s), 1 partially reproduced** (the cable car). The one file not
loaded is the unreachable, truncated `london_bridge_blitz10`
(`drawbridge.md`). Every row below is an actor the install authors; a city
that authors none is listed as *absent*, never invented. Source keys R3,
R4, R5 (`SOURCES.md`); the load rules are WLD-26 (drawbridge), WLD-28
(movers) and the cable car's recovered init/controller (UNK-44).

**London** — 45 catalogued events; 102 `.aimap` files read.

| Special actor | Authored source | Status | Reference |
|---|---|---|---|
| drawbridge leaves | `race/london/london_bridge.pathset` (4 paths; event overlays `circuit0`, `crash3`, `crash5`, `race0`) | implemented — `mm2_app::drawbridge` | WLD-26; `docs/research/drawbridge.md` |
| sailboats (tugs, water taxis, sailboards, ducks) | `race/london/london_sailboat.pathset` (16 paths) | implemented — `mm2_app::movers` | WLD-28; `docs/research/movers.md` |
| ferries | `race/london/london_ferry.pathset` (6 paths; Crash Course overlays `crash9`, `crash11`, `crash12`) | implemented — `mm2_app::movers` | WLD-28; `docs/research/movers.md` |
| Underground trains (3 cars per path) | `race/london/london_train.pathset` (8 paths) | implemented — `mm2_app::movers` | WLD-28; `docs/research/movers.md` |
| parked-car strips | `race/london/london_parkedcar.pathset` (91 strips; overlays `crash0`, `crash3`) | implemented — `mm2_app::city::spawn_parked_cars` | `docs/research/parked.md` |
| cable cars | none — `city/london.bai` authors 28 train curves, 0 tram curves, 0 tram-line termini (0 one-sided tram roads) | absent (authored that way) | UNK-44; `docs/research/specials.md` |
| water / recovery rooms | `city/london.water` level −3.8, 3 room refs | implemented — `mm2_app::water`, `mm2_app::recovery` | WLD-23 / DSN-34; `docs/research/environment.md` |
| `[Subway]` AI-map train | 0 of 102 `.aimap` files define one | absent (a mod's row would list unresolved) | WLD-31; `docs/research/specials.md` |
| `[Hookmen]` | 0 rows over 102 `.aimap` files | absent; semantics unknown (UNK-45) | WLD-31 / UNK-45 |

Two overlays load but no catalogued event selects them
(`london_bridge_multi`, `london_parkedcar_test`); they are authored
leftovers, shown not failed.

**San Francisco** — 45 catalogued events; 107 `.aimap` files read.

| Special actor | Authored source | Status | Reference |
|---|---|---|---|
| drawbridge leaves | `race/sf/sf_bridge.pathset` (1 path) | implemented — `mm2_app::drawbridge` | WLD-26; `docs/research/drawbridge.md` |
| sailboats | `race/sf/sf_sailboat.pathset` (16 paths: 12 name their model, 4 fall back to the family default) | implemented — `mm2_app::movers` | WLD-28; `docs/research/movers.md` |
| ferries | `race/sf/sf_ferry.pathset` (1 path) | implemented — `mm2_app::movers` | WLD-28; `docs/research/movers.md` |
| Underground trains | none — SF authors no `sf_train.pathset` (not in `EXPECTED_SPECIAL_PATHSETS`) | absent (authored that way) | `docs/research/specials.md` |
| parked-car strips | `race/sf/sf_parkedcar.pathset` (55 strips) | implemented — `mm2_app::city::spawn_parked_cars` | `docs/research/parked.md` |
| cable cars | `city/sf.bai`: 42 tram curves on 21/379 roads, 4 tram-line termini → 4 cars on 2 closed circuits; assets `geometry/va_cablecar_f.pkg`, `bound/va_cablecar_f_bound.bnd`, `tune/vehicle/va_cablecar_f.aivehicledata` all resolve | **partially reproduced** — `mm2_app::cablecar` (signal-gated; audio and the init gate not reproduced) | UNK-44; `docs/research/specials.md` |
| water / recovery rooms | `city/sf.water` level −1.9, 3 room refs | implemented — `mm2_app::water`, `mm2_app::recovery` | WLD-23 / DSN-34; `docs/research/environment.md` |
| `[Subway]` AI-map train | 0 of 107 `.aimap` files define one | absent (a mod's row would list unresolved) | WLD-31; `docs/research/specials.md` |
| `[Hookmen]` | 0 rows over 107 `.aimap` files | absent; semantics unknown (UNK-45) | WLD-31 / UNK-45 |

**Open items this record carries explicitly unresolved (F28-AC01).**

- Cable-car object audio — the `cablecar`, `cablecarstart`/`cablecarstop`,
  `cablecarbell*` and `streetcable` clips ship; no trigger is recovered
  (UNK-44).
- The `+0x40` init gate of `AIMAP.Init`'s parameter block: what sets it is
  unrecovered, so whether every original San Francisco session spawns the
  four cars is unknown; the port spawns them in every local session
  (UNK-44).
- `[Hookmen]` semantics: what the row's first name means, its fourth float
  and flag, and what a hookman does (UNK-45). Retail authors none.
- `[Subway]` has a parser (`mm2_formats::aimap::Aimap::subway`) but no
  runtime; retail's one `[Subway]` line is a `#` comment, so the step is
  never reached.
- Cable-car side-of-road, AI-bubble behaviour and the `+0x4c` model offset
  (all in UNK-44).

**AC mapping.** *F28-AC01* (each expected actor inventoried, implemented
or explicitly unresolved with a source reference) — satisfied at
original-content level by the two tables above and the `mm2-inspect
specials` run they summarise; the actor set is the install's own files
plus the executable's AI-map init (`[Subway]`, `[Hookmen]`). *F28-AC06*
(both cities' coverage reported without inventing actors) — the 9/9
expected default pathsets, the per-city counts and the absent rows (SF
Underground, London cable cars) come from the fingerprinted install; no
actor outside it is named. The cable car stays **partial** and the four
open items above stay open, so the F28 feature row is *active*, not
checked. Rendered and playtest evidence for F28-AC02–AC05 lives in
`docs/research/specials.md` §"What the port does" and the `mm2_app` tests
it names.

## 4. Cross-cutting gaps (not owned by one feature)

1. **Audio is unlistened.** The mixer, decoders and cue logic have code and
   synthetic evidence; the plan records no offline mix or captured listen
   (F07-AC05, F08-AC05, F18-AC05, F23-AC05).
2. **No recorded human playthrough** for any journey. Every race/lesson
   result is a headless or scripted run. The single-player
   profile→select→load→race→outcome→menu path is covered on synthetic
   fixtures (F31-B.1/B.2/B.3) and on one real stock event (F31-B.5,
   `menu::a_retail_stock_event_drives_from_the_menu_to_a_finished_result`,
   `MM2_RETAIL`-gated), but the retail leg drives with the production
   scripted evidence driver, whose finish is record-ineligible (DSN-16):
   an *eligible* retail reward leg — a human-driven finish banked to the
   profile — and a human playthrough remain open. F31-AC03's multiplayer
   lobby→mode→result→rematch→disconnect journey is covered on a synthetic
   fixture over loopback (F31-B.4), and a retail-data multiplayer journey
   runs two real processes over loopback on London Blitz:0 (F31-B.6,
   `net_drive::a_retail_stock_event_journey_runs_from_the_lobby_to_leave`,
   `MM2_RETAIL`-gated, clean link, scripted host driver — original-content
   validation, not human play). That leg does not exercise a lossy link,
   the host's end-of-round/rematch with a client still seated, or a second
   client; those stay the synthetic B.4 leg's.
3. **Original-fidelity comparisons are absent.** Placing, pacing, AI
   difficulty, traction-in-rain, damage thresholds and police behaviour are
   engine self-metrics or `designed` readings; the UNK list in
   `docs/original-rules.md` is the authoritative open set.
4. **Crash Course is the remaining scope hole** where parsed data exists
   but gameplay does not (F21 evaluators). It is not covered by a
   stand-in and stays in the denominator. (F19's runtime crowd landed:
   walk, density, recycle and reactions are on main — the open F19 set
   is UNK-42/43, audio and rendered inspection.)
5. **Multiplayer is loopback only.** No LAN or Internet evidence exists; the
   Cops & Robbers rule core is built but several original rules are unknown.
6. **Mod coverage is uneven.** Override infrastructure exists, but no audit
   shows which consumers honour it (F29-AC01/AC05).
7. **Rejected stock-adjacent content stays rejected** (8 vehicle extras,
   2 partial circuits, `va_garbagetruck`, `pedmodel_wolf`); none is silently
   substituted, and none is needed for the 21-car roster.

## 5. Suggested priority order (a recommendation, not a plan change)

1. F21 evaluator recovery — the largest scoped-out *gameplay* hole whose
   data is already inventoried.
2. F19 open semantics — the original's crossing behaviour (UNK-42) and the original
   car-sensing rule (UNK-43) are the remaining pedestrian gameplay gaps;
   the runtime crowd (walk/density/recycle/react) is on main.
3. An audible offline mix harness (closes four audio ACs at once).
4. F29 consumer coverage for the remaining record families (surface tables, menu art, localization).
5. An *eligible* retail reward leg: a human-driven (or otherwise
   record-eligible) finish banked to the profile. F31-B.5 already covers
   the retail menu → session → real course → results path with the
   scripted evidence driver, which deliberately banks nothing (DSN-16).
6. LAN-scope two-machine run once an operator can supply the second host.

## 6. Refreshing this audit

```sh
mm2-inspect inventory <install> [--strict]
mm2-inspect events <install>
mm2-inspect race-defs <install>
mm2-inspect event <install> --all
mm2-inspect audio <install>
```

`--strict` fails on an empty required catalog or missing data; the rejected
rows in §2 are why a strict retail run is expected to exit non-zero today.
