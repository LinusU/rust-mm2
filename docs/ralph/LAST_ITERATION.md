# Last implementation iteration

- Task ID and title: F13-A.1 — the results-flow leg of per-event
  Checkpoint objective/finish semantics.
- Starting commit and resulting commits: started at
  `d758b72c95e674a319b57458cd427eeb6e4a1846` (clean tree, branch
  `ralph/night`; F04-C.3 passed external gates + review).
- Why this slice: the plan's first-listed candidate (F13-A).
  `Session`'s `Playing → Results` transition was legal since F01-A
  but nothing ever made it — a finished or timed-out race left the
  session in `Playing` forever while the HUD only knew
  `FINISHED`/`OUT OF TIME` on a `Complete` race. UI-5 documents a
  results screen after every race, so the terminal-resolution →
  session transition is a real semantic gap implementable now,
  without F15 opponents or F05 destruction.
- What landed (code):
  - `mm2_app::race::advance_race` — when the step that records a
    participant's `Finished`/`TimedOut` result also resolves a
    `PlayerControl::Local` participant, the session transitions
    `Playing → Results` on that same step. The race clock, per-
    participant progress and `ResultLedger` freeze with the phase
    change (the system is gated on `is_playing`). A remote/AI
    participant resolving while the local driver still races changes
    nothing; `RacePhase::Complete` still waits for every participant.
  - `mm2_app::main::update_hud` — during `Results` the HUD line
    surfaces the local outcome (`FINISHED <t>s` carrying the
    recorded race-clock time, `OUT OF TIME`) instead of stale
    checkpoint counts. A full placing/results screen is F13-B/F17.
  - No new machinery: the shared `RaceDefinition`/`RaceProgress`/
    `ParticipantState`/`ResultLedger` and the generation-scoped
    session lifecycle are reused unchanged.
- Tests added (all in `tests/race.rs`, production systems through
  `load_session_world`/`advance_race`/`drive_session`):
  - `local_finish_moves_the_session_to_results` — finish →
    `ParticipantState::Finished` + `RacePhase::Complete` +
    `SessionPhase::Results`; the ledger keeps exactly one result.
  - `a_non_local_resolution_does_not_end_the_local_race` — a Remote
    finish while the local driver races leaves `Playing`/`Running`;
    the local finish then resolves both (AC03 strengthened).
  - `local_timeout_moves_the_session_to_results` — `TimedOut` is
    terminal for the session too (AC02's failure leg).
  - `restart_from_results_rebegins_the_session` — Results is a live
    phase: restart unloads/loads, generation increments, the stale
    `RaceState` is gone (AC05 leg).
- Ledger evidence gathered (install
  `/Users/linus/coding/rust-mm2/retail`, `fnv1a64:e91e6cd4b2ae30d9`):
  - All 24 checkpoint waypoint files (12/city) measured: ≥3 rows
    each, row 0 = start line, distinct last row = finish trigger
    (WPT-2's inferred convention now measured on the full roster —
    london race0 finish ~840 m from the start, london race9's
    closest at ~49 m).
  - `.aimap` vs `.aimap_p` cross-checked against `mmracedata.csv`:
    `[Opponent]`/`[Police]` counts equal the Amateur/Professional
    parameter blocks on 23/24 checkpoint events → RACE-11
    (verified_original). The one anomaly is authored: `sf/race0`
    amateur Opponents=7 while `race0.aimap` wires 6 —
    `race0-a-6.opp` ships unreferenced. Reported, not repaired.
    `docs/research/aimap.md` updated; MP-9's "`_p` = MP variant"
    suspicion corrected; non-Checkpoint `_p` semantics stay
    unverified.
- Retail runtime evidence (`target/debug/mm2`):
  - `--city sf --event checkpoint:0 --bot --headless --frames 12000`
    → `phase=results race=Complete cp=6/6 results=1
    outcome=finished` — the bot's finish ends the session at
    Results on real content. The record's `status=fail` "fell
    through the world" is the pre-existing post-resolution sanity
    check (car ends >25 m under spawn altitude during the remaining
    idle frames) — already documented for 8 SF runs incl. this
    event, not a regression.
  - `--city london --event blitz:0 --headless --frames 2000` →
    `status=pass`, `phase=results race=Complete cp=3/3 results=1
    outcome=timed-out` — the deadline's TimedOut is terminal too.
- What this proves / does not prove:
  - Proves: a local terminal resolution now ends the playing session
    per UI-5 on the production path, synthetic tests and two real
    authored events; per-participant progress stays independent;
    Results is restartable without stale state; the aimap
    difficulty-split convention is measured, not guessed.
  - Does not prove: a placing/results screen (HUD line only);
    opponent participation (F15); progression/unlock saving (F16);
    recovery-penalty semantics (F05); that `_p` means Professional
    outside the checkpoint roster; original-executable results-flow
    timing.
- Classification: `Playing → Results` on local terminal resolution
  is documented (UI-5); the same-step transition and the outcome
  HUD line are implementation choice (DSN-11); `.aimap`/`.aimap_p`
  = Amateur/Professional rosters is verified_original (RACE-11).
- Commands actually run and results:
  - `cargo test -p mm2_app --test race` — 31/31 pass (27 existing +
    4 new).
  - `cargo build -p mm2_app --bin mm2` — clean.
  - Retail runs and data measurements as recorded above.
  - Gates at the candidate commit: `cargo fmt --all -- --check`
    PASS; `cargo clippy --workspace --all-targets --all-features
    -- -D warnings` PASS; `cargo test --workspace` PASS — all
    suites, 0 failures (incl. 31 mm2_app race tests).
- Acceptance IDs satisfied / still open: F13-AC02 (finish
  eligibility now ends the session; invalid-crossing tests already
  existed) and AC03 (independent progress incl. terminal
  resolution) strengthened; AC05 gains a restart-from-Results leg.
  AC04 stays open until F15 opponents race; AC06 stays open until
  representative original races are playable with an honest
  coverage matrix (bot finishes 4/64 events — route-aware driving
  is F15 scope).
- Deferred deliberately: results/placing screen (F13-B/F17),
  progression persistence (F16), recovery/destruction penalties
  (F05), tod/weather event overrides (F18), non-Checkpoint `_p`
  semantics.
- Stock data/GPU/audio/network limitations: retail evidence is
  headless physics records through the VFS plus file measurement;
  no GPU capture, no original-executable comparison.
- Unresolved blockers or discovered regressions: none known.
- Next smallest useful action: F13-A remainder (coverage matrix,
  ledger legs not blocked by F05/F16/F18) or F09-C.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
