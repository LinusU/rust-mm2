# Last implementation iteration

- Task ID and title: F00-B.2 — original-rules ledger (child of F00-B;
  completes the F00-B scope alongside B.1's inventory). Also added the
  `mm*data.csv` event-metadata table parser so the ledger's authored
  roster claims are reproducible through the VFS.
- Starting commit and resulting commit: started at
  `156b0cc7621b8ba743047a52ab0f67922f06643a` (clean tree, branch
  `ralph/night`, external check+review had just passed); result = this
  commit.
- Production code changed:
  - `crates/mm2_formats/src/racedata.rs` (new): `EventTable` parser for
    `race/<city>/mm{race,blitz,circuit,crash}data.csv` — one row per
    selectable event carrying the ten-parameter block twice (Amateur +
    Professional halves; the amateur/pro mapping is marked inferred in
    the doc since headers are unlabeled). Structural failures return
    `FormatError`; malformed rows become `TableDiagnostic`s.
  - `crates/mm2_formats/src/lib.rs`: `pub mod racedata`.
  - `crates/mm2_content/src/expect.rs`: `EXPECTED_EVENT_TABLES` — the
    four table names per race city, flagged race-vs-lesson family.
  - `tools/mm2_inspect/src/inventory.rs`: `events()` now reads each
    discovered `mm*data.csv` through the VFS and parses it. Row counts
    go to the races notes (`sf event-metadata rows: mmracedata.csv=12
    rows, …`); missing expected tables, malformed tables and malformed
    rows become rejected records (strict-visible), `mmcrashdata.csv`
    auditing against the lessons family.
- Documentation changed:
  - `docs/original-rules.md` (new): the F00-B.2 ledger — ~90 classified
    facts (verified_original / documented / inferred / designed /
    unknown) covering drivers/progression, all five modes, Crash Course,
    vehicle/paint unlocks, damage, police, multiplayer + C&R, HUD/map/
    cameras, controls/options, menus, world behavior, plus a designed-
    departures list and a 15-item UNK open-question table.
- Source material (all from the local retail install, read-only):
  - `MM2HELP.HLP` decompiled locally with helpdeco (GPLv3 tool, cloned
    to /tmp only — decompiled text is original content and is NOT
    committed; the ledger cites topic names).
  - `Readme.rtf` / `TROUBLE.RTF` via `textutil`; `Booklet.pdf` via pypdf
    in a /tmp venv.
  - Authored data via `mm2-inspect dump`/`inventory`: `mm*data.csv`
    aggregates and all `tune/*.info` Unlock fields.
- Key findings now on record:
  - Authored selectable rosters are 12 checkpoint / 10 blitz / 10
    circuit / 13 crash-course rows per city — matching help's "12
    Checkpoint Races" and "nine lessons + midterms + final". The extra
    `.aimap` files beyond those rosters stay in the expected denominator
    but their selectability is UNK-2.
  - Help documents 20 vehicles; the roster ships 21 — `vpmoonrover` is
    the undocumented one (UNK-3).
  - Circuit races have zero peds, ambient and cops in data (confirms
    documented no-peds rule; no-cops is a new verified fact).
  - Blitz: no opponents/cops, per-event time limits 18–120 (unit
    unverified). Checkpoint: fixed authored cop counts 0–8.
  - `.info` `UnlockScore`/`UnlockFlags` exist (GTR-1 = 8000) but bit/score
    semantics stay unknown (UNK-6).
- Tests added/changed and why: 4 new `racedata` unit tests (retail-shaped
  table, wrong header, empty input, malformed rows as diagnostics) +
  `synthetic_install_counts` extended (well-formed table row count,
  missing table reject, malformed `mmcrashdata` reject on lessons).
- Commands actually run and results:
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features --
    -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, ~112 tests, 0 failures.
  - `mm2-inspect inventory /Users/linus/coding/rust-mm2/retail` —
    event-metadata rows line present per city; counts unchanged
    (races 80/111/78, lessons 42/42).
  - `mm2-inspect inventory <retail> --strict` — exit 2, 33 findings
    (same genuine findings as before; all 8 tables parse cleanly so the
    new check adds no noise on retail).
  - `mm2-inspect inventory <retail> --json` — parses; strict_failures
    embedded.
- Acceptance IDs satisfied / still open:
  - F00 req. 4 (original-rules ledger): delivered —
    `docs/original-rules.md` marks every fact
    verified_original/documented/inferred/designed/unknown with source
    citations. Candidate, pending external check.
  - F00-AC06: advanced — unverified rules stay explicitly open (UNK-1..
    UNK-15); nothing doc-only is claimed as verified.
  - F00-AC02/AC03: advanced slightly — malformed/missing `mm*data.csv`
    tables now reject with logical path + reason through the normal
    strict path.
  - F00-AC01: gates re-run and pass.
  - F00-AC04/AC05: unchanged — dev-world and visual/headless smokes
    still not run (F00-C).
- Evidence files: none; no screenshots/original-content captures
  committed. Decompiled help text lives only in /tmp.
- Stock data/GPU/audio/network limitations: help text was extracted
  offline and cited, not reproduced; no original executable was run, so
  *documented* facts remain doc-trust (the ledger says so). GPU/audio/
  network still unexercised run-wide.
- Unresolved blockers or discovered regressions: none new. The UNK list
  needs either observable original behavior (running retail — not
  possible in this environment) or further format research; it should
  not block unrelated ready work.
- Next smallest useful action: F00-C — unified evidence commands
  (dev-world smoke AC04, visual + headless smokes AC05) with explicit
  missing-capability outcomes.

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
