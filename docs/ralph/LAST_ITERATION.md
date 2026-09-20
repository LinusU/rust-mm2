# Last implementation iteration

- Task ID and title: F04-A.2 — banger placement-binding research and
  audit: which stamped props (INST/pathset/proprule sources) bind to
  `tune/banger/*.dgbangerdata` records, plus the recovered runtime
  structure from MM2Hook (R4). The research/audit half of F04-A's
  "placement binding + state machine" leg — no runtime breakage was
  implemented.
- Starting commit and resulting commits: started at
  `ad471b0e2825a6cdf3537833e2a9c107afaf1512` (clean tree, branch
  `ralph/night`; F04-A.1 had just passed external gates + review,
  verdict pass, no blocking findings). Result = one feature commit
  plus this handoff note.
- Why this slice: F04-A.2 was the selected continuation — the record↔
  geometry link was verified (WLD-15) but *which placements exercise
  which records* was unknown, and the runtime state machine could not
  be researched from data alone. The binding question turned out to be
  fully decidable from retail data: measure it, don't guess it.
- What changed:
  - `crates/mm2_formats/src/banger.rs` — new `geometry_owner(stem,
    pkg_exists)` helper: the PKG stem a record's geometry lives in
    (fragment → `<base>` when `geometry/<base>.pkg` exists; named → own
    stem or longest `_`-base with a PKG; else own stem = `.mtx` part or
    dead ref). `FnMut` callback so callers can cache lookups.
  - `tools/mm2_inspect/src/bind.rs` (new) + `main.rs` `banger-bind`
    command: audits every stamped-prop source through the same VFS —
    `city/**/*.inst`, `city/**/*.pathset`, `race/<city>/*.pathset`,
    `propdefs*.csv`, `proprules*.csv`, `props*.csv` (incl root-level
    `city/props.csv` group table and `.csv.txt` exports), plus PSDL
    `prop_rule` byte → rule → def → file reachability. Sources are
    classified expected/overlay/extra; expected-but-missing files stay
    in the denominator; dev/backup dirs (`city/phys`, `sfai`,
    `variant`, `sf/bak`) are audited, not filtered. Per file: bound /
    unbound / dead placed-name tallies (dead = no `geometry/<n>.pkg`),
    INST `modifiers` histograms, pathset decal/label/`giz_*`/
    unresolved classification. Reverse check: every banger record's
    `geometry_owner` is tested against the placed-name union —
    `vp*`/`va*` owners count as vehicle-pipeline records, not failures.
    `--city` filters source buckets; `--strict` exits 2 on failures or
    issues.
  - `tools/mm2_inspect/src/inventory.rs` — stale "no dgbangerdata
    consumer exists" note updated (prior-review nit).
  - `docs/research/banger.md` — new "Placement binding" section with
    the verified measurements, new "Recovered runtime structure
    (MM2Hook / R4)" section, narrowed UNK-22 tail.
  - `docs/original-rules.md` — new WLD-16 (placement→banger binding,
    verified_original), UNK-22 narrowed to the remaining unknowns.
