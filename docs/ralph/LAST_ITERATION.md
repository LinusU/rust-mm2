# Last iteration — F28-B.3: ambient cars brake for a cable car (iteration 22 of the new run)

Selection: F28-B.2 (`4ad754d`) passed gates and review with no blocking findings, so no repair was owed. The reviewer's main non-blocking gap was that tram/ambient interaction was one-sided: the tram stopped for an ambient car, but ambient cars drove straight through a stopped or crossing tram. The original's probe is "any other AI vehicle", so the symmetric rule is original-derived and needed no new disassembly.

Production change:
- `mm2_app::traffic::RoadObstacle { half_length }` + `sense_points`: the body's origin and a point 2 m inside its nose and tail along local +Z (the corridor sense reads centres; 2 m = a car's own half-length). `drive_ambient` queries them (`Without<AmbientCar>, Without<Player>`) and extends its `blockers` list, which feeds both the follow-corridor and the junction-box occupancy test.
- `mm2_app::cablecar::spawn_cable_cars` attaches `RoadObstacle { half_length: nose }` to each tram.
- Docs: `docs/research/specials.md` (ambient braking paragraph replaces the "not done" note), UNK-44 row, module docs.

Tests: `tests/traffic.rs` — `ambient_cars_queue_behind_a_cable_car_on_the_lane` (follower from 2 m, 15 m/s toward a body 20 m along the lane, both facings: held, centre never within 6.36 m of the origin) and `an_unmarked_body_on_the_lane_is_not_sensed` (same body without the component is run through, so the first test is the component's doing); `tests/cablecar.rs` spawn asserts every tram carries a `RoadObstacle` equal to its nose.

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` pass; `cargo test --locked --workspace` exit 0, 58 `test result: ok`. `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail cargo test -p mm2_app --test app -- cablecar traffic::`: 54 passed.

Not covered / open: no windowed run or retail traffic in motion; tram tail assumed symmetric to the nose (unmeasured); head-on deadlock of a tram and an ambient car on shared rails resolves only by the ambient stuck recovery; audio; networked sessions; `+0x40` init gate; rail side; cross-check of other AIMAP-created objects. Status: implemented candidate; not independently checked.
