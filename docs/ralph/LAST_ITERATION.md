# Last implementation iteration

- Task ID and title: F04-A.3 — banger runtime slice: the
  dormant → active → settled state machine on Avian for
  pathset-stamped, name-bound props, using the binding verified by
  F04-A.2 (`banger-bind`). Threshold semantics remain UNK-22 —
  the activation estimate is a documented provisional stand-in, not a
  verified original rule.
- Starting commit and resulting commits: started at
  `3970b91e73a4debff99525ba1dd8b614c9ae2d08` (clean tree, branch
  `ralph/night`; F04-A.2 had just passed external gates + review,
  verdict pass, no blocking findings). Result = one feature commit
  plus this handoff note.
- Why this slice: named by the plan as the next F04-A leg. The binding
  is verified (WLD-16) and the runtime class shape is recovered (R4),
  so the state machine's first half could be implemented against real
  data without inventing thresholds.
- What changed:
  - `crates/mm2_game/src/banger.rs` (new) + `lib.rs` exports — the
    physics/rendering-independent contract: `BangerPhase`
    (Dormant/Active/Settled mirroring `dgUnhitBangerInstance`/
    `dgBangerActive`/`dgHitBangerInstance`), `Banger` component,
    `BangerDefinition` (authored Mass/Friction/Elasticity/
    ImpulseLimit2/Size/CG distilled from `BangerData`, anomalies
    collapsed to documented defaults), `BangerStateChanged` message
    (ObjectId + generation + tick + phase + `BangerCause`),
    `BangerPool` (`max_active = 32` — the R4-recovered pool size).
    `activates_on` implements the provisional UNK-22 rule; non-finite
    physicals sanitize rather than poison the solver.
  - `crates/mm2_app/src/banger.rs` (new) + `lib.rs` — `BangerDefs`
    VFS cache (`tune/banger/<name>.dgbangerdata`, lowercased, missing
    = unbound, decode failure counted per name); `banger_bundle` —
    one entity carrying collider + authored physicals (`Mass`,
    `CenterOfMass` from `CG`, `Friction`, `Restitution`),
    `CollisionEventsEnabled`, dormant `Banger`, `ObjectIdentity`,
    `AuthorityRole`, `SessionEntity`; `activate_bangers` and
    `settle_bangers` FixedLast systems, both authority-gated
    (`Predicted` drains edges but never transitions).
  - `crates/mm2_app/src/contracts.rs` — new shared
    `deepest_contact` helper (deepest manifold contact → point,
    normal, nonneg pre-solver approach speed); `collect_impacts`
    refactored onto it so "the hit" means the same thing to impact
    reporting and banger activation.
  - `crates/mm2_app/src/city.rs` — `spawn_banger_prop`: a bound,
    collidable stamp becomes one dynamic-capable root with mesh parts
    as children (no split render/collider pair, no duplicate
    collider). `stamp_pathset` resolves each name through
    `BangerDefs`; bound+collider → banger entity, else the unchanged
    `spawn_prop`. `PathsetStampReport` gains `bangers` /
    `banger_failed`; `CityReport` gains `pathset_bangers` /
    `pathset_banger_failed`; `load_city`/`spawn_event_pathsets` take
    `&mut Session` (id minting + authority role). `MIRROR_Z` is now
    `pub(crate)` for the CG mirror.
  - `crates/mm2_app/src/session.rs` — `load_session_world` passes the
    session through.
  - `crates/mm2_app/src/main.rs` / `smoke.rs` — `BangerStateChanged`
    message, `BangerPool` resource, activate/settle chained in
    `FixedLast` after `collect_impacts`; smoke records a `bng=` field
    (`Nd/Na/Ns` dormant/active/settled) when bound props exist.
  - `crates/mm2_app/examples/drive_probe.rs`,
    `tests/event.rs`, `tests/import_pipeline.rs` — signature updates
    for the threaded `&mut Session`.
  - `docs/research/banger.md` — new "Implemented runtime slice
    (F04-A.3 — provisional)" section; UNK-22 list narrowed to the
    genuinely remaining unknowns.
  - `docs/original-rules.md` — new DSN-10 (the provisional policy);
    UNK-22 updated to name the implemented stand-in.
