# Last implementation iteration

- Task ID and title: F05-A.1 — authored vehicle damage and recovery
  data boundary (the F05-A format/contract/audit leg).
- Starting commit: `2a2cb6f222d3dac45e7d555580d48843545f1629` on
  `ralph/night`; tree was clean, previous external review verdict pass
  (F17-A.5), so this is feature work.
- Why this slice: F10-B's remainder is research-gated (UNK-12) and
  F17-A's open leg is F18-gated; F05-A's dependencies (F01-B impact
  signals, F02-B vehicle content) are checked/landed and the retail
  `vehcardamage`/`vehstuck`/`vehgyro` records were already known to
  exist. It unblocks F05-B (runtime damage) and feeds F10-B's
  collision-fidelity and F20-A police legs.

## What changed

- `mm2_formats::veh` — typed decoders on the shared tune grammar:
  - `VehCarDamage` — the full 37-field retail set: damage bounds
    (`MaxDamage`/`MedDamage`/`ImpactThreshold`/`RegenerateRate`),
    `TextelDamageRadius`, `SmokeOffset`/`SmokeOffset2` pivots with
    `DoublePivot` and optional `MirrorPivot`, and a flat embedded
    `DamageEffect` particle spec (the `dgBangerData` `BirthRule`
    vocabulary plus `LifeVar`/`DampVar`/`Height`/`Intensity`/`Color`).
  - `VehStuck` — the uniform 6-field stuck-detection record.
  - `VehGyro` — `Drift`/`Spin180`/`Reverse180` required, `Roll`/`Pitch`
    optional (absent on 4 retail records — `None`, never a fabricated
    zero).
  - `DamageIssue::{NonFinite, Negative, MedAboveMax}` validation on all
    three; unknown fields preserved as warnings; expected-root checks;
    malformed values are `VehError`s, not panics.
- `mm2_game::damage` (new) — the shared contract F05-B consumes:
  `DamageSpec` distilled from `VehCarDamage`; `DamageState` — an
  authority-owned saturating accumulator whose `apply` rejects
  non-finite/non-positive/at-or-below-threshold severities (F05-AC01)
  and reports the resulting `DamageTier`/`DamageVerdict`; `tick`
  advances the authored `RegenerateRate` channel; `repair`/`reset` are
  explicitly named authority operations; `disabled_outcome(mode)`
  maps documented RACE-5/DMG-2 consequences (Blitz/Checkpoint restart,
  Circuit penalty+reset, Cruise free reset designed, CrashCourse →
  restart marked designed pending F21). The severity→damage conversion
  is a disclosed designed policy — UNK-13 stands.
- `mm2_content::assemble` — `VehicleDef` gains `damage`/`stuck`/`gyro`
  options loaded through the VFS with provenance in `sources`; a
  resolved-but-malformed record reports into `ConversionReport`
  warnings instead of sinking the load, and decoder warnings forward
  too. `None` means authored absence — no fabricated defaults.
- `mm2_content::damage` (new) — `DamageAudit::scan`: per-catalog-entry
  coverage of all three record families, the breakaway inventory
  (pkg `BREAK*` chunks ↔ `geometry/<id>_break*.mtx` ↔
  `tune/banger/<id>_break*.dgbangerdata`, dead fragments flagged),
  uncatalogued records kept in the denominator, strict failures =
  decode rejects + validation issues only.
- `mm2-inspect` — new `damage [--strict]` audit command; `car` output
  (text + JSON) now shows the decoded damage/stuck/gyro values.
- Docs — `docs/research/damage.md` (measured field census, break-part
  naming, open questions); `docs/original-rules.md` gains DMG-5..8 and
  REC-1; UNK-13 narrowed to the still-unverified runtime semantics.

## Evidence

- `cargo test -p mm2_formats --test vehicle_formats` — 20 pass (+7):
  retail field-set decode, `MirrorPivot`/`Roll`/`Pitch` absence →
  `None`, wrong-root and missing-field rejection, unknown-field
  warnings, `MedAboveMax`/`Negative`/`NonFinite` validation.
- `cargo test -p mm2_game --test damage` — 6 pass: spec extraction,
  threshold rejection (AC01: resting/curb/suspension cannot damage),
  tier accumulation + saturation at `MaxDamage`, regeneration floor,
  repair/reset, per-mode disabled outcomes.
- `cargo test -p mm2_content --test damage` — 3 pass: audit census on a
  synthetic install (parsed/malformed/missing/uncatalogued all counted,
  dead break fragment flagged, exactly one failure), `load_vehicle`
  attachment with provenance, malformed-record warning path.
- `mm2-inspect damage /Users/linus/coding/rust-mm2/retail` (install
  `fnv1a64:e91e6cd4b2ae30d9`): 29 catalog vehicles — 20 `vehcardamage`
  + 20 `vehstuck` + 21 `vehgyro` = 61/61 parsed, 0 issues;
  `vpmoonrover` authored-absent (the undocumented secret car, UNK-3);
  14 vehicles carry breakaway parts (vpsemi/vpftruck 6-piece sets);
  10 dead fragment records reported as findings (vpeagle, vpvw_cup,
  vpvwcup, vpvwcup_angel — consistent with the banger audit's dead-ref
  list). `--strict` exits 0.
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets
  --all-features -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, 701 tests, 0 failures
  across 51 test binaries (685 + 16 new).

## Still open

- This is the authored-data boundary only: no runtime impact→damage
  application, no damage tiers/visuals, no breakaway lifecycle, no
  impairment/disabled behavior, no recovery logic, no replication.
  F05 is not original-verified — parsers and synthetic tests do not
  prove original behavior.
- UNK-13 remains open: the original's accumulation quantity,
  `MedDamage`'s consumer, detach rules, `vehstuck`/`vehgyro`
  application semantics, water/out-of-bounds recovery rules.
- `DamageState::apply` consumes `approach_speed × striker_mass` — a
  disclosed designed stand-in shared with banger activation (DSN-10),
  not a recovered original conversion.
- `disabled_outcome` for Cruise and CrashCourse is designed, not
  documented (help names no free-roam consequence; F21 owns CC rules).
- F05-A stays active: the runtime legs (impact application, visual
  tiers, breakaway, impairment, recovery, replication) remain.
