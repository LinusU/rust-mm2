# Last implementation iteration

- Task ID and title: F09-B.1 — directed navigation graph over the BAI
  road network: lane sampling, elevation-aware nearest-lane queries,
  legal turn exits, seeded route choice, bounded deterministic routing,
  per-consumer route cursors, a `mm2_content` VFS loader and a
  `mm2-inspect nav` audit. Debug rendering overlays deliberately stay
  in F09-B.2; AIMAP override *application* stays in F09-B.2/F10+.
- Starting commit and resulting commits: started at
  `a5205d1917b5367378d7aca89324b96e51f48bc5` (clean tree, branch
  `ralph/night`, F09-A.2 externally checked); result = the commit on
  top of it.
- Why this slice: PLAN named F09-B as the next slice and both its
  inputs (BAI + aimap) now parse. F09-B is broad, so it split: B.1 is
  the query graph + audit; B.2 keeps debug overlays and deeper
  original-data validation. Parent F09 acceptance stays whole.
- Production code changed:
  - `crates/mm2_game/src/nav.rs` (new): `NavGraph::build(&Bai)` —
    directed arcs per road direction (right-side curves travel with
    the sections, left-side against; London's left-hand driving is
    baked into authored BAI data per the Adzima GDMag article, so no
    per-city handedness flag), lane records for vehicle/sidewalk/
    tram/train curves with measured signed lateral offset, resolved
    end→intersection connectivity with `UnresolvedEnd` degradation to
    dead ends, turn connections (no U-turns), union-find components,
    an XZ bucket grid, and `NavStats`. Queries: `sample_lane`
    (travel-direction position+tangent), `nearest_lane` (full-3D
    distance — bridge decks cannot win over ground lanes by horizontal
    proximity; `LaneQuery::rooms` filters by PSDL room for stacked
    geometry), `exits`/`legal_exits` (documented lane-position rules:
    inner lane toward centre, outer kerb-side, middle straight,
    one-ways any exit; geometric turn classification plus authored
    `ccw_delta` carried for research), `choose_exit` (seeded
    `NavRng` — SplitMix64-seeded xorshift, deterministic on every
    platform), `route` (bounded A* with `NoStartLane`/`NoGoalLane`/
    `Unreachable`/`ExpansionLimit` failures and `closed_roads` for
    future aimap `[Exceptions]`), `cursor`/`advance_cursor`/
    `cursor_sample` (per-consumer state over the immutable graph).
    Issues (`NavIssue`) report unresolved ends, degenerate lanes,
    recomputed distances and non-routable roads — reported, never
    repaired.
  - `crates/mm2_game/src/lib.rs`: `pub mod nav`.
  - `crates/mm2_content/src/nav.rs` (new): `load_nav_graph(vfs, city)`
    — resolves `city/<name>.bai` through the VFS, parses `Bai`,
    builds `NavGraph`, returns `NavBuild`; structured `NavLoadError`
    (`Resolve`/`Read`/`Parse`).
  - `crates/mm2_content/src/lib.rs`: `pub mod nav`.
  - `tools/mm2_inspect/src/main.rs`: new `nav` subcommand
    (`[--city] [--strict] [--route from:to]`) — expected denominator
    `city/{london,sf}.bai`, extras audited as unsupported-on-failure;
    prints per-city stats + every `NavIssue`; `--route` snaps both
    road midpoints to the nearest routable lane (any direction) and
    prints the arc sequence + length; `--strict` exits nonzero on
    failures/issues.
- Research/documentation: `docs/research/bai.md` gained a
  "Lane direction and the navigation graph" section — measured
  `x_axis ≈ tangent × up` (SF 1719/1723, London 2504/2508 sections,
  0 opposite), right-side curves at +x / left-side at −x in the large
  majority, `edgeDistances` proven *not* a lane ordering (profiles
  like `[7.5, 2.5, 2.5, 7.5]` on one-way sides → lanes rank by
  measured offset), sidewalk curves reliably last in the shared
  curve arrays, and the documented turn/London rules sourced to the
  Adzima article. Ledger: WLD-9/10/11 added, UNK-12 updated (graph
  builds; runtime consumption still unverified), UNK-19 added
  (`edgeDistances` semantics). `docs/architecture.md` records the
  nav contract in `mm2_game` and the loader in `mm2_content`.
