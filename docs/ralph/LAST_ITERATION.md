# Last iteration — F11-C.2 handoff-doc repair (iteration 48)

External review of iteration 47's candidate `6aab29d` (F11-C.2)
returned one blocking finding: a stale recorded test-count
baseline. This file and PLAN.md's F11-C.2 row claimed the
`mm2_inspect` suite went 12 → 15; the actual suite went 27 → 30
(`event.rs` module 7 → 10). Root cause: the baseline was copied
from F11-C.1's commit-time count ("5 → 12", correct at `1720a53`),
but ~113 commits landed between C.1 and C.2 and grew the suite to
27. The `+3` delta and the `30/30` gates line were already right.

Repair (docs-only, no code touched): corrected both claims to
`27 → 30`. Verified by recounting `#[test]` at base `93db26b`
(27 total / 7 in `event.rs`) and at `6aab29d` (30 total / 10 in
`event.rs`); `cargo test --locked -p mm2_inspect` re-run below.

The iteration-47 record follows, unchanged and still accurate.

---

# Iteration 47 — F11-C.2: catalog-wide deep event audit (`event --all`)

Iteration 47 on `ralph/night` (baseline `93db26b`, F07-B.9 scripted
drive-sequence evidence — external verify + review green). One
coherent slice of the F11-C remainder: the catalog-wide strict-audit
evidence leg the plan owed, backed by a small tooling change so the
whole catalog runs through the single-event deep check in one
command.

## Task selection

No failing gate or review finding to repair (F07-B.9 review passed,
verification gaps only). Among the listed remainders, F11-C's
run-and-record leg was the ready one: the other candidates are
research-gated (F15-B's `unkFlag`/`cornerBrakingThreshold` fields,
F18-A's `.ldef`/`.lmap` semantics under UNK-24, F05-B's detachment
rule under UNK-13), blocked on missing features (F17-B → F17-C,
F16-C's AC01 process leg → an interactive finish — scripted-driver
results are deliberately ineligible), blocked on an audio output
device (F07-AC05), or entirely designed policy (F07-B's scrape leg
has no authored sample to bind). F11-C won because the per-event
deep audit existed but had only ever been run on single rows — the
catalog-wide leg needed one small production change plus the retail
evidence run.

## What landed

- `tools/mm2_inspect/src/event.rs` — `CitySweep` + `sweep()`: run
  `inspect_event`'s full dependency-closure check on every cataloged
  event in a city (per-record deep parse incl. the aimap/pathset
  records the catalog scan leaves `Unparsed`, `RaceDefinition` and
  `OpponentRoster` builds at both difficulties, wired vehicle ids
  cross-checked against `VehicleCatalog`). The vehicle catalog is
  scanned once per run and shared — `inspect_event` now takes the id
  set instead of rescanning per row. `CitySweep::failures()`
  aggregates the same conditions `EventReport::failures()` reports
  per event plus table errors and an empty catalog.
- `mm2-inspect event` CLI — `--all` sweeps the whole catalog
  (`--city` restricts it to one stem; without `--all`, `--city` and
  `--event` stay required as before, and `--event` conflicts with
  `--all`). Output: table statuses, one line per event
  (`ready`/`incomplete`, record count, `defs ok/ok`, `rosters
  ok+Ni`), indented per-event failure detail, then a per-city
  summary carrying the extras count.
- `docs/race-coverage.md` — `--all` added to the instrument list and
  the sweep's retail numbers recorded in the denominator section.

## Evidence

Synthetic tests (`tools/mm2_inspect` suite, 27 → 30; `event.rs`
7 → 10):

- `sweep_reports_every_cataloged_event` — all three authored rows of
  the synthetic install appear in row order; the fully-wired row is
  clean, the two record-less rows are `incomplete` and named in the
  strict failure list — the denominator is never filtered;
- `sweep_surfaces_a_record_failure` — a malformed `circuit0.aimap`
  lands under `circuit:0` in the sweep failures;
- `sweep_empty_catalog_is_a_failure` — a city with no race data
  reports the empty catalog as a failure, not silence.

Retail run-and-record (`fnv1a64:e91e6cd4b2ae30d9`, read-only
install, this commit's binary):

- `mm2-inspect event <install> --all` — **90/90 cataloged events
  `ready`** (45/city), 0 incomplete, 0 failed records, 0 failed
  `RaceDefinition`/`OpponentRoster` builds at either difficulty.
- `--strict` exits 2 on **96 authored anomalies**, all previously
  disclosed classes: 77 orphan `.opp` route records (46 amateur +
  31 professional), the `sf/race0` aimap 6-vs-7 table mismatch, and
  18 per-record diagnostics the catalog-wide audits count but don't
  attribute per row — 8 `AmbDenisty` header misspells, 6 omitted
  `Filename` labels (london `crash8` + sf `crash4`/`crash9` data
  pairs), and 4 short rows (8 of 9 fields) skipped in london's
  `exam1_1.csv` (a `crash3` midterm waypoint file — authored data,
  disclosed not repaired).
- Sibling legs re-run the same commit: `events --strict` rc 0,
  `race-defs --strict` rc 0 (64 defs built per city, 26 crash-course
  rows `unsupported` by design), `opponents --strict` rc 2 on the
  same 79 authored anomalies.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` — all suites green (mm2_inspect
30/30 incl. the 3 new sweep tests).

## Classification

Implementation choice end to end — the sweep is an audit view over
the existing deep check; it makes no original-behavior claim. The
retail numbers are original-content validation evidence (named
fingerprint, full denominator, failures enumerated not filtered).

## Remaining open items

- F11-C stays active: AC02–AC05 rest on the landed runtime slices'
  test evidence (swept triggers, countdown/restart lifecycle,
  once-only ledger results) — promotion of those ACs is a review
  judgment, not new work this slice. AC06's "loaded" leg is the
  `mm2 --event` headless smoke records (F13-C matrix).
- The 96 strict findings are authored retail anomalies — they stay
  visible under `--strict` rather than being whitelisted away.
- F07-B continues: sustained-scrape semantics (no authored scrape
  sample — entirely designed) and the AC05 audible capture (needs a
  real output device).
