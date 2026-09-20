# Last implementation iteration

- Task ID and title: F11-B.2 — authored events load and run through the
  shared race runtime: `mm2_content::race_def` producer
  (`CatalogEvent` → `RaceDefinition`), `load_session_world` event
  wiring, checkpoint markers, `--event` CLI, real-path headless smoke.
- Starting commit and resulting commit: started at
  `cd7f007edcdb72aa3c8dc86fa117a4b272c5d6a0` (clean tree, branch
  `ralph/night`); result = this commit.
- Why this slice: highest-value ready task; F11-B.1's review left
  "no authored event has driven a `RaceDefinition`" as the gap, and
  the producer + session wiring is exactly the missing leg. AC05
  (event-prop/traffic-override session scope) was deliberately split
  out — no traffic/prop-override systems exist to scope yet, so it
  stays open on the parent rather than being faked.
- Production code changed:
  - `crates/mm2_content/src/events.rs`: `RecordContent` now retains
    parsed payloads (`Waypoints`, `StartPoints`, `Opp`, `CrashData`)
    instead of row counts, so producers never re-read the VFS. Circuit
    events also pick up sibling records under the short `cir<N>` stem
    — retail SF ships `cir1_strtpnts`…`cir9_strtpnts` for
    `circuit1`…`circuit9` (inferred alias, ledger WPT-3).
  - `crates/mm2_content/src/race_def.rs` (new): `race_definition(&CatalogEvent,
    difficulty)` → `RaceDefinition`. Blitz/Checkpoint → `AnyOrder`
    (waypoint row 0 = start line, middle rows = checkpoints, last row
    = finish trigger); Circuit → `Ordered` (rows 1.. + a lifted copy of
    the start line closes the lap, authored `NumLaps` ≥ 1). Radii come
    from the authored `w` column; start slots from `_strtpnts` or a
    derived pose 10 m behind the line facing the course tangent.
    Incomplete/unknown/Crash Course/too-short events are explicit
    errors. Row roles and the `a`/`w` conventions are inferred
    (ledger WPT-1/2/4, UNK-16); the 10 m offset and marker visuals
    are designed (DSN-6).
  - `crates/mm2_app/src/session.rs`: `SessionMode::Event` resolves the
    catalog → `race_definition` → session goes `Ready → Countdown`
    (cruise still goes straight to `Playing`), spawns the player on
    the event's start slot, inserts `RaceState` + `RaceProgress` +
    `ResultLedger`, and spawns session-owned checkpoint/finish marker
    entities. An unresolvable/unbuildable event is a `Failed` session,
    never a silent cruise.
  - `crates/mm2_app/src/race.rs`: `event_race_setup` (the content-crate
    call site), `spawn_checkpoint_markers` (translucent orange columns
    per gate, green finish column spawned hidden),
    `update_checkpoint_markers` (`Update` — hides cleared gates,
    reveals the finish once all gates clear, matching RACE-7).
  - `crates/mm2_app/src/main.rs`: `--event <table>:<index>` CLI,
    `ResultLedger` registration (it was never registered — a latent
    panic the first time any race ran), marker system in `Update`,
    HUD countdown/checkpoint/finish text.
  - `crates/mm2_app/src/smoke.rs`: `headless_smoke` now drives the real
    session systems (`load_session_world` + the production
    `FixedLast` chain) instead of a synthetic shortcut, so
    `--event --headless` is production-path evidence; smoke records
    carry `race=<phase> cp=<n>/<total> results=<n>`.
  - `tools/mm2_inspect` + `mm2_game/src/session.rs`: minor touch-ups
    for retained payloads / event mode plumbing.
- Tests added/changed and why:
  - `crates/mm2_content/tests/race_def.rs` (7): AnyOrder layout,
    Ordered lap closure + laps, authored widths/strtpnts, derived
    start, incomplete/unsupported/short events error explicitly.
  - `crates/mm2_content/tests/events.rs`: updated for retained
    payloads + `cir<N>` alias attribution.
  - `crates/mm2_app/tests/event.rs` (7): event load → `Countdown` →
    `Playing`, `RaceState`/`RaceProgress` present, markers spawned
    with correct visibility (finish hidden → revealed), restart
    cleans up and rebuilds markers, a full synthetic course
    drive-through finishes the race with exactly one ledger result,
    and a bad event ref fails the session.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo test -p mm2_content` — 12/12 ok (7 producer + 5 events).
  - `cargo test -p mm2_app` — all groups ok incl. 7/7 event tests.
  - Retail `--event blitz:0 --city london --headless` — `status=pass`,
    `event race loaded event=Blitz[0] gates=3`, `cp=3/3`,
    `race=Running` (finish trigger not crossed in-window — expected).
  - Retail `--event circuit:1 --city sf --headless` — `status=pass`,
    `Circuit[1] gates=10`, car spawned on the authored
    `cir1_strtpnts` grid slot.
  - Retail `--event checkpoint:0 --city london --headless` —
    `status=pass`, `cp=2/5` swept.
  - Retail `--event checkpoint:12 --headless` — `status=fail`,
    exit 3 (explicit `Failed` session, no panic).
  - Retail `--event blitz:0 --frames 90 --screenshot` — 4.3 MB PNG:
    car on the authored start, orange gate column ahead,
    `GET READY 2` countdown on the HUD.
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --workspace --all-targets --all-features --
    -D warnings` — PASS.
  - `cargo test --workspace` — PASS, all groups, 0 failures.
- Acceptance IDs satisfied / still open: F11-AC02/03/04 promoted from
  synthetic-only to authored-data-backed at candidate level (a real
  retail event drives `RaceDefinition` → countdown → swept gates →
  marker updates). AC05 stays open — event props and per-event
  traffic/density overrides need the F09/F10 systems that don't exist
  yet. Opponents/cops (F15/F20), Blitz time limit (F12) and Crash
  Course runtime (F21) remain their own tasks.
- Evidence files: `/tmp/mm2_blitz0_start.png`,
  `/tmp/mm2_blitz0_cd.png` — local only, not committed (original
  content).
- Stock data/GPU/audio/network limitations: GPU exercised (one
  screenshot). Audio/network unchanged — no such code exists. Only a
  handful of retail events sampled; no full-roster execution matrix.
- Unresolved blockers or discovered regressions: none. Provisional
  interpretations recorded as inferred/unknown in the ledger:
  waypoint row roles (WPT-2), `w` = radius (WPT-1), `a` angle
  convention (WPT-4/UNK-16), `cir<N>` alias (WPT-3). `results=0` in
  the retail smoke lines is expected — the driver didn't reach the
  finish in the window.
- Next smallest useful action: F11-C (event audit/CLI polish) or
  F12-A (Blitz timer on the authored `TimeLimit`); independent
  ready alternates: F03-A (prop audit), F09-A (BAI parser).

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
