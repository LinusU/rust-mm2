# Last implementation iteration

- Task ID and title: F04-C.3 — the hull-clearance fidelity question:
  whether the snag-avoidance hull reshape prevents tall vehicles from
  striking short props, and what shape the original struck them with.
- Starting commit and resulting commits: started at
  `46b496862ea503435cbe705cd37b4a7093bb22f9` (clean tree, branch
  `ralph/night`; F03-B.4 passed external gates + review). A separate
  nit-fix commit `53f167b` addressed the prior review's non-blocking
  doc/counting remarks first.
- Why this slice: the plan's first-listed candidate. The open question
  from F04-C.1/C.2 — a `vpbus`/`vpddbus` ghosted through cone and
  bollard rows that stop or activate under `vpbug` — is now answered:
  the reshape was responsible, and the original's own collision model
  supplies the fix.
- The finding: mm2hook's struct layout shows prop collision is
  bound-vs-bound — `dgBangerData` carries `Bound`/`ColliderId` and
  every car carries a `phBound`. The authored bound is therefore the
  shape that should strike props. Measured: the `vpddbus` authored
  bound floor is ≈0.27–0.43 m at the nose/tail lower verts — under an
  `sp_cone_f` top (0.85 m) along the whole underside — while
  `clear_underside` lifts the world hull's underside ≥0.25 m plus
  25°/15° ramps (≈1.1 m mid-body on the 5.19 m wheelbase), so
  kerb-height props pass beneath the raised belly untouched.
- What landed (code):
  - `mm2_vehicle::config` — `VehicleConfig.striker_points`: the
    unmodified authored bound, validated like `collider_points`.
  - `mm2_vehicle::vehicle` — `StrikeBound(Collider)` component: a
    prop-strike surface for shape-overlap queries only, never a world
    collider; `vehicle_bundle` builds it from `striker_points` and
    falls back to the chassis collider itself.
  - `mm2_content::convert` — keeps the bound verbatim into
    `striker_points` beside the `clear_underside`-reshaped
    `collider_points`; `assemble` paint/variant overrides inherit it
    with the rest of the collision geometry.
  - `mm2_app::banger::activate_bangers` — a second strike source beside
    `CollisionStart` edges: each moving `StrikeBound` runs
    `SpatialQuery::shape_intersections` and any dormant banger it
    overlaps is a candidate. Severity = the bound's surface velocity at
    the prop's centre (`v + ω×r`), the estimate and `ImpulseLimit2`
    gate are unchanged, the impulse lever is the prop's upwind face by
    `Size`, and the same `claimed` set dedupes against same-tick
    contact edges and between strikers. Stationary strikers query
    nothing.
  - Tests (all in `tests/banger.rs`, real physics): an overlap
    activates a prop the world hull demonstrably clears (striker keeps
    speed/altitude — no contact); a parked overlap activates nothing; a
    monument-limit prop stays dormant under a moving overlap; the
    production bundle carries `StrikeBound` from `striker_points` and
    falls back to the chassis shape.
- Retail evidence gathered (install
  `/Users/linus/coding/rust-mm2/retail`, `fnv1a64:e91e6cd4b2ae30d9`,
  `target/debug/mm2`, `--bot` driver):
  - `vpddbus --spawn=-1641.6,36.7,410,0 --frames 800` (SF): the run
    that previously passed the `sp_cone_f` cluster with zero contacts →
    `bng_ev=1a/0s/0b`, `impacts=10`, `status=pass`.
  - `vpbus --spawn=0.4,5.5,-720,0 --frames 1500` (London): through the
    `sp_bollard_black_l` road row it previously ghosted →
    `bng_ev=1a/1s/0b`.
  - `vpbug --spawn=0.4,5.5,-720,0 --frames 1500` (London): →
    `bng_ev=2a/2s/0b` (was `1a/1s` — the bound catches a second
    bollard the contact hull squeezed past).
- What this proves / does not prove:
  - Proves: the reshape was the mechanism (documented zero-contact
    ghosting); the authored bound is the right strike shape per the
    original's bound-vs-bound model; activation through overlap
    restores the documented strikes without touching the world hull or
    per-vehicle handling.
  - Does not prove: that an overlap strike imparts the same impulse as
    a manifold contact (the overlap supplies approach speed and an
    upwind-face lever, not a manifold — UNK-22 territory); whether the
    original also lets wheels/bumpers strike props a bound clears; or
    whether `ImpulseLimit2` semantics are right at all.
- Classification: the bound-vs-bound prop model is an original
  requirement (mm2hook layout + authored bound geometry); striking
  dormant bangers through a `StrikeBound` overlap while the reshaped
  hull stays the only world collider is an implementation choice; the
  estimate/gate reuse stays provisional (UNK-22).
- Commands actually run and results:
  - `cargo test -p mm2_app --test banger` — 16/16 pass incl. the 4 new
    tests.
  - `mm2 --mm2-path <retail> --city sf --car vpddbus
    --spawn=-1641.6,36.7,410,0 --bot --headless --frames 800`,
    `--city london --car vpbus --spawn=0.4,5.5,-720,0 --bot --headless
    --frames 1500`, same spawn `--car vpbug` — records above.
  - Gates at the candidate commit: `cargo fmt --all -- --check`
    pass; `cargo clippy --workspace --all-targets --all-features
    -- -D warnings` pass, exit 0; `cargo test --workspace` pass —
    all suites, 0 failures.
- Acceptance IDs satisfied / still open: F04-AC01–AC06 unchanged —
  this slice strengthens the AC01 evidence base (both sides of the
  threshold on real content, now including high-floor strikers) but
  claims no new AC.
- Deferred deliberately: wheel/bumper strike question (unknown),
  overlap-vs-contact impulse equivalence (UNK-22), `ImpulseLimit2`
  semantics, F13-A/F09-C.
- Stock data/GPU/audio/network limitations: retail evidence is
  headless physics records through the VFS; no original-executable
  comparison exists.
- Unresolved blockers or discovered regressions: none known. The
  `vpbug` London run now records `2a/2s` instead of `1a/1s` — a
  deliberate consequence (the bound is wider than the contact hull),
  documented rather than a defect.
- Next smallest useful action: F13-A or F09-C.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
