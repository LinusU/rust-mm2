# Last implementation iteration

- Task ID and title: F03-B.6 — prop-rule stamps follow the authored
  kerb chain (defect repair on the F03-B.3 stamping channel:
  trees/lamps/meters landed inside the carriageway on curved
  blocks).
- Starting commit and resulting commits: started at
  `f55276042b20adf13b3abad367f3040fa05f47ef` (externally checked
  F14-B.1 handoff; branch `ralph/night`, clean tree).
- Why this slice: operator screenshots showed trees, a phone box
  and lamp posts standing on the carriageway at kerb lines. The
  geometric probe against BAI sidewalk curves measured 532/5 083
  london stamps inside the road approximation, worst case 11.75 m
  past the kerb — a real placement defect in shipped code, ahead
  of any new feature.
- Retail install: `/Users/linus/coding/rust-mm2/retail`
  (`fnv1a64:e91e6cd4b2ae30d9`), all runs `--headless` on this
  machine's dev-profile binary.

## What changed

- **Root cause (measured, not inferred):** `walk_prop_rules`
  modelled each side's kerb as the straight chord between the two
  crossings' curb-corner vertices. The real kerb bends with the
  road surface and is not on the room perimeter — it lives in the
  room's road attributes (`RoadWithSidewalks`
  `[sw_l, road_l, road_r, sw_r]`, `DividedRoad`
  `[sw_l, rl_out, rl_in, rr_in, rr_out, sw_r]`, `SidewalkStrip`
  `(ground, top)` pairs, `RoadNoSidewalks` walkway edges). On a
  curved block the chord cuts through the carriageway — london
  room 816's authored kerb deviates ~22 m from its chord.
- **`mm2_game::props`**: `kerb_strips` extracts each room's
  (kerb, outer) vertex chains from its road attributes using the
  same `counted`/inline layouts as the city importer;
  `match_strip` picks the chain spanning the side's two curb-
  corner vertex ids (longest arc wins) and orientates it to the
  walk. Stamps now walk the authored kerb by arc length and lerp
  toward the outer chain *index-paired per cross-section*, so the
  authored kerb↔building-line correspondence is kept (previously
  the outer arc was arc-length-parametrized independently of a
  straight kerb chord).
- **Fallback:** a side with no matching strip stamps on its
  building-line arc — on the sidewalk edge, never inside the
  road — and is counted in the new `sides_no_kerb` stat plus a
  bounded issue line. Both retail cities: `sides_no_kerb = 0`.
- **`mm2_app::city`:** the prop-rule import log line reports
  `no_kerb=`.
- **`docs/research/proprules.md`:** kerb-chain geometry recorded
  (was: "curb segment"); audit numbers updated; UNK-21 unchanged
  — the *placement policy* remains inferred, the *geometry* is
  now measured.

## Tests

- `mm2_game::props` unit tests (+2, existing 4 updated to carry a
  synthetic `RoadWithSidewalks` strip):
  - `a_curved_kerb_follows_the_authored_strip` — a room whose
    right kerb bulges 14 m at mid-block; stamps land on the
    authored chain (22, 25.5) where the chord would have put
    them at x = 28–29 inside the carriageway.
  - `a_room_without_a_kerb_strip_stamps_the_building_line` —
    attr-less room stamps on the perimeter arc and counts
    `sides_no_kerb = 1`.
- Existing tests (side assignment, boundary marks, budgets)
  re-derived against the same quad rooms plus their authored
  strip — expectations unchanged (straight kerb ⇒ same spots).

## Commands actually run and results

- `cargo test -p mm2_game` — 24/24 lib tests pass incl. both new
  tests; `cargo test --workspace` — all groups, 0 failures.
- `cargo fmt --all -- --check` PASS; `cargo clippy --workspace
  --all-targets --all-features -- -D warnings` PASS.
- BAI placement audit (throwaway probe, not committed — stamp
  inside its own road's `sidewalk_inner` polygon = in-road):
  london 699 → 348 in-road (worst depth 11.22 m → 1.34 m), sf
  318 → 159 (12.24 m → 1.00 m). Residuals cluster at exactly
  0.8 m — a systematic inset between the BAI inner curve and the
  PSDL kerb verts, not placement error (stamps are constructed
  inside the authored kerb↔building-line band and cannot be
  inside the PSDL carriageway at all).
- `mm2 --mm2-path <retail> --city london
  --cam=-226,140,-126,0,-90 --nav --frames 90 --screenshot
  /tmp/propfix-nav.png` → `status=pass`, PNG local only — the
  Regent's Park bend (former worst site, room 816) now shows
  every lamp/tree on the kerb line outside the BAI lane
  polylines.

## What this proves / does not prove

- Proves: prop-rule stamps follow the authored road-edge
  geometry; no stamp can land inside the PSDL carriageway by
  construction; measured ~50% reduction of BAI-classified
  in-road stamps on both cities with the deep (>9 m) violations
  eliminated entirely; synthetic curved-kerb regression covered.
- Does not prove: that the original walks the same kerb chains —
  the geometry source is verified authored data, the *policy*
  (`start`/`distance`/`maxUse`/`lerp` semantics, side labels,
  variant pick, yaw) stays inferred under UNK-21; the residual
  0.8–1.3 m BAI inset is a data-level offset between two authored
  files, unverifiable without retail-side comparison; junction-
  fragment `SidewalkStrip` matching is endpoint-based (longest
  spanning chain) — no retail room needed the fallback.
- Acceptance IDs: advances F03-AC02's correct-transform leg for
  the prop-rule channel; F03-C's sampled-location validation
  stays open.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
