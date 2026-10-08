# Last iteration — repair of the iteration-10 external gate failure (iteration 11 of the run)

Selection: the external verify of `75d875f` failed on `net_drive::a_driven_collision_replicates_across_three_processes` (bob `impacts_applied == 0`; 115 passed / 1 failed in the `network` suite). Iteration 10 had seen the same test flake and left it unrepaired; a failing gate comes before feature work.

Root cause: test timing, not replication. Bob's frame budget (1000 frames ≈ 2000 ticks) is not a clock. The ram is a pursuit through the wire's input latency, so its first strike lands anywhere from host tick ~700 to ~1850 (temporary prints on `publish` and `RemoteSnaps::push`, reverted). Under load the host advances fewer ticks per client frame (bob applied 365–457 snaps vs ~970 idle), so a late strike's snap was published after bob had exited. Every connected client applied the rows it was sent.

Change (test-only, `crates/mm2_app/tests/net_drive.rs`): `run_collision_trio` gives alice/bob 2800/2500 frames (the shove legs' budgets; control run uses the same) and waits on the quiet clients with `until_within(90 s)` — the 15 s per-line `WAIT` otherwise trips at the larger budget on a loaded machine. Comment records why.

Evidence: before — 1 of 6 parallel copies failed (reproduced); after — 10 of 10 pass with 10 copies in parallel (~93 s each) and alone (13.6 s). Gates (foreground): `cargo fmt --all -- --check` PASS; `cargo clippy --workspace --all-targets --all-features -- -D warnings` PASS; `cargo test --workspace` exit 0.

Status: implemented candidate; not independently checked. Synthetic loopback evidence only. F23 (subtitles, pad rebinding, render scale, wheel/FFB audit) and F25-C remainders are unchanged from the previous handoff.
