# Last implementation iteration

- Task ID and title: F11-B.1 repair — wire the reset/teleport segment
  break into production (external review's single blocking finding on
  the previous F11-B.1 candidate).
- Starting commit and resulting commit: started at
  `07551a032dcff010d87d0ac53ff176a3c1f5babb` (clean tree, branch
  `ralph/night`); result = this commit.
- Review finding being repaired: `RaceProgress::break_segment` had no
  production caller. The only production teleport path is `reset_input`
  (R key, not session-phase gated) → `ResetVehicle` →
  `mm2_vehicle::vehicle_reset`, which writes `Position` directly. A
  mid-race reset therefore produced a swept segment from the pre-reset
  pose to the spawn point that consumed — and could finish — every
  checkpoint it crossed: exactly the spec's "reset near finish" AC02
  edge. The prior test called `break_segment()` by hand, so the suite
  could not see the gap.
- Root cause: `mm2_vehicle` cannot depend on `mm2_game`, so the reset
  system cannot call `break_segment` itself, and nobody bridged the
  two. The reviewer's suggested fix (drain `ResetVehicle` in the race
  system or a `FixedLast` system before it) was considered and
  rejected on timing grounds: `reset_input`/`vehicle_reset` ordering
  inside `Update` is unconstrained, so a drain can consume the message
  a frame *before* the teleport lands (break spent early), and Bevy
  messages expire after two frames while `FixedLast` legitimately runs
  zero steps in a frame at update rates above the 120 Hz fixed clock
  (break missed entirely).
- Design classification: the fix is *implementation choice* — an
  engine-internal teleport signal, no original-behavior claim.
- Production code changed:
  - `crates/mm2_vehicle/src/vehicle.rs` (new `Teleported` marker
    component): stamped by the reset path on each entity it teleports;
    persists until a swept-segment consumer claims it, inert otherwise.
  - `crates/mm2_vehicle/src/systems.rs`: `vehicle_reset` inserts
    `Teleported` in the same pass that writes `Position`/`Transform` —
    the marker and the teleport are atomic, so the re-anchor can
    neither land early nor be missed; covers every current and future
    `ResetVehicle` producer (R key now, fall-recovery/netcode later).
  - `crates/mm2_app/src/race.rs` (new
    `reanchor_teleported_participants`): consumes `Teleported` on
    `RaceProgress` entities → `break_segment()` + remove marker. Runs
    in `FixedLast` chained before `advance_race`, in every session
    phase — a reset during pause or countdown still lands.
  - `crates/mm2_app/src/main.rs`: `FixedLast` is now one deterministic
    chain `collect_impacts → publish_vehicle_telemetry →
    reanchor_teleported_participants → advance_race` (also resolves the
    review's unordered-telemetry nit).
  - `crates/mm2_app/src/input.rs` (nit): the race input-lock now also
    checks `!is_stale`, so a dead `RaceState` can't lock controls.
  - `crates/mm2_app/src/session.rs` (nit): `drive_session` doc bullet
    now lists `Countdown` among intent-handled phases.
  - `docs/architecture.md`: `FixedLast` row updated for the chain and
    the `Teleported` consumer.
- Tests added/changed and why:
  - `crates/mm2_app/tests/race.rs`: the harness now registers
    `ResetVehicle`, runs the real `mm2_vehicle::systems::vehicle_reset`
    in `Update`, and chains `reanchor_teleported_participants` before
    `advance_race` exactly like `main.rs`. Participants carry the
    vehicle components `vehicle_reset`'s query needs (no `RigidBody`,
    so physics leaves them alone between the tests' segment writes);
    `set_position` now writes `Transform` alongside `Position` because
    adding `Rotation` put participants under avian's
    `transform_to_position`, which otherwise reverts a manual
    `Position` write to the stale `GlobalTransform` a step later.
  - New `vehicle_reset_breaks_the_swept_segment` (AC02 reset leg):
    writes a real `ResetVehicle` past two ordered checkpoints —
    asserts teleport applied, zero cleared, still `Racing`, empty
    ledger, marker consumed — then verifies normal driving clears
    again. Verified to FAIL on the old wiring (cleared 2).
  - New `reset_while_paused_cannot_sweep_checkpoints`: reset under
    `Paused`, resume — the marker persists through the freeze and the
    jump never counts. Also verified to FAIL on the old wiring.
  - `crates/mm2_vehicle/tests/drive.rs`: `reset_teleports_and_clears_
    motion` now asserts the `Teleported` marker lands.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo test -p mm2_app --test race` — 15/15 ok.
  - `cargo test -p mm2_vehicle --test drive` — 15/15 ok.
  - Negative check: with the `Teleported` insert removed, both new
    tests fail (cleared 2 — the exact reported defect); restored.
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features --
    -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, 25 test result groups, 0
    failures.
- Acceptance IDs satisfied / still open: F11-AC02's teleport/reset leg
  is now exercised through the production path (candidate-level —
  synthetic positions, no authored event). AC03/AC04 unchanged
  (candidate-level). Still open: AC05 (event-prop scoping — F11-B.2)
  and all real-event evidence: no authored event has driven a
  `RaceDefinition` yet.
- Evidence files: none committed; no captures made.
- Stock data/GPU/audio/network limitations: unchanged — no authored
  event run; GPU/audio/network unexercised this iteration.
- Unresolved blockers or discovered regressions: none. One noted
  boundary: `vehicle_self_right` adjusts `Position.y` continuously to
  drop the car onto the surface below — a small correction, not a
  teleport; it does not stamp `Teleported`. If a future review shows a
  self-right hop crossing a trigger it shouldn't, the marker is the
  mechanism to reuse.
- Next smallest useful action: F11-B.2 — `mm2_content` producer
  turning a `CatalogEvent`'s records into a `RaceDefinition`
  (`RecordContent` must retain or re-read parsed waypoint/start rows),
  `load_session_world` event-mode wiring (`Ready → Countdown`,
  `RaceState`/`RaceProgress` insertion, start-slot spawn), checkpoint
  entities/markers, event-prop session scope (AC05).

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
