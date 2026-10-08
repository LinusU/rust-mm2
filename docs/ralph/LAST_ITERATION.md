# Last iteration — F25-C.3 test hardening (iteration 9 of the run)

Selection: the previous checkpoint (`00c09e5`, F23-B.9) passed gates and review with no blocking findings, so no repair was owed. I took the small open verification gap I had left on F25-C.3 (`abd3a55`): the impaired shove test judged convergence without asserting that the impairment recipe actually fired. F23's remaining items (display revert countdown, subtitles, pad rebinding) are each larger than one focused change, so they stay open.

Change (test-only, `crates/mm2_app/tests/net_drive.rs`):
- `run_shove_trio` now returns the `ImpairProxy` up/down `LinkStats` (read before the proxy drops) alongside the two client records.
- `a_shoved_seat_converges_across_three_processes_on_an_impaired_link` asserts both directions carried frames, were delayed, and dropped/duplicated/reordered at least one frame, so a recipe that silently did nothing now fails the test instead of passing it.
- Dropped the no-op `let (control_bob, alice, bob) = (...)` shadowing in `assert_shove_converged`.

Results (foreground): `cargo test --locked -p mm2_app --test network shoved_seat` 2 passed (the recipe bites on the seeded run). Full gates below.

Status: implemented candidate; not independently checked. Evidence level: synthetic loopback, three real processes. No production code changed; F25-C stays open (other impairment legs, wider matrix).
