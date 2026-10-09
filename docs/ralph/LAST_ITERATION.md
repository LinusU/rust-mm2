# Last iteration — flake measurement of the reset process legs (iteration 10 of the new run)

Selection: iteration 9's commit (`2c2056b`) passed gates and review with no blocking findings. The reviewer's first verification gap was that the wall-clock reset process tests (clean leg asserts exactly one ask; impaired leg depends on the retry) had a flake rate nobody had measured. Report 6 item 4 makes that the operator's recurring complaint, and report 7 items 1–11 are all implemented candidates, so I measured it rather than start a larger unrelated slice.

No production change. Findings:
- `net_drive::a_scheduled_reset_crosses_two_processes_and_comes_back` + `…_on_an_impaired_link`: 15/15 consecutive isolated runs passed (~13 s per run; the reviewer's ">60 s" was the whole-suite wall time, not these two).
- Under contention — the whole `net_drive::` module (19 process tests running concurrently, ~86 s) — 3/3 runs passed, so the clean leg's `requests_sent == 1` timing assumption held with the CPU saturated.
- Recorded in `docs/research/net.md` under F25-C.5. A flake below ~1/18 is not excluded; this is a bound, not a proof.

Gates (foreground): `cargo fmt --all -- --check`, `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`, `cargo test --locked --workspace` all exit 0 (fmt pass; clippy no warnings; tests 2946 passed, 0 failed).

Not verified: loopback, dev world only; other impairment recipes against a reset; trailer rig reset at process level; reset-while-moving divergence; nothing rendered.

Status: docs-only candidate; not independently checked. Next: remaining F25-B/F25-C legs, the report 6 follow-up 1 two-process breakdown leg (needs a race session in a headless process plus a way to destroy the car), or F27-C multi-client contested pickup.
