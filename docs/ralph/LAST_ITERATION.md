# Last iteration — F03-C.3 kerb-height repair + vertical placement audit

Iteration 47 on `ralph/night`, continuing from `1f914e4` (the F18-A.3
authored fog tables candidate — external review verdict **pass**;
`1f914e4` itself advanced over the operator report 4 PLAN entry only).
Operator report 4's item 4 is the highest-value ready repair: London
litter bins buried mid-body in raised pavements. The report's own lead
was the right one — the PSDL kerb datum is the defect, confirmed on
authored data rather than tuned. F03-C stays **implemented** — sampled
original-location evidence (UNK-20 density), race cleanup and mod
replacement remain open.

## Root cause (measured, not guessed)

The `Room_attributes` format doc states road vertices sit "0.15 units
below the sidewalk vertices". Retail measurement agrees exactly:

- `sw.y − road.y == 0.15` on 943/1087 london + 1205/1321 sf
  `RoadWithSidewalks` sections (the rest are authored flush
  ramps/driveways — no lift to apply, and none is).
- `top.y − ground.y == 0.15` on all 5 880 london + 5 665 sf
  `SidewalkStrip` pairs.

`walk_prop_rules` lerped straight off the kerb-foot (road-level) chain
while `mm2_app::city::emit_sidewalk` raises that chain by the same 0.15
to draw/collide the pavement — kerb-side stamps sat ≈0.135 m under the
rendered surface. The operator's screenshots show litter bins buried to
their labels.

## What changed

- `mm2_game::props`: `SIDEWALK_KERB_LIFT` (0.15, shared). `KerbStrip`
  gains `lift` — set for `RoadWithSidewalks`/`DividedRoad`/
  `SidewalkStrip`, `0` for `RoadNoSidewalks` walkways (their authored
  edge *is* the surface) — applied to the resolved, possibly-reversed
  kerb chain before `strip_at` lerps kerb→outer. XZ, arc-length spacing
  and facing are untouched; authored-flush outers keep their height.
- `mm2_app::city`: local `SIDEWALK_LIFT` is now an alias of the shared
  constant — renderer, collider, stamper and audit cannot drift apart.
- `mm2_game::props`: `Carriageway` gains `band: SurfaceBand`
  (`Driving`/`SidewalkTop`/`FloorFan`); `carriageways()` is unchanged
  semantics (driving regions, tagged) and the new
  `walkable_surfaces()` adds every standable band — lifted sidewalk
  tops plus mostly-horizontal `Fan`s selected by the renderer's own
  `vertical_facing` `|n.y| ≥ 0.3` test (walls excluded).
- `mm2-inspect placement`: the vertical leg report item 4 asked for.
  A walkable surface standing 0.05–1.0 m above a stamp, past a 0.05 m
  XZ-edge epsilon, counts `sunk` — overpass/storey cases (>1 m) and
  kerb-line grazes excluded, the *deepest* covering surface reported.
  Output gains per-channel `sunk` counts plus a channel×band histogram
  and individual `SunkHit` listings (position, room, band, penetration,
  depth-from-boundary). `Findings` bundles hits/fp_hits/sunk.

## Deferred (kept open deliberately)

- Remaining retail `sunk` findings are authored placements outside the
  repaired channel — london pathset 54 (`sp_lightthames_l`/
  `sp_pilaster_l` rows at 0.2–0.37 m), inst 94 (building origins under
  plaza `Fan` floors). Triaging whether each is authored intent or a
  distinct defect is future work; they are listed, not hidden.
- UNK-20 pathset expansion density and any per-prop authored Y
  convention remain open; this fix covers the sidewalk datum only.
- No original-executable frame comparison (retail binary not runnable).

## Verification (this tree)

- `cargo test -p mm2_game props` — pass. +6 tests: kerb-side stamp
  lands on the lifted top, authored-flush outer preserved,
  `RoadNoSidewalks` walkway keeps authored height, walkable band
  counts, `SidewalkStrip` tops, fan-normal floor/wall split. Existing
  fixtures re-authored to the real convention (sw chains at kerb-top
  height) rather than asserting the old flat bug.
- `cargo test -p mm2_inspect` — pass. +4 tests: sunk measure fires,
  1 m overpass excluded, kerb-line excluded, lowest covering surface
  wins.
- `cargo fmt --all -- --check` — pass.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — pass.
- `cargo test --locked --workspace` — pass, 64 suites / 883 tests /
  0 failures.

## Retail evidence (fingerprinted install `fnv1a64:e91e6cd4b2ae30d9`)

- `mm2-inspect placement <install> --city london` → exit 0. Walkable
  surfaces 6 446 (3 224 Driving + 1 789 SidewalkTop + 1 433 FloorFan).
  **prop-rule `sunk 0/5 118`** — the defect class is gone from the
  channel repaired. pathset 54 + inst 94 authored findings listed for
  triage.
- Before/after captures at the operator's own camera
  `cam -275.6,2.0,160.6,38,-13`: `/tmp/kerb-before.png` shows the
  LITTER bin buried mid-body; `/tmp/kerb-after.png` shows it resting
  on the pavement top. PNGs inspected; not committed (original-content
  captures stay local).

## Ledger / docs

- `docs/original-rules.md`: WLD-25 (PSDL sidewalk height — documented
  + measured verified_original).
- `docs/research/proprules.md`: the Y-semantics bullet resolved — the
  "unverified stamp height" caveat replaced by the measured 0.15 m
  convention, retail sample counts and audit/screenshot evidence.
- `docs/ralph/PLAN.md`: F03-C.3 row; F03-C parent split updated;
  report 4 item 4 annotated implemented/candidate.

## Not done / blockers

- SF sunk numbers not recorded in this note (london audited end-to-end;
  the constant is shared so the same walk applies — sf audit can be
  re-run any time).
- No windowed operator confirmation yet; headless captures stand in.
