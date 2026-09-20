# Last implementation iteration

- Task ID and title: F12-A — Blitz-specific authored rules bound into
  the shared race runtime: authored `TimeLimit` → fixed ticks,
  distilled event params, inclusive-deadline timeout enforcement,
  authoritative `TimedOut` results, HUD/smoke remaining-time output.
- Starting commit and resulting commit: started at
  `8f8ae4a8c927c6666821707ef71f0ebd59217fb4` (clean tree, branch
  `ralph/night`, F11-B.2 externally checked); result = this commit.
- Why this slice: F11-B.2's review left the Blitz timer as the named
  scope gap ("blitz time limit ... remain open by design on parent/
  other tasks"). With the `CatalogEvent → RaceDefinition` producer
  checked, the Blitz timer is the highest-value ready leg: every
  authored Blitz row carries a per-difficulty `TimeLimit` the runtime
  previously ignored entirely.
- Production code changed:
  - `crates/mm2_game/src/race.rs`: `RaceDefinition` gained
    `time_limit_ticks` + `params: EventParams`; `EventParams` distills
    the authored parameter block (`SessionConditions`,
    `Densities`, opponent/cop counts, car type) so the producer stops
    dropping authored settings on the floor. `RaceState::time_remaining`
    reports `limit − clock` saturating at 0 on the one authoritative
    race clock; `validate()` rejects `Some(0)` (`RaceError::BadTimeLimit`).
    `ParticipantState::TimedOut { race_ticks, result }` is the new
    terminal state — the driver only advances `Racing` participants,
    so a timed-out participant freezes like a finished one.
  - `crates/mm2_game/src/result.rs`: `SessionOutcome::TimedOut {
    race_ticks }` alongside `Finished`; the ledger now retains
    `SessionResult` records (was: ids only) so the timeout outcome —
    and the finish tick — are queryable (`get`, `iter`).
  - `crates/mm2_content/src/race_def.rs`: the producer binds the
    authored parameter block (`event.race_params(difficulty)`) into
    `EventParams` with range validation — selectors 0–3, densities
    0..=1, non-negative actor counts; out-of-range authored values
    fail with `RaceBuildError::BadParam`, never clamp. Blitz rows
    convert `TimeLimit` seconds → 120 Hz ticks; non-finite/
    non-positive/unrepresentable values fail explicitly. Checkpoint/
    Circuit rows stay untimed — their constant 50/40 `TimeLimit` is a
    likely-unused template column (UNK-4).
  - `crates/mm2_app/src/race.rs`: `advance_race` enforces the deadline
    while `Playing` — participant segments evaluate first, then any
    unresolved participant (Racing *or* unreleased AwaitingStart) with
    `clock >= limit` mints exactly one `TimedOut` result into the
    ledger. Finish-on-the-expiry-tick wins (DSN-7: inclusive boundary).
    `Complete` still requires every participant terminal.
  - `crates/mm2_app/src/main.rs`: HUD shows `m:ss` remaining while a
    timed race runs (same `RaceState` clock — AC04), `OUT OF TIME` on
    a timeout-completed race, elapsed time for untimed races.
  - `crates/mm2_app/src/smoke.rs`: headless smoke records carry
    `tl=<remaining>s` and `outcome=<name>` from the retained ledger.
- Tests added/changed and why:
  - `crates/mm2_content/tests/race_def.rs` (+3): authored Blitz binds
    per-difficulty limits (25 s → 3000 ticks amateur / 18 s → 2160 pro)
    plus conditions/densities/car type; Checkpoint/Circuit rows stay
    untimed; bad selectors/densities/actor counts/limits fail with
    `BadParam` naming the field.
  - `crates/mm2_game/tests/race.rs` (+1): `time_remaining` counts down
    and saturates; `validate` rejects a zero limit.
  - `crates/mm2_app/tests/race.rs` (+3, 18 total): timeout records one
    `TimedOut` result and completes without refiring; a finish on the
    exact expiry tick counts (inclusive boundary); an unreleased
    `AwaitingStart` participant can't hold the race open past the
    deadline; pause already freezes the clock (existing test) so no
    deadline accrues while paused.
  - `crates/mm2_app/tests/event.rs` (+1): end-to-end authored Blitz
    fixture — `mmblitzdata.csv` 0.5 s limit → `time_limit_ticks=60`,
    params on the definition, `Ready → Countdown → Playing`, race
    expires into `TimedOut { race_ticks: 60 }` through the real
    `advance_race`.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo test -p mm2_game -p mm2_content -p mm2_app` — all groups ok
    (incl. 10 race_def, 18 race, 8 event tests).
  - Retail `--event blitz:0 --city london --headless` — `status=pass`,
    `cp=1/3 tl=18.0s` (25 s amateur limit − ~7 s raced).
  - Retail `--event blitz:0 --city sf --headless` — `status=pass`,
    `cp=1/4 tl=23.0s` (30 s amateur limit).
  - Retail `--event blitz:0 --frames 2200 --headless` — `status=pass`,
    `race=Complete cp=3/3 results=1 tl=0.0s outcome=timed-out`: the bot
    cleared all three authored gates but never crossed the finish
    trigger inside the authored 25 s, so the deadline expired it —
    the inclusive-boundary/finish-arms-after-gates semantics on real
    data.
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --workspace --all-targets --all-features --
    -D warnings` — PASS.
  - `cargo test --workspace` — PASS, all groups, 0 failures.
- Acceptance IDs satisfied / still open:
  - F12-AC01 partial: Blitz events bind their authored limit +
    conditions/densities/actor counts at load (structural coverage of
    the full 40-row Blitz roster not yet exercised — the catalog
    already resolves all of them ready, but per-row limit binding is
    only sampled, not matrixed).
  - F12-AC03 satisfied at candidate level: timeout and
    finish-on-boundary are deterministic, tested both ways.
  - F12-AC04 partial: timer/countdown/results all run on `race.clock`;
    HUD text uses the same clock — but no audio cues exist (no audio
    system at all), and the objective/navigation HUD (RACE-6 compass)
    is F12-B/F22.
  - F12-AC02/AC05/AC06 open: invalid/repeated objectives and restart
    dedup were already covered under F11-B but Blitz-specific restart/
    reward flow isn't separately exercised; full-roster playthrough
    matrix and both-cities rendered evidence are owed by F12-C.
  - UNK-4 stays open: seconds is the bound unit by strong inference
    (course length vs MM2 speeds), documented provisional in DSN-7.
- Stock data/GPU/audio/network limitations: retail headless runs
  exercised the real VFS/authored data; no GPU screenshot this
  iteration (timer is HUD text — the windowed capture path is
  unchanged); audio/network nonexistent by design so far.
- Unresolved blockers or discovered regressions: none. The timeout
  smoke shows the scripted driver clears all gates then wanders — the
  finish trigger still requires an actual crossing (RACE-7 semantics
  hold on real data).
- Next smallest useful action: F12-B (timer/objective/navigation HUD,
  warning cues, Blitz restart flow through the shared runtime) or
  F11-C; independent ready alternates: F03-A (prop audit), F09-A
  (BAI parser).

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
