# Last implementation iteration

- Task ID and title: F01-B repair — fix the external review's blocking
  finding on the F01-B candidate (`6cb833b`): `collect_impacts` applied
  the pair dedup cooldown before the severity/manifold filters, so a
  sub-threshold `CollisionStart` consumed the 24-tick window without
  reporting and a genuine re-impact inside it was discarded silently —
  no `ImpactEvent`, no `dropped` count, no `DamageSignals`.
- Starting commit and resulting commit: started at
  `6cb833b49cd41605041c19dde125cb61b4b95e97` (clean tree, branch
  `ralph/night`); result = this commit.
- Root cause: `ImpactDedup::allow` both checks and records
  `last_emitted`. Calling it before `collisions.get`/manifold/severity
  filtering meant every edge that passed the cooldown — including ones
  about to be filtered out — reset the window. Ordering bug, not a test
  or data problem.
- Production code changed:
  - `crates/mm2_app/src/contracts.rs`: `collect_impacts` now calls
    `dedup.allow(c1, c2, tick)` only after the contact resolves and
    passes `min_severity` — the cooldown starts on a *reportable*
    contact. A filtered-out touch no longer consumes the window. Pairs
    that qualify but lose the per-tick bound still record (they are
    counted in `dropped`, and the flap collapse still applies).
  - `crates/mm2_app/src/main.rs`: spawned trailers now also carry
    `DamageSignals` — the review noted trailer-side impacts only
    accumulated on the other participant.
  - `crates/mm2_game/src/impact.rs`: `pair_cooldown_ticks` doc now says
    "after a reportable contact" (was "after reporting");
    `min_severity` documents the edge-triggered limitation — a contact
    that begins sub-threshold and escalates while staying in contact
    never emits (no new edge).
  - `crates/mm2_game/src/telemetry.rs`, `crates/mm2_vehicle/src/
    vehicle.rs`: `engine_load` docs corrected — a gear change does not
    dip the ratio (the shift-torque factor scales the delivered and
    available sides equally); only traction-control clamping dips it.
    The prior wording claimed a shift dip that cannot occur.
- Tests added/changed and why:
  - `crates/mm2_app/tests/contracts.rs`: `test_app` refactored onto
    `test_app_with_policy` so a test can widen `pair_cooldown_ticks`.
    New regression test `a_quiet_touch_does_not_suppress_a_real_
    reimpact`: a dynamic box spawned resting on the marked ground
    produces a sub-threshold first edge (asserts nothing emitted),
    then is separated 0.6 m and re-impacts at ~3.4 m/s inside a
    240-tick window — asserts the event is reported and damage
    accumulates. Verified discriminating: it FAILS on the pre-fix
    ordering (0 events for the pair) and passes on the fix.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo test -p mm2_app --test contracts` — 4/4 pass (the new test
    failed as expected before the fix).
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features --
    -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, 21 test binaries/doc-test
    groups, 0 failures.
- Acceptance IDs satisfied / still open: unchanged from the F01-B
  candidate — this repair restores the F01 req.5 impact-bounding claim
  the review found undermined (a real impact can no longer produce zero
  events and zero damage). F01-AC03/04/05 remain contract-level
  advances; AC01/02/06 unchanged from F01-A.
- Evidence files: none committed; no captures made.
- Stock data/GPU/audio/network limitations: unchanged — no GPU frame
  captured this iteration; no audio or networking code exists; authored
  surface-code meanings remain unknown per the rules ledger; real-city
  geometry still reports `SurfaceMaterial::Unspecified` (F06).
- Unresolved blockers or discovered regressions: none. Carry-overs
  unchanged: `ImpactDedup::clear` unscheduled (no `Unloading` path —
  F01-C); `last_emitted` grows per seen pair within a session (bounded
  by session lifetime, disclosed); `ResultLedger`,
  `PlayerControl::Ai`, Host/Remote authority paths have no consumers
  yet; `Countdown`/`Results`/`Paused` unreachable in the app; the
  edge-triggered escalation gap is now documented rather than fixed
  (a sustained-contact severity watcher is a real design choice, left
  for F01-C+).
- Next smallest useful action: F01-C — session start/quit/restart
  integration in the real app (schedule `despawn_session_entities`,
  drive `Unloading`, call `ImpactDedup::clear` on teardown) plus
  plugin-attach documentation.

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
