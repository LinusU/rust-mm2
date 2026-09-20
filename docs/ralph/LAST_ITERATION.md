# Last implementation iteration

- Task ID and title: F09-C.1 — directed route-constraint validation on
  the shared navigation graph (the F09-C "validate both city graphs
  and route constraints without inventing missing connectivity" leg).
- Starting commit and resulting commits: started at
  `3e514948486839d3fa6f15a8e38435c21f73df37` (clean tree, branch
  `ralph/night`; F13-A.1 passed external gates + review).
- Why this slice: the plan's named candidate (F13-A remainder or
  F09-C). Every unblocked F13-A leg is parked on F05/F16/F18 scope
  except AC06's playability matrix, which the scripted driver cannot
  yet evidence (4/64 finishes). F09-C's machinery (B.1 graph, B.2
  overrides/overlay) was implemented but its core claim — that routing
  exercises only authored connectivity — had never been *measured* on
  retail data.
- What landed (code):
  - `mm2_game::nav::NavGraph::reachable_arcs(start, closed_roads)` —
    a bounded BFS over authored `exits` only: the walk always
    terminates and can never leave authored connectivity; closed
    roads are never entered through a turn while a closed `start`
    still expands — the same rule `route` applies.
  - `mm2-inspect nav --routes <n>` — per city: a directed-reachability
    census (arcs reaching the whole graph, reach-1 arcs annotated
    `dead end` vs `no legal continuation`, unreachable ordered pairs,
    the smallest sources) plus `n` seeded `route_roads` probes between
    routable roads. Every returned route is checked for chain
    consistency — endpoints land on the asked roads, consecutive steps
    share a turn, interior arcs never sit on a closed road. Failures
    print named pairs; expansion-limit hits and violations count
    toward `--strict`.
  - `mm2-inspect nav --aimap <logical>` — substitutes any aimap's
    routing overrides for the probes/census (event `[Exceptions]`),
    replacing the default `city/<c>.aimap`. An explicit path that does
    not resolve is a hard error (exit 2); a malformed file is reported
    non-fatally, matching the existing convention.
  - `NavOptions` struct replaces `nav()`'s growing flag list (clippy
    `too_many_arguments`).
- Tests added (in `crates/mm2_game/tests/nav.rs`, synthetic fixtures):
  - `reachable_arcs_follow_only_authored_turns` — a junction entry arc
    reaches exactly the departing arcs of the other arms; the U-turn
    exclusion holds in reachability; a dead-end departure reaches
    only itself.
  - `reachable_arcs_shrink_under_road_closures` — the three-road chain:
    open reach is the full chain; closing the middle road severs it at
    the turn in both directions while a start already on the closed
    road still expands.
- Retail evidence (install `/Users/linus/coding/rust-mm2/retail`,
  `fnv1a64:e91e6cd4b2ae30d9`, `target/debug/mm2-inspect`):
  - `nav <retail> --routes 512`: London is *strongly* connected —
    all 606 arcs reach all others, 512/512 probes ok. SF is not:
    4927 ordered arc pairs (~1.3%) unreachable — road 0- dead-ends
    (WLD-11's one dead end) and six more arcs reach junctions no arm
    legally departs (one-way traps: 92/94/97/99- reach 1, 102/111-
    reach 2, 1- reaches 617); 505/512 probes ok, the 7 unreachable
    are exactly the trap sources, 0 chain violations.
  - `--aimap race/london/blitz0.aimap` (8 `[Exceptions]` closures):
    unreachable pairs rise to 15016 — the closures isolate
    115-/116-/368+/375+ (reach 1); 24/512 probes unreachable; named
    probe `270→108` routes 19 steps/1428 m open → `Unreachable`
    closed; 0 closed-road traversals.
  - `--aimap race/sf/blitz0.aimap` (3 closures): 7977 unreachable
    pairs, 11/512 probes unreachable, 0 traversals.
  - `nav --city sf --route 13:50 --aimap race/sf/blitz0.aimap`:
    the single-probe path honours the event aimap too.
- What this proves / does not prove:
  - Proves: routing on both retail graphs uses only authored turns —
    unreachable pairs are real authored constraints (one-way traps,
    dead ends, event closures), not invented or missing links; event
    `[Exceptions]` demonstrably bind at query time on real content;
    the audit reports the structure honestly including named failures.
  - Does not prove: the original runtime consumes lanes/closures the
    same way (UNK-12 stays open — this validates *our* constraints on
    original data); overlay-vs-geometry fidelity beyond B.2's captures
    (AC04 evidence is the rendered screenshots); traffic/ped use of
    the graph (no consumers exist yet — F10/F19).
- Classification: the directed census numbers are verified_original
  measurements of authored data (WLD-11 extended); `reachable_arcs`
  and the probe/chain checks are implementation choice. The "closed
  road = zero-density exception" rule remains inferred semantics
  (WLD-7) — this slice shows the binding works, not that the original
  used it identically.
- Commands actually run and results:
  - `cargo test -p mm2_game --test nav` — 24/24 pass (22 existing +
    2 new).
  - `cargo build -p mm2_inspect` — clean.
  - Retail runs as recorded above.
  - Gates at the candidate commit: `cargo fmt --all -- --check`
    PASS; `cargo clippy --locked --workspace --all-targets
    --all-features -- -D warnings` PASS; `cargo test --locked
    --workspace` PASS — all 32 groups, 0 failures.
- Acceptance IDs satisfied / still open: F09-AC05 (bounded routing,
  specific failures) exercised on real content at scale — 512 seeded
  probes all terminate with named reasons; AC02/AC03 already held;
  AC04 stays at B.2's rendered-capture level; AC06 stays synthetic
  (no second real consumer exists — F10/F19). F09-C parent marked
  implemented; not externally checked yet.
- Deferred deliberately: nothing new — the slice was scoped to route
  constraints. Still deferred from F09: nothing queued (overlays,
  audits, probes, census all exist).
- Stock data/GPU/audio/network limitations: retail evidence is
  inspect-tool output on real VFS data; no GPU capture, no
  original-executable comparison, no runtime consumer driving the
  graph.
- Unresolved blockers or discovered regressions: none known.
- Next smallest useful action: F13-A remainder (AC06 coverage matrix
  is the only unblocked leg), F14-A (circuit/lap binding — Ordered
  rule and NumLaps already wire the runtime; the ledger legs remain),
  or F11-C (event-catalog validation slice).

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
