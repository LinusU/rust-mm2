# Last iteration — report 7 item 9: closing-gate line plays one checkpoint early (iteration 3 of the new run)

Selection: previous iteration's checked commit passed gates and review; report 7 item 9 is next in order.

Cause: `RaceDefinition.finish` is a separate trigger under `AnyOrder` (`mm2_content::race_def::gates_for`: rows 1..n-1 are checkpoints, the last row is the finish, armed once all are cleared — RACE-7). `final_gate_is_next` counted `cleared_count() + 1 == checkpoints.len()`, i.e. the last *checkpoint*, so with the finish still to come it spoke when one checkpoint and the finish remained. `Ordered` (circuits) was already right: the final lap's last gate is the lifted start line, which is the finish.

Change: `race_audio::final_gate_is_next` is now `pub` and, under `AnyOrder`, is true when every checkpoint is cleared if a separate finish exists, or when one gate is left if there is none. Still once per session (`watch.final_gate` plus the announcer's own once-per-cue rule).

Tests (`mm2_app` `tests/audio.rs`): `the_closing_gate_line_waits_until_only_the_finish_is_left` (predicate walked over every state, AnyOrder with/without finish, Ordered 1/2/3 laps: exactly one state fires); `the_closing_gate_cue_reaches_the_announcer_on_the_right_crossing` (production system, observes the announcer's once-per-cue refusal); `every_retail_race_calls_the_closing_gate_once_with_only_the_finish_left` (opt-in `MM2_RETAIL`; production `race_definition` for every non-Crash-Course event, both cities: 64 races, 64 built, 0 unbuilt, 0 wrong, run on retail this iteration; prints "NOT run" otherwise).

Docs: ledger DSN-86 trigger text updated (operator recollection as evidence; the original's true trigger is still unrecovered, UNK-25). I did not look up which row "Embark On It" is; the sweep covers every race.

Not verified: no audible playback; no windowed play-through of a checkpoint race.

Status: candidate; not independently checked. Next is item 10 (bound the race-row world-clock seek).
