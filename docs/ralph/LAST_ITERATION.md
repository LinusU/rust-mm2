# Last iteration — F25-C.3: the shove on an impaired link (iteration 7 of the run)

Selection: checkpoint `9fc43a1` passed gates and review with no blocking findings, so no repair was owed beyond the reviewer's doc nit (net_app test comment said 1.5 m; the bound is 0.75 m — fixed). Next-highest F25-C item that is deterministic enough to land: AC02 (collision converges) under AC03 (impairment) — the first collision run on anything but a clean link.

Change:
- `net_drive::run_shove_trio` gains `impair: Option<Impair>`; when set, both clients join through a seeded `ImpairProxy` (`0xAC02`) armed on both directions 400 ms after `Start` (same one-shot-verb reasoning as the matrix cells), dropped before awaiting `event=left`.
- The clean leg's assertions moved into `assert_shove_converged(control_bob, alice, bob)`, shared by both legs.
- New `a_shoved_seat_converges_across_three_processes_on_an_impaired_link`: `combined` recipe (40 ms + 30 ms jitter, 5 % loss, 10 % dup, 10 % reorder); same verdicts as the clean leg. Evidence the recipe bit: alice's `snap…/≈1.9k x` stale drops vs 0 clean.
- `docs/research/net.md` and PLAN row updated.

Results (foreground): impaired leg 6/6 in isolation, clean leg 3/3 (one ran the settle bound for real: `fix1`, `resets=1`). Gates: `cargo fmt --all -- --check` exit 0; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` exit 0; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed.

Not covered / open: other seven recipe cells against a collision; divergence while moving is unbounded; kinematic-copy shove asymmetry; trailer/extra-wheel/reset legs at process level; dev world, loopback only, no rendered observation. F25-C stays open. Status: implemented candidate; not independently checked.
