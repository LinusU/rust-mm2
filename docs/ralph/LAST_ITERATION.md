# Last implementation iteration

- Task ID and title: F03-B review repair — bound the `props.pathset`
  line-strip expansion flagged by external review of candidate
  `9e444eb` (iteration 24, verdict fail, 1 blocking finding).
- Starting commit and resulting commits: started at
  `9e444eb881ea343e4bbd3cf2e4a71667e3993e0b` (clean tree, branch
  `ralph/night`); result = one repair commit plus this handoff note.
- Root cause (external review finding): `stamp_line_strip` walked
  `while t < len { out.push(...); t += spacing }` over authored
  coordinates the parser never bounds (counts are capped, magnitudes
  are not). A finite segment beyond ~4e6 m stalls `t += spacing`
  below the f32 ulp — the loop never terminates; `len = +inf`
  likewise — and even shorter hostile lengths push millions of
  instances → hang/OOM during city load. Reachable through a
  supported path: a VFS mod can override
  `city/<city>/props.pathset`, or ~16 bad bytes in a corrupt install.
- Fix (smallest real change, same style as the parsers' sanity caps):
  - `crates/mm2_app/src/city.rs`
    - `MAX_PATHSET_STAMPS = 8192`: per-file stamp budget threaded
      through every path in `load_city`. ~7x the densest retail
      expansion (London 1188 / sf 925; densest single path 162 —
      measured by dumping both retail `props.pathset` files through
      `mm2-inspect dump` and replicating the expansion in a script).
    - `stamped_transforms(path, budget) -> StampedPath { transforms,
      capped }`: every kind emits at most `budget`; `capped` counts
      suppressed stamps (saturating) instead of truncating silently.
    - `stamp_line_strip`: per-segment stamp count computed
      arithmetically (`ceil(len/spacing)` in f64; the float→int cast
      saturates into the budget `min`), index-based `t = i*s` — no
      accumulator to stall. Non-finite segment lengths skipped; a
      non-finite final vertex does not cap the row.
    - Non-finite point coordinates stamp nothing in all kinds (no
      NaN-transform entities).
    - `load_city` runs `Pathset::validate()` on the consumed file:
      issues warn-logged and counted in `CityReport::pathset_issues`;
      suppressed stamps counted in `CityReport::pathset_props_capped`.
      Both appear in the report `Display`.
    - Unrotated stamps (`Points`, zero-spacing strips, lone vertices)
      now route through `inst_transform` via `unrotated_transform` —
      semantically identical today, closes the reviewer's minor note
      about the `MIRROR_Z` convention living in one place.
  - `docs/research/pathset.md`: the bound documented as an
    implementation choice, not an original rule.
- Tests added (4, all in `city::tests`, pure checks):
  `a_hostile_segment_is_capped_instead_of_hanging` (1e18 m fixture
  asserts the cap at both a small budget and the production
  constant), `non_finite_segments_stamp_nothing_but_stay_bounded`
  (+inf/NaN segments skipped, NaN final vertex not capped, emitted
  origins finite), `non_finite_points_stamp_nothing` (Points and
  Directed), `expansion_is_bounded_by_the_stamp_budget` (capped
  counts: strip interval stamps + end cap; Points).
  Existing 11 stamping tests updated for the new signature.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features
    -- -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, all groups, 0 failures
    (mm2_app lib 18/18).
  - `mm2 --mm2-path <retail> --city london --headless` — `status=pass`;
    `1188 pathset props (0 decal paths, 0 failed, 0 capped, 0 issues)`
    — identical stamped count to the pre-fix baseline.
  - `mm2 --mm2-path <retail> --city sf --headless` — `status=pass`;
    `925 pathset props (31 decal paths, 0 failed, 0 capped, 0 issues)`.
- Acceptance IDs satisfied / still open:
  - Review blocker: addressed — expansion is bounded per file,
    non-finite data skipped and reported, overflow counted in
    `CityReport`, regression tests cover huge/infinite fixtures.
  - F03-AC01: ADVANCED as before (budget/cap semantics now also
    pinned by tests).
  - F03-AC02/AC03/AC04/AC05/AC06: unchanged — open per the previous
    handoff (spot validation, decal policy, race overlays, mod
    evidence, `.cpvs`/`.ldef`).
- Stock data/GPU/audio/network limitations: ran on the real retail
  install through the VFS; no new rendered capture needed (the stamped
  counts and transforms are byte-identical on retail — the bound is
  inert on valid data). sf headless smoke ended wheels=0/4 again —
  same airborne end state as the reviewed baseline (peak 37.2 m/s,
  moved ~171 m), not a regression.
- Unresolved blockers or discovered regressions: none known.
- Next smallest useful action: F03-A.2 scope decision, decal stamping
  (needs a strip-width rule — research first), or `race/*.pathset`
  overlays under F03-AC04. F13-A/F09-C remain ready alternatives.

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
