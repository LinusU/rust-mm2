# Ralph implementation plan

## Run context

- Starting commit: `a22e96ce1e2e8d94685752baf5275ec675000a19` on branch
  `ralph/night` (worktree `/Users/linus/coding/rust-mm2-night`). Tree was
  clean at iteration start; reviewed-snapshot baseline `822511c` is ~25
  commits behind HEAD — vehicle import, handling measurement and several
  driving fixes landed after it.
- Original installation: `/Users/linus/coding/rust-mm2/retail` — retail
  layout with `mm2core.ar`, `mm2tex.ar`, `mm2aud.ar`, `mm2audex.ar` plus
  loose files. Read-only; not in git. `mm2-inspect list` resolves 13,389
  logical paths through the VFS (counted 2026-09-20). `MM2_GAME_DIR` is
  not set; the path is passed explicitly as `<dir>`/`--mm2-path`.
- Toolchain: rustc 1.97.1 stable (`rust-toolchain.toml`: stable +
  rustfmt/clippy), macOS arm64 (Apple Silicon), Cargo workspace locked
  (`Cargo.lock` committed). bevy 0.19 (default-features off, `tga` only —
  no `bevy_audio`), avian3d 0.7.
- Capabilities: build/clippy/test all warm in `target/`. GPU/wgpu not yet
  exercised this run (no `--frames --screenshot` capture attempted).
  Audio output unexercised (no audio code exists). Network loopback
  available; no networking code exists.
- Deliberate deviations / unresolved original rules: handling departures
  are catalogued in `docs/vehicle-handling.md`; format inferences in
  `docs/research/`. Everything else (race rules, breakables, surfaces,
  traffic, pedestrians, police, Cops & Robbers, Crash Course rules) is
  unverified pending the F00-B rules ledger.
- External runner last checked checkpoint: see
  `/Users/linus/coding/ralph-run-state` (not this file).

## Status meanings

`queued`, `active`, `implemented` (candidate), `checked` (external gates + separate review passed), `verified_original` (required stock evidence exists), `blocked`.

`checked` is not synonymous with complete recreation. A clean build does not prove graphics, audio, original content, Internet connectivity or playability. Record evidence levels per feature. Only promote a status by referencing actual evidence and the external check/review report.

## Selection policy

Choose the highest-value ready small slice; repair current regressions before unrelated work. Search existing code first. Split tasks that do not fit one focused change, preserving all parent acceptance requirements. A blocked content-specific slice does not stop independent work. Do not silently omit blocked items.

**Next selected slice: F02-C remainder (per-car render leg —
needs GPU captures; per-car override matrix), F05-B remainder
(texel damage implemented in B.9;
damage-driven detachment if original — UNK-13; C&R healing driver —
needs F27's mode; replication — F25+),
F17-B remainder (needs F17-C's mode), F17-A remainder
(per-event weather controls need F18; AC03 evidence leg),
F15-B remainder (representative avoidance matrix;
`unkFlag`/`cornerBrakingThreshold`/`weirdPathfinding` consumption —
semantics unverified, research-gated),
F11-C remainder, F16-C's AC01
process-level leg, F10-B's remaining collision/queue-priority
scope, F13-C, or the F18-A remainder)** — the latest iteration
landed F15-B.9, the AC06 measured-difficulty leg: `diff=` joins the
smoke record as a run-config field naming the session difficulty,
and two production-path tests prove the switch — the authored-variant
test fields each difficulty's own lineup (vehicles, slot order,
tails, `-a-`/`-p-` routes) and the on-track test measures the same
car on the same lane at authored-0.55 vs authored-1.00 throttle
through the real spawn/drive path with `catch_up == 0` on both
sides. Retail A/B (`fnv1a64:e91e6cd4b2ae30d9`): `sf circuit:0 --bot
--headless --frames 3000` amateur → `lap=1/3 opp=0/4 cu=3
opps=…vpbug/3c,2c,5c,5c` vs `--pro` → `lap=1/4 pos=1/5 opp=0/4 cu=4
opps=…vpbug/1c,vpcaddie/2c,2c,2c` — authored NumLaps, field
composition and per-slot progress all measurably differ; `sf
blitz:0 --bot --frames 2000` amateur → `tl=0.0s` timeout at
ticks=3600 with `env=lt00 traf=0/0` vs pro → timeout at ticks=3000
with `env=lt01 traf=6/6 crx=11` — authored TimeLimit (30 s vs 25 s),
weather selector and ambient density all measure through the session.
Before that the latest iteration
landed F15-B.8, the documented `[Opponent]` tail order +
`avoidOpponents`/`look-ahead` consumption (external review pass,
no blocking findings). Before that the latest iteration
landed F15-B.7, per-opponent progress/recovery disclosure (F15
spec req 6 — the reviewer's named gap: the "all four opponents
clear gates" evidence came from instrumentation removed before
commit). The smoke record gains `opps=` — one row per spawned
opponent sorted by authored roster slot:
`<slot>:<vehicle>/<cleared>c[/<lap>l][/F|/T][/<e>e][/<r>r][/<w>w]`
— and `p_rec=<reanchors>r/<escapes>e` for the scripted player,
both activity-gated so clean records stay bit-identical.
`OpponentDriver` gains `index` (authored slot, surviving skipped
vehicle loads) + `stuck_peak` (longest continuous displacement-
bubble window — a cumulative total was rejected: a moving car's
bubble re-seats every `REANCHOR_DIST`, so the total ≈ race
duration; the peak survives window resets and re-anchors) and
`ScriptedBot` gains `escapes` (counted reverse-and-turn escapes,
preserved across re-anchor resets). sf 9000f now names every
opponent's earned gates (6/6/lap-2/6) with its recovery counts;
london 1500f is identical to the pre-slice baseline — the slice
is observability-only. Before that the latest iteration
landed F15-B.6, navigation-backed `.opp` densification: the
`sf circuit:0` p3→p4 "hillside wall" turned out to be route
fidelity, not physics — the sparse authored leg straight-lines
across Telegraph Hill bungalow rooftops while the drivable
course is the L-shaped street (BAI road 340) around the block.
`NavGraph` gains `lane_hits`/`route_candidates` (multi-entry
A* so both directions of a carriageway compete),
`route_path` and `densify_route`; `OpponentDriver` chases the
derived line while `spec` stays authored verbatim, and the
`--bot` `ScriptedRoute` gets the same treatment. The sf run
goes `cp=2/9` + 21 recoveries → `cp=4/9 lap=2/3 pos=1/5`.
Before that the latest iteration
repaired the F15-B.5 candidate's external-review findings —
doc drift, no implementation change: the committed record
claimed `Tests +5` for what was a 7→18 `tests/bot.rs`
growth (now `+11`), and described `ScriptedRoute` as bound
'when `--bot` runs' where `load_session_world` actually
binds it in every event session whose roster resolved a
route, dormant without `--bot` as its code comment states.
Before that the latest iteration
landed F15-B.5, scripted-player route guidance: the `--bot`
evidence driver borrows the event's nearest-staged resolved
`.opp` line (`ScriptedRoute`, bound at session load) and chases
a 40 m polyline lookahead bounded at the objective's nearest
route anchor, replacing the gate-to-gate straight line that cut
off `sf circuit:0`'s elevated road between gates 1→2. A bounded
re-anchor (displacement, off-route-distance, or ≥3 recoveries
without a banked gate) teleports the car back onto the route
through `ResetVehicle`/`Teleported`, with a 10 m ground probe
rejecting landing poses that have no collider beneath — the
retail descent line interpolates over a rooftop gap that used
to trap recovery in a void-fall loop (`rcv=206` → `rcv=21`,
every landing supported). Honest remainder: the p3→p4 hillside
descent itself is not drivable — the car slides off the line's
crown into a parallel ditch and grinds at ~1–2 m/s, and the AI
field shares the wall (`opp=0/4 opp_rec=5`), so the next blocker
is steep-slope drivability/collider fidelity, not routing.
Before that the latest iteration
repaired the smoke's below-world verdict (F00-C.2): it compared
the end-of-run y to `spawn - 25`, so retail `sf circuit:0`'s
legitimate 28 m descent read `fell through the world` and masked
the run's real evidence. A session-scoped `WorldFloor` now
carries `Psdl::bounds_min.y` from `load_city` to the check
(floor − 25 when bound, spawn-relative on dev worlds). The
re-run reports `status=pass` and exposes the actual remainder:
the scripted player leaves the elevated road between gates 1→2
and loops fall/recoveries at `cp=2/9` — a route-following limit
under F14-B/F15-B, not a world-hole. Before that the latest
iteration
implemented F18-A.7 and closed the remaining F18-A research
questions against the retail exe: `cityLevel::Load` opens only
`city/<stem>.cpvs` (no numbered-name construction), so the
`.cpvs` extras are offline bake sweeps — `sf_8`/`london_8`/
`london_bad` are byte-identical to base and the `_N` files nest
`_0 ⊋ _2 ⊋ _4 ⊋ _8=base ⊋ _16…_255` (the shipped table is the
N=8 bake); `.pvshist` has no exe reference (bake-tool data);
`.lmap` IS runtime-loaded (`wrong lightmap version`/`room count
mismatch` diagnostics — value semantics still open, UNK-24). The
exe's second deadly-water source — `"Water of Death(tm) [from
SDL]"` — marks every room whose first attribute is a `TextureRef`
to a drowning-class surface (42 sf / 20 london on retail, all
`deepwater`-mapped); `CityWater` now collects both mark sources
through `SurfaceTables::is_deadly_surface` (the `water_min_drag`
boundary — a documented approximation of the unrecovered class
flag) and the smoke record splits them (`wtr=-1.9/3r+42sdl` on
sf). `mm2-inspect weather` now classifies each `.cpvs` variant
against the base table and reports the SDL count. Before that
the latest iteration
repaired the F18-A.6 external-review findings: the recorded test
counts were wrong (+8 claimed, +12 actual — the `water.rs` unit
tests were uncounted and `tests/recovery.rs` is ×4, with the
claimed "resource-absent" scenario covered only implicitly), and
the documented "all-unresolvable refs → no resource" policy was
not enforced — `load_city` now drops a zero-room record like a
missing one, with a regression test. Before that the latest
iteration implemented F18-A.6, consuming the authored
`city/<stem>.water`
deadly-water record, and repaired both F18-A.5 review findings.
The `.water` refs are now verified 1-based PSDL room ids of the
flat attribute-less perimeter rooms at the water plane (London
345/351/356 → Thames reaches at −4.0 under level −3.8; SF
228/399/401 → ocean reaches at −2.0 under −1.9); a global
below-level kill is refuted by London's authored BAI lanes down
to −22. `mm2_app::water::CityWater` (session-scoped like
`CityPvs`) resolves each ref to its room's XZ perimeter plus a
bound of `max(level, room-top)` and `track_recovery` overlays it
on the wheel-`drag` classification — an exposed contact is water
whatever the collider's material says, and a car under a listed
room's bound with no contact accrues the submersion dwell rather
than free-falling (designed policy, DSN-34; the original
consumer stays unrecovered, UNK-24). Retail evidence: `wtr=-3.8/3r`
(london) / `wtr=-1.9/3r` (sf) in headless records, and a staged
`--spawn` inside a deadly room below the bound recovers through
`rcv=3w/0f/3r` with `never grounded` on both cities — the overlay
fires where no wheel contact exists. The two F18-A.5 repairs:
`--no-pvs` now rides `SessionConfig::dev.no_pvs` into the
headless session (the old `PvsEnabled` resource was inserted
only on the windowed path — `--headless --no-pvs` now reports
`pvs=off`), and `apply_city_pvs`'s camera query is
`Camera3d`-filtered so a transition-frame `Camera2d` can't add a
wrong source room. Before that the latest iteration
implemented F18-A.5, authored room-PVS render culling: the
verified `city/<stem>.cpvs` table (WLD-23) now binds through
`mm2_app::pvs` — every per-room render mesh is `CityRoom`-tagged
at spawn and `apply_city_pvs` re-resolves the source set each
frame as the union of rooms whose authored XZ perimeter contains
the active camera or the player vehicle (retail
`PointInPerimeter` is the same 2-D test; `FindRoomId`'s hint is
a search shortcut), unions the decoded lists (over-show never
under-show), and rewrites `Visibility` only on change.
`--no-pvs` mirrors retail `EnablePVS(false)`; a
missing/unparseable table yields no resource rather than a
fabricated state. Retail evidence: headless `pvs=982r/6378h/7324`
on sf and `pvs=68r/7422h/8254` on london; frozen-`--cam` A/B
captures are capture-noise-identical at gameplay height while a
75 m aerial shows authored-table omissions in distant skyline
rooms — the table's own conservatism, retail renders the same.
Before that the latest iteration
implemented F05-B.9, impact texel damage — the
`fxTexelDamage::ApplyDamage`/`Reset` leg of `vehCarDamage`
recovered from mm2hook: body tris within the authored
`TextelDamageRadius` of an impact each splat the `_dmg` texture
onto a per-vehicle clone of the clean texture at a random
barycentric UV (splat shape designed, DSN-32 —
`ApplyBirdPoopDamage` unrecovered), and repair re-blits clean.
`mm2_game::texel` owns the mechanics, `mm2_app::texel_fx` the
per-vehicle `TexelDamageRig`/`TexelSlot` clone state, the
`FixedLast` impact feed and the `TexelRepair` reset wired into
`resolve_disabled`'s three repair sites; the headless record
gains `txl=<i>i/<s>s/<r>r` on activity. Retail: sf circuit:0
`--bot` → `txl=401i/64364s/3r` tracking `dmg=…/4d/3r` repairs
exactly; rendered captures show a nose-first landing stamping
the hood/windshield while the same car's rear stays clean. The
prior iteration's doc nit is corrected too — the `_dmg`-carrying
denominator is 27 base `vp*.pkg`, not 25 (vpdb731 and vpvw_dune
join the exception list). Before that the latest iteration
implemented F02-C.4, resolving operator report 2 item 3 — the
long-standing `vpbug` half-damaged render: vehicle body
sections are authored bound to `<stem>_dmg` damage textures,
and retail's `fxTexelDamage::Init` (recovered in mm2hook)
binds the clean stem until damage is applied.
`MaterialCache::shader_material` now applies that pairing for
vehicle models; the defect was fleet-wide (22 of 27 base
`vp*.pkg`) and identical-pose captures show vpbug/vpbus clean.
Before that the latest iteration
implemented F02-C.3, resolving the `vpmoonrover` launch finding:
the retail `vehCarSim` carries exactly four `vehWheel` slots
(mm2hook source — `whl4`/`whl5` are "back-back" visuals drawn as
`whl2`/`whl3` + a stored offset), so `build_model` now flags
`whl` index ≥ 4 as `!simulated` followers (`WheelVisual.follows`)
and `assemble` emits only the four physics wheels; the follower
mounts copy the reference wheel's live droop/steer/spin plus the
authored offset. The residual weirdness is authored — the Moon
Rover's `CenterOfGravity` sits behind the physics rear axle, so
it rests nose-up on its tail (a `--clearance` failure that is
the retail rig's own equilibrium) and launches weakly; community
docs report the original misbehaves too. `vpcentury` takes the
same rule for its tandem axle and passes every leg. Still open
on F02-C: per-car render captures, per-car override matrix.
Before that the latest iteration
implemented F02-C.2: `drive_probe` gained the missing acceptance
legs — per-car `--drop` landing, roster-wide `--city … --clearance`
spawn/reset checks on real road colliders (trailers included via
the production `spawn_trailer` path, hitch gap measured), and
`--config`/`--dump-config` override handling through the same
`apply_handling_override` as `--vehicle-config`. The clearance leg
caught and fixed two real defects: hitched tractor/trailer hulls
registered a standing contact at the coincident anchors (the hitch
joint now carries `JointCollisionDisabled`), and `vpcentury`'s
trailer counted two 5 cm `TWHL` detail parts as physics wheels
(`WheelVisual.simulated` excludes sub-half-radius trailer parts;
visuals stay parked at the authored origin). Retail evidence:
`--drop` 21/21 land upright and drive away; `--clearance` 21/21
clean on sf and london (trailers 4/4, hitch gap 0.00); override
causality measured on vpbug (power, grip, mass). Before that the latest iteration
implemented F18-A.4: the `.sky` dome now binds through the
production session path — `city/<stem>.sky` resolves its authored
`geometry/<model>.pkg` dome, the session's `tod*4 + weather` slot
selects the dome's paint job (the measured 16-slot texture grid),
and the dome spawns session-owned, unlit/fog-exempt/double-sided,
scaled from its authored ~43 m extent to a designed 900 m radius,
re-centred on the active camera and rotated by the authored rate.
Retail evidence: `env=… sky=sky_dome_l:skylondon_ca_l` (london) /
`sky=sky_dome:sky_ca_f` (sf) in headless smoke records plus
pitched-up frozen-cam captures showing the authored dome textures
(Metal/Apple M1). The transform fields' readings stay designed
(UNK-24); `.cpvs` variants, `.lmap`/`.ldef`, precipitation/
wetness/audio and replication stay open. Before that the latest iteration
repaired operator report 4 item 5 as F10-B.10, the startup WARN
spam: the two bulk authored-anomaly classes (`NoVehicleLanes` per
road — 154 london / 22 sf; `walk_prop_rules` per-entry strings —
64 sf / 0 london) now emit one DEBUG summary line each instead of a
WARN per entry, with counts unchanged in `issues=`/
`proprule_issues` and per-entry detail still in `mm2-inspect
nav`/`placement`. Retail london startup: 155 WARN lines → 1 (the
genuine TEX mip warning). Before that the latest iteration
repaired the F04-C.5 candidate's external-review blocker: doc
drift. Four cross-references still claimed the knock/damage
linear `v·m` estimate is the same quantity banger activation
gates on — false since the energy split. `KnockPolicy` and
`DamageState::apply` rustdoc, `docs/research/damage.md`'s
shared-contract paragraph and the UNK-12 F10-B.6 note now
state the split (linear for knock/damage, `½·m·v²` for banger
activation; only deepest-contact severity and striker-mass
resolution shared). No implementation change. Before that the
latest iteration
repaired operator report 4 item 3 as F04-C.5: trees and large
poles were never unbound (they stamp through prop rules and
`props.pathset` as dormant bangers); the activation gate compared
`speed × striker_mass` to `ImpulseLimit2`, whose authored census
(`≈ Mass × {31.25…85342}`) is quadratic in striker speed, so every
large breakable was priced beyond reachable velocity. The gate now
feeds striker kinetic energy (`½·m·v²`) to the same authored
threshold — `sp_lightstreet_rt_f` (a 10 t pole with authored
BREAK parts) shattered on a retail SF cruise at 37.5 m/s where the
linear reading demanded ~75 m/s. UNK-22 stays open: the original's
comparison quantity is unrecovered and the energy reading is a
designed stand-in justified by the authored ladder. Before that
the latest iteration
repaired operator report 4 item 2 as F04-C.4: dormant props
answered their first contact as infinite-mass statics (the
solver ran before activation), so the striker took the full wall
impulse — the extreme bounce and total speed loss the operator
measured on parking-meter-class props. Qualifying activations
now replay the hit as a reduced-mass two-body transfer — the
solver's applied impulse is returned, `(1+e)·v·μ` paid instead
along the applied normal, and the prop launches at
`J/prop_mass` — driven entirely by authored `Mass`/`Elasticity`/
`ImpulseLimit2`, no feel constants. `deepest_contact` normalizes
normal/anchors to caller argument order, fixing a latent
stored-pair-order flip shared by every consumer. Retail
before/after on identical commands: london vpbug
`moved=55m→124m`, sf vpddbus `moved=46m→99m`. Candidate pending
external check. Before that the
latest iteration
implemented F18-A.3: the authored per-preset fog tables
(`city/<stem>_fog.csv`, discovered outside every audit denominator)
now parse in `mm2_formats::fog`, join the `mm2-inspect weather`
census (105 files; canonical tables cross-checked 16/16 against the
`.ltNN` slot names), and bind through `spawn_environment` as
`DistanceFog` on both session cameras — same effective slot as the
lighting preset, so authored-event and customization precedence
apply identically; missing/degenerate fog is an explicit
`fog.absent` diagnostic, never a silent default. Retail evidence:
`fog=650-1000` (sf) / `fog=220-320` (london) in smoke records plus a
frozen-cam foggy-vs-clear capture pair. `.sky` dome, `.cpvs`
variants, precipitation/wetness/audio and replication stay open
under F18-A/UNK-24. Before that the
latest iteration implemented F17-A.6: the menu's condition-options screen —
weather/time-of-day/traffic-density picks under a per-city
cruise `options` row (RACE-4) and a per-event `options` row
gated on RACE-3's beaten flag, seeded from the authored
`RaceParams` and launched through `SessionConfig::customization`
so picks beat authored event conditions and head the
ambient-density chain; unchanged visits stay default runs and
changed picks are record-ineligible (DRV-6 `Ineligible::Customized`).
Pedestrian/cop density and Circuit laps/opponents stay deferred.
Before that the
latest iteration implemented F18-A.2: the measured `.ltNN` presets now bind to
Bevy session lighting through the production path (authored
event conditions win via `mm2_game::effective_conditions`), with
session-legal `--weather`/`--time-of-day` selection, an
`env=ltNN(name|fallback)` smoke diagnostic and retail rendered
evidence; `.sky` dome/fog/PVS/precipitation stay open under
F18-A/UNK-24. Before that the
latest iteration repaired the F18-A.1 candidate's external-review
finding: stale recorded census/test counts ("22 per-city"/"38
extras"/"+9 tests" → measured 21/28/+21) in `environment.md`,
`LAST_ITERATION.md` and the F18-A.1 row; implementation untouched,
`751485d`'s commit-message wording disclosed rather than
rewritten. Before that the
latest iteration shipped the vehicle-coverage matrix:
`drive_probe` gained a `--controls` mode — launch to 15 m/s,
full brake to a stop, held-brake reverse, `ResetVehicle`,
finiteness, each leg through the production `vehicle_bundle` +
`VehiclePlugin` systems — and the full matrix ran against the
fingerprinted retail install: 21/21 expected stock validated
(`--strict` exit 2 on two disclosed warnings), 21/21 inside the
handling envelope, 21/21 `status=pass` + `wheels=N/N` through
the app path on sf, and 20/21 control checks clean with
`vpmoonrover`'s weak launch recorded as a `drive` finding
(resolved in F02-C.3 as authored behaviour on the retail
four-wheel rig — it still fails the launch bar, now for
documented reasons). The matrix's first run caught a real defect —
reverse was unbounded (vpbug measured -45.6 m/s): the drivetrain
torqued through `reverse_ratio` while the upshift selector and
RPM tracking ran the forward gearbox, so a held brake backed the
car toward its forward top speed. Reverse is now the single band
the authored data describes — no gear selection while reversing,
wheel-implied RPM through `reverse_ratio`, and a band-top
limiter at `upshift_rpm` (the `OptRPM` convention `Trans.Reverse`
is authored under) — bounding every car at its own authored
reverse top (vpbug -13.6 vs implied ~13.4, vppanozgt -22.5 vs
~22.6) with `reverse_speed_stays_bounded_at_the_reverse_gear` as
the regression net. `docs/vehicle-coverage.md` publishes the
matrix and its open legs; `docs/vehicle-handling.md` gains the
reverse paragraph. Before that the
latest iteration repaired the external review's blocking finding on
the F00-C.1 candidate: its absent-player pass branch treated every
non-live phase as a teardown window, so a mid-run restart whose
reload failed (`load_session_world` → `Failed` on the second
`begin`, which parks until an intent nothing issues) reported
`status=pass … phase=failed moved=none final=none`. Absence now
passes only inside the transient `Unloading → Menu → Loading`
window and only while a teardown/re-begin is in flight
(`absent_player_is_transient`: `Menu` requires a queued
quit/restart intent, `Loading` requires the generation bump);
a `Failed` cap fails with `session failed: <reason>` and a parked
`Menu` with `no player vehicle`. Before that the
latest iteration repaired a review-flagged evidence defect
(F00-C.1): `headless_smoke` cached the `PlayerVehicle` entity, so
a mid-run session restart (disabled → `RestartEvent`, the RACE-5/
DMG-2 rule the london `checkpoint:0 --bot` run exercised) left it
reading a despawned entity and reporting a false
`status=fail "non-finite pose"`. The runner now follows the live
player entity per frame, reports `final=none` only inside the
teardown window, and counts restarts as `rs=<n>` (generation
delta, on activity only); a quarantined `--restart` dev override
(`DevOverrides::restart` → `session::dev_restart_once`,
`record_eligibility`-gated) gives evidence runs a reproducible
restart trigger through the production lifecycle. The flagged run
now reads `status=pass rs=1 … race=Countdown{36}` — the
"instability" was the record, not the physics. Before that the
latest iteration landed F15-B.4, the designed catch-up assist
(DSN-27 — the original's catch-up semantics, if any, are
unverified, UNK-11): `mm2_game::race` gains `CatchUpPolicy`
(`deficit_full` 2.0 gates / `assist_max` 0.25), `course_progress`
(a participant's continuous course position in gate units —
banked gates plus the covered fraction of the leg toward the
`live_order` tie-break objective, scaled by the course's own
mean gate spacing) and `catch_up_factor` (one-directional,
linear, garbage-safe). `opponent_drive` measures every
progress-carrying participant — the human included — and lifts
a *trailing* AI driver's demand ceiling (`throttle_cap + assist`,
`corner_speed × (1 + assist)`), never slowing anyone, never
granting progress, never touching the player's input. The
factor is observable per driver (`OpponentDriver::catch_up`)
and counted in the smoke record as `cu=` on activity only —
distinct from `opp_rec=` re-anchors. Retail evidence:
`london circuit:0 --bot` mid-race `cu=7` (all trailers, leader
untouched), `sf checkpoint:0 --bot` mid-race `cu=5`, both
600-frame cruises bit-identical to the F05-B.8 review records.
Before that the
latest iteration landed F05-B.8, authored-impact sparks — the
`asLineSparks` renderer mm2hook recovers on `vehCarDamage` fired
by `RadialBlast(count, position, velocity)` from the car's
`ImpactCB` (emission shape unrecovered → designed DSN-26; UNK-13
stands). `mm2_game::effects` gains `SparkPolicy`/`VehicleSparks`/
`Spark` — a per-vehicle rig (same authored `vehcardamage` gate as
`VehicleSmoke`) bursting at each deduplicated `ImpactEvent`'s
authored contact point: `min_burst` 2 + 0.8/m/s, ceiling 16,
`max_live` 128, velocities in a rebound cone around the
participant's own side of the contact normal at ~7 m/s, 0.5 s
lives under gravity, per-vehicle `NavRng` determinism.
`mm2_app::spark_fx` emits session-stamped velocity-aligned
crossed-quad streaks in an unlit additive material
(`texture/spark.tga` through the VFS — the install's only
spark-named texture, 8×8; a missing texture falls back to an
untextured additive material rather than claiming it), advances/
fades/expires them, and counts
`spk=<b>b/<e>e/<x>x` on activity only — remote participants skip
and a non-`Playing` frame drains the feed. Before that the
latest iteration landed F05-B.7, damage impairment — the
smoke↔engine-torque coupling MM2Hook's `PhysicalEngineDamage` ini
option documents ("when the engine spews smoke … less acceleration
and less top speed"), implemented as a designed policy (DSN-25;
the original's shape is unrecovered, UNK-13):
`mm2_game::damage::ImpairmentPolicy::factor` keys on the same
authored `MedDamage` bound the DSN-24 smoke gate emits on — 1.0
below it, `power_at_med` 0.8 on reaching it, a linear ramp to
`power_at_max` 0.4 at `MaxDamage`, garbage-safe. The physics-side
`mm2_vehicle::EngineImpairment` component scales only the
drivetrain's `wheel_drive_available` (brakes, engine braking,
steering and tires are not engine output); `mm2_app::damage::
sync_impairment` mirrors each local/AI participant's authored
total into it — present exactly while `factor < 1`, cleared the
same tick a repair lands, remote participants skipped. `imp=`
joins the smoke record on activity only. Before that the
latest iteration landed F05-B.6, the authored engine-smoke visual
tier: `mm2_game::effects` distils the `vehCarDamage` embedded
particle spec (mm2hook's recovered `EngineSmokeRule`) into a
`ParticleSpec`, resolves the authored pivots
(`SmokeOffset`/`SmokeOffset2`/`DoublePivot`/`MirrorPivot` gate —
alternation via the recovered `m_CurrentPivot` reading) and owns a
designed emission policy (MedDamage→MaxDamage ramp, bounded pool,
per-vehicle `NavRng` determinism, DSN-24 — the original `Update()`
is a thunk, UNK-13 stands). `mm2_app::damage_fx` builds sprite
assets through the VFS (`fxpt2`, a measured 2×2 puff atlas;
`TexFrame` indexes its tiles), emits unlit blended camera-facing
quads per puff and integrates the authored fields (rise, drag,
growth, byte-space alpha fade) until `Life` expiry; remote
participants and paused sessions emit nothing, puffs stamp
`SessionEntity`, and `ptx=` joins the smoke record on activity.
Before that the
latest iteration landed F05-B.5, water/out-of-bounds recovery:
`VehicleRecovery` (a designed detector — no authored record exists,
DSN-23/UNK-13) rides on every local/AI participant, anchored at
each dry-grounded wheel pose; every grounded wheel on a
drag-classed water surface (deepwater only — shallow `water` stays
wadable) for `submerge_dwell` fires a recovery back to the last
dry anchor, an airborne fall `fall_margin` below the anchor fires
an out-of-bounds recovery (latched once per fall; non-finite pose
is defence-in-depth), and `resolve_recovery` answers through the
shared `ResetVehicle`/`Teleported` path with trailer re-seating,
stuck disarm and no repair. `rcv=` joins the smoke record on
activity only. Before that the
latest iteration landed F05-B.4, authored `vehgyro` consumption:
`VehicleConfig.gyro` carries the decoded record verbatim
(`Option<GyroConfig>`, `None` on authored absence, malformed rates
warn + clamp), and `vehicle_simulation` consumes it as a designed
reading of the recovered `Spinable`/`Driftable`/`Rightable` gates
(DSN-22 — the original `Update()` is unrecovered, UNK-13 stands):
handbrake + steering while travelling latches a spin
(`VehicleState::gyro_spin` — `Spin180` forward, `Reverse180` as the
J-turn), the gyro writes the authored yaw rate as the maneuver's
authority rather than torque-fighting the tires, the handbrake hold
doses the rotation (release = partial, ~π = `gyro_completed`,
opposite flick re-arms, bounded age), `Drift` relieves the slip term
of the yaw damper (`sim::yaw_damp_factor`; `drift = 0` is the
unmodified policy), and `Pitch`/`Roll` are per-axis airborne
righting rates inert on the all-zero retail roster. `gyr=` joins the
smoke record on activity only — the scripted driver never
handbrakes, so inactive runs stay bit-identical. Before that the
latest iteration landed F05-B.3, authored breakaway detachment:
`mm2_content` resolves `VehicleDef.breaks` from `BREAK<NN>` model
parts with decodable `dgbangerdata` records (unmatched assets stay
bolted on, never fabricated), `mm2_game::breakaway`'s `VehicleBreaks`
spawns on the player and AI opponents, `mm2_app::breakaway`'s
`detach_breaks` consumes the deduplicated `ImpactEvent` stream and
detaches each part when `severity × part_mass` exceeds its authored
`ImpulseLimit2` — the `limit / mass` reading the authored data is
shaped for (≈31.25 m/s panels, ≈2500 m/s never-detach anchors,
measured on every retail record). The intact node hides, a fragment
body spawns pooled through `BangerPool`, one bounded `PartDetached`
fires per attachment, and `resolve_disabled`'s repair calls
`restore_rig` — re-attach, despawn fragments, show nodes
(F05-AC03). `brk=` feeds the smoke record on activity. Retail
sf/london cruises stay bit-identical (vpbug authors no parts;
vpcoop's scripted impacts never reach ~70 mph). Iteration 31 then
repaired the candidate's two external-review blockers — both
evidence defects, no implementation fault: the DSN-21/UNK-13
ledger rows now record the implemented `severity × part_mass`
(the part's own record `Mass`; the `other_mass` wording was the
rejected draft), and a `mesh_children_respawn_under_the_fragment`
test now exercises the fragment-visual re-spawn the record
already claimed. Before that the
latest iteration landed F05-B.2, authored `vehstuck` consumption:
`VehicleStuck` (authored spec + impact-armed detector) now spawns
on the player and AI opponents, `mm2_app::stuck`'s `track_stuck`
anchors episodes off the deduplicated `ImpactEvent` stream and
fires a bounded `StuckEvent` after the authored `TimeThresh` inside
`PosThresh`, and `resolve_stuck` answers it with an in-place
upright `ResetVehicle` (`Teleported`, trailers re-seated).
Retail sf/london cruises record `vsk=3a/0d/0r`. Before that the
iteration landed F05-B.1, runtime impact→damage
application plus the session's disabled outcomes: `VehicleDamage`
(authored spec + watermark-deduped `DamageState`) now spawns on the
player and AI opponents, `mm2_app::damage`'s `apply_impact_damage`
accumulates the deduplicated `ImpactEvent` stream as
`severity × other_mass` impulse and emits bounded `DamageEvent`s,
and `resolve_disabled` enforces the documented RACE-5/DMG-2
consequences — Cruise resets to spawn, Circuit resets in place with
a designed 5 s clock penalty, Blitz/Checkpoint/CrashCourse queue
the session's own restart, AI opponents reset in place (designed).
Retail sf/london cruises record `dmg=2a/0d/0r rej=1 dup=0`. Before that the
iterations landed F05-A.1, the authored damage/recovery data boundary
plus its review-mandated census repair — externally checked at
`ae64197` (typed `vehCarDamage`/`vehStuck`/`vehGyro` decoders,
`DamageSpec`/`DamageState`/`disabled_outcome` contracts, `VehicleDef`
provenance options, `DamageAudit` + `mm2-inspect damage`, corrected
61-record retail census in DMG-5..8). Before that the
latest iteration landed F17-A.5, the Race Records screen
(DRV-5's first leg) plus the F17-AC05 capability audit:
`Screen::Records` lists the bound driver's persisted
`EventRecord`s — finishes, best time, best place, beaten
marks — under city and race-type filter rows cycled by
Left/Right/Enter; a record row whose event still resolves
and clears the gates re-launches it through `Session::begin`,
while gated/incomplete/catalog-absent records stay listed
with their reasons. Root gains `Race Records` (disabled
unbound) and `Driver's Stats` (tracked-missing) rows, and
`docs/research/menu.md` maps every documented original menu
capability to implemented/tracked/open. A reviewer-flagged
quirk is fixed too: a right-click over a row no longer
queues the hover `FocusAt` after `Back`, so the parent's
restored focus survives. Before that the
latest iteration landed F17-A.4, mouse navigation for the
menu: `menu_mouse` hit-tests `MenuRow`-tagged UI entities
and feeds `MenuCommand`s through the same
`apply`/`MenuEffect` path — hover focuses (edge-triggered
so a resting cursor never fights keys/pad), left-click
activates (disabled rows surface their reason), right-click
backs out. Before that the
latest iteration landed F10-B.8, the union of player
interest areas (the plan's "multiplayer union-of-interest
bubbles" line): `plan_ambient`/`draw_spawn` and the runtime
recycler now take the whole `Player` participant set — a
car is collected only past *every* player's
`recycle_distance`, and a spawn must land inside at least
one area's band while never materialising inside any
area's `min_player_distance`. Single-player records are
bit-identical on both retail cities. Before that the
latest iteration landed F10-B.7, authored traffic-signal
indicators (the plan's "signal-prop rendering" line): the
BAI `trafficLightOrigin`/`trafficLightAxis` pairs — parsed
but unconsumed — now surface as session-owned lamps.
`mm2_game::nav` exposes the authored data twice:
`NavArc::exit_light` (the head governing an approach) and
`NavGraph::signals`/`EndSignal` (the full authored set on
resolved ends — the `mm2-inspect bai` census proved
arc-exit-only traversal misses 388 London / 128 SF heads on
one-way upstream ends and arc-less roads). `Junctions::
signal_aspect` maps each head's aspect off the authoritative
controller — `gate`'s rule admission minus the box-yield.
`mm2_app::traffic` spawns one `TrafficSignal` per head
inside a designed 60 m sanity bound (`signals_dropped`
counts the 3 authored SF outliers), renders them as unlit
lamps (no `vasignalunit` geometry ships — textures only, so
the indicator is a designed presentation), and
`drive_signals` keeps aspects current after
`maintain_ambient` in every FixedLast chain. `sig=`/`sigd=`
join the smoke record. Tests: +3 mm2_game +3 mm2_app.
Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`): sf
`--frames 600` → `… jq=5 stuck=0 kn=1 sig=647 sigd=3`,
london → `… jq=4 stuck=0 kn=1 sig=828` — matching the audit
census exactly; a rendered capture shows a green lamp at the
authored kerb anchor on SF (local, not committed). The
aspect mapping and indicator geometry are designed (UNK-12);
authored positions/rules are original data. Before that the
latest iteration landed F10-B.6, the kinematic→dynamic
collision handover (the spec's "transition to dynamic
behaviour without duplicating bodies or injecting extreme
energy"; F10-AC03's collision-fidelity leg):
`mm2_game::traffic` gains `KnockPolicy::min_impulse` (4000
N·s, designed — the original's ambient crash rules are
unverified, UNK-12); `mm2_app::traffic`'s new `knock_ambient`
joins `collect_impacts` and `activate_bangers` as a third
`CollisionStart` consumer and shares the banger pipeline's
`approach_speed × striker_mass` estimate (moved into
`contracts`). A qualifying contact flips the same entity to
`RigidBody::Dynamic` — same hull, pose and lane velocity
plus at most the striker's approach speed along the contact
normal — drops it from `drive_ambient` (`AmbientDrive::
Knocked`), the FCFS queue and `bound_for`, so a wreck inside
the junction box keeps holding the yielded approaches; light
touches stay kinematic. Ambient spawns now carry authored
`Mass`/`CenterOfMass` (bounded fallback) and
`CollisionEventsEnabled`, and `kn=` joins the `traf=` smoke
record. Tests: +3 mm2_app (hard-hit handover — dynamic on
the same entity, frozen cursor, bounded velocity, striker
shoved; light-touch negative; a knocked wreck nominally
bound for the junction still occupies its box and holds the
green approach until despawned). Retail headless (install
`fnv1a64:e91e6cd4b2ae30d9`): sf `--frames 600` → `traf=16/16
sp=23 rec=7 … jq=5 stuck=0 kn=1`, london → `…sp=20 rec=4…
jq=4 stuck=0 kn=1` — the scripted drive knocks one ambient
car per city and all other counters hold their B.5 values.
Before that the
latest iteration landed F10-B.5, the junction-box yield that
closes F10-AC02's right-of-way leg: `JunctionPolicy` gains
`box_margin`/`box_max_rise` (designed, UNK-12) and
`junction_zone`/`inside_junction_zone` derive an occupancy zone
from the authored intersection — authored centre (endpoint
centroid on non-finite), XZ radius to the farthest member end plus
margin, vertical band so an overpass does not occupy.
`Junctions::gate` takes `box_occupied` and keeps an otherwise-
admitted approach — a light's green member or a stop-sign FCFS
head — closed while the zone holds a vehicle not bound for it;
`NeverStop`/unruled ends keep free flow, `AlwaysStop` stays
closed. `drive_ambient` pre-passes a `bound_for` map so a car
waiting at its own stop line never counts as an occupant (the
deadlock guard), while every `Player` participant does — a parked
driver or AI opponent holds the approach. Tests: +3 mm2_game
(zone geometry/fallback/vertical legs, green-member and
stop-sign-head yields, NeverStop/AlwaysStop negatives) and +2
mm2_app (a participant and an ambient car parked in the box each
hold the admitted approach at its line, release on despawn).
Retail sf `--frames 600` record unchanged (`traf=16/16 sp=23
rec=7 … jq=5 stuck=0`); london shows the expected small drift
(`sp=20 rec=4 jq=4` vs `sp=21 rec=5 jq=3`) as one approach now
stands an extra green. Before that the
latest iteration landed F10-B.4, spawn occupied-space rejection
(F10-AC04's spawn leg): `SpawnPolicy` gains a designed
tangent-aligned exclusion box — `spawn_clearance` 5 m longitudinal
(about a car length), `spawn_half_width` 2 m lateral (a neighbouring
lane stays legitimately spawnable), `spawn_max_rise` 3 m (an
overpass does not occupy) — and `spawn_occupied` evaluates it like
`corridor_gap`'s projection. `draw_spawn` takes the occupied set and
returns `SpawnDraw::Occupied` ahead of the class pick;
`plan_ambient` feeds each placed directive back into the set so two
planned cars keep clearance, and `maintain_ambient` builds it from
surviving ambient cars plus every `Player` participant (the local
driver and AI opponents alike — opponents were never covered by the
player bubble), claiming each same-tick placement for later draws
since Commands-deferred spawns are query-invisible. Tests: +4
mm2_game (box legs incl. overpass/adjacent-lane/degenerate, draw
rejection under a fixture-wide box, same-lane plan spacing,
lane-capacity drop accounting) and +2 mm2_app (a parked live car and
a bare `Player` participant each veto every refill under a wide box
— `spawned` frozen — then the default box refills again as control).
Retail sf/london `--frames 600` records unchanged
(`traf=16/16 … stuck=0`); the initial plan now drops saturated draws
honestly (sf spawns 12/16 at load, london 14/16 — the maintainer
refills both). Before that the
latest iteration landed F10-B.3, the bounded stuck recovery the
spec's "obstruction response and stuck recovery" requirement asks
for: `mm2_game::traffic` gained `StuckPolicy`/`StuckWindow` — a
displacement window (designed values, UNK-12) sized past the worst
legitimate wait this controller can impose ((members − 1) ×
(green + clear) ≈ 21 s at four members) — and `drive_ambient`
despawns a car that cannot make `min_displacement` (4 m) of
progress for `window_ticks` (4800 = 40 s at 120 Hz) into the pool
`maintain_ambient` refills from, counted as `traffic.stuck` →
`stuck=` in the `traf=` smoke record. The recovery is removal,
never a teleport through whatever pens the car (the spec's
explicit bar); a multi-cycle signal queue tail can outwait the
window and is sacrificed by design (documented). Tests: +1
mm2_game (reset/expiry/NaN legs) +2 mm2_app (a parked participant
pens a follower — it recycles after the bound without ever driving
through, twice running; a sub-window red wait crosses on green
with `stuck=0`). Retail: sf/london `--frames 600` records gain
`stuck=0` (other counters unchanged); a 3000-frame sf cruise
(6000 ticks > the window) reports `stuck=0` — a moving player
never pens a car, so no organic stuck was observed. Before that
the latest iteration repaired F10-B.2's external-review blocker:
a closed gate's
`junction_speed` ramp converged to `speed = dist/approach_time`, so
`ds = min(speed*dt, dist)` decayed the stop-line gap geometrically
and the f32 cursor stalled ~1.1e-4 m short — `at_line` (`<= 0`) was
never satisfied, stop-sign followers never registered in the FCFS
queue, and `jq=` missed every stalled hold. "At the line" is now a
designed `JunctionPolicy::stop_line_tolerance` (0.1 m) feeding
`gate()` registration, `junction_held`, and the closed-gate `ds`
clamp (the residual closes outright inside the tolerance, never
past the line). The serialisation test now requires finite crossing
ticks, and a new same-lane queued-follower test reproduces the
reported deadlock regime end-to-end. Before that the iteration
landed F10-B.2, authored junction rules: `mm2_game::traffic`'s new
`Junctions` controller gates each lane-end transfer on the BAI
`vehicleRule` code — `TrafficLight` approaches wait for their road's
phase in a deterministic member cycle (green + all-red clearance,
per-junction desync), `StopSign` approaches queue
first-come-first-served through a registered dwell, `AlwaysStop`
never opens, `NeverStop` flows. A closed gate brakes to a
`stop_inset` stop line (`junction_speed` ramp, never accelerates);
a `Turned` step whose landing sits within `enter_clearance` of a
live car/participant reverts to the lane end instead of
materialising inside a junction queue; `depart`/`retain` shed stale
queue entries on transfer, despawn and recycle. `jq=` joins `q=` in
the `traf=` smoke record. Retail census: SF authors 449 light / 138
never / 30 stop approaches across 212 junctions (mixed is the norm —
82 uniformly lit, 0 all-stop), London 369/214/23 across 254 — so
each approach gates on its own end's rule. Retail headless: sf
`jq=5`, london `jq=3` held at the sampled tick. F10-AC02's
controller leg and F10-AC04's junction-transfer leg are exercised;
original signal timing stays designed (UNK-12), no
yielding-to-crossing-traffic, spawn-vs-spawn checks, stuck recovery
or collision fidelity yet. Before that the iteration landed
F10-B.1, ambient obstruction response: `drive_ambient` senses
a forward corridor (`corridor_gap` — XZ heading, lateral half-width,
vertical tolerance, speed-scaled reach) against every `Player`
participant and other ambient cars, and `follow_speed` brakes to a
bounded `follow_gap` hold / `panic_gap` stop behind the nearest
blocker, resuming when it clears (`AmbientTraffic::queued`, `q=` in
the headless record). Same-subsystem review repairs folded in:
`maintain_ambient` is phase-gated like the driver (paused sessions no
longer churn), `draw_spawn` places inside the
`[min_player_distance, recycle_distance]` annulus (`OutOfBand` covers
both bounds — retail sf churn dropped `sp=140 rec=124` →
`sp=26 rec=10`), `AmbientTraffic::issues` warn-log at load, and
`event_race_setup` parses the event aimap once for both the opponent
roster (`opponent_roster_from_aimap`) and the ambient setup. A
measured-delta `LinearVelocity` feedback bug the pause test exposed
(kinematic bodies diverged to ~km/s) is fixed — the velocity is now
the intended `tangent * speed`. Synthetic tests cover hold/release,
two-car queueing and the pause freeze. Before that the
iteration repaired F10-A.1's external-review blocker: `plan_ambient`
could sweep a NaN-length lane into `sample_storage`'s
`s.clamp(0.0, lane.length)` panic (BAI lane vertices are raw f32 bits;
`push_lane` admitted `length = NaN` because `NaN <= EPSILON` is false).
`push_lane` now requires finite authored distances (else recompute),
finite vertices and finite length, dropping offenders under the new
`NavIssue::NonFiniteLane`; `plan_ambient`'s eligible-lane filter
re-checks finiteness (the +inf-length sibling produced NaN spawn
positions past the player-bubble check); `Bai::validate` reports
`BaiIssue::NonFiniteCurveVertex` per curve so `mm2-inspect bai` audits
it. Non-blocking notes folded in: malformed `CG` warns like `MaxAng`
through a shared `opt_vec3`, and the "All 25 retail records" doc is
corrected to the measured 23. Three new tests/regression legs cover a
NaN-vertex lane with dropped distances, an inf vertex under valid
authored distances, a recomputed non-finite distance row, the bai
validate census and the CG warning. Before that the iteration landed
F10-A.1, the ambient-traffic catalog + seeded spawn-policy planner
(`AiVehicleData` decoder, verbatim cumulative-weight `AmbientRoster`,
`plan_ambient` over `NavOverrides`-open routable lanes, VFS producer +
`TrafficAudit` + `mm2-inspect traffic`; retail: london 23/12 rostered,
sf 23/11, 0 failures, 101+104 event overrides). Before that the
iteration landed F17-A.3, driver-name text entry: Profiles → `New
driver` opens a `Screen::NewProfile { name }` field instead of
auto-naming `Driver N`; `menu_input` drains Bevy `KeyboardInput`
messages into the buffer (`Type`/`Erase` commands feed the model) —
ASCII only (the embedded font's charset), bounded at
`MAX_NAME_CHARS`, Backspace edits, Enter validates and creates, Esc
cancels, Space types rather than activating. Three new tests drive
real `KeyboardInput` messages through the production systems, one
asserting the drawn `Name: …_` field line. Before that the latest
iteration landed F17-B.3, the countdown presentation: a
`SessionEntity`-stamped `CountdownBanner` overlay (in `HudNodes`,
follows the active camera, despawns with the session) driven purely
by the authoritative `RaceState` — `ceil(remaining / RACE_TICK_HZ)`
digits `3`/`2`/`1`, a one-second `GO!` flash bounded by
`COUNTDOWN_GO_TICKS` off the race clock, hidden while `Paused`/`Results`,
against a stale generation, or with no race. Presentation-only
(DSN-19): the authority, `input_locked`, `RaceStarted` and phase
transitions are untouched. Rendered on Metal/Apple M1 — a
mid-countdown frame shows the digit over the starting grid
(`sf checkpoint:0 --frames 60` → `bytes=4157777`) and a post-release
frame shows `GO!` (`--frames 200` → `bytes=4543805`), both PNGs
inspected. Three new race tests cover the digit sequence, the `GO!`
window, pause/results/stale hiding and restart teardown. Before that
the latest iteration landed F17-B.2, the results screen and the
play→reward→return leg: `mm2_app::results` draws a
`SessionEntity`-stamped overlay at `Results` (UI-5) — local outcome
(ordinal placing + total time), the generation-scoped standings with
unresolved participants listed as still racing, the rewards actually
granted, and the persistence disposition — with Continue/Restart
rows; `record_session_results` now writes a `SessionReport` resource
(processed once per generation, local participant only, sandbox and
scripted/bot runs excluded, `record_eligibility` enforced, `TimedOut`
records nothing) so the screen presents the real disposition instead
of a log line. `Failed(reason)` now rides a `SessionNote` back to the
menu's status line; `menu_watch` enforces shell ownership (active only
at `Menu` with no pending restart) — repairing the latent defect where
a restart's transient `Menu` phase could reopen the shell over a live
session; `--finish` (quarantined, record-ineligible) sweeps the local
car through its objectives so `--frames`/`--screenshot` captures can
reach `Results` — verified on Metal/Apple M1
(`sf checkpoint:0 --finish --frames 180` → `bytes=2573476`, PNG
inspected). Nine new results tests plus two menu regressions cover
outcome/field/unresolved presentation, timeout, reward and
ineligibility notes, key ownership, continue-quit and restart legs,
`--finish` reachability + ineligibility, the menu-reopen repair and
the failed-load status. Before that the latest iteration landed
F17-B.1, the pause flow: `Esc`/pad `Start` on a live `Playing`
session takes the previously unreachable `Playing → Paused` edge
(only when `SessionAuthority::allows_pause()` — MP-6 keeps
host/remote Esc as quit), `mm2_app::pause` owns the `Paused`
keyboard and draws a `SessionEntity`-stamped Resume/Restart/Quit
overlay over the dimmed frozen world (`Options` stays a visible
disabled row naming F23), and `sync_physics_pause` mirrors the phase
onto `Time<Physics>` so Avian's whole schedule — not just inputs —
holds still; `--pause` (quarantined `DevOverrides`) auto-pauses the
first `Playing` frame so captures can render it. Known Avian quirk
recorded: the runner drains one stale-delta physics step on the
first paused FixedMain frame before the freeze takes hold. Before
that the latest iteration
landed F17-A.2, Quick Race (DRV-8): the root menu gains a
`Quick Race: <Table> #<n> (<stem>)` row that relaunches the bound
profile's `selections.last_event` with the current
vehicle/paint/difficulty selections — resolved stem-keyed through the
live `EventCatalog`, so a mod inserting/removing table rows can never
retarget the save onto a different event. Every unreachable leg is a
disabled row with its reason: no bound profile, no event played yet,
Crash Course `last_event` (not loadable, F21), missing `city/*.psdl`,
stem absent from the catalog, `Incomplete` record set, and the same
availability gate the event list enforces (`beat race0 first`). The
documented original inserts a vehicle-select screen between the pick
and the launch; this shell keeps vehicle/difficulty as persistent root
selections, so the row launches directly — recorded as an
enhanced-layout choice in the code, not an original-rules claim.
Rendered evidence: a real `sf checkpoint:0` session under
`--new-profile QR` wrote `last_event = sf/checkpoint/race0` to the
profile at session start, and `--menu --profile driver-0 --frames 90
--screenshot` drew the enabled `Quick Race: Checkpoint #0 (race0)`
row on Metal/Apple M1 (`world=menu ... bytes=172980`). Before that
the iteration repaired F17-A.1's
external-review blocker: the shell drew nothing because `bevy_ui`
renders per camera view and the only cameras were
`SessionEntity`-stamped (zero existed at boot or after quit-to-menu).
`menu_present` now owns a `MenuCamera`-tagged `Camera2d` while the
shell is active — pinned onto the UI root via `UiTargetCamera`, kept
stable across redraws, despawned when a session takes the screen
(regression test `the_menu_draws_into_its_own_camera`). A new `--menu`
flag pairs with `--frames`/`--screenshot` so the existing capture
harness can render the shell itself (`world=menu` records;
`menu_input` freezes during captures like every other input), and the
first rendered capture exposed a second defect — the bundled font
lacks `›`/`•`/`—`/`·`/`↑`/`↓` so markers and separators were tofu;
user-facing strings are now ASCII. Before that repair the iteration
landed F17-A.1, the menu
shell: a bare `--mm2-path` boot now parks in `mm2_app::menu` over the
real catalogs — Cruise cities from `city/*.psdl`, event tables/rows
from `EventCatalog`+`AvailabilityTable` (CHK-3/CC gates and incomplete
content name their reasons instead of hiding), the `listed` garage
with locked-vehicle/gated-paint refusals, the profile screen with
select/create and `X`/`Delete` behind `Screen::ConfirmDelete`
(F16-AC06's deliberate-confirmation leg — a plain activation binds;
deleting the bound profile unbinds; the last profile refuses per
DRV-7 with the reason left visible). `AvailabilityTable::of_unbound`/
`GarageTable::of_unbound` give the fresh-driver view when no profile
is bound. Launches build the real `SessionConfig` and call
`Session::begin`; `menu_watch` reopens the shell on `Unloading →
Menu`, and `drive_session` only writes `AppExit` at Menu when no
`MenuShell` exists — every session-shaping or smoke flag still boots
directly into a world. Seven headless integration tests cover boot,
the launch→quit→menu→relaunch loop (AC06's menu leg), gated/incomplete
event rows, garage→`SelectedCar` carry-through, the profile
bind/create/delete flow, the camera lifecycle, and the empty-install
honest-failure path. Rendered evidence exists: a
`--menu --frames 60 --screenshot` run against the retail install drew
the real root menu on Metal/Apple M1.
Still open under F17-A: per-event
weather/time/density (needs F18's session-legal writers) and
F17-AC03's full keyboard/gamepad + visible-focus evidence leg.
The F17-AC05 capability denominator now lives in
`docs/research/menu.md` (every documented capability is
implemented or tracked — Driver's Stats, per-screen Help and
original art/audio stay explicitly open). Before that the iteration landed F16-C.1 — the
AC01/AC04 evidence legs: a two-profile restart-isolation test drives
A to a real authored finish through the production race/progression
path, drops the app and reopens the store on a fresh handle (a new
process's only view is the files), binds B and proves no
progress/unlock/selection leak in either direction — including A's
earned grant still gating `vpreward` for B through
`vehicle_gate_note` — then drives B's own finish and proves A's file
untouched. The AC04 leg binds through a complete-but-orphaned `.tmp`
(crash between flush and rename) and verifies the bind-time heal.
Review repairs folded in: the stale `Unlock::Paint` "unverified
index base" comment now records the measured zero-based variant
(DSN-16), and VEH-5's nonzero `UnlockFlags` list gains the omitted
`vpeagle` (verified `0/1` on the roster audit). `mm2-inspect events`
now prints the reward coverage reconciliation AC05 wants — `N
authored rows → E event-bound + M milestone rules, K diagnostics` —
no longer hides a city whose authored rows are all diagnostics, and
fails strict when accounted ≠ authored; retail records 10 → 4+6, 0
diagnostics per city. Still open under F16-C: the AC01 process-level
leg (two real interactive launches completing an event) is unproven —
headless `--bot` finishes are deliberately ineligible, so the
synthetic-integration legs are the recorded evidence. Before that the iteration
landed F16-B.3, vehicle/paint selectability derived state for F17's
garage (DSN-18): `mm2_game::progression` gained `GarageTable`
(one `GarageRow` per catalog entry — `listed`, `VehicleGate`,
per-paint `PaintGate`s, verbatim `UnlockScore`/`UnlockFlags` audit
fields) evaluated per query off the persisted `unlocks` set;
`mm2_content::garage` produces it from the catalog plus the union of
every race city's `RewardTable` — a `vehicle:` grant gates the car, a
`paint:` grant gates the zero-based `Colors` index (now measured:
vpvwcup variant 5/6 = "Team Angel"/"Team MS", the documented cup
paints), roster membership is the canonical `tune/*.info` scan
(vpmoonrover's `.inf` → unlisted, UNK-3 stands), and grants that miss
the catalog land in diagnostics. The misleading
`CatalogEntry::locked` field — `UnlockScore|UnlockFlags` nonzero,
which marks the wrong set — was replaced by the raw authored fields
plus `canonical_info`. Wiring is warn-don't-enforce, mirroring the
locked `--event` launch: `mm2_app::profile::vehicle_gate_note`
surfaces a locked/unlisted/uncatalogued `--car` or remembered
selection at launch, and `--list-cars`/`mm2-inspect cars` print the
gate column. Before that the iteration
repaired F16-B.2's external-review blocker: `crash_gate` computed the
`midtrm<N>` lesson-group bounds (`3N-2`…`3N`) in u32 on an authored
tag number, so `N >= 1431655765` panicked under overflow checks and
`midtrm2863311533` wrapped onto real group 5 in release — a silent
wrong gate. The bounds now compute in u64, so an out-of-range tag can
never match a lesson row and falls through to the existing
no-lessons diagnostic + `Open`; regression test
`an_out_of_range_midterm_tag_is_open_and_diagnosed` authors lesson5-7
plus the wrapping tag and fails both ways under the old code. Before
that the iteration landed F16-B.2, event availability derived state
(DSN-17). Before that
the iteration repaired F16-B.1's first
external-review finding: the two remaining unscoped `ResultLedger`
rank consumers could report a stale prior-generation place after an
in-process restart (`Session::begin` resets `next_player`, so the
same `PlayerId` is reused each generation while the retained ledger
keeps both results). `update_hud` now ranks through
`place_of_in(session.generation(), …)` and the smoke record's
`outcome=`/`place=`/`results=` fields scope to the current
generation via a shared `smoke::result_outcome` helper — the
participant lookup picks the generation's best-ranked result and the
fallback can no longer reach back into a finished session. Unit test
`smoke::tests::outcome_scopes_to_the_session_generation` fails on the
old wiring. Before that the iteration landed F16-B.1, reward/unlock
consumption: `mm2_content::reward_table` normalizes the authored
`<city>_rewards.csv` rows (4 event-bound + 6 milestone rules per city
on retail, zero unresolvable), `mm2_game::progression` owns the
rule/evaluator layer (place criterion top-3 Amateur / 1st
Professional, beaten flags on `EventRecord`, idempotent `unlocks`
set), and `mm2_app::progression::record_session_results` drains the
authoritative `ResultLedger` into the bound profile after each event
session — generation/participant-scoped, eligibility-gated (dev
world, dev car, gameplay-affecting `DevOverrides`, mounted mods,
sandbox identity, `TimedOut` all excluded). Before that the latest
iteration landed F16-A.2, profile application wiring in `mm2_app`:
`--profile <id|name>` / `--new-profile <name>` / `--sandbox` /
`--profile-dir` / `--no-profile` flags, the `active` marker bound
implicitly on interactive runs (smoke/evidence runs stay profile-less
unless a profile is explicitly requested — their records must remain
reproducible), the bound profile's remembered vehicle/paint and rank
restored into the session config with `--car`/`--paint`/`--pro`
overriding, a remembered vehicle that no longer resolves degrading to
the stock default rather than hard-failing, and the session's actual
selections (driven vehicle + stem-keyed `EventKey`, never a row index)
persisted at session start through `load_session_world` — after the
world and race resources initialize, so a failed load records nothing.
Before that the iteration
repaired F16-A.1's second external-review blocker: the same
"deleted id is never reused" invariant was still falsifiable on a
clean public-API path — deleting the *highest-numbered* profile
erased every trace of its id (no crash-state orphan needed), so the
next `create` reallocated it and a stale `active` marker resolved to
a different person. `create` now allocates from a persisted `next-id`
high-water mark — advanced through the same tmp+rename+dir-fsync
write before the new profile's first save, floored at the highest
surviving file suffix — so deleting the max id retires it; losing
the mark degrades to the file-scan floor (a deleted highest id could
then reissue, but no live profile is displaced). Also fixed in
passing: `summarize` distinguishes a superseded main ("holds an
older revision") from a missing one, and marker reads (`active`,
`next-id`) are size-bounded like profile documents. DSN-15 wording
corrected to match. Before that the iteration
repaired F16-A.1's first external-review blocker: `.bak`/`.tmp` orphans now
occupy their id (no reallocation, listing shows recovered metadata),
`load` keeps the highest surviving `revision` (a complete `.tmp` is
the newest copy), id strings are boundary-checked before becoming
paths, `set_active` fsyncs, deletions unlink the main last and the
directory is fsynced after renames, and `validate` rejects
unsorted/duplicate `events`. Before that the iteration
landed F16-A.1, the versioned profile store (`mm2_game::profile`):
`driver-<n>` ids that are never reused, atomic tmp/`.bak`/rename saves
with backup-recovery reporting, schema-versioned documents that
preserve unknown fields, stem-keyed event progress, a sandbox kind
gating progression, DRV-7's last-profile refusal, and the `active`
selection marker — all store-level, no app wiring yet (F17 owns UI
flows; AC01's restart-isolation evidence needs F16-B's result
consumption). The previous iteration's listed candidates were
reassessed: the F11-C remainder is evidence-recording, not code;
the F15-B remainder's named items are research-gated (catch-up
semantics unverified, param-tail consumption pending verification,
`avoidOpponents` polarity open); F16-A was the highest-value ready
slice with both dependencies checked, and it unblocks the
F16-B → F17-A chain. Before that the iteration landed F11-C.1, single-event
dependency inspection (`mm2-inspect event <dir> --city <stem> --event
<table>:<row>`): one authored row's complete dependency closure
validated through the production catalog/producers — every attributed
record's parse status, deep-parse + `validate()` of the aimap/pathset
records catalog scan leaves `Unparsed`, both `RaceDefinition` and
`OpponentRoster` builds at both difficulties via the now-shared
`audit_build`/`audit_roster` units, wired vehicle ids cross-checked
against the vehicle catalog, incomplete events reported with their
full inventory rather than hidden, `--strict` failing on any finding.
Retail evidence: `sf circuit:1` (21 records, both builds, honest
unreferenced-route finding → strict 2), `london crash:0` (linked
`crash0a`-style CSV attached, `unsupported` not failed), `london
blitz:0/9`, `sf checkpoint:0` (authored count anomaly), `blitz:99` →
exit 2. Still open under F11-C's parent: the remaining AC legs are
evidence-recording, not code — the full-catalog strict audit run
against the fingerprinted install and AC02–AC05 promotion through the
already-landed runtime slices. Before that the iteration landed F15-B.3, bounded opponent
re-anchor recovery: a displacement-based stuck window (900 driving
frames without 8 m — displacement, not grounded speed, so penned,
hull-beached and knocked-off-route cars all count, including the
ungrounded cases the scripted recovery's grounded-gated detector
never sees) fires a disclosed `ResetVehicle` onto the chased route
leg — `Teleported` breaks the swept segment (no checkpoint banking,
AC04) and `reanchor_pose` walks the landing back along the authored
polyline out of un-cleared triggers (4 m clearance, ≤60 m walk,
open-route start clamp, closed-route wrap leg). Authority-only;
`OpponentDriver::reanchors` counts each assist and the smoke record
reports `opp_rec=` when any fired. Retail (`fnv1a64:e91e6cd4b2ae30d9`):
`sf checkpoint:0` 5400 `opp=5/6 opp_rec=2` (vpbug re-anchored twice;
baseline 4/6); `sf circuit:1 --bot` 14400 `opp_rec=15` across the
field (vpbug ×5) with `opp=0/7` still at the 240 s cap — the assist
fires on genuinely stuck cars and is observable, but the field still
cannot finish three laps inside the window. `sf circuit:1` 900-frame
hold-driver stays bit-identical. Still open under F15-B's parent:
catch-up semantics, AC06's measured difficulty effects, param-tail
consumption for the remaining columns, `avoidOpponents` polarity.
Before that it landed F15-B.2, the authored
`[Opponent]` parameter-tail slice of the difficulty model: the
ten-value tail decodes into `mm2_game::OpponentDriveParams` (the
vocabulary mm2hook recovers for `OpponentData`/`RegisterRoute` —
*inferred* mapping, RACE-14 new; short rows decode trailing fields
absent, not zero), and `mm2_app::opponents` binds it per driver at
spawn into a new `ScriptedTuning` overlay on the shared control law:
`maxThrottle` ceilings every throttle demand, `cornerSpeedMultiplier`
scales the corner-brake engage speed, and `avoidPlayers` gates which
human participants the traffic corridor senses. `avoidOpponents`
stays **decoded but inert** — retail authors it ≈ universally 0, so
consuming it under the inferred mapping would blind every stock
opponent to the field (the flag's order/polarity is unverified);
`avoidTraffic`/`avoidProps` stay inert (no such runtime classes);
the rest are bound, unconsumed. `ScriptedTuning::DEFAULT` reproduces
the pre-tail law bit-for-bit. Evidence: `sf circuit:1` 900-frame
hold-driver is bit-identical to the verified record; `sf
checkpoint:0` amateur (vpbug 0.70–0.75 throttle) vs professional
(0.93–1.00) runs differ in field composition but 5400-frame finish
counts land 4/6 both — disclosed as dynamics data, not a controlled
throttle A/B (vehicles and routes differ too); `sf circuit:6 --bot`
5400 holds `pos=2/7` amateur, `pos=5/7` professional. Still open
under F15-B's parent: catch-up semantics, AC06's measured difficulty
effects, `weirdPathfinding`/`distancePadding`/`cornerBrakingThreshold`
consumption once semantics verify, and the `avoidOpponents` polarity
question.
Before that it repaired F15-A.3's one external-review
blocker (`cir6_strtpnts`' all-zero `a` column is *no authored
heading* — `RaceStart.yaw_deg` is now `Option`, and a `None` slot
derives a course facing instead of a verbatim −Z: the player faces
`course_yaw`, grid-slot opponents the route staged heading/first leg).
Before that it landed F15-A.3, authored start
headings, after measuring all 612 retail `.opp` files and the
`cir*_strtpnts` grids: the `.opp` row-0 `brake` field is a staged
heading and the `_strtpnts` `a` column shares the vehicle-yaw
convention with it — both 180° from the waypoint `a` course bearing
(the UNK-16 split, now measured). The player spawn was reading slot
yaw as a bearing and adding 180° — the player spawned backward on
SF's authored circuit grids (`cir1_strtpnts` ~92° faces the −X
course; the hold-driver run now drives −X). Opponents now face their
slot's authored yaw on a grid or the route's staged heading at an
anchor, and the initial chase index skips anchors behind the staged
facing instead of U-turning off the line (`circuit1-a-0`'s line joins
mid-leg). Retail parity vs the same-iteration baseline: `sf
circuit:1` identical (`opp=0/7`, `cp=4/10` — a 240 s frame cap on a
long course, not a stall), `sf checkpoint:0` `opp=6/6` vs `3/6`,
`london circuit:0` `opp=2/7` vs `3/7` — one fewer finisher through
the authored-heading launch, disclosed honestly.
Still open under F15-B's parent:
the difficulty/param-tail driving model (UNK-11), catch-up semantics,
AC06's measured difficulty effects.
The iteration before repaired an external-review finding
(`update_checkpoint_markers` picked an arbitrary `Player`+
`RaceProgress` participant once opponents also carried both; it now
selects `PlayerControl::Local` like `update_race_warning`, regression-
tested) and landed F15-B.1, opponent traffic avoidance: a forward
corridor senses live participants (player included), a committed
route-relative offset lane aims around blockers, a closing-scaled
comfort/panic brake manages following distance, an occupancy scan
keeps commits out of occupied lanes, matched-pace cars queue rather
than weave, and a displacement-based stall abandons uncompletable
passes — the banned blocker goes fully transparent for a bounded
shove window before the clean pass retries. Retail vs the F15-A.2
baseline: `sf checkpoint:0 --bot` identical `opp=3/6`/`place=4` with
fewer impacts (195 vs 208); `london circuit:0 --bot` `opp=3/7` vs
baseline `2/7` — the first retail circuit opponent evidence at all,
run this iteration for both builds (F15-AC02's circuit leg); a
fixed-window pack run shows equal finishers with fewer impacts.
The iteration before landed F15-A.2, the opponent spawn/drive leg of
F15-A: `mm2_content::load_opponent` loads each roster vehicle with its
authored `<id>_opp.vehcarsim` tuning merged as a sparse override over
the base tune (RACE-13 — most `_opp` files omit fields and several
author an alternate Trans schema; unrecognised `_opp`-only fields
surface as warnings); `mm2_app::opponents` spawns one session-owned
participant per authored roster entry — own `VehicleDef` (per-vehicle
character kept, not player clones), `ObjectId`/`PlayerId`,
`PlayerControl::Ai`, shared `RaceProgress`, `OpponentDriver` —
and `opponent_drive` chases the authored `.opp` polyline through the
same normalized `VehicleInput` control law the scripted evidence
driver uses (advance-past-reached anchors, closed-route wrap, bounded
stuck recovery, countdown/hold gates). Provisional (UNK-16/17):
`_strtpnts` slot `index+1` when a grid ships, else the route anchor,
else a stagger behind the player; facing from the route's first leg.
A vehicle that fails to load keeps its authored slot reported and
skipped; a dead `.opp` ref holds still. Retail `sf checkpoint:0`
headless: 6 opponents spawn, `opp=3/6` finish through shared
`advance_race` validation, player places 4/7. Ledger: RACE-13 new
(`_opp` sparse-override schema measured on all 23 files), UNK-11
updated. Still open under F15-A's parent: difficulty/param-tail
driving model and everything F15-B.
The iteration before landed F15-A.1, the opponent-roster import leg
of F15-A: `mm2_game::opponent` declares
`OpponentRoute`/`OpponentSpec`/`OpponentRoster`/`OpponentIssue`;
`mm2_content::opponents::opponent_roster` builds a roster from a
`CatalogEvent` — `<stem>.aimap` binds Amateur, `<stem>.aimap_p`
Professional (explicit fallback when only the amateur file ships);
each `[Opponent]` row wires vehicle id + `.opp` route + raw param
tail (first kept as `skill`), route points preserved with every
authored column. Roster issues (`MissingVariant`, `UnresolvedRoute`,
`WrongDifficultyTag`, `CountMismatch`, `UnreferencedRoute`) are
reported, not repaired; a wired-but-missing route keeps its authored
slot. `OpponentReport::scan` + `mm2-inspect opponents` audit every
event at both difficulties and list extra roster-bearing stems;
retail: 64 builds/city, 0 failed, 26 unsupported (crash tables),
271 london + 246 sf opponents wired, 0 unresolved vehicle ids,
43+34 unreferenced route records, `sf/race0` amateur remains the only
count mismatch; `--strict` exits 2. Ledger: RACE-12 new, RACE-11
strengthened to an all-table measurement, UNK-11 narrowed to
route/parameter semantics + the driving model.
The iteration before landed the remaining authored-physicals leg of
F06-B (F06-B.2): `TireSurface` gains `drag` — the def's `drag` field
consumed raw (`_default` authors 0.0, nothing to divide by) as a
per-wheel viscous wading resistance (`-v_plane × drag × load` in the
contact plane, outside the friction ellipse — fluid resistance on the
wheel, not a tire force; only water/deepwater carry it on retail, so
dry surfaces are untouched) — and `SurfaceTables::contact_restitution`
scales the def's `elasticity` into `0..0.1` (the same cap `convert`
applies to `BoundElasticity`), attached as Avian `Restitution` on
every named-material collider. `WheelState.surface_drag` reports the
coefficient per wheel; collider `Friction` deliberately stays at
Avian's default (authored `friction` is a tire-grip coefficient —
applying it to chassis/prop contact would fight the
`MAX_COLLIDER_FRICTION` scrape policy). Retail: london
`--spawn=-80,2,805,0` drops the car onto the Thames `deepwater`
colliders (rooms ~337–364, y=−4.0) where full throttle crawls at
`peak=1.1m/s` vs the unchanged 29.0 m/s land baseline; the added
restitution shifted two recorded banger scenarios on named-material
streets (sf restage `0a/5s/2b`→`0a/2s/1b`, london ring `3a/0s`→
`3a/3s` — feature working, not regressions; `docs/research/banger.md`
annotated). AC03's runtime texture-swap invariance test landed
(a higher-priority mount replacing the texture file leaves
`SurfaceMaterial`/`TireSurface`/`Restitution` identical). Still open
under F06-B: AC05's audio/dust consumer consistency (no consumers
exist — F07 scope) and AC06's network authority (F24 scope).
The iteration before landed the traction leg of F06-B:
`mm2_vehicle::surface` declares the physics-side inputs the sim
consumes — `TireSurface` (collider component, normalized material
grip) and `TireConditions` (session resource, environment/wetness
modifier kept as a separate term). `SurfaceTables::tire_surface`
normalizes each authored `friction` against the `_default` block
(`_default` → 1.0; retail water ≈ 0.76, deepwater ≈ 0.72,
cobblestone/grass/sand → 1.0 — an implementation choice, not a
verified original rule), and `emit_psdl` attaches it beside
`SurfaceMaterial` on every collider. `vehicle_simulation` resolves
`contact_entity → TireSurface` × `TireConditions.traction` into one
`surface_grip` applied to lateral force, the longitudinal limit, the
TC cap and the friction ellipse — exactly once — and wheel telemetry
+ impact events report the environment term in `SurfaceState.traction`.
A quarantined `--traction <f>` dev override (finite, ≥0, else usage
exit 2) makes the environment leg demonstrable; retail london
`--traction 0.5` drops the 600-update smoke peak 29.0→22.3 m/s.
Synthetic tests prove per-wheel material grip over split ground, the
modifier composition, and measurable delivered-force differences on
real Avian physics (AC02's physics leg). Still open under F06-B:
`elasticity`/`drag` consumers, AC03 runtime texture-swap invariance
test, AC05 audio/dust consumer consistency, AC06 network authority.
The iteration before landed F06-A.2,
the runtime-identity leg of surface materials: `mm2_content::surface`
produces `SurfaceTables` from the VFS-resolved `city/materials.{csv,mtl}`
pair and `mm2_app::city::emit_psdl` splits each room's collider per
authored material index — every collider entity carries
`SurfaceMaterial::Authored(i)` (the `MaterialSet::defs` index space,
held session-scoped as a resource) or `Unspecified` for `none`/blank/
unmapped/dead-ref slots. Wheel raycasts and the impact pipeline read
the identity off the contact entity unchanged; `CityReport.surfaces`
records the named/none/blank/unmapped split plus table issues.
Synthetic tests prove per-region ray classification on real Avian
physics (AC01's collider leg); UNK-23 still covers the original's
`_default` policy and consumer semantics — the fallback is a
documented conservative implementation policy. The iteration
before landed F06-A.1,
the authored-data leg of surface materials: `mm2_formats::materials`
parses the global `city/materials.{csv,mtl}` pair (texture→material
map + `mtl` property blocks) and `mm2-inspect materials` audits the
pair plus each PSDL texture table's coverage, measured on retail
(137 named mappings, 8 materials, 2 dead authored refs; both cities'
tables classified). WLD-19/UNK-23 recorded. The iteration before
landed F14-B.1, the live running-order leg of
participant ranking (also the rank-presentation remainder of
F13-B): `mm2_game::race::live_order` sorts participants best→worst
while a race runs — `Finished` lead by recorded `race_ticks` (the
same key DSN-12's standings use, so the order converges as
everyone resolves), active participants by progress (`Ordered`:
`(lap, next)`; `AnyOrder`: cleared-gate count), progress ties by
straight-line XZ distance to each participant's own objective
(`checkpoints[next]` / `navigation_target`'s nearest remaining
gate or armed finish), `TimedOut` trailing, `PlayerId` breaking
all remaining ties (DSN-13, designed). The HUD shows `{ord} of {n}`
while `Countdown`/`Running` whenever ≥2 participants have a
standing (the terminal `FINISHED {ord}` line is unchanged), and
the smoke record gains `pos={i}/{n}` for the local participant.
A production-path test drives two participants through a 2-lap
Ordered course exercising every rule: progress beats position,
proximity breaks a progress tie, a finished place locks ahead of
an active leader, and the live order resolves into the ledger's
standings. Retail: london `blitz:0 --bot` headless reports
`pos=1/1 outcome=finished place=1` — the field composes on real
authored content. The iteration before
that landed F13-B.1, the standings/placing leg of
participant progress: `ResultLedger::standings`/`place_of` define the
authoritative finish ordering (DSN-12 — `Finished` outranks
`TimedOut`, `race_ticks` ascending, equal ticks broken by `PlayerId`;
unrecorded participants are unplaced), the Results HUD shows
`FINISHED {ord}[ of {n}]` beside the recorded time, and the smoke
record gains `place=` on the local participant's result. A
production-path test races two participants through a 2-lap Ordered
course — independent `next`/`lap` state, remote finishes first,
standings order them by recorded clock. Iteration
42 landed F14-A.1, the binding-honesty leg of
circuit/lap rules: authored `NumLaps` is now a checked parameter
(`BadParam` on `≤0`/overflow instead of a silent `.max(1) as u32`
clamp+truncate — the column's 2–3 amateur / 2–4 professional counts
bind per difficulty, CIR-5), `RaceProgress::advance` is inert
outside `Racing` (a resolved participant cannot re-finish or clear
more gates even if positions are still fed in — AC04's once-only
rule is a contract property, and AwaitingStart steps only
re-anchor), the dead `with_next` builder is removed, `Ordered`'s
start-lap semantics are documented in the contract (lap 1 begins at
release; the start-line copy closes each lap), and the headless
smoke record gains `lap={cur}/{laps}` for Ordered defs. Retail:
`race-defs --table circuit` shows 10/10 rows/city building with
distinct authored laps/gates/slots; london `circuit:0 --bot` records
`lap=2/3` mid-race and `lap=2/4` under `--pro`. Iteration
41 re-took the F04-C.2/-C.3 retail evidence the
operator-report repair had invalidated: on corrected geometry the
original SF barricade-row staging spawn now sits just under the authored
`ImpulseLimit2` (the bus slides around the row — corrected record,
not a regression), a perpendicular restage re-proved flat-ground
break + fragment `Slept` settle + pool caps, the London reclaim ring
is bit-identical, the F04-C.3 bound-strikes re-verified (cones
`3a/2s` — full-height bounds overlap more; bus `1a/1s`; bug `1a/1s`),
and the operator's `sp_sawhrslt_f` roadblock — actually the
`checkpoint:7` `race7.pathset` overlay, a labelling slip the re-take
caught — now shatters at ~9–12 m/s under `vpddbus`. Iteration
39 landed F09-C.1, the route-constraint leg of nav-graph validation:
`NavGraph::reachable_arcs` walks only authored exits, so the new
`mm2-inspect nav --routes` census measures real directed connectivity
instead of assuming it — London's vehicle graph is strongly connected
(606/606 arcs) while SF's has an authored one-way-trap cluster (4927
unreachable ordered pairs, ~1.3%, vs WLD-11's weak single component),
and `race/*/blitz0.aimap` `[Exceptions]` demonstrably sever routes at
query time with zero closed-road traversals. `--aimap <logical>`
supplies any event aimap's overrides for the probes. WLD-11 extended
with the directed census. Iteration 38
landed F13-A.1, the results-flow leg of per-event Checkpoint
objective/finish semantics: `Playing → Results` was a legal session
transition nothing ever made, so a finished/timed-out race sat in
`Playing` forever despite UI-5's results-screen rule. `advance_race`
now transitions on the same step it records a *local* participant's
`Finished`/`TimedOut` (remote/AI resolution never ends the local
race); the HUD's `Results` arm surfaces the outcome + recorded
finish time. Retail `phase=` records now show `results` on real
authored events (sf `checkpoint:0` `cp=6/6 outcome=finished`, london
`blitz:0` `outcome=timed-out`). Ledger evidence: `.aimap`/
`.aimap_p` measured as Amateur/Professional actor rosters on 23/24
checkpoint events (`sf/race0` authored anomaly — RACE-11), and all
24 checkpoint waypoint files confirmed ≥3 rows with a distinct
last-row finish (WPT-2). Iteration 37 resolved the
hull-clearance fidelity question as F04-C.3: mm2hook's layout shows
prop collision is bound-vs-bound (`dgBangerData.Bound`/`ColliderId`),
so `convert()` now keeps the *unmodified* authored bound beside the
snag-safe hull as `striker_points` → a `StrikeBound` component used
only for shape-overlap queries — world contact still meets only the
raised hull, while dormant bangers activate when a moving bound
overlaps them (surface velocity at the prop centre vs the authored
`ImpulseLimit2`). Retail: `vpddbus`/`vpbus` now activate the
cone/bollard rows they ghosted; `vpbug` records `2a/2s` where contact
alone gave `1a/1s`. Iteration 36 landed F03-B.4: the decal pathset channel,
the last ambient placement source. Retail decal `LineStrip` paths
interleave two authored ribbon edges (measured — even/odd pair =
cross-section, WLD-18): `mm2_app::decals::stamp_decals` builds quads
between sections, merges per texture stem, honours palette alpha and
TEX clamp flags, lifts/biases against z-fighting, and spawns
render-only entities. London 79 ribbons (zigzags, zebra crossings,
box junctions) and SF 48 (cable-car rails, skid marks) render in
place; v-tiling/stretched semantics and u orientation remain
documented inference. The decal channel closes the ambient-city
placement inventory — every remaining pathset family needs a feature
(animated objects, audio routes, event stems) rather than a stamping
rule. Iteration 35 landed F03-B.3: the
prop-rule stamping channel. The retail geometry model is verified
(WLD-17): `Psdl::paths` records are road runs between junction
crossings, `road_rooms` chains road rooms, crossings are four-point
`[outer, curb, curb, outer]` perimeter runs (end crossings named by
`start/end_crossroads` curb pairs, interior boundaries found via the
widest neighbour-marked pair), and the two arcs between crossings
are the sidewalk building lines. `mm2_game::props::walk_prop_rules`
resolves rule bytes through `proprules.csv` → `propdefs.csv` and
emits bounded `PropStamp`s; `load_city` spawns them through the
shared `PropCache` — every retail prop-rule file resolves to a
bound banger record, so all 5 002 sf / 5 083 london stamps become
dormant bangers (consistent with WLD-16). Field semantics
(`start`/`distance`/`maxUse` scope, variant pick, `minLerp`/
`maxLerp`, left/right label convention, yaw axis) remain inferred
under narrowed UNK-21; screenshots show curb-edge dressing matching
retail spacing on both cities including multi-room freeway
parapets. `decode_tex` now clamps over-declared mip counts
(`p_parkmeter_f.tex`) instead of hard-failing wgpu validation.
Natural-pool (>32 simultaneous) reclaim remains unstaged from
F04-C.2 — no surveyed retail site produces that density.
Iteration 37 repaired the F03-B.3 channel's kerb geometry
(F03-B.6): the walk modelled each side's kerb as the straight
chord between crossing curb corners, but the authored kerb bends
with the road surface and lives in the room's road attributes —
the chord put up to ~12 m of stamps inside the carriageway on
curved blocks (operator screenshots showed trees/lamps on the
road). `walk_prop_rules` now extracts each side's (kerb, outer)
vertex chains from `RoadWithSidewalks`/`DividedRoad`/
`SidewalkStrip`/`RoadNoSidewalks` attributes, matched to the
side's curb-corner vertex ids, and stamps along the authored
kerb index-paired to the outer chain; unmatched sides fall back
to the building-line arc (counted `sides_no_kerb` — 0 on both
cities). BAI audit: london in-road stamps 699 → 348 (worst
11.2 m → 1.3 m), sf 318 → 159 (12.2 m → 1.0 m); residuals are
a systematic ~0.8 m BAI-vs-PSDL curve inset, not placement
error. 5 118 london / 5 028 sf stamps; field semantics still
inferred under UNK-21.

## Baseline gate results (this checkout, 2026-09-20)

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --locked --workspace` | PASS — all groups, 0 failures (incl. 18 mm2_game race contract tests, 27 mm2_app production-path race tests, 15 mm2_vehicle drive tests, 13 F01-A session tests, 6 scripted-driver tests) |
| `mm2-inspect cars <retail>` | 29 catalog entries; all 21 `EXPECTED_STOCK_ROSTER` cars `ready`; 8 extra ids kept with explicit incompleteness reasons |
| `mm2-inspect list <retail>` | 13,389 logical paths; families: texture 3977, aud 3293, geometry 1867, tune 1356, race 1080, bound 1034, city 247, anim 95 |
| `mm2-inspect inventory <retail>` | 12 families: cities 2/5 exp/disc all parsed; vehicles 21/21 ready + 8 rejected; races 80 exp, 78 accepted, 2 partial (circuit11), 31 extras; lessons 42/42; placement 13 inst parsed; audio 7/7 families (3293 files unverified); peds 4/4 + wolf partial; MP/breakables/traffic/profile/interface discovered-only. Event-metadata tables parse: 12/10/10/13 checkpoint/blitz/circuit/crash rows per city. `--strict` exits 2 (33 findings) — honest: partial/junk records exist on retail. |
| `mm2-inspect events <retail> --strict` | exit 0 — london 45/45 ready (12 race + 10 blitz + 10 circuit + 13 crash, 33 extras listed), sf 45/45 ready (32 extras); per-event + milestone rewards resolved; no failed refs |
|| `mm2 --dev-world --headless` | `status=pass` (updates=600, ticks=1198, impacts=1 dropped=0, peak 27.9 m/s, moved 157 m, 4/4 wheels) — dev world starts with no MM2 data, car settles + drives; session clock ≈2× updates at 120 Hz; impact pipeline live |
|| `mm2 --mm2-path <retail> --city sf --headless` | `status=pass` (updates=600, ticks=1198, impacts=3 dropped=0, peak 37.2 m/s, moved 172 m) — 1171 rooms / 3763 props via VFS, imported vpbug driving on real city collision; contract pipeline emits real impacts |
|| `mm2 --dev-world --frames 90 --screenshot` | `status=pass`, 2.9 MB PNG awaited + verified (GPU/render evidence recorded on this machine) |
|| `mm2 --headless --city bogus` (no data) | `status=unavailable`, exit 4 — missing data is not a failure |
|| `mm2 --mm2-path <retail> --city bogus --headless` / `--frames 60` | `status=fail`, exit 3 — explicit failure, logical path + reason |
|| `mm2 --mm2-path <retail> --city london --event blitz:0 --headless` | `status=pass` — real authored Blitz loaded (`event race loaded event=Blitz[0] gates=3`), countdown released, car drove through all gates `cp=3/3`, `race=Running` (finish trigger not crossed in-window, expected) |
|| `mm2 --mm2-path <retail> --city sf --event circuit:1 --headless` | `status=pass` — `Circuit[1] gates=10`, car spawned on the authored `cir1_strtpnts` grid (moved 136 m from the authored slot) |
|| `mm2 --mm2-path <retail> --city london --event checkpoint:0 --headless` | `status=pass` — `Checkpoint[0] gates=5`, `cp=2/5` swept while driving straight |
|| `mm2 --mm2-path <retail> --event checkpoint:12 --headless` | `status=fail`, exit 3 — out-of-range event is an explicit `Failed` session, not a panic |
|| `mm2 --mm2-path <retail> --event blitz:0 --frames 90 --screenshot` | `status=pass`, 4.3 MB PNG — authored gate column visible ahead of the spawned car, HUD shows `GET READY` countdown |
|| `mm2 --mm2-path <retail> --city london --event blitz:0 --frames 100/700 --screenshot` | `status=pass`, ~4.3 MB PNGs — the RACE-6 nav needle renders top-center, green ahead, tilted toward the nearest gate during countdown and while driving (local captures, not committed) |
|| `mm2 --mm2-path <retail> --city london --event blitz:0 --headless --frames 1800` | `status=pass` — `race=Complete cp=3/3 results=1 tl=0.0s outcome=timed-out`: the deadline resolved the run once, ledger kept it |
|| `mm2 --mm2-path <retail> --city london --event blitz:0 --frames 1150/2600 --screenshot` | `status=pass`, ~4.3 MB PNGs — at `time 16.1s` no warning banner (above threshold); at `time 1.5s` the `LOW TIME` banner renders under the needle on its dim half-pulse (local captures, not committed). Windowed runs pace ≈0.93 race-ticks/frame, slower than headless |
|| `mm2-inspect race-defs <retail>` | exit 0 — london 45 + sf 45 authored rows: 64 built / 26 unsupported / 0 failed per city at both difficulties (13 crash-course rows × 2 = unsupported, not failures). Per-event cells show distinct authored values (gates/laps/time-limit/slots/opp/cop/tod/weather). `--table blitz --strict` and `--table crash --strict` both exit 0 |
|| `mm2 --mm2-path <retail> --city {london,sf} --event blitz:{0..9} --headless --frames 1500` | 20/20 `status=pass` — every authored Blitz row loads its own course, grounds (wheels contact), drives, `tl=` ticks. Pre-fix this caught 3 falls (london blitz:6 spawn-over-void; sf blitz:5/9 mid-drive) — all were the same DSN-6 back-off defect, repaired |
|| `mm2 --mm2-path <retail> --event blitz:10 --headless` | `status=fail` — "no authored event row for this reference"; `crash:0` → `status=fail` "crash course events are not loadable yet"; `bogus:0` → CLI usage error. Invalid refs fail explicitly |
|| `mm2 --mm2-path <retail> --city london --event blitz:6 --frames 700 --screenshot` | `status=pass`, 3.7 MB PNG (fresh path, local only) — car grounded on the elevated start deck (wheels 4/4), green nav needle, `cp 0/4`, `time 54.9s`; this event previously spawned over a void |
| `mm2 --mm2-path <retail> --city {london,sf} --event {blitz,checkpoint,circuit}:<all> --headless --bot` | 64/64 authored rows ran the production race path under the scripted driver — 4 `outcome=finished` through finish→result (london blitz:0 `cp=3/3 tl=7.6s`, london checkpoint:0 `cp=5/5`, london circuit:0 `cp=6/6` incl. lap wraps, sf checkpoint:0 `cp=6/6`); 20/20 Blitz `race=Complete` (1 finished + 19 `timed-out`, one ledger result each); 41 untimed checkpoint/circuit runs `race=Running` at the frame cap; 8 SF `status=fail` "fell through the world" (incl. sf checkpoint:0, which recorded its finish first). Cruise `--bot` (no event) passes; london blitz:0 `--pro` finishes with `tl=0.6s` |
|| `mm2 --mm2-path <retail> --city london --event blitz:0 --headless --frames 2000` (post-F13-A.1) | `status=pass` — `phase=results race=Complete cp=3/3 results=1 outcome=timed-out`: the deadline's TimedOut now ends the session at Results on a real authored event |
|| `mm2 --mm2-path <retail> --city sf --event checkpoint:0 --bot --headless --frames 12000` (post-F13-A.1) | `phase=results race=Complete cp=6/6 results=1 outcome=finished` — the bot's finish transitions `Playing → Results` on real content. `status=fail` "fell through the world" is the pre-existing post-resolution sanity check (car ends >25 m below spawn altitude during the remaining idle frames — already documented for 8 SF runs above, incl. this event) |
| `mm2-inspect bai <retail>` | exit 0 — london.bai 540 roads/328 intersections (culling 1342 rooms) + sf.bai 379/214 (culling 1172) parse byte-exact, `validate()` clean, all room refs in range vs matching PSDL; extras london_bak/sf_bak clean, sfai.bai parses with 1 authored anomaly (intersection room ref 0), london_sup/sf_sup `unsupported` (CAI1 magic, different layout — reported, not hidden). `--strict` exits 2 (2 issues, all on the sfai dev-map extra) |
| `mm2-inspect aimap <retail>` | exit 0 — 209/209 expected files resolve + parse (2 city + 207 race `.aimap`/`.aimap_p`, 0 unsupported extras): all declared counts exact, `validate()` clean except 9 cross-check issues — 8 london files carry `[Exceptions]` road ids 562–815 beyond `city/london.bai`'s 540-road space (UNK-18) and `race/sf/stunt0.aimap`'s `opp-c0.2` opponent ref is dead. `--strict` exits 2; `--city sf` filters to 107 files / 1 issue. `scan` recognizes `aimap`/`aimap_p` (all 209 parse). |
|| `mm2-inspect nav <retail> --route 13:50 --turns` | exit 0 — both city graphs build (unchanged B.1 stats/issues); city aimaps report 0 closed roads, speed limit 15.0; route probes reproduce the baseline exactly (sf `13- → 12- → 130+ → 50-` 4 steps/576 m, london 30 steps/1948 m). `--turns`: 1106 London + 1401 SF exits; 4-way Δccw=1→right, Δccw=2→straight, Δccw=3→left holds for 471/492 (95.7%) and 963/971 (99.2%) — the authored CCW index arithmetic and measured geometry agree where documented; other arities mix. |
|| `mm2-inspect nav <retail> --routes 512 [--aimap <logical>]` | exit 0 — directed-reachability census + seeded probes: london strongly connected (606/606 arcs reach all, 512/512 ok); SF 4927 unreachable ordered pairs (~1.3%) from authored traps (dead end road 0-; one-way traps 92/94/97/99- reach 1, 102/111- reach 2, 1- reaches 617), 505/512 ok, 7 unreachable (all trap sources), 0 violations. `--aimap race/london/blitz0.aimap` (8 closed) → 15016 unreachable pairs, 24/512 unreachable (`270→108`: 19 steps open → `Unreachable` closed); `--aimap race/sf/blitz0.aimap` (3 closed) → 7977, 11/512; 0 closed-road traversals anywhere. Bad `--aimap` → exit 2. |
|| `mm2 --mm2-path <retail> --city sf --headless --nav --nav-route 13:50` | `status=pass` — `nav=618a/1212l closed=0 route=4st/576m` in the smoke record: graph + aimap + probe loaded through the real session path. London `--headless --nav` 300f `status=pass` (`nav=606a/1141l closed=0 route=30st/1948m`); a 30-frame run failed `never grounded` — spawn settling, unrelated to nav. |
|| `mm2 --mm2-path <retail> --city sf --cam=… --frames 90 --screenshot … --nav [--nav-route 13:50]` | `status=pass`, awaited PNGs (~4.5/6.3 MB, local only — `screenshots/nav-sf.png`, `nav-sf-high.png`): lane polylines trace every street's authored lanes, white chevrons point along travel direction (opposing arrows on two-way streets), yellow crosses mark intersections, purple rail curves on the cable-car street, amber route highlight follows the probe arcs. |
|| `mm2 --mm2-path <retail> --city london --cam=99,150,-177,0,-50 --frames 90 --screenshot … --nav` | `status=pass`, ~6.3 MB PNG (local only — `screenshots/nav-london-high.png`): over Trafalgar Square, lanes + chevrons run on the *left* side of the carriageway (London's authored left-hand data) with intersection markers at each junction. |
|| `mm2-inspect pathset <retail>` | exit 0 — 101 discovered `.pathset` files audited (no filtering): 98 parse (2692 paths / 13185 points, kinds {0:137, 1:122, 2:2433}), 3 authored truncations fail (`race/london/{blitz10,blitz11,london_bridge_blitz10}.pathset`); `validate()` clean on all parsed; 120/126 unique asset names resolve via VFS, 6 dead refs confined to `city/phys/` + `bak/` files. `--strict` exits 2 (3 failures + 6 issues); `--city london` 46 files/3 failures/0 issues; `--city sf` 52/1 issue. `scan` parses 98 pathsets (same 3 failures). Identical results after the `Path::asset_name()` refactor. |
|| `mm2 --mm2-path <retail> --city {london,sf} --headless` | `status=pass` — `props.pathset` consumed beside the PSDL: london 1188 stamped instances / 87 paths, sf 925 / 113 paths with 31 `r4i_rails_f` decal paths classified (both `0 failed, 0 capped, 0 issues` under the 8192/file expansion bound). Reports in `city import`/`city ready` lines. |
|| `mm2 --mm2-path <retail> --city sf --cam=-1790,45,-1150,180,-8 --frames 90 --screenshot` | `status=pass`, ~3.9 MB PNG (local only — `screenshots/pathset-sf-lamps.png`): the stamped `sp_lightstreet_rt_f` lamp row renders at regular ~40 m spacing along the road. |
|| `mm2 --mm2-path <retail> --city london --cam=-469,8,-290,180,-10 --frames 90 --screenshot` | `status=pass`, ~5.6 MB PNG (local only — `screenshots/pathset-london-trees.png`): stamped `sp_tree1_s` bushes visible in the park. |
|| `mm2 --mm2-path <retail> --city london --event circuit:0 --headless` | `status=pass` (`race=Running cp=1/6`) — `event pathset overlay stamped files=1 stamped=181 labels=0 animated=0 decals=0 unresolved=0 capped=0 issues=0`: `race/london/circuit0.pathset` barricades stamped as session-owned props. `checkpoint:6` → 256 stamped (`race6.pathset`), sf `circuit:0` → 85 (`sp=0` → one per vertex). Ambient counts unchanged: london 1188, sf 925+31 decal. |
|| `mm2-inspect proprules <retail>` | exit 0 — 16/16 discovered prop-rule tables parse (6 expected `city/{london,sf}/{propdefs,proprules,props}.csv` + 10 extras incl `.csv.txt` exports, `city/phys/`, `sf/bak/`, `city/props.csv`, `geometry/props.csv` LOD table): all rule→def refs resolve; PSDL `prop_rule` bytes ↔ rule numbers verified (london 415 rule-bearing rooms ↔ n01–16, sf 397 ↔ n01–20, n14 unused); issues: 41 phys `*_m` + 3 phys group dead refs (dev city), `sp_bollard_pedsafe_l` + `va_garbagetruck.pkg` LOD dead refs, 1 room/city at undefined rule 205. `--strict` exits 2 (48 issues); `--city london` 4 files/2 issues, `--city sf` 7/1. |
|| `mm2 --mm2-path <retail> --city {london,sf} --headless` | `status=pass` — prop-rule channel consumed beside INST + `props.pathset`: sf `rooms=345 stamps=5002 bangers=5002 unresolved=0 no_crossing=0 bad_refs=109 unreached=52`, london `rooms=410 stamps=5083 bangers=5083 unresolved=0 no_crossing=0 bad_refs=0 unreached=5`. Every prop-rule PKG resolves to a bound banger record (WLD-16); bad refs are the encoded 65 0xx `road_rooms` values, counted not hidden. |
|| `mm2 --mm2-path <retail> --city {london,sf} --cam=… --frames 90 --screenshot` | `status=pass`, PNGs local only (`/tmp/proprule_sf.png`, `/tmp/proprule_sf2.png`, `/tmp/proprule_london2.png`) — street lamps line both sidewalks at authored ~29 m staggered spacing with banner arms over the road; london plaza shows phone booths, trees, bollards at curb edges; multi-room freeway parapets get lamps (interior boundaries work). |
| `mm2-inspect banger <retail>` | exit 0 — 999/999 `tune/banger/*.dgbangerdata` parse (1 expected `default` fallback + 994 extras + 4 `.#*.1.2` editor backups, 0 unsupported/failed): 216 standalone props (own PKG), 477 named parts (`.mtx`/embedded chunk), 254 resolved break fragments (`BREAK<NN>` chunks), 47 dead refs; `NumParts` ↔ distinct BREAK-index count holds on every standalone (0 mismatches). 54 issues (47 dead refs, 4 fragment `NumParts>0`, 2 glow-count mismatches, 1 `asBirthRule`); `--strict` exits 2. `scan` parses all 995 `.dgbangerdata` names |
| `mm2-inspect banger-bind <retail>` | exit 0 — 129 placement-source files audited (INST, `city/`/`race/` pathsets, propdefs/proprules/props CSVs, PSDL `prop_rule` reachability; expected/overlay/extra classified, dev+backup dirs kept): 3 unsupported (truncated `blitz10`/`blitz11` pathsets), 0 failures, 51 issues (dead placement refs — phys `*_m` names, `sp_bollard_pedsafe_l`, `r_concrete`, `prop_sp_barricadeconcr_f`, `xcp_banrred_f`). INST places static architecture (london 0/221, sf 0/165 names bound); `*_ai.inst` stamp bound `sp_stop_f` only; `props.pathset` 17/17 + 30/30 bound; propdefs 12/12 + 27/27 bound; PSDL-reachable def files 11/11 + 20/20 bound. Reverse: 269/994 records placed-reachable, 562 on 105 `vp*`/`va*` vehicle owners, 163 on 87 never-placed owners. `--strict` exits 2; `--city london` → 50 files/1 issue |
|| `mm2 --mm2-path <retail> --city sf --car vpddbus --spawn=-141.9,1.5,-608.5,115 --headless --frames 5000` | `status=pass` — `bng_ev=0a/5s/2b` at 10 000 ticks (bit-identical re-run): the post-F03-B.5 re-take of F04-C.2's flat-ground break+settle leg. The original `−170,1.5,−565,30` spawn now sits under the authored 51 888 limit (`0a/0s/0b` — corrected record); `--banger-pool 2`/`1` cap live fragments at 2/1; London `vpbug 802,6,-905,180` reclaim ring bit-identical; `checkpoint:7 --car vpddbus --spawn=-703,1.5,217,190` shatters two `sp_sawhrslt_f` at ~9–12 m/s. Full matrix in `docs/research/banger.md` §re-take. |

|| `mm2-inspect race-defs <retail> --table circuit` | exit 0 — 10/10 circuit rows/city build at both difficulties: london authored laps `3am/4pro` (except c1/c2 `2/2`, c9 `3/2`), sf `3/4` (except c8-9 `2/2`), 6–23 gates, 4–7 opp/0 cop, 1–8 start slots — every row binds its own authored lap/route config (CIR-5). |
|| `mm2-inspect opponents <retail>` (post-F15-A.1) | exit 0 — 64 roster builds/city at both difficulties (0 failed, 26 unsupported = crash tables): london 271 opponents wired / 43 issues (all spare `.opp` records — incl. `blitz3`/`blitz4` routes on 0-opponent events), sf 246 / 35 (34 spare routes + `race0` amateur 6-wired-vs-7-authored, the RACE-11 anomaly and the only count mismatch anywhere). Extras `race/london/race12.aimap` (1 wired) + `race/sf/stunt0.aimap` (1 wired, dead `opp-c0.2` ref) listed. 0 unresolved vehicle ids — pro lineups field `vpcoop2k`/`vpvwcup`/`vpdb7`/`vppanoz`/`vppanozgt`. `--strict` exits 2. |
|| `mm2 --mm2-path <retail> --city london --event circuit:0 [--pro] --headless --bot` | `status=pass` — amateur `--frames 12000` → `race=Running cp=2/6 lap=2/3` at 23 640 ticks; `--pro --frames 4000` → `lap=2/4`: Ordered lap tracking + per-difficulty `NumLaps` binding on real content. The bot deterministically wedges mid-lap-2 at `(-413,-169)` — bot-limited, not a lap-logic defect (the earlier matrix recorded this event `outcome=finished` under the same driver). |
||| `mm2 --mm2-path <retail> --city london --event blitz:0 --headless --bot --frames 2000` (post-F13-B.1) | `status=pass` — `phase=results race=Complete cp=3/3 results=1 tl=7.6s outcome=finished place=1`: the ledger standings' `place=` field records end-to-end on a real authored event (single participant → place 1, per DSN-12). |
|| `mm2 --mm2-path <retail> --city london --spawn=-80,2,805,0 --headless` (post-F06-B.2) | `status=pass` — car lands on the Thames `deepwater` colliders (final y=−4.0): full throttle reaches only `peak=1.1m/s moved=11m` in 10 s — authored `drag` bogs the car down on real content. Land baseline unchanged (`peak=29.0m/s moved=84m`); sf baseline bit-identical (`peak=37.2m/s`); `--traction 0.5` still records `peak=22.3m/s`. |
|| `mm2 --mm2-path <retail> --city {sf,london} --car {vpddbus,vpbug} --spawn=… --headless` (post-F06-B.2) | Banger re-takes on named-material streets (now carrying scaled authored `elasticity` restitution): sf `--spawn=-141.9,1.5,-608.5,115` → `bng_ev=0a/2s/1b` (was `0a/5s/2b`); london ring `--spawn=802,6,-905,180` → `bng_ev=3a/3s/0b` (was `3a/0s` — same activations, now settling). `docs/research/banger.md` annotated. |
|| `mm2-inspect placement <retail>` (post-F03-C.1) | exit 0 — all three channels audited independently against authored PSDL carriageways (london 3224 / sf 2871 regions). london: inst 1997 stamps/15 in-road, pathset 87 paths→1188 stamps/185, prop-rule 415 rooms→5118 stamps/209; sf: inst 3763/6, pathset 144→925/34 (+31 skipped decal paths), prop-rule 397→5028/0. 449 findings classify as authored intent (banner pivots mid-span, pedestrianised-street/plaza dressing, inst facades over/under roads); no systematic lateral defect in any channel. `--strict` exits 2 on the 67 pre-existing prop-rule walk issues, not on in-road counts. |

## Operator report (2026-09-20, human play-test — PRIORITY)

Source: the repository owner drove retail London and SF in a windowed
build at `8907c54` (`--city london --spawn 0.4,5.5,-720,0`, `--city sf`,
`--event circuit:7`). This is direct observation of rendered gameplay,
not a synthetic test, and it outranks the inferred explanations recorded
below.

Reported, verbatim: "all of the stuff is spawned in at the wrong height",
and a sawhorse barricade struck at over 100 km/h "didn't move it (maybe
because half of it is stuck inside the ground?)".

1. **Stamped props sat at the wrong height — root cause found and
   fixed (F03-B.5).** All three channels checked. Diagnosis (measured
   on retail `dgBangerData` + PKG geometry, not inferred): prop meshes
   are authored centred at the bound centre, `Size` is the bound's
   *full* extents, `CG` is the bound centre, and the authored stamp
   point is where the bound's *base* rests. `city.rs` previously
   stamped the mesh *centre* at the point — every prop sank by ~half
   its height, exactly matching the report. Fix: `PropOffset` —
   `+CG` for bound stamps, `−min_y` lift for unbound stamps, verbatim
   for INST — baked into render + collision vertices and BREAK
   fragments, keyed per offset class in `PropCache`. Verified on
   retail London + SF screenshots and unchanged stamp counts;
   regression test asserts collider AABBs through `load_city` + Avian.

2. **The two recorded hypotheses are superseded by the diagnosis.**
   The sawhorse that "didn't move" was sunk ~0.73 m — its bound was
   already below the road surface, so it read as immovable. The
   sink explains the same symptoms as the inferred slope-tumble and
   hull-clearance mechanisms. F04-C.3's strike-bound evidence was
   measured against *sunk* props and has been re-taken on corrected
   geometry — see item 3.

3. **F04-C.2's retail evidence re-taken (2026-09-20).** The
   flat-ground settle / repeated-hits / pool-bound runs were redone
   on the fixed placement — see `docs/research/banger.md`'s
   corrected-geometry re-take block. Findings: the original staging
   spawn now sits just under the authored limit (bus slides around
   the row, `0a/0s/0b` — corrected record, both threshold sides
   evidenced); restaged perpendicular approach → `2b` shatters, 5
   fragment `Slept` settles by 10 000 ticks; pool caps hold (2→2a,
   1→1a live); London reclaim ring bit-identical (`3a`, rec=1@2,
   rec=2@1); F04-C.3 strikes re-taken (`vpddbus` cones 3a/2s — taller
   bounds overlap more; `vpbus` 1a/1s unchanged; `vpbug` 1a/1s, was
   2a/2s on the sunk row). The operator's `sp_sawhrslt_f` roadblock
   (`checkpoint:7` overlay) now shatters at ~9–12 m/s under
   `vpddbus`. Whether a sub-limit hit should still tip a prop is
   UNK-22 — the authored `ImpulseLimit2` gate stands.

Code lead resolved: `stamp_line_strip` did place each prop at the
authored point verbatim — correct for the *transform*; the missing
piece was the content-space offset between the authored point and the
centred mesh, now the `PropOffset` contract.

Expected next iteration: return to the queued feature work (F13-A
remainder / F14-A / F11-C per the selection policy).

## Operator report 2 (2026-09-21, human play-test — PRIORITY)

Source: the repository owner drove retail London in a windowed build at
`561b8b7` (`--city london --spawn 0.4,5.5,-720,0`). Direct observation of
rendered gameplay; outranks inference.

1. **Height defect is CONFIRMED FIXED.** Operator: "All objects seems to
   be placed at correct height now". Phone boxes, litter bins, lamp posts
   and trees rest on the pavement, and `decals.pathset` ribbons render on
   the carriageway. `3ac8a5a` is validated by observation, not only by
   the re-taken `bng_ev` counters.

2. **Horizontal placement is still wrong — this is the new priority.**
   Operator: "things might be placed a bit wrong though, because the
   things in the screenshot are *in* the road. And where I spawned there
   is some trees in the middle of the road." Roadside furniture is
   landing in the carriageway. Vertical placement being right makes this
   a separate defect in the XZ mapping, not a leftover of the height bug.

   Lead, not a diagnosis: every placement channel routes through
   `inst_transform`, which carries the `MIRROR_Z` convention. If prop
   placements are Z-mirrored relative to PSDL road geometry (or the
   mirror is applied at the wrong stage for pathset / prop-rule stamps
   but not INST), kerbside objects land across the road. Other
   candidates: a handedness mismatch between authored pathset space and
   room space, or a per-room origin not being applied. Establish the
   truth against authored data and retail screenshots; do not apply a
   lateral fudge that merely looks better, and check all three channels
   (INST, `*.pathset`, PSDL `prop_rule`) independently — they may not
   share the defect.

   **Iteration 21 audit (F03-C.1).** The three channels were audited
   independently against the authored PSDL carriageway by the new
   `mm2-inspect placement <retail>` command (see its task row and the
   evidence table). Result: **no systematic lateral defect exists in
   any channel** — `MIRROR_Z` is off and stamp positions are the
   authored ones. The props the report saw *in* the road are authored
   there: the spawn area (`y≈4.85`) is a pedestrianised street encoded
   as `RoadNoSidewalks`, and its trees/bollards/crates stand on that
   walkable surface by design; `cp_banr*` banner rows pivot at road
   surface mid-span (mesh overhead); INST hits are verbatim-authored
   facades/bridges on drivable surfaces. What remains genuinely
   unproven: the pathset *expansion* positions along a segment are an
   inferred policy (UNK-20) — stamps could be denser/sparser than
   retail along the same authored path — and asymmetric-prop yaw is
   not frame-compared. A small flagged set (2 `sp_tree1_s` at
   divided-road median tapers, 2 `sp_stackboxes` in a London road
   band, 5 crosswalk hits) is listed by the audit for visual review.

   Until this is settled, the same caution as report 1 applies: do not
   roll `F03` or `F04` up to `checked` on placement evidence, and treat
   strike/settle evidence as provisional, since what a vehicle can reach
   depends on where props actually sit.

3. **Known long-standing vehicle texture defect — RESOLVED (iteration
   56, F02-C.4).** Root cause was not parsing or decode: `vpbug.pkg`
   authors the body in two halves — BODY_H sections 0/1/3 carry the left
   half on `vpbugyellow_{sd,ft,bk}` while sections 4/5/6 carry the whole
   right half (`x>0`) bound to `vpbugyellow_{sd,ft,bk}_dmg`, the
   damage-state texture variants. We bound the `_dmg` textures verbatim,
   so every clean car rendered its damage skin: the "garbage" right
   windscreen was the authored cracked-glass damage texture and the
   "smeared" quarter panel the dented variant, with the seam on the
   authored centre split. MM2Hook's recovered `fxTexelDamage::Init`
   (the `vehCarDamage::TextelDamageRadius` consumer) documents the
   retail rule — a shader naming `<stem>_dmg` is the damaged variant;
   the undamaged car binds the clean stem, the `_dmg` original is
   stashed for per-impact texel blits, and a failed clean lookup keeps
   `_dmg`. `MaterialCache::shader_material` now applies that pairing
   (vehicle models only — prop PKGs bind verbatim, `fxTexelDamage` is a
   `vehCarModel` mechanism). Fleet-wide defect: 22 of 25 parseable
   retail `vp*.pkg` ship `_dmg`-bound BODY sections (vpbus 12 sections;
   vpcop/vpmoonrover/vpmustang99 none). Verified by identical-pose
   captures: vpbug paint 0/1 and vpbus render clean. Ledger DMG-9;
   `docs/research/pkg.md` records the convention. The `_dmg`→clean
   pairing is also the binding half of `TextelDamageRadius`'s consumer —
   the impact-blit leg stays unrecovered (F05-B).

## Operator report 3 (2026-09-21, human play-test — PRIORITY)

Source: the repository owner driving retail London in a windowed build
at `563c34e`. Direct observation of rendered gameplay.

1. **Every stamped prop is rotated 90 degrees clockwise.** Operator:
   "All the props are loaded in 90 clockwise. e.g. the lampposts doesn't
   point out over the road, and bus benches stick out into the road."
   Lamp arms should reach out over the carriageway and benches should
   sit along the kerb facing it; both are turned a quarter-circle.
   **Addressed in F03-C.2** — the defect was the prop-rule channel's
   synthesized facing, not a universal basis error; see the task row.

2. **`ea1a9ef` (F03-C.1) does NOT rule this out.** That audit classifies
   each stamped *position* against carriageway triangles. It measures
   neither orientation nor the prop's footprint, so a bench whose origin
   sits correctly on the kerb while its body lies across the road passes
   it cleanly. Its conclusion "no systematic lateral defect in any
   channel" is therefore not evidence against this defect, and the 449
   in-road stamps it dismissed as authored intent should be re-read once
   orientation is fixed. Extend `mm2-inspect placement` to check
   orientation and swept footprint, not just the origin point — without
   that, no automated gate here can see what the operator sees.
   **Done in F03-C.2**: the audit now sweeps each stamp's rendered
   best-LOD geometry through the same basis the runtime builds and
   reports street-level body hits and overhead overhangs separately.

3. Code lead, not a diagnosis: `yawed_transform` (city.rs) assigns the
   path tangent `d` to `x_axis` and `(-d.z, 0, d.x)` — `d` rotated +90
   degrees about Y — to `z_axis`. If authored prop geometry faces along
   +Z rather than +X, every derived-basis stamp is rotated by exactly a
   quarter-turn, which is the reported symptom. Both callers are the
   `props.pathset` stamp path and the prop-rule stamp path.

   Discriminator worth running first: INST placements carry their own
   authored full basis and never call `yawed_transform`. If INST-placed
   lamps and benches are correctly oriented while pathset and prop-rule
   ones are not, the defect is in this basis and not in a mesh
   convention. If INST props are rotated too, it is not this function.

   Establish the authored convention from retail data before changing
   it. Do not apply a blanket 90-degree correction because the result
   looks better; a compensating rotation in the wrong place will hide
   the real convention error and break mods later.

   **Outcome (measured, F03-C.2):** the basis was *not* the defect.
   Pathset direction→+X is verified correct on retail (85/85 sf
   `sp_lightstreet_rt_f` lamp arms over carriageways; barricade walls
   continuous). Prop-rule props are instead authored front-first along
   local **−X**, and the walk was handing each stamp the *walk*
   direction — a quarter-turn error for every kerb prop. The walk now
   measures the kerb→building-line direction per stamp. No blanket
   rotation was applied.

4. **Vehicle handling is operator-owned — do not tune it.** The operator
   judges current steering and handling to be poor, and intends to tune
   it personally by feel rather than have it adjusted autonomously.
   Continue structural and contract work on vehicles, but do not change
   handling parameters or tuning curves for feel. Relatedly: the
   frequent `opponent re-anchored onto its route after a bounded stuck`
   events observed on `circuit:0` (Minis, both rosters, roughly one per
   five seconds) are understood to be downstream of vehicle handling,
   not a routing or placement defect. Do not treat them as an opponent
   AI bug, and do not widen the re-anchor recovery to mask them further.

## Operator report 4 (2026-09-22, human play-test — PRIORITY)

Source: the repository owner driving retail London in a windowed build
on an M5 Max, at `bcaacae` and `c66922e`. Direct observation of
rendered gameplay. **These observations outrank the loop's own
evidence.** Every defect below currently reports `pass` in the smoke
counters — counters cannot see geometry, motion continuity, or
collision response.

Suggested order of work: the items that invalidate already-recorded
acceptance evidence first — **4, then 2, then 1** — then investigate
**3**, then **5**.

**SCOPE NOTE — read before selecting any task below.** Operator report 3
item 4 marked vehicle handling as operator-owned and off-limits. That
still holds for steering, grip and driving feel. It does **not** cover
item 2 below. Prop collision response is driven by authored
`dgBangerData` (`Mass`, `Elasticity`, `ImpulseLimit2`) and is a
data-fidelity question, not a feel question. **Do not refuse item 2
under the handling rule.**

1. **Ambient traffic teleports across intersections.** Operator: "There
   are cars roaming around, but when they get to intersections they
   teleport over them to the next road." Cars jump the junction rather
   than driving through its interior. Everything else about ambient
   traffic looks right in play: London free roam reports `density=0.5
   target=16 spawned=14`, 0 stuck, `signals=828`.

   Lead, not a diagnosis: the lane follower may reposition to the next
   lane's entry instead of traversing the junction interior. Check the
   lane transfer in F10-B.2 (authored junction rules), F10-B.5
   (junction-box yield), and whether F10-B.6's kinematic-to-dynamic
   handover skips junction interiors.

   This invalidates the F10 acceptance evidence gathered from counters
   alone. Spawn, recycle and stuck counts structurally cannot see a
   teleport: a car that jumps a junction still spawns, still moves its
   displacement window, and never registers as stuck. Re-take that
   evidence with a measure that can see position continuity across a
   transfer, not just aggregate motion.

   **Iteration 49 fix (F10-B.9) — implemented, candidate.** The lead
   was the mechanism: `advance_lane_cursor` consumed the remaining
   approach-lane distance and set `cursor.along = 0` on the exit lane,
   so `sample_lane` placed the car on the far side of the junction —
   the interior was skipped entirely. BAI carries no authored
   junction-interior geometry, so `NavGraph::crossing_path` generates
   a cubic Hermite (endpoint poses sampled in travel direction,
   chord-scaled tangents, resampled polyline) and `LaneCursor`
   traverses it incrementally while keeping the approach lane
   identity — occupancy and right-of-way survive the crossing, and
   the car lands on the rank-preserved exit lane only when the path
   is exhausted. A new pose-continuity watchdog counts teleport-like
   per-update moves (`jmp=` smoke field) beside the commit count
   (`crx=`). The junction-gap census (`mm2-inspect nav --gaps`) built
   for the evidence also surfaced an authored-data quirk: several SF
   lane curves are vertex-reversed relative to section order
   (authoring artefact, not a direction marker — measured, see
   `docs/research/bai.md`), now normalized at `NavGraph::build`.
   Retail headless: sf `--frames 3000` → `traf=16/16 sp=43 rec=27
   dead=0 uns=0 q=0 jq=3 stuck=0 crx=59 jmp=0`, london → `…sp=22
   rec=6… jq=2 stuck=0 crx=68 jmp=0`. The generated geometry is an
   implementation choice — the original's runtime crossing path is
   unrecovered (UNK-12). See the F10-B.9 task row.

2. **Breakable collision response is wrong.** Operator: "Hitting
   breakables does result in extreme bounce of my car though, especially
   at high speed. In the original game, if you had enough speed you
   would just plow right thru things ... the small parking meters takes
   away all my speed and leaves me flying in another direction."

   Expected behaviour: momentum carries through a light prop. A parking
   meter should cost a little speed and barely deflect the car. Today it
   behaves like hitting a wall, and more speed does not help.

   Lead, not a diagnosis: `break_banger` fires on a qualifying collision
   edge, so the solver may resolve the car against a still-static prop
   collider in the same step and only then replace it with fragments —
   the car takes the full rebound of an immovable object regardless of
   the prop's authored `Mass`. That single mechanism would explain the
   bounce, the total speed loss and the absence of plow-through
   together. Check how authored `Mass`, `Elasticity` and
   `ImpulseLimit2` reach the runtime body, and whether restitution is
   taken from `Elasticity` correctly.

   Establish the authored model against retail data first. **Do NOT
   simply lower restitution or scale impulses until it feels better** —
   a compensating fudge will hide the real data path and break mods
   later. This is the same failure mode operator report 3 warned about
   for prop rotation.

   This invalidates F04 and F05 acceptance evidence.

   **Iteration 48 fix (F04-C.4) — implemented, candidate.** The report's
   lead was the mechanism: `activate_bangers` runs after Avian's solver,
   so a dormant prop answered its first contact as an infinite-mass
   static body and the striker took the full wall response regardless
   of authored `Mass`. The activation now replays the hit as a
   reduced-mass two-body transfer — `ContactDetails` reports the solver's
   applied normal impulse and combined restitution, the applied impulse
   is returned and `(1+e)·v·μ` paid instead (along the applied normal,
   so glancing hits keep their deflection), and the prop launches at
   `impulse / prop_mass`. Bound-only strikes pay the same share with no
   wall impulse to return; coincident contact+overlap folds once.
   Authored `Mass`/`Elasticity`/`ImpulseLimit2` data paths untouched, no
   feel constants. Retail before/after on identical commands: london
   `vpbug --spawn=0.4,5.5,-720,0` `moved=55m→124m, bng_ev=2a→1a`; sf
   `vpddbus --spawn=-1641.6,36.7,410,0` `moved=46m→99m, bng_ev=2a→3a`.
   See the F04-C.4 task row; UNK-22 keeps the original exchange quantity
   open.

3. **Trees and large light poles cannot be broken at all.** Operator:
   "I'm not able to break trees and larger light poles at all in this
   version."

   This one is a **research question first, not a defect report.** Note
   that F04-A.2 recorded INST props as 0/221 bound in London and 0/165
   in SF — the static channel binds no banger records — so these may be
   unbreakable by authored data rather than by defect. Determine
   whether the original game breaks them. If it does, the INST-to-banger
   binding is incomplete and that is the bug. If it does not, record it
   as correct fidelity and close it. **Do not guess**, and do not make
   them breakable to match an expectation that retail may not share.

   **Iteration 50 fix (F04-C.5) — implemented, candidate.** The binding
   hypothesis was tested first and ruled out: INST carries zero
   tree/pole names in either city, but trees and light poles reach the
   world through prop-rule stamping and `props.pathset` and already
   spawn as dormant bangers — `sp_tree1_s` is authored breakable
   (NumParts=5, BREAK01–05 chunks, fragment records). The defect was
   the gate: `impulse_estimate` compared `approach_speed ×
   striker_mass` against `ImpulseLimit2`, so a 4 250 kg `vpbug` needed
   ~470 m/s for a tree and ~120 m/s for a streetlight — unreachable,
   matching the report exactly. A census of every placed bound record
   shows `ImpulseLimit2 ≈ Mass × {31.25, 500, 800, 2000, 85342}` — a
   ladder that is quadratic in striker speed: read as striker kinetic
   energy `½·m·v²`, meters/cones fall at walking speed, benches and
   dumpsters at ~5–9 m/s, poles and trees at ~11–33 m/s, gantries
   higher, and the authored-immovable outliers (`1e30` bridges,
   `sp_lightthames_l`) stay unreachable. Activation now feeds
   `impact_energy` to `activates_on` at both the contact and
   `StrikeBound` gates; traffic knock and breakaway keep their own
   linear readings. Classified designed under UNK-22 — the original's
   comparison quantity is unrecovered; the reading is justified by the
   authored-limit census, not claimed as original. Retail evidence
   (install `fnv1a64:e91e6cd4b2ae30d9`): SF headless cruise produced
   `banger shattered name=sp_lightstreet_rt_f severity=37.5
   estimate=702303` — a 10 t freeway streetlight pole (NumParts=2) the
   linear reading priced at ~75 m/s; a London junction run activated
   `sp_can_royal_l` at 13.4 m/s. A named `sp_tree1_s` shatter was
   attempted but not staged — junction-fan trees sit behind kerb
   clutter and ~25 m median spacing stops a straight-line `hold`
   driver below the ~22–31 m/s the tree class needs; the unit
   regression covers the threshold math both ways. See the F04-C.5
   task row.

4. **Sidewalk props are sunk by roughly the kerb height.** Operator:
   "things on the sidewalk doesn't spawn on the correct z height? The
   garbagebox extends under the sidewalk a bit?" Screenshot evidence on
   the Mac Studio:
   `screenshots/1790089019040_cam_-275.6,2.0,160.6,38,-13.png` — a
   London LITTER bin on a raised pavement, buried to roughly the level
   of its "LITTER" text, with a thin spike of geometry protruding below
   over the kerb face.

   **This is NOT the defect `3ac8a5a` fixed.** That one was the content
   offset (`Size` read as half-extents when retail authors full
   extents) and is genuinely fixed elsewhere in the scene. This is the
   **datum**: F03-B.6 made prop-rule stamps walk the authored kerb
   chain, resolving (kerb, outer) vertex pairs from the room's road
   attributes, and the kerb vertex sits at road level, at the foot of
   the kerb face. A prop standing on a raised pavement needs the
   pavement top, one kerb-height higher.

   Testable without guessing: compare each stamp's Y against the
   outer/building-line chain rather than the kerb chain, and check
   whether the discrepancy equals the authored kerb height in the PSDL
   road attributes. If it does, the datum is the defect and the
   correction is exact rather than tuned.

   `mm2-inspect placement` structurally cannot see this today: F03-C.2
   extended it to sweep footprints, but horizontally. Extend it to
   report **vertical penetration into the walkable surface** so this is
   machine-visible.

   This invalidates the F03-C.1/C.2 placement evidence, which measured
   horizontal containment only.

   **Iteration 47 fix (F03-C.3) — implemented, candidate.** The datum
   hypothesis was confirmed on authored data: the `Room_attributes`
   format doc states the road vertices sit "0.15 units below the
   sidewalk vertices", and retail measurement agrees (`sw.y − road.y
   == 0.15` on 943/1087 london + 1205/1321 sf `RoadWithSidewalks`
   sections — the rest are authored flush ramps — and `top.y −
   ground.y == 0.15` on all 11 545 `SidewalkStrip` pairs). The walk
   interpolated straight off the kerb foot; it now lifts kerb chains
   by the shared `mm2_game::SIDEWALK_KERB_LIFT` (the same constant
   `mm2_app::city::emit_sidewalk` raises them with) before lerping —
   `RoadWithSidewalks`, `DividedRoad` and `SidewalkStrip` lifted,
   `RoadNoSidewalks` walkways untouched (their authored edge is the
   surface). The audit gained the vertical leg it asked for:
   `walkable_surfaces` classifies every region into `Driving` /
   `SidewalkTop` / `FloorFan` (mostly-horizontal `Fan`s only — the
   renderer's own `|n.y| ≥ 0.3` test), and the placement audit reports
   a `sunk` count plus a channel×band histogram per city. Retail
   London post-fix: prop-rule `sunk 0/5118` (was the defect class);
   before/after captures `/tmp/kerb-{before,after}.png` at the
   operator's `cam -275.6,2.0,160.6,38,-13` show the LITTER bin
   resting on the pavement top instead of buried to its label.
   Remaining sunk findings are authored placements outside the
   prop-rule channel (pathset 54, inst 94 — plaza-floor and underpass
   placements, triage data, not the kerb defect). See the F03-C.3
   task row.

5. **Nuisance: startup WARN spam.** 154 `road N: no routable vehicle
   lanes` WARN lines on every launch, one per road, in both free roam
   and races. The count is already reported in the ambient summary as
   `issues=154`. Make it a single DEBUG summary line. The spam currently
   buries genuine warnings such as the "TEX declares more mip levels
   than its size allows" one.

   **Iteration 52 fix (F10-B.10) — implemented, candidate.** The spam
   was two per-entry WARN classes, both expected authored-data
   anomalies already counted elsewhere: `NavIssue::NoVehicleLanes`
   (154 london / 22 sf roads — pedestrian/special/disabled roads,
   WLD-11) warned once per road in `load_ambient_traffic`, and
   `walk_prop_rules`'s per-entry strings (64 on sf — the 0xx-encoded
   `road_rooms` records already counted as `bad_refs`; london authors
   none) warned once per entry in `load_city`. `partition_nav_issues`
   now routes the bulk class to a single DEBUG line carrying the count
   and the road list while every other plan/nav issue kind keeps its
   individual WARN; the walk strings collapse into one DEBUG line with
   the count and the full (bounded) list. Nothing is lost: `issues=`
   in the ambient summary and `proprule_issues`/`bad_refs`/`unreached`
   in the stamped summary still count them, and `mm2-inspect nav` /
   `mm2-inspect placement` still list every entry. Retail london
   startup: 155 WARN lines → 1 — the genuine `p_parkmeter_f.tex` mip
   warning the report cited as buried. +2 `mm2_app` unit tests. See
   the F10-B.10 task row.

**Confirmed working in this play-test — do not regress.** Prop height
(content offset) and prop rotation both look correct in play; impact
sparks look right; pushing another car around works; ambient traffic
spawns, recycles and reports 0 stuck; traffic signals place at 828 BAI
light origins; races run with 7 opponents plus the player and a working
place indicator.

**Still open and unchanged.** Vehicle handling is poor and remains
operator-owned. The frequent `opponent re-anchored ... bounded stuck`
events on `circuit:0` (Minis, both rosters, ~12 in 25s, no better since
F15-B.4's catch-up assist) are downstream of handling — not an
opponent-AI or routing defect. Do not widen the re-anchor recovery to
mask them.

## Task table

| Task | Status | Dependencies | Evidence / reason / next action |
|---|---|---|---|
| F00-A | checked | - | Audit done in this planning pass: gates pass, env/install/toolchain recorded above. Externally checked. |
| F00-B | checked | F00-A | Split into F00-B.1 (inventory) and F00-B.2 (rules ledger); both children externally checked. |
| F00-B.1 | checked | F00-A | `mm2-inspect inventory` landed: versioned report (engine commit + fnv1a64 catalog fingerprint) with expected/discovered/accepted/rejected/unverified counts across 12 families; `--strict` exits 2 with 33 findings on retail (8 incomplete vehicles, 2 partial circuit11, junk/partial records). Externally checked. |
| F00-B.2 | checked | F00-A | `docs/original-rules.md` ledger landed: ~90 classified facts from MM2HELP.HLP (decompiled locally via helpdeco), Readme.rtf, Booklet.pdf and authored data. `mm2_formats::racedata` parses the `mm*data.csv` event tables; inventory cross-checks them (12/10/10/13 rows/city, missing/malformed → rejected). Externally checked. |
| F00-C | checked | F00-B | `mm2 --headless` headless physics smoke (no window/GPU; dev world + real cities via VFS), windowed `--frames`/`--screenshot` visual smoke that awaits the capture file, unified `smoke=` records versioned by build-embedded commit, statuses pass/fail/unavailable → exits 0/3/4. `--city` without data sources is `unavailable`; a requested missing city/vehicle is an explicit failure. Externally checked (review pass 2026-09-20; AC04/AC05 verified by reviewer). |
| F00-C.1 | implemented | F00-C | Smoke-record restart repair + `--restart` evidence knob. `headless_smoke` cached the `PlayerVehicle` entity once, so a mid-run session restart (a `RestartEvent` disabled outcome, a results-row/Backspace restart) left it reading a despawned entity — `Position` lookups returned `None`, formatted `final=(NaN,NaN,NaN)` and failed `status=fail "non-finite pose"` on a legitimate lifecycle event (external review flagged exactly this on london `checkpoint:0 --bot`, where the scripted driver disabled mid-race and the event restarted per RACE-5/DMG-2). The runner now re-resolves the live player every frame (Hold input + telemetry sampling follow the respawn), reads the end-of-run pose/participant/progress/gyro fields from the live entity, reports `final=none`/`moved=none` only while the cap lands inside the teardown window (entity absent = lifecycle, not a missing pose — an entity present without finite components still fails), and counts teardown/begin cycles as `rs=<n>` (session generation delta, emitted on activity only so restart-free records stay bit-identical). New quarantined `DevOverrides::restart` (`--restart`) queues the session's own restart intent once on the first `Playing` frame — same production `Unloading → Menu → begin` path, `record_eligibility`-gated (`DevOverride("restart")`) — giving evidence runs a reproducible restart trigger. Tests: +1 mm2_app (dev-world `headless_smoke` under `dev.restart` — pass, `rs=1`, no NaN, `phase=playing` at cap) +1 eligibility leg. Retail (`fnv1a64:e91e6cd4b2ae30d9`): london `checkpoint:0 --bot` 1500f now `status=pass rs=1 … final=(-448,-0.2,-187) race=Countdown{36} pos=5/5` (was `fail "non-finite pose"`); 3000f → `rs=2 phase=playing ticks=288 cu=3` — the bot disables repeatedly and each restart plays out; sf/london cruises bit-identical (no `rs=`). Candidate pending external check. |
| F00-C.2 | implemented | F00-C | Smoke below-world repair: the end-of-run verdict compared the player's final y to `spawn_pos.y - 25`, so any session legitimately descending >25 m below its spawn read `fell through the world` — caught on retail `sf circuit:0 --bot`, whose authored course bottoms at y=15.85 under a ~43.5 start grid (the car was racing normally at `cp=2/9`, `wheels=4/4`). `mm2_app::city` gains `WorldFloor` — the session-scoped authored floor, `Psdl::bounds_min.y` when finite (absent on dev worlds and non-finite bounds) — carried on `LoadedCity`, inserted by `load_session_world`, removed by `despawn_session_entities`. `headless_smoke` fails below `floor - 25` when bound, spawn-relative otherwise; sf's authored −2.15 gives a −27.15 line. Implementation choice — an evidence-runner threshold, not a claimed original rule. Tests: +2 mm2_app (`descending_below_spawn_is_not_a_world_fall` — synthetic city, road at y=0, authored bounds to −60, dev-spawn at y=40: the car drops, grounds, drives off the edge into a recovery loop and ends `final=(-0,-0.1,-2)` ~40 m under spawn — pass where the spawn-relative check failed; `world_floor_does_not_leak_across_restart` — a planted `WorldFloor` does not survive teardown into a dev-world session). Retail (`fnv1a64:e91e6cd4b2ae30d9`): `sf circuit:0 --bot --headless --frames 6000` → `status=pass` (was `fail "fell through the world"`) at the same endpoint `final=(-2107,17.1,-52)` — 44 m above the −27.15 line. Honest remainder, now visible instead of masked: the scripted player still stalls at `cp=2/9 lap=1/3 pos=4/5` after 7 fall/recoveries — it left the elevated road between gates 1→2 (gate 2 at y=37.8, car ends at y=17.1, ~155 m past it in x) — a route-following limit under F14-B/F15-B; `opp=0/4 opp_rec=5`. Candidate pending external check. |
| F01-A | checked | F00-A | `SessionConfig` (world/mode/event-ref/difficulty/conditions/densities/seed/vehicle/authority + quarantined `DevOverrides`) and `Session` (`Menu→Loading→Ready→Countdown→Playing→Paused/Results→Unloading→Menu`, `Failed` load-time only, MP-6 pause rule enforced from authority) landed in `mm2_game`; `SessionEntity(gen)` ownership markers thread through world/vehicle/HUD/camera spawns; `advance_session_tick` fixed clock; `WorldState`/`ActiveWorld`/`CamStart` replaced. 13 session tests; smoke records now carry `ticks=`. Externally checked (review pass 2026-09-20). |
| F01-B | checked | F01-A | `mm2_game` contract modules landed: `ids` (`PlayerId`, generation-scoped `ObjectId`/`ObjectIdentity`, `AuthorityRole`, `Player`/`PlayerControl`), `surface` (`SurfaceMaterial` carrying authored codes uninterpreted + `SurfaceState` keeping physical vs visual identity separate), `impact` (`ImpactEvent`/`ImpactPolicy`/`ImpactDedup`/`ImpactId`), `telemetry` (`VehicleTelemetry`/`WheelTelemetry`/`DamageSignals`), `result` (`SessionResult`/`ResultId`/`ResultLedger` dedup). `Session` mints generation-scoped ids. `mm2_vehicle`: `VehicleInput.forced_gear`, `VehicleState.engine_load`, `WheelState.contact_entity`, `CollisionEventsEnabled` in `vehicle_bundle`. `mm2_app::contracts`: `collect_impacts` (Avian `CollisionStart`+`Collisions` → bounded deduplicated events, stable-id participants, damage accumulation) + `publish_vehicle_telemetry` (read-only snapshot per fixed step, Playing only); HUD reads the snapshot; smoke records `impacts=`/`dropped=`. 9 game + 4 app real-physics + 2 vehicle contract tests. First candidate failed review on a real defect — `dedup.allow` ran before the severity/manifold filters so a sub-threshold touch consumed the pair cooldown and silently suppressed a re-impact; fixed (dedup now applies only to reportable contacts) with regression test `a_quiet_touch_does_not_suppress_a_real_reimpact`, which fails on the old ordering and passes on the fix. Trailer now carries `DamageSignals`; `engine_load`/edge-trigger docs corrected. Externally checked (review pass at iteration 006/007, commit `d0cca68`). |
| F01-C | implemented | F01-B | `mm2_app::session` module drives the lifecycle in the real app: `load_session_world` (was `Startup` `setup`, moved to lib + gated on `Loading`), `session_control_input` (`Esc` quit→teardown→exit, `Backspace` restart — no menus exist yet, F17), `drive_session` (`Unloading` waits for observable despawn → clears `ImpactFilter` (new `reset()`: dedup map is `Entity`-keyed and the next session may recycle entities; ids/evidence counters restart per session) + `SpawnPoint.trailers` → `Menu` → `begin` or `AppExit`), chained after `despawn_session_entities`. `docs/architecture.md` gained the ownership/scheduling/plugin-attach section (AC06); README controls updated. 5 new integration tests run the production systems headlessly: restart leaves exactly one session (AC01), quit→Menu→AppExit, failed load keeps no player sim + retries/quits (AC02), impact ids/counters restart per session, fixed-tick driving is update-rate independent (AC03). Windowed `--frames 60` dev-world smoke and retail `--city sf --headless` smoke both pass unchanged. Candidate pending external check. |
| F02-A | implemented | F00-B, F01-B | `VehicleCatalog::scan` + `EXPECTED_STOCK_ROSTER` + `stock_audit_failures` + `mm2-inspect cars/validate-cars` exist and ran on retail install tonight (21/21 ready, 8 audited extras). Candidate pending external check; deps not yet checked. |
| F02-B | implemented | F02-A | `mm2_content::convert`, handling measurement (`analysis.rs`, drive probe), recent steering/gearbox/levelling fixes. Candidate; gap list still owed by F02-A audit. |
| F02-C | active | F02-B | Split: C.1 (roster coverage matrix + the reverse-band repair it caught — implemented below), C.2 (probe legs + the two trailer fixes they caught — implemented below), C.3 (`vpmoonrover` launch finding resolved — four-`vehWheel` architecture + back-back followers — implemented below). Remaining: per-car render leg (GPU captures), per-car override matrix (causality measured on vpbug only). |
| F02-C.1 | implemented | F02-B | Stock-roster coverage matrix + reverse-band repair. `drive_probe --controls`: launch → brake → held-brake reverse → `ResetVehicle` → finiteness through the production sim, per ready car. Reverse repair: no gear selection while reversing, RPM through `reverse_ratio`, band-top limiter at `upshift_rpm` — vpbug was -45.6 m/s, now -13.6 at its authored ~13.4; regression test `reverse_speed_stays_bounded_at_the_reverse_gear` (+1 mm2_vehicle). Retail (`fnv1a64:e91e6cd4b2ae30d9`, 2026-09-22): 29 ids discovered, 21/21 expected stock ready+validated (2 warnings: vpcentury hitch fallbacks, vpford 4/5 paints), 8 unlisted extras fail with reasons (denominator intact), `handling --strict` exit 0, 21/21 app-path `status=pass` `wheels=N/N`, 20/21 controls ok — `vpmoonrover` FAIL(drive) recorded open. `docs/vehicle-coverage.md` publishes the matrix + open legs; `docs/vehicle-handling.md` gains the reverse paragraph. Candidate pending external check. |
| F02-C.2 | implemented | F02-C.1 | Probe legs + the trailer repairs they caught. `drive_probe` gains `--drop` (per-car landing: released airborne, grounded/upright/drive-away/finite), `--city <c> --clearance` (roster on real city colliders: every wheel grounded + zero hull contacts at spawn and after `ResetVehicle`, trailers spawned through production `spawn_trailer` reported separately with hitch-anchor gap), `--config`/`--dump-config` (TOML override through the same `apply_handling_override` as `--vehicle-config`, wheel-count mismatch rejected). Clearance caught two real defects, both fixed: the `vpsemi` tractor/trailer hulls held a standing contact at the coincident hitch anchors (hitch joint now carries `JointCollisionDisabled`; regression test `tests/trailer.rs`), and `vpcentury`'s trailer counted two 5 cm `TWHL` detail parts as wheels (`WheelVisual.simulated` flag in `build_model`; `load_trailer` filters them, visual mounts stay parked; warnings surface through `def.report`). Moon rover needed an adaptive settle (the probe's ~1.5 m spawn drop wallows >6 s before grounding clean). Retail (same fingerprint): `--drop` 21/21 ok (air 0.88–0.92 s, up 1.00, drive-away, finite); `--clearance` 21/21 ok on sf AND london (trailers 4/4 wheels, hitch gap 0.00 spawn+reset); override causality on vpbug — half power 5.4→5.8 s/59.1→47.0 m/s, half longitudinal grip 5.4→9.4 s accel + stop 8.2→14.5 m, 2× mass 5.4→23.6 s, wheel-count mismatch rejected. `apply_handling_override` gains 2 unit tests (rig re-pin, wheel-count rejection). `docs/vehicle-coverage.md` updated. Candidate pending external check. |
| F02-C.3 | implemented | F02-C.2 | `vpmoonrover` launch finding resolved as a conversion defect, not a physics one. The retail `vehCarSim` carries exactly four `vehWheel` slots (front L/R, back L/R — mm2hook `vehCarSim::Init`: `whl4`/`whl5` pivots are only recorded as `BackBack*WheelPosDiff` vs `GetWheel(2/3)` centres, and `vehCarModel::Draw` renders them as the reference wheel's matrix + offset). Our 6-corner rig was statically indeterminate — porpoising, 131° drift, ~6 m/s. `WheelVisual` gains `follows: Option<usize>`; `build_model` flags `whl` index ≥ 4 as `!simulated` followers of `whl(N−2)` with a load warning; `assemble` emits only simulated wheels into `WheelGeom`/`WheelConfig` (trailer path already filtered); `spawn_vehicle_model` mounts a follower on the reference wheel's physics index with the authored car-space offset (`WheelMount.follow_offset`), so it copies live droop/steer/spin — `usize::MAX` parked mounts unchanged. `drive_probe` gains `--trace` (4 Hz telemetry: speed, vy, height, pitch, gear, rpm, up-axis, heading, angular velocity, per-wheel grounded/traction/suspension). Retail (`fnv1a64:e91e6cd4b2ae30d9`): moonrover now rides its authored statics — `CenterOfGravity` +0.4 m rearward puts CoM behind the physics rear axle → rests nose-up on its tail (clearance FAIL on sf/london is the retail rig's equilibrium, documented), `--drop` ok, `--controls` still FAIL(drive) at ~1 m/s launch (cut test vehicle; community docs report the original misbehaves — "rear end levitates" matches follower wheels floating at the pitched-down tail); `vpcentury` (the other 6-wheel model) passes drop/controls/clearance on both cities — launch wheelie transient (0-100 4.7→6.9 s), then straight at 54 m/s; `validate-cars` 21/21; roster accel otherwise unchanged. Regression test `wheels_beyond_four_are_back_back_followers`; `docs/vehicle-handling.md` gains the four-wheel convention; `docs/vehicle-coverage.md` updated. Pre-existing note: `vppanozgt` shows ~160° heading drift at top speed in the accel probe — committed behaviour (authored `.vehgyro` drift relief), unaffected by this change (its rig is 4 wheels throughout). Candidate pending external check. |
| F03-A | implemented | F00-B, F01-A | INST + PSDL placement parsed. Split: A.1 (`.pathset` parser + audit — implemented below), A.2 (prop-rule tables — implemented below). `.opp`/race `.csv` waypoints parsed under F11-A. Every discovered placement-source grammar now has a parser; remaining unmapped sources are feature-deferred: `.cpvs`/`.ldef` → F18, `audio_pathsets/` → F07/F08, PSDL `paths` records preserved raw (semantics unknown). |
| F03-A.1 | implemented | F00-B, F01-A | `mm2_formats::pathset`: PTH1 parser measured on all 101 retail files — named paths of attributed points, kinds 0 single/1 directed-pairs/2 line-strip, quarter-metre spacing; per-point attribute word + `current_path`/`selection` cursors preserved raw (inferred dev-tool state, UNK-20). `Pathset::validate()`: unknown kind, odd directed-pair count, non-finite point, out-of-range cursor, empty name — zero issues on retail (66 empty paths are authored data, not flagged). `mm2-inspect pathset <install> [--city] [--strict]`: denominator = every discovered `.pathset` (101 — no filtering): 98 parse, 3 authored truncations fail (`race/london/{blitz10,blitz11,london_bridge_blitz10}.pathset`); name cross-check resolves `geometry/<n>.pkg`/`texture/<n>.*` with `PREFIX:` state decorations stripped and `PATHnn` labels skipped — 120/126 names resolve, 6 dead refs all in `city/phys/` + `bak/` files; `--strict` exits 2. `scan` recognizes `.pathset`. `docs/research/pathset.md` + ledger WLD-12/UNK-20. Candidate pending external check. |
| F03-A.2 | implemented | F03-A.1 | `mm2_formats::proprules`: `PropDefs`/`PropRules`/`PropGroups`/`PropLodStats` parsers + `validate()` — `n{NN}left`/`n{NN}right` rules → `PropRule::rule_key()` byte/side split. `mm2-inspect proprules <install> [--city] [--strict]`: expected = `city/<stock>/{propdefs,proprules,props}.csv`, denominator = every discovered `city/**` prop-rule CSV incl `.csv.txt` exports + `geometry/props.csv` (different LOD schema). Cross-checks: rule refs → sibling defs, def file refs + group names → `geometry/<n>.pkg`, LOD rows → `geometry/<name>`, PSDL `prop_rule` bytes → defined rule numbers. Retail: 16/16 parse; byte↔rule link verified (WLD-14); 48 issues — 41 phys `*_m` + 3 phys group dead refs, `sp_bollard_pedsafe_l`, `va_garbagetruck.pkg`, rule 205 ×2; `--strict` exits 2. docs/research/proprules.md + WLD-14/UNK-21. Runtime stamping unwired — perimeter-walk semantics unverified (UNK-21). Candidate pending external check. |
| F03-B | implemented | F03-A | ~2000/3763 INST props instantiate with collision (README; `city.rs`) plus `props.pathset` ambient rows stamped through the shared `PropCache` — london 1188, sf 925 instances, decal paths classified. Stamping micro-semantics inferred (UNK-20); audio pathsets unconsumed. First candidate failed review on an unbounded line-strip expansion (huge/non-finite authored coordinates could stall `t += spacing` and OOM the load); repaired with the 8192/file stamp budget, arithmetic per-segment counting, non-finite skipping, load-time `validate()` and `pathset_props_capped`/`pathset_issues` report fields plus regression tests. Externally checked at `f6e151f`. |
| F03-B.2 | checked | F03-B | Event `.pathset` overlays (F03-AC04 leg): `event_race_setup` returns `EventSetup { definition, pathsets }` — the `<stem>.pathset` records the catalog attributes to the resolved event; `load_session_world` calls `city::spawn_event_pathsets` which runs the shared `stamp_pathset` classifier (`prop`/`PATHnn` label/`giz_*` animated/decal/`unresolved` + per-file 8192 budget + `validate()` issues) through a fresh `PropCache` into `event-pathset-*` session-owned entities. Teardown removes exactly the overlay; re-entry restamps once (AC04 — restart test asserts identical gen-2 count, no gen-1 survivors). Parse/read failures land in `EventPathsetReport::failed_files`, warned, non-fatal (optional record). Retail: london `circuit0` 181 props, `race6` 256, sf `circuit0` 85 — all `sp_*` names. `<object>_<event>` overrides (`london_bridge_circuit0`…) stay extras — all `giz_*`/`PATHnn`/`sp_pcar*` on retail, need animated-object/parked-car features. Tests: +3 in `tests/event.rs`. Externally checked at `afff57d`. |
| F03-B.3 | implemented | F03-B | Prop-rule stamping channel (WLD-17/UNK-21). `mm2_game::props::walk_prop_rules` — pure geometry walk: `RoomPath::road_rooms` chains, `start/end_crossroads` curb-pair junction runs, interior room-to-road boundaries found via the widest neighbour-marked perimeter pair, the two arcs between crossings walked as sidewalk building lines with side labels measured against travel direction (left/right of *travel*, not winding). Per-side `start`/`distance`/`maxUse` budgets and `(minLerp+maxLerp)/2` curb→outer placement are inferred policies; `file1–4` pick is a deterministic hash; output bounded at `MAX_PROP_RULE_STAMPS`, issues + stats counted. `load_city` spawns `PropStamp`s through the shared `PropCache` — bound names become dormant bangers via `spawn_banger_prop` (same path as pathset stamps), the rest `spawn_prop`; `CityReport` gains `proprule_*` counters + Display fields. Retail: sf 345 rooms/5002 stamps/5002 bangers, london 410/5083/5083 — 0 unresolved, 0 no_crossing; 109 bad `road_rooms` refs (65 0xx encoded values) + 52 sf / 5 london unreached rule rooms counted honestly. Screenshots: lamps line both sidewalks at authored ~29 m stagger, london phone booths/trees/bollards at curb edges, multi-room freeway parapet lamps — visually consistent. Unit tests (4, synthetic PSDL) + app test through `load_city`. TEX fix included: `decode_tex` clamps declared mip count to the size-supported max (`p_parkmeter_f.tex` 7 mips on 32×32 → hard wgpu error, now warned+clamped). UNK-21 narrowed to field semantics; geometry promoted to WLD-17. Candidate pending external check. |
| F03-B.4 | implemented | F03-B.3 | Decal pathset channel — the last ambient placement source. `mm2_app::decals::stamp_decals` consumes `<dir>/<stem>/decals.pathset`: measured geometry (WLD-18 — `LineStrip` points interleave two authored ribbon edges, even/odd pair = cross-section, consecutive sections join into quads; authored widths ~1 m zigzag → 20 m junction paint), `u` across the pair / `v` along the centre line tiled per `spacing` (inferred axis policy — texture contents agree: zigzag oscillates along V, rxwalk bars along U), palette alpha honoured via new `decode_rgba_honoring_alpha` (P8 decals carry authored translucency; ordinary `decode_rgba` unchanged), alpha-bearing textures `AlphaMode::Blend`, 2 cm lift + −1 depth bias, lit double-sided render-only entities merged per texture stem — no colliders. Every path classified: ribbons/labels/`giz_*` animated/PKG-props/unresolved/empty/degenerate/odd-tail/skipped quads/capped/missing textures/issues all counted in `CityReport.decals`. Retail: london 79 ribbons/85 quads/3 entities (zigzag, box junction, zebra), sf 48/223/2 (rails, skid marks) — 0 unresolved, 0 missing textures; screenshots show rail channels, zebra stripes, junction wash. `props.pathset` keeps classifying its 31 stale `r4i_rails_f` copies (26 byte-identical) without double-stamping. `MaterialCache::get_decal` shares the existing texture→material cache. Candidate pending external check. |
| F03-B.5 | implemented | F03-B.3, F04-B.1 | Placement-height repair for the operator report — all three channels. Root cause measured on retail `dgBangerData`+PKG pairs (temporary `probe_height` probe, since removed): prop meshes are authored centred at the bound centre; `Size` is the bound's *full* extents (`CG ± Size/2`, `CG.y = Size.y/2` on every measured record — cone 0.425/0.85, sawhorse 0.727/1.453, streetlamp 3.862/7.702, tptpole 6.151/12.309); the authored stamp point is where the bound's *base* rests. `city.rs` stamped the mesh centre at the point → props sank ~half their height. `PropOffset` contract: `Bound(+CG)` for stamped bound names, `Ground(−min_y, lift-only)` for unbound, `Verbatim` for INST — baked into render verts, collision accumulation and BREAK fragment pieces; `PropCache` keyed by name+offset class (same pkg can reach all three channels). `mm2_game`/`mm2_formats` docs corrected (`Size` full extents, `CG` bound centre); `angular_kick`/`strike` reach use half-extents of `Size` (kick lever now measured from the bound centre = CoM, not the origin); existing tests' settle windows widened for the physically-correct stronger kick. Regression test `tests/banger.rs::stamped_props_rest_their_bounds_on_the_path_point` asserts Avian `ColliderAabb` on all three channels through `load_city`. Retail: london headless counts unchanged (1997 inst / 1188 pathset / 5083 proprule, 0 unresolved, 0 decode failures); screenshots `/tmp/props-fixed-{london-trees,sf-lamps}.png` vs `screenshots/pathset-*.png` — trees rooted, lamps full height, benches/bollards on the surface, SF hill trees no longer stumps. Operator-visible defect corrected; F04-C.2/-C.3 retail evidence re-taken on corrected geometry (see their rows + `docs/research/banger.md` — new staging spawn for the break/settle leg, rest unchanged). Candidate pending external check. |
| F03-C | implemented | F03-B | Split: C.1 (lateral placement audit), C.2 (asymmetric-prop yaw repair + swept-footprint audit — implemented below), C.3 (kerb-height repair + vertical/sunk audit — implemented below). Owed: sampled original locations beyond the stamp-position audit (UNK-20 expansion density), race cleanup, mod replacement end-to-end. |
| F03-C.1 | implemented | F03-B | Operator-report-2 lateral check. `mm2_game::props` gains shared `path_stamp_sites` (city.rs now wraps it — runtime/audit one policy), `MAX_PATHSET_STAMPS`, and `carriageways` (authored carriageway triangles+rings from `RoadWithSidewalks`/`DividedRoad`/`RoadNoSidewalks`/`RoadFan`/`Crosswalk` attrs, medians excluded). `mm2-inspect placement <install> [--city] [--strict]` audits all three channels independently: INST origins, `props.pathset` expansion (same budget + classifier as runtime), `walk_prop_rules` stamps — each position classified vs carriageway tris in a ±height band with lateral depth-from-edge; hits listed deepest-first + channel×kind histogram; in-road counts are findings, `--strict` exits 2 on failures/issues only. Retail (install `fnv1a64:e91e6cd4b2ae30d9`, exit 0): london 3224 regions — inst 1997 stamps/15 hits, pathset 87 paths/1188 stamps/185 hits, prop-rule 415 rooms/5118 stamps/209 hits; sf 2871 regions — inst 3763/6, pathset 144/925/34, prop-rule 397/5028/0; 449 total, 0 failures, 67 issues (pre-existing prop-rule walk counts — 65 0xx `road_rooms` + unreached rooms — so `--strict` exits 2). Findings classify as authored intent: sf `cp_banr*` banner pivots stamped at road surface mid-span (depth to 10 m, dy≈0 — mesh hangs overhead), london `kl_hydeparkbridge03_l`/`wl_subway02_s_l` inst on/under drivable surfaces, prop-rule kerb-edge stamps at depth≈0 on `RoadNoSidewalks` walkway strips, plaza dressing on `RoadFan`/`RoadNoSidewalks` pedestrian surfaces — **no systematic lateral defect in any channel**; the report's "trees in the road" are authored stamps on the pedestrianised street at the spawn area. Flagged for visual review: 2 `sp_stackboxes_4_l` 1.2–1.8 m inside london RoadWithSidewalks room 570, 2 `sp_tree1_s` 1–3.9 m inside london DividedRoad rooms 937/952 (checked `/tmp/placement-divroad2.png` — median taper at a junction, authored), 5 crosswalk hits. UNK-20 expansion density + prop yaw stay open. Candidate pending external check. |
| F03-C.2 | implemented | F03-C.1 | Operator-report-3 prop yaw repair + swept-footprint audit. **Root cause (measured, not guessed):** every directional prop-rule kerb prop is authored front/arm-first along local **−X** (`sp_lightstreet_f` arm ≈ −7 m, `sp_traflitdual_f` mast ≈ −11.6 m, bench/sign faces −X; each `dgBangerData` bound wraps the pole alone) while `walk_prop_rules` stamped `forward = side.forward` — the walk direction — putting the face a quarter-turn off the carriageway. The pathset channel was already correct: painted direction → local +X verified on retail (85/85 sf `sp_lightstreet_rt_f` line-strip arm tips land over a carriageway; `sp_barricadeconcl_*_f` 5 m wall segments form continuous barriers — a +Z reading would comb them across the road). **Fix:** `PropStamp.forward` is now the per-stamp kerb→building-line direction from the strip cross-section (`norm(outer − curb)`, fallbacks `position − centre`, then the side's right), so +X goes building-ward and the authored −X front faces the road; curved kerbs rotate each stamp with the road edge. `yawed_transform` unchanged. **Shared helpers:** `mm2_game::props` gains `yawed_basis` (the INST-style axis images both runtime and audit share), `stamp_content_offset` (bound `+CG` / ground `−min_y` lift) and `stamp_space_verts` (best-LOD-per-stem verts, shadow/dmg excluded — the same selection `pkg_to_parts` renders); `lod_split` moved to `mm2_formats::pkg`; `city.rs` `yawed_transform`/`PropCache::build` now consume them. **Audit extension:** `mm2-inspect placement` sweeps every pathset/prop-rule stamp's `stamp_space_verts` through its basis, tests each vert against carriageway tris — street-level band (≤2.5 m) → `body_in_road` past 0.15 m edge epsilon, overhead band (≤8 m) → `overhang` — reporting channel×kind and channel×prop histograms; INST keeps the origin check (verbatim authored transforms). **Retail evidence (install `fnv1a64:e91e6cd4b2ae30d9`, exit 0):** prop-rule `body_in_road` london 574 → **243** and sf 108 → **2** vs the A/B-reverted walk-facing orientation (residuals: authored `RoadNoSidewalks`/`RoadFan` plaza dressing — park lamps, phone booths, benches — plus kerb-edge grazes); prop-rule `overhang` rose ~1 700 → 2 542 london / 747 → 2 573 sf (lamp arms now reach over carriageways). Before/after captures `/tmp/lamps_{before,after}.png` on sf room 444's lamp rows (`--cam=-1470,38,392,90,-8`): before shows arms parallel to the kerb, after shows them over the road — matching the operator's description. Tests: straight-room right/left forwards, multi-room, curved-kerb per-stamp facing, no-kerb building-line fallback, `footprint_catches_the_rotation_the_origin_misses`. Docs: `proprules.md` (measured −X-front evidence), `pathset.md` (+X direction verification), ledger UNK-20/UNK-21 narrowed. Gate repair folded in: rust-1.98 `chunks_exact_to_as_chunks` → `as_chunks` across 4 files. Candidate pending external check. |
|| F03-C.3 | implemented | F03-C.2 | Operator-report-4 item-4 kerb-height repair + vertical placement audit. **Root cause (measured, not guessed):** the `Room_attributes` doc states road vertices sit "0.15 units below the sidewalk vertices" and retail data agrees — `sw.y − road.y == 0.15` on 943/1087 london + 1205/1321 sf `RoadWithSidewalks` sections (residuals are authored flush ramps/driveways), `top.y − ground.y == 0.15` on all 5 880 london + 5 665 sf `SidewalkStrip` pairs — while `walk_prop_rules` lerped straight off the kerb-foot (road-level) chain, burying kerb-side props ≈0.135 m into the raised pavement. **Fix:** `KerbStrip` carries a `lift` — `SIDEWALK_KERB_LIFT` (0.15, shared with `mm2_app::city`'s `SIDEWALK_LIFT`) for `RoadWithSidewalks`/`DividedRoad`/`SidewalkStrip`, `0` for `RoadNoSidewalks` walkways — applied to the resolved (and possibly reversed) kerb chain before `strip_at` lerps kerb→outer; XZ/arc-length/facing untouched. **Audit extension:** `Carriageway` gains `SurfaceBand` (`Driving`/`SidewalkTop`/`FloorFan`) and `walkable_surfaces` emits every standable band — driving regions plus lifted sidewalk tops and mostly-horizontal `Fan`s (`|n.y| ≥ 0.3`, the renderer's `vertical_facing` test; walls excluded). `mm2-inspect placement` measures each stamp origin against it: a walkable surface standing 0.05–1.0 m above the stamp past a 0.05 m XZ-edge epsilon counts `sunk` (overpass/storey and kerb-line cases excluded), reported per channel plus a channel×band histogram. **Retail evidence (install `fnv1a64:e91e6cd4b2ae30d9`, exit 0):** london walkable surfaces 6 446 (3 224 Driving + 1 789 SidewalkTop + 1 433 FloorFan); prop-rule `sunk 0/5 118` post-fix — before/after captures `/tmp/kerb-{before,after}.png` at the operator's `cam -275.6,2.0,160.6,38,-13` show the LITTER bin resting on the pavement top. Remaining sunk findings are authored non-prop-rule placements for triage (london pathset 54 — incl. `sp_lightthames_l`/`sp_pilaster_l` rows at 0.2–0.37 m; inst 94 — building origins under plaza `Fan` floors). Tests: +6 props (kerb-side stamp on the top, flush authored outer kept, walkway authored height, walkable band counts, strip tops, fan normals) +4 placement (sunk measure, limits, lowest-surface winner); existing fixture verts now author sw chains at kerb-top height. Docs: `proprules.md` Y-bullet resolved, ledger WLD-25. Gates: fmt/clippy(locked)/test(locked) all pass, 883 tests. Candidate pending external check. |
| F04-A | implemented | F01-B, F03-B | Split: A.1 (banger parser + record↔geometry audit — externally checked at `ad471b0`), A.2 (placement→banger binding audit + R4 runtime-model research — externally checked) and A.3 (dormant→active→settled runtime slice — implemented below). Threshold semantics and the Timer despawn remain open (UNK-22); the prop-rule channel landed in F03-B.3 — the parent stays non-checked until the remaining legs land and AC01–AC06 each see direct evidence. |
| F04-A.1 | implemented | F01-B, F03-B | `mm2_formats::banger`: typed `BangerData`/`BirthRule` decoder on the shared `tune` grammar — Size/CG/Mass/Elasticity/Friction/ImpulseLimit2/NumParts + ids + `asBirthRule` variant (warning); `BangerIssue` validation (non-finite/negative physicals, glow-count, missing birth rule); `stem_role` (fallback/fragment/named). `mm2-inspect banger <install> [--strict]`: expected = `default.dgbangerdata`, denominator = every discovered file incl `.#*.1.2` backups; stem→geometry resolved via VFS (own pkg / `.mtx` / `BREAK<NN>` chunk in base pkg / longest-base part chunk), standalone `NumParts` ↔ BREAK-index counts. Retail: 999/999 parse, 216 standalone/477 part/254 fragment/47 dead refs, 54 issues, `--strict` exits 2; `scan` recognizes `.dgbangerdata`. docs/research/banger.md + WLD-15/UNK-22. Runtime unwired by design. Candidate pending external check. |
| F04-A.2 | implemented | F04-A.1 | `mm2-inspect banger-bind <install> [--city] [--strict]` + `mm2_formats::banger::geometry_owner` (record → owning PKG stem). Audits every stamped-prop source through the VFS: INST files, `city/`/`race/` `*.pathset` (incl overlays, backups, dev cities — no filtering), `propdefs/proprules/props.csv`, PSDL `prop_rule` reachability. Binding rule `N` → `tune/banger/<N>.dgbangerdata`. Retail: 129 source files, 3 unsupported (truncated blitz10/11 pathsets), 0 failures, 51 issues (dead placement refs kept in denominator). INST = static channel (london 0/221, sf 0/165 bound); `*_ai.inst` = bound `sp_stop_f` supplements; pathset/prop-rule names ~all bound. Reverse: 269/994 placed-reachable, 562 vehicle-owned, 163 never-placed. docs/research/banger.md binding + R4 runtime model; WLD-16 added, UNK-22 narrowed. Runtime state machine still unwired. Candidate pending external check. |
|| F04-A.3 | implemented | F04-A.2 | `mm2_game::banger` (`BangerPhase` dormant/active/settled, `Banger`, `BangerDefinition` distilled from `BangerData`, `BangerStateChanged` message, `BangerPool` ×32 — the R4-recovered pool size) + `mm2_app::banger` (`BangerDefs` VFS cache, `banger_bundle`, `activate_bangers`, `settle_bangers`). `stamp_pathset` binds each prop name: bound+collidable → one session-owned dynamic-capable entity (mesh children follow); unbound/failed/no-collider → the ordinary static pair (decode failures counted, not hidden). Activation reads raw `CollisionStart` via shared `deepest_contact`; provisional estimate `approach_speed × striker_mass` vs `ImpulseLimit2` (UNK-22); one impulse + `Size`-derived spin kick; oldest-first pool reclaim; Avian sleep → `Settled` static; authority-gated (`Predicted` never transitions). 7 integration tests on real physics + 5 `mm2_game` unit tests; retail headless london `bng=1188d/0a/0s`, sf `bng=925d/0a/0s`, 0 decode failures. DSN-10 added. Deferred: fragments (F04-B), BirthRule/audio/flash/decal effects, prop-rule channel, Timer despawn, replication. AC01–AC06 stay open. Candidate pending external check. |
| F04-B | implemented | F04-A | Split: B.1 (BREAK<NN> fragment spawning on activation — implemented below). Fragment timing vs a later break threshold, `NumParts` runtime role, `BirthRule`/audio/flash/decal effects and original-content activation remain open (UNK-22); parent stays non-checked until F04-C + AC evidence land. |
| F04-B.1 | implemented | F04-A.3 | `BangerPhase::Broken`; `pkg_to_parts` splits `BREAK<NN>` chunks into `FragmentModel`s (authored file order); `stamp_pathset` stamps `BangerPieces` (each piece resolves `tune/banger/<name>_break<NN>`, parent-def fallback). `break_banger` on a qualifying edge: parent → `Broken` (collider + mesh children removed, one `BangerStateChanged`, never also `Active`), one dynamic fragment body per collidable piece — own `ObjectId`, session-owned, convex collider, impact velocity + CG-lever spin, pool-bounded via `claim_slot` (pending same-tick spawns counted; `max_active = 0` spawns nothing). Pieces without colliders → ordinary activation. Fragments spawn `Active` and settle through `settle_bangers`. 4 new integration tests (shatter/pool-bound/no-collider-fallback/stamp pieces) + teardown coverage; retail sf `bng=925d/0a/0s/0b` pieces=3092, london 1188/2710, overlay barricades 0 pieces (authored). DSN-10 + UNK-22 updated. F04-A.3 review nits fixed (INST-doc wording, zero-cap). Deferred: `BirthRule` particles, audio/flash/decal effects, prop-rule channel, Timer despawn, replication, original-content activation. AC01–AC06 stay open. Candidate pending external check. |
| F04-C | implemented | F04-B | Split: C.1 (original-content strike evidence + dev spawn/tooling), C.2 (flat-ground fragment settle + repeated-collision/pool-reclaim retail runs) and C.3 (the hull-clearance fidelity question — implemented below). Break timing vs a separate original break threshold, `NumParts` runtime role and natural-pool (>32 simultaneous) reclaim stay open; parent stays non-checked. |
| F04-C.1 | implemented | F04-B.1 | `DevOverrides::spawn` + `--spawn x,y,z[,yaw]` (quarantined dev pose applied after world/event spawn selection); `bng_ev=<a>a/<s>s/<b>b` emitted-transition counters in the headless smoke record (distinct from `bng=` end-state buckets; fragment spawns are silent by contract). New tests: `dev_spawn_override_pins_the_player_pose`; `restart_restores_stamped_placements_after_a_break` — synthetic city, real `load_session_world` + `drive_session` restart, husk+fragments removed, dormant restamp under gen-2, zero generation leaks. Retail evidence (iteration-33 correction — first recorded London command was irreproducible, retracted): `vpbug --spawn 0.4,5.5,-720,0` and `--spawn 112.3,5.5,-745,0` → deterministic `sp_bollard_black_l` activation+settle `bng_ev=1a/1s/0b` on flat London road rows (verified ×2, bit-identical); SF `sp_wrongwayfw` break into its authored 3 fragments (`bng_ev=0a/0s/1b`, `bng=924d/3a/0s/1b`); below-threshold graze + sub-threshold cone block → no transition (AC01 both sides, real content); SF `sp_cone_f` activates (`bng_ev=1a`) but does not settle on slopes; `circuit:7` overlay stamps 899 bangers (all `sp_barricadeconc[lr]_f`, 0 pieces); the `sp_sawhrslt_f` roadblock belongs to `checkpoint:7`'s `race7.pathset` (111 stamps, 13 sawhorses — row corrected post-F03-B.5). Gaps: fragments/knocked props stay `Active` on slopes (slope-tumbling suspected, unproven); hull underside clearance means tall vehicles cannot contact <~1 m props (raycast wheels) — open fidelity question; no GPU break capture; UNK-22 semantics unverified. Candidate pending external check. |
| F04-C.2 | implemented | F04-C.1 | `DevOverrides::banger_pool` + `--banger-pool <n>` (quarantined dev bound applied session-scoped in `load_session_world`, re-stamped on restart); smoke record gains `bng_pool=<n>` when overridden and `bng_rec=<n>` counting `Reclaimed`-cause settles (default records bit-identical). New test `dev_banger_pool_override_bounds_the_active_pool` (override lands, unconfigured session keeps ×32). Retail evidence (install `fnv1a64:e91e6cd4b2ae30d9`): flat-ground break+settle — `vpddbus --spawn=-170,1.5,-565,30` on the SF `sp_barricadewood_f` row (limit 51888, NumParts 4, y≈0 lot) → `bng_ev=0a/1s/1b` at 2400 ticks, fragments sleep 1→2→3 of 4 by 10000 ticks; repeated collisions — `impacts=104/105` at 5000/10000 ticks of pen battering, all finite, no extra transitions; pool cap — same spawn `--banger-pool 2`/`1` → `2a`/`1a` (authored-4 fragments capped, rest skipped); repeated activations + reclaim — `vpbug --spawn=802,6,-905,180` through the London `sp_bollard_stone_l` plaza ring → `bng_ev=3a/0s/0b` default, `bng_rec=1` at pool 2, `bng_rec=2` at pool 1 (bit-identical re-run). Gaps: natural-pool (>32 simultaneous) reclaim not staged — no surveyed retail site yields that density; slope-settle question answered (flat sleeps, slopes tumble); `vpddbus` clears `sp_cone_f` without contact (hull question unchanged); UNK-22 unverified. **Re-taken post-F03-B.5 (same install):** the original `−170,1.5,−565,30` spawn now falls just under the 51 888 limit (`0a/0s/0b` — bus slides around the row; corrected record, both threshold sides evidenced incl. `--spawn=-164,1.5,-552,30` stalling dormant at 9.7 m/s); restaged perpendicular `--spawn=-141.9,1.5,-608.5,115` → `0a/5s/2b` at 10 000 ticks (two shatters at ~82 800 estimate, 5 fragment `Slept` settles, bit-identical re-run, `impacts=80` finite); `--banger-pool 2`/`1` → `1a/1s/2b`/`1a/0s/2b` caps hold; London `vpbug 802,6,-905,180` reclaim ring bit-identical (`3a` default, `bng_rec=1`@2, `=2`@1). `bng=` denominators now include the prop-rule channel. Candidate pending external check. |
| F04-C.3 | implemented | F04-C.2 | Hull-clearance fidelity resolved via bound-vs-bound evidence (mm2hook: `dgBangerData.Bound`/`ColliderId`; every car carries a `phBound`). `VehicleConfig.striker_points` = the unmodified authored bound (validated like `collider_points`, inherited through `assemble` overrides); `vehicle_bundle` attaches it as `StrikeBound` — a component for shape-overlap queries only, never a world collider (fallback: the chassis collider itself). `activate_bangers` gains a second strike source: each moving `StrikeBound` runs `SpatialQuery::shape_intersections` against dormant bangers; severity = bound surface velocity at the prop centre, estimate/gate = the same `impulse_estimate` vs `ImpulseLimit2`, lever = the prop's upwind face by `Size`, deduped against same-tick contact edges. World contact still meets only the snag-safe hull — no global lowering, per-vehicle bounds kept verbatim. 4 new integration tests (overlap strikes what the hull clears, parked overlap inert, below-limit dormant, bundle surface). Retail (install `fnv1a64:e91e6cd4b2ae30d9`, `--bot`): `vpddbus --spawn=-1641.6,36.7,410,0` → `bng_ev=1a/0s/0b` over the `sp_cone_f` cluster it previously ghosted; `vpbus --spawn=0.4,5.5,-720,0` → `bng_ev=1a/1s/0b` through the `sp_bollard_black_l` row; `vpbug` same run → `2a/2s` (was `1a/1s` — bound catches a second bollard). **Re-taken post-F03-B.5 (same install, `--bot`, 1500f):** `vpddbus` → `3a/2s/0b` (full-height cone bounds overlap more of the row; two settle on the slope), `vpbus` → `1a/1s/0b` unchanged, `vpbug` → `1a/1s/0b` (was `2a/2s` on the sunk row — halved reach + corrected kick changed the clipped bollard). Classified implementation choice on an original bound-vs-bound requirement; overlap supplies speed/lever, no manifold — impulse equivalence to a contact is provisional (UNK-22). Whether wheels/bumpers struck bound-clearing props in the original stays unknown. Candidate pending external check. |
| F04-C.4 | implemented | F04-C.3 | Operator-report-4 item-2 repair — breakable collision response. **Root cause:** `activate_bangers` runs post-solver, so a dormant prop answered its first contact as an infinite-mass static body — the striker took the full wall impulse, then the prop activated with a flat approach-speed kick. Explains the extreme bounce, the total speed loss and the absent plow-through together. **Fix:** `deepest_contact` now returns `ContactDetails` — point, argument-order-normalized normal (the stored pair order no longer leaks: a latent normal/anchor flip for every consumer is fixed), severity, combined restitution, the solver's applied normal impulse, and both body anchors — shared unchanged by `collect_impacts`, `activate_bangers` and `knock_ambient`. A qualifying activation computes `J = (1+e)·v·μ` (reduced mass of striker and authored `Mass`), returns the applied wall impulse to the striker along the normal the solver pushed on (`applied_dir`, tracked separately so glancing contacts keep their lateral component), pays `J/striker_mass` as Δv plus a contact-anchor torque term, and launches the prop at `J/prop_mass` plus the authored-`Size` spin kick. `StrikeBound` overlaps pay the same share (`applied_impulse = 0`, record `Elasticity`, zero lever → linear-only correction); a bound strike coincident with a below-limit world contact folds the real solve data in so the striker pays once. Unresolvable striker mass degrades to the pre-transfer launch and skips the correction. Authored `Mass`/`Elasticity`/`ImpulseLimit2` untouched; no tuning constants. Tests +3: light-prop plow-through (striker retains ~19/20 m/s where the wall left ≈−5), bound-strike momentum share, coincident contact+bound charges once. Retail (install `fnv1a64:e91e6cd4b2ae30d9`, identical commands before/after): london `vpbug --spawn=0.4,5.5,-720,0 --frames 1500` → `moved=55m peak=17.4 bng_ev=2a/1s` **before**, `moved=124m peak=24.5 bng_ev=1a` **after**; sf `vpddbus --spawn=-1641.6,36.7,410,0 --frames 800` → `moved=46m peak=19.6 bng_ev=2a` before, `moved=99m peak=19.7 bng_ev=3a/1s` after. Classified implementation choice under UNK-22 — the original's exchange quantity is unrecovered; the measured defect is repaired through the authored data path. Gates: fmt/clippy(locked)/test(locked) pass. Candidate pending external check. |
| F04-C.5 | implemented | F04-C.4 | Operator-report-4 item-3 repair — banger activation threshold reading. **Binding ruled out first:** INST binds no tree/pole names in either stock city, but trees/lights arrive via prop-rule stamping + `props.pathset` and already spawn as dormant bangers; `sp_tree1_s`/`sp_lightstreet_*`/`sp_light_tall_l` are authored breakable (BREAK<NN> chunks + fragment records). **Root cause:** the activation estimate compared `approach_speed × striker_mass` to `ImpulseLimit2` — the authored census (`ImpulseLimit2 ≈ Mass × {31.25, 500, 800, 2000, 85342}`, 1e30 for bridges/monuments) is quadratic in striker speed, so every large breakable needed unreachable speeds under a linear reading (vpbug ~470 m/s for `sp_tree1_s`, ~120 m/s for `sp_lightstreet_l`). **Fix:** `mm2_app::contracts` gains `impact_energy` (`½·m·v²`, same 1 kg fallback for unresolved masses) sharing `striker_mass` with the retained linear `impulse_estimate`; both `activate_bangers` gates (real manifold + `StrikeBound` overlap) now feed `impact_energy` to `activates_on`. Traffic knock (`knock_ambient`) keeps its designed linear threshold and breakaway keeps its own `severity × part_mass` reading — neither shares the banger gate. `banger shattered`/`banger activated` debug lines now name the prop. Tests: limit-sensitive rows retuned to energy scale; new `an_authored_tree_limit_demands_speed_not_just_mass` — a 2e6 tree-class limit stays dormant at ~20 m/s and activates at ~80 m/s. 21/21 banger tests pass. Retail (install `fnv1a64:e91e6cd4b2ae30d9`): SF `vpbug` headless cruise → `banger shattered name=sp_lightstreet_rt_f severity=37.5 estimate=702303` (`bng_ev=0a/1s/1b`) — a 10 t NumParts=2 streetlight the linear reading priced at ~75 m/s; London `vpbug --spawn=510,5.5,-245,-51.7` → `banger activated prop=sp_can_royal_l severity=13.4 estimate=89736` crossing the (624,-335) junction. Gap: a named `sp_tree1_s` shatter not staged after ~12 attempts — in-road trees sit inside junction fans behind kerb clutter or on medians with ~25 m spacing, so the straight-line `hold` driver cannot reach the ~22–31 m/s the class needs; the census + unit regression cover the threshold math both ways and `sp_lightstreet_rt_f` proves the end-to-end break path on the same code. Classified designed under UNK-22 — energy is the implemented reading because the authored ladder is quadratic, not a recovered original. Gates: fmt/clippy(locked)/test(locked) pass. Candidate pending external check. |
| F05-A | checked | F01-B, F02-B | A.1 (authored damage/recovery data boundary) externally reviewed + gates passed 2026-09-22 (review verdict pass, `ae64197`). Runtime work continues under F05-B. |
| F05-A.1 | implemented | F01-B, F02-B | `mm2_formats::veh`: typed `VehCarDamage` (uniform 38-field retail set incl. flat `DamageEffect` particle spec + `MirrorPivot` as a 39th field on 7 records), `VehStuck` (6 fields) and `VehGyro` (optional `Roll`/`Pitch` — strict `opt_f32`: absent `None`, non-numeric errors, never fabricated) decoders on the shared tune grammar; `DamageIssue::{NonFinite, Negative, MedAboveMax}` validation. `mm2_game::damage`: `DamageSpec` (authored bounds), `DamageState` (threshold-gated saturating accumulator + `DamageTier`/`DamageVerdict`, regeneration channel, authority-named `repair`/`reset`), `disabled_outcome(mode)` mapping documented RACE-5/DMG-2 (CrashCourse → restart is designed). `mm2_content`: `VehicleDef` gains `damage`/`stuck`/`gyro` options with source provenance — resolved-but-malformed records warn into `ConversionReport` instead of sinking the load; `DamageAudit` census. `mm2-inspect damage <install> [--strict]`: every discovered damage-family record decoded, per-vehicle breakaway inventory (pkg `BREAK*` chunks ↔ `_break*.mtx` ↔ `_break*` banger records, dead fragments flagged), uncatalogued records kept in denominator; `car` output shows the decoded damage/stuck/gyro values. Retail (install `fnv1a64:e91e6cd4b2ae30d9`, 2026-09-22): 29 catalog vehicles — 20 `vehcardamage` + 20 `vehstuck` + 21 `vehgyro` = 61/61 parsed, 0 issues, `vpmoonrover` authored-absent, 14 vehicles with breakaway parts (vpsemi/vpftruck 6-piece max), 10 dead fragment records as findings; `--strict` exits 0. First candidate failed external review: the recorded per-vehicle census was falsified in six places (ImpactThreshold uniformity, DoublePivot, MirrorPivot set, gyro Roll/Pitch absent-set, TextelDamageRadius range, vehstuck Turn/MoveThresh). Repaired 2026-09-22 by scripted `mm2-inspect dump` census of all 61 records — every enumerated claim in veh.rs/damage.rs docs, docs/research/damage.md and DMG-5..8 corrected to measured values (MaxDamage min is 187.5k vpcoop/vpcoop2k, not vpauditt; MedDamage min 80k). docs/research/damage.md + DMG-5..8/REC-1/UNK-13 narrowed. Runtime damage application, visual/impairment effects and recovery behavior NOT implemented — authored-data boundary only. **Externally checked** — review verdict pass at `ae64197` (2026-09-22); minor non-blocking doc nit (vplafrance breakaway-set tie) fixed in F05-B.1's docs pass. |
| F05-B | active | F05-A | Split: B.1 (runtime impact→damage application + disabled outcomes), B.2 (authored `vehstuck` detection + in-place recovery), B.3 (authored breakaway detachment — impact-driven reading, DSN-21), B.4 (authored `vehgyro` consumption — designed application reading, DSN-22), B.5 (water/out-of-bounds recovery — designed policy end to end, DSN-23), B.6 (authored engine-smoke visual tier — designed emission policy, DSN-24), B.7 (engine impairment — designed smoke↔torque coupling, DSN-25), B.8 (authored-impact sparks — `asLineSparks`, designed emission policy, DSN-26) and B.9 (impact texel damage — `fxTexelDamage::ApplyDamage`/`Reset`, recovered mechanics + designed splat, DSN-32) implemented below. Remaining: damage-driven detachment if the original uses it (UNK-13), C&R healing driver (DMG-4 — needs the mode), replication (F25+). |
| F05-B.8 | implemented | F05-B.7 | Impact sparks — `asLineSparks` recovered on `vehCarDamage` (mm2hook: `ImpactCB` fires `RadialBlast(count, position, velocity)`); emission shape/count/life/texture binding unrecovered → designed DSN-26, UNK-13 stands. `mm2_game::effects`: `SparkPolicy` (`sparks_per_speed` 0.8, `min_burst` 2, `max_burst` 16, `max_live` 128, `speed` 7 ±`speed_var` 3, `spread` 0.8 rad, `life` 0.5 ±`life_var` 0.2, `length` 0.04, `gravity` 9.8 — designed), `VehicleSparks` (per-vehicle rig gated on authored `vehcardamage`, `NavRng` seeded by `ObjectId`; `burst(point, outward, severity, live, emitter)` — garbage-safe: non-finite/`severity <= 0` bursts nothing, `live >= max_live` truncates, zero/NaN normals fall back to straight up), `Spark` (`advance` Euler + expiry, `streak()` speed-scaled length floored at `length`, `alpha()` linear `1 − age/life`). `mm2_app::spark_fx`: `emit_sparks` (`Update`, chained with `advance_sparks` — an independent broadcast reader; `collect_impacts`/`apply_impact_damage` publish the stream in `FixedLast`) drains the deduped `ImpactEvent` stream — one burst per rigged local/AI participant at the authored contact point, rebound side = participant's own side of `normal`; remote skipped (F25+), non-`Playing` drains without emitting; a per-emitter local live count closes the deferred-spawn gap so burst-heavy frames stay inside `max_live`. Streaks are session-stamped entities — velocity-aligned crossed quads (width 0.03 m), unlit `AlphaMode::Add` `StandardMaterial` cloned per spark (warm tint), `spark.tga` via `city::load_image` (8×8 fleck, the install's only spark-named texture; missing → warn + untextured additive material, emission continues; absent `SparkFx` resource → nothing emits). `advance_sparks` reposes along live velocity, writes the linear fade, despawns on expiry. `SparkFxReport` → `spk=<b>b/<e>e/<x>x` on activity only. `VehicleSparks` attached at player + opponent spawn behind the same `def.damage` gate as `VehicleDamage`/`VehicleSmoke`; `drive_session` teardown resets the report. Tests: +3 mm2_game (burst floor/scaling/ceiling + garbage severity, deterministic + distinct streams + contact-point birth + rebound hemisphere + emitter + live bound + degenerate/NaN normals, advance gravity/streak/alpha/expiry + garbage-dt) +9 mm2_app (contact-point emission, both participants' own rebound sides, remote/unrigged/missing-assets/stale negatives, streak velocity-tracking + fade, expiry, pool bound under a burst-heavy feed). Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`): sf vpbug `--frames 600` → `impacts=12 dmg=3a/0d/0r spk=6b/20e/18x` (bursts on the 6 player-involved impacts; trajectory bit-identical to pre-change), london → `impacts=7 dmg=1a/0d/0r spk=5b/23e/23x`, re-run identical. Original burst semantics + texture name stay unverified (UNK-13); texel damage open per F05-B row. Candidate pending external check. |
| F05-B.9 | implemented | F05-B.8 | Impact texel damage — the `fxTexelDamage` leg of `vehCarDamage` (DMG-9's pairing leg landed in F02-C.4). `ApplyDamage`/`Reset` semantics recovered from mm2hook (`src/modules/effects/texeldamage.{h,cpp}`): every body tri with a vertex within the authored `TextelDamageRadius` of the car-space impact point earns one splat at a random barycentric UV; repair re-blits the clean texture over the clone. The splat shape (`ApplyBirdPoopDamage`) is an unrecovered binary call → designed radial blit (DSN-32, UNK-13 stands). `mm2_game::texel`: `TexelDamageMesh`/`TexelDamageTri`/`TexelSplat` (`splats` — radius test + normalized `frand` barycentric pick off a per-vehicle `NavRng` seeded by object id, garbage-safe), `TexelDamagePolicy` + `splat_blit` (48 px probability-dithered disc `1 − d/r` + 0.1 floor, mip-chain propagation, bounds-clipped — follows mm2hook's debug reimplementation). `mm2_app::texel_fx`: `TexelSlot` (material/`clean`/`damage`/`current` — the recovered `CurrentShaders`/`DamageTextures` mirror), `TexelDamageRig` component (radius + slots + car-space tri soup + policy + `NavRng`), `TexelDamageBuilder` (records `PartRole::Body` tris only on slots that pair a `_dmg`), `apply_texel_damage` (FixedLast after `apply_impact_damage`, same deduped stream/gates, world point → car space via the rig transform), `TexelRepair` SystemParam (`resolve_disabled`'s three `damage.reset()` sites call `reset` — Cruise free, Circuit penalty, AI), `TexelDamageReport` → `txl=<i>i/<s>s/<r>r` on activity only. `MaterialCache::texel_binding` pairs clean↔`_dmg` (orphan `_dmg` reuses the bound texture both ways — retail's `gfxGetTexture` cache semantics), clones `current` per vehicle, clones the material, validates 4bpp formats. Player + opponents build the rig behind `def.damage`; ambient traffic and trailers pass `None`. Tests: +8 mm2_game (radius filter, barycentric UV, determinism, degenerate/garbage legs, blit disc/floor/clipping/mip legs) +3 mm2_app (rig build body-only + material/texture independence, impact stamps cloned pixels, `resolve_disabled` repair restores clean + counts) +1 city (clean/damage pairing incl. orphan leg + per-vehicle clones); 17 test harnesses gained the new `TexelDamageReport` resource. Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`): sf cruise vpbug `--frames 1800` → `impacts=42 dmg=10a/0d/0r spk=15b/105e/105x txl=15i/2543s/0r`; sf `circuit:0 --bot --frames 12000` → `dmg=237a/4d/3r … txl=401i/64364s/3r` — resets track repairs exactly, every player impact (applied + 164 threshold-rejected) stamped. Rendered (local, not committed): `--spawn` nose-first 11 m drop stamps hood/windshield/lamps while the same car's rear stays clean (`/tmp/txl-front.png` vs `/tmp/txl-dbg2.png`); a light settle stamps the rear (`/tmp/txl-dbg-clean.png`). Splat shape unverified (DSN-32); a rendered post-repair capture not staged (repair proven by `txl` reset counters + pixel-restore test). Candidate pending external check. |
| F05-B.7 | implemented | F05-B.6 | Engine impairment — the smoke↔torque coupling MM2Hook's `PhysicalEngineDamage` ini option documents ("when the engine spews smoke … less acceleration and less top speed"; shape unrecovered → designed DSN-25, UNK-13 stands). `mm2_game::damage::ImpairmentPolicy` (`power_at_med` 0.8, `power_at_max` 0.4 — designed): `factor(total, spec)` is 1.0 below `MedDamage`, steps to `power_at_med` on reaching it — `factor < 1` exactly when the DSN-24 smoke gate emits, so the documented coupling holds by construction — then ramps linearly to `power_at_max` at `MaxDamage`; degenerate specs and non-finite input degrade to full output. `mm2_vehicle::EngineImpairment` (new component) scales only `wheel_drive_available` — brakes, `engine_brake_nm`, steering and tire forces are not engine output and stay unscaled; the sim sanitises the factor (non-finite → 1.0, clamp 0..1) and absence of the component is identical to 1.0. `mm2_app::damage::sync_impairment` (FixedLast after `resolve_disabled`, authority + `Playing` gated) mirrors each local/AI participant's authored total — insert while `factor < 1` (`DamageReport.impaired`), update as it deepens, remove the same tick a repair lands (`restored`); remote participants skipped (F25+). `imp=<i>i/<r>r` smoke field on activity only. Tests: +2 mm2_game (ramp/boundary legs, degenerate/garbage sanitisation), +4 mm2_vehicle (impaired accelerates slower but drives, full/garbage bit-identical, zero-scale no-drive, brakes unimpaired), +3 mm2_app (component+report tracking incl. deepening, repair-restore, remote skip). Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`): active SF vpcoop tumble `--spawn=-1316,60,381,0` 6000f → `status=pass impacts=133 dmg=23a/0d/0r vsk=39a/1d/1r ptx=639e/621x imp=1i/0r` (crossed 80k MedDamage once, stayed impaired, never disabled — trajectory diverges from the pre-change record as expected once the engine weakens); inactive SF vpbug + london scripted cruises `status=pass`, no `imp=`/`ptx=` fields — undamaged runs stay bit-identical. Candidate pending external check. |
| F05-B.6 | implemented | F05-B.5 | Authored engine-smoke visual tier — the `vehCarDamage` embedded particle spec (mm2hook's recovered `EngineSmokeRule`) rendered as billboard puffs. `mm2_game::effects`: `ParticleSpec` (every `DamageEffect` field verbatim; consumed: `PositionVar`/`Velocity`±var/`Life`±var/`Radius`±var/`Drag`±var/`DRadius`±var/`DAlpha`±var/`Gravity`/`TexFrame` range/`Color`; carried unconsumed: `Position`/`Mass`/`Damp`/`DRotation`/`InitialBlast`/`SpewRate`/`SpewTimeLimit`/`BirthFlags`/`Height`/`Intensity`), `SmokePolicy` (designed DSN-24: rate 0 below `MedDamage`, 4→24 puffs/s ramp to `MaxDamage`, `max_live` 48/vehicle, `atlas_tiles` 2), `VehicleSmoke` component (authored pivot gate — `SmokeOffset` always, `MirrorPivot != 0` mirrors about x else non-zero `SmokeOffset2` seconds; `DoublePivot` emits all pivots per burst else `m_CurrentPivot` alternation; per-emitter `NavRng` seeded by object id → replicable emission), `SmokePuff` component (the entity is the particle; `advance` integrates designed field readings — gravity +Y rise, exponential drag, `DRadius` growth, byte-space `DAlpha` fade of the ARGB `Color` — expiry on authored `Life`). `mm2_app::damage_fx`: `smoke_assets` builds `SmokeAssets` once per session through `city::load_image` (`texture/fxpt2` — measured 2×2 puff atlas, `TexFrame` indexes tiles like `asSparkPos::TexCoordOffset`; missing texture warns → untextured fallback, never sinks), per-tile UV-baked quads + an unlit blended material; `drive_smoke` emits from every `VehicleDamage`+`VehicleSmoke` entity (`Remote` skipped — its authority renders its own; `is_playing` gated), `advance_smoke` integrates/billboards/fades/despawns. `SmokeFxReport` (`ptx=` smoke field on activity) + `SmokeFx` session-scoped (inserted on load, removed + report reset on teardown). `VehicleSmoke` attached at player + opponent spawn behind the same `def.damage` authored-presence gate as `VehicleDamage`. Tests: +11 mm2_game (spec carry, pivot gate legs incl. mirror, rate ramp, alternation, double-pivot, live bound, deterministic puff draws, frame clamp, integration, alpha fade, garbage-dt) +10 mm2_app (pivot emission, intact/remote/missing-assets negatives, repair-stop, expiry+bound, pause freeze, session stamp, billboard, material alpha sync). Retail headless pending run; original cadence/pivot-switch/`fxpt2` binding stay unverified (UNK-13). Candidate pending external check. |
| F05-B.5 | implemented | F05-B.4 | Water/out-of-bounds recovery — a designed policy end to end (no authored record exists: the damage-family census covers `vehcardamage`/`vehstuck`/`vehgyro` only, DMG-2 covers destruction; the original's rules stay UNK-13, DSN-23). `mm2_game::recovery`: `RecoveryPolicy` (`water_min_drag` 0.3 splits retail `deepwater` 0.5 from shallow `water` 0.119 — ponds stay wadable under F06-B.2; `submerge_dwell` 2.0 s; `fall_margin` 50 m), `GroundContact` (the app side's per-step wheel-surface classification so the contract never borrows the wheel type), `VehicleRecovery` component (anchor = last pose a grounded wheel read a dry surface — water colliders are solid, so drowning is a surface class under the wheels, not a missing floor; pre-anchored at spawn; OOB leg latched once per fall; non-finite pose fires at once as defence-in-depth — a NaN inside the physics step trips the wheel raycast first), `RecoveryEvent` (bounded: fresh dwell per fire, one per fall). `mm2_app::recovery`: `track_recovery` reads `WheelState::surface_drag` (the same coefficient the wading term consumes) — any dry wheel re-anchors, all-wet accrues the dwell, airborne past the margin fires OOB — and `resolve_recovery` answers with `ResetVehicle` to the anchor (`Teleported`, no checkpoint sweep; spawn fallback when no anchor exists), trailers re-seat at authored offsets, armed stuck episodes disarm, recovery is *not* a repair (damage/detached parts persist). Local + AI identical (designed), remote skipped (F25+), `Disabled` wrecks skipped (the damage outcome owns them). Spawned unconditionally on the player + AI opponents (no authored record to gate). `rcv=<w>w/<f>f/<r>r` smoke field on activity only — dry-ground runs stay bit-identical. Tests: +9 mm2_game (anchor/dwell/escape/fall-latch/legit-landing/NaN/recovered/garbage-dt legs) +11 mm2_app (deep-water recovery to the dry anchor end-to-end, shallow-water wading negative, in-dwell escape, off-world fall recovery, legit big drop, recovery-is-not-repair, remote/disabled/stale-generation negatives, stuck disarm, trailer re-seat). Retail headless pending run; original water/OOB semantics stay unverified. Candidate pending external check. |
| F05-B.4 | implemented | F05-B.3 | Authored `vehgyro` consumption. `mm2_vehicle::config`: `GyroConfig` (spin180/reverse180/drift + optional pitch/roll — `Option` preserved, never fabricated) on `VehicleConfig.gyro`; validation requires finite, nonnegative rates (pitch/roll keep the authored sign, inert ≤ 0). `mm2_content`: `ConvertInput.gyro` → `config.gyro` verbatim — malformed negative/non-finite rates warn + clamp rather than sink the load; `report.imported` provenance records the gyro fields. `mm2_vehicle::vehicle`: `GyroSpin` latch (rate/rotated/age) + `gyro_spins`/`gyro_completed` evidence counters on `VehicleState`. `mm2_vehicle::systems` (designed application, DSN-22 — original `Update()` unrecovered, UNK-13): handbrake + steering while travelling latches a spin — `Spin180` forward, `Reverse180` in reverse — the gyro writes the authored yaw rate directly (rate actuator, not torque: tire friction correctly resists rotation once speed scrubs and would stall the maneuver ~140°); the handbrake hold doses rotation — release ends partway, ~π completes, opposite flick re-arms, bounded age prevents indefinite servoing. `sim::yaw_damp_factor` relieves the damper's slip term by `Drift` (`drift = 0` = unmodified policy); `Pitch`/`Roll` are per-axis airborne righting rates shaped on `air_control` — authored 0.0/absent on all 21 retail records, inert on stock. `mm2_app::smoke`: `gyr=N/M` only on maneuver activity (bot never handbrakes → inactive records bit-identical). Tests: +7 mm2_vehicle integration (no-record negative, held-spin completion, tap dosing, J-turn rate, re-arm, drift-relief comparison, airborne righting + unaided control) +1 sim unit test; +1 mm2_content leg (verbatim conversion + provenance + absent axes) +1 malformed/out-of-range gyro leg; audit denominator gains the two new fixture ids. Candidate pending external check. |
| F05-B.3 | implemented | F05-B.2 | Authored breakaway detachment. `mm2_game::breakaway`: `BreakPartSpec` (part stem + distilled `BangerDefinition` verbatim), `VehicleBreaks` component (`detachable(approach_speed)` per part, one-shot `detach`, `restore` draining spawned fragments), `PartDetached` message (object/generation/tick/part/fragment/estimate — bounded one per attachment, F05-AC06). `mm2_content::assemble`: `VehicleDef.breaks` resolves only parts carrying *both* a `PartRole::Break` model part and a decodable `<id>_<part>.dgbangerdata` record — malformed records warn into `ConversionReport`, unmatched chunks/records stay bolted/dead (the 10 retail dead fragments never spawn). `mm2_app::breakaway`: `BreakPartVisual` tags each break node's local pose + convex hull + centroid (car_visual `PartRole::Break` arm); `detach_breaks` (FixedLast, authority+Playing gated, drains stale) consumes the deduped `ImpactEvent` stream — a part detaches when `severity × part_mass` exceeds its authored `ImpulseLimit2`, the `limit / mass` reading the authored data is shaped for: measured ≈31.25 m/s (~70 mph) on every ordinary panel, ≈2500 m/s "never" on heavy-rig anchors (DSN-21 — the comparison stays UNK-22, damage-vs-impact-driven stays UNK-13). Detach hides the intact node and spawns a fragment body at `car_pose × node_local` — part's own hull, record physicals, car velocity at centroid + `dir × severity` kick/spin — pooled via the shared `BangerPool` (`BangerMut`/`claim_slot` made `pub(crate)`); a pool-bound part still leaves the rig (`fragment: None`). `resolve_disabled` calls `restore_rig` at each `damage.reset()` site — repair re-attaches, despawns fragments, shows nodes (F05-AC03); plain reset/stuck recovery is not a repair. Record `CG`/`Size` anchors unconsumed (mixed conventions — UNK-13); fragment `CenterOfMass` is the hull centroid. Remote rigs skipped (F25+). `brk=` smoke field on activity. Tests: +6 mm2_game (spec carry, per-part limits incl. at-limit, dedup, garbage, bounded detach, restore), +11 mm2_app (detach-once+hide+event+fragment, below-limit, per-part thresholds, repair-restore, reset-is-not-repair, mesh-child re-spawn, remote/no-rig/pool-bound/no-node/stale-generation negatives). Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`): sf + london `--frames 600` bit-identical, no `brk=` (vpbug authors no parts — measured absence); vpcoop sf `--frames 1200` `impacts=27 dmg=6a/0d/0r vsk=9a/0d/0r`, still no `brk=` — scripted approach speeds never reach ~31 m/s, matching the authored thresholds. Open: per F05-B row. Candidate pending external check. |
| F05-B.1 | implemented | F05-A | Runtime impact→damage application + session disabled outcomes. `mm2_game::damage`: `VehicleDamage` component (authored `DamageSpec` + `DamageState` behind a monotonic `ImpactId` watermark — re-delivered/out-of-order impacts return new `DamageVerdict::Duplicate`, F05-AC06), `DamageEvent` (object/generation/tick/impact/applied-severity/total/tier, emitted per accepted application only), `DISABLED_PENALTY_TICKS` (5 s, designed — RACE-5 documents a penalty but no magnitude, UNK-13). `mm2_app::damage`: `apply_impact_damage` (FixedLast, authority+Playing gated, drains stale) consumes the deduped `ImpactEvent` stream — per-participant impulse = `severity × other_mass` (own mass vs world/unresolvable; designed conversion, UNK-13) — emitting `DamageEvent`s and counting `DamageReport` (`dmg=` smoke field); `resolve_disabled` enforces `disabled_outcome` idempotently (live-tier re-check — two disabling impacts in one tick resolve once): Cruise → `ResetVehicle` to spawn + trailers + repair, Circuit → in-place reset + `clock += DISABLED_PENALTY_TICKS` + repair, Blitz/Checkpoint/CrashCourse → the session's own `restart` intent (production teardown + re-`begin`, the documented "restart the event"), AI opponents → in-place reset + repair under every mode (designed, UNK-13), remote participants skipped (F25+). Player + opponent spawns attach `VehicleDamage` only when `vehcardamage` decoded — authored absence stays undamageable. `drive_session` resets the report on teardown. Tests: +2 mm2_game (watermark/dup+spec-wrap legs), +11 mm2_app (sub-threshold/garbage/undamageable/stale-generation negatives, tier+event stream, dup-suppression, cruise/circuit/blitz outcomes, AI outcome, one-tick double-disable, real roof-drop end-to-end). Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`): sf `--frames 600` → `dmg=2a/0d/0r rej=1 dup=0`, london → same — the scripted cruise's two real hits apply through the pipeline; all other counters bit-identical to B.8. Open: visual tiers/breakaway/impairment/water/stuck-gyro consumption/C&R healing/replication per F05-B row. Candidate pending external check. |
| F05-B.2 | implemented | F05-B.1 | Authored `vehstuck` detection + bounded in-place recovery. `mm2_game::stuck`: `StuckSpec` (six authored fields verbatim — `rotation`/`translation` decoded, not consumed, UNK-13), `VehicleStuck` component (spawned only when `vehstuck` decoded — designed interpretation of MM2Hook's recovered `vehStuck` struct: `ImpactEvent` anchors `m_LastImpactPos`, `pos`/`move` thresh hysteresis, `turn` tumbling leg re-anchors the settle window, `time_thresh` fires `StuckVerdict::Stuck` once), `StuckEvent` (object/generation/tick — bounded, one per episode). `mm2_app::stuck`: `track_stuck` (FixedLast, authority+Playing gated, drains stale) arms off the deduped impact stream and advances detectors — `StuckReport` (`vsk=` smoke field) counts armed/detections/recovered; `resolve_stuck` answers detections with `ResetVehicle` onto the shared `upright_recovery_pose` (heading kept, hull on resting surface — in place is the authored reading: `Rotation` 0, `Translation` ≈ 0.1), trailers re-seated at authored offsets, `Teleported` so no checkpoint sweep; local+AI identical (designed), remote skipped (F25+), `Disabled` wrecks skipped on arm+observe and `resolve_disabled` disarms (`track_stuck` scheduled before the outcome so the skip sees the pre-repair tier). `mm2_content::convert` feeds authored `TimeThresh` to `assists.self_right_delay` — the modern assist keeps covering impact-free rollovers. Tests: +9 mm2_game (arm/hysteresis/turn-leg/fire/one-shot/re-anchor/garbage), +11 mm2_app (pipeline arm→detect→recover, drive-away escape, unimpacted, roofed/wedged righting, AI + trailer + remote + disabled + stale-generation negatives, real 4 m roof-drop end-to-end). Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`): sf + london `--frames 600` → `vsk=3a/0d/0r` — impacts arm, none persist under the moving scripted driver; all pre-existing counters bit-identical. Open: `vehgyro`, water/OOB recovery, visual/impairment legs per F05-B row. Candidate pending external check. |
| F05-C | queued | F05-B | — |
| F06-A | implemented | F00-B, F01-B | A.1 (parser + audit — below) + A.2 (runtime identity — externally checked at `d292898`): `emit_psdl` splits colliders per authored material index; `SurfaceMaterial` on every collider; `SurfaceTables` session-scoped; `CityReport.surfaces` diagnostics. |
| F06-A.1 | implemented | F00-B, F01-B | `mm2_formats::materials`: `MaterialSet` (line-oriented `mtl <name> { key: v… }` blocks; brace may sit on next line, `:` optional, `//` comments; all ten retail fields preserved + typed accessors) and `MaterialMap` (`texture,physics` csv; `none` keyword, header recorded, short/extra-cell rows into `diagnostics`), each with `validate()` issues (duplicate/missing/bad/negative fields, missing `_default`, dup rows, bad header) + `undefined_refs` cross-check. `mm2_formats::tex::frame_base_stem` shares the `<stem>-NNNN` animated-frame convention (s_thames-0009 → s_thames). `mm2-inspect materials <install> [--city] [--strict]`: expected `city/materials.{csv,mtl}` + every discovered `.mtl`/`materials*.csv`, csv→mtl ref check, texture-file resolution split (semantic-only stems informational), and per-city PSDL texture-table coverage (named/`none`/blank-slot/unmapped, frame-stem fallback). Retail: 3423 rows (137 named, 3286 `none`), 8 materials, 2 dead refs (`transbay_ramp_f→ash`, `s_grass2mud→mud`) — `--strict` exits 2; london 469 names = 152 named + 308 none + 6 blank + 3 unmapped, sf 457 = 148 + 301 + 6 + 2. `scan` recognizes `.mtl`. docs/research/materials.md + WLD-19/UNK-23. Runtime lookup/traction unwired — the parsed pair is not yet a `SurfaceMaterial` source. Candidate pending external check. |
| F06-B | active | F06-A | Traction leg (checked at `7bfbba8`) + F06-B.2 authored-physicals leg (below): `TireSurface` gains `drag` (raw authored value — per-wheel viscous wading resistance, outside the ellipse); `contact_restitution` scales `elasticity` ×0.1 → Avian `Restitution` on named colliders; `WheelState.surface_drag`; AC03 mod-override swap test (cosmetic texture replacement leaves material identity + physics components identical). Retail: Thames spawn `peak=1.1m/s` vs unchanged 29.0 land baseline; restitution shifted two recorded banger scenarios (annotated in `docs/research/banger.md`). Remaining: AC05 audio/dust consumer consistency (F07 scope), AC06 network authority (F24 scope). Candidate pending external check. |
| F06-C | queued | F06-B | — |
| F07-A | queued | F01-B, F02-B, F06-A | No audio decoders/voices; `aud/` family = 3293 files incl. cardata/dmusic/spchdata. bevy built without `bevy_audio`. |
| F07-B | queued | F07-A | — |
| F07-C | queued | F07-B | — |
| F08-A | queued | F01-B, F07-A | — |
| F08-B | queued | F08-A | — |
| F08-C | queued | F08-B | — |
| F09-A | implemented | F00-B, F01-A | Split into A.1 (BAI parser + audit — externally checked) and A.2 (`.aimap` parser + audit — implemented below). Parse-level work done; route queries/semantics continue under F09-B. |
| F09-A.1 | implemented | F00-B, F01-A | `mm2_formats::bai`: `CAI1` parser — roads (per-side lane/sidewalk/rail curves, rooms, half-width, base speed, flags, per-section frames), intersections (room, center, counterclockwise road refs), per-room large/small culling lists. Measured layout correction vs the R3 doc: the `[lanes+sidewalks][sections]` distance matrix precedes the per-curve edge distances (`docs/research/bai.md`). `Bai::validate()` reports `BaiIssue` diagnostics: duplicate ids, unknown flag/ambient/rule codes, dangling/mismatched end↔intersection back-refs, room-0 refs, dangling culling refs, <2 sections, non-finite curve vertices (added in the F10-A.1 review repair). `mm2-inspect bai <install> [--city] [--strict]`: expected = `city/{london,sf}.bai`, every other `city/*.bai` audited as an extra, room refs cross-checked against the same-stem PSDL. Retail: both expected files parse byte-exact, validate clean, rooms in range; `_bak` copies clean; `sfai.bai` parses with 1 authored anomaly (intersection room ref 0 — reported); `_sup` files share CAI1 magic but don't fit the layout → `unsupported` (not hidden). `--strict` exits 2 (2 issues, all on the sfai dev-map extra). `.bai` added to `scan` recognized formats — `_sup` files now appear as honest parse failures there. Candidate pending external check. |
| F09-A.2 | implemented | F00-B, F01-A | `mm2_formats::aimap`: INI-like parser measured on all 209 retail files — `#` comments, `[Section]` headers, scalar vs counted-list bodies (exact-count on retail), free-form `[Traffic Lights]`, unknown sections preserved verbatim in `unknown_sections`. Typed records keep undocumented numeric tails raw (`PoliceRecord.params`, `OpponentRecord.params` — two retail shapes each); malformed rows → `diagnostics` + skip. `Aimap::validate()` reports `AimapIssue`: duplicate exception roads, ambient-weight range/monotonicity/1.0-closure, non-0/1 flags, negative scalars, uninterpreted sections. `mm2-inspect aimap <install> [--city] [--strict]`: expected = `city/<stock>.aimap` + every discovered `race/<stock>/*.aimap{,_p}` (209/209 resolve+parse), extras audited as unsupported-on-failure; cross-checks exception road ids vs same-city `city/<city>.bai` and opponent `.opp` refs via VFS. Retail: 9 issues — 8 london files with exception ids 562–815 beyond the 540-road BAI (UNK-18/WLD-8, reported not repaired), stunt0's `opp-c0.2` ref dead. `--strict` exits 2. `.aimap`/`.aimap_p` added to `scan` recognized formats. docs/research/aimap.md + ledger WLD-6/7/8, UNK-12/18. Candidate pending external check. |
| F09-B | queued | F09-A | Split into B.1 (nav graph + audit — implemented below) and B.2 (debug overlays + deeper retail route validation). Parent AC04 stays open until B.2. |
| F09-B.1 | implemented | F09-A | `mm2_game::nav::NavGraph::build(&Bai)`: directed arcs (right-side curves travel with sections, left-side against — London left-hand is authored data per the Adzima article, no handedness flag), vehicle/sidewalk/tram/train lane records ranked by measured lateral offset (`edgeDistances` is not an ordering — UNK-19), end→intersection resolution with dead-end degradation + `NavIssue` reporting, turn connections minus U-turns, union-find components, XZ grid. Queries: `sample_lane`, 3D `nearest_lane` (elevation-aware; `rooms` hint for stacked geometry), `legal_exits` (documented lane-position rules; geometric turn class + authored `ccw_delta`), seeded `choose_exit`, bounded A* `route` (`closed_roads` hook for aimap `[Exceptions]`; specific `RouteError`s), per-consumer `RouteCursor`s. `mm2_content::nav::load_nav_graph` (VFS→`Bai`→graph). `mm2-inspect nav <install> [--city] [--strict] [--route from:to]`. Retail: London 540 roads→606 arcs/1141 vehicle+1080 sidewalk+28 rail lanes/328 ints/0 dead ends/1 component; SF 379→618/1212+758+42/214/1/1; 176 non-routable roads reported; routes resolve (sf 13→50 = 4 steps/576 m). AC01/AC03/AC05/AC06 satisfied at synthetic level (20 tests); AC04 open → B.2. Candidate pending external check. |
| F09-B.2 | implemented | F09-B.1 | `NavOverrides` distils a parsed aimap (zero-density `[Exceptions]` → `closed_roads`, `[Speed Limit]` → per-road/file-default speed lookup) with `route_options()` feeding the existing `closed_roads` hook. `NavGraph::route_roads` road-index probe (anchors on the first arc's first lane midpoint — a centreline anchor sits equidistant between directions and can snap the dead-end-facing lane). `mm2_content::load_nav_overrides` (absent → `None`, malformed → error). `mm2_app::nav_overlay`: session-scoped `CityNav` (graph + issues + overrides + probe), `overlay_lines` pure segment builder (8 classes: fwd/bwd lanes, sidewalk, rail, direction chevron, closed, route, intersection), `draw_nav_overlay` gizmos, `hud_summary`; `--nav`/`--nav-route` CLI; HUD + `smoke=` `nav=` fields; resource removed on session teardown. `mm2-inspect nav --turns` + aimap-applied `--route`. Retail: probes reproduce baseline exactly; 4-way Δccw↔geometry agreement 95.7% London/99.2% SF (`docs/research/bai.md`). Rendered captures on both cities (local, not committed). Tests: +2 game, +5 content, +7 app. AC04 candidate pending external check. |
| F09-C | implemented | F09-B | Split: C.1 (directed route-constraint validation — implemented below). AC01/AC03/AC05/AC06 synthetic since B.1 (+2 new tests), AC02 via the bai/aimap audits, AC04 via B.2 rendered captures; the remaining AC evidence levels are per-slice. |
| F09-C.1 | implemented | F09-B | `NavGraph::reachable_arcs(start, closed_roads)` — bounded BFS over authored `exits` (start included even when closed; closed roads never entered through a turn, matching `route` semantics). `mm2-inspect nav`: `--routes <n>` runs a per-arc directed-reachability census (full-reach count, reach-1 arcs annotated dead-end vs no-legal-continuation, unreachable ordered pairs, smallest sources) plus `n` seeded `route_roads` probes — endpoints must land on the asked roads, consecutive steps must share a turn, interior arcs must not sit on a closed road; failures print named pairs, expansion-limit/violations count toward `--strict`. `--aimap <logical>` substitutes any aimap's overrides (must resolve → exit 2). Retail (`fnv1a64:e91e6cd4b2ae30d9`): london strongly connected (606/606 arcs, 512/512 probes ok); SF 4927 unreachable ordered pairs (~1.3%) — authored one-way traps, not invented links (0 violations; roads 92/94/97/99 reach 1, 102/111 reach 2, road 0- is the dead end). `race/london/blitz0.aimap` (8 closed) → 15016 unreachable, 24/512 probes unreachable (`270→108`: 19 steps open → `Unreachable` closed); `race/sf/blitz0.aimap` (3) → 7977, 11/512. WLD-11 extended, `docs/research/bai.md` updated. Tests +2 in `mm2_game/tests/nav.rs` (24 total). Candidate pending external check. |
| F10-A | active | F01-B, F02-A, F09-B | Split into A.1 (catalog + planner) and A.2 (session-scoped runtime — implemented below). Remaining: intersection controller/signals/right-of-way, queueing/obstruction/stuck handling, collision-response fidelity, spawn-vs-spawn overlap rejection, verified original population/bubble constants (UNK-12), F10-AC evidence vs the spec. |
| F10-A.2 | implemented | F10-A.1 | Runtime consumer of `plan_ambient`. `mm2_game::traffic`: `eligible_lanes`/`draw_spawn` extracted for planner+respawner reuse; `LaneCursor`/`advance_lane_cursor` — seeded legal-exit turns through new `NavGraph::transfer_lane`, closed-road exits skipped, lane rank preserved, explicit `DeadEnd`. `mm2_content`: `opponents::event_aimap` (difficulty selection shared with `opponent_roster`), `ambient_setup` (event roster replaces city's; overrides merged — closed union, event exceptions first; event speed-limit/left-drive wins), `assemble::ambient_vehicle` (pkg+mtx+bound; aivehicledata not a VehicleConfig — no drivetrain). `mm2_app::traffic`: `AmbientTraffic` resource + `AmbientCar`; `load_ambient_traffic` spawns session-owned kinematic bodies (bound hull / Size-box collider, real `va_*` model) on planned poses; `drive_ambient` lane-follows in FixedLast with per-road speed refresh on turns and dead-end despawn; `maintain_ambient` recycles outside the bubble and respawns to target (bounded attempts). `load_session_world` layers event aimap over city, inserts/removes the resource with the session; `traf=` headless field (absent without a roster). Synthetic-install app tests (real session path): seeded placement + replay, drive/dead-end/recycle, event `[Density] 0.0` authors off, teardown/replan. Retail sf headless: `traf=16/16 sp=140 rec=124 dead=0 uns=0`, 1212 eligible lanes. Kinematic followers only — no controller/collision claims; candidate pending external check. |
| F10-A.1 | implemented | F01-B, F02-A, F09-B | `mm2_formats::veh::AiVehicleData` — typed `aiVehicleData` decoder (Mass/Size/MaxAng/Elasticity/Friction/MaxDamage/PtxThresh/Spring/Damping/Limit/RubberSpring/RubberDamp + optional CG; MSVC `1.#QNAN0`/`-1.#INF000` literals decode to non-finite, malformed optional vectors warn not zero). `mm2_game::traffic`: `AmbientSpec`/`AmbientRoster` (authored cumulative-weight table — duplicate ids legit, e.g. london `va_compact_s` on two bands), `SpawnPolicy` (designed pool/distance bound — original values UNK-12), `plan_ambient` seeded planner: density-scaled target, cumulative pick, position over `NavOverrides`-open routable vehicle lanes, bounded min-player-distance retries, unresolved-class draws drop + report once per id (never rebalanced). `NavRng::next_f32` added. `mm2_content::traffic`: `ambient_roster`/`ambient_roster_from_aimap` (event files carry their own tables — `race/london/roam.aimap{,_p}`/`roambak` measured), `EXPECTED_AMBIENTS` (23 records), `TrafficAudit::scan` — per-class `aivehicledata`/`pkg`/`bnd` + `.mtx` count, unrostered ids, undiscovered expected ids, event `aimap{,_p}` override census. `mm2-inspect traffic <install> [--city] [--strict]`. Retail (`fnv1a64:e91e6cd4b2ae30d9`): london 23 discovered/12 rostered/11 unrostered, sf 23/11/12, all rostered assets ok, 0 diagnostics; 101 london + 104 sf event overrides (35/53 exceptions, 3/6 density, 3/0 own rosters); strict exits 0. WLD-20 added, UNK-12 narrowed, docs/research/aimap.md extended. Tests: +6 formats, +8 game, +3 content. No runtime spawning yet — candidate pending external check. External review #12 failed on one panic: `plan_ambient` swept a NaN-length lane (BAI vertices are raw f32 bits, unchecked) into `sample_storage`'s `s.clamp(0.0, lane.length)`, which panics on a NaN bound; a +inf length was the sibling NaN-position case. Repaired: `push_lane` now requires finite authored distances (else recompute), finite vertices and finite length, dropping offenders with the new `NavIssue::NonFiniteLane`; `plan_ambient`'s eligible-lane filter re-checks finiteness; `Bai::validate` reports `BaiIssue::NonFiniteCurveVertex` per curve; malformed `CG` warns like `MaxAng` (shared `opt_vec3`) and the "25 retail records" doc corrected to 23. Tests: +1 game (`plan_skips_non_finite_lane_geometry`), +1 formats bai (`validate_flags_non_finite_curve_vertices`), +1 leg in `aivehicledata_tolerates_absent_cg_and_flags_garbage`. |
| F10-B.1 | implemented | F10-A.2 | Ambient obstruction response. `mm2_game::traffic`: `FollowPolicy` (designed — original braking unverified, UNK-12), `corridor_gap` (nearest blocker in a forward corridor: XZ heading, half-width, vertical tolerance, speed-scaled reach), `follow_speed` (clear → road limit; blocked → brake to `follow_gap`; `panic_gap` → instant stop; `turn_speed` cap on intersection turns). `mm2_app::traffic`: blocker list = every `Player` participant + ambient cars; `queued` counter; `maintain_ambient` phase-gated like the driver; `issues` warn-logged; `LinearVelocity` now the intended `tangent * speed` (measured-delta feedback diverged kinematic bodies to ~km/s — found by the new pause test). `draw_spawn` takes `&SpawnPolicy` and places inside the `[min, recycle]` annulus (`OutOfBand` both bounds). `mm2_content`: `opponent_roster_from_aimap` splits the roster build so `event_race_setup` parses the aimap once. `q=` in `traf=` smoke. Retail sf: `traf=16/16 sp=26 rec=10 dead=0 uns=0 q=0` (was `sp=140 rec=124`). Tests: +3 game (annulus, corridor, follow law), +3 app (parked hold/release, two-car queue, pause freeze); fixture PSDL widened — the player had no ground. Open: no controller/signals/right-of-way/stuck recovery, no occupied-space checks at spawn or junction transfer, AC02/AC03 unmet. Candidate pending external check. |
| F10-B.2 | implemented | F10-B.1 | Authored junction rules + occupied-transfer check. `mm2_game::traffic`: `JunctionPolicy` (designed values, UNK-12), `JunctionGate`, `Junctions` — `approach` maps a lane end to `(junction, road, rule)`; `signal_members` builds each junction's light-controlled road set in authored ccw order; deterministic member phase (green + all-red clearance, `PHASE_SPREAD` per-junction desync); stop-sign FCFS queue gated on stand-at-line + dwell; `AlwaysStop` never opens; `junction_speed` decel ramp to the `stop_inset` line; `depart`/`retain` shed stale entities. `mm2_app::traffic`: per-car gate in `drive_ambient`, closed-gate ds-clamp at the line, `Turned` landing occupancy check within `enter_clearance` reverts to the lane end, `junction_held` counter → `jq=` smoke field. Fixture parameterises `vehicleRule` per end. Retail census in `docs/research/bai.md` (SF 449/138/30 over 212 junctions — 82 lit/92 mixed/0 all-stop; London 369/214/23 over 254). Retail headless: sf `jq=5`, london `jq=3`. Tests: +4 game (rule binding, FCFS+dwell, member cycle+all-red, ramp), +5 app (stop-sign serialisation order, transfer-only-on-green, red-window hold/release, AlwaysStop vs NeverStop mixed junction, occupied-exit hold/release). Open: no yielding to crossing traffic inside the box, signal props unrendered, original timings unverified. Candidate pending external check. |
| F10-B.3 | implemented | F10-B.2 | Bounded stuck recovery (spec req 3; AC05's stuck-outcomes leg). `mm2_game::traffic`: `StuckPolicy` (`window_ticks` 4800 = 40 s @120 Hz, `min_displacement` 4 m — designed, UNK-12; sized past the worst legitimate wait this controller imposes, a 4-member signal's ~2520-tick red) + `StuckWindow` (anchor + still-ticks; ≥`min_displacement` progress re-anchors, non-finite poses reset). `mm2_app::traffic`: `AmbientCar.stuck` seeded at spawn; `drive_ambient` expires the window → despawn + `junctions.depart` + `traffic.stuck += 1` — the recovery is removal into the pool `maintain_ambient` refills, never a teleport through the pen. `stuck_policy` is pub for test/evidence binding; `stuck=` joins the `traf=` smoke record. Tests: +1 game (`stuck_window_resets_on_progress_and_expires_stationary`) +2 app (`a_penned_car_is_recycled_after_the_stuck_window` — a parked participant pens two followers in a row, each despawns after the bound having never closed inside 4 m; `a_signal_wait_shorter_than_the_window_never_recovers` — a 1200-tick window over a ≤360-tick red, crosses on green, `stuck=0`). Retail (install `fnv1a64:e91e6cd4b2ae30d9`): sf `--frames 600` → `traf=16/16 sp=23 rec=7 dead=0 uns=0 q=0 jq=5 stuck=0`, london → `…sp=21 rec=5… jq=3 stuck=0`; sf `--frames 3000` (6000 ticks > window) → `sp=37 rec=21 stuck=0` — no organic pen in a moving-player cruise, so the recovery is synthetic-verified only. Open: multi-cycle queue tails at heavily loaded signals can outwait the window (designed sacrifice); despawn near the player is a visible pop; original stuck behaviour unverified (UNK-12). Candidate pending external check. |
| F10-B.4 | implemented | F10-B.3 | Spawn occupied-space rejection (F10-AC04's spawn leg). `mm2_game::traffic`: `SpawnPolicy` gains a designed tangent-aligned exclusion box — `spawn_clearance` 5 m longitudinal (~a car length), `spawn_half_width` 2 m (an adjacent lane never blocks), `spawn_max_rise` 3 m (an overpass doesn't occupy); `spawn_occupied` box check mirroring `corridor_gap`'s projection; `draw_spawn` takes `occupied: &[[f32; 3]]` and returns `SpawnDraw::Occupied` before the class pick; `plan_ambient` accumulates placed positions so its own directives keep clearance (rejections land in `dropped`). `mm2_app::traffic`: `maintain_ambient` builds the occupied set from surviving ambient cars + every `Player` participant (local driver and AI opponents alike — opponents were never covered by the player bubble) and feeds each same-tick placement into it (Commands-deferred spawns are query-invisible); `AmbientTraffic::policy` is pub for test/evidence binding. Tests: +4 game (`spawn_occupied_boxes_the_sample_tangent`, `draw_spawn_rejects_occupied_space`, `plan_keeps_same_lane_spawns_clear_of_each_other`, `plan_drops_directives_past_a_lane_s_capacity`) +2 app (`respawns_reject_space_a_live_car_occupies`, `respawns_reject_space_a_participant_occupies` — each vetoes every refill under a fixture-wide box, then a default-box refill proves the respawner still works). Retail (install `fnv1a64:e91e6cd4b2ae30d9`): sf `--frames 600` → `traf=16/16 sp=23 rec=7 dead=0 uns=0 q=0 jq=5 stuck=0`, london → `…sp=21 rec=5… jq=3 stuck=0` — records unchanged; the initial plan now drops saturated draws honestly (sf spawns 12/16 at load, london 14/16 — the maintainer refills both). Original spawn-overlap behaviour unverified (UNK-12); props/static-geometry occupancy out of scope. Candidate pending external check. |
| F10-B.5 | implemented | F10-B.4 | Junction-box yield (F10-AC02's right-of-way leg): an admitted approach must still wait while the physical box is occupied. `mm2_game::traffic`: `JunctionPolicy` gains `box_margin` 3 m/`box_max_rise` 3 m (designed, UNK-12); `junction_zone` derives centre (authored `center`, endpoint-centroid fallback on non-finite) + XZ radius (farthest member end + margin) from `NavGraph`, `inside_junction_zone` adds the vertical band; `Junctions::gate` takes `box_occupied` and closes a green member or stop-sign FCFS head while it holds — `NeverStop`/unruled ends keep free flow, `AlwaysStop` stays closed. `mm2_app::traffic`: `drive_ambient` pre-passes `bound_for` (entity → approach junction) and reports occupancy from the corridor blocker set (ambient cars + all `Player` participants); blockers bound for the same junction are excluded so two approaches waiting at their own lines cannot deadlock, participants always count. Tests: +3 game (zone geometry/offset-centre/non-finite fallback/vertical/out-of-range, green-member yield+release, stop-sign-head yield vs NeverStop/AlwaysStop) +2 app (participant-in-box holds a lit approach through green, ambient-car-in-box holds the FCFS head — both release on despawn). Retail (install `fnv1a64:e91e6cd4b2ae30d9`): sf `--frames 600` → `traf=16/16 sp=23 rec=7 dead=0 uns=0 q=0 jq=5 stuck=0` unchanged, london → `…sp=20 rec=4… jq=4` (expected drift — an approach now stands an extra green). Open: point-occupancy not hulls, props/geometry excluded, committed-turn/same-tick-release convergence approximate, collision fidelity (AC03), original box/yield rules unverified (UNK-12). Candidate pending external check. |
| F10-B.6 | implemented | F10-B.5 | Kinematic→dynamic collision handover (spec req: "transition to dynamic behaviour without duplicating bodies or injecting extreme energy"; F10-AC03's collision leg). `mm2_game::traffic`: `KnockPolicy::min_impulse` 4000 N·s (designed — original ambient crash rules unverified, UNK-12). `mm2_app::contracts`: `impulse_estimate` (approach speed × striker mass, `ComputedMass`, 1 kg fallback) moved out of `banger.rs` so handover and banger activation share one estimate. `mm2_app::traffic`: `AmbientDrive::{Lane,Knocked}` on `AmbientCar`; ambient spawns add authored `Mass`/`CenterOfMass` (finite-or-fallback) + `CollisionEventsEnabled`; `knock_ambient` — third `CollisionStart` consumer, same authority/phase gate and message drain as `drive_ambient` — flips the same entity to `RigidBody::Dynamic` on a qualifying contact (hull/pose/lane velocity preserved, bounded contact-normal kick ≤ approach speed), `junctions.depart`s it, counts `traffic.knocked` → `kn=` smoke field (printed only when nonzero). `drive_ambient` skips `Knocked` cars (solver-owned) and `bound_for` maps `Lane` cars only, so a wreck inside the junction box legitimately occupies it. Tests: +3 app (`a_hard_hit_hands_the_follower_to_dynamics`, `a_light_touch_leaves_the_car_lane_following`, `a_knocked_wreck_occupies_the_junction_box`). Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`): sf `--frames 600` → `traf=16/16 sp=23 rec=7 dead=0 uns=0 q=0 jq=5 stuck=0 kn=1`, london → `…sp=20 rec=4… jq=4 stuck=0 kn=1` — one real handover per city, all other counters at their B.5 values. Open: wreck-vs-vehicle contact needs the striker's `CollisionEventsEnabled` (ambient cars now carry it, so wreck-wreck and prop-fragment strikes register); impulse-estimate semantics shared with DSN-10 (provisional); knock direction is a point-normal kick, not per-contact impulses; no rendered/manual evidence; original crash behaviour unverified (UNK-12). Candidate pending external check. |
| F10-B.7 | implemented | F10-B.6 | Authored traffic-signal indicators (the "signal-prop rendering" line). `mm2_formats::bai` already parsed `trafficLightOrigin`/`trafficLightAxis`; `mm2_game::nav` now surfaces them twice — `NavArc::exit_light` (the head governing an approach) and `NavGraph::signals`/`EndSignal` (the full authored set: every nonzero finite origin on a resolved end — the census proved arc-exit-only traversal would miss 388 London / 128 SF heads on one-way upstream ends and arc-less roads). `mm2_game::traffic`: `SignalAspect` + `Junctions::signal_aspect` — `gate`'s rule admission minus the box-yield (green while the road holds the phase, red out of phase/all-red, constant green on `NeverStop`/unruled/non-member ends, `Stop` on stop-signed ends, red on `AlwaysStop`). `mm2_app::traffic`: session-owned `TrafficSignal` lamps (unlit sphere at the authored anchor, shared mesh/aspect materials — no `vasignalunit` geometry ships, textures only) spawned at load inside a designed 60 m `SIGNAL_MAX_DISTANCE` sanity bound (`signals_dropped` counts outliers); `drive_signals` runs after `maintain_ambient` in every FixedLast chain, swapping material only on aspect change; `sig=`/`sigd=` smoke fields. Tests: +3 game (verbatim marker extraction/zero/non-finite, full authored-set coverage, aspect-vs-phase sweep) +3 app (spawn-at-origin+teardown, phase tracking with never-two-greens + all-red, outlier drop counted). Retail (install `fnv1a64:e91e6cd4b2ae30d9`): sf `--frames 600` → `sig=647 sigd=3`, london → `sig=828` — matching the `mm2-inspect bai` census exactly; rendered screenshot of a lit SF junction lamp captured locally (not committed). Open: `trafficLightAxis` unused (convention unverified), indicator geometry/aspect mapping designed not original, no signal-pole prop model, AC02's signal leg remains presentation-only. Candidate pending external check. |
| F10-B.8 | implemented | F10-B.7 | Union of player interest areas (spec req 2's multiplayer leg; AC04's "near any player"; "two players far apart" edge). `mm2_game::traffic`: `plan_ambient`/`draw_spawn` take `interest: &[[f32; 3]]` instead of one `player_at`; new `in_spawn_band` (inside at least one area's `recycle_distance` *and* outside every area's `min_player_distance` — a car can never materialise next to anybody) and `within_interest` (the any-bubble survival test) carry the shared semantics. `mm2_app::traffic`: `maintain_ambient` builds the set live from every `Player` participant's `Position` each tick — local driver, remote drivers, AI opponents (designed composition — each can hold corridor/box/collision interactions); a car despawns only past *every* area's radius and respawns draw inside the union band; the `PlayerVehicle`-only query is gone, so a participant-less session freezes rather than draining. `load_ambient_traffic` takes `interest: &[Vec3]` (the local spawn alone at load — all participants stage on one grid). Tests: +2 mm2_game (predicate legs — per-area admission, min-distance veto across areas, empty/non-finite sets, recycler any-bubble; plan level — union populates through the covering area, empty set spawns nothing) +2 mm2_app (a remote player's bubble holds the population the teleported-out local player left — `recycled=0`, control run collects all; respawns draw only inside the remote band). One review-shaped repair: `respawns_reject_space_a_participant_occupies`'s mid-network participant now legitimately vetoes every refill through the union min-distance — the fixture participant moved to 150 m south (inside the union, past every lane's exclusion) so the widened box remains the rejecting mechanism. Retail (install `fnv1a64:e91e6cd4b2ae30d9`): sf `--frames 600` → `traf=16/16 sp=23 rec=7 dead=0 uns=0 q=0 jq=5 stuck=0 kn=1 sig=647 sigd=3`, london → `…sp=20 rec=4… jq=4 stuck=0 kn=1 sig=828` — both bit-identical to B.7 (single player → single area, by construction). `sf checkpoint:0 --bot --frames 1200` → `traf=3/3 sp=5 rec=2` with opponents spreading (pre-existing "fell through the world" sanity-fail on this event unchanged — documented bot-limited SF defect). Open: interest composition (AI areas count) is designed not original; hysteresis still implicit (spawn outer bound = recycle radius); interactions-preservation beyond distance (e.g. a mid-pileup wreck outside all bubbles) not special-cased. Candidate pending external check. |
| F10-B.9 | implemented | F10-B.8 | Junction-interior crossing paths (operator report 4 item 1 — the visible teleport). `mm2_game::nav`: `CrossingPath` (resampled cubic Hermite, shared `sample_polyline`) + `NavGraph::crossing_path(from,to)` — endpoint poses sampled in travel direction (`from` at `length`, `to` at `0`), chord-scaled tangents, `CROSSING_MIN_GAP`/`CROSSING_STEP` bounds, `None` on coincident endpoints so direct transfers stay direct. `orient_curve` normalizes vertex-reversed lane curves to section order at build — the `nav --gaps` census exposed them (raw median transfer chord 92 m SF/51 m London, max 1643 m — a reversed lane's travel end sits a road-length from its exit junction; after normalization medians ~20-26 m, max 87 m); regression test `authored_reversed_lane_vertices_normalize_to_section_order`. `mm2_game::traffic`: `LaneCursor.crossing` + `Crossing` (path, covered distance, destination); `LaneAdvance::{Along,Entered,Crossing,Landed,DeadEnd}`; `advance_lane_cursor` commits a crossing only when the gate admits, traverses it inside the same call, reports `Entered` even when the step also lands. `mm2_app::traffic`: crossing traversal in `drive_ambient` (gate skip while crossing, turn cap, landing occupancy check, `bound_for` keeps the approach junction), `crossings`/`jumps` counters + pose-continuity watchdog → `crx=`/`jmp=` smoke fields. Tests: +5 mm2_game (path endpoint continuity, monotonic no-jump samples, commit-then-land, whole-interior-in-one-step still reports the commit, lane-rank preservation) +1 mm2_app (`a_lane_follower_crosses_the_junction_interior_without_a_jump` — car observed inside the box, max per-update move <1 m, `crossings≥1`, `jumps=0`); `a_traffic_light_admits_only_the_green_member_road` re-keys "crossed" on commit not landing (the landing can fall in a later phase). Retail (install `fnv1a64:e91e6cd4b2ae30d9`): sf `--frames 3000` → `traf=16/16 sp=43 rec=27 dead=0 uns=0 q=0 jq=3 stuck=0 crx=59 jmp=0`, london → `traf=16/16 sp=22 rec=6 dead=0 uns=0 q=0 jq=2 stuck=0 crx=68 jmp=0`. `mm2-inspect nav --gaps` added for the census. Open: generated geometry is an implementation choice — original runtime crossing paths unrecovered (UNK-12); no windowed playtest yet. Candidate pending external check. |
| F10-B.10 | implemented | F10-B.9 | Startup WARN aggregation (operator report 4 item 5). Two per-entry WARN classes were expected authored-data anomalies already counted elsewhere: `NavIssue::NoVehicleLanes` per road in `load_ambient_traffic` (154 london / 22 sf — pedestrian/special/disabled roads, WLD-11) and `walk_prop_rules`'s per-entry strings in `load_city` (64 sf — the 0xx-encoded `road_rooms` records already counted as `bad_refs`; london authors none). New `partition_nav_issues` routes the bulk class to one DEBUG line (count + road list) while every other plan/nav issue kind keeps its individual WARN; the walk strings collapse into one DEBUG line (count + the bounded list). Counts unchanged — `issues=` in the ambient summary, `proprule_issues` + `bad_refs`/`unreached` in the stamped summary; per-entry detail still in `mm2-inspect nav`/`placement`. Tests: +2 `mm2_app` unit (partition keeps the warn list ordered and complete). Retail (`fnv1a64:e91e6cd4b2ae30d9`, `--headless --frames 60`): london 155 WARN → 1 (the genuine `p_parkmeter_f.tex` mip warning); sf 87 WARN → 1 (same); `RUST_LOG=mm2_app=debug` shows both summaries — london `count=154` matching the reported `issues=154`, sf `count=22` + walk `count=64`. Candidate pending external check. |
| F10-B | active | F10-A | B.1 obstruction response + B.2 authored junction rules + B.3 bounded stuck recovery + B.4 spawn occupied-space rejection + B.5 junction-box yield + B.6 collision handover + B.7 authored signal indicators + B.8 union of player interest areas + B.9 junction-interior crossing paths + B.10 startup WARN aggregation implemented above. Remaining: queue-through-intersection priority, collision fidelity vs AC03's full checklist (player-hit feel, damage), original junction/spawn timing and interior-crossing geometry (UNK-12), signal-prop model fidelity (real unit geometry — none ships; axis convention unverified), F10-AC evidence vs the spec. |
| F10-C | queued | F10-B | — |
| F11-A | checked | F00-B, F01-A | `mm2_formats::racefiles` shared classifier (was private to `mm2-inspect` inventory); new parsers `waypoints` (waypoint + `_strtpnts` CSVs), `opp`, `crashdata` (tolerates retail `AmbDenisty` typo / omitted `Filename` label / named tail columns — kept as diagnostics), `rewards`. `mm2_content::EventCatalog`: VFS scan per city, `mm*data.csv` rows → `EventRef`-keyed entries (ready/incomplete + failed refs), dep records attached by stem, Crash Course `Filename` links resolve whole linked stems, rewards + milestone rewards linked, extras listed. `mm2-inspect events <install> [--city] [--strict]` — strict exits 0 on retail: 45/45 events ready per city. Producer correctly placed in `mm2_content` after the iteration-010 review rejection. Externally checked (review pass at `bb56272`, iteration 011 feedback). |
| F11-B | active | F11-A | Split into B.1 (shared runtime contract + driver — checked) and B.2 (`mm2_content` catalog→`RaceDefinition` producer + event-session loading wiring). Parent AC05 (event-prop/traffic-override session scope) stays open — no traffic/prop-override systems exist to scope yet. |
| F11-B.1 | implemented | F11-A | `mm2_game::race`: `Checkpoint` swept cylinder test (XZ radius + ±height band + opt-in direction flag), `CheckpointRule` (`AnyOrder` documented BLZ-1/CHK-1 / `Ordered` documented CIR-1 — carried on the definition, not imposed), `RaceDefinition` (checkpoints/finish/start slots/laps/countdown + `validate`), `RaceState` (generation-stamped countdown→running→complete, `input_locked`, `is_stale`), `RaceProgress` (per-checkpoint cleared flags, ordered `next`/lap wrap, `break_segment` for teleport/reset, one segment consumes every checkpoint it crosses), `RaceStarted` message, `SessionOutcome::Finished{race_ticks}` on `SessionResult`. `mm2_app::race::advance_race` in `FixedLast` (post-solver `Position` segments): countdown→one `RaceStarted`+`Countdown→Playing`, clock+advance while `Playing`, `Finished` → mint+record `SessionResult` once into `ResultLedger`, all-finished → `Complete`; authority-gated (Remote never steps). Teardown: `drive_session` removes `RaceState`; `vehicle_input` honours `input_locked` (stale-gated); `Countdown` quittable/restartable. First candidate failed review on a real defect — `break_segment` had no production caller, so an R-key `ResetVehicle` teleport swept (and could finish) every checkpoint between the two poses; repaired with `mm2_vehicle::Teleported` stamped by `vehicle_reset` atomically with the `Position` write (chosen over draining `ResetVehicle` in the race system, which has message-lifetime and Update-ordering holes) plus `reanchor_teleported_participants` chained before `advance_race`; regression tests `vehicle_reset_breaks_the_swept_segment` + `reset_while_paused_cannot_sweep_checkpoints` fail on the old wiring. Tests: 11 contract + 15 production-path (AC02 high-speed/wrong-height/repeated/teleport + reset-via-message + paused-reset, AC03 countdown-once/pause-freeze/restart-removes-timer, AC04 once-only results with provenance, ties, remote-authority no-op, quit during countdown). Candidate pending external re-check. |
| F11-B.2 | implemented | F11-B.1 | `RecordContent` retains parsed payloads (`Waypoints`/`StartPoints`/`Opp`/`CrashData`). `mm2_content::race_def::race_definition` builds a `RaceDefinition` from a resolved `CatalogEvent`: Blitz/Checkpoint → `AnyOrder` (row 0 = start line, last row = finish trigger), Circuit → `Ordered` (rows 1.. + lifted line copy closes the lap, authored `NumLaps`); authored `w` radii; authored `_strtpnts` grids or a derived tangent start; Crash Course rejected explicitly. Catalog attributes SF's `cir<N>_strtpnts` siblings to `circuit<N>` (inferred alias, WPT-3). `load_session_world` event mode: catalog resolve → `Ready → Countdown`, spawn on the authored/derived slot, `RaceState`/`RaceProgress`/`ResultLedger` inserted (the ledger was unregistered — latent panic once any race ran), session-owned orange/green checkpoint/finish markers + `update_checkpoint_markers` (cleared gates hidden, finish revealed after all gates). `--event <table>:<index>` CLI; `headless_smoke` rewired onto the real session systems so `--event --headless` is production-path evidence. Retail: London `Blitz[0]` 3/3 gates swept (`race=Running` — finish not crossed in-window), SF `Circuit[1]` authored-grid spawn, `checkpoint:12` → clean `Failed` exit 3; screenshot shows gate column + `GET READY` countdown. 7 producer + 7 app event tests incl. a full synthetic course finish → one ledger result. AC05 event-prop/traffic scoping deferred (no such systems exist to scope yet). Candidate pending external check. |
| F11-C | active | F11-B | Split: C.1 (single-event dependency inspection — implemented below). Remaining: full-catalog strict audit evidence vs the retail fingerprint (the existing `events`/`race-defs`/`opponents` audits cover it — run-and-record leg), AC02–AC05 evidence promotion through the landed runtime slices, AC06's "loaded" leg is covered by the `mm2 --event` headless smoke records. |
| F11-C.1 | implemented | F11-B | `mm2-inspect event <dir> --city <stem> --event <table>:<row> [--strict]` — one authored event row resolved through the production `EventCatalog` and its whole dependency closure validated independently of the app (AC06's inspect leg): every attributed record's parse status, the aimap/pathset records the catalog leaves `Unparsed` deep-parsed + `validate()`d here, the production `race_definition`/`opponent_roster` builds at both difficulties via the now-public `audit_build`/`audit_roster` units the catalog-wide audits share, and wired vehicle ids cross-checked against `VehicleCatalog`. Unknown rows are a lookup error (exit 2); found-but-incomplete events still print the full record inventory so the missing piece is visible; `--strict` fails on incomplete status, failed refs/records, validation issues, failed builds, roster issues or unresolved vehicle ids. Retail (`fnv1a64:e91e6cd4b2ae30d9`): `sf circuit:1` reports 21 records, defs 10g×3/4lap, rosters 7opp/7rt and honestly surfaces the authored `circuit1-{a,p}-7.opp` unreferenced-route issue (strict exits 2); `london crash:0` deep-parses the linked `longjump.csv` and reports both builds `unsupported (F21)` without failure; `london blitz:9`/`blitz:0`/`sf checkpoint:0` (incl. the authored 7-vs-6 count anomaly) all resolve ready; `blitz:99` exits 2. Tests +7 (`mm2_inspect` 5 → 12): spec parsing, complete event, unknown/wrong-city lookups, incomplete, malformed aimap, unresolved wired vehicle, crash-unsupported. Candidate pending external check. |
| F12-A | checked | F02-B, F11-B | Authored Blitz `TimeLimit` binds per difficulty → 120 Hz ticks (seconds, provisional UNK-4); distilled `EventParams` (conditions/densities/actor counts) validated at build — out-of-range authored values are explicit `BadParam` errors. `advance_race` enforces an inclusive deadline (finish on the expiry tick counts, DSN-7), mints exactly one `TimedOut` result per unresolved participant into the retained ledger; `SessionOutcome::TimedOut` added. HUD shows `m:ss` remaining / `OUT OF TIME`; smoke records `tl=`/`outcome=`. Retail: london blitz:0 `cp=3/3 → timed-out` (bot cleared all gates, never crossed the finish — RACE-7 holds), sf blitz:0 `tl=23.0s` running. Externally checked (review pass, iteration 015 feedback) — AC03 candidate-level; AC01/02/04–06 partially open per review gaps (full-roster matrix, objective/nav HUD, warning cues → F12-B/C). |
| F12-B | implemented | F12-A | Split: B.1 = navigation arrow (checked), B.2 = low-time warning cue (implemented below). In-scope legs done: timer/objectives/finish/failure + HUD/navigation + warning cue. Still open on the parent: results/fail screens beyond HUD text (F17/UI-5 scope), audio cue (needs F07), AC01/AC06 catalog evidence (F12-C). |
| F12-B.1 | checked | F12-A | RACE-6 navigation arrow. `mm2_game::race`: `navigation_target` (nearest un-cleared gate XZ, explicit `picked` wins until cleared then falls back, armed `Finish` once all gates clear, `Ordered` → `None` per HUD-2), `cycle_target` (authored-order walk, wraps, skips cleared, empty → clears pick), `relative_bearing` (signed driver-frame angle, `+` = right, ground-plane only), `TargetSelection` component, `RaceProgress::remaining`. `mm2_app::race`: session-owned `NavArrow`/`NavArrowPart` UI needle + diamond tip (node-drawn — embedded font is ASCII-only, DSN-8), `spawn_nav_arrow`, `nav_target_input` (X/Z cycle — original X/S blocked by WASD brake, DSN-8), `update_nav_arrow` (rotation = bearing, green ahead / yellow behind, hidden without a live target). Tests: +6 contract, +5 production-path (bearing/color/visibility through `Position` writes, X/Z cycling incl. edge-trigger + Complete gate, finish arming, teardown despawn). Retail: london `blitz:0` headless `status=pass` (tl ticking); windowed captures at frames 100 (countdown) + 700 (driving) show the needle green ahead tracking the gate. Yellow-behind leg covered by tests, not rendered. Externally checked (review pass at `56e764c`, iteration 016 feedback). |
| F12-B.2 | implemented | F12-B.1 | Low-time warning cue — designed policy (DSN-9): no documented original rule (HUD-2 lists only the countdown timer). `mm2_app::race`: `LOW_TIME_TICKS` (10 s, inclusive threshold), `LOW_TIME_FLASH_TICKS` (0.5 s half-period), `LOW_TIME_BRIGHT`/`LOW_TIME_DIM`, session-owned `LowTimeWarning` `LOW TIME` UI banner + `spawn_race_warning`, `update_race_warning` — armed while a timed race runs and the local participant is unresolved, pulsing bright/dim on the remaining ticks themselves (same race clock as the deadline → AC04; freezes with pause), hidden for stale/complete/countdown/untimed races and a resolved local participant (`PlayerControl::Local` filtered — avoids the latent multi-participant wrinkle the B.1 review flagged on the arrow). Tests: +4 production-path (threshold boundary + pulse cadence + pause freeze + timeout hide; countdown gate for sub-threshold limits; resolved-local hides while a remote races; untimed never warns + teardown despawn). Retail: london `blitz:0` headless `status=pass` incl. `--frames 1800` → `race=Complete outcome=timed-out`; windowed capture at `time 1.5s` shows the armed banner (dim half-pulse), a `time 16.1s` frame shows it correctly hidden. Candidate pending external check. |
| F12-C | implemented | F12-B | Structural leg (checked at `f62a977`): `RaceDefReport` + `mm2-inspect race-defs`, retail 90/90 rows build at both difficulties, 0 failed; headless Blitz matrix 20/20; invalid refs fail explicitly; DSN-6 spawn repair (`f8bd917`). Scripted-completion leg (this iteration, candidate): `mm2_app::scripted` — the `--bot` evidence driver. `ScriptedDrive` resource flag + session-owned `ScriptedBot` component state; `drive_target` follows the live objective (earliest un-cleared gate in authored order for `AnyOrder` — the waypoint rows are the only route data blitz/checkpoint events ship — `progress.next` for `Ordered`, armed finish last); `scripted_input` proportional steer + throttle bands + corner brake + speed cap + two-phase stuck escape (reverse-and-turn, alternating side); `scripted_drive` owns `VehicleInput` after the keyboard mapping only while `--bot` inserts the resource, honors the countdown lock and yields on a resolved participant. Wired headless (`smoke::Driver::{Hold,Scripted}`, `driver=` record field) and windowed (`.after(input::vehicle_input)`, gated `resource_exists`). Tests +6 (`tests/bot.rs`): control law, stuck escape, live-target selection, countdown lock, L-turn AnyOrder finish → 1 `Finished` result, 2-lap Ordered circuit finish incl. lap wraps — all through `load_session_world`→`advance_race`. Retail bot matrix 64/64 events both cities (see baseline table): 4 finished, all 20 Blitz `race=Complete`, 41 `Running` at frame cap, 8 SF falls — bot-limited evidence, not a playability claim; most retail courses need route-aware driving (F15). Still open on the parent: reward-fact leg (AC05 — no reward emission exists to exercise, F16 scope). Candidate pending external check. |
| F13-A | active | F02-B, F11-B | Split into A.1 (results flow — implemented below). London race0–13, SF race0–11 (+r0) authored data present; `.aimap`/`.aimap_p` measured as Amateur/Professional rosters (RACE-11). Remaining: unlock/progression ledger leg (CHK-2/CHK-3 — F16 scope), recovery-penalty leg (RACE-5/DMG-2 destruction — F05 scope), environmental-override leg (tod/weather binding — F18 scope), and AC06's representative-playability + honest coverage matrix. |
| F13-A.1 | implemented | F02-B, F11-B | Results flow (UI-5, DSN-11): `advance_race` transitions `Playing → Results` on the same step it records a *local* participant's terminal resolution (`Finished`/`TimedOut`) — the race clock, progress and ledger freeze with the phase; a remote/AI participant resolving while the local driver still races ends nothing (per-participant progress stays independent). `update_hud` shows the outcome + recorded finish time during `Results`. Tests +4 in `tests/race.rs` (31 total): finish→Results + once-only ledger, non-local resolution keeps `Playing` then local finish resolves, timeout→Results, restart-from-Results rebegins gen-2 with no stale `RaceState`. Retail: sf `checkpoint:0 --bot` → `phase=results cp=6/6 outcome=finished`; london `blitz:0` → `phase=results outcome=timed-out`. Ledger: RACE-11 (aimap difficulty rosters, 23/24 + `sf/race0` anomaly), WPT-2 measured on all 24 waypoint files, MP-9 corrected. AC02/AC03/AC05 evidence strengthened; AC04 needs F15, AC06 needs representative playability. Candidate pending external check. |
| F13-B | active | F13-A | Split into B.1 (standings ordering + placing presentation — implemented below). Remaining: live position/leaderboard semantics while racing (shared with F14-B), opponent integration hooks (F15 scope), full results screen (F17). |
| F13-B.1 | implemented | F13-A | `ResultLedger::standings`/`place_of` — the authoritative finish ordering (DSN-12, designed: no verified original placing rule): `Finished` outranks `TimedOut`, `race_ticks` ascending, equal ticks broken by `PlayerId` (the spec's required explicit tie resolution); unrecorded participants are unplaced. Results HUD shows `FINISHED {ord}[ of {n}] {time}s` from the standings; smoke record gains `place=` on the local participant's result. Tests: +1 contract (out-of-order recording, same-tick tie by participant, timeout below finish, unrecorded → unplaced) +1 production-path (two participants — remote + local — interleaved through a 2-lap Ordered course: independent `next`/`lap`, remote resolves without ending the local race, standings order by recorded clock). Retail (`fnv1a64:e91e6cd4b2ae30d9`): london `blitz:0 --bot --frames 2000` → `race=Complete cp=3/3 outcome=finished place=1` — the standings field end-to-end on real content. Repair (iter 20, review finding): the HUD and smoke record ranked through the *unscoped* `place_of`/`iter()` — after an in-process restart the retained ledger's stale results could re-rank the live session (same `PlayerId` reissued by `next_player` reset); both now scope via `place_of_in`/`standings_in(session.generation())`, `results=` counts the generation, and `smoke::result_outcome` carries a unit regression test. Advances F13-AC03 and F14-AC03's ordering leg; live rank/leaderboard while racing stays F14-B scope. Candidate pending external check. |
| F13-C | queued | F13-B, F15-B | — |
| F14-A | active | F02-B, F11-B | Split into A.1 (binding honesty + lap evidence — implemented below). London circuit0–11, SF circuit0–11 authored data present (circuit11 partial: opp/pathset only, no .aimap). SF `cir1–9` are the circuit events' start grids under a short stem — aliased to `circuit<N>` since F11-B.2 (WPT-3). Remaining: event `.aimap` `[Exceptions]`/density scoping needs consumers (F15/F10 scope), AC06 representative-playability matrix. |
| F14-A.1 | implemented | F02-B, F11-B | Circuit/lap binding hardening + evidence. `race_def`: authored `NumLaps` is a checked parameter like every other authored value — `BadParam` on `≤0`/overflow (was a silent `.max(1) as u32` clamp+truncate); `laps: 0` stays unbound on AnyOrder rows (UNK-5 template junk). `RaceProgress::advance` is inert outside `Racing` — re-anchors for `AwaitingStart`, can never re-finish or clear gates once resolved (AC04's once-only rule is now a contract property; F14-AC02's repeated-finish-hits leg). Dead `with_next` builder removed; `Ordered` doc spells out start-lap semantics (lap 1 begins at release, the start-line copy closes each lap). Smoke record gains `lap={cur}/{laps}` for Ordered defs (any-order records bit-identical). Tests: +2 contract (closing gate counts once per completed sequence; resolved participant inert) +1 producer (`NumLaps` 0/-2/5e9 → BadParam, per-difficulty blocks, checkpoint junk ignored); two existing tests now set `Racing` before `advance` to match the driver gate. Retail (`fnv1a64:e91e6cd4b2ae30d9`): `race-defs --table circuit` — 10/10 rows/city, distinct authored laps (london 3am/4pro except c1,c2 2/2, c9 3/2; sf 3/4 except c8-9 2/2), 6–23 gates, 4–7 opp/0 cop, 1–8 slots; london `circuit:0 --bot` → `race=Running cp=2/6 lap=2/3`, `--pro` → `lap=2/4` (bot wedges mid-lap-2 — bot-limited, deterministic, finish previously recorded). AC01 strengthened, AC02 negative legs evidenced; AC03–AC06 open. Externally checked (review pass at `561b8b7`). |
| F14-B | queued | F14-A | — |
| F14-C | queued | F14-B, F15-B | — |
| F15-A | active | F02-B, F09-B, F11-B | Split into A.1 (roster/route-intent import) + A.2 (spawn/drive) + A.3 (authored start headings — all implemented below). Remaining against the parent: the difficulty/param-tail driving model (UNK-11) and any F15-A acceptance legs the external review still counts open. |
| F15-A.1 | implemented | F02-B, F09-B, F11-B | Opponent-roster import: `mm2_game::opponent` contract (`OpponentRoute`/`OpponentSpec`/`OpponentRoster`/`OpponentIssue` — `resolved_routes()` keeps dead wired refs distinct from spare files); `mm2_content::opponents::opponent_roster` builds from a `CatalogEvent` — `.aimap`→Amateur, `.aimap_p`→Professional with explicit `MissingVariant` fallback; each row wires geo id + `.opp` route (VFS-resolved, all point columns preserved) + param tail (`skill` = first). Issues: `CountMismatch` vs table `Opponents`, `UnresolvedRoute`/`RouteFailed`, `WrongDifficultyTag`, `UnreferencedRoute` — reported, denominator kept. `OpponentReport::scan` + `mm2-inspect opponents <install> [--city] [--strict]` audit every catalog event ×2 difficulties + extra roster stems + `VehicleCatalog` vehicle resolution. Tests +12 (`tests/opponents.rs` 9 content + `tests/opponent.rs` 3 contract): variant selection, fallback, dead route keeps slot, tag mismatch, scoped spare routes, count mismatch diagnostic, crash/incomplete rejection, extras+vehicle report, route length/skill/wired-count. Retail: 64 builds/city, 0 failed, 26 unsupported (crash tables), 271+246 wired, 0 unresolved vehicles, 43+34 spare routes, `sf/race0` amateur sole count mismatch; `--strict` exits 2. Ledger RACE-11/RACE-12, UNK-11 narrowed. Candidate pending external check. |
| F15-A.2 | implemented | F15-A.1 | Opponent spawn/drive runtime. `mm2_content::load_opponent` loads each roster vehicle with its authored `<id>_opp.vehcarsim` merged as a sparse override over the base tune (`TuneBlock::merge_overlay`, RACE-13: most `_opp` files omit fields the base carries; several author an alternate Trans schema — `NumGears`/`GearRatios`/`Up|DownshiftRPM`/`DownshiftBias` — whose fields surface as unrecognised-field warnings). `EventSetup.roster` carries the built roster; `load_session_world` calls `spawn_opponents`: one session-owned entity per authored entry — own `VehicleDef` (per-vehicle mass/power/size, not player clones), minted `ObjectId`/`PlayerId`, `PlayerControl::Ai`, session authority role, `DamageSignals`, `RaceProgress` on the shared definition, `OpponentDriver`; load failure warns + skips only its slot; dead `.opp` ref holds still. `opponent_drive` (Update, `main.rs` + `smoke.rs`) chases the `.opp` polyline through `scripted_input` — the same normalized `VehicleInput` law as the evidence bot (proportional steer, corner brake, bounded reverse-and-turn recovery); gates on `is_playing`, countdown lock and resolved participants; `route_target` advances past reached/passed anchors, wraps closed routes, bounded retry on degenerate ones. `spawn_pose` (provisional, UNK-16/17): `_strtpnts` slot `index+1` → route anchor → designed stagger; facing from the route's first leg. Smoke record gains `opp={resolved}/{spawned}` only when a roster exists. Tests +11 (`tests/opponents.rs`, synthetic install incl. `.bnd` fixture): distinct-entity spawn, unloadable-vehicle slot skip, dead-route holds still, countdown lock, route-target advance/skip/open-complete/closed-wrap, spawn-pose preference, drive-and-finish through `advance_race`, restart respawns the lineup. Retail (`fnv1a64:e91e6cd4b2ae30d9`): `sf checkpoint:0 --bot --frames 5400` → `opponent roster spawned opponents=6`, `opp=3/6` resolved `Finished` through shared validation, `results=4`, player `place=4` of `pos=4/7`; roster issues (`race0` count mismatch + `race0-a-6.opp` unreferenced) surface in-run. First run exposed `_opp` files failing the strict tune decoder — fixed by the documented merge. Candidate pending external check. |
| F15-A.3 | implemented | F15-A.2 | Authored start headings. Measured on all 612 retail `.opp` files + the `cir*_strtpnts` grids: the `.opp` `brake` header misleads — a nonzero value marks a staging record carrying a heading in vehicle-yaw degrees (forward `(−sin a, −cos a)`; row 0 on 592 files, 542 agreeing with course direction within ~25°, grid events sharing one value; `race/sf/race5-a-{5,6,7}` carry a second staging row mid-file — trigger open). `_strtpnts` `a` is the same convention — both sit exactly 180° from the waypoint `a` course bearing (`atan2(dx,dz)`), resolving the UNK-16 split. `RaceStart.yaw_deg` now stores vehicle yaw and `session.rs` spawns with `to_radians()` directly — the old bearing read +180° spawned the player backward on SF's authored circuit grids; the no-grid fallback derives `atan2(−dx,−dz)` from the row0→row1 tangent. `OpponentRoute::start_heading_deg()` exposes row-0's staged heading (raw `brake` kept); `spawn_pose` faces the slot's `yaw_deg` on a grid, the staged heading at a route anchor, the first leg only when no authored heading exists, and the player's yaw for a route-less stagger; `initial_route_index` starts the chase at the first anchor ahead of the staged facing and not already reached — a staged start joins the `.opp` line mid-leg (`circuit1-a-0`: heading −X, row 1 +X behind), so chasing row 1 U-turned off the line. Tests +4 (`tests/opponents.rs` 19 total: authored staging heading beats the first leg, chase index skips behind-facing anchors both directions, spawn faces authored −X through `load_session_world` with `driver.next` past the tail; grid-slot facing now asserts authored yaw) +1 (`tests/event.rs`: `authored_strtpnts_yaw_faces_the_player_spawn` — retail-style 90° grid → −X verbatim) + producer assertions for the vehicle-yaw fallback + strtpnts convention. Retail (`fnv1a64:e91e6cd4b2ae30d9`, vs a same-iteration `09e2b00` baseline worktree run for both builds): `sf circuit:1` hold-driver 900 → drives −X off the authored grid (baseline +X — backward, verified same command); `sf circuit:1 --bot 14400` identical `opp=0/7` `cp=4/10` lap 1 (240 s cap on a 3-lap × 10-gate course, not a stall — metrics near-identical); `sf checkpoint:0 --bot 5400` `opp=6/6` vs baseline `3/6` (impacts 233 vs 195 — both runs end with the scripted player off-course through the world, a bot limit on both builds); `london circuit:0 --bot 14400` `opp=2/7` vs baseline `3/7` — one fewer finisher through the authored-heading launch, impacts 576 vs 408; disclosed, not claimed parity. **Review repair (iter 11):** external review reproduced one blocking regression — `cir6_strtpnts` authors `a = 0` on all three rows, and a verbatim 0 yaw (−Z) spawned the player and grid-slot opponents backward off a course whose `.opp` routes stage ~177–183° (+Z). `RaceStart.yaw_deg` is now `Option<f32>` — an authored `a = 0` means no heading, the same zero-means-unset rule `OpponentRoute::start_heading_deg` already applied to `.opp brake == 0`. Consumers of `None` derive a course facing: the player takes `RaceDefinition::course_yaw` (first trigger ≥2 m in XZ — the same facing the no-grid tangent fallback derives), grid-slot opponents fall through to the route staged heading → first leg → player yaw chain. Tests +3 (`race_def` a=0→None, `event` zero-grid → course facing through `load_session_world`, `opponents` headless-slot staged/first-leg/player fallbacks) +1 contract (`course_yaw`). Retail re-run: `sf circuit:6` hold-driver `(-1478,-402) → (-1646,-393)` down-course through gate 0 (was `z=-483` backward on the rejected candidate); `--bot 5400` `pos=2/7` field progress; `sf circuit:1` hold-driver reproduces `(-507,-52)` unchanged. Ledger: WPT-4 rewritten (both `a` conventions measured + the a=0-unset rule), UNK-16 narrowed to gate-direction enforcement, UNK-11 `.opp brake` staging measured, UNK-17 narrowed to participant↔slot mapping. Candidate pending external check. |
| F15-B | active | F15-A | Split into B.1 (participant avoidance/overtake), B.2 (authored `[Opponent]` param-tail decode/consume — checked at `a203f833`), B.3 (bounded re-anchor recovery), B.4 (designed catch-up assist + `cu=` disclosure — implemented below), B.5 (scripted-player route guidance + bounded re-anchor — implemented below), B.6 (nav-graph `.opp` densification — implemented below; the sf p3→p4 "hillside wall" was route fidelity — the authored leg straight-lines across rooftops — not collider fidelity), B.7 (per-opponent smoke disclosure `opps=`/`p_rec=` — implemented below; replaces the removed instrumentation the reviewer flagged), B.8 (documented tail-column order + `avoidOpponents`/look-ahead consumption — implemented below), B.9 (AC06 measured-difficulty leg + `diff=` record disclosure — implemented below). Remaining: representative avoidance matrix, `unkFlag`/`cornerBrakingThreshold`/`weirdPathfinding` consumption once semantics verify, original-runtime semantics of the consumed fields (designed readings meanwhile). |
| F15-B.1 | implemented | F15-A | Opponent traffic avoidance/overtake in `opponent_drive` (designed controller, no original-AI claim). Per frame each AI builds a `Traffic` snapshot of all other participants — the local player is an obstacle exactly like another AI — and `nearest_blocker` resolves the closest car inside a forward corridor (`BLOCK_HALF_WIDTH` 2.4 m lane, `reach` speed-scaled). A pass commits only on a real obstruction: a standing blocker (`< CRAWL_SPEED`) anywhere in the corridor, or a moving one genuinely closing (`> FOLLOW_RELEASE`); a matched-pace car is a queue to sit in, not a reason to leave the route. Commit picks a side (`pick_pass_side` + a `PASS_SCAN` room weighting over every nearby car — never into an occupied lane) stored as `pass_side`; the aim becomes `pos + fwd·PASS_LOOKAHEAD + route-lateral·PASS_OFFSET` so the offset lane bends with the road. `held_blocker` holds the pass across the wide window until the blocker is `PASS_BEHIND` behind — no cut-back across its nose — and `PASS_RELEASE` bounds the linger. `apply_gap_brake`: moving blockers inside the comfort gap get a soft adaptive-cruise brake even at matched pace (queues keep gaps instead of riding bumpers); standing blockers brake only on a real approach so crawl-pace steering can still complete the drive-around; `PANIC_GAP` brakes hard on any active closure. Bounded response: `PASS_STALL` frames without `PASS_STALL_DIST` of displacement abandons the pass and bans that blocker for `PASS_BAN` — fully transparent (no aim, no brake) so the route line can push or slip past, then the clean pass retries. Static geometry stays with `ScriptedBot`'s existing bounded recovery. Review repair: `update_checkpoint_markers` now selects the `PlayerControl::Local` participant (was `iter().next()` — ambiguous once opponents carried `Player` too). Tests +5 (`tests/opponents.rs` 16 total): corridor/panic/center pick, corridor-only sensing, comfort-gap brake bands incl. matched-pace queue brake and crawl no-brake, parked-car drive-around through `load_session_world`→`advance_race` with zero blocker contacts, local-marker disambiguation. Retail (`fnv1a64:e91e6cd4b2ae30d9`, vs F15-A.2 baseline re-run this iteration): `sf checkpoint:0 --bot 5400` `opp=3/6` place 4, impacts 195 vs 208; `london circuit:0 --bot 14400` `opp=3/7` place 4 vs baseline `2/7` place 3 — first retail circuit opponent evidence, run this iteration for both builds (AC02 circuit leg); `sf checkpoint:0` hold-driver `opp=4/6` impacts 165 vs baseline 232; `london circuit:0` hold-driver `opp=3/7` vs baseline `4/7`. Known limit: on the tightest 8-car narrow-street circuit a mid-pack knot can still circulate at crawl pace for tens of seconds before the stall/ban cycle clears it — bounded churn, not permanent standstill (baseline also stranded 2 cars permanently); field-wide pace under heavy traffic is an F15-B/F15-C open item. Candidate pending external check. |
| F15-B.2 | implemented | F15-B.1 | Authored `[Opponent]` parameter-tail slice (checked at `a203f833` — external review verdict pass): the ten-value tail decodes positionally into `mm2_game::OpponentDriveParams` using mm2hook's recovered `OpponentData`/`RegisterRoute` vocabulary — *inferred* mapping (RACE-14), short rows decode trailing fields absent not zero, raw params kept verbatim. `mm2_app` binds it per driver at spawn through a `ScriptedTuning` overlay: `maxThrottle` → throttle ceiling [0,1], `cornerSpeedMultiplier` → corner-brake engage-speed scale, `avoidPlayers` → gates the corridor's sensing of human participants (AI sensed unconditionally — retail `avoidOpponents` ≈ universal 0, kept inert pending polarity verification). `ScriptedTuning::DEFAULT` is the pre-tail law bit-for-bit; `scripted_input` stays a default wrapper so `--bot` is untouched. Tests +7: verbatim-row decode, cap/floor law + DEFAULT equivalence, spawn binding, on-track maxThrottle A/B, parked-player corridor gate. Retail: `sf circuit:1` 900-frame hold bit-identical; `sf checkpoint:0` amateur vs pro `opp=4/6` both (dynamics data — vehicles/routes confound). |
| F15-B.3 | implemented | F15-B.2 | Bounded re-anchor recovery (DSN-14, AC03/AC04): `OpponentDriver` gains a displacement-based stuck window — `stuck_pos`/`stuck_frames` measure 900 driving frames without 8 m of displacement, so penned, hull-beached (ungrounded cars never reach the scripted recovery's grounded-gated stuck counter) and knocked-off-route opponents all count. At the bound the authority teleports the car through the production `ResetVehicle` path onto the route leg it was chasing — `Teleported` breaks the swept segment so the jump banks no checkpoints — and `reanchor_pose` projects the position onto the chased leg then walks backward along the authored polyline (4 m clearance, further while inside an un-cleared trigger, ≤60 m total, open routes clamp at the start, closed routes walk the wrap leg for `next == 0`) so the landing sits outside every pending gate. `driver.next` recomputes via `initial_route_index`; recovery/pass/stall/stuck state resets; `reanchors` counts each assist and the smoke record adds `opp_rec=` when nonzero. Predicted clients never teleport (authority gate); non-finite positions reset the window instead. Tests +7 (`tests/opponents.rs` 31 total): chased-leg projection + facing, walk-back out of pending triggers across a leg boundary, cleared gates don't extend the walk, closed wrap + open clamp, degenerate routes, penned-car end-to-end (walls the car off-lane through the `Teleported` contract — zero gates banked while penned, disclosed teleport onto the route, upright landing, marker consumed, then a real finish), staged-budget dispatch through `ResetVehicle`, no re-anchor on a progressing field. Retail (`fnv1a64:e91e6cd4b2ae30d9`): `sf checkpoint:0` 5400 `opp=5/6 opp_rec=2` (vpbug twice) vs F15-B.2 baseline `4/6`; `sf circuit:1 --bot` 14400 `opp_rec=15` field-wide (vpbug ×5, vpcoop) with `opp=0/7` still at the 240 s cap — the assist fires on genuinely stuck cars and is observable but does not manufacture finishers; `sf circuit:1` 900-frame hold-driver bit-identical `final=(-507,18.9,-52)`. Known limit: a route that threads a difficult pocket re-sticks the car after landing — re-anchors repeat every ~15 s (each disclosed), not a progress guarantee. Candidate pending external check. |
| F15-B.4 | implemented | F15-B.3 | Designed catch-up assist + disclosure (DSN-27 — whether the original rubber-bands at all is unverified, UNK-11; the F15 spec permits rubber-banding only when "explicit, observable, scoped by rules and not falsely recorded as physical racing"). `mm2_game::race` gains `CatchUpPolicy` (`deficit_full` 2.0 gates, `assist_max` 0.25 — both implementation choices), `mean_gate_spacing` (the course's own authored leg scale), `course_progress` (a participant's continuous position in gate units: banked gates — `lap × gates + next` `Ordered` / cleared count `AnyOrder` — plus the covered fraction of the leg toward the same objective `live_order` tie-breaks on, garbage-safe to banked-only) and `catch_up_factor` (0 at/ahead of the lead, linear to `assist_max` at `deficit_full`, NaN-safe). `opponent_drive` resolves the leader as the best `course_progress` across *every* progress-carrying participant — the human included — over one shared leg scale (mean authored gate spacing, 80 m designed fallback `CATCH_UP_LEG_REF`); a trailing AI driver's *demand ceiling* lifts one-directionally (`throttle_cap + assist` ≤ 1.0, `corner_speed × (1 + assist)`) — nobody is slowed below authored tuning, the player is never a recipient, progress is still earned through the same swept triggers. Observable: `OpponentDriver::catch_up` carries the live factor (zeroed on every non-driving path), the smoke record counts assisted drivers as `cu=` on activity only, kept distinct from `opp_rec=` re-anchors — a demand lift is not a recovery. Tests +6 (`tests/race.rs` 26 total, `tests/opponents.rs` 32 total): spacing mean/degenerate legs, ordered lap+leg banking incl. leader/follower deficit, any-order cleared+objective fraction, degenerate-input finiteness, factor ramp/saturation/garbage legs, and a production-path A/B (parked player teleported through the shared `advance` — leaders never lifted, trailers' `throttle` exceeds the authored cap, `reanchors` stays 0, no progress granted). Retail (`fnv1a64:e91e6cd4b2ae30d9`, dev build `68a7b99+diff`): `london circuit:0 --bot` 2700-frame mid-race `status=pass pos=1/8 lap=2/3 opp=0/7 opp_rec=1 cu=7` — all trailers assisted, leader never; `sf checkpoint:0 --bot` 1500 `cu=5` mid-race (`pos=5/7`); `sf --car vpbug --bot` 600 and `london --bot` 600 cruises bit-identical to the F05-B.8 review records (no roster → no `opp=`/`cu=` fields). Disclosed: `cu=` zeroes when the session leaves `Playing` (end-of-race records show none — the field reports the live assist, not a cumulative count); `sf checkpoint:0`'s end-of-run `status=fail "fell through the world"` is the pre-existing altitude-threshold trip on a descending course (car grounded `wheels=4/4`, `cp=3/6`, racing normally — the same failure class noted in iteration 11's notes, unrelated to this slice). AC06's measured-difficulty leg and the remaining authored-tail items stay open under F15-B's parent. Candidate pending external check. |
| F15-B.5 | implemented | F15-B.4 | Scripted-player route guidance + bounded re-anchor (the `sf circuit:0` descent stall F00-C.2's repair exposed). `mm2_app::scripted` gains `ScriptedRoute` — a session-owned component `load_session_world` binds on the player in every event session whose roster resolved a route — dormant without `--bot` (`scripted_drive` is resource-gated; no `ScriptedDrive` check on the insert) (`pick_bot_route` = nearest-staged resolved `.opp`; unresolved/empty skipped; implementation choice — retail assigns routes to AI, never the player). `route_aim` chases anchors bounded at the objective's nearest route anchor (open + closed-route gate-behind detection; the chase can never run past an uncleared gate) and aims at a 40 m polyline lookahead so bends are entered before the apex — `.opp` anchors sit 40–200 m apart, raw-anchor chasing turned too late. Three bounded re-anchor arms share one `ResetVehicle`/`Teleported` teleport (swept segment broken, walk-back stays clear of pending triggers, `reanchors` disclosed): displacement 900 f/8 m (penned), 3-D off-route 300 f/>12 m (a fall loop's bubble never fills — `route_distance` sees the car under an elevated leg), and ≥3 `RecoveryEvent`s without a banked gate (`RaceProgress::crossings` as the stamp) while still off the line. The walk-back's `blocked` closure adds a 10 m `SpatialQuery` ground probe — the `.opp` line is car-height sampling, not a ground promise, and the retail descent interpolates over a rooftop gap where a blind teleport fell forever. `opponents` helpers shared: `point_reached`/`route_is_closed`/`REANCHOR_*`/`SPAWN_LIFT` now `pub(crate)`. Tests +11 (`tests/bot.rs` 7→18): aim chase/lookahead/cap/behind/closed/empty, 3-D distance, route pick, routed-detour integration finish, penned re-anchor resumption, recovery-count arm. Retail (`fnv1a64:e91e6cd4b2ae30d9`): `sf circuit:0 --bot --headless --frames 9000` → `status=pass cp=2/9 rcv=21f/21r wheels=4/4 dropped=0 final=(-2026,29.8,-51)` vs pre-slice `rcv=206` void-fall loop to y≈−11; dev trace showed gates 1→2 banked on-line (offd<5) and every re-anchor landing supported (offd≈1 after). **Known remainder:** the p3→p4 hillside descent is not drivable — car slides ~15 m off the line's crown into a parallel ditch and grinds at ~1–2 m/s through 6 disclosed re-anchors; AI field shares the wall (`opp=0/4 opp_rec=5`) — recorded then as a steep-slope/collider-fidelity task; F15-B.6's probing disproved that (route fidelity — the leg straight-lines across rooftops) and resolved it. Candidate pending external check. |
| F15-B.6 | implemented | F15-B.5 | Navigation-backed `.opp` densification — the `sf circuit:0` p3→p4 "hillside wall" F15-B.5 recorded as a collider/steep-slope defect was **route fidelity**, not physics: ground probes showed the authored p3→p4 straight line climbing a ~7 m step onto Telegraph Hill bungalow roof terraces (y≈40/46 vs road y≈33) and crossing a second roof row — undrivable at any speed (a 33 m/s jump attempt still wedged on a wall) — while BAI road 340 authors the drivable course as an L-shaped street around the block, and a direct physics probe drove that descent at ~40 m/s. Every retail `.opp` is sparse intent (8–20 anchors/lap, 40–200 m legs), so the runtime derives the driven line from the road graph between anchors. `mm2_game::nav` gains `lane_hits` (all eligible snaps, nearest-first; `nearest_lane` picks its head), `route_candidates` (multi-entry A* through the shared `route_search`: every lane within `ENTRY_TOLERANCE` 12 m of the nearest snap seeds its arc at snap cost — both directions of a carriageway compete, neighbouring roads don't; `forward_on_arc` rejects a same-arc goal lying behind the start in travel direction), `route_path` (samples the routed arcs in travel direction through `transfer_lane` rank preservation + `crossing_path` junction interiors), and `densify_route` (a leg leaving `ROUTE_CORRIDOR` 14 m of every vehicle lane is re-pathed at `DENSIFY_STEP` 8 m; authored anchors stay verbatim — gate binding/checkpoint semantics unchanged; closed routes densify the wrap leg and re-emit the head anchor; unroutable legs keep the authored line rather than inventing geometry). `OpponentDriver.route` carries the derived line — `spec` stays authored verbatim; `driving_route()` applies it with `RouteOptions::default` (the event aimap's `[Exceptions]` ambient closures deliberately *not* consumed — they forbid ambient traffic on a road, they do not remove a road from a race course; implementation choice). `load_session_world` builds the city's graph once per event (~2 ms on sf.bai — cheaper than cloning/sharing traffic's) and densifies the bot's `ScriptedRoute` plus every opponent route; a failed/absent graph leaves routes verbatim. Tests +7 (`tests/nav.rs` 25→32): goalward-direction entry where single-snap `route()` dead-ends, stacked-level snap preference, lane-walk sampling incl. junction chords, corridor-leg verbatim, off-road re-path, closed-wrap re-emission, unroutable-leg verbatim. Retail (`fnv1a64:e91e6cd4b2ae30d9`): `sf circuit:0 --bot --headless --frames 9000` → `status=pass cp=4/9 lap=2/3 pos=1/5 opp_rec=4` vs pre-slice `cp=2/9` + 21 fall-recoveries wedged on the hill (an instrumented run under this slice's candidate code showed all four opponents clearing gates 2/4/3/2 vs `opp_rec=5` wall-shared); `route_candidates` on the real p3→p4 leg resolves a single 258 m `road 340 Backward` arc vs the old single-snap 699 m wrong-direction detour. `london circuit:0 --bot` regression: 1500f `status=pass cp=3/6 pos=5/8 opp_rec=1` (one gate behind the reviewer's pre-slice `cp=4/6` — densified-line pacing delta, not a stall), 9000f `opp=1/7` (a finisher). Gates: fmt/clippy clean, `cargo test --workspace` 67 suites 0 failures. Candidate pending external check. |
| F15-B.7 | implemented | F15-B.6 | Per-opponent progress/recovery disclosure in the smoke record (F15 spec req 6) — the F15-B.6 reviewer's named verification gap: the "all four opponents clear gates" evidence came from temporary instrumentation removed before commit, and opponent re-anchors were `info!`-only. `ScriptedBot` gains `escapes` (counted each time the two-phase reverse-and-turn escape fires); `OpponentDriver` gains `index` (the authored roster slot — a skipped vehicle load does not renumber the field) and `stuck_peak` (the longest continuous spell inside one displacement bubble — a cumulative `stuck_total` was implemented then rejected: a moving car's bubble re-seats every `REANCHOR_DIST`, so the cumulative ≈ race duration regardless of health, while the peak stays a handful of frames on a progressing car and reaches `REANCHOR_FRAMES` on a penned one; it survives both the displacement-window reset and the re-anchor). Both re-anchor resets (opponent + scripted player) now preserve `escapes` across their `ScriptedBot` reset. `smoke.rs` gains `opps=` — one row per spawned opponent sorted by authored slot: `<slot>:<vehicle>/<cleared>c[/<lap>l][/F|/T][/<escapes>e][/<reanchors>r][/<stuck>w]`; `cleared` is `RaceProgress::cleared_count` (per-lap under `Ordered`), `lap` uses the record's `lap+1` convention, `w` counts update frames — and `p_rec=<reanchors>r/<escapes>e` for the scripted player. Both fields print only on activity so a clean record stays bit-identical. Tests +1 (`smoke::opponent_detail_reports_progress_and_recovery_per_slot` — roster-slot sorting over out-of-order entity collection, AnyOrder no-lap, `T`, empty roster) + assertions in 4 existing tests (penned re-anchor peak/escapes, dispatch-path peak survival, progressing-field peak bound, bot escape counting); suites 67. Retail (`fnv1a64:e91e6cd4b2ae30d9`, headless): `sf circuit:0 --bot 9000f` → `status=pass cp=4/9 lap=2/3 pos=1/5 opp_rec=4 cu=4` with `opps=0:vpbug/6c/1l/13e/3r/900w,1:vpbug/6c/1l/4e/697w,2:vpbug/0c/2l/10e/872w,3:vpbug/6c/1l/15e/1r/900w p_rec=3r/2e` — `opp_rec` 4 equals the rows' `r` sum; slot 2 wrapped to lap 2 mid-field while two penned cars show exactly `REANCHOR_FRAMES` peaks. `london circuit:0 --bot 1500f` → `status=pass cp=3/6 pos=5/8 opp_rec=1` — bit-identical headline to the pre-slice regression disclosure (observability-only slice) with all seven slots reporting progress (`p_rec=0r/1e`). Gates: fmt/clippy clean, `cargo test --workspace` 67 suites 0 failures. Candidate pending external check. |
| F15-B.8 | implemented | F15-B.7 | Documented `[Opponent]` tail column order + `avoidOpponents`/look-ahead consumption. The community format reference (angel-file-formats `AIMAP.md`, R3) publishes all ten tail columns in order, and mm2hook's recovered `RegisterRoute` signature lists its flag arguments in exactly the tail's flag-column order (`unkFlag, avoidTraffic, avoidProps, avoidPlayers, avoidOpponents, weirdPathfinding` ↔ cols 1, 4–8) with defaults bracketed by each float column's retail range — superseding B.2's inferred order, which had the flag block shifted one column late (it made `unused` a 43%-set flag on ordinary rows and `avoidOpponents` ≈ universal-0; both anomalies resolve under the documented order). `OpponentDriveParams` re-decodes: `unused_flag` ← col 1, `distance_padding` ← col 2, `corner_brake` ← col 3, `avoid_traffic/props/players/opponents` ← cols 4–7, `weird_pathfinding` ← col 8. `OpponentDriver` gains `avoid_opponents` + `look_ahead`: `senses()` gates AI participants by authored `avoidOpponents` (59% of 536 retail rows author 0 — the field genuinely does not dodge itself on those rows) and human participants by `avoidPlayers` (75% author 0); `look_ahead` (authored col 2, default `LOOK_AHEAD_DEFAULT` 75 = the `RegisterRoute` `someDistancePadding` default) replaces the speed-scaled corridor reach — a designed reading of the documented "Look Ahead Distance" name; `unkFlag`/`cornerBrakingThreshold`/`weirdPathfinding` stay bound-unconsumed (R3's brake-demand floor has no continuous quantity in our binary corner-brake). Tests: decode test now asserts the corrected positions incl. a verbatim london `crash6` follow-car row separating `unused_flag` (col 1) from `weird_pathfinding` (col 8); `authored_avoid_flags_gate_the_corridor` covers both classes independently; `authored_tail_binds_the_driver_tuning` binds both avoid flags + look-ahead per driver; `authored_avoid_opponents_decides_the_parked_ai` (a route-less roster slot parked mid-lane) and `authored_look_ahead_sets_the_sensing_distance` (150 vs 30 m reach against a 90 m blocker) run the production path. Retail A/B (`fnv1a64:e91e6cd4b2ae30d9`, `circuit:0 --bot --headless --frames 1500`): sf — same headline (`cp=2/9 lap=1/3 pos=4/5 opp=0/4 cu=3 moved=467m final=(-2154,15.1,-32)`), impacts 58→55, opponent stuck peaks 158/98/137/74→104/109/110/74 — authored `0 1 0 1 0` rows now make the field transparent to the player (avoidPlayers col 6 = 0) while still sensing each other (col 7 = 1); london — impacts 149→84, `cp=3/6→4/6 pos=5/8→2/8` (the B.6-recorded pacing regression recovered: the fixed authored 50 m reach brakes earlier than the old speed-scaled 14+1.4·v at low speed), `opp_rec=1 cu=6` unchanged. Docs: `aimap.md` tail section rewritten (documented order, per-column retail ranges, consumption), RACE-14 re-classed inferred→documented, UNK-11 narrowed. Gates: fmt/clippy clean, `cargo test --locked --workspace` all suites 0 failures. Candidate pending external check. |
| F15-B.9 | implemented | F15-B.8 | AC06's measured-difficulty leg + `diff=` record disclosure. The smoke record gains `diff=<amateur|professional>` — an unconditional run-config field beside `driver=` (not activity-gated; every record now names the authored parameter block — DRV-2/3 — and aimap variant — RACE-11 — that selected its content; `Difficulty::as_str` added). Two production-path tests in `tests/opponents.rs` (36 total): `session_difficulty_fields_the_authored_variant` — one fixture install carrying both `.aimap` and `.aimap_p` with different vehicles, slot order, tails and `-a-`/`-p-` route geometry; each difficulty's session spawns exactly its authored lineup through `load_session_world` → `event_aimap` → `opponent_roster` → `load_opponent` (asserted per slot: vehicle id, bound `throttle_cap`, route-variant lane offset). `session_difficulty_measures_on_track` — the same `vpt` chasing identical lane geometry under each difficulty, only authored col-0 differing (0.55 vs 1.00): the professional field covers measurably more road at a fixed tick (>10 m over 480 updates) with `catch_up == 0` on both sides — a lone leader earns no assist, so the delta is authored tuning, never the disclosed assist. `tests/smoke.rs` +1 (`record_names_the_configured_difficulty` — a `--pro`-equivalent config records `diff=professional`, default records `diff=amateur`). Retail A/B (`fnv1a64:e91e6cd4b2ae30d9`, `sf --bot --headless`): `circuit:0` 3000f — amateur `diff=amateur lap=1/3 pos=3/5 opp=0/4 cu=3 opps=0:vpbug/3c/1l/…,1:vpbug/2c,2:vpbug/5c,3:vpbug/5c` vs `--pro` `diff=professional lap=1/4 pos=1/5 opp=0/4 cu=4 opps=0:vpbug/1c,1:vpcaddie/2c,2:vpcaddie/2c,3:vpcaddie/2c` — authored NumLaps (3/4), field composition and per-slot progress all differ measurably (confound disclosed: the variants author different routes and vehicles, so the delta is the authored difficulty's whole effect, not a single variable). `blitz:0` 2000f — amateur `race=Complete tl=0.0s outcome=timed-out` at ticks=3600 with `env=lt00(clear-morning) traf=0/0` vs pro at ticks=3000 with `env=lt01(cloudy-morning) sky=…sky_pa_f fog=600-1000 traf=6/6 crx=11 jq=1` — authored TimeLimit 30 s vs 25 s, Weather 0 vs 1, Ambient 0.0 vs 0.2 all measure end-to-end. Catch-up stays disclosed (`cu=`) and tested (B.4). Candidate pending external check. |
| F15-C | queued | F15-B | — |
| F16-A | active | F01-A, F11-A | Split into A.1 (versioned profile store) and A.2 (app wiring — both implemented below). Remaining against the parent: AC01's restart-isolation evidence, which needs F16-B's authoritative result→progress consumption to be observable, and UI create/select/delete flows (F17 scope — the CLI flags are the interim flow primitives). `players/` retail binaries (17 files, `player<N>.sav`/`*.cfg`) inspected — our own format, no compatibility claimed (DSN-15). |
| F16-A.1 | implemented | F01-A, F11-A | `mm2_game::profile` — `PlayerProfile` (id/name/`Difficulty` rank/`ProfileKind`/revision/`ProfileProgress`/`ProfileSelections`/unknown-field-preserving `extra`, serde JSON `version=1`) + `ProfileStore`: `driver-<n>` ids allocated from a persisted `next-id` high-water mark (advanced atomically before the new profile's first save, floored at the highest surviving file suffix) — a deleted id is never reallocated even when every file it owned is gone, and a `.bak`/`.tmp` orphaned by an interrupted save or delete still owns its id — one `<id>.json` per profile plus `.bak` rotation, atomic tmp-write+`sync_all`+rename saves + directory fsync, `load` keeps the highest surviving `revision` (an orphaned `.tmp` is always the newest attempt) reporting `recovered_from_backup`, corrupt files still listed by id and never deleted by a read, orphan ids list with recovered metadata (superseded vs missing main distinguished), `version` mismatch rejected, size-bounded reads (documents *and* `active`/`next-id` markers), id↔file-stem agreement + sorted/unique `events` checked, path-safe stem validation at the id boundary, `active` marker for the selected profile, DRV-7 enforced (`LastProfile` on the final profile), `ProfileKind::Sandbox` gated by `records_progress()` for F16-B. `EventKey{city,table,stem}` keys progress by authored stem, not row index (spec req 4). `Difficulty`/`EventTableKind` gained serde derives (+`Ord` on the latter); `default_root()` resolves the OS data dir — installs untouched. Tests 20 (`tests/profile.rs`): round-trip, unknown-field preservation, isolation, duplicate names, invalid names, id non-reuse incl. orphan-`.bak` occupancy and the max-id delete + stale-`active`-marker legs, corrupt-main→bak recovery, interrupted-write recovery incl. listing/no-realloc legs, complete-tmp newest-copy recovery + superseded-main listing, orphan ownership + deliberate delete, malformed-id boundary rejection, unsorted/duplicate event rejection, fully-corrupt reporting+preservation, version rejection, id-mismatch, last-profile refusal, delete scoping, active marker, sandbox gate. Candidate pending external check. |
| F16-A.2 | implemented | F16-A.1 | `mm2_app::profile` — the store's application wiring. CLI: `--profile <id|name>` binds an existing profile (unique display names resolve, ambiguous/unknown selectors are usage errors), `--new-profile <name>` creates and binds (rank from `--pro`, `--sandbox` for a dev identity — `requires`/`conflicts` enforced by clap), `--profile-dir` overrides the store root and counts as an explicit request, `--no-profile` opts out entirely. Binding rules: interactive runs with no profile flag bind the store's `active` marker; smoke/evidence runs (`--headless`/`--frames`/`--screenshot`) never bind implicitly — their records stay reproducible — but honor an explicit request; explicit failures exit 2, implicit ones warn and run profile-less. A profile recovered from `.tmp`/`.bak` re-saves once at bind so the main file heals (AC04 seam); selecting marks `active` for later runs. `choose_launch` merges remembered selections: `--car` beats the remembered vehicle, `--paint` its paint, `--pro` its rank; a remembered car that fails to load retries paint 0 then falls back to the stock default (warned — saved prefs never gate launch). `EventSetup.key` carries `EventKey{city,table,stem}`; `load_session_world` calls `note_session_start` only after the world/race resources initialize, persisting the driven vehicle + the event's stem-keyed identity immediately (a dev-car session leaves the remembered vehicle alone; a cruise leaves `last_event` alone; a failed load records nothing). Headless smoke accepts a bound profile (`profile=<id>` on the record when present — absent otherwise, records bit-identical). `last_event` is *not* launched — Quick Race is F17 scope. Tests +11 (`tests/profile.rs`): id/name/ambiguous/unknown resolution, create-binds-and-marks-active, implicit `active` bind + empty-store none, corrupt `active` degrades profile-less, backup recovery heals the main file, flag-over-remembered precedence, session-start persistence of vehicle + `EventKey` through `load_session_world`, dev-car/cruise non-clobbering, per-profile file isolation, failed-load records nothing. Candidate pending external check. |
| F16-B | active | F16-A | Split into B.1 (reward import + authoritative consumption — implemented below), B.2 (availability derived state — implemented below) and B.3 (vehicle/paint selectability — implemented below). Remaining against the parent: availability *enforcement* is F17's menu flow (both queries exist and warn on locked `--event`/`--car` launches); `vpmoonrover` has no authored rule (UNK-3); Pro points (DRV-4/UNK-8) unimplemented; AC01's two-profile restart observation and AC06's deliberate-delete UI are F16-C/F17 evidence. |
| F16-B.1 | implemented | F16-A | Reward/unlock import + authoritative result consumption (DSN-16). `mm2_content::reward_table` maps `race/<city>/<city>_rewards.csv` into a `RewardTable`: milestone rows keyed `{blitz,circuit,race},half|all` against the catalog's authored family sizes (the indexed-attach fix now decodes the row's `race_type` — a `race,N` row binds `race<N>`, not `crash<N>`), indexed rows bind the event whose stem is `<prefix><N>`, and rows that match no event stay in `diagnostics` rather than dropping silently. `mm2_game::progression`: `Unlock` (`vehicle:<id>` / `paint:<id>:<variant>` — variant is the zero-based paint index, measured under VEH-4), `place_requirement` (top-3 Amateur / 1st Professional — RACE-3/CHK-3/VEH-3/VEH-4 documented wording), `record_eligibility` (DRV-6 extension: dev world, dev car, gameplay-affecting `DevOverrides`, mounted mods → ineligible), `apply_result` (indexed + milestone rules read per-rank `beaten` flags; the `unlocks` set dedups re-grants, AC02). `EventRecord` gains `finishes`/`best_race_ticks`/`best_place`/`beaten_{amateur,professional}`; `ResultLedger` gains generation-scoped `standings_in`/`place_of_in` so a restarted session's place ignores prior generations. `mm2_app::progression::EventRewards` (session-scoped, inserted only on successful event setup, removed on teardown) + `record_session_results` drains the ledger into the bound profile — authoritative `ResultId`s only, never UI (req 3) — filters to the current generation + local participant + `Finished` + event-associated results, skips sandbox/profile-less runs and `--bot` (`ScriptedDrive`) sessions — the scripted driver is evidence tooling, not the player — saves only on change. `mm2-inspect events` prints the normalized summary. Tests +22: game-unit (place criterion, milestone math, dedup, timeout), content (family-aware attach, stray-index diagnostics, denominators), app-integration ×8 (real `load_session_world`→drive-to-finish→profile-on-disk, amateur/pro criteria, ineligible paths, non-local results, bot-driven finish, duplicate delivery). Retail audit 2026-09-21: london+sf each 4 event-bound + 6 milestone rules over Blitz=10/Checkpoint=12/Circuit=10/CrashCourse=13, 0 diagnostics. VEH-3/VEH-4/CC-6 promoted to verified_original — authored rows match help exactly. Candidate pending external check. |
| F16-B.2 | implemented | F16-B.1 | Event availability derived state (DSN-17). `mm2_game::progression`: `EventGate` (`Open`/`AfterAll(EventKey)`s), `AvailabilityTable` (`rows` in catalog order + `diagnostics`), `EventAvailability{unlocked, customizable, blocked_by}` evaluated per query off the persisted `beaten` flags — sandbox profiles evaluate unrestricted (spec req 5). `mm2_content::availability::availability_table` builds it from the catalog: checkpoint rows gate in authored-order sets of three (CHK-2/CHK-3), crash rows read the authored `Description` tags — lessons open, `midtrm<N>` gates `lesson{3N-2..3N}`, `final` gates all midterms (CC-2/CC-3) — Blitz/Circuit open; unreadable tags/missing groups fail open + diagnosed. Wiring: `EventSetup.availability` → `EventRewards.availability` (session-scoped like the reward table); a `--event` launch of a still-locked event runs but warns naming the un-beaten prerequisites (enforcement is F17's menu, not the CLI); `mm2-inspect events` prints the open/gated split + each gate's prerequisites. Tests +10: game ×5 (set gating, partial-beat blocking, RACE-3 customizable flag, midterm group, sandbox unrestricted, uncatalogued key), content ×4 (set chunking, tag arithmetic incl. partial groups, unrecognized tag diagnostic, midterm-less/final fallbacks), app ×1 (locked launch reaches Countdown with the gate visible on the resource). Retail (install `fnv1a64:e91e6cd4b2ae30d9`, `mm2-inspect events` both cities): 32 open / 13 gated / 0 diagnostics — `race3-5←race0-2`, `race6-8←race3-5`, `race9-11←race6-8`; `crash3←crash0-2`, `crash7←crash4-6`, `crash11←crash8-10`, `crash12←crash3,7,11` — matching CHK-2/CHK-3/CC-3 exactly. Candidate pending external check. |
| F16-B.3 | implemented | F16-B.1 | Vehicle/paint selectability derived state for F17's garage (DSN-18). `mm2_game::progression`: `VehicleGate`/`PaintGate` (`Open`/`Reward`), `GarageRow` (id + `listed` + gates + verbatim `UnlockScore`/`UnlockFlags` audit fields), `GarageTable::{evaluate,of,row}`, `VehicleAvailability{unlocked, paints[]}` — evaluated per query off the persisted `unlocks` set, sandbox unrestricted (spec req 5). `mm2_content::garage`: `garage_table(catalog, &[&RewardTable])` unions every city's authored grants (a London unlock opens in SF's garage), `scan_garage(vfs)` composes catalog + `race_cities` scans + reward tables and folds reward diagnostics in. Semantics: `vehicle:<id>` grants gate the car (VEH-3), `paint:<id>:<variant>` gates the zero-based `Colors` index (VEH-4 — the index-base question is now measured closed: vpvwcup variant 5/6 land on "Team Angel"/"Team MS", the documented Angel/Microsoft cup paints); a locked vehicle reports all paints locked and unlocking the car does not open gated paints. Roster membership (`GarageRow::listed`) is the canonical `tune/*.info` scan — designed reading: fallback-extension or metadata-less entries (vpmoonrover's `.inf`, UNK-3; pkg/tune-only dev leftovers) are unlisted but still evaluate. Grants for uncatalogued ids/out-of-range variants → diagnostics, never dropped. `CatalogEntry::locked` (`UnlockScore|UnlockFlags` ≠ 0) was measured wrong for this purpose — nonzero on `vpbus`/`vpbullet`/`vpcentury`/`vpcop`/`vpsemi`/`vppanozgt` but zero on six reward-locked cars — and is replaced by raw `unlock_score`/`unlock_flags` + `canonical_info` (VEH-5/UNK-6 recorded). Wiring warn-don't-enforce like the locked `--event` launch: `mm2_app::profile::vehicle_gate_note` → `VehicleGateNote::{Uncatalogued,Unlisted,Locked,LockedPaint}` warns once at launch on a gated `--car`/remembered selection; `--list-cars` and `mm2-inspect cars` print the `gate` column (`open`/`reward`/`unlisted`, `+Ng` gated paints) + authored `s/f` fields. Tests +10: game ×6 (fresh-profile gates, vehicle grant opens car not gated paints, paint grant opens exactly its index, unknown ids inert, sandbox unrestricted, uncatalogued row), content ×3 (grant→gate mapping + unlisted flags, off-catalog/out-of-range diagnostics, two-city union through `scan_garage`), app ×1 (`vehicle_gate_note` notes locked/gated-paint/unlisted/uncatalogued, grants clear, sandbox silent). Retail audit (`mm2-inspect cars`, install `fnv1a64:e91e6cd4b2ae30d9`): `reward` = exactly the 8 VEH-3 locked cars; gated-paint counts match the authored rewards (vpvwcup 4+3g for variants 4/5/6); vpmoonrover + dev leftovers `unlisted`; 0 garage diagnostics. Candidate pending external check. |
| F16-C | active | F12-B, F13-B, F14-B, F16-B | Split: C.1 (AC01 two-profile isolation + AC04 `.tmp`-recovery app legs — implemented below). AC06's deliberate-delete UI confirmation landed in F17-A.1 — `Screen::ConfirmDelete` + `MenuCommand::Delete`/`ConfirmDelete` + the `X`/`Delete`+West-button path, covered by `profiles_bind_create_and_delete`. Remaining: process-level AC01 evidence (an interactive finish — `--bot` results are deliberately ineligible). |
| F16-C.1 | implemented | F16-B | AC01/AC04 evidence legs. `tests/progression.rs` gains `two_profiles_isolate_progress_across_a_restart`: A drives the authored course to a real finish (production `advance_race` → `record_session_results`; the 900-frame throttle loop is now the shared `drive_to_finish` helper), the app+store drop and the store reopens on the same directory, B binds fresh with zero progress/unlocks/selections while A's earned grant still gates `vpreward` for B through `vehicle_gate_note`, then B drives the same event to its own finish — records + grants land on B alone, A's progress/selections untouched (spec req 6 no-leak, both directions). `tests/profile.rs` gains `an_interrupted_save_recovers_through_the_bind` — a flushed-but-never-renamed revision-3 `.tmp` beats the stale revision-2 main (`recovered_from_backup`), the bind-time heal re-saves it and a fresh load is clean. Review repairs: `Unlock::Paint`'s stale "unverified index base" comment → measured zero-based (DSN-16); VEH-5's nonzero-`UnlockFlags` list gains `vpeagle` (unlisted, `0/1`). `mm2-inspect events` reward coverage now prints `N authored rows → E event-bound + M milestone rules, K diagnostics`, reports a city whose rows are all diagnostics instead of hiding them, and pushes a strict failure when accounted ≠ authored. Retail (`fnv1a64:e91e6cd4b2ae30d9`): london + sf each `10 authored rows → 4 event-bound + 6 milestone (Blitz=10 Checkpoint=12 Circuit=10 CrashCourse=13), 0 diagnostics`. Candidate pending external check. |
| F17-A | active | F01-A, F02-A, F11-A, F16-A | Split: A.1 (menu shell — profile/mode/content selection over the real catalogs), A.2 (Quick Race — DRV-8's `last_event` launch), A.3 (driver-name text entry), A.4 (mouse navigation) and A.5 (Race Records screen + AC05 capability audit) and A.6 (condition options — weather/time/traffic picks through `SessionConfig::customization`, RACE-3 gate + RACE-4 cruise), all implemented below. Remaining: A.6's deferred option fields (ped/cop density need F19/F20 consumers; Circuit laps/opponents need authored writers), Quick Race customization, F17-AC03's full keyboard/gamepad + visible-focus evidence leg. AC05's denominator is `docs/research/menu.md` — every documented capability is implemented or tracked. |
| F17-A.6 | implemented | F17-A.5, F18-A.2 | Menu condition options (UI-2's screen, RACE-3's per-event gate + RACE-4's always-open cruise leg). `Screen::Customize{target, conditions, densities, seed_*}` lists Weather / Time of day / Traffic density / Start rows — selectors cycle 0-3, density steps in quarters (`step4`/`step_density`; authored seeds between steps snap to the nearest step on the first press). Cruise city rows gain an always-open `  options` row; every EventList event gains a `  options` row gated on `EventAvailability.customizable` — RACE-3's beaten flag — disabled with the reason (no bound driver / `beat this race…` / the event's own launch gate / out-of-range authored values via `authored_seed`, which distills `RaceParams` the same way `event_params` does and refuses to fabricate a seed). Event seeds come from the selected difficulty's authored block. `SessionConfig::customization: Option<SessionCustomization>` carries the picks: `effective_conditions` prefers them over authored `EventParams::conditions` (the `.ltNN` lighting binds the picked slot; `EnvironmentReport.source = Customized`), and `load_ambient_traffic`'s density chain takes `customization.densities.traffic` first (its layering stays an implementation choice, UNK-12). DRV-6 maps onto the field's presence: the menu sets it only when picks ≠ seed, so an unchanged visit launches a record-eligible default run; `record_eligibility` returns the new `Ineligible::Customized` for a genuinely customized session. `LaunchCustomize`/`Cycle{Weather,TimeOfDay,TrafficDensity}` ride the shared `MenuCommand`/`apply` path — keyboard, gamepad and mouse reach it identically. Deferred: ped/cop density (no F19/F20 consumers — the field rides the authored seed), Circuit laps/opponents, Quick Race options. Ledger: DSN-29; `docs/research/menu.md` marks the capability implemented (partial). Tests +7: `tests/menu.rs` ×4 (options-row gating across unbound/unbeaten/CHK-3-locked/incomplete, seeded screen + cycled picks landing on the launched `SessionConfig` with `EnvironmentReport` slot 4/`Customized` + `Ineligible::Customized`, unchanged visit → `customization: None` + eligible, cruise options open profile-less), `tests/environment.rs` ×1 (customized (3,3) binds `lt15` over authored (1,2)); `mm2_game` ×2 (`effective_conditions_prefers_the_player_customization`, `record_eligibility_refuses_customized_conditions`). Candidate pending external check. |
| F17-A.5 | implemented | F17-A.4 | Race Records (DRV-5's first leg) + the F17-AC05 capability audit. `Screen::Records{city,table}` lists the bound driver's persisted `EventRecord`s sorted deterministically (city → authored table order → stem): each row shows `best <time> place <n> x<finishes>` plus `[A]`/`[P]` beaten marks, and re-launches the event through `Session::begin` when it still resolves — gated rows name the unbeaten prerequisites, `Incomplete` rows their missing files, catalog-absent stems report themselves, Crash Course stays F21-gated. Two filter rows (`City:`, `Race type:`) cycle `all` + the values present in the records via Left/Right/Activate (`cycle_record_filter` + `cycle_choice`; authored table order, not lexicographic). Root gains `Race Records` (disabled unbound — records are per-driver) and `Driver's Stats` (tracked-missing — nothing persists aggregate stats). The documented screen's Amateur Times / Pro Times / Pro Points sort keys are deliberately absent: `EventRecord` stores one best time and no points field exists (DRV-4 formula is UNK-8) — shown fields are persisted data, not fabricated columns. Shared refactor: `availability_reason` now backs EventList, Quick Race and record rows identically. `docs/research/menu.md` audits every documented original menu capability (UI-1..5, DRV-1..8, CTL-8) as implemented/tracked/open — the AC05 denominator. Review repair folded in: `menu_mouse` right-click now returns after queuing `Back` — a stale hover `FocusAt` no longer applies post-pop and clobbers the parent's restored focus. Tests +5 (`tests/menu.rs` 21 total): `the_records_screen_shows_persisted_results_and_relaunches` (seeded finishes → `1:30.5`/`place 1`/`x2`/`[A]` → activate lands `Event(checkpoint:0)`), `unresolvable_records_stay_listed_with_their_reasons` (gated/incomplete/absent/crash records all disabled with reasons, sorted order, no launch), `records_filters_narrow_and_widen_the_list` (both filters cycle and wrap; Enter cycles like Right), `a_fresh_profile_opens_records_to_the_empty_state` (per-driver isolation), `a_right_click_backs_out_without_clobbering_the_restored_focus` (regression). Candidate pending external check. |
| F17-A.4 | implemented | F17-A.3 | Mouse navigation (F17 spec req 5's mouse leg). `menu_mouse` runs before `menu_input` in the same chain: reads the primary window's cursor, maps logical → physical through the camera's target scaling factor (falling back to `window.scale_factor()` — `computed.target_info` is a render-side product headless runs never see), clips to the camera viewport when one is resolved, and hit-tests `ComputedNode`/`UiGlobalTransform` rects of `MenuRow`-tagged row entities — the same space `bevy_ui`'s picking backend uses (layout runs in PostUpdate, so the test is at most one frame stale). Hover → `MenuCommand::FocusAt(index)` — edge-triggered, only a *moved* cursor asserts focus, so a resting cursor never fights keyboard/gamepad. Left-click → `FocusAt` + `Activate`: a click on a disabled row surfaces its reason instead of navigating. Right-click → `Back` from anywhere — the mouse's Esc. Commands queue on `MenuShell::pending` and execute through the one `apply`/`MenuEffect` loop; `reopen` and the inactive gate clear it so a stale click can't fire into a session or a reopened menu. `menu_present` tags each row with `MenuRow{index}` and stretches rows full-width so the hit box covers the line, not just glyphs; pause/results overlay matches fold `FocusAt` into their no-op arm. Tests +2 (`tests/menu.rs` 16 total): `the_mouse_focuses_rows_and_clicks_drive_the_same_commands` (1:1 MenuRow↔`shell.rows` mapping, empty space focuses nothing, hover→focus, resting cursor yields to ArrowUp without re-asserting, click→`Push(CruiseCity)`, right-click→`Back` to Root) and `a_click_on_a_disabled_row_shows_its_reason` (Options click → `not implemented yet (F23)` status, no navigation). Candidate pending external check. |
| F17-A.3 | implemented | F17-A.1 | Driver-name text entry. Profiles → `New driver` pushes `Screen::NewProfile { name }` instead of auto-naming `Driver N`; `menu_input` drains `MessageReader<KeyboardInput>` every frame — on the entry screen `KeyboardInput.text` appends OS-resolved characters (layout/Shift/repeat honoured, `Type(c)` per char) and `key_code == Backspace` emits `Erase`; off-screen the stream is still drained so a nav key's text can't leak into a freshly opened field. The screen is a text field, not a row list: nav bindings are off (Space types a space rather than activating; arrows move no focus), Enter → `Activate` (empty/whitespace → `type a name first` status), Esc/East → `Back`, South → `Activate` for gamepad-less-keyboard use. `apply` enforces ASCII-only (bundled font can't draw more; a stored tofu name is worse than a refused char — reason on the status line) and `MAX_NAME_CHARS`; `create_profile` runs the store's `resolve(ProfileRequest::Create{rank=menu difficulty, Standard})` — success binds (`MenuEffect::Bind`), refreshes the list, pops to Profiles with `created <name>`; failure keeps the screen open, buffer intact. `menu_present` draws `Name: <buffer>_` plus per-screen footer text. Tests +3 (`tests/menu.rs` 14 total) driving real `KeyboardInput` messages: `new_driver_entry_types_edits_and_binds` (types `Ada Lovelacex`, Backspace → `Ada Lovelace`, asserts the drawn `Name: Ada Lovelace_` line, Enter → bound profile persisted in the store), `new_driver_entry_refuses_empty_and_cancels` (empty and whitespace-only Enter → `type a name first`, Esc cancels, reopen starts empty), `new_driver_entry_bounds_and_filters_input` (non-ASCII refusal, arrows inert, Space types, 32-char cap); the existing `profiles_bind_create_and_delete` leg now types `Dave`. Rendered: `--menu --frames 90 --screenshot` on Metal/Apple M1 → `status=pass bytes=172070`, root menu intact (entry-screen capture is interactive-only — input is frozen during `--frames`). Candidate pending external check. |
| F17-A.2 | implemented | F17-A.1 | Quick Race (DRV-8). `menu::quick_race_row` adds a root row between Cruise and Events: with a bound profile carrying `selections.last_event` it resolves the stem-keyed `EventKey` back through the live catalog (`table`+`stem` match → `EventRef`, never the stale row index) and launches it through the same `Session::begin` path the event list uses. Disabled-with-reason legs: no bound profile, no event played yet, Crash Course key (not loadable, F21), missing `city/*.psdl`, stem no longer in the catalog, `Incomplete` records, availability gate (`beat <stems> first`). The documented original's vehicle-select step is folded into the root's persistent vehicle/difficulty selections — enhanced-layout choice, recorded in code. Tests +2 (`tests/menu.rs` 9 total): `quick_race_replays_the_last_event` (bind → Events→race0 launch → `note_session_start` persists the stem key on disk → quit → `Quick Race: Checkpoint #0 (race0)` enabled → activate lands `SessionMode::Event(checkpoint:0)`), `quick_race_reports_an_unresolvable_last_event` (gated race3 / incomplete race2 / unknown race99 all disabled with reasons, activation never launches). Rendered evidence: `--new-profile QR --city sf --event checkpoint:0 --headless --frames 300` wrote `last_event=sf/checkpoint/race0`, then `--menu --profile driver-0 --frames 90 --screenshot /tmp/mm2-menu-qr.png` → `smoke=visual world=menu status=pass bytes=172980` on Metal/Apple M1 — the enabled row draws under Cruise with `Driver: QR (driver-0)` bound and the remembered VW New Beetle restored. Candidate pending external check. |
| F17-A.1 | implemented | F17-A deps | `mm2_app::menu`: `MenuShell` (screen stack + focus + rebuilt row model + launch selections), `MenuData` (profile-store handle + lazily-scanned cities/`VehicleCatalog`/`GarageTable`/`EventCatalog`/`AvailabilityTable` — scans once, VFS is static), `MenuCommand`/`MenuEffect` (model emits, `menu_input` executes — launches resolve `load_by_id` then `Session::begin`; binds/unbinds write `ActiveProfile`), `menu_watch` (reopens the shell when the session reaches `Menu` — quit-to-menu), `menu_present` (`bevy_ui` text tree, focused `›` marker, disabled rows dimmed with their reason). Screens: Root (Cruise/Events/Vehicle/Driver/Difficulty + disabled-with-reason Options+Multiplayer + Quit), CruiseCity (`city/*.psdl` stems, missing psdl names it), EventCity→EventTable→EventList (real `EventRef`s; incomplete rows report missing files, CHK-3/CC gates name the unbeaten prerequisites, Crash Course and empty/errored tables disabled with reasons — nothing hidden, nothing dead-ends), Garage (`listed` roster only; locked/incomplete refused with reasons), Paints (`Colors` names, gated indices disabled), Profiles (list/create/`X`-delete behind `Screen::ConfirmDelete` — F16-AC06's deliberate confirmation; DRV-7's last-profile refusal surfaces as a status line; deleting the bound profile unbinds). Launch builds `SessionConfig{world,mode,difficulty,vehicle,mods_active}` + `SelectedCar`/`TunedVehicle` and `Session::begin`s — the same loader as a direct boot. `mm2_game::progression` gained `AvailabilityTable::of_unbound`/`GarageTable::of_unbound` — the fresh-driver view (restricted, nothing beaten) for profile-less evaluation, sharing the bound-profile evaluators via a `beaten`/`holds` closure. Boot: `menu_mode` = no session-shaping flag and no smoke flag — `--city`/`--event`/`--dev-world`/`--spawn`/`--cam`/`--vehicle-config`/`--bot`/`--nav`/… all stay direct launches; `drive_session`'s Menu-quit only exits when no `MenuShell` exists. Input: arrows/WASD nav, Enter/Space select, Esc/Backspace back (quit at root), X/Delete delete; gamepad dpad+left-stick edge nav, South select, East back, West delete. Shell seeds from `choose_launch` (CLI > remembered > default) and reseeds difficulty from the bound profile's rank (DRV-2). Tests +6 (`tests/menu.rs`): boot parks at Menu + draws rows; cruise→Playing→Esc→menu→repeat with single menu root/player (AC06 menu leg); event rows carry incomplete/gated reasons and the open row launches the real `EventRef`; garage/paint gating carries to `SelectedCar`; profile bind/create/delete/unbind/last-profile refusal; empty install reports reasons and nothing launches. Candidate pending external check. |
| F17-B | active | F17-A | Split: B.1 (pause/resume — implemented below), B.2 (results screen + reward/return leg — implemented below), B.3 (countdown presentation — implemented below). Remaining: C&R/scoring result variants (F17-C scope). |
| F17-B.1 | implemented | F17-A | Pause/resume (F17 req 4's pause leg). `session_control_input` maps `Esc`/pad `Start` onto a new `SessionControl::pause` intent only for a `Playing` session whose authority `allows_pause()` (MP-6 — host/remote and `Countdown`/`Results`/`Failed` keep Esc as quit); `drive_session` takes `Playing → Paused` and its quit/restart arm covers `Paused` and clears stale pause intents. `mm2_app::pause`: `pause_input` owns the `Paused` keyboard (scheduled between the intent reader and the driver so the entering Esc can't re-read as resume), `sync_physics_pause` mirrors the phase onto `Time<Physics>` (Avian's runner skips the schedule — noted quirk: it drains one stale-delta step on the first paused frame), `pause_present` draws a `SessionEntity`-stamped `PauseUi` overlay (in `HudNodes`, follows the active camera) with Resume/Restart/Quit-to-menu|Quit and a disabled `Options` row naming F23, `dev_pause_once` backs `--pause` (one-shot; capture-only evidence flag). `reset_input` is `is_playing`-gated. Tests +5 (`tests/session.rs` 12 total): Esc→Paused→frozen Position/tick over 30 updates→Esc resume, overlay rows drive real intents (Resume/Quit→`AppExit`/disabled-row status), restart-from-pause cleanliness, Host authority keeps Esc=quit, `dev.pause` one-shot; `tests/menu.rs` in-session Esc sites route through the pause Quit row. Rendered: `--city sf --pause --frames 90 --screenshot` → `status=pass bytes=3434617` on Metal/Apple M1, PNG inspected (dimmed overlay over the frozen cruise). Candidate pending external check. |
| F17-B.2 | implemented | F17-B.1 | Results screen + play→reward→return leg (UI-5's screen, AC01's in-session half, AC04's failed-load leg). `mm2_app::results`: `ResultsMenu` (focus/status/dirty — presentation only) + `results_input` owns the keyboard at `Results` (scheduled between `session_control_input` and `drive_session`; Esc/Backspace = Continue) + `results_present` draws a `SessionEntity`-stamped `ResultsUi` overlay (in `HudNodes`, follows the active camera): local outcome line (`ordinal placing of n — m:ss.s`), generation-scoped standings rows (resolved participants placed per DSN-12; unresolved listed as `still racing`, never ranked), granted-reward lines, the `SessionReport` disposition note, and Continue(-to-menu|exit)/Restart rows. `record_session_results` now accumulates a `SessionReport` resource — generation-scoped, processed once (consumed `ResultId`s marked even when a gate refuses), local `Player` only, sandbox/`ScriptedDrive`/`record_eligibility` gates recorded as the on-screen reason instead of only logged; `TimedOut` notes non-recording. `SessionNote` carries `Failed(reason)` through teardown onto the reopened shell's status (`load failed: <reason>`), cleared on the next successful load. `menu_watch` now enforces ownership — shell active only at `Menu` with no pending `restart`, forced inactive otherwise (repairs the latent defect where a restart's transient `Menu` phase reopened the shell over the live session; `menu_input` is `Menu`-gated). `DevOverrides::finish` + `dev_finish_once` sweep the local participant to its next objective once per update until `Results` (countdown lock honoured; `Teleported` untouched so swept segments stay honest) — `record_eligibility` refuses it as `dev override finish`, so `--frames`/`--screenshot` evidence runs can't bank progress. `ordinal` moved to `mm2_game::result` (shared by HUD/results, 11th/12th/13th correct). Tests +11: `tests/results.rs` ×9 (outcome+field, unresolved participants, timeout presentation, profileless/ineligible notes, granted rewards, Results key ownership, continue-quit teardown, restart replays, `--finish`→Results+ineligible), `tests/menu.rs` ×2 (restart doesn't reopen the shell, failed launch returns to menu with the reason). Rendered: `sf --event checkpoint:0 --finish --frames 180 --screenshot` → `status=pass bytes=2573476` on Metal/Apple M1, PNG inspected (overlay over the finished race: `1st of 7 — 0.1s`, six `still racing` opponents, profileless note). Candidate pending external check. |
| F17-B.3 | implemented | F17-B.2 | Countdown presentation (F17 req 4's pre-race leg). `mm2_app::race` gains `CountdownBanner`/`CountdownBannerText`, `COUNTDOWN_GO_TICKS` (one race-clock second), `spawn_countdown_banner` (called from `load_session_world` beside the nav arrow/low-time warning, `SessionEntity`-stamped) and `update_countdown_banner` (wired into `Update` + `HudNodes` so `retarget_hud` keeps it on the active camera). Presentation-only, driven by the authoritative `RaceState`: `Countdown{remaining}` shows `ceil(remaining / RACE_TICK_HZ)` as `3`/`2`/`1`; on release a centered `GO!` shows until the race clock passes `COUNTDOWN_GO_TICKS`; hidden while `Paused`/`Results`, on a stale-generation `RaceState`, or with no race. Authority untouched — `advance_race`, `input_locked`, `RaceStarted`, phase transitions all unchanged; a zero-length countdown shows only `GO!`. Ledger: DSN-19 records the digits/`GO!`/one-second window as a designed presentation policy (no verified original timing claimed; the 3 s default stays provisional). `session.rs` module doc repaired (Results Esc belongs to `results_input`, not `session_control_input` — review nit). Tests +3 (`tests/race.rs` 36 total): digits `3→2→1` then `GO!` then hidden at the window edge; zero-length countdown shows `GO!`, pause hides/resume restores while the frozen clock stays in-window, `Results` hides it; stale generation hides, valid race restores, restart teardown despawns the banner. Rendered on Metal/Apple M1: `sf --event checkpoint:0 --frames 60 --screenshot` → `status=pass bytes=4157777` (digit `2` centered over the grid, PNG inspected) and `--frames 200` → `status=pass bytes=4543805` (green `GO!` at 0.9 s race clock, PNG inspected). Candidate pending external check. |
| F17-C | queued | F12-B, F15-B, F16-B, F17-B | — |
| F18-A | active | F01-B, F06-B | Split: A.1 (preset inventory — implemented below), A.2 (preset→Bevy lighting binding + session-legal selection — implemented below), A.3 (`_fog.csv` per-preset fog tables → `DistanceFog` — implemented below), A.4 (`.sky` dome consumption — implemented below), A.5 (`.cpvs` room-PVS render culling — implemented below), A.6 (`.water` deadly-water consumption — implemented below), A.7 (SDL water-surface marking + `.cpvs`/`.pvshist`/`.lmap` research closure — implemented below). Remaining: `.ldef`/`.lmap` consumers, precipitation/wetness/audio and condition replication (F18-B/C scope — see UNK-24). Menu weather/tod selection landed in F17-A.6 through `SessionConfig::customization`. |
| F18-A.1 | implemented | F18-A deps | Weather/environment preset inventory (F18 req 1's authored side). New pure parsers in `mm2_formats`: `sky` (`.sky` dome record — R4 `lvlSky` field names), `lighting` (`.ltNN` on the shared tune grammar — `Key`/`Fill1`/`Fill2` + packed `Ambient`, `WeatherKind`/`TimeOfDay`/`preset_index`, measured `NN = tod×4 + weather` grid), `ldef` (bake-source path + integer rows verbatim), `cpvs` (`PVS0` RLE — corrected: `index_count−1` lists with compressed end-offsets, fill=raw count/literal=`control−0x7F`; mm2hook `IsRoomVisible` 2-bit layout verified 1340/1341 London self-visible vs 888 under the doc offset reading; bounded decode + `validate`), `PvsHist` (`from to weight` text — the archive entry is DAVE-compressed storage, not file format), `water` (level + room refs), `lmap` (`LMP0` + per-room i32, `0xCDCDCDCD` sentinel preserved). `mm2-inspect weather [--city] [--strict]` audits all 102 discovered files (denominator unfiltered: 74 expected = 21/city × 2 + 32 `amb_*`; 28 extras incl. 23 `.cpvs` variants, three named `.ldef`s, `city/phys/j01.sky`, `sf082100.pvshist`) with cross-checks — dome→`geometry/*.pkg`, `amb_*`↔`sky_*` 32/32 name pairing (inferred), ltNN name↔slot + 16/16 coverage, cpvs `lists==rooms+1`/lmap/pvshist/water refs vs `city/<stem>.psdl`; authored anomalies (self-invisible rooms, sf.lmap 1125 vs 1171, sentinel, dev-path sources) are notes, so stock retail `--strict` exits 0. Retail audit: 102/102 parsed, 0 issues — measured facts recorded in `docs/research/environment.md`, ledger rows WLD-21/22/23 + narrowed UNK-1 + new UNK-24. Tests +21 (parsers incl. malformed-input cases). First candidate `751485d` failed external review on recorded-evidence defects only (implementation claims independently re-verified): docs said "22 per-city files"/"38 extras"/"+9 unit tests" vs measured 21/28/+21. Repaired 2026-09-22 by re-measuring the census and test count and correcting `environment.md`/`LAST_ITERATION.md`/this row; the stale commit-message wording on the recorded candidate is disclosed in `LAST_ITERATION.md`. Candidate pending external check. |
| F18-A.2 | implemented | F18-A.1 | Preset→Bevy lighting binding + session-legal selection (F18 req 1's mapping leg, partial req 6 CLI leg). `mm2_formats::lighting`: `LightSpec::to_light_dir`/`travel_dir` — R4's recovered `setLightDirectionInv` convention (to-light `= (−cos h·cos p, −sin p, −sin h·cos p)`; Bevy forward gets the negated travel dir). `mm2_game::effective_conditions` is the single shared resolver — authored `EventParams::conditions` win while an event runs (RACE-2), `SessionConfig::conditions` is the cruise/dev fallback. New `mm2_app::environment::spawn_environment` runs in `load_session_world` after event resolution: selects `city/<stem>.ltNN` on the measured `NN = tod*4 + weather` grid (WLD-21), spawns three `DirectionalLight`s (authored colours verbatim, key-only shadows — designed) and a `GlobalAmbientLight` from the packed BGRA ambient; uniform 15 000 lux per light and ambient brightness 2 000 are designed scales anchored to the previous fixed rig (recorded in `environment.md` + DSN-28). Missing/unparseable preset spawns the pre-preset rig and sets `EnvironmentReport.fallback` — explicit F18-AC06 diagnostic; report + entities are session-scoped (`SessionEntity`-stamped, report removed on teardown). CLI: `--weather`/`--time-of-day` 0-3 (invalid exits 2; event mode warns they only feed the fallback; both excluded from `menu_mode` like every session-shaping flag). Smoke records carry `env=ltNN(<name>|fallback)` on city worlds. Tests +6: `lighting.rs` direction-convention test (noon up, negation, unit vector, below-horizon positive pitch), `mm2_game` precedence test, `tests/environment.rs` ×4 over a synthetic city (slot-8 bind incl. authored colours/direction/ambient, missing-preset fallback report, authored-event precedence over configured (3,3), off-schema fields → issues not fallback). Retail evidence (fingerprinted install): `sf.lt00`→`clear-morning`, `sf.lt06`→`foggy-noon`, `sf.lt15`→`rainy-night`, `london.lt05`→`cloudy-noon` bound through the real path (`environment lighting bound` + `env=` in `smoke=headless-physics` records); rendered captures at frozen `--cam` (Metal/Apple M1) show visibly different lighting per preset — honest note: `rainy-night` renders pastel-bright (authored pastel fills + grey-80 ambient + below-horizon key, no fog yet — fog isn't in `.ltNN`, UNK-24); `--weather 4` exits 2. Deferred: `.sky` dome, fog, `.cpvs` PVS, precipitation/wetness/audio, menu pickers, condition replication — F18-A stays active. Candidate pending external check. |
| F18-A.3 | implemented | F18-A.2 | Authored per-preset fog tables → camera `DistanceFog` (F18 req 1's fog leg — the files existed outside every audit denominator). `mm2_formats::fog`: `FogTable`/`FogRow`/`FogIssue` parser for `city/<stem>_fog.csv` — the `fog red,fog green,fog blue,fog start,fog end,description (ignored)` header then 16 `r,g,b,start,end,label` rows; row index is the same `tod*4 + weather` slot as `.ltNN` (verified: 16/16 row labels equal the `.ltNN` names on both retail cities; the landing site is MM2Hook's recovered `lvlSky::FogColors/FogNearClip/FogFarClip[16]` — WLD-24). `spawn_environment` reads `city/<stem>_fog.csv` through the VFS at the same effective slot the lighting preset uses — authored event and `SessionCustomization` precedence apply identically — and `load_session_world` attaches `DistanceFog` (`FogFalloff::Linear{start,end}` — inferred curve, authored distances; RGB→`Color::srgb`, clamped 0-255; directional glow `Color::NONE`) to both session cameras. Failure policy is explicit, never a default: missing/`unparseable`/no-row/`degenerate` (`end <= start`, non-finite, negative) → no fog + `EnvironmentReport.fog.absent` naming the reason; table anomalies count into `fog.issues` without blocking a valid row; a fallen-back lighting preset still binds its fog row. `mm2-inspect weather` census grows to 105 files (`<stem>_fog.csv` expected ×2, `sf_fog_orig.csv` extra — parsed, not cross-checked: different label convention, wider authored bands, reads as an earlier revision) with a new 16/16 label↔slot cross-check; `--strict` stays green on retail (105/105, 0 issues). Smoke `env=` gains ` fog=<start>-<end>`/`fog=none`. Tests +3 (`tests/environment.rs` 8 total): authored row binds on both cameras (slot path/RGB/linear band asserted), authored-event conditions select the fog row over configured, degenerate band binds nothing with `absent="degenerate"`; the existing missing-preset test now asserts `fog=none`/`absent="missing"`; `mm2_formats::fog` unit tests +6. Retail evidence (fingerprinted install): `env=lt00(clear-morning) fog=650-1000` on sf, `fog=220-320` on london through headless smoke records; frozen-`--cam` capture pair at sf `foggy-noon` (authored 10-120 m) vs `clear-noon` (600-1000 m) — skyline dissolves into authored grey vs fully resolved (Metal/Apple M1, PNGs inspected; sky clear-colour stays designed blue — no `.sky` dome yet, disclosed in `environment.md`). Ledger: WLD-24 + DSN-30; `environment.md` gains the `_fog.csv` section and updated census. Candidate pending external check. |
| F18-A.4 | implemented | F18-A.3 | `.sky` dome consumption (F18 req 1's dome leg). `mm2_app::environment::spawn_sky_dome` runs in `load_session_world` beside `spawn_environment` at the same effective slot: `city/<stem>.sky` → `SkyDef` → `geometry/<model>.pkg` through the VFS → the dome's paint job `slot % jobs` (the measured 16-slot `tod*4 + weather` texture grid — `sky_dome_l` job i = `skylondon_{c,p,f,r}{a,n,d,m}_l`; `sky_dome` the `sky_*_f` equivalents) → render parts via the new crate-visible `city::pkg_paint_parts` (`pkg_to_parts` gained a `paint` parameter selecting `paint * per_job + offset`; prop callers stay on 0). The dome spawns `SessionEntity`-stamped, render-only (no collider/fragments), unlit + `fog_enabled=false` + `cull_mode=None` + `NotShadowCaster` — the authored texture is the sky's final colour — scaled from its measured ~43 m extent to a designed 900 m radius; `drive_sky_dome` (Update, after the camera systems) re-centres it on the active camera in XZ at the authored `HatYOffset` and advances `RotationRate` rad/s. The field readings are designed, not recovered (UNK-24): world-height / vertical-squash / radians-per-second, camera-centring horizon policy. Failure policy explicit: missing/unparseable `.sky`, unresolvable/unparseable/empty model, non-finite transform → no dome + `SkyReport.absent` (`missing`/`unparseable`/`degenerate`/`model unavailable`/`model unparseable`/`empty`); a missing dome texture warns and draws the shared fallback (prop policy). `env=` gains ` sky=<model>:<texture>`/` sky=none`. Tests +8 (`tests/environment.rs` 16 total): authored bind (slot 7 → paint 1 → `dome_1`, scale/authored fields/session stamp/unlit+fog-exempt+double-sided material), camera-follow + authored rotation step, and the absent legs (missing/unparseable `.sky`, unresolvable model, <16-job wrap, missing-texture fallback, degenerate transform). Retail evidence (fingerprinted install, 2026-09-23): `env=lt00(clear-morning) fog=220-320 sky=sky_dome_l:skylondon_ca_l` london / `fog=650-1000 sky=sky_dome:sky_ca_f` sf through `--headless` smoke records; pitched-up frozen-`--cam` captures on both cities show the authored dome textures unlit/unfogged (Metal/Apple M1, PNGs inspected). Ledger: DSN-31; `environment.md` gains the dome binding + updated open list. `.cpvs`/`.lmap`/`.ldef`, precipitation/wetness/audio and replication stay open. Candidate pending external check. |
| F18-A.5 | implemented | F18-A.4 | `.cpvs` room-PVS render culling (F18 req 1's visibility leg). `mm2_app::pvs` (new): `CityRoom(u32)` tags every per-room render group at spawn (authored id = `Psdl::rooms` index + 1 — the same space the lists index; an explicit `Visibility::Inherited` is inserted too, `Mesh3d` doesn't require it so the cull query would match nothing); `CityPvs` holds the parsed `Cpvs` plus per-room perimeter cells (`mm2_formats::cpvs`/`psdl` — parser crates untouched). `apply_city_pvs` runs in Update after the camera systems (windowed + headless smoke, where `chase_follow` now runs so the camera path is exercised identically): each frame it resolves the union of rooms whose authored XZ perimeter contains the active camera transform *or* the player vehicle's `Position` — retail `sdlPage16::PointInPerimeter` is the same 2-D test and `FindRoomId`'s `previousRoom` is a recovered search hint, so a rescan-per-frame gives the same answer without ordering state — decodes each source's list once on change, and rewrites `Visibility` only when the source set changed, the toggle flipped, or new `CityRoom` entities arrived (`Added` leg — the first update precedes the deferred session spawns). Unioning means a stacked-deck or camera-lagged pick over-shows, never under-shows; the player leg keeps the street under the car a source and is a headless run's only position. `PvsEnabled`/`--no-pvs` mirror retail `sm_EnablePVS`/`EnablePVS(false)`, default on; disabled, unresolved, or list-less sources bypass entirely, and a missing/unparseable `<stem>.cpvs` (mod cities may ship none) yields no `CityPvs` resource — unculled and logged, never fabricated. Colliders are physics, never culled. Session-scoped: inserted by `load_session_world`, removed in `drive_session` teardown. Smoke record gains ` pvs=<room>r/<hidden>h/<tagged>` (` pvs=off` when disabled; absent without a table — dev-world records stay bit-identical). Ledger: DSN-33; `environment.md` records the runtime binding and the authored-table caveat (elevated viewpoints see authored-invisible rooms — the same holes retail renders from the same table). Tests +5 (`pvs.rs`): tiled-room resolution + culling, overlapping-room union, unresolved/disabled bypass + miss counter, full apply path (hide/show on camera move, `culled`/`tagged` counts), player-position fallback. Retail evidence (fingerprinted install, 2026-09-23): sf `--headless --frames 900 --spawn=-1319.5,80,214` → `pvs=982r/6378h/7324` tracking the driven car's room; london `--frames 600` → `pvs=68r/7422h/8254`; frozen-`--cam` A/B on/off captures (Metal/Apple M1, ImageMagick AE) — street level ≈capture noise (360 px over 3.7 MPX vs 1.5 kpx same-run baseline), a 75 m aerial shows ~28 kpx of real difference all in distant skyline rooms (authored-table conservatism — retail draws the same holes). Passed external review 2026-09-23 with two non-blocking defects, both repaired in the next iteration: `--no-pvs` was silently ignored headless (the `PvsEnabled` resource was inserted only on the windowed path — the flag now rides `SessionConfig::dev.no_pvs` and `--headless --no-pvs` reports `pvs=off`; the dead resource is gone), and the camera query gained `With<Camera3d>` (a transition-frame `Camera2d` could add a wrong source room — bounded to over-show, now excluded, regression-tested). Candidate pending external check. |
| F18-A.6 | implemented | F18-A.5 | `.water` deadly-water consumption (F18 req 1's water leg). The refs' meaning was verified before designing the consumer: they are 1-based PSDL room ids (same space as `PerimeterPoint.room`/`road_rooms`/`.cpvs` lists) and all six retail refs resolve to flat attribute-less perimeter rooms at the water plane — london 345/351/356 are Thames reaches at −4.0 under the authored −3.8 level, sf 228/399/401 ocean reaches at −2.0 under −1.9; the 0-based reading lands sf's 401 on a road tunnel at y≈30 — refuted. London's authored BAI lanes run to −22, so a global "below level = deadly" rule is refuted too: the level applies *inside listed rooms only*. `mm2_app::water::CityWater` (new, session-scoped like `CityPvs`) is built in `load_city` beside the `.cpvs` block — sibling `city/<stem>.water` through the VFS → each ref → its room's XZ perimeter (`city::room_poly`, now shared with `pvs.rs`) + bound `max(level, room-top)` (honours mm2kiwi's "deadly water at any height" for elevated mod water; 0.1 m slack) → `LoadedCity.water`; `load_session_world` inserts the resource and `drive_session` teardown removes it. `track_recovery`'s `ground_contact` now takes the `CityWater`: a grounded wheel whose contact point is exposed classifies water whatever its collider `drag` says, and a car under a listed bound with no contact — clipped through the plane — is `Submerged` (accrues the dwell) rather than airborne; a deck above the bound inside the same XZ stays dry. Missing/unparseable record, non-finite level, or all-unresolvable refs → no resource (material classification still covers ordinary water — never a fabricated kill zone); skipped refs are reported (`skipped` count + `wtr=<level>/<rooms>r+<n>s` in the smoke record; absent field = no record, so dev-world records stay bit-identical). Designed policy end-to-end (DSN-34) — the original consumer is unrecovered (UNK-24). Review repair folded in: the zero-room drop was documented but not enforced — `load_city` inserted a `CityWater` with `rooms: []` when every ref failed; it now warns and yields `None` like a missing record, so a present `CityWater` always lists ≥1 deadly room. Tests +13 (12 in the candidate, +1 review-repair regression): `water.rs` unit ×4 (1-based refs, skipped-not-reinterpreted, room+bound exposure, elevated-room bound); `pvs.rs` `system_ignores_2d_cameras` (the A.5 repair); `tests/recovery.rs` ×4 over a synthetic deadly-water city (contact inside a listed room → water event despite dry material, airborne-under-bound → submerged + recovery, deck above bound → dry, non-listed below-grade room untouched — the resource-absent path stays covered by the pre-existing recovery tests, no dedicated case); `tests/session.rs` teardown removes `CityWater`; `tests/import_pipeline.rs` ×3 (record binds — level/rooms/skipped; missing/unparseable/non-finite → none; all-unresolvable/no refs → none, the repair regression). Retail evidence (fingerprinted install, 2026-09-23): sf/london headless report `wtr=-1.9/3r`/`wtr=-3.8/3r`; staged `--spawn` inside london room 345 / sf room 228 below the bound → `rcv=3w/0f/3r` with `never grounded` on both cities — the airborne overlay fires on authored data. Candidate pending external check. |
| F18-A.7 | implemented | F18-A.6 | SDL deadly-water marking + environment research closure (F18 req 1's water leg, exe side). Binary evidence resolved the two open F18-A questions: (1) `cityLevel::Load` opens exactly one PVS stream — `Open("city", stem, "cpvs")`, no numbered-name construction — so the 23 `.cpvs` extras are offline bake artifacts; measured relations (the audit now reports them): `sf_8`/`london_8`/`london_bad` byte-identical to base, the `_N` files nest `_0 ⊋ _2 ⊋ _4 ⊋ _8=base ⊋ _16 ⊋ _32 ⊋ _64 ⊋ _128 ⊋ _255` (stricter occlusion with N, not distance cuts), `_00`/`_254` independent, `sf082100` an earlier-bake subset. (2) `"Water of Death(tm)"` is logged `[from .water file]` *and* `[from SDL]`: refs bounds-check (`0 < ref < nRooms`) and set the runtime flag (mm2hook `RoomFlags::Water` = 0x4 — the six retail ref rooms carry it in stored `room_flags`), while the SDL pass marks every room whose FIRST attribute is a `TextureRef` to a drowning-class surface; `GetWaterLevel` returns the one global level and the kill is `flagged && pos.y < level`. The per-texture class flag's derivation is unrecovered — `mm2_content::SurfaceTables::is_deadly_surface` approximates it as authored `drag >= water_min_drag` (0.3), which reproduces the retail mark set exactly (42 sf `s_ocean` rooms, 20 london `s_thames`; shallow `water`/`s_pond` never mark) — documented inference, not a claimed original test. `CityWater::build` now takes the loaded surface tables and appends SDL marks (bound = level verbatim; a ref'd room keeps its authored bound); `.lmap` confirmed runtime-loaded by exe diagnostics (values still UNK-24); `.pvshist` has no exe reference (bake-tool data). `mm2-inspect weather` gains the per-variant base-relation classification (note lines) and a per-city SDL-mark count; smoke `wtr=` splits `wtr=<level>/<refs>r(+<n>sdl)`. `mm2_formats::psdl` gains `texture_ref_index` (the shared `data + 256*subtype − 1` decode — `city.rs`'s `decode_texture_ref` now delegates) and the recovered `RoomFlags` bit names on `room_flags`. Tests +5 (`water.rs`: first-attr mark binds + level bound, shallow `water` never marks, non-first TextureRef ignored, ref+SDL dedup keeps the elevated bound, no-tables → no SDL pass). Retail evidence (fingerprinted install, 2026-09-23): headless sf → `wtr=-1.9/3r+42sdl`, london → `wtr=-3.8/3r+20sdl`; `mm2-inspect weather` reports all 23 variant relations and both SDL counts, 105/105 parsed 0 issues. Ledger: DSN-34 extended (marking verified, exposure shape still designed), WLD-23/UNK-24 updated; `docs/research/environment.md` records all findings. Candidate pending external check. |
| F18-B | queued | F18-A | — |
| F18-C | queued | F18-B | — |
| F19-A | queued | F00-B, F01-B, F09-B | `anim/` (95 files: `.anim`, `.skel`, `.mod`, pedanim_*) present; no ped rigs/import. |
| F19-B | queued | F19-A | — |
| F19-C | queued | F19-B | — |
| F20-A | queued | F01-B, F05-B, F10-B | `vpcop` + `_cop` tuning variants exist; no police logic. |
| F20-B | queued | F20-A | — |
| F20-C | queued | F20-B | — |
| F21-A | queued | F02-B, F11-B, F16-B | London crash0–12/exam/final/reverse180 (cab course) and SF crash0–12 + accel/collide/corner/evade/frogger/jump/stunt/exam (stunt course) present. |
| F21-B | queued | F21-A | — |
| F21-C | queued | F21-B | — |
| F22-A | queued | F01-A, F02-B, F11-B | Dev HUD (speed/gear/RPM/grounded/cam pose) exists; no race HUD, city map or markers. |
| F22-B | queued | F22-A | Chase + free cameras exist; no cockpit/mirror/occlusion handling. |
| F22-C | queued | F22-B | — |
| F23-A | queued | F01-A, F16-A | Fixed keyboard + gamepad mapping in `input.rs`; no rebind/settings persistence. |
| F23-B | queued | F23-A | — |
| F23-C | queued | F23-B | — |
| F24-A | queued | F01-B, F02-A | No networking; transport choice deferred to F24 per ARCHITECTURE.md. |
| F24-B | queued | F24-A | — |
| F24-C | queued | F24-B | — |
| F25-A | queued | F02-B, F24-B | — |
| F25-B | queued | F25-A | — |
| F25-C | queued | F25-B | — |
| F26-A | queued | F04-B, F10-B, F11-B, F25-B | — |
| F26-B | queued | F26-A | — |
| F26-C | queued | F12-B, F13-B, F14-B, F26-B | — |
| F27-A | queued | F01-B, F11-A, F25-B | C&R rule matrix unverified. |
| F27-B | queued | F27-A | — |
| F27-C | queued | F26-A, F27-B | — |
| F28-A | queued | F01-B, F03-B, F09-B | City-specific actors/boundaries unmapped. |
| F28-B | queued | F28-A | — |
| F28-C | queued | F28-B | — |
| F29-A | queued | F00-B, F02-A, F03-B | VFS override infra + `docs/modding.md` + `examples/mods` exist; consumer coverage audit owed. |
| F29-B | queued | F29-A | — |
| F29-C | queued | F29-B | — |
| F30-A | queued | F01-C, F02-B, F03-B, F10-B | Native arm64 macOS dev build runs; no perf/soak harness. |
| F30-B | queued | F30-A | — |
| F30-C | queued | F30-B | — |
| F31-A | queued | F00-C | Gap audit can run early once F00-C evidence commands exist. |
| F31-B | queued | all -B slices + F31-A | — |
| F31-C | queued | all -C slices + F31-B | — |

## Blockers

- `MM2_GAME_DIR` / documented install configuration is not established;
  the retail path is currently passed by hand. F00-B.1 should record the
  convention it uses.
- GPU capture exercised by F00-C: `--frames 90 --screenshot` on the dev
  world produced a verified 2.9 MB PNG on this machine (Apple Silicon,
  windowed wgpu). Visual smoke now reports `pass` only after the file
  lands; headless Linux needs `--headless` (visual → `unavailable`).
- No audio device test possible yet: no audio code exists and bevy is
  built without `bevy_audio` (deliberate minimal-features setup; adding
  the feature is a real change for F07, not a config flag to flip
  silently).
- Original rules are unverified for everything outside
  `docs/research/` + `docs/vehicle-handling.md` +
  `docs/original-rules.md`. The ledger marks each fact
  verified_original/documented/inferred/designed/unknown; its UNK list
  (cop AI, C&R mechanics, enum maps, race names, ped behavior, …) stays
  open until evidenced — documented ≠ verified.
- Single writer rule: this worktree shares build artifacts/stash with
  other agents per AGENTS.md — rebuilds must be coordinated.

## Relevant discoveries

- Vehicle catalog (retail install, `mm2-inspect cars`, 2026-09-20): 29
  entries; 21/21 `EXPECTED_STOCK_ROSTER` ready. Extras kept as audited
  incompletes: `vpdb731`, `vpftruck`, `vplafrance`, `vpvw_cup`,
  `vpvwdune`, `vpvwcup_angel` (spare/variant ids missing metadata and/or
  tuning), `vpeagle` (has `.info` — "Silver Eagle Fire Truck" — but no
  model/bounds/wheels; the shipped fire truck is `vpsemi`), `vpvw_dune`
  (display "VW Dune Beetle", missing carsim).
- Race families on the retail install (`race/{london,sf}`): london has
  `blitz0–12`, `race0–13`, `circuit0–11`, `crash0–12`, `exam1–2`,
  `final1–2`, `reverse180`; sf has `blitz0–13`, `race0–11` + `r0`,
  `circuit0–11`, `crash0–12`, `accel0`, `collide0`, `corner0`, `evade0`,
  `frogger0`, `jump0–2`, `ramp`, `stunt0`, `exam1–2`, `reverse180`,
  `dbugps2`. Both cities' `circuit11` are partial (opp/pathset only, no
  `.aimap`); SF `cir1–9` are `_strtpnts` records, not circuit events.
  File kinds: `.aimap` (108), `.aimap_p` (99), `.opp` (612),
  `.pathset` (57), `*waypoints.csv`/`*_strtpnts` (175 csv), plus
  oddities (`bak`, `old`, `ps2`, `.1`/`.4`/`.5` suffixes, `csvs` dir).
  `mm{race,blitz,circuit,crash}data.csv` per city are the authored event
  metadata tables — parsed by `mm2_formats::racedata`: 12 checkpoint /
  10 blitz / 10 circuit / 13 crash-course rows per city, one row per
  selectable event with Amateur + Professional parameter blocks
  (car/time-of-day/weather/opponents/cops/ambient/peds/laps/timelimit/
  difficulty). See `docs/original-rules.md` for the full ledger.
- Race file grammar (F11-A, 2026-09-20): the `race/<city>` classifier
  now lives in `mm2_formats::racefiles` (shared by inventory and the
  event catalog). Waypoint CSVs use `x,y,z,a,radius|poly count,frame
  rate|frane rate,...` headers; `*_strtpnts` are headerless numeric CSV;
  `.opp` is 9-column CSV (x,y,z,brake,fwd/side offsets,target speed,
  speed/side start) with `-a`/`-p` difficulty suffixes; `.aimap` is
  INI-like text; `.pathset` is binary `PTH1`. Crash `crash<N>data.csv`
  headers are inconsistent on retail — `AmbDenisty` typo, named tail
  columns (`Misc`, `cornerspeed`, `chkflags`, `numopp`), and london
  `crash8data.csv` omits the `Filename` label while rows still carry
  it — so the parser keys off the `Event,Checkpoints,TimeLimit` prefix
  and keeps anomalies as diagnostics. `*_rewards.csv` links crash
  sub-event indexes to vehicle/paint unlocks plus Half/All milestone
  rows per mode.
- Pathset grammar (F03-A.1, 2026-09-20): `PTH1` binary — named paths
  of `(u32 attributes, f32 xyz)` points with a kind byte (0 single /
  1 directed-pairs / 2 line-strip) and quarter-metre spacing; props
  name `geometry/<n>.pkg`, decals `texture/<n>.*`, `PATHnn` are route
  labels, `PREFIX:` marks event states. 98/101 retail files parse;
  `blitz10/11` + `london_bridge_blitz10.pathset` are authored
  truncations. See `docs/research/pathset.md`.
- City files: `city/{city,london,sf,sfai,variant}.psdl`,
  `{london,sf}{,_sup,_bak}.bai`, `sfai.bai`, 44 `.pathset`, 42 `.csv`,
  35 `.ldef`, 25 `.cpvs`, 13 `.inst`, 3 `.sky`, 3 `.txt`,
  3 `.pvshist`.
- Audio families: `aud/aud11` (2136), `aud/spchdata` (538), `aud/aud22`
  (388), `aud/dmusic` (101), `aud/cardata` (94), `aud/creaturedata` (21),
  `aud/ambient` (15) — format support unassessed (F07-A/F08-A).
- Animation: `anim/` 95 files — `.anim`, `.skel`, `.mod`, `.rays`,
  `.shaders`, `pedanim_*`, `pedmodel_woman` — support unassessed (F19-A).
- Recent-history focus is vehicle handling (drive probe, steering slip,
  levelling, gearbox) — `docs/vehicle-handling.md` is the authority on
  which deviations from retail numbers are deliberate.
- `mm2-inspect` commands: `scan` (`--strict`), `list`, `resolve`,
  `lookup`, `tex`, `pkg`, `psdl`, `dump`, `cars`, `car` (`--paint`,
  `--json`), `handling` (`--strict`), `validate-cars` (`--all`,
  `--strict`), `inventory` (`--json`, `--strict`), `events` (`--city`,
  `--strict`), `race-defs` (`--city`, `--table`, `--strict`),
  `bai` (`--city`, `--strict`), `aimap` (`--city`, `--strict`),
  `nav` (`--city`, `--route`, `--routes`, `--aimap`, `--turns`,
  `--strict`),
  `pathset` (`--city`, `--strict`), `proprules` (`--city`, `--strict`).
