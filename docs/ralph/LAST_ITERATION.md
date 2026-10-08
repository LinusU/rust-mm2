# Last iteration — report 7 item 8: low-time warning beeps every second (iteration 2 of the new run)

Selection: previous iteration's checked commit passed gates and review; report 7 item 8 is next in order.

Change: `race_audio::race_cue_voices` replaced the one-shot `watch.warned` bool with `warned_second: Option<u32>`. While the race is `Running` and `time_remaining()` is in `1..=LOW_TIME_TICKS`, the displayed second is `ceil(remaining / RACE_TICK_HZ)` (the same quantity the countdown banner shows); `timerwarning` plays whenever it drops below the last announced second. Result: ten beeps, at remaining = 10 s, 9 s … 1 s, none at 0. Choices: a hitch spanning several seconds coalesces to one beep for the current second (same policy as the start countdown); the tracker only counts down so a clock wobbling back across a boundary cannot repeat a beep; pause returns before the tracker (frozen), restart/new generation resets the watch via the existing identity/stale reset.

Tests (`mm2_app` `tests/audio.rs`): `the_low_time_warning_beeps_once_per_second_from_ten_to_one` (steps the clock one fixed tick at a time over 21 s; asserts exactly ten beeps at remaining = 10..1 s and one per update at most); `the_low_time_warning_coalesces_hitches_and_ignores_clock_wobble`. Existing `race_effects_*` tests still pass.

Docs: ledger DSN-65 and `docs/research/audio.md` record the operator recollection as the evidence (human play memory; not a recovered binary rule, tick alignment still designed).

Not verified: no audible playback was checked (no audio device exercised); beep alignment relative to the HUD flash is by shared `ceil` second, not eyeballed. Original timing not recovered.

Status: candidate; not independently checked. Next is item 9 ("Now go full out for the finish" one checkpoint early).
