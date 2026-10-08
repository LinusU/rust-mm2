# Last iteration — refuse the wire stop conditions on a lone headless run (iteration 6 of the new run)

Selection: iteration 5's commit (`4a0a9c9`) passed gates and review with no blocking findings. The review left one non-blocking nit: `--until-impacts`, `--until-peer-left` and `--deadline` were accepted with `--headless` but silently ignored on the single-process `headless_smoke` path (only `budget.frames` was passed), so a lone run reported a pass having waited on nothing. Report 7 items 1–11 are all implemented candidates; report 6 follow-up 2 is already reconciled (ledger DSN-11); follow-up 1's two-process leg needs a retail Blitz/Checkpoint wreck that the dev-world process harness cannot produce (no authored damage), and follow-up 3 is F26-A scope — those stay open.

Change: `main.rs` — when a stop condition is armed and the run is neither `--join` nor `--host`, print a `status=fail` record ("need --join or --host: a single-process run only counts --frames") and exit with the Fail code, rather than running `--frames` and passing. Records without an armed condition are unchanged.

Test: `net_edge::a_lone_headless_run_refuses_wire_stop_conditions` runs the real `mm2` binary with `--until-impacts 1` and with `--deadline 5` on the dev world and asserts the failed record and exit code.

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` pass; `cargo test --locked --workspace` exit 0.

Not verified: no windowed, retail or GPU run (CLI validation only). Residual from iteration 4 stands (rate/growth limit not applied to race rows). The review's other note (peer-left from a leave/rejoin scenario) is unchanged: a link `Closed` is terminal by design.

Status: candidate; not independently checked. Next iteration: the F25-B/F25-C remainders in the plan (breakdown two-process leg once a retail event-level harness exists; collision under impairment).
