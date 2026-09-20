# Last implementation iteration

- Task ID and title: F04-B.1 — BREAK<NN> fragment spawning on
  activation: a bound pathset prop whose PKG carries authored
  `BREAK<NN>` chunks shatters into per-piece fragment bodies instead
  of tipping over. Threshold and fragment-timing semantics remain
  UNK-22 — the break fires on the same provisional activation edge,
  which is a documented implementation choice, not verified original
  behavior.
- Starting commit and resulting commits: started at
  `dd29ea1cafd2705095c938019e90d11a0b65a158` (clean tree, branch
  `ralph/night`; F04-A.3 had just passed external gates + review,
  verdict pass, no blocking findings, two non-blocking nits both
  addressed here). Result = one feature commit plus this handoff
  note.
- Why this slice: named by the plan as the next F04 leg. The binding
  (WLD-16), the `NumParts` ↔ BREAK-index audit (WLD-15: 0 mismatches
  on retail) and the dormant→active→settled machine were all in
  place; fragments were the remaining named piece of F04-AC03.
- What changed:
  - `crates/mm2_game/src/banger.rs` — `BangerPhase::Broken` added to
    the contract (terminal like `Settled`; the placement remains as
    the break event's identity). Module doc corrected: the verified
    INST rule is that ordinary INST names do not bind while
    `*_ai.inst` binds `sp_stop_f` — the old wording overstated it.
    `num_parts` doc now says fragment spawning is chunk-driven.
  - `crates/mm2_app/src/banger.rs` — `FragmentPiece` (index,
    `<name>_break<NN>` record or the parent's def, render parts,
    convex-hull collider) and the `BangerPieces` component.
    `banger_bundle` now takes a `Banger` so fragments spawn already
    `Active`; the `CG` mirror is a shared helper. `activate_bangers`
    gained a `claim_slot` pool gate that accounts pending same-tick
    spawns and returns `false` at a degenerate `max_active = 0`
    (closes the F04-A.3 review nit where zero capacity could be
    exceeded). `break_banger` performs the transition: parent phase
    `Broken`, velocities zeroed, collider/body/mesh children removed,
    one `BangerStateChanged` (`Broken`) emitted, then one dynamic
    fragment entity per collidable piece — own `ObjectId`, same
    session owner + authority role, impact velocity plus its own
    CG-lever spin kick, pool-bounded. A placement with pieces but no
    collidable piece takes the ordinary activation path.
  - `crates/mm2_app/src/city.rs` — `pkg_to_parts` splits `BREAK<NN>`
    chunks out of the intact LOD set into `FragmentModel`s (authored
    file order → deterministic fragment sequence); the dormant prop
    no longer draws/collides break chunks as part of its unified
    surface. `stamp_pathset` resolves each piece's
    `<name>_break<NN>` record through `BangerDefs` (parent def
    fallback) and stamps `BangerPieces` via `spawn_banger_prop`.
    `PathsetStampReport.pieces` counts collidable pieces;
    `CityReport.pathset_pieces` and both stamp log lines report it.
  - `crates/mm2_app/src/session.rs` — overlay stamp log line gains
    `bangers`/`pieces`.
  - `crates/mm2_app/src/smoke.rs` — `bng=` gains a `/Nb` broken
    bucket.
  - `docs/research/banger.md` — runtime slice section renamed
    F04-A/B, `Broken` policy documented, deferred list narrowed,
    UNK-22 keeps the genuinely open semantics.
  - `docs/original-rules.md` — DSN-10 updated with the break path;
    UNK-22 gains fragment-timing.
- Implemented semantics (provisional where marked):
  - Dormant → Broken: same qualifying `CollisionStart` edge as
    activation (one `ImpulseLimit2` gate for both — UNK-22). The
    parent keeps entity/identity/generation; exactly one
    `BangerStateChanged` fires for it — the parent never also
    reports `Active` (F04-AC03 shape).
  - Fragments: one dynamic body per collidable `BREAK<NN>` chunk, in
    authored file order, minted `ObjectId`s, `Banger` phase `Active`
    from spawn so they settle through the shared path and count
    against the ×32 pool — a break at capacity reclaims oldest-first
    or skips pieces (pool = 1 test bounds it; `max_active = 0`
    spawns nothing).
  - Piece records: `<name>_break<NN>` `dgbangerdata` when authored
    (254 exist on retail), parent def otherwise — documented
    fallback for authored gaps.
  - Piece colliders: convex hull over the chunk's triangles (dynamic
    debris does not need the prop's authored concavity; degenerate
    pieces fall back to trimesh or count as non-collidable).
