# Last implementation iteration

- Task ID and title: F04-C.2 — flat-ground retail break/fragment-settle
  evidence, repeated-collision and pool-reclaim retail runs.
- Starting commit and resulting commits: started at
  `8907c54424656176424a237b10264d0cb61c98cb` (clean tree, branch
  `ralph/night`; F04-C.1 passed external gates + review).
- What landed (code):
  - `DevOverrides::banger_pool` + `--banger-pool <n>` — quarantined dev
    bound on simultaneously active bangers, applied session-scoped in
    `load_session_world` (re-stamped every generation, like `--spawn`).
  - Smoke record: `bng_pool=<n>` emitted when the bound is overridden;
    `bng_rec=<n>` counts `BangerStateChanged` transitions whose cause is
    `Reclaimed` — a pool settle is otherwise indistinguishable from an
    Avian-sleep settle in `bng_ev=`. Default records are bit-identical
    (both fields only appear when meaningful).
  - Test `dev_banger_pool_override_bounds_the_active_pool`: the override
    lands on the session resource through the real load path; an
    unconfigured session keeps the recovered ×32.
- Retail evidence gathered (install
  `/Users/linus/coding/rust-mm2/retail`, fnv1a64:e91e6cd4b2ae30d9,
  headless `hold` driver via `target/debug/mm2`):
  - Flat-ground break + fragment settle (SF):
    `--city sf --car vpddbus --spawn=-170,1.5,-565,30 --headless`
    strikes the `sp_barricadewood_f` row (limit 51888, NumParts 4)
    stamped diagonally across the y≈0 lot at (−177…−188, −578…−601).
    `--frames 1200` → `bng=924d/3a/1s/1b bng_ev=0a/1s/1b`;
    `--frames 2500` → `2a/2s` (`impacts=104`);
    `--frames 5000` → `1a/3s` (`impacts=105`). Fragments reach Avian
    sleep one by one on flat ground — answers F04-C.1's open
    slope-settle observation: the settle path works on real fragments;
    sloped ground keeps them tumbling.
  - Repeated collisions within budget (same run): 104–105 deduplicated
    impacts of continued pen battering, all poses finite, dormant count
    stable — no duplicate bodies, no extra transitions (later hits are
    below the 51 888 limit or land on already-broken pieces).
  - Pool bound enforced (dev-bound runs, same spawn):
    `--banger-pool 2 --frames 1200` → `bng=924d/2a/0s/1b bng_pool=2`
    (2 of the authored 4 fragments spawn, the rest skipped at the cap);
    `--banger-pool 1` → `bng=924d/1a/0s/1b bng_pool=1`.
  - Repeated activations + pool reclaim (London):
    `--city london --car vpbug --spawn=802,6,-905,180 --headless
    --frames 800` drives ~50 m through the `sp_bollard_stone_l` ring
    stamped around the plaza at (786…827, −814…−857). Default bound →
    `bng=1185d/3a/0s/0b bng_ev=3a/0s/0b` (three distinct activations on
    successive ticks). `--banger-pool 2` → `bng_ev=3a/3s/0b
    bng_pool=2 bng_rec=1` — the third hit reclaimed the oldest active.
    `--banger-pool 1` → `bng_rec=2`. Re-ran bit-identical.
  - Hull-clearance corollary: `vpddbus` (~4 915 kg) drives clean over
    `sp_cone_f` clusters (zero contacts where `vpbug` activates) —
    striker choice must match prop height to hull floor; the fidelity
    question stays open.
- What this proves / does not prove:
  - Proves: fragment settling on real flat-ground placements
    (Active→Settled via Avian sleep); repeated collisions stay within
    the configured bound with finite transforms on retail; the pool cap
    is enforced on real placements; pool reclaim (oldest-first
    `Reclaimed` settle) fires on real placements under a configured
    bound.
  - Does not prove: reclaim at the natural ×32 cap — no surveyed retail
    site produces >32 simultaneous actives (breaks yield 2–5 pieces,
    hits must land on separate ticks); recorded as an honest gap, the
    dev bound exercises the same claim/reclaim path. Original
    threshold/break-timing semantics stay UNK-22. No GPU/audio/network
    evidence — all runs are headless physics through the VFS.
- Commands actually run and results:
  - Targeted test: `cargo test -p mm2_app --test session
    dev_banger_pool` — pass.
  - All retail probe runs above; screenshots `/tmp/sf_barr*.png`,
    `/tmp/sf_fwy.png`, `/tmp/lon_r7*.png` (local only, not committed).
  - Full gates re-run before commit — results in the handoff below.
- Acceptance IDs satisfied / still open:
  - F04-AC04 (repeated collisions + budgets): now has retail evidence —
    repeated hits within a configured bound, cap enforced, reclaim
    observed. Natural-×32 reclaim remains a stated gap; AC stays open
    pending review.
  - F04-AC02: retail support — three sequential activations with no
    duplicate bodies; synthetic dedup coverage unchanged. Open.
  - F04-AC01/AC03/AC05: unchanged from F04-C.1 evidence. AC06 open
    (awaits F26).
- Deferred deliberately: `BirthRule` particles, `AudioId`/`Flash`/
  `TexNumber` effects, decals, prop-rule stamping (UNK-21), `Timer`
  despawn, replication (F26), GPU capture of a break, hull-vs-low-prop
  fidelity review, natural-pool reclaim staging.
- Stock data/GPU/audio/network limitations: all evidence is headless
  physics through the VFS on the real install (read-only).
- Unresolved blockers or discovered regressions: none known.
- Next smallest useful action: prop-rule stamping research (UNK-21 —
  the second verified banger channel), decal stamping research, the
  hull-clearance fidelity question, F13-A or F09-C.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
