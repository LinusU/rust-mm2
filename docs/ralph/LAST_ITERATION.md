# Last implementation iteration

- Task ID and title: F11-A — race event catalog and shared runtime,
  first split: `EventRef`-resolving VFS event catalog over the
  already-parsed `mm*data.csv` tables plus `.opp`/waypoint/crash-data/
  rewards parsing (exactly the split the previous handoff predicted).
- Starting commit and resulting commit: started at
  `745c0b4e441132511c6c61465847613301edfe71` (clean tree, branch
  `ralph/night`); result = this commit.
- Production code changed:
  - `crates/mm2_formats/src/racefiles.rs` (new): the `race/<city>`
    filename classifier moved here verbatim from
    `tools/mm2_inspect/src/inventory.rs` so the tool and the catalog
    share one grammar — `RaceFileKind` (aimap/aimap_p/pathset/waypoints/
    startpoints/opp/crash-data/rewards/…), stem extraction and
    per-record difficulty suffixes (`-a`/`-p`). `inventory.rs` now
    imports it (its ~100-line private copy deleted).
  - `crates/mm2_formats/src/waypoints.rs` (new): `WaypointFile`
    (`x,y,z,a,<radius|poly count>,<frame rate|frane rate>,…` — both
    authored header variants accepted, width label retained) and
    `StartPointsFile` (headerless numeric `*_strtpnts`).
  - `crates/mm2_formats/src/opp.rs` (new): 9-column opponent CSV —
    position, brake, forward/side offset, target speed, speed/side
    start.
  - `crates/mm2_formats/src/crashdata.rs` (new): Crash Course
    `crash<N>data.csv`/`_p` tables. Retail headers are inconsistent
    (`AmbDenisty` typo, named tail columns `Misc`/`cornerspeed`/
    `chkflags`/`numopp`, london `crash8data.csv` omits the `Filename`
    label while rows still carry it), so the parser anchors on the
    `Event,Checkpoints,TimeLimit` prefix, keeps fixed row positions and
    reports header anomalies as diagnostics rather than rejecting.
  - `crates/mm2_formats/src/rewards.rs` (new): `*_rewards.csv` —
    per-crash-event unlock rows (vehicle id + paint variant + message)
    and Half/All milestone rows per mode.
  - `crates/mm2_game/src/events.rs` (new): `EventCatalog::scan(vfs,
    city)` — classifies every `race/<city>` record via `racefiles`,
    parses the four `mm*data.csv` metadata tables into `EventRef`-keyed
    entries (stable table/index identity, Amateur + Professional rows),
    attaches dependency records by stem (aimap/aimap_p/pathset/
    waypoints/startpoints/per-difficulty opp), resolves Crash Course
    `Filename` links to whole linked stems (so `follow.csv` also pulls
    `follow-0.opp`), links event + milestone rewards, keeps unclaimed
    stems as explicit extras, and reports ready/incomplete status with
    failed references. Missing/malformed data stays visible in the
    expected denominators.
  - `tools/mm2_inspect`: new `events <dir> [--city <city>] [--strict]`
    command — per-city metadata table row counts/diagnostics, event
    index/stem/description/status, compact dependency tags, failed
    refs, rewards, extras, milestone rewards. Strict fails on empty
    catalogs, missing/malformed tables, incomplete events or catalog
    failures. `mm2_game` added as a dependency of the tool.
- Tests added/changed and why:
  - Parser unit tests in each new `mm2_formats` module: valid files,
    wrong headers, malformed rows, `AmbDenisty`/omitted-`Filename`/
    named-tail crash variants, fractional density, difficulty suffixes.
  - `crates/mm2_game/tests/events.rs` (new, synthetic VFS via
    tempfile): catalog resolves metadata rows to stems, attaches
    dependency records and difficulty variants, keeps a missing
    dependency as a failed reference on an incomplete event, links
    rewards + milestones, and preserves unclaimed files as extras.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features --
    -D warnings` — PASS (initial run failed on `manual_contains`,
    `excessive_precision` and a let-binding return; fixed in code).
  - `cargo test --locked --workspace` — PASS, 0 failures (23 test
    result groups).
  - `cargo run -p mm2_inspect -- events /Users/linus/coding/rust-mm2/
    retail --strict` — exit 0: london 45/45 events ready (12 race +
    10 blitz + 10 circuit + 13 crash; 33 extras listed incl.
    race12/13, roam, london_bridge_*, multicop), sf 45/45 ready (32
    extras incl. stunt0, r0, cir1–9 startpoint records); all crash
    rewards + 6 milestone rewards per city resolved; no failed refs.
- Acceptance IDs satisfied / still open: F11-AC01 evidence (full
  expected roster listed with status and failed refs; strict fails
  empty catalogs) and F11-AC06 evidence (a selected retail event —
  e.g. `sf crash7 midtrm2` — resolves with its authored dependencies:
  aimap/aimap_p, both `data:` variants, linked waypoint records and
  `opp` sibling) are candidate-level pending external check. AC02–AC05
  are runtime lifecycle criteria owned by F11-B and remain open — no
  checkpoints are swept, no countdown runs, no results emit, no props
  are spawned.
- Evidence files: none committed; no captures made.
- Stock data/GPU/audio/network limitations: unchanged — no audio or
  networking code; GPU not exercised this iteration. Catalog/parsing
  evidence is real retail data, but no runtime race exists yet: record
  semantics (`.aimap` sections, `.pathset` PTH1 blobs, waypoint `a`
  column) are carried uninterpreted where unverified, not claimed as
  original behavior.
- Unresolved blockers or discovered regressions: none. Carry-overs:
  `circuit11` remains partial on retail (opp/pathset only — listed as
  extras, not table events); `Session` Countdown/Results/Paused phases
  still unreachable; menus (F17), profiles (F16), opponent AI (F15)
  absent.
- Next smallest useful action: F11-B — shared race lifecycle over this
  catalog: countdown + input lock, swept checkpoint triggers from
  parsed waypoints/widths, participant progress, restart/cleanup,
  once-only `SessionResult` emission. If the runner requires checked
  deps first, F03-A or F09-A are independent ready alternates.

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
