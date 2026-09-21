# Last implementation iteration

- Task ID and title: F17-B.1 — pause/resume: `Esc`/pad `Start` pauses a
  live local session, an overlay drives Resume/Restart/Quit, and the
  physics clock freezes for the duration. Chosen over the listed F17-A
  remainder (weather controls are F18-blocked; mouse nav/text entry are
  nav polish) and F16-C's AC01 leg (needs an interactive human session):
  `SessionPhase::Paused`, `allows_pause` (MP-6) and the frozen race
  clocks already existed, but no input path could ever reach `Paused`.
- Starting commit: `19f1f0e4510f33d765c198cbf14519a6fcbb2794` on
  `ralph/night`; tree was clean.

## What changed

- `crates/mm2_app/src/pause.rs` (new) — the in-session half of the
  phase machine's `Paused` state:
  - `PauseMenu` resource: focus, transient status line, redraw latch —
    presentation only; session flow stays in `Session`/`SessionControl`.
  - `pause_input` owns the keyboard while `Paused` (arrow/D-pad/stick
    focus, Enter/Space/South activate, Esc/Backspace/East/Start
    resume). Scheduled after `session_control_input` (which ignores
    `Paused`) and before `drive_session`, so the Esc that entered pause
    is never re-read as a resume in the same update.
  - `sync_physics_pause` mirrors `phase == Paused` onto
    `Time<Physics>::pause()`/`unpause()` — Avian's schedule runner
    skips the step, so the whole world (not just inputs) holds still.
    Mirroring covers every exit path: resume, restart and quit all
    leave `Paused`.
  - `pause_present` draws a `SessionEntity`-stamped overlay (dimmed
    backdrop over the frozen world) carrying `PauseUi`, which
    `HudNodes`/`retarget_hud` keep on the active camera. Quit/restart
    teardown removes it with the session.
  - `dev_pause_once` is the `--pause` dev override: pause the first
    `Playing` frame so a `--frames`/`--screenshot` capture (live input
    frozen) can render the overlay.
  - Rows: Resume, Restart, Quit-to-menu/Quit — and `Options` as a
    visible disabled row naming F23, same honesty convention as the
    root menu (F17 req 3).
- `crates/mm2_app/src/session.rs`
  - `SessionControl` gains a `pause` intent; `quit`/`restart` still win
    a same-frame conflict.
  - `session_control_input` maps Esc/gamepad Start onto `pause` only
    for a `Playing` session whose authority `allows_pause()` (MP-6);
    `Countdown`/`Results`/`Failed` and non-local authorities keep Esc
    as quit — the key always escapes a live session rather than going
    dead.
  - `drive_session`: `Playing + pause` transitions to `Paused`; the
    quit/restart arm now covers `Paused` and clears a stale pause
    intent so the next session's `Playing` can never consume it.
- `crates/mm2_app/src/main.rs`
  - `--pause` CLI flag (conflicts with `--headless`) →
    `DevOverrides::pause`; excluded from `menu_mode` flag selection.
  - `PauseMenu` resource + pause systems in a separate `add_systems`
    (the main Update tuple is at Bevy's arity limit), with the ordering
    contract above and `sync_physics_pause`/`pause_present` after
    `drive_session` so the entering/leaving update sees the settled
    phase.
  - `HudNodes` gains `With<pause::PauseUi>`; `reset_input` now requires
    `session.is_playing()` — `R` can't teleport while paused.
- `crates/mm2_game/src/config.rs` — `DevOverrides::pause` field
  (quarantined evidence-only, like `--spawn`/`--cam`).
- `crates/mm2_app/tests/session.rs` — +4 tests (12 total):
  `esc_pauses_then_resumes_a_frozen_world` (Esc → `Paused` →
  `Time<Physics>` paused → overlay up → 30 frozen updates: no position
  or session-tick drift → Esc resumes → clock/tick resume),
  `pause_menu_rows_drive_the_session` (Enter on Resume resumes; the
  disabled Options row lands its reason on the status line without
  navigating; Quit → `Unloading → Menu` → `AppExit` with no shell),
  `pause_menu_restart_reloads_clean` (generation bumps, world/HUD
  respawn singly, physics unpaused, overlay gone),
  `esc_quits_when_the_authority_cannot_pause` (`SessionAuthority::Host`
  keeps Esc as quit — MP-6), `dev_pause_pauses_once_at_playing`
  (`dev.pause` pauses once; a resume stays resumed).
- `crates/mm2_app/tests/menu.rs` — harness wires the pause systems;
  the four in-session Esc sites now leave through the pause menu
  (`quit_session` helper handles both `Playing`→pause→Quit and
  `Countdown`→direct-quit legs).

## Evidence

- Rendered: `./target/debug/mm2 --mm2-path
  /Users/linus/coding/rust-mm2/retail --city sf --pause --frames 90
  --screenshot /tmp/mm2-pause.png` → `smoke=visual world=city/sf.psdl
  status=pass frames=done screenshot=/tmp/mm2-pause.png
  bytes=3434617` on Apple M1/Metal. The PNG was inspected: the dimmed
  overlay draws over the frozen SF cruise — "Paused" heading, focused
  `> Resume`, Restart, disabled `Options - not implemented yet (F23)`,
  Quit, and the controls footer. Capture kept out of git (local
  evidence only).
- `cargo test --locked -p mm2_app --test session --test menu` — PASS
  (12 + 9).
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, all suites, 0 failures.
- Note: Avian's schedule runner drains one stale-delta step on the
  first paused FixedMain frame (its delta is zeroed after the run
  check, not before) — the freeze is effective from the second paused
  frame; invisible in practice and documented in the freeze test.

## Still open

- F17-B remainder: results screens and the play → reward → return leg
  (AC01's in-session half), `Failed` sessions carrying the reason back
  to the menu, countdown visuals.
- F17-A remainder, unchanged: per-event weather/time/density controls
  (need F18's session-legal writers), mouse navigation, text entry for
  profile names, and the original-menu audit against F17-AC05's
  capability denominator.
- Gamepad pause path is source-only (no device to drive it); the
  pause-overlay dims but does not hide the world — a HUD legibility
  check at other camera modes is unverified.
- F17-AC01/02/04/05 remain unverified; rendered evidence covers the
  root menu and now the pause overlay — sub-screens, the
  play→reward→return leg and high-DPI layouts (spec req 6) are still
  missing.
- F16-C remainder: AC01's process-level leg (two real interactive
  launches completing an event) still needs a playable session;
  `--bot` finishes are deliberately ineligible.