- Implemented semantics (provisional where marked):
  - Dormant → Active: raw `CollisionStart` edge with deepest-contact
    approach speed > 0; estimate = `severity × striker_mass`
    (striker's `ComputedMass`, 1 kg fallback) must exceed
    `ImpulseLimit2` — provisional (UNK-22). One edge, one transition:
    `RigidBody` → dynamic (inserted — Avian 0.7 `RigidBody` is
    immutable), impulse leaves the prop at the striker's approach
    speed plus a `Size`-derived solid-cuboid spin kick.
  - Pool: at 32 actives the oldest `(activated_tick, slot)` settles
    with `BangerCause::Reclaimed` — reclaim order provisional.
  - Active → Settled: Avian `Sleeping` → velocities zeroed,
    `RigidBody` → static at rest pose; terminal for the session
    (teardown restamps). The recovered `Timer` despawn is not
    implemented — the slice settles instead.
  - `0` limit activates on any approach (most retail props);
    `≈1e30` never activates (bridge gates, monuments — a monument
    cannot go dynamic from a generic collision).
- Deferred deliberately: BREAK-chunk fragments (F04-B), `BirthRule`
  particles, `AudioId`/`Flash`/`TexNumber` effects, decals, prop-rule
  channel stamping, `Timer` despawn, replication (F26). `NumParts` is
  carried on `BangerDefinition` but not acted on.
- Tests added:
  - `crates/mm2_app/tests/banger.rs` (7, real Avian physics):
    hard impact activates once then settles (identity/generation/tick
    stamps, static-again body, finite pose, knocked loose); monument
    limit never activates; midrange limit discriminates (dormant vs
    active); `Predicted` sessions never transition; ×1 pool reclaims
    oldest-first with `Reclaimed`; bound pathset names stamp as
    dormant bangers (single entity + mesh child, no duplicate
    collider, teardown despawns root+children); malformed/missing
    records fall back to static props with `banger_failed` counted.
  - `crates/mm2_game/src/banger.rs` unit tests (5): limit
    discrimination incl. zero/at-limit/huge, authored-junk
    sanitization, finite angular kick.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo fmt --all -- --check` — PASS (after auto-format).
  - `cargo clippy --locked --workspace --all-targets --all-features
    -- -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, all 32 suites,
    0 failures (incl. new 7 banger integration + 5 contract tests).
  - `mm2 --mm2-path <retail> --city sf --headless --frames 120` —
    `status=pass`: `pathset props stamped stamped=925 bangers=925
    failed=0 capped=0`, `0 banger-decode-failed`,
    `bng=925d/0a/0s` — every stamped prop bound dormant, matching the
    F04-A.2 audit (30/30 names).
  - `mm2 --mm2-path <retail> --city london --headless --frames 60` —
    `status=pass`: 1188/1188 bound, `bng=1188d/0a/0s`.
  - No activation was exercised on retail (the hold driver does not
    strike a prop); activation evidence is the synthetic physics
    tests, not original-content proof.
- Acceptance IDs satisfied / still open:
  - F04-AC01–AC06: all remain OPEN. This slice implements the state
    machine's first half only — fragments, authored-effect fidelity,
    the Timer despawn, prop-rule channel and replication have no
    evidence. No F04 completeness is claimed.
  - DSN-10: added — the provisional runtime policy is explicitly an
    implementation choice, not an original-behavior claim.
  - UNK-22: still open — `ImpulseLimit2` comparison quantity, exact
    transition conditions, `INST_BANGER` acquisition, pool reclaim
    order, `Timer` despawn, BirthRule timing, fallback selection.
- Scope decisions recorded: `Settled` is terminal for the session
  (no recovered `dgHitBangerInstance`→anything transition); settle
  replaces the recovered `Timer` despawn for now. Bound props without
  collision stamp unbound — unreachable today since prop colliders
  derive from the same triangles as the render parts, kept as a
  defensive guard. `*_ai.inst` stop-sign supplements stamp through
  INST, not the pathset path, so they remain static this slice
  (INST placements do not resolve `BangerDefs` — pathset is the
  verified world channel).
- Stock data/GPU/audio/network limitations: retail numbers above are
  code-path validation through the VFS (binding + counts), not
  original-behaviour proof. No GPU capture needed — no rendering
  change (the same mesh parts render, now parented). No audio or
  network code exists.
- Unresolved blockers or discovered regressions: none known.
- Next smallest useful action: F04-B — BREAK<NN> fragment spawning on
  activation (`NumParts` consumption, effects timing stays UNK-22);
  prop-rule stamping research (UNK-21 — the second verified banger
  channel); decal stamping; F13-A; F09-C.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
