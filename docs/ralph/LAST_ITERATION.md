# Last implementation iteration

- Task ID and title: F05-B.1 — runtime impact→damage application and
  disabled outcomes (first runtime leg of F05-B; F05-A passed external
  review at `ae64197`, so its dependant became the highest-value ready
  slice).
- Starting commit: `ae64197ab8871e95d3b6420943da41e2b60a2944` on
  `ralph/night`; tree clean.

## What changed

- `crates/mm2_game/src/damage.rs`
  - `VehicleDamage` component: authored `DamageSpec` + `DamageState`
    behind a monotonic `ImpactId` watermark — re-delivered or
    out-of-order impacts return the new `DamageVerdict::Duplicate`
    instead of double-applying (F05-AC06, on top of the upstream
    `ImpactDedup` pair window). Spawned only when `vehcardamage`
    decoded — authored absence stays undamageable, never a fabricated
    spec.
  - `DamageEvent` message (object/generation/tick/impact/applied
    severity/total/tier), emitted per accepted application only —
    rejected/duplicate deliveries are non-events, so the stream stays
    bounded by the impact pipeline's per-tick cap.
  - `DISABLED_PENALTY_TICKS` = 5 s — RACE-5/DMG-2 documents a Circuit
    time penalty but no magnitude (designed, UNK-13).
- `crates/mm2_app/src/damage.rs` (new)
  - `apply_impact_damage` (FixedLast after `collect_impacts`,
    authority + `Playing` gated, drains stale input): per impact, each
    participant's delivered impulse = `severity × other_mass` (the
    vehicle's own mass when the other side is the static world or has
    no resolvable mass — same estimate family as `impulse_estimate`,
    designed conversion, UNK-13); emits `DamageEvent`s, counts
    `DamageReport` (`dmg=` smoke field).
  - `resolve_disabled` (chained after apply): enforces
    `disabled_outcome` on `Disabled` events, re-checking the live tier
    so two disabling impacts in one tick resolve once — Cruise →
    `ResetVehicle` to spawn + trailers + repair; Circuit → in-place
    reset + `clock += DISABLED_PENALTY_TICKS` + repair;
    Blitz/Checkpoint/CrashCourse → the session's own `restart` intent
    (production teardown + re-`begin`); AI opponents → in-place reset
    + repair under every mode (designed); remote participants skipped
    (F25+ authority).
- `crates/mm2_app/src/session.rs` — player spawn attaches
  `VehicleDamage` when `def.damage` is authored; `drive_session`
  resets `DamageReport` on teardown (one new param — all test
  harnesses gained the resource next to `ImpactFilter`).
- `crates/mm2_app/src/opponents.rs` — same attachment for AI vehicles.
- `crates/mm2_app/src/main.rs` + `smoke.rs` — `DamageEvent` message,
  `DamageReport` resource, both systems in the FixedLast chain;
  `dmg={a}a/{d}d/{r}r rej={} dup={}` smoke field (only when the
  pipeline saw a delivery — impact-free records stay bit-identical).
- Tests: +2 mm2_game (watermark dup/stale legs incl. repair-survival,
  spec-wrap), +11 mm2_app (sub-threshold/garbage/undamageable/
  stale-generation negatives, tier walk + event stream, duplicate
  suppression, cruise/circuit/blitz outcomes, AI outcome, two
  disabling impacts one tick → one resolution, real 4 m roof-drop
  end-to-end).
- Docs: `docs/research/damage.md` gains the runtime section and the
  review-noted vplafrance breakaway-set correction (ties vpsemi/
  vpftruck at 6 pieces — REC-1 updated likewise).

## Evidence

- `cargo test -p mm2_game --test damage` — 8 pass.
- `cargo test -p mm2_app --test damage` — 11 pass.
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, 714 tests across 52
  binaries, 0 failures (was 701/51; +13 tests, +1 binary).
- Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`):
  - `mm2 --mm2-path <retail> --city sf --headless --frames 600` →
    `traf=16/16 sp=23 rec=7 dead=0 uns=0 q=0 jq=5 stuck=0 kn=1
    sig=647 sigd=3 … dmg=2a/0d/0r rej=1 dup=0`
  - `--city london --headless --frames 600` → `… kn=1 sig=828 …
    dmg=2a/0d/0r rej=1 dup=0`
  - All pre-existing counters bit-identical to the F10-B.8 records;
    the scripted cruise takes two real damaging hits per city plus one
    sub-threshold rejection through the production pipeline.

## Still open (F05-B/F05-C scope)

- Visual tiers: smoke pivots (`SmokeOffset`/`SmokeOffset2`,
  `DoublePivot`/`MirrorPivot`), `TextelDamageRadius` decals, damage
  effect spec — decoded but unrendered.
- Breakaway lifecycle: REC-1's authored inventory is inventoried only;
  detachment rules (damage- vs impact-driven) stay UNK-13.
- Impairment short of destruction; `vehstuck`/`vehgyro` consumption;
  water/out-of-bounds recovery; DMG-4 C&R healing (channel exists, no
  mode drives it); replication (authority-gated but untested over the
  wire).
- Designed (not original-verified): `severity × other_mass` conversion,
  5 s circuit penalty, AI in-place reset outcome, cruise free-reset.
- F05-B.1 is candidate pending external check.
