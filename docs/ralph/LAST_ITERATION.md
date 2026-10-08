# Last iteration — F28-C.6: Crash Course lessons in the retail water audit (iteration 4 of the run)

Selection: the previous checkpoint (`9fa21a6`) passed gates and review with no blocking findings, so no repair was owed. Its open list named Crash Course lesson points/routes as not audited; this iteration closes that part of the F28-AC04 audit.

Change (test only, no production code): `tests/recovery.rs` `retail_no_authored_race_point_stands_in_deadly_water` additionally builds every Crash Course lesson through production `lesson_race_setup` at both difficulties and checks each leg's start slots and gates, the lead-car route anchors and the police posts against the city's `CityWater`. Non-empty lesson/leg/point denominators are asserted; a lesson that fails to build is added to the findings list instead of being skipped.

Retail (`MM2_RETAIL=<retail> cargo test --locked -p mm2_app --test app retail_no_authored -- --nocapture`): sf 26 lesson setups / 28 legs / 218 points / 228 lead-car anchors / 40 police posts; london 26 / 30 / 256 / 324 / 38; 0 in deadly water. Earlier race/route counts unchanged (732 / 614 points; 246 / 271 routes).

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` pass; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed (MM2_RETAIL unset there, so the retail test is a skip in that run; the retail audit was run separately with it set, as above).

Not covered / open: route segments between anchors, ambient/BAI lanes, windowed tunnel/bridge drive; the 5 m column cap remains a designed stand-in. Status: implemented candidate; not independently checked.