- Deferred deliberately: `BirthRule` particles, `AudioId`/`Flash`/
  `TexNumber` effects, decals, prop-rule channel stamping, `Timer`
  despawn, replication (F26), original-content activation evidence.
- Tests added (`crates/mm2_app/tests/banger.rs`, real Avian):
  a breakable prop shatters into its authored pieces — exactly one
  `Broken` event, no `Active` event for the parent, pieces mint own
  `ObjectId`s with colliders + render children + finite poses and
  settle; pool = 1 bounds fragment spawns; a piece set with no
  collidable piece tips over instead (ordinary activation);
  breakable pathset stamping extracts authored pieces with
  `<name>_break<NN>` defs and parent-def fallback; teardown removes
  parent + fragments. All prior F04-A tests still pass.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features
    -- -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, all 32 suites,
    0 failures (11 banger integration + 5 contract tests).
  - `mm2 --mm2-path <retail> --city sf --headless --frames 60` —
    `status=pass`: `stamped=925 bangers=925 pieces=3092`,
    `bng=925d/0a/0s/0b`, 0 decode failures — every stamped banger
    carries its authored pieces, all dormant (hold driver).
  - `mm2 --mm2-path <retail> --city london --headless --frames 60` —
    `status=pass`: `stamped=1188 bangers=1188 pieces=2710`,
    `bng=1188d/0a/0s/0b`.
  - `mm2 --mm2-path <retail> --city london --event circuit:0
    --headless --frames 60` — `status=pass`: overlay
    `stamped=181 bangers=181 pieces=0`, `bng=1369d/0a/0s/0b`.
    `pieces=0` is authored data, not a wiring gap: the stamped
    `sp_barricadeconcr_f` PKG carries no BREAK chunks and its record
    says `NumParts 0` (verified via `mm2-inspect pkg`/`banger`) —
    concrete barricades tip over, they do not shatter.
  - No activation or break was exercised on retail content (the hold
    driver never strikes a prop); break evidence is the synthetic
    physics tests plus real-data piece extraction, not
    original-behavior proof.
- Acceptance IDs satisfied / still open:
  - F04-AC01/02/04/05/06: unchanged from F04-A.3 — partial synthetic
    evidence, still open.
  - F04-AC03: partially evidenced — authored pieces spawn with one
    logical break event in synthetic tests; original break timing,
    `BirthRule`/audio/flash effects and real-content activation are
    unproven, so the AC stays open.
  - DSN-10: updated — the break path is explicitly implementation
    choice, not original behavior.
  - UNK-22: still open — `ImpulseLimit2` quantity, fragment timing,
    `NumParts` runtime role, reclaim order, `Timer`, `BirthRule`
    timing, `INST_BANGER` acquisition, fallback selection.
- Scope decisions recorded: `Broken` is terminal like `Settled` (no
  recovered transition out of it; teardown/restamp restores the
  placement). Fragments spawn `Active` directly rather than passing
  through a dormant phase — they only exist post-impact. The parent
  keeps its `ObjectId` as the break event's identity; pieces mint
  fresh ids. Pieces with no collider never spawn as bodies — and a
  prop whose every piece lacks collision tips over instead of
  breaking into nothing.
- Stock data/GPU/audio/network limitations: piece counts are
  code-path validation through the VFS on real PKGs, not visual or
  original-behavior proof. No GPU capture needed — no rendering
  change claimed (fragment parts reuse the same mesh pipeline). No
  audio or network code exists.
- Unresolved blockers or discovered regressions: none known.
- Next smallest useful action: F04-C — threshold/repeated-collision/
  reset and resource-budget validation; prop-rule stamping research
  (UNK-21 — the second verified banger channel); `BirthRule` effect
  research; F13-A; F09-C.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
