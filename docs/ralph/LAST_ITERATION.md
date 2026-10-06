# Last iteration — world clock session reset + wire test (new-run iteration 1)

Selection: F26-A follow-up. Review of the previous slice found two
gaps: `WorldClock` was never reset between sessions (a second race in
one process counted on from the last session's total, so the
tolerance check compared the new host's tick against a stale one), and
the `apply_race_snap` → `WorldClock::sync` path had no test.

Change: `advance_world_clock` (worldclock.rs) now zeroes the clock and
drops an undelivered seek in `Menu`/`Loading`/`Unloading`/`Failed`.
`Ready` and `Paused` leave it alone — a joiner's first race row seeks
while still `Ready`. New tests: `the_clock_starts_over_between_sessions`
(worldclock.rs; the existing system test now stands the session in
`Countdown`, since the default `Menu` phase resets) and
`a_snap_race_row_re_seeks_the_scenery_clock` (net_app.rs — real loopback
row: queued seek to `countdown_ticks + clock`, jitter inside the
tolerance ignored, real drift queued).

Decision recorded: proximity-leaf replication is not worth a protocol
bump now — WLD-26 notes no retail bridge path uses `prox`, so it only
matters for mods. Latency compensation needs an RTT estimate the link
does not have; the offset is one-way delay, under the 6-tick tolerance
on LAN. Both stay open in PLAN F26-A. No two-process leg: the dev-world
cruise carries no `RaceState`, so there is no race row to seek from.

Gates (all exit 0): `cargo fmt --all -- --check`, `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings`, `cargo test
--locked --workspace` (1943 passed, 0 failed). No test processes left.
Status: implemented candidate, not independently checked.
