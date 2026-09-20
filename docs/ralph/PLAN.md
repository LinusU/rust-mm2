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

**Next selected slice: F01-A** — typed session configuration and
explicit session ownership/lifecycle transitions (see task table).
F00-B's children are externally checked; F00-C (evidence commands) is
implemented as a candidate. F01-A is the next foundation slice: `mm2_game`
is still a 40-line stub with no SessionConfig/lifecycle.

## Baseline gate results (this checkout, 2026-09-20)

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --locked --workspace` | PASS — 17 test binaries/doc-test groups, ~108 tests, 0 failures |
| `mm2-inspect cars <retail>` | 29 catalog entries; all 21 `EXPECTED_STOCK_ROSTER` cars `ready`; 8 extra ids kept with explicit incompleteness reasons |
| `mm2-inspect list <retail>` | 13,389 logical paths; families: texture 3977, aud 3293, geometry 1867, tune 1356, race 1080, bound 1034, city 247, anim 95 |
| `mm2-inspect inventory <retail>` | 12 families: cities 2/5 exp/disc all parsed; vehicles 21/21 ready + 8 rejected; races 80 exp, 78 accepted, 2 partial (circuit11), 31 extras; lessons 42/42; placement 13 inst parsed; audio 7/7 families (3293 files unverified); peds 4/4 + wolf partial; MP/breakables/traffic/profile/interface discovered-only. Event-metadata tables parse: 12/10/10/13 checkpoint/blitz/circuit/crash rows per city. `--strict` exits 2 (33 findings) — honest: partial/junk records exist on retail. |
|| `mm2 --dev-world --headless` | `status=pass` (updates=600, peak 27.9 m/s, moved 157 m, 4/4 wheels) — dev world starts with no MM2 data, car settles + drives |
|| `mm2 --mm2-path <retail> --city sf --headless --frames 300` | `status=pass` — 1171 rooms / 3763 props via VFS, imported vpbug drove 30 m on real collision |
|| `mm2 --dev-world --frames 90 --screenshot` | `status=pass`, 2.9 MB PNG awaited + verified (GPU/render evidence recorded on this machine) |
|| `mm2 --headless --city bogus` (no data) | `status=unavailable`, exit 4 — missing data is not a failure |
|| `mm2 --mm2-path <retail> --city bogus --headless` / `--frames 60` | `status=fail`, exit 3 — explicit failure, logical path + reason |

## Task table

| Task | Status | Dependencies | Evidence / reason / next action |
|---|---|---|---|
| F00-A | implemented | - | Audit done in this planning pass: gates pass, env/install/toolchain recorded above. Candidate pending external check/review. |
| F00-B | implemented | F00-A | Split into F00-B.1 (inventory) and F00-B.2 (rules ledger); both children implemented. Candidate pending external check. |
| F00-B.1 | implemented | F00-A | `mm2-inspect inventory` landed: versioned report (engine commit + fnv1a64 catalog fingerprint) with expected/discovered/accepted/rejected/unverified counts across 12 families; `--strict` exits 2 with 33 findings on retail (8 incomplete vehicles, 2 partial circuit11, junk/partial records). Candidate pending external check. |
| F00-B.2 | implemented | F00-A | `docs/original-rules.md` ledger landed: ~90 classified facts from MM2HELP.HLP (decompiled locally via helpdeco), Readme.rtf, Booklet.pdf and authored data. `mm2_formats::racedata` parses the `mm*data.csv` event tables; inventory cross-checks them (12/10/10/13 rows/city, missing/malformed → rejected). Candidate pending external check. |
| F00-C | implemented | F00-B | `mm2 --headless` headless physics smoke (no window/GPU; dev world + real cities via VFS), windowed `--frames`/`--screenshot` visual smoke that awaits the capture file, unified `smoke=` records versioned by build-embedded commit, statuses pass/fail/unavailable → exits 0/3/4. `--city` without data sources is `unavailable`; a requested missing city/vehicle is an explicit failure. AC04+AC05 advanced. Candidate pending external check. |
| F01-A | queued | F00-A | `mm2_game` is a 40-line stub (`WorldMode`, `Mm2Vfs`, markers). No SessionConfig/lifecycle yet. |
| F01-B | queued | F01-A | No stable IDs, VehicleTelemetry, ImpactEvent, SurfaceState or result contracts exist. |
| F01-C | queued | F01-B | No session start/quit integration tests or plugin-attach docs yet. |
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
| F11-A | queued | F00-B, F01-A | Race data inventoried at path level (see Discoveries); no `.aimap`/`.opp`/`.pathset` parsers or event catalog. |
| F11-B | queued | F11-A | — |
| F11-C | queued | F11-B | — |
| F12-A | queued | F02-B, F11-B | London blitz0–12, SF blitz0–13 authored data present. |
| F12-B | queued | F12-A | — |
| F12-C | queued | F12-B | — |
| F13-A | queued | F02-B, F11-B | London race0–13, SF race0–11 (+r0) authored data present. |
| F13-B | queued | F13-A | — |
| F13-C | queued | F13-B, F15-B | — |
| F14-A | queued | F02-B, F11-B | London circuit0–11, SF circuit0–11 authored data present (circuit11 partial: opp/pathset only, no .aimap). SF `cir1–9` are only `_strtpnts` files, not circuit events. |
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
  `--strict`), `inventory` (`--json`, `--strict`).
