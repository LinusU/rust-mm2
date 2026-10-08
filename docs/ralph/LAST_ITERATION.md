# Last iteration — F21-B.15: a retried lesson hands back a repaired stock vehicle (iteration 3 of the new run)

Selection: the F21-B.14 checkpoint (`11fc7dd`) passed gates and review; no failing gate to repair. Its review listed "VehicleDamage reset on retry not covered (synthetic harness has `SelectedCar.def = None`)". No production change — evidence on the existing restart path.
Change: `tests/lesson_launch.rs` gains `event_app_with_car` (`event_app` delegates to it) and the opt-in (`MM2_RETAIL`, "not run" otherwise) test `a_retried_retail_lesson_hands_back_a_repaired_stock_vehicle`: for each school's required vehicle (london `vpcab`, sf `vpbullet`, loaded through `load_vehicle`, launched on the real city PSDL, Crash Course row 0) drive `VehicleDamage` past `max_damage`, detach every `VehicleBreaks` part, restart, and require a new entity at generation 2 with total 0, `Intact`, health 1.0 and nothing detached. 2/2 checked, 3 parts shed in total.
Mutation check: temporarily pre-damaging the car at its spawn site in `session.rs` made the test fail for both cities ("damage carried over"); the mutation was reverted (diff is test-only).
Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` rc 0; `cargo test --locked --workspace` rc 0, 58 result blocks ok, 0 failed; with `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail` all 9 `lesson_launch` tests pass.
Status: implemented candidate, not independently checked. Limits: damage is set on the component, not delivered through the impact stream; only row 0 per city; not public-CI (needs retail); start pose still compared with the first launch, not the authored start row. Still open for F21-B: instruction/voice flow, family evaluators (UNK-35), lead cars/cops, lesson-only reward credit. F21-B and F21-C stay open.

---

