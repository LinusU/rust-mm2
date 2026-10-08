# Last iteration — F28-B.4: measure the cable car's tail (iteration 23 of the new run)

Selection: F28-B.3 (`5808351`) passed gates and review with no blocking findings, so no repair was owed. Its review named the tram tail extent as assumed symmetric to the nose and unmeasured; that is the one gap closable here with the retail install, so this iteration measures it.

Production change:
- `mm2_app::cablecar::CableCar` gains `tail` (`-aabb.min.z` of the model's collider, clamped 1..12 m, falling back to the nose); `spawn_cable_cars` attaches `RoadObstacle { nose, tail }`.
- `mm2_app::traffic::RoadObstacle { half_length }` becomes `{ nose, tail }`; `sense_points` insets each end by 2 m independently.
- Docs: `docs/research/specials.md` ambient-braking paragraph states the measurement; PLAN F28-A row records F28-B.4.

Evidence: `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail cargo test -p mm2_app --test app -- retail_the_production --nocapture` prints nose 4.360179, tail 4.360175, half-width 1.3201735 for all four SF cars (symmetric — the earlier assumption held); the test now asserts |tail − nose| < 0.05. New `tests/traffic.rs::an_obstacles_sense_points_follow_its_nose_and_tail` (nose 6/tail 3 → points at +4/−1 m; a stub shorter than the inset senses only its origin). The spawn test asserts `RoadObstacle` equals `(nose, tail)` per car.

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` pass; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed.

Not covered / open: no windowed run or retail traffic in motion; head-on tram/ambient deadlock resolves only by ambient stuck recovery; audio; networked sessions; `+0x40` init gate; rail side; cross-check of other AIMAP-created objects (UNK-44 stays open). Status: implemented candidate; not independently checked.
