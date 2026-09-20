# Last implementation iteration

- Task ID and title: F03-B.2 — event-scoped `race/<city>/<stem>.pathset`
  overlays, the F03-AC04 leg named in the plan's next-slice list.
- Starting commit and resulting commits: started at
  `f6e151fc5b688ed2c891fbcd2709ad6943198eaf` (clean tree, branch
  `ralph/night`; the F03-B expansion-bound repair had just passed
  external gates + review, verdict pass, no blocking findings).
  Result = one feature commit plus this handoff note.
- Why this slice: no failing gate/review finding to repair, so the
  highest-value ready task from the plan's list. Race overlays advance
  F03-AC04 directly; decal stamping needs a strip-width rule
  (research first) and F13-A's deps are still candidates.
- What changed:
  - `crates/mm2_app/src/city.rs`
    - `PathsetStampReport` + shared `stamp_pathset` — the
      `props.pathset` loop in `load_city` extracted so both consumers
      run one classification: prop paths stamp through `PropCache`;
      `PATHnn` names → `label_paths`; `giz_*` names →
      `animated_paths` (movable objects — a static trimesh collider
      is the wrong class, so they are counted, not stamped);
      texture-resolving names → `decal_paths`; dead refs →
      `unresolved_paths`; per-file `MAX_PATHSET_STAMPS` budget and
      `Pathset::validate()` issues threaded through as before.
    - `EventPathsetReport` + `spawn_event_pathsets(commands, vfs,
      logicals, meshes, images, materials, owner)` — reads/parses/
      stamps each `.pathset` logical with `event-pathset-*` entity
      names through a fresh `PropCache` (the city's own cache is
      local to `load_city`); `failed_files` for unreadable/
      unparseable records (warned, non-fatal — the catalog treats
      `.pathset` as an optional overlay record); animated-texture
      components and `missing_textures`/`missing_prims` reported
      like `load_city`.
  - `crates/mm2_app/src/race.rs`: `event_race_setup` now returns
    `EventSetup { definition, pathsets }` — the event's
    `RaceFileKind::Pathset` record logicals straight from the
    resolved `CatalogEvent` (single catalog scan).
  - `crates/mm2_app/src/session.rs`: `load_session_world` calls
    `spawn_event_pathsets` after a successful event resolve —
    `event pathset overlay stamped files=… stamped=… labels=…
    animated=… decals=… unresolved=… capped=… issues=…
    failed_files=…`.
- Tests added (3, in `crates/mm2_app/tests/event.rs`, production-path
  harness): `event_pathset_overlay_spawns_session_owned_props` (4
  stamps × part+collider = 8 `event-pathset-*` `CityEntity`s, all
  `SessionEntity(1)`), `restarting_the_event_respawns_its_overlay_once`
  (AC04 — identical gen-2 count, zero gen-1 survivors),
  `event_pathset_classification_counts_every_path` (strip stamps;
  PATHnn label/giz_/decal/dead-ref each counted; undocumented kind 9
  is a validate issue stamping nothing; truncated file →
  `failed_files`). PTH1 + PKG3 fixtures added locally.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo fmt --all -- --check` — PASS (after one auto-format).
  - `cargo clippy --locked --workspace --all-targets --all-features
    -- -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, all 31 groups,
    0 failures (event.rs 11/11).
  - `mm2 --mm2-path <retail> --city london --event circuit:0
    --headless` — `status=pass`, `overlay files=1 stamped=181
    labels=0 animated=0 decals=0 unresolved=0 capped=0 issues=0`,
    `race=Running cp=1/6`.
  - `mm2 --mm2-path <retail> --city london --event checkpoint:6
    --headless` — `status=pass`, `stamped=256` (`race6.pathset`).
  - `mm2 --mm2-path <retail> --city sf --event circuit:0 --headless`
    — `status=pass`, `stamped=85` (`sp=0` → one prop per vertex).
  - `mm2 --mm2-path <retail> --city london --headless` — ambient
    counts unchanged: `1188 pathset props (0 decal …)`.
  - `mm2 --mm2-path <retail> --city sf --headless` — `925 pathset
    props (31 decal …)`; ends wheels=0/4 airborne again — the same
    known airborne end state as the checked baseline, not a
    regression.
- Acceptance IDs satisfied / still open:
  - F03-AC04: ADVANCED — synthetic restart test proves adds/removes
    only event objects with no duplicates; retail events stamp real
    authored overlays. "Entering and exiting a race twice" via menu
    flow is F17 territory; the session restart path is the exercised
    mechanism.
  - F03-AC01: ADVANCED — shared stamping now also exercised for
    event files.
  - F03-AC02/AC03/AC05/AC06: unchanged — open (spot validation,
    ramp/decal collision, mod-override evidence, full source-family
    audit → F03-C).
- Scope decisions recorded: `giz_*` animated objects classified and
  counted, not stamped (wrong physics class as statics; needs the
  animated-object feature — bridges/ferries/parked cars).
  `<city>_<object>.pathset` ambient sets and `<object>_<event>`
  overrides stay unconsumed extras — all `giz_*`/`PATHnn`/`sp_pcar*`
  on retail, nothing stampable lost on reachable events. Crash-course
  stem pathsets are claimed but unreachable (CrashCourse setup
  rejects first, F21). `PREFIX:` names strip before classifying —
  `OPEN:giz_*` still counts as animated.
- Stock data/GPU/audio/network limitations: ran on the real retail
  install through the VFS; no rendered capture this iteration
  (stamped transforms are the same pipeline the checked city pathset
  code emits). No audio/network code exists.
- Unresolved blockers or discovered regressions: none known.
- Next smallest useful action: F03-A.2 scope decision (`.cpvs`,
  `.ldef`, embedded PSDL props), decal stamping research, or
  F13-A/F09-C.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
