# Last implementation iteration

- Task ID and title: F03-A.1 — parse and audit the `PTH1` placement
  pathsets (`*.pathset`), the missing authored placement format named
  by F03-A.
- Starting commit and resulting commits: started at
  `3f08fa7c877ce4895c1889383ef747928ada4016` (clean tree, branch
  `ralph/night`, F09-B.2 externally checked); result = the commit on
  top of it.
- Why this slice: PLAN named F13-A / F03-A / F09-C. F03-A's remaining
  unmapped sources were `.pathset`, `.cpvs`, `.ldef`; `.pathset` is
  the only one that is a *placement* format (cpvs/ldef are F18-claimed
  culling/lights), it is documented in R3, and it is the dependency
  F03-B prop instantiation actually needs. Split as F03-A.1 —
  parser + audit, no runtime consumer yet (instantiation is F03-B).
- Review fix-forward applied first: the F09-B.2 `--turns` summary
  fractions were wrong (denominators undercounted) — corrected to
  471/492 (95.7%) london and 963/971 (99.2%) sf in
  `docs/research/bai.md`, `PLAN.md` and the previous
  `LAST_ITERATION.md`. Conclusion unchanged.
- Production code changed:
  - `crates/mm2_formats/src/pathset.rs` (new): PTH1 parser —
    `"PTH1" u32 path_count u32 current_path`, then per path
    `char[32] NUL-padded name + u32 point_count + u32 selection +
    points(u32 attributes + f32 xyz) + u8 kind + u8 spacing + pad[2]`.
    Typed `PathKind` (0 single-points / 1 directed-pairs /
    2 line-strip, `kind_code` kept raw), `spacing_metres()`
    (quarter-metre units), per-point `attributes` word and the
    `current_path`/`selection` cursors preserved verbatim (inferred
    dev-tool state — UNK-20). `Pathset::validate()` reports
    `PathsetIssue`: undocumented kind, odd directed-pair count,
    non-finite coordinate, out-of-range `current_path`, empty name.
    Sanity caps (4096 paths / 64k points) bound corrupt counts;
    trailing bytes are an error.
  - `crates/mm2_formats/src/lib.rs`: `pub mod pathset`.
  - `tools/mm2_inspect/src/main.rs`: `pathset <install> [--city]
    [--strict]` audit. Denominator = *every* discovered `.pathset`
    (no filtering — junk extensions never match the glob anyway).
    Per file: paths/points/kind histogram + `validate()` issues +
    a name cross-check — `PATHnn` labels skipped, `PREFIX:` event
    decorations stripped for the lookup, then `geometry/<n>.pkg`
    or `texture/<n>.{tex,tga,png,ktx2}` must resolve (VFS is
    case-insensitive; `r4i_railsX_f` resolves). `--city` restricts
    to `city/<stem>/` + `race/<stem>/`; an empty discovery is an
    error. `scan` now recognizes `.pathset`.
- Research/documentation: `docs/research/pathset.md` records the
  measured grammar, the kind/spacing semantics (R3-documented,
  byte-confirmed), the preserved uninterpreted fields, the
  placement-source inventory table for F03-A, and the audit results.
  Ledger: new WLD-12 (verified_original grammar + name resolution)
  and UNK-20 (attributes word, `PREFIX:` states, truncated files);
  UNK-12 updated — `.pathset` now parses, runtime consumption still
  unverified. Event-catalog `RecordContent::Unparsed` still covers
  `.pathset` — parsing records into the catalog belongs to the F03-B
  consumer slice, same as `.aimap`.
- Retail findings reported by the audit (not repaired):
  - 101 files = expected denominator; 98 parse — 2692 paths /
    13185 points, kinds {0:137, 1:122, 2:2433}. Every parsed path
    uses a documented kind.
  - 3 authored truncations fail with specific errors:
    `race/london/blitz10.pathset` (58 B, truncated mid-header),
    `race/london/blitz11.pathset` (663 B, path 1 declares 0x40000
    points), `race/london/london_bridge_blitz10.pathset` (326 B,
    truncated mid-points). UNK-20.
  - 66 zero-point paths across both cities are normal authored data
    (an empty path stamps nothing) — not flagged.
  - Name cross-check: 120/126 unique asset names resolve; the 6 dead
    refs (`r_concrete`, `sp_boxfruit_f`, `sp_fruitcard_l`,
    `sp_plantcard_l`, `prop_sp_*`, `xcp_banrred_f`) are confined to
    `city/phys/` test sets, `city/race0.pathset` and `bak/` snapshots.
    Every mainline `city/<city>/{props,decals*}.pathset` name
    resolves.
- Tests added and why:
  - `mm2_formats::pathset` unit tests (8): all three kinds + field
    decode + `spacing_metres`; bad magic; truncated point list;
    trailing bytes; absurd counts (sanity cap); zero-point paths are
    legal (the retail case); `validate()` reports every issue class;
    32-byte unterminated names.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features
    -- -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, all groups, 0 failures.
  - `mm2-inspect pathset <retail>` — exit 0; 98/101 parsed, 3
    failures, 6 issues (numbers above).
  - `mm2-inspect pathset <retail> --strict` — exit 2 (3 failures +
    6 issues).
  - `mm2-inspect pathset <retail> --city london` — 46 files, 3
    failures, 0 issues; `--city sf` — 52 files, 1 issue
    (`xcp_banrred_f` in `bak/`).
  - `mm2-inspect scan <retail>` — `.pathset` recognized: 98 parsed,
    same 3 failures listed.
- Acceptance IDs satisfied / still open:
  - F03-AC06 (strict audit covers every expected source family):
    ADVANCED — pathsets now have a strict per-file audit with an
    unfiltered denominator; INST/PSDL were already covered. `.cpvs`/
    `.ldef` remain unparsed (F18 scope), so AC06 stays open.
  - F03-AC01 (synthetic pathset tests: spacing, rotation/pairs,
    scale, repeated/shared models, deterministic IDs): PARTIAL —
    parser-level tests cover kinds/pairs/spacing; stamped-placement
    determinism needs the F03-B instantiation slice.
  - F03-AC02/03/04/05: open — runtime placement evidence owed by
    F03-B/F03-C.
  - F03-A parent: stays `queued` — `.cpvs`/`.ldef` sources and
    embedded-PSDL-prop coverage remain (A.2 scope).
- Stock data/GPU/audio/network limitations: audit ran on the real
  retail install through the VFS; no rendering/audio/network paths
  touched. The 3 truncated retail files and 6 dead name refs are
  reported as authored anomalies, not repaired. How the original
  runtime consumes pathsets (which files load per session, exact
  stamping semantics) is unverified — UNK-12/UNK-20.
- Unresolved blockers or discovered regressions: none introduced.
- Next smallest useful action: F03-A.2 (remaining placement-source
  inventory: `.cpvs`/`.ldef` scope decision vs F18, embedded PSDL
  props) or F13-A (checkpoint rules; deps candidates) or F09-C.
  F03-B prop instantiation can now consume `mm2_formats::pathset`.

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
