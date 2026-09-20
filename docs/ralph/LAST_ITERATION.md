# Last implementation iteration

- Task ID and title: F12-B.2 — the low-time warning cue for timed
  races: a session-owned `LOW TIME` banner in `mm2_app` that pulses on
  the authoritative race clock once `time_remaining` reaches 10 s,
  classified **designed** (DSN-9) because the ledger documents no
  original low-time rule (HUD-2 lists only the countdown timer).
- Starting commit and resulting commit: started at
  `56e764c9f266912aed64143d9f89452c3d22c1e4` (clean tree, branch
  `ralph/night`, F12-B.1 externally checked); result = this commit.
- Why this slice: LAST_ITERATION named it — "F12-B remainder: decide
  the low-time warning cue (designed policy) or close F12-B". It was
  the last implementable leg of F12-B's required "warning cues"
  behavior; results/fail screens beyond HUD text stay F17/UI-5 scope,
  and an audio cue is impossible until F07 exists. F12-B moves to
  candidate with those scopes recorded.
- Production code changed:
  - `crates/mm2_app/src/race.rs`: `LOW_TIME_TICKS` (10 s at
    `RACE_TICK_HZ`, inclusive threshold matching the deadline's
    convention), `LOW_TIME_FLASH_TICKS` (0.5 s half-period),
    `LOW_TIME_BRIGHT`/`LOW_TIME_DIM`, the `LowTimeWarning` marker,
    `spawn_race_warning` (hidden `LOW TIME` UI text under the nav
    arrow, session-owned), and `update_race_warning` — armed while the
    race is `Running`, timed, and the `PlayerControl::Local`
    participant is unresolved; the bright/dim phase is
    `((LOW_TIME_TICKS - remaining) / FLASH) % 2`, derived from the
    remaining ticks so it freezes with a pause and cannot drift from
    the deadline (AC04). Hidden for stale/complete/countdown/untimed
    races and a resolved local participant — the `Local` filter avoids
    the latent multi-participant wrinkle the B.1 review flagged on the
    arrow systems.
  - `crates/mm2_app/src/session.rs`: `spawn_race_warning` called in
    the event branch next to `spawn_nav_arrow`, so non-event sessions
    carry no dead UI.
  - `crates/mm2_app/src/main.rs`: `update_race_warning` registered in
    `Update`; the race presentation systems were nested into a
    sub-tuple because the `Update` tuple hit Bevy's 20-system
    `IntoScheduleConfigs` limit.
  - `docs/original-rules.md`: `DSN-9` records the cue as designed —
    threshold, cadence, same-clock derivation, and the deferred audio
    cue.
- Tests added/changed and why:
  - `crates/mm2_app/tests/race.rs` (+4, 27 total): the harness now
    spawns the banner via production `spawn_race_warning` like the
    real session.
    - `low_time_warning_pulses_on_the_race_clock` — hidden above the
      threshold, arms bright as remaining crosses it mid-update,
      bright→dim→bright on the 0.5 s cadence, pause holds the pulse
      phase, expiry (`TimedOut` → `Complete`) hides it.
    - `low_time_warning_waits_for_the_running_phase` — a limit shorter
      than the threshold still shows nothing during countdown, then
      warns from the first running tick.
    - `low_time_warning_ignores_other_participants_deadlines` — a
      finished local participant stops seeing the cue while a
      `Remote` participant keeps the race `Running`.
    - `untimed_race_never_warns_and_teardown_cleans_up` — `None`
      remaining never warns; restart despawns the session-owned node.
    - Harness note discovered: the first `app.update()` runs no fixed
      step, so `clock = 2(n−1)−1` after release — test timing
      comments corrected to match.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo test -p mm2_app --test race` — 27/27 ok.
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features
    -- -D warnings` — PASS (exit 0).
  - `cargo test --locked --workspace` — PASS, all groups, 0 failures.
  - Retail `--city london --event blitz:0 --headless --frames 1800` —
    `status=pass`, `race=Complete cp=3/3 results=1 tl=0.0s
    outcome=timed-out` (warning systems live; deadline resolved once).
  - Retail `--city london --event blitz:0 --headless --frames 1200` —
    `status=pass`, `tl=8.0s` running (sub-threshold).
  - Retail `--city london --event blitz:0 --frames 1150 --screenshot`
    — `status=pass`, PNG shows `time 16.1s`, no banner (windowed runs
    pace ≈0.93 race-ticks/frame, slower than headless — needed 2600
    frames to get under the threshold).
  - Retail `--city london --event blitz:0 --frames 2600 --screenshot`
    — `status=pass`, 4.4 MB PNG: `time 1.5s` with the `LOW TIME`
    banner rendered under the green needle on its dim half-pulse
    (local capture, not committed).
- Acceptance IDs satisfied / still open:
  - F12-AC04 advances: the cue is pure presentation on the same
    authoritative race clock — pulse phase derived from
    `time_remaining` ticks, verified deterministic incl. pause freeze.
    Still partial (no audio exists).
  - F12-AC02/AC03 legs unchanged (F12-A status): the cue adds no
    success path — a resolved/expired race only hides it.
  - F12-AC05 leg: the banner is session-owned — teardown test proves
    despawn; nothing stale survives a restart.
  - F12-AC01/AC06 unchanged — the full Blitz catalog matrix is F12-C.
- Stock data/GPU/audio/network limitations: rendered evidence covers
  the armed banner (dim half) and the hidden state above the
  threshold; the bright half and the pulse toggle are exercised by
  tests, not captured — a mid-pulse frame pair needs timing control a
  `--frames` capture can't supply precisely. No audio system exists,
  so an audible warning stays unimplementable (recorded in DSN-9).
- Unresolved blockers or discovered regressions: none. Incidental
  finding worth knowing: reusing a `--screenshot` path lets the
  waiter pass on stale bytes when the new capture fails — use fresh
  paths (this run's first 2600-frame attempt hit it).
- Next smallest useful action: F12-C — the Blitz catalog
  structural/playthrough matrix (AC01/AC06). Independent ready
  alternates: F03-A (prop audit) or F09-A (BAI parser).

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
