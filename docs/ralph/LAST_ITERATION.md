# Last iteration — scenery poses across peers (iteration 14 of the new run)

Selection: iteration 13 (`916af0e`) passed gates and review with no blocking findings. The checkable half left of report 6 follow-up 3 was the drawbridge/mover actors: the docs said "no check that drawbridge/mover *poses* match the host's after a seek". F27-C still needs a second driven client, so I took this.

Change (candidate, not independently checked):
- `worldclock.rs`: `SceneryProbe` + `sample_scenery` (registered in the headless smoke only; nothing in the game reads it). Every 60 world ticks it hashes the poses of timed drawbridge leaves, boats, ferries and train cars (order-independent, 1 mm quantised) and keeps the newest 32. The record prints `scen=n<actors>,<tick>:<digest>,…` after `wclk=`.
- `net_drive`: `two_retail_processes_stand_the_same_scenery_in_a_cruise` and `_in_a_race` (`MM2_RETAIL`-gated, retail london): at least four shared world ticks, one digest at each, same actor count.
- **Defect found and repaired.** The first run failed: a client matched the host until its first re-seek, then differed at every tick (cruise from tick 360, race from tick 60). `advance_world_clock` replayed the actors to `target` and set the clock to `target`, but the drivers run after it in the same frame and take that frame's step, so a re-seeked client's actors stayed one step (~8 ms) ahead of its clock. That is inside `SYNC_TOLERANCE_TICKS`, so no existing check could see it. A seek in a stepping phase (Countdown/Playing/Results) now replays `target - 1` (a target of 0 keeps the frame's step, clock 1); a `Ready` seek (joiner's first race row) replays in full because no driver step follows.
- Unit tests: seeked leaf equals live leaf at the same clock for each stepping phase and several targets, the `Ready` full replay, probe stride/order-independence/noise tolerance/bounded ring. `the_system_seeks_timed_leaves…` now asserts the one-short replay plus the driver step.

Evidence (retail london, loopback, headless; `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail`): before the fix both new legs failed as above; after it both pass 4/4 runs (~6 s each; cruise client `seek6`, race client `seek3`). Host digests are identical before and after (host unchanged).

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --locked --workspace` exit 0, 58 `test result: ok` lines, none failed (without `MM2_RETAIL`; the retail legs ran separately above).

Not verified: proximity leaves (state depends on cars, still per-peer); scenery/object sounds; impaired-link or windowed run; same machine and binary, so cross-platform float agreement of the replay is unobserved. Follow-up 3 is still not closed for those.

Status: implemented candidate; not independently checked. Next: F27-C multi-client contested pickup (needs a second driven client), proximity-leaf trigger replication, or the remaining F25/F26 lifecycle items.
