# Last iteration — impact-spark ledger repair (F05-B.8)

Iteration 37 on `ralph/night`, continuing from `2dfbb03` (the
F05-B.8 impact-sparks candidate — external review verdict **fail**:
documentation defects only; the implementation, its tests and both
retail headless records were independently verified). Task id stays
`F05-B.8` — the impact-sparks leg of F05-B's remaining work.

## Repair (iteration 37)

Root cause — two ledger-vs-code mislabels in the iteration-36
write-up:

1. Three docs recorded `emit_sparks` as running in "`FixedUpdate`
   with `collect_impacts` + `apply_impact_damage`". The code
   schedules `(emit_sparks, advance_sparks).chain()` in `Update`
   (`main.rs`, `smoke.rs`) and the producers run in `FixedLast`.
   Harmless in practice — independent broadcast readers see every
   buffered message — but the ledger must record the build as made
   (the iteration-030 class of defect).
2. `RadialBlast`'s second parameter was written `radius`
   (`Vector3 *` in two places). mm2hook's recovered header
   (`src/modules/effects/linespark.h`) declares
   `RadialBlast(int count, Vector3 &position, Vector3 &velocity)` —
   re-verified against the public source this iteration.

Actions (doc/comment-only — no behavior change, so the iteration-36
runtime evidence stands):

- Schedule `FixedUpdate`→`Update` + producer slot `FixedLast`:
  `docs/research/damage.md`, `docs/ralph/PLAN.md` (F05-B.8 row),
  this file.
- Parameter `radius`→`position` (and `Vector3 *`→`Vector3 &`):
  `docs/research/damage.md`, `docs/ralph/PLAN.md` (selection
  paragraph + F05-B.8 row), `docs/original-rules.md` (DSN-26 row),
  this file, and the same wrong signature in the doc comments of
  `crates/mm2_app/src/spark_fx.rs` and
  `crates/mm2_game/src/effects.rs`.

Verification: schedule re-checked against
`crates/mm2_app/src/main.rs` (the `(emit_sparks, advance_sparks)`
Update chain; `collect_impacts`/`apply_impact_damage` inside the
FixedLast chain) and `crates/mm2_app/src/smoke.rs` (identical
headless wiring); signature re-checked against mm2hook
`linespark.h`. `cargo fmt --all -- --check`, `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` and
`cargo test --locked --workspace` re-run — see Evidence.

## Iteration 36 record (labels corrected)

Continuing from `1425c17` (the externally checked F05-B.7
engine-impairment candidate — review verdict pass). Task id:
`F05-B.8` — the impact-sparks leg of F05-B's remaining work (F05
spec req 4: "apply documented damage consequences / effects").

## Slice choice

Of the F05-B remainder, the texel-damage + sparks line was recorded
as blocked on "a contact-point feed that does not exist yet". That
blocker was stale: `mm2_game::impact::ImpactEvent` already carries
`point`/`normal`/`severity` and `collect_impacts` already fills them
from the Avian manifolds — the feed exists and is deduplicated,
bounded and generation-stamped. No new contact system was needed.

Between the two consumers, `asLineSparks` sparks are the actionable
leg: mm2hook recovers a per-vehicle `asLineSparks* Sparks` on
`vehCarDamage` fired from `ImpactCB` via
`RadialBlast(count, Vector3 &position, Vector3 &velocity)`, and the
install ships `texture/spark.tga` (8×8, the only spark-named
texture). `TextelDamageRadius`/`ImpactsTable` stays open — its
`fxTexelDamage` consumer and decal-vs-deformation semantics are
unrecovered and it needs a real rendering decision, not just a feed.

The exact emission shape (count, velocities, cadence, life, texture
name) is unrecovered — `vehCarDamage::Update()` is a thunk and
`SparkMultiplier` is runtime state no retail tune record authors —
so the implemented policy is designed (DSN-26; UNK-13 stands).

## What changed

- `mm2_game::effects` (designed, DSN-26):
  - `SparkPolicy` — `sparks_per_speed` 0.8, `min_burst` 2,
    `max_burst` 16, `max_live` 128/vehicle, `speed` 7 ±3 m/s,
    `spread` 0.8 rad, `life` 0.5 ±0.2 s, `length` 0.04 m,
    `gravity` 9.8 m/s². `burst_count(severity)` returns 0 for
    non-finite or non-positive severity.
  - `VehicleSparks` — the per-vehicle rig (the `asLineSparks*`
    counterpart), `NavRng` seeded from the `ObjectId` like
    `VehicleSmoke`. `burst(point, outward, severity, live,
    emitter)` spawns streaks at the authored contact point with
    velocities = `outward` (the participant's own side of the
    contact normal) + ±`spread` lateral jitter renormalized at
    `speed` ±`speed_var`; a degenerate normal falls back to
    straight up, `live >= max_live` truncates.
  - `Spark` — `advance(dt)` integrates gravity/position and
    reports expiry at `life` (garbage `dt` accrues nothing);
    `streak()` is the speed-scaled length floored at `length`;
    `alpha()` is a linear `1 − age/life` burn-down.
