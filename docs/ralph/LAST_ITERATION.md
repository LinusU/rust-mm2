# Last implementation iteration

- Task ID and title: F11-B.1 — shared race lifecycle, swept triggers,
  participant progress and result identity (contract + driver slice of
  F11-B; the catalog→`RaceDefinition` producer and event-session
  loading wiring are split off as F11-B.2 — the task was too broad for
  one coherent change, parent acceptance retained).
- Starting commit and resulting commit: started at
  `bb562724b18f5414539d03add94c2e712f4a301a` (clean tree, branch
  `ralph/night`); result = this commit.
- Design classification (per the contract's four classes):
  - `CheckpointRule::AnyOrder` / `Ordered` are *documented* rules
    (BLZ-1/CHK-1 vs CIR-1) carried on `RaceDefinition`, not imposed
    across modes (spec non-goal).
  - Separate finish trigger inert until full clearance — *documented*
    (RACE-7).
  - Trigger vertical band ±8 m, direction-check default off, 3 s
    countdown at 120 Hz — *designed* defaults (DSN-5 in
    `docs/original-rules.md`); no authored value exists for any.
- Production code changed:
  - `crates/mm2_game/src/race.rs` (new): `Checkpoint` swept cylinder
    test (closest XZ approach within `radius` and `±height`; segment,
    not sampling — a fast car cannot skip a trigger), `CheckpointRule`,
    `RaceDefinition` + `validate`, `RaceState` (generation-stamped
    countdown/clock resource, `input_locked`, `is_stale`),
    `RaceProgress` (per-checkpoint cleared flags, ordered `next`/lap
    wrap, `break_segment` for teleport/reset, one segment consumes
    every checkpoint it crosses), `ParticipantState`, `RaceStarted`
    message, `RaceError`.
  - `crates/mm2_game/src/result.rs`: `SessionResult` gains
    `outcome: SessionOutcome` (`Finished { race_ticks }`) — the first
    real result producer's provenance payload; enum extends as modes
    land.
  - `crates/mm2_app/src/race.rs` (new): `advance_race` in `FixedLast`
    (post-solver `Position` segments — the same path tests drive).
    Countdown ticks during session `Countdown`/`Playing`, releases
    exactly once (`RaceStarted` + participants `AwaitingStart→Racing` +
    `Countdown→Playing`), clock + swept `advance` while `Playing`,
    `Finished` mints+records one `SessionResult` per participant into
    `ResultLedger`, all-finished → `Complete`. Gated on
    `authority_role().is_authority()` — a `Remote` session never steps
    its race; stale-generation resources never step.
  - `crates/mm2_app/src/session.rs`: `drive_session` removes the
    `RaceState` resource on `Unloading → Menu` (no old timer survives);
    `Countdown` is now quittable/restartable like other live phases.
  - `crates/mm2_app/src/input.rs`: `vehicle_input` also honours
    `RaceState::input_locked` — belt-and-braces over the `Countdown`
    session phase for a race resource that outlives its gate.
  - `crates/mm2_app/src/main.rs`: `add_message::<RaceStarted>` +
    `race::advance_race` scheduled in `FixedLast`.
  - `docs/architecture.md`: `mm2_game` bullet gains the race contract;
    `FixedLast` row gains `advance_race`.
  - `docs/original-rules.md`: DSN-5 records the designed defaults.
- Tests added/changed and why:
  - `crates/mm2_game/tests/race.rs` (new, 11 tests): swept geometry
    (high-speed skip, wrong height, boundary, inside/parked, direction
    flag), AnyOrder independent+once clearing, RACE-7 finish gating,
    Ordered sequence/lap-wrap/finish, multi-checkpoint single segment,
    `break_segment` re-anchor, staleness/input-lock, late join,
    validation.
  - `crates/mm2_app/tests/race.rs` (new, 13 tests): the production
    `advance_race` path — countdown unlocks exactly once (AC03),
    countdown completing while session already Playing, AC02 cases via
    `Position` writes (high-speed, wrong-height, repeated, teleport),
    once-only result with generation/participant/event provenance
    (AC04), tied finishes, pause freeze, restart removes `RaceState`
    + a planted stale resource never ticks, quit during countdown,
    remote-authority no-op, and a pin that `advance` is the shared
    step.
  - `crates/mm2_game/tests/contracts.rs`: the three `SessionResult`
    constructors updated for the new `outcome` field.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo test -p mm2_game --test race` — 11/11 ok.
  - `cargo test -p mm2_app --test race` — 13/13 ok.
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features --
    -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, 25 test result groups, 0
    failures.
  - `cargo run -p mm2_app --bin mm2 -- --dev-world --headless` —
    `status=pass`, identical to baseline (updates=600, ticks=1198,
    impacts=1, peak 27.9 m/s, moved 157 m): the race system no-ops
    with no `RaceState`.
- Acceptance IDs satisfied / still open: F11-AC02 (synthetic
  high-speed/wrong-height/repeated/teleport through the production
  trigger path — candidate-level), F11-AC03 (countdown once,
  deterministic pause, no surviving timer — candidate-level), F11-AC04
  (once-only results with provenance — candidate-level). Still open:
  AC01/AC06 (catalog evidence — F11-A checked), AC05 (event props —
  F11-B.2), and *every* AC's real-event evidence: no authored event has
  been loaded through this runtime yet (F11-B.2).
- Evidence files: none committed; no captures made.
- Stock data/GPU/audio/network limitations: no authored event was run —
  all tests use synthetic definitions; `RecordContent` retains counts
  not rows, so no real waypoint data has driven a `RaceDefinition` yet.
  GPU/audio/network unexercised.
- Unresolved blockers or discovered regressions: none. The `Countdown`
  session phase previously had no entrants — nothing else depended on
  it being terminal.
- Next smallest useful action: F11-B.2 — `mm2_content` producer
  turning a `CatalogEvent`'s records into a `RaceDefinition`
  (`RecordContent` must retain or re-read parsed waypoint/start rows),
  `load_session_world` event-mode wiring (`Ready → Countdown`,
  `RaceState`/`RaceProgress` insertion, start-slot spawn), checkpoint
  entities/markers, event-prop session scope (AC05).

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
