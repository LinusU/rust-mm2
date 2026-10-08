# Last iteration — F28-B.5: cable cars in networked Cruise (iteration 1 of the next run)

Selection: the previous checkpoint (`482ef96`) passed gates and review, so no repair was owed. F28's spec req 5 / AC05 (authoritative state, late join) was the largest hole left in the plan's F28 open list: networked sessions spawned no cable cars at all, because their stops follow one process's signal clock and the cars on its rails.

Production change:
- `mm2_app::cablecar::fields_cable_cars`: local → run; `Host` → run in the free-roam Cruise that replicates ambient traffic (`fields_ambient_traffic`); `Remote` → never. `session.rs` passes it as `eligible`.
- `mm2_app::worldtraffic`: `CAR_CABLE` (`SnapCar.state = 2`). `publish_traffic` chains the host's `CableCar` rows first (lowest ids, never trimmed by the 64-row bound); `TrafficLedger::collect` takes a wire `state` (`drive_state()` for ambient cars). `apply_row` accepts state 2 and `spawn_cable_copy` loads `va_cablecar_f` once per replica (`MovableModels`) and spawns a kinematic `TrafficCopy` (model + collider, no `CableCar`); a missing model is cached, warned once and counted `unresolved`. No protocol version bump (old peers refuse state 2 counted).
- Evidence plumbing: `NetDriveReport::{cable_sent, cable_live}` and a smoke ` cable=sent<n>,live<n>` cell after `cars=` (absent when zero).
- Docs: `docs/research/specials.md` (policy + limits), `docs/research/net.md` (cable rows), PLAN F28-A/F28-B.

Tests: `tests/traffic.rs` `a_client_copies_the_hosts_cable_car_and_a_late_joiner_finds_it_in_place` (collector → codec → client; copy has Kinematic/Collider/model, no `CableCar`; same entity follows; a fresh client joins in place; copy retires after the TTL) and `a_client_without_the_cable_model_counts_the_rows_and_spawns_nothing`; `tests/cablecar.rs` `only_the_authority_runs_the_cable_cars`. The test's host collector mirrors `publish_traffic`'s chain (the system itself needs a `HostLink`); the real system is exercised by the two-process leg.

Original-data evidence: `MM2_RETAIL=<retail> cargo test -p mm2_app --test network two_retail_processes_replicate_the_hosts_traffic` — host `cable=sent1176,live0`, client `cable=sent0,live4` (4 retail sf cars, real loopback, headless; extended the test to assert `>=4` sent / `4` live).

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` pass (one `clone_on_copy` in the new test fixed first); `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed.

Not covered / open: cable-car audio, the `+0x40` init gate, rail side, head-on deadlock; a city with no ambient roster publishes no frame (retail has one); no impairment cell for cable rows; no windowed capture of a copy. Status: implemented candidate; not independently checked.
