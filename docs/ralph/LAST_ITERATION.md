# Last iteration — report 7 item 10: bound the race-row world-clock seek (iteration 4 of the new run)

Selection: iteration 3's commit passed gates and review (no blocking findings); items 1–9 are done as candidates, item 10 is next in order.

Cause: `apply_race_snap` fed `world_ticks(race)` (countdown + the row's unvalidated `u64` clock) straight into `WorldClock::sync`, which queued any target; `advance_world_clock` then replays every actor that many steps in one fixed step. The `World`-frame path was already bounded by `MAX_SEEK_TICKS` in `WorldStage::push`, the row path was not. `countdown + race.clock` could also overflow at `u64::MAX`.

Change:
- `WorldClock::sync` now owns the `MAX_SEEK_TICKS` gate for all sources and returns `SyncOutcome::{InTolerance, Queued, Refused}`; a refusal queues nothing and leaves an already-queued seek alone.
- `apply_race_snap` counts a refusal into the new `NetDriveReport.world_row_refused`; the row's phase/clock still mirror (diagnose, do not coerce). Smoke `wclk=` gains `rowref<n>` only when non-zero, so other records stay bit-identical.
- `world_ticks` uses `saturating_add`.

Tests (`mm2_app` lib): `sync_refuses_a_target_past_the_seek_bound_and_queues_nothing`; `an_absurd_race_row_clock_is_refused_and_the_client_stays_responsive` (production `apply_snapshots` + `advance_world_clock`: honest row seeks, bound+1 and `u64::MAX` rows are refused counted, no seek queued, clock keeps stepping, no overflow). `docs/research/net.md` documents the gate and the residual below.

Residual (stated, not closed): the `WorldLimits` rate/growth bound is not applied to race rows; a host raising its row clock under the cap can still force one capped replay per row.

Gates: fmt, clippy -D warnings, `cargo test --locked --workspace` all exit 0.

Not verified: no two-process run with a hostile host; in-process tests only.

Status: candidate; not independently checked. Next is item 11 (three-process collision test waits on its condition).
