# Last iteration — report 7 item 1: police ram (iteration 1 of the new run)

Selection: report 7 outranks everything and item 1 is first. `chase_input` braked every pursuing cop to a halt within `SHADOW_RANGE` 8 m of its goal.

Change (ledger DSN-99, COP-4/COP-9 notes updated; operator recollection is the only evidence):
- `police::chase_input` takes `ram`. `police_pursuit` passes it when the cop has the target in contact this tick (`Pursuing(0)`). Ram: no arrival braking, no stop inside 8 m; inside `RAM_PRESS_RANGE` 7 m full throttle toward the target with the stuck watch and turn-around quiet (a push is not a wedge, so it does not back off); inside `RAM_COMMIT_RANGE` 25 m the bend pace cap is lifted. Contact lost keeps the old halt at the empty last-seen spot (renamed `ARRIVE_RANGE`).
- `police::count_cop_impacts` (+ `PursuitReport::rams`, smoke `pur=...,x<N>`) counts `ImpactEvent`s between a cop and a local human, scheduled with `police_pursuit` in the app and the headless smoke.
- Tests: +4 unit (drive through at speed, target alongside, keeps pressing without wedge/turn-around, degenerate goal), report string, +1 `tests/police.rs::a_chasing_cop_rams_the_player_and_stays_in_contact` (verified to FAIL with ram forced off: `rams: 0`).

Measured run (retail `fnv1a64:e91e6cd4b2ae30d9`, sf Cruise, `--spawn=59,1.5,95,0 --headless`, player idle): 600 frames `status=pass pur=1/0/1,x4`; 1800 frames `pur=1/0/1,nr2,x8`. No before figure recorded (counter did not exist).

Gates (foreground): fmt PASS; clippy `--locked --workspace --all-targets --all-features -D warnings` PASS; `cargo test --locked --workspace --no-fail-fast` exit 0 (73 `test result` lines, none failed).

Status: candidate; not independently checked. Not seen: no windowed capture of a ram; damage/spin-out feel unverified; a moving player was not measured. Report 7 items 2-11 untouched; next is item 2 (light-bar flares).
