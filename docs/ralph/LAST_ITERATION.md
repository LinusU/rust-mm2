# Last iteration — report 6 follow-up 1's two-process breakdown leg (iteration 11 of the new run)

Selection: iteration 10's docs-only commit (`db54c89`) passed gates and review with no blocking findings. Report 7 items 1–11 are all implemented candidates, so I took the oldest open networking item: report 6 follow-up 1 ("prove it with a two-process leg"), which earlier iterations had left needing a race session in a headless process plus a way to destroy a car.

Change (candidate, not independently checked):
- `--wreck-at <tick> [--wreck-seat <id>]` — quarantined `DevOverrides` pair, record-ineligible. `damage::dev_wreck_at` (authority only, one-shot, waits for its seat) calls `VehicleDamage::wreck()` and writes the `DamageEvent` an impact would, so `resolve_disabled` takes the mode's real arm for that participant. Inert on a predicted client.
- A hosting launch used to refuse any dev override (`advertise`). `HostLink::open` now splits only the authority-local pair (`host_local_dev`) off the advertised config and stamps it back at `Started`; every other override (e.g. `--traction`) still refuses — `mm2_host_flag_gates_are_named_exits` caught my first, too-wide version of this.
- `--until-repaired <n>` stop condition (`stop=repaired`); `DamageReport.dead` and the record's `imp=` cell gains `/<n>d` (dead-engine episodes, only when non-zero); the breakdown repair log line now carries `tick`.
- Tests: 4 in-process (`tests/damage.rs`: remote seat breaks down and repairs once; no seat wrecks the local car; waits for a seat that has not joined; inert on a predicted client), 1 `StopWatch` unit, 1 eligibility leg, 1 `net_app` host-dev split (including a `--traction` pin still refused), and the process leg `net_drive::a_remote_drivers_breakdown_crosses_two_processes` (`MM2_RETAIL`-gated).

Evidence (retail london `checkpoint:0`, loopback, headless, 3/3 passes ~7.5 s each; the test skips without `MM2_RETAIL`): host `--wreck-at 900 --wreck-seat 1`, parked client. Client record `stop=repaired imp=1i/1r/1d dsyn1468 ptx=129e/111x`; host log `remote vehicle destroyed` tick 900, `repaired after its breakdown` tick 1499 (599 ticks, asserted 595–610) — the five-second breakdown, not an instant reset. Recorded in `docs/research/net.md` (report 6 follow-up 1 bullet).

Gates (foreground): `cargo fmt --all -- --check`, `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` exit 0; `cargo test --locked --workspace` exit 0, 2953 passed, 0 failed (without `MM2_RETAIL`; the retail leg run separately above).

Not verified: the destruction is the knob's, not a driven wreck; no impaired link; no second client watching the wrecked seat's copy; the client's smoke plume is observed (`ptx`) but not asserted; nothing rendered; Blitz not run (Checkpoint only).

Status: implemented candidate; not independently checked. Next: F27-C multi-client contested pickup, the F25-B impaired-link breakdown leg, or report 6 follow-up 2 (reconcile `field_races` with the networked results deferral).
