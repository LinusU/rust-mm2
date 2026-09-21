# Last implementation iteration

- Task ID and title: F03-C.2 — prop-rule stamp orientation repair
  (operator report 3, item 1: "every stamped prop is rotated 90 degrees
  clockwise") plus the swept-footprint audit the report's item 2 asked
  for. Selected over queued feature work per the operator-priority
  policy.
- Starting commit: `5e0751a17fc46bf751f04aa6984a947cf30ff1fb` on
  `ralph/night`; tree was clean.
- Retail install: `/Users/linus/coding/rust-mm2/retail` — read-only,
  audited below (fingerprint `fnv1a64:e91e6cd4b2ae30d9`).

## What the defect actually was

The report's code lead (`yawed_transform`) was measured against retail
data first, per the report's own instruction — and it is *not* the
defect:

- Pathset `direction` → local +X is verified correct: all 85 sampled
  `sp_lightstreet_rt_f` line-strip stamps (SF kerb lamp rows) place
  their local-+Z arm tip over a carriageway, and
  `sp_barricadeconcl_{l,r}_f`'s 5 m wall segments (authored extending
  local +X) form continuous barriers — a +Z mapping would comb them
  perpendicular to the road.
- Prop-rule props are a different authored convention: every
  directional kerb prop measured (`sp_lightstreet_f`,
  `sp_lightbanrb_f`, `sp_traflitdual_f`, `sp_benchwood_f`, sign
  plates) carries its front/arm on local **−X**, and its
  `dgBangerData` bound wraps only the pole/base. The walk stamped
  `forward = side.forward` (the walk direction), so each prop's face
  pointed a quarter-turn away from the road — exactly the reported
  symptom.

## What changed

- `crates/mm2_game/src/props.rs`: `walk_prop_rules` now sets
  `PropStamp.forward` to the per-stamp kerb→building-line direction
  from the strip cross-section — `norm_xz(outer − curb)`, falling back
  to `norm_xz(position − centre)` on the building-line fallback, then
  the side's own right on degenerate geometry. +X goes building-ward,
  so the authored −X front lands on the carriageway, and curved kerbs
  rotate each stamp individually. New shared helpers the audit reuses:
  `yawed_basis` (the axis images `yawed_transform` builds),
  `stamp_content_offset` (bound `+CG` / ground `−min_y` lift) and
  `stamp_space_verts` (best-LOD-per-stem rendered verts, shadow/dmg
  stand-ins excluded — the same selection `pkg_to_parts` renders and
  collides).
- `crates/mm2_formats/src/pkg.rs`: `lod_split` moved here verbatim
  from `city.rs` (PKG naming is a format concern) so both consumers
  share it.
- `crates/mm2_app/src/city.rs`: `yawed_transform` wraps `yawed_basis`
  (unchanged semantics); `PropCache::build`'s `Ground` arm uses
  `stamp_content_offset`; `stamp_prop_rules` docs describe the new
  `forward` contract.
- `tools/mm2_inspect/src/placement.rs`: the placement audit now also
  sweeps each pathset/prop-rule stamp's `stamp_space_verts` through
  the stamp's basis and tests every vertex against the carriageway
  regions — street-level band (`≤2.5 m` above the surface) deeper than
  0.15 m counts `body_in_road`, the overhead band (`≤8 m`) counts
  `overhang`. Per-channel `swept`/`body-in-road`/`overhang` counters,
  channel×kind and channel×prop histograms, deepest-first hit list.
  INST keeps the origin-only check (verbatim authored transforms —
  nothing to sweep for orientation). Origin and footprint results
  stay separate; both are findings, not failures. PKG/banger
  resolution mirrors `PropCache`/`BangerDefs` exactly (lowercased key,
  `geometry/<n>` → `<n>` fallback, `tune/banger` CG with the
  `from_record` finite-clean).
- `docs/research/proprules.md`, `docs/research/pathset.md`,
  `docs/original-rules.md` (UNK-20/UNK-21 narrowed) updated with the
  measured evidence.

## Tests

- `props.rs`: straight-room right/left forwards now assert `[1,0,0]` /
  `[-1,0,0]` (kerb→building-line), multi-room the same; curved-kerb
  test asserts each stamp faces its own cross-section; the no-kerb
  fallback test asserts stamps aim away from the room centre. Pathset
  directed-strip tests still assert the authored direction.
- `placement.rs`: new `footprint_catches_the_rotation_the_origin_
  misses` — a bench-shaped prop yawed across the kerb registers
  `body_in_road` (0.5 m deep) while the origin check sees nothing, and
  yawed the authored way it does not; pole-top vertex exercises the
  overhang band.
- 42 test suites, 0 failures.

## Commands actually run and results

- `cargo fmt --all -- --check` — PASS (after `cargo fmt` on new code).
- `cargo clippy --workspace --all-targets --all-features -- -D
  warnings` — PASS. **Pre-existing gate failure repaired first:** the
  rust-1.98 `chunks_exact_to_as_chunks` lint fired on 14 untouched
  sites in `psdl.rs`/`tex.rs`/`props.rs`/`model.rs`/`city.rs`/
  `analyze_city.rs`; all converted to `as_chunks::<N>().0`
  mechanically. Also folded `measure_footprint`'s 8 args into a
  `FootprintStamp` struct for `too_many_arguments`.
- `cargo test --workspace` — PASS, all suites green.
- `mm2-inspect placement <retail>` — exit 0. Combined: 449 in-road
  origins, **488 body-in-road footprint hits**, 67 issues (pre-existing
  walk counts), 0 failures. Per channel:
  - london prop-rule: 5118 swept, **243 body-in-road** (vs **574** on
    the A/B-reverted walk-facing orientation), 2542 overhang (vs
    ~1700 — lamp arms now reach over carriageways).
  - sf prop-rule: 5028 swept, **2 body-in-road** (vs **108**), 2573
    overhang (vs 747).
  - Residuals classify as authored: `RoadNoSidewalks`/`RoadFan` plaza
    dressing (`sp_lightpark_f`, `sp_phonebooth_l`, `sp_benchwood_f`,
    `sp_can_royal_l`), freeway supports in DividedRoad medians,
    kerb-edge grazes.
- Deterministic before/after captures on sf room 444's lamp rows
  (`--cam=-1470,38,392,90,-8`, `--frames 90`): `/tmp/lamps_before.png`
  (walk-facing — arms parallel to the kerb, as the operator reported)
  vs `/tmp/lamps_after.png` (kerb→building-line — arms reach over the
  carriageway from both kerbs). Both are render-verified.

## Still open

- The residual footprint hits are findings, not proven defects — the
  plaza-dressing/kerb-graze classification is plausible but each
  cluster could still hide an authored-intent mismatch; the hit list +
  histograms are there for a human pass.
- Whether the original composes the identical prop-rule basis from the
  same cross-section stays inferred — the format carries no
  orientation field (UNK-21 narrowed accordingly).
- `yawed_basis` asserts +X→direction for pathset *and* prop-rule; the
  measured evidence is strong (85/85 lamp arms, barricade walls) but
  it is still a single-city prop sample — a frame-by-frame comparison
  against retail screenshots is not yet done.
- Gate repair folded in (the `chunks_exact` lint appeared between
  `5e0751a`'s check and this iteration — toolchain drift, not this
  diff's code). If the runner pins an older toolchain, `as_chunks`
  needs Rust ≥ 1.88 — current stable is 1.98.1.
- F03-C remainder: race cleanup, mod replacement end-to-end,
  UNK-20 expansion density.
- Candidate pending external check.
