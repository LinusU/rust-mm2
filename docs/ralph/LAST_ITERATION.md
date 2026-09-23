# Last iteration — F10-B.10: startup WARN aggregation (operator report 4 item 5)

Iteration 52 on `ralph/night`. Selected the last open operator-report-4
item: startup WARN spam. The report named the 154 `road N: no routable
vehicle lanes` lines; the same failure mode existed in a second class
found while verifying on retail.

## What changed

Two per-entry WARN classes, both expected authored-data anomalies
already counted in existing report/summary fields:

1. `mm2_app::traffic::load_ambient_traffic` — every plan/nav issue
   was `warn!`'d per entry. New `partition_nav_issues`
   routes `NoVehicleLanes` (pedestrian/special/disabled roads — WLD-11;
   154 london / 22 sf authored) to one `debug!` summary carrying the
   count and the full road list. Every other plan/nav issue kind
   (`UnresolvedEnd`, `DegenerateLane`, `NonFiniteLane`,
   `LaneDistancesRecomputed`, all `TrafficIssue`s) keeps its individual
   WARN — they are rare anomalies, not bulk authored data.
2. `mm2_app::city::load_city` — `walk_prop_rules`'s per-entry issue
   strings (64 on sf: the 0xx-encoded `road_rooms` records already
   counted as `bad_refs`; london authors none) collapse into one
   `debug!` line with count + the bounded list. Prop-rule *table*
   diagnostics keep their WARN.

Nothing is dropped: `traffic.issues` still records every issue
(`issues=` in the ambient `info!` unchanged — london `issues=154`),
`report.proprule_issues`/`bad_refs`/`unreached` unchanged, and
`mm2-inspect nav` / `mm2-inspect placement` still list every entry.

## Verification (this tree)

- `cargo fmt --all -- --check` — pass.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — pass.
- `cargo test --locked --workspace` — pass, 64 suites, 0 failures;
  +2 `mm2_app` unit tests for the partition (bulk class out, WARN list
  ordered/complete, empty input).
- Retail (`fnv1a64:e91e6cd4b2ae30d9`, `--headless --frames 60`): london
  startup WARN lines 155 → 1 — the genuine `p_parkmeter_f.tex` mip
  warning the report cited as buried; sf 87 → 1 (same line).
  `RUST_LOG=mm2_app=debug` shows both summaries with correct counts
  (london `count=154`, sf `count=22`, sf walk `count=64`).

## Not done / blockers

- None for this item — operator report 4 is now fully addressed
  (items 1–5 implemented across F10-B.9, F04-C.4, F04-C.5, F03-C.3,
  F10-B.10; all candidates pending external check).
- Carried: UNK-22 (banger activation quantity), the named
  `sp_tree1_s` retail shatter not yet staged, `vpmoonrover` launch
  wander (open `drive` finding), vehicle handling remains
  operator-owned.