- `mm2_app::spark_fx`:
  - `spark_assets` — `spark.tga` through `city::load_image` (mod
    overrides included) onto an unlit `AlphaMode::Add`
    `StandardMaterial` (warm tint — the authored fleck has no
    alpha, so black adds nothing and the per-spark alpha scales the
    contribution). A missing texture warns and falls back to an
    untextured additive material — the standard missing-texture
    policy; emission continues.
  - `emit_sparks` (`Update`, chained with `advance_sparks`) —
    drains the `ImpactEvent` buffer as an independent broadcast
    reader of the stream the `FixedLast` producers
    `collect_impacts`/`apply_impact_damage` publish: one
    burst per rigged local/AI participant at `event.point`,
    rebound side = that participant's side of `event.normal`.
    Remote participants skip (their authority renders its own,
    F25+); a non-`Playing` frame drains without emitting; a
    per-emitter local live count closes the deferred-spawn gap so
    burst-heavy frames stay inside `max_live`. Each spark spawns a
    session-stamped entity — velocity-aligned crossed quads
    (width 0.03 m) in a per-spark cloned material.
  - `advance_sparks` — reposes each streak along its live
    velocity, writes the linear fade, despawns on expiry.
  - `SparkFxReport` — `spk=<b>b/<e>e/<x>x` on the smoke record,
    activity only; `SparkFx` resource is session-scoped (inserted
    on load, removed on teardown; absent → nothing emits).
- Wiring: `VehicleSparks` attached at player + opponent spawn
  behind the same `def.damage` authored-presence gate as
  `VehicleDamage`/`VehicleSmoke`, same seed domain; `SparkFxReport`
  registered in the real app and headless smoke chains;
  `drive_session` resets the report on teardown. The banger/bot/
  menu/event/nav-overlay/opponents/profile/progression/race/
  results/session/traffic test harnesses gained the one-line
  `init_resource::<SparkFxReport>()` their `drive_session` chains
  now require (the first workspace run caught two panics from its
  absence).

## Tests

- `mm2_game/tests/effects.rs` (+3): burst floor/scaling/ceiling +
  garbage severity (`burst_count_scales_with_severity_and_clamps`);
  determinism, distinct streams, contact-point birth, rebound
  hemisphere, emitter, live bound, degenerate/NaN normals
  (`bursts_are_deterministic_and_bounded`); gravity, streak,
  alpha, expiry, garbage-dt (`spark_advance_falls_and_expires`).
- `mm2_app/tests/spark_fx.rs` (new, 9): a reportable impact sparks
  at the contact point; both rigged participants burst on their own
  rebound sides; remote participant never sparks locally; unrigged
  participant emits nothing; absent `SparkFx` emits nothing; stale
  generations drain without emitting; streaks track velocity and
  fade; sparks expire on `life`; the pool stays bounded under a
  burst-heavy feed.

## Evidence

Iteration 37 re-run (doc/comment-only diff):

- `cargo fmt --all -- --check`: PASS.
- `cargo clippy --locked --workspace --all-targets --all-features
  -- -D warnings`: PASS.
- `cargo test --locked --workspace`: 63 suites, 0 failures.

Iteration 36 (implementation candidate `2dfbb03`):

- `cargo fmt --all -- --check`: PASS.
- `cargo clippy --locked --workspace --all-targets --all-features
  -- -D warnings`: clean.
- `cargo test --locked --workspace`: 63 suites, 0 failures
  (mm2_game effects 14/14, mm2_app spark_fx 9/9).
- Retail headless on the supplied install
  (`fnv1a64:e91e6cd4b2ae30d9`), `--bot --frames 600`:
  - SF vpbug scripted cruise: `status=pass … impacts=12
    dmg=3a/0d/0r rej=3 dup=0 vsk=6a/0d/0r spk=6b/20e/18x` — bursts
    on the six player-involved impacts (3 applied + 3 rejected
    damage deliveries), every streak born and expired; all other
    counters and the final pose bit-identical to the pre-change
    record (sparks are presentation-only). Re-run identical.
  - London scripted cruise: `… impacts=7 dmg=1a/0d/0r rej=4 dup=0
    vsk=5a/0d/0r spk=5b/23e/23x` — same pattern, bit-identical
    elsewhere.

## Classification / open items

- F05-B.8 is `implemented` (candidate) — external review of
  `2dfbb03` rejected the candidate on the two doc mislabels above
  (implementation, tests and retail records all verified by the
  reviewer); iteration 37 repaired the labels. Re-candidate
  pending external gates + review.
- Classifications: the per-vehicle `asLineSparks` renderer and the
  `ImpactCB`→`RadialBlast` call shape are recovered (mm2hook);
  `spark.tga` is a recovered asset; every emission value, the
  streak render and the texture *binding* are **designed**
  (DSN-26); the original's burst semantics stay UNK-13.
- Honest gaps: no GPU/operator view of the streaks (headless
  counters only — the render path is unlit quads, verified by
  transform/material assertions, not a screenshot); remote-skip is
  synthetic (no networking); whether `spark.tga` is the texture
  the original `Init` bound is unrecovered.
- Still open in F05-B: `TextelDamageRadius`/`ImpactsTable` texel
  damage (`fxTexelDamage` consumer + decal-vs-deformation
  semantics unrecovered), damage-driven detachment if the original
  uses it, C&R healing driver (DMG-4 — needs F27), replication
  (F25+).
