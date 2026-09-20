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

**Next selected slice: F13-A remainder, F14-A, or F11-C** — iteration
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
   measured against *sunk* props (props were lower than authored —
   the striker bound had *more* clearance, not less); its
   implementation stays, but the "previously ghosted" evidence must
   be re-taken on corrected geometry before F04-C.3 rolls up.

3. **F04-C.2's retail evidence remains unreliable — re-take owed.**
   "Flat-ground settle, repeated hits, pool reclaim" was measured
   against misplaced geometry. Re-run the `--spawn` evidence commands
   on the fixed placement before F04 rolls up to `checked`.

Code lead resolved: `stamp_line_strip` did place each prop at the
authored point verbatim — correct for the *transform*; the missing
piece was the content-space offset between the authored point and the
centred mesh, now the `PropOffset` contract.

Expected next iteration: re-take the F04-C.2 retail runs on the fixed
placement; then return to the queued feature work.

## Task table

| Task | Status | Dependencies | Evidence / reason / next action |
|---|---|---|---|
| F00-A | checked | - | Audit done in this planning pass: gates pass, env/install/toolchain recorded above. Externally checked. |
| F00-B | checked | F00-A | Split into F00-B.1 (inventory) and F00-B.2 (rules ledger); both children externally checked. |
| F00-B.1 | checked | F00-A | `mm2-inspect inventory` landed: versioned report (engine commit + fnv1a64 catalog fingerprint) with expected/discovered/accepted/rejected/unverified counts across 12 families; `--strict` exits 2 with 33 findings on retail (8 incomplete vehicles, 2 partial circuit11, junk/partial records). Externally checked. |
| F00-B.2 | checked | F00-A | `docs/original-rules.md` ledger landed: ~90 classified facts from MM2HELP.HLP (decompiled locally via helpdeco), Readme.rtf, Booklet.pdf and authored data. `mm2_formats::racedata` parses the `mm*data.csv` event tables; inventory cross-checks them (12/10/10/13 rows/city, missing/malformed → rejected). Externally checked. |
| F00-C | checked | F00-B | `mm2 --headless` headless physics smoke (no window/GPU; dev world + real cities via VFS), windowed `--frames`/`--screenshot` visual smoke that awaits the capture file, unified `smoke=` records versioned by build-embedded commit, statuses pass/fail/unavailable → exits 0/3/4. `--city` without data sources is `unavailable`; a requested missing city/vehicle is an explicit failure. Externally checked (review pass 2026-09-20; AC04/AC05 verified by reviewer). |
| F01-A | checked | F00-A | `SessionConfig` (world/mode/event-ref/difficulty/conditions/densities/seed/vehicle/authority + quarantined `DevOverrides`) and `Session` (`Menu→Loading→Ready→Countdown→Playing→Paused/Results→Unloading→Menu`, `Failed` load-time only, MP-6 pause rule enforced from authority) landed in `mm2_game`; `SessionEntity(gen)` ownership markers thread through world/vehicle/HUD/camera spawns; `advance_session_tick` fixed clock; `WorldState`/`ActiveWorld`/`CamStart` replaced. 13 session tests; smoke records now carry `ticks=`. Externally checked (review pass 2026-09-20). |
| F01-B | checked | F01-A | `mm2_game` contract modules landed: `ids` (`PlayerId`, generation-scoped `ObjectId`/`ObjectIdentity`, `AuthorityRole`, `Player`/`PlayerControl`), `surface` (`SurfaceMaterial` carrying authored codes uninterpreted + `SurfaceState` keeping physical vs visual identity separate), `impact` (`ImpactEvent`/`ImpactPolicy`/`ImpactDedup`/`ImpactId`), `telemetry` (`VehicleTelemetry`/`WheelTelemetry`/`DamageSignals`), `result` (`SessionResult`/`ResultId`/`ResultLedger` dedup). `Session` mints generation-scoped ids. `mm2_vehicle`: `VehicleInput.forced_gear`, `VehicleState.engine_load`, `WheelState.contact_entity`, `CollisionEventsEnabled` in `vehicle_bundle`. `mm2_app::contracts`: `collect_impacts` (Avian `CollisionStart`+`Collisions` → bounded deduplicated events, stable-id participants, damage accumulation) + `publish_vehicle_telemetry` (read-only snapshot per fixed step, Playing only); HUD reads the snapshot; smoke records `impacts=`/`dropped=`. 9 game + 4 app real-physics + 2 vehicle contract tests. First candidate failed review on a real defect — `dedup.allow` ran before the severity/manifold filters so a sub-threshold touch consumed the pair cooldown and silently suppressed a re-impact; fixed (dedup now applies only to reportable contacts) with regression test `a_quiet_touch_does_not_suppress_a_real_reimpact`, which fails on the old ordering and passes on the fix. Trailer now carries `DamageSignals`; `engine_load`/edge-trigger docs corrected. Externally checked (review pass at iteration 006/007, commit `d0cca68`). |
| F01-C | implemented | F01-B | `mm2_app::session` module drives the lifecycle in the real app: `load_session_world` (was `Startup` `setup`, moved to lib + gated on `Loading`), `session_control_input` (`Esc` quit→teardown→exit, `Backspace` restart — no menus exist yet, F17), `drive_session` (`Unloading` waits for observable despawn → clears `ImpactFilter` (new `reset()`: dedup map is `Entity`-keyed and the next session may recycle entities; ids/evidence counters restart per session) + `SpawnPoint.trailers` → `Menu` → `begin` or `AppExit`), chained after `despawn_session_entities`. `docs/architecture.md` gained the ownership/scheduling/plugin-attach section (AC06); README controls updated. 5 new integration tests run the production systems headlessly: restart leaves exactly one session (AC01), quit→Menu→AppExit, failed load keeps no player sim + retries/quits (AC02), impact ids/counters restart per session, fixed-tick driving is update-rate independent (AC03). Windowed `--frames 60` dev-world smoke and retail `--city sf --headless` smoke both pass unchanged. Candidate pending external check. |
| F02-A | implemented | F00-B, F01-B | `VehicleCatalog::scan` + `EXPECTED_STOCK_ROSTER` + `stock_audit_failures` + `mm2-inspect cars/validate-cars` exist and ran on retail install tonight (21/21 ready, 8 audited extras). Candidate pending external check; deps not yet checked. |
| F02-B | implemented | F02-A | `mm2_content::convert`, handling measurement (`analysis.rs`, drive probe), recent steering/gearbox/levelling fixes. Candidate; gap list still owed by F02-A audit. |
| F02-C | queued | F02-B | Tooling exists (`handling` roster audit, `validate-cars --all`); owed: run full roster/paint/handling matrix and publish honest original-vs-synthetic coverage. |
| F03-A | implemented | F00-B, F01-A | INST + PSDL placement parsed. Split: A.1 (`.pathset` parser + audit — implemented below), A.2 (prop-rule tables — implemented below). `.opp`/race `.csv` waypoints parsed under F11-A. Every discovered placement-source grammar now has a parser; remaining unmapped sources are feature-deferred: `.cpvs`/`.ldef` → F18, `audio_pathsets/` → F07/F08, PSDL `paths` records preserved raw (semantics unknown). |
| F03-A.1 | implemented | F00-B, F01-A | `mm2_formats::pathset`: PTH1 parser measured on all 101 retail files — named paths of attributed points, kinds 0 single/1 directed-pairs/2 line-strip, quarter-metre spacing; per-point attribute word + `current_path`/`selection` cursors preserved raw (inferred dev-tool state, UNK-20). `Pathset::validate()`: unknown kind, odd directed-pair count, non-finite point, out-of-range cursor, empty name — zero issues on retail (66 empty paths are authored data, not flagged). `mm2-inspect pathset <install> [--city] [--strict]`: denominator = every discovered `.pathset` (101 — no filtering): 98 parse, 3 authored truncations fail (`race/london/{blitz10,blitz11,london_bridge_blitz10}.pathset`); name cross-check resolves `geometry/<n>.pkg`/`texture/<n>.*` with `PREFIX:` state decorations stripped and `PATHnn` labels skipped — 120/126 names resolve, 6 dead refs all in `city/phys/` + `bak/` files; `--strict` exits 2. `scan` recognizes `.pathset`. `docs/research/pathset.md` + ledger WLD-12/UNK-20. Candidate pending external check. |
| F03-A.2 | implemented | F03-A.1 | `mm2_formats::proprules`: `PropDefs`/`PropRules`/`PropGroups`/`PropLodStats` parsers + `validate()` — `n{NN}left`/`n{NN}right` rules → `PropRule::rule_key()` byte/side split. `mm2-inspect proprules <install> [--city] [--strict]`: expected = `city/<stock>/{propdefs,proprules,props}.csv`, denominator = every discovered `city/**` prop-rule CSV incl `.csv.txt` exports + `geometry/props.csv` (different LOD schema). Cross-checks: rule refs → sibling defs, def file refs + group names → `geometry/<n>.pkg`, LOD rows → `geometry/<name>`, PSDL `prop_rule` bytes → defined rule numbers. Retail: 16/16 parse; byte↔rule link verified (WLD-14); 48 issues — 41 phys `*_m` + 3 phys group dead refs, `sp_bollard_pedsafe_l`, `va_garbagetruck.pkg`, rule 205 ×2; `--strict` exits 2. docs/research/proprules.md + WLD-14/UNK-21. Runtime stamping unwired — perimeter-walk semantics unverified (UNK-21). Candidate pending external check. |
| F03-B | implemented | F03-A | ~2000/3763 INST props instantiate with collision (README; `city.rs`) plus `props.pathset` ambient rows stamped through the shared `PropCache` — london 1188, sf 925 instances, decal paths classified. Stamping micro-semantics inferred (UNK-20); audio pathsets unconsumed. First candidate failed review on an unbounded line-strip expansion (huge/non-finite authored coordinates could stall `t += spacing` and OOM the load); repaired with the 8192/file stamp budget, arithmetic per-segment counting, non-finite skipping, load-time `validate()` and `pathset_props_capped`/`pathset_issues` report fields plus regression tests. Externally checked at `f6e151f`. |
| F03-B.2 | checked | F03-B | Event `.pathset` overlays (F03-AC04 leg): `event_race_setup` returns `EventSetup { definition, pathsets }` — the `<stem>.pathset` records the catalog attributes to the resolved event; `load_session_world` calls `city::spawn_event_pathsets` which runs the shared `stamp_pathset` classifier (`prop`/`PATHnn` label/`giz_*` animated/decal/`unresolved` + per-file 8192 budget + `validate()` issues) through a fresh `PropCache` into `event-pathset-*` session-owned entities. Teardown removes exactly the overlay; re-entry restamps once (AC04 — restart test asserts identical gen-2 count, no gen-1 survivors). Parse/read failures land in `EventPathsetReport::failed_files`, warned, non-fatal (optional record). Retail: london `circuit0` 181 props, `race6` 256, sf `circuit0` 85 — all `sp_*` names. `<object>_<event>` overrides (`london_bridge_circuit0`…) stay extras — all `giz_*`/`PATHnn`/`sp_pcar*` on retail, need animated-object/parked-car features. Tests: +3 in `tests/event.rs`. Externally checked at `afff57d`. |
| F03-B.3 | implemented | F03-B | Prop-rule stamping channel (WLD-17/UNK-21). `mm2_game::props::walk_prop_rules` — pure geometry walk: `RoomPath::road_rooms` chains, `start/end_crossroads` curb-pair junction runs, interior room-to-road boundaries found via the widest neighbour-marked perimeter pair, the two arcs between crossings walked as sidewalk building lines with side labels measured against travel direction (left/right of *travel*, not winding). Per-side `start`/`distance`/`maxUse` budgets and `(minLerp+maxLerp)/2` curb→outer placement are inferred policies; `file1–4` pick is a deterministic hash; output bounded at `MAX_PROP_RULE_STAMPS`, issues + stats counted. `load_city` spawns `PropStamp`s through the shared `PropCache` — bound names become dormant bangers via `spawn_banger_prop` (same path as pathset stamps), the rest `spawn_prop`; `CityReport` gains `proprule_*` counters + Display fields. Retail: sf 345 rooms/5002 stamps/5002 bangers, london 410/5083/5083 — 0 unresolved, 0 no_crossing; 109 bad `road_rooms` refs (65 0xx encoded values) + 52 sf / 5 london unreached rule rooms counted honestly. Screenshots: lamps line both sidewalks at authored ~29 m stagger, london phone booths/trees/bollards at curb edges, multi-room freeway parapet lamps — visually consistent. Unit tests (4, synthetic PSDL) + app test through `load_city`. TEX fix included: `decode_tex` clamps declared mip count to the size-supported max (`p_parkmeter_f.tex` 7 mips on 32×32 → hard wgpu error, now warned+clamped). UNK-21 narrowed to field semantics; geometry promoted to WLD-17. Candidate pending external check. |
| F03-B.4 | implemented | F03-B.3 | Decal pathset channel — the last ambient placement source. `mm2_app::decals::stamp_decals` consumes `<dir>/<stem>/decals.pathset`: measured geometry (WLD-18 — `LineStrip` points interleave two authored ribbon edges, even/odd pair = cross-section, consecutive sections join into quads; authored widths ~1 m zigzag → 20 m junction paint), `u` across the pair / `v` along the centre line tiled per `spacing` (inferred axis policy — texture contents agree: zigzag oscillates along V, rxwalk bars along U), palette alpha honoured via new `decode_rgba_honoring_alpha` (P8 decals carry authored translucency; ordinary `decode_rgba` unchanged), alpha-bearing textures `AlphaMode::Blend`, 2 cm lift + −1 depth bias, lit double-sided render-only entities merged per texture stem — no colliders. Every path classified: ribbons/labels/`giz_*` animated/PKG-props/unresolved/empty/degenerate/odd-tail/skipped quads/capped/missing textures/issues all counted in `CityReport.decals`. Retail: london 79 ribbons/85 quads/3 entities (zigzag, box junction, zebra), sf 48/223/2 (rails, skid marks) — 0 unresolved, 0 missing textures; screenshots show rail channels, zebra stripes, junction wash. `props.pathset` keeps classifying its 31 stale `r4i_rails_f` copies (26 byte-identical) without double-stamping. `MaterialCache::get_decal` shares the existing texture→material cache. Candidate pending external check. |
| F03-B.5 | implemented | F03-B.3, F04-B.1 | Placement-height repair for the operator report — all three channels. Root cause measured on retail `dgBangerData`+PKG pairs (temporary `probe_height` probe, since removed): prop meshes are authored centred at the bound centre; `Size` is the bound's *full* extents (`CG ± Size/2`, `CG.y = Size.y/2` on every measured record — cone 0.425/0.85, sawhorse 0.727/1.453, streetlamp 3.862/7.702, tptpole 6.151/12.309); the authored stamp point is where the bound's *base* rests. `city.rs` stamped the mesh centre at the point → props sank ~half their height. `PropOffset` contract: `Bound(+CG)` for stamped bound names, `Ground(−min_y, lift-only)` for unbound, `Verbatim` for INST — baked into render verts, collision accumulation and BREAK fragment pieces; `PropCache` keyed by name+offset class (same pkg can reach all three channels). `mm2_game`/`mm2_formats` docs corrected (`Size` full extents, `CG` bound centre); `angular_kick`/`strike` reach use half-extents of `Size` (kick lever now measured from the bound centre = CoM, not the origin); existing tests' settle windows widened for the physically-correct stronger kick. Regression test `tests/banger.rs::stamped_props_rest_their_bounds_on_the_path_point` asserts Avian `ColliderAabb` on all three channels through `load_city`. Retail: london headless counts unchanged (1997 inst / 1188 pathset / 5083 proprule, 0 unresolved, 0 decode failures); screenshots `/tmp/props-fixed-{london-trees,sf-lamps}.png` vs `screenshots/pathset-*.png` — trees rooted, lamps full height, benches/bollards on the surface, SF hill trees no longer stumps. Operator-visible defect corrected; F04-C.2/-C.3 retail evidence must be re-taken on corrected geometry before F04 rolls up. Candidate pending external check. |
| F03-C | queued | F03-B | Owed: sampled original locations, all source records, race cleanup, mod replacement end-to-end. |
|| F04-A | implemented | F01-B, F03-B | Split: A.1 (banger parser + record↔geometry audit — externally checked at `ad471b0`), A.2 (placement→banger binding audit + R4 runtime-model research — externally checked) and A.3 (dormant→active→settled runtime slice — implemented below). Threshold semantics and the Timer despawn remain open (UNK-22); the prop-rule channel landed in F03-B.3 — the parent stays non-checked until the remaining legs land and AC01–AC06 each see direct evidence. |
| F04-A.1 | implemented | F01-B, F03-B | `mm2_formats::banger`: typed `BangerData`/`BirthRule` decoder on the shared `tune` grammar — Size/CG/Mass/Elasticity/Friction/ImpulseLimit2/NumParts + ids + `asBirthRule` variant (warning); `BangerIssue` validation (non-finite/negative physicals, glow-count, missing birth rule); `stem_role` (fallback/fragment/named). `mm2-inspect banger <install> [--strict]`: expected = `default.dgbangerdata`, denominator = every discovered file incl `.#*.1.2` backups; stem→geometry resolved via VFS (own pkg / `.mtx` / `BREAK<NN>` chunk in base pkg / longest-base part chunk), standalone `NumParts` ↔ BREAK-index counts. Retail: 999/999 parse, 216 standalone/477 part/254 fragment/47 dead refs, 54 issues, `--strict` exits 2; `scan` recognizes `.dgbangerdata`. docs/research/banger.md + WLD-15/UNK-22. Runtime unwired by design. Candidate pending external check. |
| F04-A.2 | implemented | F04-A.1 | `mm2-inspect banger-bind <install> [--city] [--strict]` + `mm2_formats::banger::geometry_owner` (record → owning PKG stem). Audits every stamped-prop source through the VFS: INST files, `city/`/`race/` `*.pathset` (incl overlays, backups, dev cities — no filtering), `propdefs/proprules/props.csv`, PSDL `prop_rule` reachability. Binding rule `N` → `tune/banger/<N>.dgbangerdata`. Retail: 129 source files, 3 unsupported (truncated blitz10/11 pathsets), 0 failures, 51 issues (dead placement refs kept in denominator). INST = static channel (london 0/221, sf 0/165 bound); `*_ai.inst` = bound `sp_stop_f` supplements; pathset/prop-rule names ~all bound. Reverse: 269/994 placed-reachable, 562 vehicle-owned, 163 never-placed. docs/research/banger.md binding + R4 runtime model; WLD-16 added, UNK-22 narrowed. Runtime state machine still unwired. Candidate pending external check. |
|| F04-A.3 | implemented | F04-A.2 | `mm2_game::banger` (`BangerPhase` dormant/active/settled, `Banger`, `BangerDefinition` distilled from `BangerData`, `BangerStateChanged` message, `BangerPool` ×32 — the R4-recovered pool size) + `mm2_app::banger` (`BangerDefs` VFS cache, `banger_bundle`, `activate_bangers`, `settle_bangers`). `stamp_pathset` binds each prop name: bound+collidable → one session-owned dynamic-capable entity (mesh children follow); unbound/failed/no-collider → the ordinary static pair (decode failures counted, not hidden). Activation reads raw `CollisionStart` via shared `deepest_contact`; provisional estimate `approach_speed × striker_mass` vs `ImpulseLimit2` (UNK-22); one impulse + `Size`-derived spin kick; oldest-first pool reclaim; Avian sleep → `Settled` static; authority-gated (`Predicted` never transitions). 7 integration tests on real physics + 5 `mm2_game` unit tests; retail headless london `bng=1188d/0a/0s`, sf `bng=925d/0a/0s`, 0 decode failures. DSN-10 added. Deferred: fragments (F04-B), BirthRule/audio/flash/decal effects, prop-rule channel, Timer despawn, replication. AC01–AC06 stay open. Candidate pending external check. |
| F04-B | implemented | F04-A | Split: B.1 (BREAK<NN> fragment spawning on activation — implemented below). Fragment timing vs a later break threshold, `NumParts` runtime role, `BirthRule`/audio/flash/decal effects and original-content activation remain open (UNK-22); parent stays non-checked until F04-C + AC evidence land. |
| F04-B.1 | implemented | F04-A.3 | `BangerPhase::Broken`; `pkg_to_parts` splits `BREAK<NN>` chunks into `FragmentModel`s (authored file order); `stamp_pathset` stamps `BangerPieces` (each piece resolves `tune/banger/<name>_break<NN>`, parent-def fallback). `break_banger` on a qualifying edge: parent → `Broken` (collider + mesh children removed, one `BangerStateChanged`, never also `Active`), one dynamic fragment body per collidable piece — own `ObjectId`, session-owned, convex collider, impact velocity + CG-lever spin, pool-bounded via `claim_slot` (pending same-tick spawns counted; `max_active = 0` spawns nothing). Pieces without colliders → ordinary activation. Fragments spawn `Active` and settle through `settle_bangers`. 4 new integration tests (shatter/pool-bound/no-collider-fallback/stamp pieces) + teardown coverage; retail sf `bng=925d/0a/0s/0b` pieces=3092, london 1188/2710, overlay barricades 0 pieces (authored). DSN-10 + UNK-22 updated. F04-A.3 review nits fixed (INST-doc wording, zero-cap). Deferred: `BirthRule` particles, audio/flash/decal effects, prop-rule channel, Timer despawn, replication, original-content activation. AC01–AC06 stay open. Candidate pending external check. |
| F04-C | implemented | F04-B | Split: C.1 (original-content strike evidence + dev spawn/tooling), C.2 (flat-ground fragment settle + repeated-collision/pool-reclaim retail runs) and C.3 (the hull-clearance fidelity question — implemented below). Break timing vs a separate original break threshold, `NumParts` runtime role and natural-pool (>32 simultaneous) reclaim stay open; parent stays non-checked. |
| F04-C.1 | implemented | F04-B.1 | `DevOverrides::spawn` + `--spawn x,y,z[,yaw]` (quarantined dev pose applied after world/event spawn selection); `bng_ev=<a>a/<s>s/<b>b` emitted-transition counters in the headless smoke record (distinct from `bng=` end-state buckets; fragment spawns are silent by contract). New tests: `dev_spawn_override_pins_the_player_pose`; `restart_restores_stamped_placements_after_a_break` — synthetic city, real `load_session_world` + `drive_session` restart, husk+fragments removed, dormant restamp under gen-2, zero generation leaks. Retail evidence (iteration-33 correction — first recorded London command was irreproducible, retracted): `vpbug --spawn 0.4,5.5,-720,0` and `--spawn 112.3,5.5,-745,0` → deterministic `sp_bollard_black_l` activation+settle `bng_ev=1a/1s/0b` on flat London road rows (verified ×2, bit-identical); SF `sp_wrongwayfw` break into its authored 3 fragments (`bng_ev=0a/0s/1b`, `bng=924d/3a/0s/1b`); below-threshold graze + sub-threshold cone block → no transition (AC01 both sides, real content); SF `sp_cone_f` activates (`bng_ev=1a`) but does not settle on slopes; `circuit:7` overlay stamps 899 bangers incl. `sp_sawhrslt_f` walls. Gaps: fragments/knocked props stay `Active` on slopes (slope-tumbling suspected, unproven); hull underside clearance means tall vehicles cannot contact <~1 m props (raycast wheels) — open fidelity question; no GPU break capture; UNK-22 semantics unverified. Candidate pending external check. |
| F04-C.2 | implemented | F04-C.1 | `DevOverrides::banger_pool` + `--banger-pool <n>` (quarantined dev bound applied session-scoped in `load_session_world`, re-stamped on restart); smoke record gains `bng_pool=<n>` when overridden and `bng_rec=<n>` counting `Reclaimed`-cause settles (default records bit-identical). New test `dev_banger_pool_override_bounds_the_active_pool` (override lands, unconfigured session keeps ×32). Retail evidence (install `fnv1a64:e91e6cd4b2ae30d9`): flat-ground break+settle — `vpddbus --spawn=-170,1.5,-565,30` on the SF `sp_barricadewood_f` row (limit 51888, NumParts 4, y≈0 lot) → `bng_ev=0a/1s/1b` at 2400 ticks, fragments sleep 1→2→3 of 4 by 10000 ticks; repeated collisions — `impacts=104/105` at 5000/10000 ticks of pen battering, all finite, no extra transitions; pool cap — same spawn `--banger-pool 2`/`1` → `2a`/`1a` (authored-4 fragments capped, rest skipped); repeated activations + reclaim — `vpbug --spawn=802,6,-905,180` through the London `sp_bollard_stone_l` plaza ring → `bng_ev=3a/0s/0b` default, `bng_rec=1` at pool 2, `bng_rec=2` at pool 1 (bit-identical re-run). Gaps: natural-pool (>32 simultaneous) reclaim not staged — no surveyed retail site yields that density; slope-settle question answered (flat sleeps, slopes tumble); `vpddbus` clears `sp_cone_f` without contact (hull question unchanged); UNK-22 unverified. Candidate pending external check. |
| F04-C.3 | implemented | F04-C.2 | Hull-clearance fidelity resolved via bound-vs-bound evidence (mm2hook: `dgBangerData.Bound`/`ColliderId`; every car carries a `phBound`). `VehicleConfig.striker_points` = the unmodified authored bound (validated like `collider_points`, inherited through `assemble` overrides); `vehicle_bundle` attaches it as `StrikeBound` — a component for shape-overlap queries only, never a world collider (fallback: the chassis collider itself). `activate_bangers` gains a second strike source: each moving `StrikeBound` runs `SpatialQuery::shape_intersections` against dormant bangers; severity = bound surface velocity at the prop centre, estimate/gate = the same `impulse_estimate` vs `ImpulseLimit2`, lever = the prop's upwind face by `Size`, deduped against same-tick contact edges. World contact still meets only the snag-safe hull — no global lowering, per-vehicle bounds kept verbatim. 4 new integration tests (overlap strikes what the hull clears, parked overlap inert, below-limit dormant, bundle surface). Retail (install `fnv1a64:e91e6cd4b2ae30d9`, `--bot`): `vpddbus --spawn=-1641.6,36.7,410,0` → `bng_ev=1a/0s/0b` over the `sp_cone_f` cluster it previously ghosted; `vpbus --spawn=0.4,5.5,-720,0` → `bng_ev=1a/1s/0b` through the `sp_bollard_black_l` row; `vpbug` same run → `2a/2s` (was `1a/1s` — bound catches a second bollard). Classified implementation choice on an original bound-vs-bound requirement; overlap supplies speed/lever, no manifold — impulse equivalence to a contact is provisional (UNK-22). Whether wheels/bumpers struck bound-clearing props in the original stays unknown. Candidate pending external check. |
| F05-A | queued | F01-B, F02-B | `.vehcardamage` readable via generic tune parser; no typed rules or runtime. |
| F05-B | queued | F05-A | — |
| F05-C | queued | F05-B | — |
| F06-A | queued | F00-B, F01-B | PSDL room attributes parsed; no typed surface/material representation or traction hookup. |
| F06-B | queued | F06-A | — |
| F06-C | queued | F06-B | — |
| F07-A | queued | F01-B, F02-B, F06-A | No audio decoders/voices; `aud/` family = 3293 files incl. cardata/dmusic/spchdata. bevy built without `bevy_audio`. |
| F07-B | queued | F07-A | — |
| F07-C | queued | F07-B | — |
| F08-A | queued | F01-B, F07-A | — |
| F08-B | queued | F08-A | — |
| F08-C | queued | F08-B | — |
| F09-A | implemented | F00-B, F01-A | Split into A.1 (BAI parser + audit — externally checked) and A.2 (`.aimap` parser + audit — implemented below). Parse-level work done; route queries/semantics continue under F09-B. |
| F09-A.1 | implemented | F00-B, F01-A | `mm2_formats::bai`: `CAI1` parser — roads (per-side lane/sidewalk/rail curves, rooms, half-width, base speed, flags, per-section frames), intersections (room, center, counterclockwise road refs), per-room large/small culling lists. Measured layout correction vs the R3 doc: the `[lanes+sidewalks][sections]` distance matrix precedes the per-curve edge distances (`docs/research/bai.md`). `Bai::validate()` reports `BaiIssue` diagnostics: duplicate ids, unknown flag/ambient/rule codes, dangling/mismatched end↔intersection back-refs, room-0 refs, dangling culling refs, <2 sections. `mm2-inspect bai <install> [--city] [--strict]`: expected = `city/{london,sf}.bai`, every other `city/*.bai` audited as an extra, room refs cross-checked against the same-stem PSDL. Retail: both expected files parse byte-exact, validate clean, rooms in range; `_bak` copies clean; `sfai.bai` parses with 1 authored anomaly (intersection room ref 0 — reported); `_sup` files share CAI1 magic but don't fit the layout → `unsupported` (not hidden). `--strict` exits 2 (2 issues, all on the sfai dev-map extra). `.bai` added to `scan` recognized formats — `_sup` files now appear as honest parse failures there. Candidate pending external check. |
| F09-A.2 | implemented | F00-B, F01-A | `mm2_formats::aimap`: INI-like parser measured on all 209 retail files — `#` comments, `[Section]` headers, scalar vs counted-list bodies (exact-count on retail), free-form `[Traffic Lights]`, unknown sections preserved verbatim in `unknown_sections`. Typed records keep undocumented numeric tails raw (`PoliceRecord.params`, `OpponentRecord.params` — two retail shapes each); malformed rows → `diagnostics` + skip. `Aimap::validate()` reports `AimapIssue`: duplicate exception roads, ambient-weight range/monotonicity/1.0-closure, non-0/1 flags, negative scalars, uninterpreted sections. `mm2-inspect aimap <install> [--city] [--strict]`: expected = `city/<stock>.aimap` + every discovered `race/<stock>/*.aimap{,_p}` (209/209 resolve+parse), extras audited as unsupported-on-failure; cross-checks exception road ids vs same-city `city/<city>.bai` and opponent `.opp` refs via VFS. Retail: 9 issues — 8 london files with exception ids 562–815 beyond the 540-road BAI (UNK-18/WLD-8, reported not repaired), stunt0's `opp-c0.2` ref dead. `--strict` exits 2. `.aimap`/`.aimap_p` added to `scan` recognized formats. docs/research/aimap.md + ledger WLD-6/7/8, UNK-12/18. Candidate pending external check. |
| F09-B | queued | F09-A | Split into B.1 (nav graph + audit — implemented below) and B.2 (debug overlays + deeper retail route validation). Parent AC04 stays open until B.2. |
| F09-B.1 | implemented | F09-A | `mm2_game::nav::NavGraph::build(&Bai)`: directed arcs (right-side curves travel with sections, left-side against — London left-hand is authored data per the Adzima article, no handedness flag), vehicle/sidewalk/tram/train lane records ranked by measured lateral offset (`edgeDistances` is not an ordering — UNK-19), end→intersection resolution with dead-end degradation + `NavIssue` reporting, turn connections minus U-turns, union-find components, XZ grid. Queries: `sample_lane`, 3D `nearest_lane` (elevation-aware; `rooms` hint for stacked geometry), `legal_exits` (documented lane-position rules; geometric turn class + authored `ccw_delta`), seeded `choose_exit`, bounded A* `route` (`closed_roads` hook for aimap `[Exceptions]`; specific `RouteError`s), per-consumer `RouteCursor`s. `mm2_content::nav::load_nav_graph` (VFS→`Bai`→graph). `mm2-inspect nav <install> [--city] [--strict] [--route from:to]`. Retail: London 540 roads→606 arcs/1141 vehicle+1080 sidewalk+28 rail lanes/328 ints/0 dead ends/1 component; SF 379→618/1212+758+42/214/1/1; 176 non-routable roads reported; routes resolve (sf 13→50 = 4 steps/576 m). AC01/AC03/AC05/AC06 satisfied at synthetic level (20 tests); AC04 open → B.2. Candidate pending external check. |
| F09-B.2 | implemented | F09-B.1 | `NavOverrides` distils a parsed aimap (zero-density `[Exceptions]` → `closed_roads`, `[Speed Limit]` → per-road/file-default speed lookup) with `route_options()` feeding the existing `closed_roads` hook. `NavGraph::route_roads` road-index probe (anchors on the first arc's first lane midpoint — a centreline anchor sits equidistant between directions and can snap the dead-end-facing lane). `mm2_content::load_nav_overrides` (absent → `None`, malformed → error). `mm2_app::nav_overlay`: session-scoped `CityNav` (graph + issues + overrides + probe), `overlay_lines` pure segment builder (8 classes: fwd/bwd lanes, sidewalk, rail, direction chevron, closed, route, intersection), `draw_nav_overlay` gizmos, `hud_summary`; `--nav`/`--nav-route` CLI; HUD + `smoke=` `nav=` fields; resource removed on session teardown. `mm2-inspect nav --turns` + aimap-applied `--route`. Retail: probes reproduce baseline exactly; 4-way Δccw↔geometry agreement 95.7% London/99.2% SF (`docs/research/bai.md`). Rendered captures on both cities (local, not committed). Tests: +2 game, +5 content, +7 app. AC04 candidate pending external check. |
| F09-C | implemented | F09-B | Split: C.1 (directed route-constraint validation — implemented below). AC01/AC03/AC05/AC06 synthetic since B.1 (+2 new tests), AC02 via the bai/aimap audits, AC04 via B.2 rendered captures; the remaining AC evidence levels are per-slice. |
| F09-C.1 | implemented | F09-B | `NavGraph::reachable_arcs(start, closed_roads)` — bounded BFS over authored `exits` (start included even when closed; closed roads never entered through a turn, matching `route` semantics). `mm2-inspect nav`: `--routes <n>` runs a per-arc directed-reachability census (full-reach count, reach-1 arcs annotated dead-end vs no-legal-continuation, unreachable ordered pairs, smallest sources) plus `n` seeded `route_roads` probes — endpoints must land on the asked roads, consecutive steps must share a turn, interior arcs must not sit on a closed road; failures print named pairs, expansion-limit/violations count toward `--strict`. `--aimap <logical>` substitutes any aimap's overrides (must resolve → exit 2). Retail (`fnv1a64:e91e6cd4b2ae30d9`): london strongly connected (606/606 arcs, 512/512 probes ok); SF 4927 unreachable ordered pairs (~1.3%) — authored one-way traps, not invented links (0 violations; roads 92/94/97/99 reach 1, 102/111 reach 2, road 0- is the dead end). `race/london/blitz0.aimap` (8 closed) → 15016 unreachable, 24/512 probes unreachable (`270→108`: 19 steps open → `Unreachable` closed); `race/sf/blitz0.aimap` (3) → 7977, 11/512. WLD-11 extended, `docs/research/bai.md` updated. Tests +2 in `mm2_game/tests/nav.rs` (24 total). Candidate pending external check. |
| F10-A | queued | F01-B, F02-A, F09-B | `va*` traffic vehicles exist in install; no ambient-traffic code. |
| F10-B | queued | F10-A | — |
| F10-C | queued | F10-B | — |
| F11-A | checked | F00-B, F01-A | `mm2_formats::racefiles` shared classifier (was private to `mm2-inspect` inventory); new parsers `waypoints` (waypoint + `_strtpnts` CSVs), `opp`, `crashdata` (tolerates retail `AmbDenisty` typo / omitted `Filename` label / named tail columns — kept as diagnostics), `rewards`. `mm2_content::EventCatalog`: VFS scan per city, `mm*data.csv` rows → `EventRef`-keyed entries (ready/incomplete + failed refs), dep records attached by stem, Crash Course `Filename` links resolve whole linked stems, rewards + milestone rewards linked, extras listed. `mm2-inspect events <install> [--city] [--strict]` — strict exits 0 on retail: 45/45 events ready per city. Producer correctly placed in `mm2_content` after the iteration-010 review rejection. Externally checked (review pass at `bb56272`, iteration 011 feedback). |
| F11-B | active | F11-A | Split into B.1 (shared runtime contract + driver — checked) and B.2 (`mm2_content` catalog→`RaceDefinition` producer + event-session loading wiring). Parent AC05 (event-prop/traffic-override session scope) stays open — no traffic/prop-override systems exist to scope yet. |
| F11-B.1 | implemented | F11-A | `mm2_game::race`: `Checkpoint` swept cylinder test (XZ radius + ±height band + opt-in direction flag), `CheckpointRule` (`AnyOrder` documented BLZ-1/CHK-1 / `Ordered` documented CIR-1 — carried on the definition, not imposed), `RaceDefinition` (checkpoints/finish/start slots/laps/countdown + `validate`), `RaceState` (generation-stamped countdown→running→complete, `input_locked`, `is_stale`), `RaceProgress` (per-checkpoint cleared flags, ordered `next`/lap wrap, `break_segment` for teleport/reset, one segment consumes every checkpoint it crosses), `RaceStarted` message, `SessionOutcome::Finished{race_ticks}` on `SessionResult`. `mm2_app::race::advance_race` in `FixedLast` (post-solver `Position` segments): countdown→one `RaceStarted`+`Countdown→Playing`, clock+advance while `Playing`, `Finished` → mint+record `SessionResult` once into `ResultLedger`, all-finished → `Complete`; authority-gated (Remote never steps). Teardown: `drive_session` removes `RaceState`; `vehicle_input` honours `input_locked` (stale-gated); `Countdown` quittable/restartable. First candidate failed review on a real defect — `break_segment` had no production caller, so an R-key `ResetVehicle` teleport swept (and could finish) every checkpoint between the two poses; repaired with `mm2_vehicle::Teleported` stamped by `vehicle_reset` atomically with the `Position` write (chosen over draining `ResetVehicle` in the race system, which has message-lifetime and Update-ordering holes) plus `reanchor_teleported_participants` chained before `advance_race`; regression tests `vehicle_reset_breaks_the_swept_segment` + `reset_while_paused_cannot_sweep_checkpoints` fail on the old wiring. Tests: 11 contract + 15 production-path (AC02 high-speed/wrong-height/repeated/teleport + reset-via-message + paused-reset, AC03 countdown-once/pause-freeze/restart-removes-timer, AC04 once-only results with provenance, ties, remote-authority no-op, quit during countdown). Candidate pending external re-check. |
| F11-B.2 | implemented | F11-B.1 | `RecordContent` retains parsed payloads (`Waypoints`/`StartPoints`/`Opp`/`CrashData`). `mm2_content::race_def::race_definition` builds a `RaceDefinition` from a resolved `CatalogEvent`: Blitz/Checkpoint → `AnyOrder` (row 0 = start line, last row = finish trigger), Circuit → `Ordered` (rows 1.. + lifted line copy closes the lap, authored `NumLaps`); authored `w` radii; authored `_strtpnts` grids or a derived tangent start; Crash Course rejected explicitly. Catalog attributes SF's `cir<N>_strtpnts` siblings to `circuit<N>` (inferred alias, WPT-3). `load_session_world` event mode: catalog resolve → `Ready → Countdown`, spawn on the authored/derived slot, `RaceState`/`RaceProgress`/`ResultLedger` inserted (the ledger was unregistered — latent panic once any race ran), session-owned orange/green checkpoint/finish markers + `update_checkpoint_markers` (cleared gates hidden, finish revealed after all gates). `--event <table>:<index>` CLI; `headless_smoke` rewired onto the real session systems so `--event --headless` is production-path evidence. Retail: London `Blitz[0]` 3/3 gates swept (`race=Running` — finish not crossed in-window), SF `Circuit[1]` authored-grid spawn, `checkpoint:12` → clean `Failed` exit 3; screenshot shows gate column + `GET READY` countdown. 7 producer + 7 app event tests incl. a full synthetic course finish → one ledger result. AC05 event-prop/traffic scoping deferred (no such systems exist to scope yet). Candidate pending external check. |
| F11-C | queued | F11-B | — |
| F12-A | checked | F02-B, F11-B | Authored Blitz `TimeLimit` binds per difficulty → 120 Hz ticks (seconds, provisional UNK-4); distilled `EventParams` (conditions/densities/actor counts) validated at build — out-of-range authored values are explicit `BadParam` errors. `advance_race` enforces an inclusive deadline (finish on the expiry tick counts, DSN-7), mints exactly one `TimedOut` result per unresolved participant into the retained ledger; `SessionOutcome::TimedOut` added. HUD shows `m:ss` remaining / `OUT OF TIME`; smoke records `tl=`/`outcome=`. Retail: london blitz:0 `cp=3/3 → timed-out` (bot cleared all gates, never crossed the finish — RACE-7 holds), sf blitz:0 `tl=23.0s` running. Externally checked (review pass, iteration 015 feedback) — AC03 candidate-level; AC01/02/04–06 partially open per review gaps (full-roster matrix, objective/nav HUD, warning cues → F12-B/C). |
| F12-B | implemented | F12-A | Split: B.1 = navigation arrow (checked), B.2 = low-time warning cue (implemented below). In-scope legs done: timer/objectives/finish/failure + HUD/navigation + warning cue. Still open on the parent: results/fail screens beyond HUD text (F17/UI-5 scope), audio cue (needs F07), AC01/AC06 catalog evidence (F12-C). |
| F12-B.1 | checked | F12-A | RACE-6 navigation arrow. `mm2_game::race`: `navigation_target` (nearest un-cleared gate XZ, explicit `picked` wins until cleared then falls back, armed `Finish` once all gates clear, `Ordered` → `None` per HUD-2), `cycle_target` (authored-order walk, wraps, skips cleared, empty → clears pick), `relative_bearing` (signed driver-frame angle, `+` = right, ground-plane only), `TargetSelection` component, `RaceProgress::remaining`. `mm2_app::race`: session-owned `NavArrow`/`NavArrowPart` UI needle + diamond tip (node-drawn — embedded font is ASCII-only, DSN-8), `spawn_nav_arrow`, `nav_target_input` (X/Z cycle — original X/S blocked by WASD brake, DSN-8), `update_nav_arrow` (rotation = bearing, green ahead / yellow behind, hidden without a live target). Tests: +6 contract, +5 production-path (bearing/color/visibility through `Position` writes, X/Z cycling incl. edge-trigger + Complete gate, finish arming, teardown despawn). Retail: london `blitz:0` headless `status=pass` (tl ticking); windowed captures at frames 100 (countdown) + 700 (driving) show the needle green ahead tracking the gate. Yellow-behind leg covered by tests, not rendered. Externally checked (review pass at `56e764c`, iteration 016 feedback). |
| F12-B.2 | implemented | F12-B.1 | Low-time warning cue — designed policy (DSN-9): no documented original rule (HUD-2 lists only the countdown timer). `mm2_app::race`: `LOW_TIME_TICKS` (10 s, inclusive threshold), `LOW_TIME_FLASH_TICKS` (0.5 s half-period), `LOW_TIME_BRIGHT`/`LOW_TIME_DIM`, session-owned `LowTimeWarning` `LOW TIME` UI banner + `spawn_race_warning`, `update_race_warning` — armed while a timed race runs and the local participant is unresolved, pulsing bright/dim on the remaining ticks themselves (same race clock as the deadline → AC04; freezes with pause), hidden for stale/complete/countdown/untimed races and a resolved local participant (`PlayerControl::Local` filtered — avoids the latent multi-participant wrinkle the B.1 review flagged on the arrow). Tests: +4 production-path (threshold boundary + pulse cadence + pause freeze + timeout hide; countdown gate for sub-threshold limits; resolved-local hides while a remote races; untimed never warns + teardown despawn). Retail: london `blitz:0` headless `status=pass` incl. `--frames 1800` → `race=Complete outcome=timed-out`; windowed capture at `time 1.5s` shows the armed banner (dim half-pulse), a `time 16.1s` frame shows it correctly hidden. Candidate pending external check. |
| F12-C | implemented | F12-B | Structural leg (checked at `f62a977`): `RaceDefReport` + `mm2-inspect race-defs`, retail 90/90 rows build at both difficulties, 0 failed; headless Blitz matrix 20/20; invalid refs fail explicitly; DSN-6 spawn repair (`f8bd917`). Scripted-completion leg (this iteration, candidate): `mm2_app::scripted` — the `--bot` evidence driver. `ScriptedDrive` resource flag + session-owned `ScriptedBot` component state; `drive_target` follows the live objective (earliest un-cleared gate in authored order for `AnyOrder` — the waypoint rows are the only route data blitz/checkpoint events ship — `progress.next` for `Ordered`, armed finish last); `scripted_input` proportional steer + throttle bands + corner brake + speed cap + two-phase stuck escape (reverse-and-turn, alternating side); `scripted_drive` owns `VehicleInput` after the keyboard mapping only while `--bot` inserts the resource, honors the countdown lock and yields on a resolved participant. Wired headless (`smoke::Driver::{Hold,Scripted}`, `driver=` record field) and windowed (`.after(input::vehicle_input)`, gated `resource_exists`). Tests +6 (`tests/bot.rs`): control law, stuck escape, live-target selection, countdown lock, L-turn AnyOrder finish → 1 `Finished` result, 2-lap Ordered circuit finish incl. lap wraps — all through `load_session_world`→`advance_race`. Retail bot matrix 64/64 events both cities (see baseline table): 4 finished, all 20 Blitz `race=Complete`, 41 `Running` at frame cap, 8 SF falls — bot-limited evidence, not a playability claim; most retail courses need route-aware driving (F15). Still open on the parent: reward-fact leg (AC05 — no reward emission exists to exercise, F16 scope). Candidate pending external check. |
| F13-A | active | F02-B, F11-B | Split into A.1 (results flow — implemented below). London race0–13, SF race0–11 (+r0) authored data present; `.aimap`/`.aimap_p` measured as Amateur/Professional rosters (RACE-11). Remaining: unlock/progression ledger leg (CHK-2/CHK-3 — F16 scope), recovery-penalty leg (RACE-5/DMG-2 destruction — F05 scope), environmental-override leg (tod/weather binding — F18 scope), and AC06's representative-playability + honest coverage matrix. |
| F13-A.1 | implemented | F02-B, F11-B | Results flow (UI-5, DSN-11): `advance_race` transitions `Playing → Results` on the same step it records a *local* participant's terminal resolution (`Finished`/`TimedOut`) — the race clock, progress and ledger freeze with the phase; a remote/AI participant resolving while the local driver still races ends nothing (per-participant progress stays independent). `update_hud` shows the outcome + recorded finish time during `Results`. Tests +4 in `tests/race.rs` (31 total): finish→Results + once-only ledger, non-local resolution keeps `Playing` then local finish resolves, timeout→Results, restart-from-Results rebegins gen-2 with no stale `RaceState`. Retail: sf `checkpoint:0 --bot` → `phase=results cp=6/6 outcome=finished`; london `blitz:0` → `phase=results outcome=timed-out`. Ledger: RACE-11 (aimap difficulty rosters, 23/24 + `sf/race0` anomaly), WPT-2 measured on all 24 waypoint files, MP-9 corrected. AC02/AC03/AC05 evidence strengthened; AC04 needs F15, AC06 needs representative playability. Candidate pending external check. |
| F13-B | queued | F13-A | — |
| F13-C | queued | F13-B, F15-B | — |
| F14-A | queued | F02-B, F11-B | London circuit0–11, SF circuit0–11 authored data present (circuit11 partial: opp/pathset only, no .aimap). SF `cir1–9` are the circuit events' start grids under a short stem — aliased to `circuit<N>` since F11-B.2 (WPT-3). |
| F14-B | queued | F14-A | — |
| F14-C | queued | F14-B, F15-B | — |
| F15-A | queued | F02-B, F09-B, F11-B | 612 `.opp` files present; no opponent AI. |
| F15-B | queued | F15-A | — |
| F15-C | queued | F15-B | — |
| F16-A | queued | F01-A, F11-A | No profile storage; `players/` dir exists in install (17 files). |
| F16-B | queued | F16-A | `locked` flags already read from `.info` (catalog); unlock rules unimplemented. |
| F16-C | queued | F12-B, F13-B, F14-B, F16-B | — |
| F17-A | queued | F01-A, F02-A, F11-A, F16-A | App boots straight into a world; no menus. |
| F17-B | queued | F17-A | — |
| F17-C | queued | F12-B, F15-B, F16-B, F17-B | — |
| F18-A | queued | F01-B, F06-B | `city/*.sky`, `*.ldef`, `*.cpvs` present unparsed; no weather/time-of-day selection. |
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
