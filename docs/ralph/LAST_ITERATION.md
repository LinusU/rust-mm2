# Last implementation iteration

- Task ID and title: F16-B.3 — vehicle/paint selectability derived
  state (`unlocks` set → roster) for F17's garage. Selected from
  TASKS.json/plan as the open F16-B derived-state leg; no external
  review blockers were outstanding.
- Starting commit: `eb772c841c9836625df2753afbc6b4d78b2d89c9` on
  `ralph/night`; tree was clean.

## What changed

- `mm2_game::progression` (DSN-18): `VehicleGate`/`PaintGate`
  (`Open`/`Reward`), `GarageRow` (`id`, `listed`, `gate`,
  `paint_gates`, verbatim `unlock_score`/`unlock_flags` audit fields),
  `GarageTable::{evaluate,of,row}` returning
  `VehicleAvailability{unlocked, paints[]}` — evaluated per query off
  `profile.progress.unlocks`, sandbox identities unrestricted (spec
  req 5). A locked vehicle reports every paint locked; granting the
  car does not open its reward-gated paints.
- `mm2_content::garage` (new): `garage_table(catalog, &[&RewardTable])`
  unions every race city's authored grants — `vehicle:<id>` rows gate
  the car (VEH-3), `paint:<id>:<variant>` rows gate the zero-based
  `Colors` index — and `scan_garage(vfs)` composes the whole surface
  (catalog + `race_cities` + per-city reward tables, reward
  diagnostics folded in). Grants for uncatalogued ids or out-of-range
  variants land in `diagnostics`, never dropped.
- `mm2_content::catalog`: `CatalogEntry::locked` removed — measured
  wrong for this purpose (`UnlockScore|UnlockFlags` nonzero marks
  `vpbus`/`vpbullet`/`vpcentury`/`vpcop`/`vpsemi`/`vppanozgt` but
  misses six reward-locked cars). Replaced by raw `unlock_score`,
  `unlock_flags` (audit, UNK-6) and `canonical_info` — whether the
  metadata resolved via `tune/<id>.info` itself. Roster membership
  (`GarageRow::listed`) is that canonical scan: fallback-extension or
  metadata-less entries (`vpmoonrover`'s `.inf` — UNK-3 stands, dev
  leftovers) are unlisted but still evaluate. Designed reading,
  documented as such.
- `mm2_content::events`: shared `race_cities(vfs)` (expected ∪
  discovered `race/<city>/` dirs); mm2-inspect's local copy now uses
  it.
- `mm2_app`: warn-don't-enforce wiring mirroring the locked `--event`
  launch — `profile::vehicle_gate_note` reports
  `Uncatalogued`/`Unlisted`/`Locked`/`LockedPaint` for a gated
  `--car` or remembered selection once at launch (F17's menu owns
  enforcement). `--list-cars` and `mm2-inspect cars` print a `gate`
  column (`open`/`reward`/`unlisted`, `+Ng` gated paints) plus the raw
  authored `s/f` fields.
- `docs/original-rules.md`: DSN-18 added; DSN-16 and VEH-4 updated —
  `VariantNum` is now *measured* as the zero-based paint index
  (vpvwcup variant 5/6 = "Team Angel"/"Team MS", the documented
  Angel/Microsoft cup paints; every authored variant lands on its
  car's declared `Colors` index); VEH-5 records the `UnlockFlags`
  non-correlation.

## Tests

- `mm2_game` +6 (`tests/progression.rs`, 18 total): fresh-profile
  gates, vehicle grant opens car but not gated paints, paint grant
  opens exactly its index, unknown unlock ids inert, sandbox
  unrestricted, uncatalogued id has no row.
- `mm2_content` +3 (`tests/garage.rs`, new): grant→gate mapping +
  `listed` flags on a synthetic install, off-catalog/out-of-range
  grants diagnose, `scan_garage` unions two cities' reward tables.
- `mm2_app` +1 (`tests/progression.rs`, 10 total):
  `vehicle_gate_note` reports Locked/LockedPaint/Unlisted/
  Uncatalogued, granted unlocks clear the note, sandbox never notes.

## Commands actually run and results

- `cargo test -p mm2_content --test garage` — PASS (3/3).
- `cargo test -p mm2_game --test progression` — PASS (18/18).
- `cargo test -p mm2_app --test progression
  a_gated_vehicle_selection_reports_its_note` — PASS (1/1).
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --workspace --all-targets --all-features -- -D
  warnings` — PASS.
- `cargo test --workspace` — PASS, 44 suites, 0 failures.
- Retail audit (`mm2-inspect cars`, install
  `fnv1a64:e91e6cd4b2ae30d9`): 29 entries; `reward` = exactly the 8
  VEH-3 locked cars (vp4x4, vpauditt, vpcoop2k, vpdb7, vpdune,
  vppanozgt, vpsemi, vpvwcup); gated-paint counts match the authored
  rewards (vpvwcup 4+3g for variants 4/5/6, vpauditt/vpbullet/vpcoop/
  vpcoop2k/vpdune/vppanozgt 4+1g, vpsemi/vpddbus 3+1g, vpcab 1+1g);
  vpmoonrover and the dev leftovers `unlisted`; zero garage
  diagnostics.

## Still open

- F17 owns enforcement: the garage query is reported/warned, not
  enforced — no menu exists yet.
- `UnlockScore`/`UnlockFlags` runtime semantics stay UNK-6 (kept
  verbatim on `GarageRow` for audit; `vppanozgt`'s 8000 score is
  consistent with a Pro-points paint gate but unverified — DRV-4/
  UNK-8 unimplemented).
- Whether the original's select roster is exactly the `tune/*.info`
  scan is a designed reading — vpmoonrover's entry path stays UNK-3.
- F16-C evidence (restart isolation, corruption recovery, deliberate
  delete, full AC05 audit) and F16-B parent's remaining items stand.
- Candidate pending external check.
