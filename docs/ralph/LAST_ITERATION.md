# Last implementation iteration

- Task ID and title: F06-A.1 — authored surface-material tables parsed
  and audited (`city/materials.mtl` property blocks +
  `city/materials.csv` texture→material map).
- Starting commit and resulting commits: started at
  `f7158806e02ef9d7a9605b07a63f2036e5d228d1` (externally checked
  F03-B.6 handoff; branch `ralph/night`, clean tree).
- Why this slice: F06's deps (F00-B, F01-B) are both checked and the
  traction work needs its authored input first. The retail install
  ships exactly one `materials.{csv,mtl}` pair — the texture-name →
  physics-material system every later F06 leg must resolve through.
  Parsing + auditing it is the smallest honest step: it verifies the
  grammar, measures real coverage against both PSDL texture tables,
  and surfaces authored anomalies before any runtime semantics are
  assumed.
- Retail install: `/Users/linus/coding/rust-mm2/retail`
  (`fnv1a64:e91e6cd4b2ae30d9`).

## What changed

- **`mm2_formats::materials` (new):**
  - `MaterialSet::parse` — line-oriented `mtl <name> { … }` blocks;
    `{` may sit on the next line, `key:` is optional (whitespace
    separates otherwise), lone `}` closes, `//` comments. Fields kept
    raw with typed accessors (`f32`, `vec_f32`, `vec_i64`, `text`);
    `index()` gives the authored index space.
  - `MaterialMap::parse` — `texture,physics` csv; `none` keyword =
    "no named material"; header recorded not enforced; short/extra-
    cell rows go to `diagnostics` (bounded, recoverable).
  - `MaterialIssue` + `validate()` on both: duplicate defs/rows,
    missing `_default`, missing/bad/negative/unknown fields,
    nonstandard header. `MaterialMap::undefined_refs(&set)` cross-
    checks named refs against defined materials.
- **`mm2_formats::tex::frame_base_stem`** — shared `<stem>-NNNN`
  animated-frame convention (exactly 4 digits): `s_thames-0009` →
  `s_thames`. The PSDL texture table references single frames of the
  water sequences while `materials.csv` keys the base stem — the same
  convention `mm2_app::city::load_image_sequence` expands in the other
  direction.
- **`mm2-inspect materials <install> [--city] [--strict]`** — audit
  over the VFS: expected `city/materials.{csv,mtl}` (missing = parse
  failure), every discovered `.mtl`/`materials*.csv` in the
  denominator, csv→mtl dead-ref issues, defined-vs-used material
  counts, texture-file resolution split (semantic-only stems reported
  as informational, not defects), and per-city PSDL texture-table
  coverage — named/`none`/blank-slot/unmapped with the frame-stem
  fallback. `scan` recognizes `.mtl`. `--strict` exits nonzero on
  issues/failures.
- **`docs/research/materials.md`** — grammar, field table, measured
  coverage, dead refs, the separate `floors/walls.csv` neighborhood
  table called out.
- **`docs/original-rules.md`** — WLD-19 (verified_original: pair
  structure, counts, coverage, anomalies) and UNK-23 (consumer chain,
  `_default` policy, dead-ref semantics, `sound`/`ptx*` spaces,
  `width/height/depth` meaning, force-path combination — all
  unverified).

## Retail audit (measured)

```
city/materials.csv  3423 rows (137 named, 3286 none)
city/materials.mtl  8 materials (deepwater,_default,grass,water,
                    dirt,sand,cobblestone,wood)
issues: transbay_ramp_f→ash (l.2563), s_grass2mud→mud (l.2824)
        — dead authored refs; --strict exits 2
defined: 8, used: 7 (unused: _default,dirt,wood)
csv stems resolving to texture/ files: 3287/3423 (136 semantic-only)
london psdl: 469 names = 152 named + 308 none + 6 blank + 3 unmapped
  (cobblestone×141, grass×10, deepwater×1;
   unmapped: sliver, sf_win_brickyel01_2s_4_l, sf_base_tan09_1s_5_l)
sf psdl:     457 names = 148 named + 301 none + 6 blank + 2 unmapped
  (cobblestone×136, grass×10, deepwater×1, water×1;
   unmapped: sliver, gw_stc_offwhite_marswin_f)
```

## Tests

- `mm2_formats::materials` — 7 unit tests: block parsing (brace on
  next line, colon-less field, comment), garbage rejection
  (unterminated/missing brace/nameless), validate issues (duplicate,
  missing default/field, negative, bad ptx shape, unknown field), csv
  parsing (header, `none`, diagnostics), duplicate/bad-header,
  `undefined_refs`.
- `mm2_formats::tex::frame_base_stem` — strips only exactly-4-digit
  suffixes; non-frame dashes, wrong digit counts and empty bases kept.

## Commands actually run and results

- `cargo fmt --all -- --check` PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` PASS.
- `cargo test --locked --workspace` — all groups, 0 failures.
- `mm2-inspect materials <retail>` — output above; `--strict` exits 2
  on the two authored dead refs.

## What this proves / does not prove

- Proves: both authored tables parse under a verified grammar; the
  texture→material map covers ~all real texture names in both cities'
  PSDL tables once frame stems and blank slots are normalized; the
  audit distinguishes named/`none`/unmapped honestly and fails strict
  on dead refs.
- Does not prove: any runtime behavior — no surface identity reaches
  colliders, no traction consumes `friction`, no consumer chain is
  verified (UNK-23). The `_default` fallback for unmapped names, the
  `ash`/`mud` dead-ref semantics and the frame-stem normalization are
  all unverified against the original.
- Acceptance IDs: supplies the authored-data leg F06-A needs (source
  surface IDs begin here as material names/index space); F06-AC
  contact/traction legs all remain open.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
