# Last implementation iteration

- Task ID and title: F11-A repair — review-rejected candidate
  (`22e2ded`, verdict fail, sole blocking finding: `EventCatalog`'s
  VFS-scanning/parse producer lived inside `mm2_game`, contradicting
  `docs/architecture.md`'s rule that `mm2_game` never does parsing or
  VFS work).
- Root cause: crate placement, not logic. The written boundary and the
  code disagreed with no acknowledgment; `docs/architecture.md` also
  omitted `mm2_content` entirely, so the intended home for
  VFS→runtime producers was undocumented.
- Starting commit and resulting commit: started at
  `22e2ded764935a46f51992d358c4d26601a8520a` (clean tree, branch
  `ralph/night`); result = this commit.
- Production code changed:
  - `crates/mm2_game/src/events.rs` → `crates/mm2_content/src/events.rs`
    (`git mv`, logic unchanged): the whole `EventCatalog` module —
    types plus `scan` producer — now lives beside the precedent
    `mm2_content::catalog::VehicleCatalog::scan(&Vfs)`. Only its
    `EventRef`/`EventTableKind` import moved from `crate::config` to
    `mm2_game`.
  - `crates/mm2_content`: gained `mm2_game` as a dependency (acyclic
    `mm2_content → mm2_game` edge; `mm2_game` does not depend on
    `mm2_content`), `pub mod events` + re-exports, and a `tempfile`
    dev-dependency for the moved test.
  - `crates/mm2_game`: `events` module and its re-exports removed;
    `tempfile` dev-dependency dropped (only the moved test used it).
    `EventRef`/`EventTableKind`/`SessionConfig` stay — they are
    contract types, which is exactly where the doc wants them.
  - `crates/mm2_game/tests/events.rs` →
    `crates/mm2_content/tests/events.rs` (`git mv`): imports split
    between `mm2_content` (catalog types) and `mm2_game`
    (`EventRef`/`EventTableKind`); all five tests unchanged.
  - `tools/mm2_inspect`: the `events` command's five `mm2_game::`
    references re-pointed to `mm2_content::`; the tool's `mm2_game`
    dependency removed (it had no other use).
  - `docs/architecture.md`: `mm2_content` added to the layer diagram
    and given a dependency-rule bullet as the content→runtime producer
    layer; the `mm2_game` bullet now states the boundary precisely
    (may reference `mm2_formats` types and carry the `Mm2Vfs` handle,
    but never lists, reads or parses content itself); the plugin-attach
    bullet names `mm2_content` as the home for file-content producers.
  - `Cargo.lock`: regenerated for the three manifest edge changes only
    (no version changes).
- Tests added/changed and why: no behavior change — the five
  `tests/events.rs` cases moved crates with the module and still cover
  the same AC01/AC06 surface (row→stem indexing, dependency attach,
  crash links, rewards, extras, resolve errors, empty/malformed
  installs). The regression is architectural: `mm2_game` no longer
  compiles any VFS/parsing code path, so the boundary is enforced by
  the dependency graph, not by review.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features --
    -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, 23 test result groups,
    0 failures (the events suite now runs under `mm2_content`).
  - `cargo run -p mm2_inspect -- events /Users/linus/coding/rust-mm2/
    retail --strict` — exit 0, output identical to the pre-move
    baseline: london 45/45 ready (33 extras), sf 45/45 ready (32
    extras), rewards + 6 milestone rewards per city, no failed refs.
- Acceptance IDs satisfied / still open: unchanged — F11-AC01 and
  F11-AC06 remain candidate-level pending external check; AC02–AC05
  are F11-B runtime lifecycle scope and remain open.
- Evidence files: none committed; no captures made.
- Stock data/GPU/audio/network limitations: unchanged — no audio or
  networking code; GPU not exercised. `.aimap`/`.pathset` records are
  still resolved-but-unparsed (`RecordContent::Unparsed`); record
  semantics unverified where undocumented.
- Unresolved blockers or discovered regressions: none. Review's
  non-blocking verification gaps from iteration 009 still stand (mod
  fifth-table blind spot, `race/<city>` dir pseudo-extra, indexed
  reward rows ignoring `race_type`, row diagnostics not printed by the
  events command, `StartPointsFile` without a table consumer) — all
  mod-path/cosmetic, none retail-observable; left recorded rather than
  silently widened.
- Next smallest useful action: F11-B — shared race lifecycle over this
  catalog (countdown + input lock, swept checkpoint triggers from
  parsed waypoints/widths, participant progress, restart/cleanup,
  once-only `SessionResult` emission), now building on the correctly
  placed module. If the runner requires checked deps first, F03-A or
  F09-A are independent ready alternates.

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
