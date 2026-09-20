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

**Next selected slice: F12-C remainder** — the structural leg landed
this iteration (`RaceDefReport` + `mm2-inspect race-defs`, retail
90/90 authored rows build at both difficulties, 0 failed) and the
headless Blitz matrix runs 20/20 — after it caught and we fixed a
real spawn defect (DSN-6 back-off could place the car off an elevated
start deck; london `blitz:6` fell to y≈−907). Still owed on F12-C:
scripted/bot-assisted *completions* exercising finish→result on
retail events (current bot only holds throttle → `race=Running`),
the Checkpoint/Circuit headless matrix, and reward-fact verification.
Independent ready alternates: F03-A (prop audit) or F09-A (BAI
parser).

## Baseline gate results (this checkout, 2026-09-20)

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --locked --workspace` | PASS — all groups, 0 failures (incl. 18 mm2_game race contract tests, 27 mm2_app production-path race tests, 15 mm2_vehicle drive tests, 13 F01-A session tests) |
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
| F03-A | queued | F00-B, F01-A | INST + PSDL placement parsed. Unmapped sources remain: `.pathset`, `.opp`, `.cpvs`, `.ldef`, race `.csv` waypoints, embedded PSDL props. |
| F03-B | implemented | F03-A | ~2000 INST props instantiate with collision (README; `city.rs`). Candidate; original-location spot validation still owed (F03-C). |
| F03-C | queued | F03-B | Owed: sampled original locations, all source records, race cleanup, mod replacement end-to-end. |
| F04-A | queued | F01-B, F03-B | No breakable-prop classification or state transitions yet. |
| F04-B | queued | F04-A | — |
| F04-C | queued | F04-B | — |
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
| F09-A | queued | F00-B, F01-A | `city/london.bai`, `city/sf.bai` (+ `_sup`/`_bak` variants, `sfai.bai`) present in install; no BAI parser in `mm2_formats`. |
| F09-B | queued | F09-A | — |
| F09-C | queued | F09-B | — |
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
| F12-C | implemented | F12-B | First slice landed (two commits): `mm2_content::RaceDefReport` — per-row, per-difficulty production-builder audit (`Built`/`Unsupported`/`Failed`, table errors kept, denominator never filtered) + `mm2-inspect race-defs [--city/--table/--strict]`; retail 90/90 rows build, crash=unsupported not failure. Headless Blitz matrix 20/20 pass; invalid refs (out-of-range/crash/bogus) fail explicitly; london blitz:6 rendered post-fix. The matrix caught a real spawn defect — DSN-6 back-off could land off an elevated deck; now spawns on the authored line (commit `f8bd917`, ledger updated). Remaining open: scripted completions → finish/result on retail, Checkpoint/Circuit matrix, reward facts. Candidate pending external check. |
| F13-A | queued | F02-B, F11-B | London race0–13, SF race0–11 (+r0) authored data present. |
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
  `--strict`), `race-defs` (`--city`, `--table`, `--strict`).
