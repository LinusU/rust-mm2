# Last iteration — F23-B.9: windowed resolution option (iteration 8 of the run)

Selection: the previous checkpoint (`abd3a55`, F25-C.3) passed gates and review with no blocking findings, so no repair was owed. The reviewer's verification gaps (the impairment proxy is not asserted to have bitten in the test itself; cosmetic no-op shadowing in `assert_shove_converged`) are test-hardening nits inside F25-C, which has taken three iterations in a row; I moved to an independent ready item instead — F23 (controls/options/accessibility), whose req 2 still lacked "resolution/scaling" and whose edge-case list names "invalid display mode". Left on F25-C's list: assert the proxy's drop/dup counters in the impaired shove test; drop the no-op `let` shadowing.

Change:
- `settings::WindowSize` (4 listed 16:9 sizes), `GraphicsSettings.window_size`, `cycled_window_size`, `window_size_row` ("Window size: 1280 x 720", with a "(windowed mode)" note under fullscreen), loaded field-by-field like the other settings.
- `apply_display_settings` now also takes the primary `Monitor`: resizes the window when the (display, size) pair changes — not on unrelated settings changes, so a dragged window is left alone — and clamps to the largest listed size that fits the monitor's logical size (`WindowSize::fitting`) without rewriting the saved choice.
- Startup window built from the setting (the `WINDOW_SIZE` const is gone); perf report `settings.window` follows the setting.
- Menu Options screen and pause graphics page each gain the row; existing tests' hard-coded Options/pause row indexes re-pinned (+1 after VSync).

Results (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` exit 0; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed. New: 5 settings unit tests (steps/persist/invalid, `fitting`, apply with a stand-in 2x monitor incl. drag-survival and fullscreen round trip, oversize startup, headless), menu + session tests step the row and persist it.

Not covered / open: no real display or monitor exercised (stand-in `Monitor` component only); no revert-countdown for mode switches; no exclusive fullscreen/mode list; no render-scale; subtitles, pad rebinding, auto-reverse row still open in F23. Status: implemented candidate; not independently checked.
