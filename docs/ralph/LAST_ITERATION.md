# Last iteration — F25-C.1: a driven collision across real processes (iteration 5 of the run)

Selection: the previous checkpoint (`fc2dd06`) passed gates and review with no blocking findings, so no repair was owed. Operator report 6 directs the loop to networking (F25-B remainder, F25-C, F26-A); the last few iterations had all been F28-C water audits. F25-AC02 (player-player collision converges) had only in-process legs: the process-level matrix drove cars side by side, so `imp` was spawn-landing noise. This slice puts a real driven collision across three OS processes.

Change:
- New `--ram` evidence driver (`input::ram_drive` + pure `ram_input`; `smoke::Driver::Ram`, record `driver=ram`; wired in the windowed app and the headless smoke, `send_drive_input` ordered after it). Full throttle capped at 11 m/s, steers at the nearest other vehicle, drives straight first while a target is inside 14 m and more than 0.45 rad off the nose, neutral outside a live unlocked session. Evidence driver, not gameplay; documented in the README.
- `tests/net_drive.rs::a_driven_collision_replicates_across_three_processes`: host and one client `--parked` (victims), the other client `--ram`; an all-parked control run beside it. Asserts the control has no impacts anywhere, the ram client reached 5 m/s and recorded a local impact, the host published `imp` rows, and the uninvolved client applied replicated rows. `docs/research/net.md` records the measured ranges and what is deliberately not asserted.
- 4 unit tests (`tests/input.rs`, `ram::`).

Design note: the first two pursuit recipes (plain bearing steering; then a stand-off without a speed cap) missed the neighbour in roughly half the runs (full-lock orbit; then a turning circle too wide at 30 m/s). The final recipe was run 8 times in a row, 8/8 passes. The victim varies with the lobby's connect order, so the test asserts only order-independent facts.

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` pass; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed. The new leg runs unconditionally (dev world, no install needed), ~8.5 s.

Not covered / open: pose agreement across processes after the shove (records carry only each process's own car); collision under an impairment recipe; dev world only (no authored damage, `dsyn0`); loopback only; no rendered observation. F25-C stays open. Status: implemented candidate; not independently checked.
