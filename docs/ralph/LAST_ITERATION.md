# Last iteration — F28-C.4: retail water-vs-route audit (iteration 2 of the run)

Selection: the previous checkpoint (`2ea98b0`) passed gates and review, so no repair was owed. Its review flagged the retail cable-car leg as not run by the gate; I ran it here (`MM2_RETAIL=<retail> cargo test --locked -p mm2_app --test network two_retail_processes_replicate_the_hosts_traffic -- --nocapture`; the review's claim that the test lives under `--test net_drive` is wrong, `net_drive.rs` is a module of the `network` target): host `cable=sent1152,live0`, client `cable=sent0,live4`, ok. This reproduces the previous figures (sent count differs run to run).

Task: F28 AC04's open item, "retail boundaries/`.water` rooms against real routes".

Production change: `mm2_app::water::CityWater::is_deadly` now bounds a room's column `WATER_DEPTH` = 5 m under its bound. The new retail audit found London's Thames tunnel (rooms 670–672, floors y −13…−11) under water rooms 347–349 (surface −4.0): the 2D point-in-perimeter test counted a Blitz gate there (and any car driving the tunnel) as drowned. Designed stand-in for the original's room occupancy; DSN-34 and `docs/research/environment.md` corrected.

Tests: `tests/recovery.rs` `retail_no_authored_race_point_stands_in_deadly_water` (MM2_RETAIL-gated; SF 45 water rooms, 64 definitions, 732 points; London 23 rooms, 64 definitions, 614 points; 0 in deadly water; wet-grid control > 0) — failed before the fix with 2 London points, passes after. `water::tests::a_tunnel_under_the_footprint_is_not_water` plus the deep-point assertion moved from −10 to −8.

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` pass; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed (MM2_RETAIL unset in that run; the retail audit and the cable-car test were run separately with it set).

Not covered / open: `.opp` opponent routes, Crash Course points and ambient/BAI lanes are not in the audit; no windowed drive through the tunnel; cable-car audio and the other F28 items in PLAN. Status: implemented candidate; not independently checked.
