# Last implementation iteration

- Task ID and title: F17-B.2 — results screen and the
  play → reward → return leg: a real overlay at `Results` (UI-5), an
  authoritative `SessionReport` so the screen presents the actual
  persistence/reward disposition, `Failed` carrying its reason back to
  the menu, a menu-reopen repair on the restart path, and a
  quarantined `--finish` dev override so captures can reach the phase.
  Chosen over the remaining F17-B items (countdown visuals are
  presentational; C&R scoring needs F17-C's mode) and the F17-A
  remainder (weather controls are F18-blocked): `Results` existed in
  the phase machine but was only a HUD line with Esc-quit, and the
  reward path's outcome was invisible to the player.
- Starting commit: `e82b79f06db240bdec7aac2dc16998dff1088c8a` on
  `ralph/night`; tree was clean.

## What changed

- `crates/mm2_app/src/results.rs` (new) — the `Results` phase's
  screen, mirroring `pause.rs`'s shape:
  - `ResultsMenu` resource: focus, status, redraw latch —
    presentation only; session flow stays in `Session`/
    `SessionControl`.
  - `results_input` owns the keyboard while `Results` (arrow/D-pad/
    stick focus, Enter/Space/South activate, Esc/Backspace/East
    continue). Scheduled between `session_control_input` (which now
    ignores `Results` for Esc) and `drive_session`, so the key that
    ended the race can't be re-read as a quit in the same update.
  - `results_present` draws a `SessionEntity`-stamped `ResultsUi`
    overlay which `HudNodes`/`retarget_hud` keep on the active
    camera; teardown removes it with the session. Rows: outcome line
    (`1st of 7 — 0.1s` / timeout), standings from the
    generation-scoped ledger (resolved participants placed per
    DSN-12; unresolved listed `still racing`, never ranked), granted
    rewards, the `SessionReport` note, and
    Continue(-to-menu)/Quit + Restart.
  - `dev_finish_once` backs `--finish`: teleports the local
    participant to its next navigation target once per update until
    the run resolves (countdown `input_locked` honoured; no
    `Teleported` stamp, so swept segments stay honest).
- `crates/mm2_app/src/progression.rs` — `SessionReport` resource
  (`generation`, `recorded`, `granted`, `note`) and
  `record_session_results` reworked: resets on generation change,
  consumes the local participant's pending results exactly once
  (blocked results still marked consumed), applies the
  sandbox/`ScriptedDrive`/`record_eligibility` gates and records the
  refusing reason as the report's note instead of only logging it;
  `TimedOut` notes non-recording. The report is what the results
  screen prints.
- `crates/mm2_app/src/session.rs` — `SessionNote` resource carries a
  `Failed(reason)` through teardown; `drive_session` also removes
  `RaceState`/`EventRewards`/`SessionReport` and the city/surface
  session resources on teardown; `session_control_input` stops
  claiming Esc at `Results` (the overlay owns it).
- `crates/mm2_app/src/menu.rs` — `menu_watch` enforces shell
  ownership: active only while `Menu` *and* no `restart` is pending
  (repairs a latent defect — a restart transits `Menu` for one
  update and could reopen the shell over the live session, stealing
  keys and spawning the menu camera); consumes `SessionNote.failure`
  onto the status line (`load failed: <reason>`). `menu_input` is
  phase-gated to `Menu`.
- `crates/mm2_game` — `DevOverrides::finish` (documented as
  outcome-changing, unlike the presentation-only `--pause`);
  `record_eligibility` refuses it as `dev override finish`;
  `result::ordinal` (`1st`/`2nd`/`3rd`/`11th`…) shared by the HUD
  and the overlay — the app's local duplicate was removed.
- `crates/mm2_app/src/main.rs` / `smoke.rs` / `lib.rs` — `--finish`
  flag → `DevOverrides::finish`; `results` module registered;
  `ResultsMenu`/`SessionReport`/`SessionNote` resources and the
  results/finish systems wired into both the app and the headless
  smoke schedule with the ordering contract above.
- `crates/mm2_app/tests/results.rs` (new, 9 tests): outcome+field
  presentation, unresolved participants listed, timeout
  presentation, profileless + dev-override ineligibility notes,
  granted-reward lines, stray keys ignored at `Results`,
  Continue→teardown→`AppExit`, Restart replays the event,
  `--finish` reaches `Results` and reports ineligible.
- `crates/mm2_app/tests/menu.rs` (+2, 11 total): a restart from the
  pause menu never reopens the shell mid-session; a failed launch
  returns to the menu showing the reason.
- `crates/mm2_app/tests/session.rs` — harness wires the new
  resources/systems for fidelity.

## Evidence

- Rendered: `./target/debug/mm2 --mm2-path
  /Users/linus/coding/rust-mm2/retail --city sf --event checkpoint:0
  --finish --frames 180 --screenshot /tmp/mm2-results-review.png` →
  `smoke=visual world=city/sf.psdl status=pass frames=done
  screenshot=/tmp/mm2-results-review.png bytes=2573476` on Apple
  M1/Metal. The PNG was inspected: the overlay draws over the
  finished race — "Race results" heading, `1st of 7 — 0.1s` outcome,
  `1. You — 0.1s` plus six opponents listed `still racing`, the
  `no driver profile — progress is not saved` note, focused `> Quit`
  and `Restart race` rows, controls footer. Capture kept out of git
  (local evidence only).
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — PASS.
- `cargo test --workspace` — PASS, all suites, 0 failures (results
  9, menu 11, session 13).

## Still open

- F17-B remainder: countdown presentation; C&R/points result
  variants are F17-C scope (no such mode exists yet).
- The results overlay's `Continue` is quit-to-menu (or `AppExit`
  with no shell) — the original's post-race flow returns to a
  results/records sub-screen; a Records screen (DRV-5) is
  unimplemented.
- Standings show unresolved opponents as `still racing` — honest,
  but a session that sits at `Results` forever never resolves them;
  no "wait for field" behaviour is claimed (the original ends the
  race on the local finish too — DSN-11).
- Gamepad results path is source-only (no device to drive it);
  overlay legibility at other camera modes/high-DPI unverified —
  same standing gap as the pause overlay.
- F17-A remainder, unchanged: per-event weather/time/density
  controls (need F18's session-legal writers), mouse navigation,
  text entry for profile names, and the original-menu audit against
  F17-AC05's capability denominator.
- F17-AC02/AC05 remain unverified; AC01's return leg and AC04's
  failed-load leg are now exercised by tests + the rendered capture,
  pending external check.
- F16-C remainder: AC01's process-level leg (two real interactive
  launches completing an event) still needs a playable session;
  `--bot` and `--finish` results are deliberately ineligible.
