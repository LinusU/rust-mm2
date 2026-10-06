# Last iteration — world clock re-seek (new-run iteration 6)

Selection: F26-A's recorded fix, first slice. Timed drawbridge leaves,
boats, ferries and trains were phase-skewed across peers.

Change: `crates/mm2_app/src/worldclock.rs` (new). `WorldClock` counts
running fixed steps; `apply_race_snap` (netdrive.rs) turns each race row
into a world tick and `WorldClock::sync` queues a seek when more than 6
ticks off; `advance_world_clock` replays the actors from their spawn state
(`WorldStart<T>`, captured on `Added`) before the drivers run. Chained in
main.rs and smoke.rs.

Not done (open in PLAN F26-A): proximity leaves stay local (need a
host-carried trigger); no latency compensation; no two-process leg;
Complete-phase rows do not seek (race clock freezes there).

Tests: 5 new in worldclock.rs (all pass). Gates: see commit result below.
Status: implemented candidate, not independently checked.
