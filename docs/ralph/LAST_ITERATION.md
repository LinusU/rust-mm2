# Last implementation iteration

- Task ID and title: F01-B — stable ids, telemetry, surface/impact/
  result contracts and local authority boundaries (the reconciled
  plan's next slice after F01-A passed external check+review).
- Starting commit and resulting commit: started at
  `2e45611f5d466db2b6d6d87d50187d3b6fc6e147` (clean tree, branch
  `ralph/night`); result = this commit.
- Production code changed:
  - `crates/mm2_game/src/ids.rs` (new): `PlayerId`, `ObjectId`
    (generation + slot, `ObjectId::WORLD` sentinel for unmarked static
    geometry), `ObjectIdentity` ECS component, `Player`/`PlayerControl`,
    `AuthorityRole` (`Authority`/`Predicted`) — all distinct from Bevy
    `Entity`.
  - `crates/mm2_game/src/surface.rs` (new): `SurfaceMaterial`
    (`Authored(u16)` carries original codes uninterpreted — meaning is
    unknown per the rules ledger, plus `Unspecified`) and `SurfaceState`
    (material + traction/visual fields kept separate from physics
    friction).
  - `crates/mm2_game/src/impact.rs` (new): `ImpactEvent` (id,
    generation, tick, participants, point/normal, severity, surface),
    `ImpactPolicy` (min severity + per-tick bound + pair cooldown),
    `ImpactDedup` — one event per physical impact, never raw contact
    spam.
  - `crates/mm2_game/src/telemetry.rs` (new): `VehicleTelemetry`
    read-only snapshot (object id, tick, authority, pose/velocities,
    rpm/engine_load/gear/reverse/shifting, per-wheel telemetry, damage),
    `WheelTelemetry` (grounded, contact point/normal, slip angle,
    traction demand, surface), `DamageSignals` accumulator.
  - `crates/mm2_game/src/result.rs` (new): `ResultId` (generation +
    participant + event + sequence), `SessionResult`, `ResultLedger`
    with `DuplicateResult` rejection.
  - `crates/mm2_game/src/session.rs`: `Session` mints generation-scoped
    `ObjectId`/`PlayerId`/`ResultId` (`mint_*`) and reports
    `authority_role()` (`Local`/`Host` → `Authority`, `Remote` →
    `Predicted`).
  - `crates/mm2_game/src/config.rs`: `SessionAuthority::is_authoritative`;
    `EventRef`/`EventTableKind` now `Hash`/`Eq` for result identity.
  - `crates/mm2_vehicle/src/vehicle.rs`: `VehicleInput.forced_gear`
    (explicit gear override; out-of-range clamps to top gear, `None`
    resumes the automatic selector), `VehicleState.engine_load`,
    `WheelState.contact_entity`.
  - `crates/mm2_vehicle/src/systems.rs`: forced-gear pinning, engine
    load = delivered drive force / demandable max (0 airborne), ray-hit
    entity recorded per wheel.
  - `crates/mm2_vehicle/src/lib.rs`: `vehicle_bundle` carries
    `CollisionEventsEnabled` so chassis contacts reach the impact
    pipeline.
  - `crates/mm2_app/src/contracts.rs` (new): `collect_impacts` —
    `CollisionStart` edges + `Collisions` manifolds → bounded
    deduplicated `ImpactEvent`s; participants resolve `ObjectIdentity`
    (collider→body→`WORLD`); severity = pre-solver normal approach
    speed; the passive side's `SurfaceMaterial` becomes the event
    surface; impacts accumulate `DamageSignals`; `ImpactFilter` exposes
    `emitted`/`dropped` counters. `publish_vehicle_telemetry` — inserts
    the read-only snapshot per fixed step while `Playing`. Both run in
    `FixedLast`; the reader drains while paused so no stale burst
    flushes on resume.
  - `crates/mm2_app/src/main.rs`: registers `ImpactEvent` +
    `ImpactFilter`, schedules both systems; player vehicle and trailer
    spawn with `ObjectIdentity`/`Player`/`AuthorityRole`/`DamageSignals`;
    HUD reads `VehicleTelemetry` instead of mutable sim state.
  - `crates/mm2_app/src/smoke.rs`: same wiring in the headless harness;
    smoke records now report `impacts=`/`dropped=`.
- Documentation changed:
  - `docs/architecture.md`: dependency diagram draws the
    `mm2_game → mm2_formats` edge; `mm2_game` bullet names the
    contracts; new "Contract bridge" section; vehicle bullets cover
    `forced_gear`/`engine_load`/`contact_entity`/collision events.
  - `docs/ralph/PLAN.md`: F01-A promoted to `checked` (external review
    pass), F01-B marked `implemented` (candidate), next slice F01-C.
- Key behavior verified on this machine (macOS arm64, retail install):
  - `mm2 --dev-world --headless` → `status=pass`, `updates=600
    ticks=1198 impacts=1 dropped=0` peak 27.9 m/s, moved 157 m, exit 0.
  - `mm2 --mm2-path <retail> --city sf --headless` → `status=pass`,
    `updates=600 ticks=1198 impacts=3 dropped=0` peak 37.2 m/s, moved
    172 m — 1171 rooms / 3763 props via VFS, stock `vpbug` on real city
    collision, exit 0.
- Tests added/changed and why: `crates/mm2_game/tests/contracts.rs` —
  9 contract units (id minting/generation isolation, authority role,
  surface carrying authored codes, impact dedup/policy, telemetry
  defaults, result identity + ledger dedup). `crates/mm2_app/tests/
  contracts.rs` — 3 real-physics integration tests: stamped telemetry
  each fixed step (wheel surfaces resolve through the marked ground),
  roof-first 4 m chassis drop → bounded impacts with stable
  participants/`WORLD` sentinel/authored surface/damage accumulation,
  paused session publishes nothing and drains contact edges.
  `crates/mm2_vehicle/tests/drive.rs` gained `forced_gear_pins_the_
  gearbox` and `engine_load_tracks_delivered_demand`.
- Commands actually run and results:
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features --
    -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, 21 test binaries/doc-test
    groups, 0 failures.
- Acceptance IDs satisfied / still open:
  - F01-AC04 (advanced): `ResultId`/`SessionResult`/`ResultLedger` give
    results a stable deduplicated identity tied to session generation +
    `EventRef`; no race system produces results yet — contract only.
  - F01-AC05 (advanced): `AuthorityRole` is stamped per entity and in
    every telemetry snapshot; `Host`/`Remote` remain unexercised (no
    networking exists) — not a multiplayer claim.
  - F01-AC03 (advanced): consumers now read `VehicleTelemetry` and
    `ImpactEvent` contracts stamped with the session clock — the input
    side of event equivalence remains F01-C.
  - F01-AC01/AC02/AC06: unchanged from F01-A.
- Evidence files: none committed; no captures made.
- Stock data/GPU/audio/network limitations: unchanged — no GPU frame
  captured this iteration; no audio or networking code exists. The
  marked `SurfaceMaterial::Authored(3)` in tests is a synthetic fixture,
  not an original-content claim; authored surface-code meanings stay
  unknown per the rules ledger.
- Unresolved blockers or discovered regressions: none. Deliberate
  carry-overs: `ImpactEvent` is a Bevy message (two-frame lifetime) so
  consumers must read per update; `ImpactDedup::clear` is not yet called
  on teardown (no `Unloading` path exists — F01-C);
  `SurfaceState.traction` is a placeholder field pending F06 surface
  rules; `Countdown`/`Results`/`Paused` phases remain unreachable in the
  app (documented F01-A carry-over).
- Next smallest useful action: F01-C — session start/quit/restart
  integration in the real app (schedule `despawn_session_entities`,
  drive `Unloading`) plus plugin-attach documentation.

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
