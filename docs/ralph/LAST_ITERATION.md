# Last iteration — impaired-link leg of the remote breakdown (iteration 12 of the new run)

Selection: iteration 11 (`eb62a4154`) passed gates and review with no blocking findings. Report 6 follow-up 2 turned out to be landed already (`cd7ad71`, DSN-11 text and `the_field_keeps_racing_through_the_wire_deferral`), so the open networking items were F27-C and F25-B's impaired breakdown leg; I took the latter — the smallest and the one the previous review named as uncovered ("no impaired link").

Change (candidate, not independently checked; test-only):
- `net_drive`: the breakdown run is now `run_breakdown(retail, Option<Impair>)` returning a `BreakdownRun` (client record, host `destroyed`/`repaired` log lines, host record, proxy counters), with `assert_breakdown` holding the shared assertions (client `stop=repaired`, exactly one dead episode `imp=…/1d`, `dsyn>0`; host interval 595–610 ticks). The clean test is unchanged in what it asserts.
- New `a_remote_drivers_breakdown_survives_an_impaired_link` (`MM2_RETAIL`-gated): the client reaches the host through a seeded `ImpairProxy` with the matrix's lossy recipe (30 % loss, 10 % dup, 10 % reorder, 40±30 ms, both directions, armed 400 ms after `Start`); both directions must show impairment.
- `docs/research/net.md` (report 6 follow-up 1 bullet) records it.

Evidence (retail london `checkpoint:0`, loopback, headless; `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail`): both breakdown legs 3/3 passes (~12 s per pair). Impaired: host `destroyed` tick 900 → `repaired` tick 1499 (599 ticks), client `stop=repaired imp=1i/1r/1d`, host `in…/36x` shows the duplicated inputs the relay injected.

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` exit 0; `cargo test --locked --workspace` exit 0, 58 `test result: ok` lines, none failed (without `MM2_RETAIL`; the retail legs were run separately above).

Not verified: the destruction is the knob's, not a driven wreck; no second client watching the wrecked seat's copy; smoke plume observed (`ptx`) not asserted; nothing rendered; Blitz not run; loopback only; the impaired leg uses one seed and one recipe, not the full matrix.

Status: implemented candidate; not independently checked. Next: F27-C multi-client contested pickup (needs a second driven client), or report 6 follow-up 3 (F26-A actors) audit.
