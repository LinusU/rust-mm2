# Last implementation iteration

- Task ID and title: F04-C.1 repair — external review found the
  recorded London activation+settle evidence irreproducible and
  physically impossible as written; this iteration replaced it with
  verified commands and corrected every doc that carried the claim.
- Starting commit and resulting commits: started at
  `e19821ac1cf4433d73c4871eeb614644620a12d8` (clean tree, branch
  `ralph/night`; F04-C.1 verify gates passed but review verdict fail
  on the false-evidence blocker). Result = one docs-only correction
  commit; no code changed.
- Root cause of the blocker: the recorded command `vpbug
  --spawn 762,0.5,-424,0` drives −Z ~7 m from spawn into static
  geometry at (761, −431) — it never reaches the `sp_bollard_black_l`
  at (759.7, −427), and even on contact a 1000 kg vpbug would need
  >10.86 m/s in ~3.5 m from rest (observed peak 8.9 m/s). The claim
  in the commit message, PLAN.md, LAST_ITERATION.md and
  docs/research/banger.md was false recorded evidence.
- Investigation path (why earlier probes missed): stamped positions
  were re-derived by expanding `props.pathset` per
  `stamped_transforms` (kind-0 = per-vertex, kind-2 = per-segment at
  `spacing` + end cap) and cross-checked against BAI road
  centre-lines. Probe runs showed `vpbus` driving *through*
  `sp_bollard_black_l` rows and `sp_cone_l` clusters with zero
  contacts — the vehicle collider's underside is deliberately raised
  (`clear_underside`: ≥0.25 m floor + ~25° approach / ~15° breakover
  ramps) and wheels are raycast, not colliders, so props shorter
  than the local hull floor pass underneath untouched. A 1.36 m
  bollard reaches `vpbug`'s hull but not the bus's ~1 m nose floor.
  `ImpulseLimit2` never comes into play without a contact.
- Corrected retail evidence (this machine, macOS arm64,
  `target/debug/mm2 --mm2-path /Users/linus/coding/rust-mm2/retail`):
  - London activation + settle, leg 1: `--city london --car vpbug
    --spawn 0.4,5.5,-720,0 --headless --frames 1500` →
    `status=pass impacts=8 peak=17.2m/s moved=69m
    final=(-15,5.0,-788) bng=1187d/0a/1s/0b bng_ev=1a/1s/0b`.
    Run twice, bit-identical. The `sp_bollard_black_l` row is
    stamped across road454 at (−8.3…4.9, 5.0, −742…−744).
  - London activation + settle, leg 2: `--city london --car vpbug
    --spawn 112.3,5.5,-745,0 --headless --frames 1500` →
    `bng=1187d/0a/1s/0b bng_ev=1a/1s/0b` — the sibling row across
    road467. Same end record, independent placement.
  - SF activation (no settle): `--city sf --car vpbug
    --spawn=-1641.6,36.7,410,0 --headless --frames 800` →
    `bng=924d/1a/0s/0b bng_ev=1a/0s/0b`; still `1a/0s` at 5000
    ticks — the cone (limit 8500, struck ~32 m/s) never sleeps on
    the sloped streets.
  - SF below-threshold block: `--spawn=-1641.6,36.7,389,0` → the
    bug reached the same cone at 7.7 m/s; `impacts=1`, no
    transition, and the dormant cone stopped the car at `moved=7m`
    — the static-collider side of the threshold on a second prop
    family.
  - Unchanged legs from F04-C.1 (reviewer-verified, not re-run this
    iteration): SF `sp_wrongwayfw` break `bng_ev=0a/0s/1b` and the
    ~3 m/s graze → nothing.
- What this proves / does not prove:
  - Proves: activation AND settle both fire on real authored content
    — a bound prop went Dormant→Active on impact and later
    Active→Settled through Avian sleep, on flat London streets, on
    two independent placements; an SF prop activated too. AC01's
    both-sides (graze → nothing, strike → transition) now holds on
    two prop families (`sp_wrongwayfw`, `sp_cone_f`).
  - Does not prove: original timing/threshold semantics (UNK-22);
    settle behaviour on slopes (knocked props stay `Active` on SF
    hills — same open observation as the freeway fragments);
    visuals (headless only); whether the original lets wheels/low
    bumpers strike kerb-height props — under the raised hull a tall
    vehicle can never touch <~1 m props (open fidelity question,
    recorded in banger.md).
- Commands actually run and results:
  - All probe/verification runs above via the debug `mm2` binary at
    `e19821a`; no code or test changes were needed — the defect was
    in the record, not the implementation.
  - `cargo fmt --all -- --check`, `cargo clippy --locked --workspace
    --all-targets --all-features -- -D warnings`, `cargo test
    --locked --workspace` — unchanged tree vs the verified commit;
    re-run before commit, results below in handoff.
- Acceptance IDs satisfied / still open:
  - F04-AC01: activation+settle on original content now genuinely
    evidenced (London ×2, SF activation); both-sides threshold on
    two prop families. Still open pending review.
  - F04-AC02/AC04/AC06: unchanged (synthetic coverage).
  - F04-AC03: break fragments on retail data verified in F04-C.1;
    timing/effects unproven — open.
  - F04-AC05: unchanged (session-level synthetic restart coverage).
- Deferred deliberately: `BirthRule` particles, `AudioId`/`Flash`/
  `TexNumber` effects, decals, prop-rule stamping (UNK-21), `Timer`
  despawn, replication (F26), GPU capture of a break, fragment/prop
  settle on slopes, hull-vs-low-prop fidelity review.
- Scope decisions recorded: none new — the repair is documentation
  only; the `--spawn`/`bng_ev` tooling and the state machine are
  unchanged.
- Stock data/GPU/audio/network limitations: all evidence is headless
  physics through the VFS on the real install (read-only). No GPU,
  audio, or network capability exercised.
- Unresolved blockers or discovered regressions: none known. The
  slope-settle observation (props stay `Active` on sloped ground) is
  tracked, not proven a defect; the hull-clearance finding may
  warrant a handling-model review (departure vs. original).
- Next smallest useful action: F04-C.2 — a flat-ground retail break
  for fragment-settle evidence (the London bollard rows show flat
  streets work; needs a `NumParts` prop reachable there), repeated-
  collision/pool-reclaim retail runs; prop-rule stamping research
  (UNK-21); the hull-clearance fidelity question (should wheels or a
  lower bumper region strike kerb-height props?); F13-A; F09-C.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
