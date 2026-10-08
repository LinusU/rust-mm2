# Last iteration — F29-C.6: the Professional side of lesson mods (iteration 14 of the new run)

Selection: F29-C.5 (`7c3c6fe`) passed gates and review with no blocking findings, so no repair was owed. F21 stays blocked (UNK-35). The review's verification gaps named the Professional `crash<N>data_p` table override and the `.aimap_p` fallback as untested under lesson mods; this slice covers exactly those. No production change was needed.
Tests (`tests/mod_override_race.rs`, synthetic `testcity`), `lesson_race_setup` at both difficulties:
- `a_mod_for_one_lesson_difficulty_moves_only_that_difficulty` — a `_p` table moves only Professional's leg budget; a `.aimap_p` moves only Professional's lead car; a mod's `.aimap` alone reaches Professional via the RACE-11 fallback (the fixture ships no `.aimap_p`) until a `_p` is mounted; unmounting restores both stock lessons.
- `professional_lesson_mods_are_gameplay_and_move_the_fingerprint` — each `_p` mod classifies as gameplay and moves the fingerprint.
- `a_malformed_professional_lesson_aimap_is_refused_while_amateur_still_launches` — a malformed `.aimap_p` is `LessonSetupError::Aimap` at Professional (no fallback to the good `.aimap`); Amateur never reads it and launches.
Not done: the LeadRoute refusal is still unverified against a retail install (MM2_RETAIL unset); DegenerateRoute is covered at `lesson_race_setup` level only; surface tables, menu art, localization.
Gates (foreground): fmt pass; clippy `--locked --workspace --all-targets --all-features -D warnings` finished clean; `cargo test --locked --workspace` rc 0, 58 result blocks ok, 0 failed.
Status: implemented candidate, not independently checked; synthetic only. Tests passed on first run, so no mutation check shows they would fail on a regression; the assertions are on distinct values per difficulty.