- Retail findings reported by the audit (not repaired):
  - London: 540 roads (166 one-way) → 606 arcs, 1141 vehicle + 1080
    sidewalk + 28 rail lanes, 328 intersections, 0 dead ends, 1
    connected component.
  - SF: 379 roads (96 one-way) → 618 arcs, 1212 + 758 + 42 lanes,
    214 intersections, 1 dead end, 1 component.
  - 176 roads carry no routable vehicle lanes (pedestrian/special/
    disabled or curve-less) — listed individually as issues.
  - Route probes succeed on both cities, e.g. `13→50` on SF = 4 steps
    (576 m) `13- → 12- → 130+ → 50-`; London `13→50` = 30 steps
    (1948 m).
  - A self-review fix mattered on real data: vehicle curves on
    `PedestriansOnly`/`Disabled` sides were briefly stamped with the
    next arc's id; after the fix London's routable-lane count dropped
    1381→1141 (SF 1235→1212) — the honest counts.
- Tests added and why (`mm2_game/tests/nav.rs`, 20): straight
  two-way roads produce two oppositely directed arcs (AC01);
  curved roads sample correctly; one-way roads (right-only and
  left-only authored forms) produce a single arc (AC01); 4-way and
  T intersections wire legal exits, lane position governs legal
  turns, middle lanes go straight, one-ways take any exit, U-turns
  excluded; unresolved ends degrade to dead ends with issues (AC05
  input-honesty); stacked bridge/ground lanes snap by 3D distance
  not horizontal proximity and room hints disambiguate (AC03);
  routing fails `Unreachable`/`ExpansionLimit`/`NoStartLane`/
  `NoGoalLane` with bounds honored (AC05); `closed_roads` blocks
  transit but not endpoints; seeded exit choice is deterministic;
  two `RouteCursor`s advance independently over one graph (AC06);
  degenerate lanes drop out with issues.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features
    -- -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, 29 test-result groups,
    0 failures (incl. 20 new nav tests).
  - `mm2-inspect nav /Users/linus/coding/rust-mm2/retail` — exit 0:
    both expected BAIs build (stats above), 0 failures, 176 issues.
  - `mm2-inspect nav <retail> --route 13:50` — both cities route
    (SF 4 steps/576 m, London 30 steps/1948 m).
  - `mm2-inspect nav <retail> --strict` — exit 2 on the 176
    non-routable-road issues (honest report, not hidden).
- Acceptance IDs satisfied / still open:
  - F09-AC01 — synthetic straight/curved/one-way/intersection/
    dead-end/multilevel cases produce legal directed routes:
    SATISFIED at synthetic level (20 tests).
  - F09-AC03 — bridge and ground lanes do not connect by horizontal
    proximity: SATISFIED at synthetic level (3D nearest-lane + room
    hints; components only join through authored intersections).
  - F09-AC05 — bounded search with specific failures: SATISFIED
    (Unreachable/ExpansionLimit/NoStartLane/NoGoalLane).
  - F09-AC06 — two consumers share the graph without disturbing each
    other: SATISFIED (immutable graph + per-consumer cursors).
  - F09-AC02 — already satisfied by A.1/A.2; this slice adds
    nav-level issue reporting on top.
  - F09-AC04 — debug overlays match geometry/direction: OPEN, F09-B.2.
- Stock data/GPU/audio/network limitations: audit + synthetic
  evidence only — no runtime consumer drives on the graph yet
  (traffic F10, opponents F15, police F20), so whether the original
  honors the documented lane rules at runtime stays unverified
  (UNK-12). Turn *classification* is geometric, not the original's
  CCW index arithmetic (only documented for 4-ways); `ccw_delta` is
  carried for future reconciliation. `edgeDistances` meaning unknown
  (UNK-19). AIMAP `[Exceptions]`/`[Speed Limit]` are not yet applied
  to routing (hook exists via `closed_roads`). No rendered/audio/
  network evidence this iteration.
- Unresolved blockers or discovered regressions: none introduced.
- Next smallest useful action: F09-B.2 (debug-render overlays over
  imported geometry + deeper retail route validation against authored
  data). Alternates: F13-A (checkpoint rules; deps F02-B/F11-B are
  candidates, not checked) or F03-A (prop audit).

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