- Key verified findings (retail, VFS `fnv1a64:e91e6cd4b2ae30d9`):
  - INST is the static-architecture channel: `city/london.inst` 1997
    placements / 221 names — 0 bound; `city/sf.inst` 3763 / 165 — 0
    bound. Not one of 386 distinct INST names has a banger record.
    `modifiers` is not the `lvlInstance` flag word (low bits are paint
    variants; 0x0100 lands on monuments).
  - `city/*_ai.inst` / `*.sdl_ai.inst` supplements stamp only
    `sp_stop_f` (40–67 instances each, all `modifiers=0x0200`) —
    bound; stop signs are knockable AI-relevant props.
  - `props.pathset`: london 17/17, sf 30/30 prop names bound.
    `propdefs.csv`: 12/12 + 27/27 pkg names bound. `props.csv` groups:
    18/19 london (`sp_bollard_pedsafe_l` dead), 16/16 sf. PSDL
    `prop_rule` reachability: london 17 rule numbers/415 rooms →
    15 defs → 11 files all bound; sf 20/397 → 25 defs → 20 files bound.
    Race overlays: every resolved prop name bound.
  - Reverse coverage (994 records excl `default` + 4 `.#*` backups):
    269 reachable via placements, 562 on 105 `vp*`/`va*` vehicle
    owners (vehicle pipeline), 163 on 87 authored-but-never-placed
    owners (unused `sp_*` variants, `pt_*`/`wpobj_gold` markers,
    `giz_*`, building ornaments).
  - R4 (mm2hook) recovered structure: `dgBangerDataManager::
    AddBangerDataEntry(name, partName)`, `dgUnhitBangerInstance`
    (Y-axis + matrix variants), `dgHitBangerInstance`,
    `dgBangerActive` (physics + `phSleep` + `Target` back-pointer +
    `asParticles` + `Timer`), `dgBangerActiveManager` pool of 32,
    `lvlInstance` `INST_BANGER/STATIC/LANDMARK/VISIBLE` flags. Implied
    dormant→active(pooled)→hit/despawn model documented as such —
    thresholds unverified.
- Tests added: `geometry_owner` cases in `banger.rs` (standalone,
  fragment→base, missing base, longest-base named part, dead ref,
  default) + `classify_source` bucket/kind table in `bind.rs`.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo fmt --all -- --check` — PASS (after auto-format).
  - `cargo clippy --locked --workspace --all-targets --all-features
    -- -D warnings` — PASS (fixed needless-lifetime lint on
    `geometry_owner`).
  - `cargo test --locked --workspace` — PASS, all groups, 0 failures
    (mm2_formats 105/105 incl. new `geometry_owner` tests;
    mm2_inspect 10/10 incl. classify tests).
  - `mm2-inspect banger-bind <retail>` — exit 0: 129 source files,
    3 unsupported (`race/london/{blitz10,blitz11,london_bridge_blitz10}
    .pathset` authored truncations — same 3 the pathset audit flags),
    0 failures, 51 issues (dead placement refs kept in the
    denominator: 45 phys `*_m`/`r_concrete`/`sp_boxfruit_f`… dev-city
    names, `sp_bollard_pedsafe_l`, `prop_sp_barricadeconcr_f`,
    `xcp_banrred_f`).
  - `mm2-inspect banger-bind <retail> --strict` — exit 2 (51 issues).
  - `mm2-inspect banger-bind <retail> --city london` — 50 files,
    london-only unions/reachability, 1 issue.
- Acceptance IDs satisfied / still open:
  - F04-AC01–AC06: all remain OPEN — no runtime activation, state
    machine, fragment spawn, cleanup, reset or replication exists.
    This slice supplies the verified binding + recovered model those
    criteria need.
  - WLD-16: verified — placement binding by name, per channel.
  - UNK-22: narrowed — `ImpulseLimit2` comparison, dormant→active→
    hit/despawn transition conditions, ×32 pool reclaim order,
    `INST_BANGER` acquisition, id namespaces, `type: a`, BirthRule
    timing, fallback selection, fragment `NumParts>0` meaning.
- Scope decisions recorded: audit-only slice — same research-first
  posture as A.1. Vehicle banger records are explicitly not world-
  placement failures (they bind via `partName` in the vehicle
  pipeline). Pathset dead names join the placed union so per-bucket
  dead counts are honest; one issue per distinct dead name per file.
  Expected-file parse failures count as failures; extras/overlays as
  unsupported (consistent with pathset/proprules audits).
- Stock data/GPU/audio/network limitations: all numbers from the real
  retail install through the VFS; no rendered capture needed (no
  rendering change). No audio/network code exists.
- Unresolved blockers or discovered regressions: none known.
- Next smallest useful action: F04-A.3 — dormant/active/hit banger
  state machine on Avian for pathset-stamped props using the verified
  binding (threshold semantics stay UNK-22-provisional), or
  independent ready work: prop-rule stamping research (UNK-21), decal
  stamping, F13-A, F09-C.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
