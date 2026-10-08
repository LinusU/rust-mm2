# Last iteration — F23-A.7 Auto Reverse (iteration 18 of the run)

Selection: the iteration-17 review passed with no blocking findings; its only code-closable gap was already closed. Next ready F23 item that needs no hardware or unavailable content: the original's documented **Auto Reverse** control option (CTL-3). Subtitles are blocked (no retail table references voice text, UNK-35); render scale needs a render-world resolution override that cannot be checked without a GPU capture; both left open.

Change (DSN-98, designed — what the original does with the option off is unrecovered):
- `ControlSettings.auto_reverse` (default on = shipped behaviour; persisted; older `controls.json` stays on) with an `Auto reverse: On/Off` row (`ControlItem::AutoReverse`) on the main-menu Controls screen and the pause Controls page; Reset to defaults restores it.
- `controls::BrakeCarry`, called from `input::vehicle_input` after the device mapping: with the option off, a brake pressed while the car moves (> 0.5 m/s) and still held at <= 0.5 m/s becomes a handbrake hold, so the sim never sees a brake at a standstill and never engages reverse. Release and press again at a standstill = reverse; it stays a reverse pedal while backing away. Pause/countdown forgets the press. No sim or wire change (brake/handbrake bytes only), so a predicted copy matches the authority.
- Pause Controls page grew to 21 rows and draws at 19 px (was 20 px); fit against 720p is estimated, not captured.
- Tests: +7 `controls` unit (default-on passthrough, carry-through-stop hold, fresh press reverses and keeps reversing, stronger handbrake kept + `release`, no-state passthrough, persistence/older file, row toggle + reset), +1 `tests/input.rs::auto_reverse_off_needs_a_fresh_press_to_reverse` through the real system including a pause; menu and session row pins moved by one. Ledger row DSN-98 added.

Gates (foreground): `cargo fmt --all -- --check` PASS; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` PASS; `cargo test --locked --workspace --no-fail-fast` exit 0 (58 `test result:` lines in my run, none failed; the external verify.log counts differently).

Status: candidate; not independently checked. Synthetic only: no real keyboard, no capture of the new row, feel unverified. Still open for F23: render scale, wheel/FFB hardware audit, AC06 hardware record, `Q`/menu/free-fly keys not rebindable, mouse-button rebinding, original R reverse toggle, subtitles (content-blocked), 720p fit of the pause Controls page.
