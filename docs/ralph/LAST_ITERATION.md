# Last iteration — F28-C.5: opponent routes in the retail water audit (iteration 3 of the run)

Selection: the previous checkpoint (`7cc40e8`) passed gates and review with no blocking findings, so no repair was owed. Its first verification gap was that `.opp` opponent routes were not in the water audit; this iteration closes that gap for the anchors.

Change (test only, no production code): `tests/recovery.rs` `retail_no_authored_race_point_stands_in_deadly_water` also builds the production `opponent_roster` for every ready event at both difficulties and tests every wired route anchor against the city's `CityWater`, with non-empty route/anchor denominators asserted. Retail (`MM2_RETAIL=<retail> cargo test --locked -p mm2_app --test app retail_no_authored -- --nocapture`): sf 246 routes / 4591 anchors, london 271 / 7216, 0 in deadly water; start/gate counts unchanged (732 / 614).

Control: a throwaway probe (reverted) dropped each anchor to 1 m under the surface; SF 0 and London 6 anchors lie over a water footprint (bridge deck, y −0.15, surface −3.8), so the y-aware test does discriminate and those six are correctly dry.

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` pass; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed (MM2_RETAIL unset there; the retail audit was run separately with it set, as above).

Not covered / open: route segments between anchors, ambient/BAI lanes, Crash Course lesson routes/points, windowed tunnel/bridge drive; the 5 m column cap remains a designed stand-in. Status: implemented candidate; not independently checked.
