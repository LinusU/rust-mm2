# Last iteration — damage impairment (smoke↔engine-torque coupling)

Iteration 35 on `ralph/night`, continuing from `a9fd413` (the
externally checked F05-B.6 authored engine-smoke candidate — review
verdict pass, non-blocking nits only). Task id: `F05-B.7` — the
impairment leg of F05-B's remaining work (F05 spec req 4: "apply
documented impairment/disabled-state rules").

## Slice choice

Of the F05-B remainder — texel damage + sparks (needs a
contact-point feed that does not exist yet), damage-driven
detachment if the original uses it (UNK-13, not actionable),
impairment, C&R healing (needs F27's mode) and replication (F25+) —
impairment is the only ready leg with original evidence behind it:
MM2Hook's `mm2.ini` `PhysicalEngineDamage` option documents that the
original couples engine torque to damage — "when the engine spews
smoke" the vehicle has "less acceleration and less top speed". The
coupling is documented; the shape is unrecovered
(`vehCarDamage::Update()` is a thunk), so the implemented policy is
designed (DSN-25), keyed on the same authored `MedDamage` bound the
DSN-24 smoke gate emits on.

## What changed

- `mm2_game::damage::ImpairmentPolicy` (designed, DSN-25):
  `power_at_med` 0.8, `power_at_max` 0.4. `factor(total, spec)` is
  1.0 below `MedDamage`, steps to `power_at_med` on reaching it —
  `factor < 1` exactly when smoke emits, so the documented "when it
  spews smoke" coupling holds by construction — then ramps linearly
  to `power_at_max` at `MaxDamage`. Degenerate specs
  (`MedDamage >= MaxDamage`, non-finite fields), non-finite totals
  and garbage floors all degrade to full output: impairment can
  never stall or over-drive a car.
- `mm2_vehicle::vehicle::EngineImpairment` — the physics-side
  input: a per-vehicle component holding the drive-torque fraction.
  The sim knows nothing about damage; absence of the component is
  identical to 1.0, and the value is sanitised every step
  (non-finite → 1.0, clamps 0..1).
- `mm2_vehicle::systems::vehicle_simulation`: `wheel_drive_available`
  scales by the factor — forward and reverse drive both weaken, and
  `engine_load` stays consistent (numerator and denominator scale
  together). Foot brakes, `engine_brake_nm`, steering and tire
  forces are not engine output and stay unscaled — the documented
  symptoms are "less acceleration and less top speed", which torque
  scaling produces by construction.
- `mm2_app::damage::sync_impairment` (FixedLast, after
  `resolve_disabled`, authority + `Playing` gated): mirrors each
  local/AI participant's authored total into the component —
  inserted exactly while `factor < 1`, updated as damage deepens,
  removed the same tick a repair returns the state to `Intact`.
  Remote participants are skipped (their authority impairs its own
  sim, F25+); a pause freezes the factor with the rest of the sim.
  `DamageReport` counts `impaired`/`restored` episodes; the headless
  record reports `imp=<i>i/<r>r` on activity only — undamaged runs
  stay bit-identical.

## Tests

- `mm2_game/tests/damage.rs` (+2): ramp boundary legs (1.0 below
  med, `power_at_med` at med, midpoint, floor at/past max);
  degenerate/garbage legs (NaN/neg-infinite totals, NaN spec field,
  `med >= max` band, NaN/out-of-range floors).
- `mm2_vehicle/tests/impairment.rs` (new, 4): a 0.4 factor clearly
  lags the healthy run but still drives; factor 1.0 / NaN /
  infinity / 2.5 are bit-identical to no component; factor 0
  delivers no drive while the rest of the car stays healthy; a
  zero-factor car still brakes to a stop (brakes are not engine
  output).
- `mm2_app/tests/damage.rs` (+3): intact cars carry no component,
  a `MedDamage` crossing inserts `EngineImpairment` mirroring the
  policy factor and counts one episode (deeper damage deepens the
  factor without re-counting); a disabling hit's repair removes the
  component the same tick (`restored`); a remote participant's
  accumulator still tracks damage but its engine is never impaired.

## Evidence

- `cargo test -p mm2_game --test damage`: 10/10 pass.
- `cargo test -p mm2_vehicle --test impairment`: 4/4 pass.
- `cargo test -p mm2_app --test damage`: 14/14 pass.
- `cargo fmt --all -- --check`: PASS; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings`: clean;
  `cargo test --locked --workspace`: 62 suites, 0 failures.
- Retail headless on the supplied install
  (`fnv1a64:e91e6cd4b2ae30d9`):
  - Inactive leg (SF scripted cruise, vpbug): `status=pass
    updates=600 ticks=1200 driver=scripted … dmg=3a/0d/0r rej=3
    dup=0 vsk=6a/0d/0r` — no `imp=`/`ptx=` fields; the run stays
    bit-identical to the pre-change record (vpbug never crosses
    its 150k `MedDamage`). London scripted cruise likewise:
    `… dmg=1a/0d/0r rej=4 dup=0 vsk=5a/0d/0r`, no `imp=`.
  - Active leg (SF, vpcoop `--spawn=-1316,60,381,0`, 6000 frames,
    Hold driver): `status=pass … impacts=133 dmg=23a/0d/0r
    vsk=39a/1d/1r ptx=639e/621x imp=1i/0r` — the tumble run crossed
    the Mini's authored 80k `MedDamage` once: the engine factor
    dropped (`imp=1i`), smoke emitted at the authored pivots, and
    the car never reached `MaxDamage`, so no repair and `0r`. The
    trajectory diverged from the pre-change record (impacts 162→133,
    no disabled outcome, no `brk=`) — expected: once damaged, the
    impaired engine changes the speed profile and therefore the
    later impacts. The impairment is *why* this leg differs; the
    counters show the coupling is live on real authored bounds.

## Classification / open items

- F05-B.7 is `implemented` (candidate) — pending external gates +
  review.
- Classifications: the coupling itself is documented original
  behavior (MM2Hook `PhysicalEngineDamage`); onset bound
  (`MedDamage`), ramp shape and floors are **designed** (DSN-25);
  the original's magnitude/mechanism stays UNK-13.
- Honest gaps: no operator feel-test of impaired driving (headless
  evidence only — the factor is real in the sim; how it plays is
  operator-owned territory); the remote-participant policy is
  exercised only via a synthetic `PlayerControl::Remote` unit test —
  no networking exists.
- Still open in F05-B: `TextelDamageRadius`/`ImpactsTable` texel
  damage + `asLineSparks` sparks (needs a contact-point feed),
  damage-driven detachment if the original uses it, C&R healing
  driver (DMG-4 — needs F27), replication (F25+).
