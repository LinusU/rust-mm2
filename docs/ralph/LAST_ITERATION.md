# Last implementation iteration

- Task ID and title: F00-B.1 — versioned content inventory via
  `mm2-inspect inventory` (child of F00-B; advances F00-AC02). Also
  repaired the external review's factual defect: SF circuit authored
  data is `circuit0–11`, not `cir1–9/circuit8–9` (fixed in PLAN.md
  Discoveries + F14-A row; `cir1–9` are `_strtpnts` records only).
- Starting commit and resulting commit: started at
  `4f624e314deb9f94f061e5e0f27ea00eb4dc375b` (clean tree, branch
  `ralph/night`); result = this commit.
- Production code changed:
  - `crates/mm2_content/src/expect.rs` (new): authored stock
    denominators — expected cities, per-city event rosters
    (london 40 races/16 lessons, sf 40 races/26 lessons), ped
    archetypes, audio families. Shared next to `EXPECTED_STOCK_ROSTER`
    so F11's event catalog audits against the same table.
  - `tools/mm2_inspect/src/inventory.rs` (new): report builder composing
    the existing VFS, `VehicleCatalog`, `Psdl`/`inst` parsers — no
    second loader. 12 families, per-family
    expected/discovered/accepted/rejected/unverified/extras counts,
    rejected entries preserved with reasons, `fnv1a64` catalog
    fingerprint over resolved-path provenance + source sizes,
    `--json`/`--strict` modes.
  - `tools/mm2_inspect/build.rs` (new): embeds `git rev-parse HEAD`
    (+`-dirty`) so reports are versioned by engine commit.
  - `tools/mm2_inspect/src/main.rs`: `inventory` subcommand;
    `build_vfs_report` exposes mount diagnostics (archive/mod list)
    alongside the VFS.
  - `tools/mm2_inspect/Cargo.toml`: `tempfile` dev-dependency (same
    version already used by other workspace crates).
- Tests added/changed and why: 5 new tests —
  `expect::roster_sizes_match_retail_audit` (expected table totals),
  `inventory::race_file_classification` (14 file-kind/stem cases
  incl. `.#` junk, `-a/-p` opp tails, `data_p`, `waypoints`,
  `strtpnts`), `synthetic_install_counts` (synthetic loose-file install
  exercising every family's expected/missing/partial/junk paths),
  `empty_install_never_passes_strict` (F00-AC02 empty-catalog guard),
  `fingerprint_changes_with_catalog` (determinism + change detection).
- Commands actually run and results:
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features --
    -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, ~108 tests, 0 failures.
  - `mm2-inspect inventory /Users/linus/coding/rust-mm2/retail` — 13,389
    logical paths; cities 5/5 parsed (2 expected + 3 extras); vehicles
    21/29 accepted (8 rejected w/ dep reasons); races 80 expected, 78
    accepted, `circuit11` partial in both cities (opp/pathset only, no
    `.aimap`), 31 extra records; crash lessons 42/42; placement 13 inst
    parsed + 157 unparsed aux; audio 7/7 families present, 3293 files
    unverified; peds 4/4 + wolf partial + CVS junk rejected; MP 131
    markers (99 `.aimap_p`, copchase/multicop/`*_p` variants, no C&R
    data); traffic 23 va* ids (garbagetruck lacks model); breakables
    995; profiles 17; interface/effects 117.
  - `mm2-inspect inventory <retail> --json` — parses as JSON; 12
    families, 33 strict failures listed.
  - `mm2-inspect inventory <retail> --strict` — exit 2 (findings are
    real partial/junk authored records, not tool errors).
- Acceptance IDs satisfied / still open:
  - F00-AC02: advanced — the report carries all five counts per family
    plus extras; strict fails on empty expected catalogs and on real
    rejected entries. Candidate, pending external check.
  - F00-AC06: advanced — unverified content (audio, races, peds, MP,
    traffic, breakables) is reported as unverified, never as supported.
  - F00-AC01/AC03/AC04/AC05: unchanged from last iteration (AC01
    gates re-run and pass; dev-world smoke still not run).
- Evidence files: none; no screenshots/original-content captures.
- Stock data/GPU/audio/network limitations: retail install enumerated
  read-only; race/audio/anim/banger formats have no parser so those
  families report presence-level acceptance + unverified content — the
  classification is stated in the report notes, not hidden.
- Unresolved blockers or discovered regressions: none new. `circuit11`
  (both cities) and the SF `r0` record are authored-but-partial; whether
  they are real events or dev leftovers stays `unknown` until F00-B.2's
  rules ledger. `MM2_GAME_DIR` convention still open — inventory keeps
  the explicit-dir convention used by every `mm2-inspect` command.
- Next smallest useful action: F00-B.2 — original-rules ledger
  (MM2HELP.HLP / Readme.rtf / authored data), each fact classified
  verified_original/documented/inferred/designed/unknown.

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
