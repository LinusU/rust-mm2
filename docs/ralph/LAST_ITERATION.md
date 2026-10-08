# Last iteration — review follow-up for F23-A.4 (iteration 14 of the run)

Selection: the external review of `a4989c5` passed with no blocking findings but listed two repairable gaps; repairing a review finding comes before new feature work, and both are small.

1. Overclaim: LAST_ITERATION/PLAN/`pad_map.rs` said the pad design was "recorded as DSN-95", but `docs/original-rules.md` had no such row. Added the DSN-95 row (designed, enhanced policy; rules, loader behavior, synthetic-only evidence). DSN-94 is referenced nowhere in the repo, so nothing to add for it.
2. Harsh load: a `controls.json` `pad` object with an unrecognised action key failed deserialization of the whole file, discarding the key bindings too. The file's `pad` map is now keyed by string; an unknown action name is dropped alone with a load-warning line, the rest of the file (keys, other pad entries) is kept. Test: `controls::tests::an_unknown_pad_action_is_dropped_without_losing_the_keys`. Unknown `DriveAction` keys under `bindings` still take the whole-file path (unchanged).

Gates (foreground): `cargo fmt --all -- --check` PASS; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` PASS; `cargo test --locked --workspace` exit 0 (58 `test result: ok`, 0 failed).

Status: implemented candidate; not independently checked. Still open for F23: subtitles (no verified transcript source), render scale, wheel/FFB audit, AC06 hardware record, non-driving keys not rebindable, mouse/wheel/auto-reverse rows; pad pages have no captured view and no physical-pad run.
