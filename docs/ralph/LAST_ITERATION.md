# Last implementation iteration

- Task ID and title: F03-C.1 — lateral prop-placement audit across all
  three channels (operator report 2, item 2: "things in the road").
  The prop-rule channel was already repaired in F03-B.6; INST and
  `props.pathset` were still unverified, and the report asked for all
  three checked independently. Selected over queued feature work per
  the operator-priority policy.
- Starting commit: `563c34ea7c1313ccff9b78cb4f36f81b85dfb490` on
  `ralph/night`; tree was clean.
- Retail install: `/Users/linus/coding/rust-mm2/retail` — read-only,
  audited below.

## What changed

- `crates/mm2_game/src/props.rs`: new shared `path_stamp_sites`
  (Points / Directed-pairs / LineStrip expansion, quarter-metre
  spacing, per-segment restart, final-vertex cap, non-finite skips,
  odd directed tails dropped) under `MAX_PATHSET_STAMPS = 8192`;
  new `carriageways` extraction — drivable-region boundary rings +
  triangulated surfaces from `RoadWithSidewalks`, `DividedRoad`
  (median excluded), `RoadNoSidewalks`, `RoadFan` and `Crosswalk`
  attributes; `SidewalkStrip`/facades/medians excluded.
- `crates/mm2_app/src/city.rs`: `stamped_transforms` now wraps
  `path_stamp_sites` — runtime and audit share one expansion policy;
  the duplicated expansion and local stamp cap were removed.
- `tools/mm2_inspect/src/placement.rs` + `main.rs`: new
  `mm2-inspect placement <install> [--city] [--strict]` — per city it
  parses `city/<c>.inst`, `city/<c>/props.pathset` and
  `propdefs/proprules.csv` + PSDL `prop_rule` bytes, measures every
  stamped position against the carriageway regions (XZ inside a tri +
  ±1.0/0.6 m height band), and reports a channel×kind histogram plus
  the deepest hits with `dy=`/`depth=`. Denominators keep everything:
  parse failures, unresolved names, skipped labels/decals/`giz_*`,
  cap overflow, walk issues. In-road counts are *findings*, not
  failures — retail authors legitimately stamp on these surfaces —
  so `--strict` exits 2 on failures/issues only.
- `crates/mm2_game/src/lib.rs`: exports `Carriageway`,
  `MAX_PATHSET_STAMPS`, `PathStampSite`, `PathStampSites`,
  `carriageways`, `path_stamp_sites`.

## Tests

- `props.rs` (new): Points/Directed/LineStrip expansion, odd-tail
  drop, per-segment spacing restart, budget capping + counted
  overflow, zero-spacing and unknown kinds; carriageway extraction on
  synthetic PSDLs — divided road → two strips with the median
  excluded, fans/crosswalks/no-sidewalk roads, malformed attrs skipped.
- `placement.rs` (new): flat/sloped surface classification, height
  band bounds, degenerate tris, depth-from-edge, stamps vs issues
  accounting.
- Existing `city.rs` pathset tests re-verify orientation/budget
  through the shared impl.

## Commands actually run and results

- `cargo fmt --all -- --check` — PASS (after a first failure on the
  new code; fixed by `cargo fmt`).
- `cargo clippy --workspace --all-targets --all-features -- -D
  warnings` — PASS (after fixing a redundant closure + a needless
  lifetime).
- `cargo test --workspace` — PASS, all suites green, 0 failures.
- `mm2-inspect placement <retail>` — exit 0. London (3224 regions):
  inst 1997 stamps/15 in-road, pathset 87 paths→1188/185, prop-rule
  415 rooms→5118/209. SF (2871 regions): inst 3763/6, pathset
  144→925/34 (+31 skipped decal paths), prop-rule 397→5028/0. 449
  findings, 0 failures, 67 issues (pre-existing prop-rule walk counts:
  65 0xx `road_rooms` + unreached rooms). `--strict` → exit 2 on those
  issues.
- `mm2 --mm2-path <retail> --city london --spawn 0.4,5.5,-720,0
  --frames 90 --screenshot /tmp/placement-spawn.png` — the operator's
  spawn reproduced: bollards across a pedestrianised street, kerbside
  lamps/dressing, no tree in the carriageway (the known `vpbug` rear-
  windscreen texture defect visible, unrelated).
- Divided-road captures `/tmp/placement-divroad{,2}.png` — the flagged
  `sp_tree1_s` sits at a median taper meeting a junction; plausible
  authored dressing, not a placement bug.

## Findings

- **No systematic lateral defect in any channel.** `MIRROR_Z` is off;
  INST stamps verbatim authored transforms; pathset/prop-rule stamps
  land where the authored data puts them.
- The props the operator saw in the road are authored there: the spawn
  area is a pedestrianised street encoded `RoadNoSidewalks` (trees/
  bollards/crates on the walkable surface by design); SF `cp_banr*`
  banner rows pivot at road surface mid-span (depth to 10 m, dy≈0 —
  mesh hangs overhead); INST hits are authored facades/bridges over
  and under drivable surfaces; prop-rule `RoadNoSidewalks` hits are
  kerb-edge stamps at depth≈0.
- Flagged for review (deepest non-banner hits): 2 `sp_stackboxes_4_l`
  ~1.2–1.8 m inside london `RoadWithSidewalks` room 570, 2 `sp_tree1_s`
  ~1–3.9 m inside london `DividedRoad` rooms 937/952 (visually checked:
  median taper at a junction), 5 crosswalk stamps.

## Still open

- The audit measures stamp *positions*; pathset expansion positions
  along a segment remain an inferred policy (UNK-20 — stamps could be
  denser/sparser than retail along the same authored path) and
  asymmetric-prop yaw is not frame-compared.
- The 5 hard-surface flagged hits above are authored-intent
  ambiguities pending a human look, not proven defects.
- Per report 2's caution, F03/F04 do not roll up to `checked` on this
  evidence; strike/settle evidence stays provisional.
- F03-C remainder: race cleanup, mod replacement end-to-end.
- Candidate pending external check.
