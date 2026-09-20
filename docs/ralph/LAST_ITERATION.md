# Last implementation iteration

- Task ID and title: F04-A.1 — banger data parser and record↔geometry
  audit (`tune/banger/*.dgbangerdata`): a typed `mm2_formats::banger`
  decoder plus an `mm2-inspect banger` audit, the parser/classification
  half of F04-A "authored breakable-prop behavior" from TASKS.json.
- Starting commit and resulting commits: started at
  `0d0be9ff5d74ee3596b0d58871fc39898bb37a8c` (clean tree, branch
  `ralph/night`; F03-A.2 had just passed external gates + review,
  verdict pass, no blocking findings). Result = one feature commit
  plus this handoff note.
- Why this slice: no failing gate/review finding to repair; F04-A was
  the only queued task whose dependencies (F01-B, F03-B) were both
  checked — the runner selected it. F04-A is broad ("parse/classify
  authored breakable-prop behavior AND define stable state
  transitions"), so it split: A.1 = typed parsing + auditing (this
  iteration, research-first); A.2 = runtime binding/state-machine
  research (queued) — the record↔geometry link is verifiable from data
  (WLD-15) but the breakage state machine is not (UNK-22), and wiring
  Avian impulses on an unverified threshold would guess original
  behavior.
- What changed:
  - `crates/mm2_formats/src/banger.rs` (new) — `BangerData`/`BirthRule`
    typed decoder on the shared `tune` block grammar (no duplicate
    parser). Decodes Size/CG/Mass/Elasticity/Friction/ImpulseLimit2/
    NumParts/AudioId/TexNumber/SpinAxis/Flash/BillFlags/YRadius +
    optional NumGlows/GlowOffset list/ColliderId/CollisionPrim/
    CollisionType; integer fields read the grammar's f64 values so
    odd flag values never round through f32. `BirthRule` decodes the
    full 23-field particle spec. The `asBirthRule` variant
    (`sp_tree1_s_break06`, the only one on retail) is accepted and
    reported as a warning; a truly absent block is `None` →
    `BangerIssue::MissingBirthRule`. `validate()` reports
    `BangerIssue`: non-finite or negative physical values, NumGlows↔
    GlowOffset mismatch, missing birth rule. `stem_role()` classifies
    fallback/`_break<NN>` fragment/named. Unknown field/block names
    accumulate into `warnings` rather than failing.
  - `crates/mm2_formats/src/lib.rs` — `pub mod banger`.
  - `tools/mm2_inspect/src/main.rs` — `banger` command: expected =
    `tune/banger/default.dgbangerdata` (counted even if absent);
    denominator = every discovered `tune/banger/*.dgbangerdata*` incl
    `.#*.1.2` editor backups (parsed, classified `backup`). Each
    record's stem resolves through the VFS: own `geometry/<stem>.pkg`
    → standalone, `geometry/<stem>.mtx` → part, `<base>_break<NN>` →
    `BREAK<NN>` chunk inside `geometry/<base>.pkg` (PKG chunk names
    cached per package), else a longest-base `_` split into a part
    chunk, else dead ref. Standalone `NumParts` is cross-checked
    against the package's distinct BREAK indices; fragment/part
    records carrying `NumParts>0` are flagged (no pkg of their own).
    `--strict` exits 2 on any failure or issue. `scan` recognizes
    `.dgbangerdata`.
  - `docs/research/banger.md` + ledger WLD-15/UNK-22.
  - Removed the temporary `examples/survey_bangers.rs` survey tool;
    its measurements are in the research doc.
- Tests added (11, in the module): retail-shaped record incl. all
  optional fields, wrong root rejection, missing required fields,
  non-integer ID rejection, missing `BirthRule` → issue,
  `asBirthRule` → warning + decode, missing optional fields → `None`,
  glow-count mismatch, negative + non-finite values, stem-role
  classification (default/fragment/named, empty index, non-digit
  index, empty base).
- Commands actually run and results (this machine, macOS arm64):
  - `cargo fmt --all -- --check` — PASS (after one auto-format).
  - `cargo clippy --locked --workspace --all-targets --all-features
    -- -D warnings` — PASS (survey example deleted; it held lint
    violations).
  - `cargo test --locked --workspace` — PASS, all groups, 0 failures
    (mm2_formats lib 104/104 incl. 11 new banger tests).
  - `mm2-inspect banger <retail>` — exit 0: 999/999 parse (1 expected
    fallback + 994 extras + 4 backups, 0 unsupported/failed):
    216 standalone props, 477 named parts, 254 resolved fragments,
    47 dead refs (30 named + 17 fragments). `NumParts` ↔ BREAK-index
    count: 0 mismatches on standalones. 54 issues = 47 dead refs +
    4 fragment `NumParts>0` + 2 `GlowOffset`-without-`NumGlows` +
    1 `asBirthRule`.
  - `mm2-inspect banger <retail> --strict` — exit 2 (54 issues —
    authored anomalies/dead refs, not load failures).
  - `mm2-inspect scan <retail>` — `dgbangerdata 995` parsed (the 4
    `.1.2` backups don't carry the extension), 13,389 total logical.
- Acceptance IDs satisfied / still open:
  - F04-AC01–AC06: all remain OPEN — this slice parses and classifies
    authored data; no runtime activation, breakage, fragment spawn,
    cleanup, reset or replication exists to exercise. The audit gives
    AC03's "authored fallback pieces" their verified data source
    (WLD-15).
  - WLD-15: verified — stem↔geometry naming convention + `NumParts`↔
    BREAK-chunk correspondence.
  - UNK-22: opened — impulse-threshold comparison, state transitions,
    fallback selection, id namespaces, `type: a`, BirthRule timing.
- Scope decisions recorded: no runtime consumer this slice — same
  research-first posture as UNK-21/UNK-20. Fragment records with
  `NumParts>0` (4 on retail, all `sp_tree*` oddities) are flagged as
  issues since no child geometry exists; whether the runtime reads it
  as fragments-of-fragments or inert leftover stays UNK-22.
  `.#*.dgbangerdata.1.2` backups parse and classify `backup` — in the
  denominator, out of geometry checks. `default.dgbangerdata` is the
  one expected record; how the original selects it is unverified.
- Stock data/GPU/audio/network limitations: all audit numbers are
  from the real retail install through the VFS (fingerprint
  `fnv1a64:e91e6cd4b2ae30d9`); no rendered capture needed (no
  rendering change). No audio/network code exists.
- Unresolved blockers or discovered regressions: none known.
- Next smallest useful action: F04-A.2 — which stamped props
  (INST/pathset/proprule) bind to banger records + `ImpulseLimit2`/
  state-transition research (UNK-22), or independent ready work:
  prop-rule stamping research (UNK-21), decal stamping, F13-A, F09-C.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
