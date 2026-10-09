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
| F08 ambience/commentary/music | active (A), B/C queued | retail-data (classification) | commentary beyond pre-race weather/time cues, music (DirectMusic), AC05 capture; no listening evidence |
| F09 road network | implemented | retail-data | AC04 sampled-road overlay comparison in both cities is local/visual only; UNK-18/19 |
| F10 ambient traffic | active | retail-data (soak) | signals/right-of-way fidelity (UNK-12), player-hit feel (manual), original population constants |
| F11 race catalog/runtime | checked (A) / active | retail-data | AC02–05 rest on runtime-slice tests, not an aggregate matrix |
| F12 Blitz | implemented | retail-data (headless 20/20) | no human play of representative events; reward leg (F16); UNK-4 time unit provisional |
| F13 Checkpoint | active | retail-data | original-fidelity comparison (placing, pacing, AI competence) absent; traversal stalls on london-2/5/6/9, sf-7/9 |
| F14 Circuit | implemented | retail-data | same stalls; catalog-wide completability claim gated on F15-B; UNK-11/38 |
| F15 opponent AI | active | retail-data (controller read from `Midtown2.exe`) | `unkFlag`/`cornerBrakingThreshold`/`weirdPathfinding` consumption; designed readings stand in |
| F16 profiles/unlocks | active | synthetic + retail-data | AC01 process-level interactive finish (`--bot` results deliberately ineligible); UNK-6/8 |
| F17 menu/flow | active | synthetic + local render | AC03 keyboard/gamepad + visible-focus evidence; C&R/scoring result variants; no human journey recorded |
| F18 weather/time | active | synthetic + retail-data | AC05 audio captures + full preset capture set (one cloudy-noon/rainy-night sf pair recorded; 32-slot retail matrix test landed in F18-C); `.ldef`/`.lmap` consumers; network replication; UNK-24/39/40 |
| F19 pedestrians | active (A only) | retail-data (parsers, skin/deform *domain types*) | **no runtime**: spawn, skinned render, routes, reaction, audio, reset (reqs 3–6; AC02–AC06); UNK-15/41 |
| F20 police | active | synthetic + retail-data | pursuit rules unknown (UNK-9) — behaviour is *designed*; trigger/loss/outcome matrix and session restrictions |
| F21 Crash Course | active | structural catalog only | **evaluators unrecovered** (UNK-14/35): lesson-by-lesson pass/fail, instruction/voice flow, reward credit; 42 lessons not playable as a course |
| F22 HUD/map/cameras | active | synthetic + local render | UNK-26…37 presentation details are inferred; AC05 visual inspection on atypical vehicles |
| F23 controls/options | active | synthetic | audio/graphics option *effect* (no output device test), accessibility, hot-plug, wheel/force-feedback unverified |
| F24 multiplayer session | active | synthetic + real loopback processes | LAN/Internet matrix, dead-but-open link, NAT, lobby UI polish; reports must stay loopback-labelled |
| F25 networked driving | active | loopback, impairment matrix measured | sub-epoch drift, full input-replay prediction, LAN/Internet scope, rejoin |
| F26 shared world/races | active | synthetic + loopback | late-joiner bulk ledger, spectator/race-ends-during-late-join, two-process breakdown leg |
| F27 Cops & Robbers | active | synthetic + loopback | what knocks gold loose, pickup radius, respawn, disconnect/out-of-bounds (UNK-10) — stock variants each need status (AC06) |
| F28 city specials | active | retail-data | `mm2-inspect specials` report landed; cable car runs partially (4 cars, 2 circuits in SF, signal-gated; no audio, no player/ambient obstacles, init gate unknown — UNK-44); late-join phase (F26) |
| F29 modding | active | synthetic | per-consumer override tests exist for car texture, handling, prop, audio cue (`mod_override.rs`) and race rules — Checkpoint, Blitz and Circuit tables, waypoints, grids, and the `.aimap`/`.opp` roster with a malformed aimap refused (`mod_override_race.rs`); a Crash Course lesson's malformed aimap is refused too (`lesson_launch.rs`); a mod's lesson sub-event table, lead-car aimap and `.opp` route reach `lesson_race_setup`, and an unusable lead-car route is refused (`mod_override_race.rs`, `lesson_launch.rs`); a mod's `tune/<city>.cinfo` renames menu races and classifies cosmetic (`menu.rs`); open: menu art, `mmlang.dll` strings, a real-install run |
| F30 performance/packaging | active | local measurement | release-profile baseline for named scenes; memory/draw-call/voice/bandwidth columns; packaging step, real OS layouts |
| F31 final audit | A.1 this doc; B.1 synthetic single-player journey; C queued | synthetic | B.1: `menu::a_named_driver_races_earns_a_reward_and_finds_it_after_a_relaunch` (name a driver → locked paint/race → launch → authored finish → results → restart → menu → relaunch). Open: retail-data journey, lesson/C&R/multiplayer journeys, negative-path journeys |

## 4. Cross-cutting gaps (not owned by one feature)

1. **Audio is unlistened.** The mixer, decoders and cue logic have code and
   synthetic evidence; the plan records no offline mix or captured listen
   (F07-AC05, F08-AC05, F18-AC05, F23-AC05).
2. **No recorded human playthrough** for any journey. Every race/lesson
   result is a headless or scripted run (F31-B.1 adds one synthetic
   menu-driven journey, `tests/menu.rs`, with no human and no retail data);
   F31-AC03 needs complete
   profile→select→load→race→reward→menu journeys on the production path.
3. **Original-fidelity comparisons are absent.** Placing, pacing, AI
   difficulty, traction-in-rain, damage thresholds and police behaviour are
   engine self-metrics or `designed` readings; the UNK list in
   `docs/original-rules.md` is the authoritative open set.
4. **Crash Course and pedestrians are the two scope holes** where parsed
   data exists but gameplay does not (F21 evaluators, F19 runtime). Neither
   is covered by a stand-in; both stay in the denominator.
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
2. F19 runtime spawn/skin render — the other hole; unblocks F20/F28 reaction.
3. An audible offline mix harness (closes four audio ACs at once).
4. F29 consumer coverage for the remaining record families (surface tables, menu art, localization).
5. A scripted full-journey harness (F31-B) over the real menu → session →
   results → reward path, run on retail data.
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
