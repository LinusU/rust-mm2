# Last iteration — F23-A.6 mouse driving follow-up (iteration 17 of the run)

Selection: the iteration-16 review passed (CTL-2 mouse driving, code-checked checkpoint) with no blocking findings. Of its verification gaps, one is closable in code: "no explicit test for the pause phase with the mouse buttons held". The other gaps need hardware or a capture (real mouse, original feel, 720p fit of the pause Controls page) and stay open.

Change: test only, no production change. `tests/input.rs::a_pause_releases_a_held_mouse_button_and_resume_restores_it` drives the production `vehicle_input` with mouse driving on, left button held and the cursor at x=600/800: playing gives throttle 1.0 / steering 0.5; `SessionPhase::Paused` gives zero throttle, brake and steering with the button still down; resuming to `Playing` with the button still down drives again (the hold is read from live button state, not latched across the pause).

Gates (foreground): `cargo fmt --all -- --check` PASS; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` PASS; `cargo test --locked --workspace --no-fail-fast` exit 0 (58 `test result:` lines, 0 failed; the run passed the 120 s tool limit and was awaited to completion).

Status: test candidate; not independently checked. Still open for F23: real-mouse feel (designed, not original — DSN-97), relative/captured mouse, mouse-button rebinding, 720p fit of the 20 px pause Controls page (unmeasured), subtitles, render scale, wheel/FFB audit, AC06 hardware record, auto-reverse row.
