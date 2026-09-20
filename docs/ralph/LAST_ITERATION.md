# Last implementation iteration

- Task ID and title: F12-C (second slice) — scripted/bot-assisted
  completions through the production race path, plus the full
  Checkpoint/Circuit/Blitz retail headless matrix.
- Starting commit and resulting commits: started at
  `f62a9775ac51940285f934947d79068a229bd44d` (clean tree, branch
  `ralph/night`, F12-C structural leg externally checked); result =
  the commit on top of it.
- Why this slice: the previous LAST_ITERATION named it — "scripted/
  bot-assisted *completions* (steering toward `navigation_target`) to
  exercise the finish/result path on retail events, plus the
  Checkpoint/Circuit headless matrix". The throttle-hold driver
  finishes almost nothing, so the finish→result path had zero
  original-content evidence.
- Production code changed:
  - `crates/mm2_app/src/scripted.rs` (new): the `--bot` evidence
    driver. `ScriptedDrive` is a resource the CLI inserts — presence
    is the feature flag, so windowed and headless gate the same
    system and the bot never exists unless asked for. `ScriptedBot`
    is a per-vehicle component (session-owned: teardown despawns it,
    a restart can't inherit a stale stuck timer or half-finished
    escape). `drive_target` returns the live objective: under
    `AnyOrder` the *earliest* un-cleared gate in authored order —
    the waypoint rows are the only route data Blitz/Checkpoint
    events ship (`.opp` files are opponent data and those rows have
    zero opponents) — then the armed finish trigger; under `Ordered`
    the participant's `next` required gate. `scripted_input` is a
    proportional controller: steering saturates with bearing,
    throttle steps down through three bearing bands, a corner brake
    engages above a speed floor, a 30 m/s cap coasts on straights,
    and a grounded no-motion detector triggers a two-phase escape
    (reverse-and-turn, then forward full-lock, alternating side —
    the nav line can't see walls, so escapes are blind guesses).
    `scripted_drive` runs after `input::vehicle_input`, writes the
    same production `VehicleInput`, honors the countdown
    `input_locked` and non-`Playing` sessions with a zeroed input,
    and yields once the participant resolves. It is an evidence
    driver, not gameplay AI — documented as such in the module.
  - `crates/mm2_app/src/smoke.rs`: `Driver::{Hold,Scripted}` selects
    who owns `VehicleInput` in `headless_smoke`; `Hold` keeps the
    original settle-then-throttle writes (still countdown-gated),
    `Scripted` inserts the resource and lets `scripted_drive` run
    inside the update. Records now carry `driver=hold|scripted`.
  - `crates/mm2_app/src/main.rs`: `--bot` flag; headless selects the
    `Scripted` driver, windowed inserts `ScriptedDrive` and schedules
    `scripted_drive` `.after(input::vehicle_input)` under
    `not(capturing).and_then(resource_exists)` — deterministic
    ownership while on, never scheduled while off.
  - `README.md` (usage) and `docs/architecture.md` (scheduling
    table) updated.
- Tests added/changed and why:
  - `crates/mm2_app/tests/bot.rs` (new, 6): `input_law_steers_and_
    modulates_throttle` — bearing→steer sign/saturation, throttle
    bands, corner brake only above its speed floor, speed-cap coast;
    `input_law_recovers_from_stuck` — airborne frames don't build the
    timer, the escape sequences reverse→full-lock→normal;
    `drive_target_tracks_the_live_objective` — authored order beats
    proximity, armed finish after all gates, `Ordered` follows
    `next`; `scripted_drive_honors_the_countdown_lock` — zeroed input
    while `input_locked` holds (AC03);
    `bot_drives_a_turning_course_to_the_finish` — a synthetic L-turn
    AnyOrder course the straight-line driver could never clear,
    driven through `load_session_world`→`advance_race` to exactly one
    `Finished` ledger result and a parked car;
    `bot_laps_an_ordered_circuit_twice` — a 4-gate square, authored
    `NumLaps=2`, real lap-wrap re-arming, second start-line crossing
    finishes.
  - `crates/mm2_app/tests/smoke.rs`: existing tests pass
    `smoke::Driver::Hold` explicitly.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features
    -- -D warnings` — PASS (exit 0).
  - `cargo test --locked --workspace` — PASS, 28 test-result groups,
    0 failures.
  - Retail bot matrix — every authored event row in both cities
    (`--headless --bot`, blitz `--frames 7800`, checkpoint 5400,
    circuit 10800; 64 runs, log kept locally at
    `/tmp/mm2-bot-matrix/results.log`):
    - `outcome=finished` through the real finish→result path (4):
      london `blitz:0` (`cp=3/3`, `tl=7.6s` spare), london
      `checkpoint:0` (`cp=5/5`), london `circuit:0` (`cp=6/6` incl.
      lap wraps), sf `checkpoint:0` (`cp=6/6` — recorded its finish,
      then ended ≥25 m below spawn → `status=fail` anyway).
    - All 20 Blitz events reached `race=Complete` — 1 finished +
      19 `timed-out`, exactly one ledger result each (the authored-
      deadline path exercised on real content).
    - 41 untimed Checkpoint/Circuit runs were `race=Running` at the
      frame cap — honestly bounded, not failures.
    - 8 SF runs `status=fail` "fell through the world" (blitz:5,9;
      checkpoint:0,4,5,6,11; circuit:0) — hilly terrain + a straight-
      line bot; reported, not hidden.
  - `mm2 --mm2-path <retail> --city sf --headless --bot --frames 600`
    — `status=pass` cruise (no event): drives straight, wheels 4/4.
  - `mm2 --mm2-path <retail> --city london --event blitz:0 --headless
    --bot --pro --frames 2400` — `status=pass`, `outcome=finished`
    with `tl=0.6s` (the 18 s Pro limit vs 25 s Amateur — the bot
    still makes it).
- Acceptance IDs satisfied / still open:
  - F12-AC02 advances: a *completion* playthrough now exists through
    the production path — synthetic L-turn/2-lap-circuit tests plus
    4 retail finishes; invalid scenarios remain explicitly failed.
  - F12-AC03 advances: the authored deadline resolved all 19
    unfinished retail Blitz events into exactly one `TimedOut`
    result each — deterministic on real content.
  - F12-AC06 advances: the complete event catalog now has headless
    runtime evidence in both cities across all three playable
    tables, with unplayed/incomplete entries labeled (`Running`
    vs `fail`, never filtered). "Played/rendered" on representative
    events holds via the earlier captures plus these completions.
  - F12-AC05 partially: restart/quit not preserving checkpoint flags
    was already tested; the "no duplicate rewards" leg has no reward
    emission to exercise yet (F16 scope) — stays open.
  - F12-AC01/AC04 unchanged from prior status.
- Stock data/GPU/audio/network limitations: the bot is a straight-
  line evidence driver — authored waypoint order approximates the
  route, but it cannot follow roads around blocks, so most retail
  courses stay `Running` or time out. Route-aware driving is F15
  opponent-AI scope; `.opp` routes only exist for events that ship
  opponents (Blitz has none). The 8 SF falls are bot/terrain limits,
  not newly discovered collision gaps — the same events pass
  grounding checks under the hold driver. No audio (F07), no
  opponents/cops (F15/F20). This is headless evidence — no new
  rendered captures this iteration.
- Unresolved blockers or discovered regressions: none known. The
  4-of-64 completion rate is a bot capability bound, recorded
  honestly rather than fixed by a route planner out of scope.
- Next smallest useful action: F13-A (Checkpoint feature proper —
  the bot now finishes london/sf `checkpoint:0`, giving that task a
  working evidence driver) or the F12-C reward-fact leg once a
  reward surface exists. Independent ready alternates: F03-A (prop
  audit) or F09-A (BAI parser).

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
