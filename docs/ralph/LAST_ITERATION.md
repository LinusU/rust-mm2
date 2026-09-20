# Last implementation iteration

- Task ID and title: F01-C — session start/quit/restart integration in
  the real app, plus the plugin-attach documentation the F01 spec asks
  for (AC06). Iteration 007 produced no diff (session terminated
  mid-turn); this iteration implements the queued slice at `d0cca68`.
- Starting commit and resulting commit: started at
  `d0cca688732c139fd35b9ae02f3b0b0bf698fd9d` (clean tree, branch
  `ralph/night`); result = this commit.
- Production code changed:
  - `crates/mm2_app/src/session.rs` (new lib module): the binary's
    `Startup` `setup` system moved here as `load_session_world`, now
    gated on `SessionPhase::Loading` so every `begin` re-runs it;
    `SpawnPoint`/`SelectedCar`/`TunedVehicle`/`Hud`/`ErrorText`/
    `AssetStores` moved with it. New: `SessionControl` intents,
    `session_control_input` (`Esc` → teardown then exit; `Backspace` →
    restart with the same config — there is no menu yet, that is F17),
    `loading`/`unloading` run conditions, and `drive_session` — the
    phase driver. `Unloading` stays until the `SessionEntity` root
    query is observably empty (chained after `despawn_session_entities`
    so it normally completes same-frame), then clears session-scoped
    state (`ImpactFilter::reset`, `SpawnPoint.trailers`) → `Menu`. At
    `Menu`, `restart` calls `Session::begin` with the retained config
    and `quit` writes `AppExit::Success`; `quit` wins if both intents
    are set.
  - `crates/mm2_app/src/contracts.rs`: `ImpactFilter::reset()` — clears
    the `Entity`-keyed dedup map (a recycled entity in the next session
    must not inherit a cooldown) and restarts the per-session id +
    emitted/dropped counters.
  - `crates/mm2_app/src/main.rs`: schedules the new systems in `Update`
    (`load_session_world.run_if(loading)`, `session_control_input` gated
    like other inputs on `not(capturing)`, despawn→driver chained);
    `SessionControl` resource; moved types imported from the lib.
  - `docs/architecture.md`: new "Session lifecycle and how features
    attach" section — phase diagram, per-schedule system table, the
    `SessionEntity`/teardown/reset ownership rules, and how a feature
    plugin attaches contract types vs producers (AC06).
  - `README.md`: `Esc`/`Backspace` rows in the controls table.
- Tests added/changed and why:
  - `crates/mm2_app/tests/session.rs` (new, 5 tests): a headless app
    wired like the binary (real `load_session_world`/`drive_session`/
    `despawn_session_entities` + contract pipeline, real Avian) verifies
    AC01 (restart leaves exactly one of every session-owned entity —
    cameras, player, HUD, physics bodies — all stamped generation 2,
    trailer bookkeeping cleared, clock restarted), quit → `Menu` →
    `AppExit::Success`, AC02 (a missing-city load fails with zero player
    simulation/physics bodies, then retries identically and still quits
    cleanly), the impact stream restarts ids per generation, and AC03
    (identical throttle at 60 vs 30 updates/s yields the same
    pose/speed after the same 240 fixed ticks).
  - Test choreography notes: the dev car's spawn drop is caught by its
    raycast wheels with no chassis contact, so impact evidence uses a
    dropped marked box (same trick as the contracts tests);
    `AppExit`/`ImpactEvent` messages age out of their buffers, so the
    tests observe them fresh rather than after N fixed updates.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo test -p mm2_app --test session` — 5/5 pass.
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features --
    -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, 0 failures.
  - `cargo run -p mm2_app --bin mm2 -- --dev-world --frames 60` —
    `smoke=visual status=pass` (windowed path through the moved spawn).
  - `cargo run -p mm2_app --bin mm2 -- --mm2-path <retail> --city sf
    --headless` — `status=pass` (updates=600, ticks=1198, impacts=3,
    dropped=0, peak=37.2 m/s, moved=172 m) — identical to the F00-C
    baseline metrics.
- Acceptance IDs satisfied / still open: F01-AC01 and AC02 now have
  production-path integration evidence (was contract-level only);
  AC03's fixed-tick claim gained a cross-batching equivalence test;
  AC06's documented ownership/scheduling diagram landed. AC04/AC05 were
  already evidenced at contract level. F01 remains a feature candidate —
  interactive quit/restart was not exercised by hand, and Paused/
  Countdown/Results phases are still unreachable in the app (no race or
  menu consumer yet — F11/F17).
- Evidence files: none committed; no captures made.
- Stock data/GPU/audio/network limitations: unchanged — no audio or
  networking code exists; authored surface-code meanings remain unknown
  per the rules ledger; real-city geometry still reports
  `SurfaceMaterial::Unspecified` (F06). Interactive Esc/Backspace
  handling is covered only at the system level, not by a manual
  playtest.
- Unresolved blockers or discovered regressions: none. Carry-overs:
  `ResultLedger`, `PlayerControl::Ai`, Host/Remote authority paths still
  have no consumers; `Countdown`/`Results`/`Paused` unreachable in the
  app; `ImpactDedup` still grows per seen pair within a session
  (bounded by session lifetime, disclosed); the edge-triggered
  severity-escalation gap documented in `ImpactPolicy::min_severity`
  remains open by design.
- Next smallest useful action: F11-A — race catalog and shared runtime
  (deps checked; unblocks F12–F15, F21, F26, F27). Likely split:
  `EventRef`-resolving event catalog over the already-parsed
  `mm*data.csv` tables first, then `.opp`/waypoint parsing.

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
