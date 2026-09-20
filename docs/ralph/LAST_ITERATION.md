# Last implementation iteration

- Task ID and title: F03-B continuation — consume
  `city/<city>/props.pathset` and stamp ambient prop rows (trees,
  lamps, barricades) through the shared `PropCache`.
- Starting commit and resulting commits: started at
  `9ef2534df59241926a144899a0a29146836ac05d` (clean tree, branch
  `ralph/night`, F03-A.1 externally checked); result =
  `2ff2582` "Stamp ambient props from city props.pathset files
  (F03-B)" plus this handoff note.
- Why this slice: PLAN named F13-A / F03-A.2 / F09-C, but the checked
  F03-A.1 parser was written explicitly for this consumer and the
  previous report teed it up ("F03-B prop instantiation can now
  consume `mm2_formats::pathset`"). It is the highest-value remaining
  F03 work — visible city population — versus another audit slice.
  F13-A's substance is largely already implemented by F11-B.2/F12
  (checkpoint events load, run AnyOrder, finish arms, elapsed clock
  shows); its remainder is opponent/cop scope (F15/F20).
- Production code changed:
  - `crates/mm2_formats/src/pathset.rs`: `Path::asset_name()` — the
    shared name classifier (`PREFIX:` event-state decorations
    stripped to the last `:`-segment, `PATHnn` route labels and empty
    tails → `None`).
  - `tools/mm2_inspect/src/main.rs`: pathset audit's private
    label/prefix helpers replaced by `asset_name()`; audit output on
    retail is byte-identical to the checked baseline (98/101, 3
    failures, 6 issues, `--strict` exit 2).
  - `crates/mm2_app/src/city.rs`:
    - `stamped_transforms(path)` — pure expansion per documented kind
      (R3): `Points` one unrotated prop/vertex; `Directed` one prop
      per pair yawed about Y so local +X runs to the second point
      (INST heading convention — the axis is undocumented, inferred);
      `LineStrip` each segment filled at `spacing` intervals from its
      start (t < len; shared vertex stamped by the next segment's
      t=0), final vertex caps the row, stamps yaw along the segment.
      Zero spacing → per-vertex; lone vertex stamps once; odd
      `Directed` tail / unknown kinds stamp nothing.
    - `spawn_prop` helper shared by INST and pathset placements.
    - `load_city`: `PropCache` hoisted so INST + pathset share model
      builds; consumes `<dir>/<stem>/props.pathset` next to the PSDL;
      names resolving to `texture/<n>.*` instead of a PKG are counted
      as `pathset_decal_paths` (SF's 31 `r4i_rails_f` cable-car rail
      decals inside `props.pathset`), not failures; `PATHnn` labels
      skipped; entities named `pathset-<name>-<path>-<stamp>`
      (deterministic).
    - `CityReport`: `pathset_props_spawned` / `pathset_decal_paths` /
      `pathset_props_failed` + Display.
- Semantics deliberately NOT claimed original (UNK-20): which local
  axis directed yaw maps (INST convention assumed), per-segment vs
  continuous spacing, the end cap, zero-spacing meaning, decal strip
  width/orientation. Decals, `audio_pathsets/`, `race/*.pathset`
  overlays and `city/phys/`+`bak/` dev sets remain unconsumed on
  purpose.
- Tests added and why (11, all in `city::tests`, pure Mat4 checks):
  per-vertex `Points`; `Directed` pair yaw + degenerate-offset
  fallback + odd-tail drop; `LineStrip` spacing positions + per-segment
  restart + shared-vertex single stamp + end cap + yaw per segment;
  zero-spacing per-vertex; zero-length segment skip; lone-vertex strip;
  empty/unknown-kind no-ops; `asset_name` prefix/label cases.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo fmt --all -- --check` — PASS (after `cargo fmt`).
  - `cargo clippy --locked --workspace --all-targets --all-features
    -- -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, all groups, 0 failures.
  - `mm2 --mm2-path <retail> --city london --headless` — `status=pass`;
    report `1188 pathset props (0 decal paths, 0 failed)`.
  - `mm2 --mm2-path <retail> --city sf --headless` — `status=pass`;
    `925 pathset props (31 decal paths, 0 failed)`.
  - `mm2 --mm2-path <retail> --city sf --cam=-1790,45,-1150,180,-8
    --frames 90 --screenshot` — `status=pass`, ~3.9 MB PNG
    (`screenshots/pathset-sf-lamps.png`, gitignored): the stamped
    `sp_lightstreet_rt_f` lamp row draws evenly spaced along the road.
  - `mm2 --mm2-path <retail> --city london --cam=-469,8,-290,180,-10
    --frames 90 --screenshot` — `status=pass`, ~5.6 MB PNG
    (`screenshots/pathset-london-trees.png`): stamped `sp_tree1_s`
    bushes in the park.
  - `mm2-inspect pathset <retail>` / `--strict` / `--city london` —
    identical to checked baseline (98/101, exit 2 strict, 43/46 + 3
    failures london).
- Acceptance IDs satisfied / still open:
  - F03-AC01 (synthetic pathset tests: spacing, rotation, scale,
    repeated/shared models, deterministic IDs): ADVANCED — expansion
    tests pin spacing/yaw/edge cases; nonunit scale is N/A (pathsets
    carry no scale); shared models covered by the hoisted `PropCache`
    (INST+pathset share builds); IDs deterministic by index naming.
    Rendered retail evidence for stamp rows recorded above.
  - F03-AC02 (sampled original locations contain expected props):
    PARTIAL — lamp rows + park bushes render at authored locations;
    systematic spot validation remains F03-C.
  - F03-AC03/AC04/AC05: open — decal collision policy, race overlay
    add/remove (race pathsets unconsumed), mod-override pathset
    evidence.
  - F03-AC06: still open (`.cpvs`/`.ldef` + decal family coverage).
- Stock data/GPU/audio/network limitations: ran on the real retail
  install through the VFS; rendered evidence on this machine's GPU.
  Collision classification is the same interim static-trimesh policy
  INST props use — trees/lamps are solid until F04 reclassifies
  breakables; SF's 31 rail decals are reported unhandled, not hidden.
- Unresolved blockers or discovered regressions: none introduced.
- Next smallest useful action: F03-A.2 scope decision (`.cpvs`/`.ldef`
  vs F18, embedded PSDL props) or decal stamping (needs a strip-width
  rule — research first) or `race/*.pathset` event-overlay consumption
  under F03-AC04. F13-A/F09-C remain ready alternatives.

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
