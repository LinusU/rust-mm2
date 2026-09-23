# Last iteration — F04-C.5 banger activation threshold reading

Iteration 50 on `ralph/night`, continuing from the externally-passed
F10-B.9 junction-crossing candidate. Operator report 4's item 3 was next:
"trees and larger light poles cannot be broken at all." The report framed
it as a research question — the INST placement channel binds no banger
records, so unbreakable trees could have been correct fidelity. It is a
real defect, but not the one the framing suspected.

## Root cause (measured, not guessed)

Binding is complete: INST places zero tree/pole names in either stock
city, but trees, streetlights and poles reach the world through prop-rule
stamping and `props.pathset` and already spawn as dormant bangers.
`sp_tree1_s` is authored breakable — NumParts=5, BREAK01–05 chunks,
fragment records.

The gate was wrong. `impulse_estimate` compared `approach_speed ×
striker_mass` to `ImpulseLimit2`. A census of every placed bound record
shows `ImpulseLimit2 ≈ Mass × {31.25, 500, 800, 2000, 85342}` — a ladder
quadratic in striker speed. Under the linear reading a 4 250 kg `vpbug`
needed ~470 m/s for a tree and ~120 m/s for a streetlight — unreachable,
which is exactly what the operator saw. Read as striker kinetic energy
`½·m·v²` the same authored numbers form a coherent ladder: meters/cones
at walking speed, benches/dumpsters ~5–9 m/s, poles/trees ~11–33 m/s,
gantry props higher, and the authored-immovable outliers (`1e30` bridges,
`sp_lightthames_l`) stay unreachable. Breakability under the energy
reading correlates with authored BREAK<NN> fragment presence.

## What changed

- `mm2_app::contracts`: `impact_energy` (`½·m·v²`) shares a new
  `striker_mass` resolver with the retained linear `impulse_estimate`;
  same 1 kg fallback for unresolved/invalid masses.
- `mm2_app::banger`: both `activate_bangers` gates — the real manifold
  path and the `StrikeBound` overlap path — feed `impact_energy` to
  `activates_on`. `banger shattered`/`banger activated` debug lines now
  name the prop.
- Unchanged by design: `knock_ambient` keeps its designed linear
  threshold; breakaway keeps its own `severity × part_mass` reading; the
  F04-C.4 two-body momentum transfer, pool, fragment spawning, predicted
  authority gating and placement-height behavior are untouched.
- `mm2_game::banger`, `docs/research/banger.md`,
  `docs/original-rules.md` (DSN-10/UNK-22) updated to describe the
  energy reading as designed/provisional — justified by the authored
  census, not claimed as recovered original semantics.

## Verification (this tree)

- `cargo fmt --all -- --check` — pass.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — pass.
- `cargo test --locked --workspace` — pass, all suites 0 failures;
  `tests/banger.rs` 21/21 including the new
  `an_authored_tree_limit_demands_speed_not_just_mass` (a 2e6 tree-class
  limit dormant at ~20 m/s, active at ~80 m/s) plus retuned
  energy-scale thresholds on the existing limit-sensitive tests.

## Retail evidence (fingerprinted install `fnv1a64:e91e6cd4b2ae30d9`)

- `mm2 --city sf --headless --frames 1200` → `status=pass …
  bng_ev=0a/1s/1b`; debug line `banger shattered
  name=sp_lightstreet_rt_f severity=37.5 estimate=702303` — a 10 t
  freeway streetlight pole with authored BREAK parts, priced at ~75 m/s
  under the old reading (unreachable on that stretch) and ~12–17 m/s
  under the new one. A knocked ambient car struck it mid-run.
- `mm2 --city london --headless --frames 1500 --car vpbug
  --spawn=510,5.5,-245,-51.7` → `status=pass peak=37.6m/s moved=178m
  bng_ev=1a`; `banger activated prop=sp_can_royal_l severity=13.4
  estimate=89736` — mid-tier activation on a junction crossing.
- Below-limit behavior intact: repeated runs show dormant props still
  stopping strikers (`bng_ev=0` at sub-threshold speeds).

## Not done / blockers

- A named `sp_tree1_s` shatter was attempted ~12 times without success —
  in-road trees sit inside junction fans behind kerb clutter or on
  medians with ~25 m spacing, so the straight-line `hold` driver cannot
  reach the ~22–31 m/s the class needs before the first trunk. This is
  a staging limitation, not a code defect: the tree shares the pole's
  code path, and the unit regression covers the 2e6 threshold in both
  directions. A curved approach or a driver that can aim mid-run would
  close the evidence gap.
- The original's comparison quantity stays unrecovered (UNK-22) — the
  energy reading is a designed stand-in selected by the authored-limit
  census, not verified original behavior.
- Operator report 4 remaining item: 5 (startup warn aggregation).
