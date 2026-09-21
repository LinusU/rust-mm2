# Last implementation iteration

- Task ID and title: F11-C.1 — single-event dependency inspection:
  `mm2-inspect event <dir> --city <stem> --event <table>:<row>
  [--strict]` resolves one authored event row and validates its
  complete dependency closure without launching the game (the
  F11-AC06 inspect leg).
- Starting commit: `2b81ca2e84492047e516f5495efa5ce9f3c5a69e` on
  `ralph/night` — the iteration-13 candidate the external review
  passed (F15-B.3).
- Retail install: `/Users/linus/coding/rust-mm2/retail`
  (`fnv1a64:e91e6cd4b2ae30d9`).

## Why this slice

The plan's selection policy offered F13-B/F14-B remainders, the F15-B
remainder, or F11-C. F14-B's presentation leg is already landed
(`update_hud` shows lap/gate/clock/live place/finish). F11-AC06 asks
for a selected original event to be independently inspectable and
dependency-validated; the existing audits are catalog-wide
(`events`/`race-defs`/`opponents`) and leave `.aimap`/`.pathset`
records `Unparsed`, so a single event could not be deep-checked
without scanning everything. This slice is the smallest unit that
closes that gap through the production APIs.

## What changed

- `mm2_content::race_def::audit_build` and
  `mm2_content::opponents::audit_roster` are now `pub` — the
  per-event build-and-summarize units the catalog-wide
  `RaceDefReport`/`OpponentReport` already used; no behavior change,
  just shared with the single-event path so the two can never
  disagree.
- `tools/mm2_inspect/src/event.rs` — new module:
  - `inspect(vfs, city, table, index) -> Result<EventReport, String>`
    scans the city catalog once and looks the row up via
    `EventCatalog::get`. Unknown refs (bad row, no such table, wrong
    city) are a hard lookup error — exit 2. Found-but-incomplete
    events still produce a full report so the missing piece is
    visible.
  - Every attributed record gets a `RecordCheck`: parsed kinds
    report row counts + diagnostics; `Failed` reports the reason;
    `Unparsed` records are deep-parsed here — `.aimap`/`.aimap_p`
    through `Aimap::parse`+`validate()`, `.pathset` through
    `Pathset::parse`+`validate()`, other kinds honestly labelled
    uninterpreted.
  - Both production builds run at both difficulties via
    `audit_build`/`audit_roster`; Crash Course reports `unsupported`
    (deferred to F21), never a failure.
  - Wired opponent vehicle ids accumulate into a scratch set and are
    cross-checked against `VehicleCatalog::scan` — a roster wiring a
    `vp*` that resolves nowhere is reported.
  - `EventReport::failures()` collects everything `--strict` exits
    on: incomplete status, failed refs/records, record validation
    issues, failed builds, roster issues, unresolved vehicles.
  - `parse_event_spec` accepts the same `<table>:<row>` vocabulary
    as `mm2 --event` (`checkpoint|race`, `blitz`, `circuit`,
    `crash|crashcourse`).
- `main.rs` — `Event` subcommand + dispatch; reuses `build_vfs`,
  `parse_table_filter`, `describe_build`, `describe_roster`.

## Tests (`mm2_inspect` 5 → 12)

- `parses_event_specs` — `circuit:0`, `race:` alias, malformed specs.
- `inspects_a_complete_event` — synthetic install (circuit table +
  waypoints + `_strtpnts` + `.aimap`/`.aimap_p` + `-a-`/`-p-` `.opp`
  routes + empty PTH1 + vehicle catalog stubs): status ready, zero
  failures, aimap deep-parsed, both builds `Built`.
- `unknown_event_is_a_lookup_error` — out-of-range row, missing
  table, wrong city.
- `incomplete_event_reports_but_fails_strict` — removed `.aimap` →
  `status: Incomplete` + non-empty failures.
- `malformed_aimap_is_a_record_failure` — the catalog marks aimaps
  `Unparsed` at scan time; the deep check is what flags it.
- `unresolved_wired_vehicle_is_reported` — roster wires a vehicle id
  absent from the vehicle catalog.
- `crash_course_reports_unsupported_not_failed` — `data.csv` +
  `data_p.csv` + referenced waypoint CSV attached, both builds
  `Unsupported`, zero failures.

## Commands actually run and results

- `cargo fmt --all -- --check` PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` PASS.
- `cargo test --locked --workspace` — 38 suites ok, 0 failures.
- Retail evidence — `fnv1a64:e91e6cd4b2ae30d9`
  (`./target/debug/mm2-inspect event /Users/linus/coding/rust-mm2/retail`):
  - `--city sf --event circuit:1` — ready; 21 records (16 `.opp`
    routes, both aimaps, pathset 23 paths/112 points, waypoints,
    `cir1_strtpnts` alias grid); defs `10g×3lap`/`10g×4lap`; rosters
    `7opp 7rt` both; surfaces the authored anomaly that
    `circuit1-a-7.opp`/`circuit1-p-7.opp` are wired to no opponent —
    `--strict` exits 2 on exactly that finding.
  - `--city london --event crash:0` — ready; `longjump.csv` resolved
    through the crash-data `Filename` link and reported; both builds
    `unsupported (crash course — F21)`, no failure.
  - `--city london --event blitz:0`/`blitz:9` — ready; `radius`-label
    waypoints, authored time limits bound, `[Exceptions]` counted.
  - `--city sf --event checkpoint:0` — ready; reports the known
    RACE-11 authored anomaly (table claims 7 opponents, aimap wires
    6, `race0-a-6.opp` unreferenced) as roster issues.
  - `--city london --event blitz:99` — `error: no authored event row
    for Blitz:99 in london`, exit 2.
- Evidence classification: code gates + synthetic tests + retail CLI
  runs. No GPU/rendered/audio evidence — none needed for an
  inspection command. Original-content coverage is one-row-at-a-time;
  the catalog-wide audits remain the completeness denominator.

## Ledger / research updates

- `docs/ralph/PLAN.md` — F11-C → active (split recorded), F11-C.1
  row added, selection narrative updated.
- No `docs/original-rules.md` change — this slice adds no original-
  behavior claims; it reports authored data and the existing
  designed policies.

## Still open

- F11-C remainder is evidence-recording, not code: full-catalog
  strict audit runs against the fingerprinted install, AC02–AC05
  promotion through the landed runtime slices, AC06's "loaded" leg
  already covered by `mm2 --event` headless smoke records.
- F15-B: catch-up semantics, measured difficulty effects, remaining
  param-tail fields, fixed-seed soak (unchanged).
- F13-B/F14-B remainders, F16-A, F07/F08, F10, F21+ (unchanged).
- `sf checkpoint:0`'s idle-player "fell through the world" smoke
  artifact remains pre-existing (unrelated to this slice).
