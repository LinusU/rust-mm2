# Last iteration — F28-A.6 / F28-B.1: the cable car runs (iteration 20 of the new run)

Selection: F28-A.5 (`592f66a`) passed gates and review with no blocking findings, so no repair was owed. Its own handoff named the next step: recover the cable cars' motion so F28-B (a runtime) could start. Disassembly of `Midtown2.exe` gave enough to build the whole actor, so this iteration does both halves.

Recovered (verified_original, `docs/research/specials.md` § The cable car, rule ledger UNK-44):
- next-road rule at a road end (`0x53fcb0`): 1 tram road → turn round, 2 → the other, 4 → straight across, else none. Retail SF has only 1/2/4 → two closed circuits (26 and 16 legs).
- speed controller (`0x540560`): cruise 15 m/s, look-ahead 25 m, own accel `1.5 + 2·rand/32768`, constant-rate braking to 0.25 m short of a closed road end or 2.5 m behind an obstacle, no braking between roads; road-end answer by rule (`NeverStop` yes, `TrafficLight` green, `StopSign` queue).

Production change:
- `mm2_formats::bai`: `TramLeg`/`TramHop`/`TramCircuit`/`TramPlan`, `Bai::tram_start`/`tram_curve`/`tram_hop`/`tram_circuit`/`tram_plan`. Retail finding that shaped it: all 21 tram roads store the right curve end→start and the left start→end, so curves are oriented by geometry; the side each direction uses is a documented *choice* (right-hand running).
- `mm2_game::cablecar`: `CableRoute` (closed arc-length table, smooth junction hops, heading chord) and `CableMotion` (the controller). `Junctions::gate_approach` (the lane `gate` now delegates to it).
- `mm2_app::cablecar`: `spawn_cable_cars` (local sessions only; London none), `drive_cable_cars` (FixedLast after `drive_movers`; gates through `AmbientTraffic` when present, other cable cars on the circuit are obstacles); wired in `session.rs`, `main.rs`, `smoke.rs`.
- `mm2-inspect specials`: cars/circuits/unrouted per city (unrouted fails `--strict`); the cable car moves from "unresolved" to a new "partially reproduced" list stating what is missing.

Tests: bai unit tests (hop rules, circuits, plan, curve orientation), 12 `cablecar` unit tests (every controller branch; mutation-checked the red-after-cleared latch), `gate_approach == gate` over 800 ticks per rule, `mm2_app/tests/cablecar.rs` (plan, ECS drive continuity/freeze/following, unreadable/absent/ineligible spawn; `MM2_RETAIL`: all four cars round both circuits, 84/84 red stops, production spawn fields 4 bodies with 0 missing textures, London none).
Windowed evidence (local, not in git): `mm2 --city sf --frames` with ambient traffic: cars accelerate to 15 m/s, stop 0.3 m short on red, go on green; screenshot shows the model on the rails facing its direction of travel.

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` pass; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed. Retail-gated legs run separately with `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail`: 9 cablecar tests pass.

Not covered / open: audio (`cablecar*`, `streetcable`), the player and ambient cars as obstacles (a body in the way is shoved), networked sessions (no cars), the init gate `+0x40`, AI-bubble behaviour, the `+0x4c` nose offset sign, the rail side per direction, StopSign/`+0x160`/`+0x162` branches (unused on retail tram roads). No independent visual/audio review.
Status: implemented candidate; F28-A AC01 advanced (cable car actor exists), not complete.
