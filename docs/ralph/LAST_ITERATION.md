# Last implementation iteration

- Task ID and title: F15-A.1 — opponent-roster import: the
  `CatalogEvent → OpponentRoster` producer, the `OpponentReport`
  audit, and `mm2-inspect opponents`.
- Starting commit: `5de58083d6d3ec8373e20c9b5d33b45b146a5f9a`
  (externally checked F06-B.2; branch `ralph/night`).
- Why this slice: the plan's named candidates (F13-B/F14-B
  remainders) both block on real opponent participants, and F15-A's
  first half — "import opponent rosters/route intent" — was a ready
  dependency-light slice: `.aimap`/`.aimap_p`/`.opp` parsers, the
  event catalog and `race_def` all exist; what was missing was the
  production roster contract and an audit path. The driving
  controller, spawn slots and everything behavioral stay open under
  F15-A (F15-A.2) — this is data loading, not opponent AI.
- Retail install: `/Users/linus/coding/rust-mm2/retail`
  (`fnv1a64:e91e6cd4b2ae30d9`).

## What changed

- **`mm2_game::opponent`** (new module): `OpponentRoute` (authored
  `.opp` points + `length()`), `OpponentSpec` (`vehicle`, resolved
  `route`, raw `params`, `skill()` = first value), `OpponentRoster`
  (`entries`, `issues`, `resolved_routes()` keeps dead wired refs
  distinct from spare files), `OpponentIssue` (`MissingVariant`,
  `UnresolvedRoute`, `RouteFailed`, `WrongDifficultyTag`,
  `CountMismatch`, `UnreferencedRoute`).
- **`mm2_content::opponents`** (new): `opponent_roster(&Vfs,
  &CatalogEvent, Difficulty)` — `.aimap` binds Amateur, `.aimap_p`
  Professional with an explicit fallback when an event ships only one
  variant (recorded as `MissingVariant`).
  Each `[Opponent]` row keeps its authored slot even when the wired
  `.opp` does not resolve; route points preserve every authored
  column. Issues are attached, never dropped or repaired: wired-vs-
  table `Opponents` count mismatch, dead route ref, `-a-`/`-p-`
  name-tag disagreement with the selected difficulty, `.opp` records
  the selected roster references nowhere (scoped per variant — the
  other difficulty's files are not noise). `OpponentReport::scan`
  walks a whole city: every catalog event × both difficulties, plus
  extra roster-bearing stems (aimap files outside the event tables
  that still wire `[Opponent]` rows) and `VehicleCatalog` vehicle-id
  resolution.
- **`mm2-inspect opponents <install> [--city] [--strict]`**: per-event
  Amateur/Professional lines (wired count, table count when they
  differ, resolved routes, distinct vehicles, issue count), every
  issue, extra stems, unresolved vehicles, city totals; `--strict`
  exits nonzero on any failure or issue.
- Ledger: `race_def.rs`'s dangling `RACE-12` comment corrected to
  `RACE-7`; RACE-11 strengthened (the wired-vs-table equality now
  measured on every table kind, not just checkpoint); RACE-12 new
  (roster wiring model + spare routes + extras); UNK-11 narrowed to
  route/parameter semantics and the driving model.

## Tests

- `mm2_content/tests/opponents.rs` — +9 against a synthetic VFS
  through the production builder: `.aimap`→Amateur; `.aimap_p`
  preferred for Professional; missing-`_p` fallback; dead route ref
  retains its authored slot with `UnresolvedRoute`; `-a-` route wired
  from the `_p` file → `WrongDifficultyTag`; spare routes
  scoped to the selected variant; count mismatch is diagnostic, not
  a build failure; incomplete event + crash table rejected/
  unsupported; `OpponentReport` covers events + extras + vehicle
  resolution.
- `mm2_game/tests/opponent.rs` — +3: route `length()` sums segment
  distances; `skill()` reads the first authored param;
  `resolved_routes()` counts wired lines only (dead ref excluded,
  spare file excluded).

## Commands actually run and results

- `cargo fmt --all -- --check` PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` PASS.
- `cargo test --locked --workspace` — all groups, 0 failures.
- `mm2-inspect opponents <retail>` — exit 0. Per city: 64 roster
  builds at both difficulties, 0 failed, 26 unsupported (the two
  crash-course tables — no race records). London: 271 opponents
  wired, 43 issues (all `UnreferencedRoute` — spare `.opp` files,
  incl. `blitz3`/`blitz4` route files on 0-opponent events). SF: 246
  wired, 35 issues (34 spare routes + `race0` amateur 6 wired vs 7
  authored — the RACE-11 anomaly, and the only count mismatch on any
  table kind). Extras listed: `race/london/race12.aimap` (1 wired),
  `race/sf/stunt0.aimap` (1 wired + dead `opp-c0.2` ref). 0
  unresolved vehicle ids; pro lineups field `vpcoop2k`, `vpvwcup`,
  `vpdb7`, `vppanoz`, `vppanozgt` — all `ready` catalog entries.
- `mm2-inspect opponents <retail> --strict` — exit 2 on the findings
  above (expected: strict means "fail on any issue").

## What this proves / does not prove

- Proves: the authored opponent lineup is loadable per event per
  difficulty through the production path — real vehicle ids (not
  player clones), real `.opp` driving lines with every column
  preserved, the `.aimap`/`.aimap_p` Amateur/Professional binding
  re-verified across every event table; authored inconsistencies
  (the `sf/race0` count mismatch, spare route files, the dead
  `stunt0` ref) are surfaced with the denominator intact.
- Does not prove: any driving — no opponent entities spawn, no
  controller consumes the routes, no `.opp` column semantics are
  claimed (UNK-11 keeps those open); the 10-value `[Opponent]` tail
  beyond `skill` is raw; how opponents take grid slots is UNK-17;
  what the spare `.opp` files were for is unknown. This slice is a
  roster/data contract, not opponent AI.
- Acceptance IDs: F15-AC01's import leg advanced (roster + distinct
  vehicles resolved — the "spawn in valid start slots" half needs
  F15-A.2); AC02–AC06 untouched (all require driving opponents).
  F15-A stays `active`; F15 stays incomplete.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
