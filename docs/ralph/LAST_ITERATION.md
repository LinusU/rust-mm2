# Last implementation iteration

- Task ID and title: F12-C (first slice) — the complete-catalog
  structural audit plus the retail valid/invalid Blitz scenario matrix.
  Includes a spawn-policy repair the matrix itself caught (DSN-6
  amendment, committed separately).
- Starting commit and resulting commits: started at
  `32eab12276e762753452f1cc0ff9349c4d001206` (clean tree, branch
  `ralph/night`, F12-B.2 externally checked); results =
  `f8bd917` (spawn repair) + the audit commit on top.
- Why this slice: LAST_ITERATION named it — "F12-C: the Blitz catalog
  structural/playthrough matrix (AC01/AC06)". Split deliberately: this
  commit delivers the *structural* leg (every cataloged event through
  the production builder at both difficulties) plus the headless
  playable-scenario matrix. Scripted/bot-assisted full playthrough
  completions and reward-fact emission stay open on the parent.
- Production code changed:
  - `crates/mm2_content/src/race_def.rs`: `RaceDefReport::scan(vfs,
    city)` — the whole-city audit. One `RaceDefEntry` per cataloged
    table row (denominator = authored rows, never filtered), each
    built at Amateur + Professional through the production
    `race_definition` producer. Builds classify as
    `RaceDefBuild::Built(RaceDefSummary)` (gates, finish, laps,
    `time_limit_ticks`, start slots, opponents, cops, authored
    `CarType`, time-of-day/weather selectors — the AC01 evidence that
    each event binds its own authored settings),
    `Unsupported` (Crash Course — deferred F21 scope, never counted
    as failure), or `Failed(RaceBuildError)` (NotReady/incomplete,
    bad param, too few rows). Table-level scan errors are kept
    separately so a missing `mm*data.csv` can't silently shrink the
    denominator. Counts: `built/unsupported/failed/failed_events`
    (a single-difficulty failure still flags the event).
  - `tools/mm2_inspect`: `race-defs <dir> [--city] [--table]
    [--strict]` prints the matrix (per-event am/pro cells), defaults
    to every discovered `race/<city>/`, strict exits nonzero on empty
    catalogs, table errors, or failed builds — unsupported does not
    fail strict. `mm2_game` added as a dep for `EventTableKind`/
    `RACE_TICK_HZ`; the city-discovery loop shared with `events` was
    factored into `race_cities`.
  - `crates/mm2_content/src/race_def.rs` (commit `f8bd917`): the
    DSN-6 fallback start now spawns **on** the authored start line
    (was: 10 m back along the row0→row1 tangent — an invented point).
    Retail evidence forced this: london `blitz:6`'s start line sits on
    an elevated deck and the back-off landed past its edge — the car
    spawned over a void and fell to y≈−907 without ever grounding.
    The authored line is the only point the waypoint data guarantees
    is on the course. `docs/original-rules.md` DSN-6 updated.
- Tests added/changed and why:
  - `crates/mm2_content/tests/race_def.rs` (+3, 13 total):
    `report_audits_every_event_at_both_difficulties` — a mixed
    synthetic install (ready/incomplete/blitz/circuit/crash rows)
    produces one entry per row with correct
    built=6/unsupported=2/failed=2 counts and per-difficulty authored
    values (distinct amateur/pro time limits);
    `report_flags_a_failure_on_one_difficulty_only` — a pro-only bad
    `TimeLimit` flags the event while keeping the good amateur build;
    `report_records_table_errors_and_an_empty_catalog` — no
    `race/<city>/` yields 4 table errors + 0 entries, not silence.
    Existing fallback-start test renamed/asserts the on-line slot.
  - `crates/mm2_app/tests/event.rs`: the authored-load test now
    expects the player at `COURSE[0]` (the authored line), not the
    old back-off point.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features
    -- -D warnings` — PASS (exit 0).
  - `cargo test --locked --workspace` — PASS, all groups, 0 failures.
  - `mm2-inspect race-defs <retail>` — london: 45 events, 64 built /
    26 unsupported / 0 failed; sf: identical. Per-event cells show
    distinct authored values (e.g. london `blitz9` 21 gates/120 s am
    vs 103 s pro; `race5` 7 opp/1 cop am vs 7/4 pro). `--table blitz
    --strict` exits 0; `--table crash --strict` exits 0 (unsupported
    is not a failure).
  - Retail headless Blitz matrix (all 20 authored rows, `--frames
    1500`): 20/20 `status=pass` — every event loads its own course,
    the car grounds (wheels contact) and drives, `tl=` ticks down.
    Before the spawn fix this matrix caught 3 falls: london blitz:6
    (spawn-over-void, never grounded), sf blitz:5/9 (fell through the
    world while driving — both were the same back-off defect, not
    collision gaps; they pass after the fix).
  - Invalid scenarios: `blitz:10` → `status=fail` "no authored event
    row for this reference"; `crash:0` → `status=fail` "crash course
    events are not loadable yet"; `bogus:0` → CLI usage error. All
    explicit, no panics.
  - Rendered: london `blitz:6` `--frames 700 --screenshot` — 3.7 MB
    PNG (fresh path, local only): car grounded on the elevated deck
    (wheels 4/4), green nav needle, `cp 0/4`, `time 54.9s`. Plus
    earlier sf `blitz:0` capture (4.6 MB): gate marker + needle +
    `time 25.6s`.
- Acceptance IDs satisfied / still open:
  - F12-AC01 advances: all 90 authored rows across both cities build
    validated definitions through the production producer at both
    difficulties; per-event authored values are distinct and reported
    (not a shared template). Structural leg done; no event is dropped
    from the denominator.
  - F12-AC02 partially: valid events load and run; invalid refs
    (out-of-range, unsupported kind, bad params) fail explicitly and
    cannot manufacture a session. Scripted full *completion*
    playthroughs on retail remain open.
  - F12-AC06 partially: complete catalog structurally tested +
    headless-run on all 20 authored Blitz rows; representative renders
    in both cities exist. Checkpoint/Circuit headless matrix and any
    playthrough-to-finish remain open — unplayed entries stay labeled
    (the audit itself says `built`, not `played`).
  - F12-AC03/AC04/AC05 unchanged from F12-A/B status.
- Stock data/GPU/audio/network limitations: playthrough evidence is
  drive-forward-only — the bot holds throttle, so most runs end
  `race=Running` mid-course, not `Complete`; finishing retail events
  needs steering/scripted inputs or opponents. No audio (F07). No
  opponents/cops spawn yet (F15/F20) — `opp`/`cop` counts are reported
  as authored facts, not simulated actors.
- Unresolved blockers or discovered regressions: none known. The
  matrix's 3 earlier falls were all the same DSN-6 back-off defect,
  repaired — not collision-coverage bugs.
- Next smallest useful action: F12-C remainder — scripted/bot-assisted
  *completions* (steering toward `navigation_target`) to exercise the
  finish/result path on retail events, plus the Checkpoint/Circuit
  headless matrix; then F13-A (Checkpoint feature) proper. Independent
  ready alternates: F03-A (prop audit) or F09-A (BAI parser).

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
