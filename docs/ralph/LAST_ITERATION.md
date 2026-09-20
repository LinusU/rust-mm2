# Last implementation iteration

- Task ID and title: F03-A.2 — roadside-prop rule tables
  (`propdefs.csv`, `proprules.csv`, `props.csv`, `geometry/props.csv`):
  typed parsers plus an `mm2-inspect proprules` audit, completing the
  F03-A placement-source inventory named in the plan's next-slice list.
- Starting commit and resulting commits: started at
  `afff57d4e32fef786ea63d42ad4396279a7e6c45` (clean tree, branch
  `ralph/night`; F03-B.2 had just passed external gates + review,
  verdict pass, no blocking findings). Result = one feature commit
  plus this handoff note.
- Why this slice: no failing gate/review finding to repair, so the
  first-listed ready task. It explains the long-parsed-but-unused PSDL
  `prop_rule` byte and leaves only feature-deferred placement sources
  (`.cpvs`/`.ldef` → F18, `audio_pathsets/` → F07/F08). Runtime
  stamping deliberately not attempted — the perimeter-walk semantics
  are unverified (UNK-21), same research-first posture as decals.
- What changed:
  - `crates/mm2_formats/src/proprules.rs` (new) — `PropDefs`,
    `PropRules`, `PropGroups`, `PropLodStats` parsers in the
    `racedata` CSV style (strict header prefix, per-row
    `TableDiagnostic`s, blank-line tolerant). `PropRule::rule_key()`
    splits `n{NN}left`/`n{NN}right` into the PSDL byte value + side.
    `validate()` reports `PropRuleIssue` — duplicate defs/rules/
    entries, def-without-files, nonpositive distance/maxUse, negative
    start, inverted lerp range, non-conforming rule names, empty
    rules, repeated rule props. `props.csv` tolerates the `city/phys/`
    dev copy's mislabeled `name,start` header (labels preserved).
  - `crates/mm2_formats/src/lib.rs` — `pub mod proprules`.
  - `tools/mm2_inspect/src/main.rs` — `proprules` command: expected =
    `city/<stock>/{propdefs,proprules,props}.csv`; denominator = every
    discovered `city/**` `propdefs*`/`proprules*`/`props*` `.csv`/
    `.csv.txt` plus `geometry/props.csv` (same basename, different LOD
    schema, own parser). Cross-checks: rule prop refs → sibling def
    names, def `fileN` + group names → `geometry/<n>.pkg`, LOD rows →
    `geometry/<name>`, nonzero PSDL `prop_rule` bytes → defined rule
    numbers; `--city` restricts to `city/<stem>/`, `--strict` exits 2
    on failures or issues.
  - `docs/research/proprules.md` + ledger WLD-14/UNK-21.
- Tests added (9, in the module): retail-shaped parses, header
  rejection, diagnostic rows, every `PropRuleIssue` variant,
  `rule_key` edge cases (`n0xleft`, `n300left`, `n01`, `left`,
  `n1mid`, empty), mislabeled-dev-header tolerance, LOD dup/numeric.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo fmt --all -- --check` — PASS (after one auto-format).
  - `cargo clippy --locked --workspace --all-targets --all-features
    -- -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, all groups, 0 failures
    (mm2_formats lib 93/93 incl. 9 new proprules tests).
  - `mm2-inspect proprules <retail>` — exit 0: 16/16 files parse
    (6 expected + 10 extras), 0 unsupported, 0 failures, 48 issues:
    41 `city/phys/` `*_m` + 3 phys group dead refs (dev city, same
    names the pathset audit flags), `sp_bollard_pedsafe_l` and
    `va_garbagetruck.pkg` LOD dead refs, 1 room per city referencing
    undefined rule 205. PSDL check: london 415 rule-bearing rooms ↔
    n01–n16, sf 397 ↔ n01–n20 (n14 unused).
  - `mm2-inspect proprules <retail> --strict` — exit 2 (48 issues).
  - `--city london` → 4 files / 2 issues; `--city sf` → 7 / 1.
- Acceptance IDs satisfied / still open:
  - F03-AC06: ADVANCED — the strict placement audit now covers the
    prop-rule source family too (INST + PSDL + pathset + prop rules
    all have parsers and audits); F03-C still owns the end-to-end
    strict claim.
  - F03-AC01/02/03/04/05: unchanged — synthetic stamping tests, spot
    validation, ramp/decal collision, race enter/exit, mod override
    evidence stay open under F03-B/F03-C.
- Scope decisions recorded: no runtime consumer this slice — the
  byte→rule link is verified (WLD-14) but which perimeter edges
  left/right apply to and the start/distance/maxUse/lerp semantics
  are UNK-21. `geometry/props.csv` parsed with its own LOD schema,
  not folded into the group table. `.csv.txt` exports audited as
  extras (parse identically; not game-loaded). `scan`/`inventory`
  unchanged (pathset precedent: dedicated audits carry coverage).
- Stock data/GPU/audio/network limitations: all audit numbers are
  from the real retail install through the VFS; no rendered capture
  needed (no rendering change). No audio/network code exists.
- Unresolved blockers or discovered regressions: none known.
- Next smallest useful action: prop-rule stamping research against
  retail room geometry (UNK-21 — which perimeter edges left/right
  apply to, what `start` measures from), decal stamping research,
  `<object>_<event>` pathset consumers (need animated-object/parked-
  car features), or F13-A/F09-C. F04-A (banger data) is also now
  dependency-ready.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
