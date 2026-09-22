# Last implementation iteration

- Task ID and title: F05-A.1 repair — external review of `099088e`
  rejected the candidate: the recorded retail damage census was
  falsified in at least six places inside `verified_original` ledger
  rows, `docs/research/damage.md` and doc comments.
- Starting commit: `099088e9cc39119eec42424d3974ba853e4edb88` on
  `ralph/night`; tree clean. This is a review-finding repair, not new
  feature work.

## Root cause

Iteration 26's census was written into the ledger/docs without a
per-record check — several "uniform on every record" claims were
inferred from a sample rather than measured. Re-measured every
contested field across all 61 records by dumping each
`tune/vehicle/*.{vehcardamage,vehstuck,vehgyro}` through the
production VFS (`mm2-inspect dump <install> <logical>`, install
`fnv1a64:e91e6cd4b2ae30d9`) and tabulating per-vehicle values.

## Corrected measured facts (verified via dump census)

- `ImpactThreshold`: 1500 on 19 of 20 records; **vpcaddie authors
  100** — the "1500 on every record" claim and the F05-AC01
  "sits well above resting contact" reasoning were wrong for it.
- `DoublePivot`: **1 on vpddbus, vppanoz, vppanozgt**, 0 on the
  other 17 — not "0 on every record".
- `MirrorPivot`: present on **vpbullet, vpbus, vpcaddie, vpcop,
  vpddbus, vpdune, vpmustang99** (all 0) — the committed set
  (vpbus/vpcab/vpcaddie/vpcoop/vpcop/vpddbus/vpsemi, "tall bodies")
  was wrong on 3 of 7 names and the gloss was contradicted both
  ways.
- `vehgyro` `Roll`/`Pitch`: absent on **vpbug, vpcab, vpford,
  vpvwcup_angel** — not vpdune/vpford/vpmustang99/vpvwcup_angel
  (vpdune and vpmustang99 author them at 0.0).
- `TextelDamageRadius`: **0.4 (vpcaddie/vpcentury) – 20.0
  (vp4x4/vppanozgt)** — not "~0.5".
- `vehstuck` `Turn`: ≈ π (3.141593) on 10 of 20 (vpford 3.098593),
  1.57 on only 6 — not "≈ π/2 on most". `MoveThresh` (1.75)
  **exceeds** `PosThresh` (1.25) on every record — the "movement
  bound under PosThresh" gloss was backwards.
- Additional falsified claims found while re-measuring: `MaxDamage`
  minimum is **187 500 (vpcoop/vpcoop2k)**, not 238 750 (vpauditt);
  `MedDamage` minimum is **80 000**, not 150 000; the field set is
  **38 fields** (39 with `MirrorPivot`), not 37.
- Confirmed still true: 20/20/21 record counts, `RegenerateRate` 0
  and `Rotation` 0 on every record, `MedDamage < MaxDamage` on every
  record, `PosThresh` 1.25 / `MoveThresh` 1.75 uniform, `TimeThresh`
  2.0 on the same 6 records that carry `Turn` 1.57, `Translation`
  ≈ 0.1 (vpcoop 0.164), `Roll`/`Pitch` authored 0.0 where present.

## What changed

- `crates/mm2_formats/src/veh.rs` — every falsified doc claim
  corrected to the measured values above; `VehGyro` `Roll`/`Pitch`
  now decode through a new strict `opt_f32` (present-but-non-numeric
  is a decode error, matching `opt_i64`/`MirrorPivot`) instead of
  `root.f32()`, which silently equated malformed with absent.
- `crates/mm2_formats/tests/vehicle_formats.rs` — fixture comment
  corrected (38 fields); `vehgyro_decodes_optional_fields` gains the
  `Roll fast` non-numeric → decode-error regression case.
- `crates/mm2_game/src/damage.rs` — module doc drops the false
  "authored 1500 sits well above" margin claim; `DamageSpec`/
  `impact_threshold` docs carry the measured bounds.
- `docs/research/damage.md` — full census table corrected; stuck
  and gyro sections rewritten to measured values.
- `docs/original-rules.md` — DMG-5/6/7/8 rewritten with the measured
  per-vehicle facts (evidence cites now include `mm2-inspect dump`).
- `docs/ralph/PLAN.md` — F05-A.1 row updated (field count, strict
  `opt_f32`, review-failure note and the repair).

## Evidence

- `cargo test -p mm2_formats --test vehicle_formats` — 20 pass, incl.
  the new non-numeric-`Roll` rejection inside
  `vehgyro_decodes_optional_fields`.
- `cargo test -p mm2_game --test damage` — 6 pass.
- `mm2-inspect damage <retail> --strict` — exit 0: 29 catalog
  vehicles, 61/61 records parsed, 0 issues, 14 with breakaway parts,
  10 dead-fragment diagnostics (unchanged).
- `mm2-inspect car <retail> vpcaddie` — production path reports
  `threshold 100`, `decal r 0.40 m` (the corrected values).
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, 701 tests, 0 failures
  across 51 test binaries.

## Still open

- F05-A.1 remains candidate pending external check — the census
  claims now rest on the scripted dump census recorded above, which
  the reviewer can reproduce per record.
- Unchanged open items from the original slice: no runtime
  impact→damage application, tiers/visuals, breakaway lifecycle,
  impairment/disabled behavior, recovery logic or replication;
  UNK-13 accumulation semantics; DSN-10 designed impulse estimate;
  designed `disabled_outcome` mappings for Cruise/CrashCourse.
