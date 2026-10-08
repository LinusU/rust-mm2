# Last iteration — F25-C.2: cross-process pose agreement after a shove (iteration 6 of the run)

Selection: the previous checkpoint (`4301bb7`) passed gates and review with no blocking findings, so no repair was owed. Its named gap was "pose agreement across processes after the shove is not asserted". F25-AC02 (collision converges, no permanently diverged replicas) is the highest-value open networking item inside F25-C.

Change:
- Smoke record: `seats=<id>:<x>,<z>/…` (every wire seat the process holds, own car + copies; lobby runs only) and a trailing `fix<n>` cell in `net=`.
- `--ram` is role-split (`input::ram_drive`): on the authority it turns at 5 m/s while the target is off the nose and parks after its first strike at speed (`RamStrike`), so the field comes to rest; a joined client keeps the old law. Applying the new law on the client broke `a_driven_collision…` (3/6, then 0/8), because client inputs reach the authority late.
- `net_drive::a_shoved_seat_converges_across_three_processes`: the *host* rams (deterministic: no prediction, no latency), alice/bob parked, joined in fixed order; control run gives the grid. Asserts both cars left their slots (>1.5 m) and every seat both clients hold agrees within 1.0 m. The host's own record is not usable (remote seats are gone when it prints), so the authority is seen through the copies.
- **Real defect found and fixed:** in 2 of 26 runs alice's predicted car ended 7 m from the authority's copy permanently (own seat only took snapshots on a reset epoch; the local sim shoved it harder because the host car is a kinematic copy). New `netdrive::SettleWatch` reseats a car that sits at rest (<0.3 m/s both sides) >0.75 m from the authority's copy for 60 snaps. First bound 1.5 m failed a loaded full-suite run (1.43 m off), so 0.75 m. Documented in `docs/research/net.md`.
- Tests: 2 `SettleWatch` units, `net_app::a_predicted_car_at_rest_apart_from_the_authority_is_reseated` (loopback socket), 2 `ram` units, the process leg. Test waits use `until_within(90 s)`: the 15 s per-line wait timed out under the loaded full run.

Gates (foreground): `cargo fmt --all -- --check`; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`; `cargo test --locked --workspace` all pass: fmt exit 0, clippy exit 0, test exit 0 with 58 `test result: ok`, none failed.

Flake notes: process leg 10/10 idle, 3/3 inside full `network` runs; `a_driven_collision…` 8/8 after the role split.

Not covered / open: divergence while moving is unbounded; the kinematic-copy shove asymmetry itself; collision under an impairment recipe; trailer/extra-wheel/reset legs at process level; dev world, loopback only; no rendered observation. F25-C stays open. Status: implemented candidate; not independently checked.
