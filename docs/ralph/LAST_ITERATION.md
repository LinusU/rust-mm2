# Last implementation iteration

- Task ID and title: F09-A.2 — parse and validate the `.aimap` /
  `.aimap_p` AI-map override files (speed limits, per-road exceptions,
  police/opponent spawns, ambient rosters) with independent fixtures
  and a `mm2-inspect` audit command.
- Starting commit and resulting commits: started at
  `ef999273c82d1fb3a234cf234fc9574b4c4d8112` (clean tree, branch
  `ralph/night`, F09-A.1 BAI leg externally checked); result = the
  commit on top of it.
- Why this slice: PLAN named it as the direct continuation of F09-A —
  it completes the parent slice's parse coverage and gives F09-B both
  of its inputs (BAI graph + per-event overrides). Dependencies F00-B
  and F01-A are both externally `checked`.
- Production code changed:
  - `crates/mm2_formats/src/aimap.rs` (new): `Aimap::parse(&str)` for
    the INI-like grammar measured on all 209 retail files — `#`
    comments/blanks ignored anywhere, `[Name]` section headers, scalar
    bodies (`[Density]`, `[Speed Limit]`, `[Ambients Drive On The
    Left]`, `[CopChaseDistance]`, `[AmbientLaneChanges]`), counted-list
    bodies (`[Exceptions]`, `[Police]`, `[Opponent]`, `[Ambient
    Types/Density]`, `[GoodWeatherPedName / BadWeatherPedName]`,
    `[Hookmen]`), free-form `[Traffic Lights]`. Grammar violations
    (orphan data, malformed headers, bad/mismatched counts, counts
    capped at 1<<16) are `FormatError`; malformed rows inside valid
    sections are skipped into `diagnostics`. Typed records keep
    undocumented numeric tails raw (`PoliceRecord.params`,
    `OpponentRecord.params` — two retail shapes each, 8/5 and 10/1
    numeric columns); unknown sections are preserved verbatim in
    `unknown_sections`. `Aimap::validate()` reports `AimapIssue`:
    duplicate exception roads, ambient-weight range/monotonicity/
    1.0-closure, non-0/1 flags, negative scalars, uninterpreted
    sections.
  - `crates/mm2_formats/src/lib.rs`: `pub mod aimap`.
  - `tools/mm2_inspect/src/main.rs`: new `aimap` subcommand
    (`[--city] [--strict]`) — expected denominator is
    `city/<stock>.aimap` plus every discovered
    `race/<stock>/*.aimap{,_p}`; other aimaps audit as extras
    (failures `unsupported`, not hidden). Parsed files get
    `validate()` issues + diagnostics plus two cross-checks:
    `[Exceptions]` road ids against the same-city `city/<city>.bai`
    road-id set, and `[Opponent]` waypoint refs resolved through the
    VFS. `.aimap`/`.aimap_p` added to `scan` recognized formats.
- Format research (docs/research/aimap.md): full section vocabulary
  enumerated (12 sections), body forms classified scalar/counted/
  free-form, every retail count verified exact, police/opponent tail
  shapes and the two anomalies below recorded. Ledger: WLD-6/7/8
  added, UNK-12 updated (aimap structure parses; runtime semantics
  stay open), UNK-18 added.
- Retail findings reported by the audit (not repaired):
  - 8 london files (`blitz0_p`, `blitz2{,_p}`, `blitz5`, `race0{,_p}`,
    `race1{,_p}`) carry `[Exceptions]` road ids 562–815, outside
    `city/london.bai`'s sequential 0–539 road space — every SF id is
    in range. Possibly the `london_sup.bai` id space (unparsed) or
    dead authored refs — UNK-18.
  - `race/sf/stunt0.aimap` names opponent waypoint `opp-c0.2`, which
    resolves nowhere in the VFS.
  - `roambak.aimap` ambient rows omit the flag column (accepted,
    flag=0).
- Tests added and why (`aimap::tests`, 9): all-sections retail-shaped
  fixture incl. both police/opponent tail shapes + 2-field ambient
  row; optional sections absent; comments/CRLF/blank tolerance; orphan
  data → error; count under/over/non-numeric/missing/implausible-cap
  → error; malformed rows → diagnostics not panics; unknown section
  preserved + flagged by validate(); all five issue kinds; malformed
  `[` header → error.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features
    -- -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, all groups 0 failures
    (mm2_formats lib now 76 tests incl. 9 new).
  - `mm2-inspect aimap /Users/linus/coding/rust-mm2/retail` — exit 0:
    209/209 expected files resolve + parse (2 city + 207 race), 0
    unsupported extras, 9 issues (the 8 london exception-id groups +
    the dead `opp-c0.2` ref, all reported above).
  - `mm2-inspect aimap <retail> --strict` — exit 2, "0 failures, 9
    issues".
  - `mm2-inspect aimap <retail> --city sf` — 107 files, 1 issue.
  - `mm2-inspect scan <retail>` — `aimap 110` / `aimap_p 99`
    recognized, all parsed; the 3 scan failures are pre-existing and
    unrelated (2 `_sup` BAIs + `geometry/thing.pkg` bad magic,
    unchanged code paths).
- Acceptance IDs satisfied / still open:
  - F09-AC02 advances further: aimap road ids and `.opp` refs are
    validated (cross-checks in the audit), and unsupported/unknown
    sections surface verbatim rather than being dropped.
  - F09-AC01 unchanged (parse-level fixtures only; directed route
    queries are F09-B).
  - F09-AC03/AC04/AC05/AC06 unchanged — F09-B/F09-C scope.
- Stock data/GPU/audio/network limitations: parser + audit evidence
  only — no runtime consumer of the overrides exists (ambient traffic
  is F10, opponents F15, police F20), so semantics (does the original
  honor `[Speed Limit]`/`[Density]`/spawns as authored) stay
  unverified — honestly disclosed in the ledger. Police/opponent tail
  columns, `[Traffic Lights]` shape, `[Hookmen]` rows and the `_p`
  variant's role are unknown and preserved raw. No rendered/audio/
  network evidence this iteration.
- Unresolved blockers or discovered regressions: none introduced.
  `geometry/thing.pkg`'s scan failure predates this work (unchanged
  pkg parser); noted so it is not mistaken for a regression.
- Next smallest useful action: F09-B (directed route queries,
  elevation-aware sampling, debug overlays — both inputs now parse).
  Alternates: F13-A (checkpoint rules; deps F02-B/F11-B are
  candidates, not checked) or F03-A (prop audit).

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
