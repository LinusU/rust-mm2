# Last implementation iteration

- Task ID and title: F04-C.2/-C.3 evidence re-take — re-run the
  retail strike/settle/pool commands the F03-B.5 placement repair
  had invalidated (the external review's top verification gap and
  the named next action in the previous handoff).
- Starting commit and resulting commits: started at
  `3ac8a5a4ff96c46b0065c5196ebacc185c940b09` (the externally checked
  F03-B.5 commit; branch `ralph/night`, clean tree).
- Why this slice: review/repair before new feature work. The
  corrected prop bounds invalidate every retail transition count
  recorded against the sunk geometry; F04 cannot roll up until the
  evidence is re-taken. No production code change resulted — the
  re-take produced honest measurements, including one legitimately
  changed outcome that is a *corrected record*, not a defect.
- Retail install: `/Users/linus/coding/rust-mm2/retail`
  (`fnv1a64:e91e6cd4b2ae30d9`), all runs `--headless` on this
  machine's dev-profile binary; `bng=` denominators now include the
  prop-rule channel (sf 5 927, london 6 271 dormant bangers).

## What changed vs the sunk-geometry records

- **F04-C.3 bound-strikes (`--bot`, 1500f):**
  - `vpddbus --spawn=-1641.6,36.7,410,0` → `bng_ev=3a/2s/0b` (was
    `1a/0s`; re-run bit-identical). Full-height `sp_cone_f` bounds
    (0.85 m vs ~0.43 m exposed when sunk) overlap the bus bound
    across more of the row; two sleep on the slope.
  - `vpbus --spawn=0.4,5.5,-720,0` → `1a/1s/0b` (unchanged).
  - `vpbug` same run → `1a/1s/0b` (was `2a/2s` on the sunk row —
    halved strike reach + the corrected ~4× kick changed which
    bollard the trajectory clips).
- **F04-C.2 settle/pool/repeat:**
  - Original spawn `--spawn=-170,1.5,-565,30` → `0a/0s/0b` at
    10 000 ticks: the bus reaches the `sp_barricadewood_f` row's
    first prop at ~10 m/s → estimate ~49 000 < authored 51 888 →
    stays dormant, bus slides around the row. The pre-fix `1b`
    here was measured on the sunk bound; the corrected record is
    that this spawn sits just under the limit.
  - Negative leg re-taken: `--spawn=-164,1.5,-552,30` → `0a/0s/0b`,
    peak 9.7 m/s — dormant barricade stops the bus dead.
  - Restaged positive leg (same row, perpendicular approach):
    `--spawn=-141.9,1.5,-608.5,115` → two `banger shattered` at
    ~16.85 m/s (estimate ~82 800) → `0a/5s/2b` at 10 000 ticks:
    8 fragments, 5 `Slept`-settled on the flat lot, `impacts=80`
    of battering all finite. Re-run bit-identical.
  - Pool bound on fragments (same spawn): `--banger-pool 2` →
    `1a/1s/2b`; `--banger-pool 1` → `1a/0s/2b` — live pieces never
    exceed the cap.
  - London reclaim ring (`vpbug --spawn=802,6,-905,180`):
    `3a/0s/0b` default, `bng_rec=1` at pool 2, `bng_rec=2` at
    pool 1 — bit-identical to the pre-fix records.
- **Operator's sawhorse (root-cause confirmation):**
  `sp_sawhrslt_f` resolves only to `race/london/race7.pathset` —
  the `checkpoint:7` overlay (13 `Points` stamps near (−695, 235),
  limit 34 982, NumParts 3; `circuit7.pathset` is all concrete
  barricades — an event-name slip in the earlier notes, now
  corrected in both docs). `vpddbus --event checkpoint:7
  --spawn=-703,1.5,217,190` → two `banger shattered` at 8.7 and
  11.7 m/s → `0a/1s/2b`. The prop that "didn't move" at 100+ km/h
  in the operator's build was buried ~0.73 m; on corrected
  geometry it shatters at a third of that speed.

## What this proves / does not prove

- Proves: activation/break/settle/pool-cap/reclaim all behave on
  corrected geometry on real content; both sides of the authored
  `ImpulseLimit2` gate have corrected-geometry evidence; the
  strike-bound repair still fires (more so, on taller bounds);
  results are deterministic (bit-identical re-runs).
- Does not prove: sub-limit hit → tip (UNK-22 — a ~1 200 kg car at
  100 km/h still estimates under the sawhorse's 34 982 limit);
  natural-pool (>32) reclaim (no surveyed site); impulse
  equivalence of bound-strike vs manifold contact; GPU/audio/net
  coverage. The re-take is counter evidence, not original-rule
  verification.
- Incidental finding, recorded not fixed: two `sp_parkmtr_f`
  prop-rule props now sit inside the longer runway of one staging
  path — new denominator from the prop-rule channel, not a defect.

## Commands actually run and results

- `cargo build -p mm2_app --bin mm2` — clean.
- All smoke commands above → `status=pass`; debug-level
  `mm2_app::banger` logs captured the per-prop
  name/severity/estimate for each transition.
- `mm2 --event checkpoint:7 --cam=<overhead> --frames <n>
  --screenshot /tmp/*.png` → awaited PNGs used to locate the
  roadblock (local captures only, not committed).
- `cargo fmt --all -- --check` PASS; `cargo clippy --workspace
  --all-targets --all-features -- -D warnings` PASS;
  `cargo test --workspace` PASS (docs-only diff, suite unchanged).
- Acceptance IDs: advances F04-C.2/C.3's original-content evidence
  to the corrected geometry; F04 stays `implemented` pending
  external check + UNK-22. AC01's both-sides-of-threshold leg is
  re-evidenced.
- Deferred deliberately: F13-A remainder / F14-A / F11-C (next
  slice per selection policy); per-prop audit of the remaining
  ~980 records stays open.
- Unresolved blockers or regressions: none found this iteration.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
