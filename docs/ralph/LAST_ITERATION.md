# Last iteration — water/out-of-bounds recovery

Iteration 33 on `ralph/night`, continuing from `97b4238` (the
externally checked F05-B.4 gyro candidate — review verdict pass).
Task id: `F05-B.5` — the spec's remaining safe-recovery leg
("water/out-of-bounds recovery" in F05 req 5 / F05-AC05), following
the established detector → bounded-event → `ResetVehicle` pattern
(B.2 `vehstuck`, B.3 `dgbangerdata`, B.4 `vehgyro`).

## Slice choice

The previous candidate is externally clean. Of the F05-B remainder —
visual tiers, damage-driven detachment-if-original, impairment,
water/OOB recovery, C&R healing, replication — water/OOB is the
highest-value ready slice: it is the last unimplemented named
recovery in spec req 5, it has real user-facing stakes (today a car
that enters the Thames is stranded crawling at ~1 m/s forever, and a
car that leaves the world falls until the smoke check fails), and it
needs no blocked input (C&R healing wants the C&R mode, replication
is F25+). It is also the only recovery leg with *no authored data*:
the damage-family census (DMG-5..8) covers `vehcardamage`/`vehstuck`/
`vehgyro` only, and DMG-2 covers destruction — so every bound is
designed (DSN-23) and UNK-13 keeps the original's actual rules open.

## What changed

- `mm2_game::recovery` (new contract module):
  - `RecoveryPolicy` — designed bounds: `water_min_drag` 0.3 (splits
    retail `deepwater` 0.5 from shallow `water` 0.119 — a pond stays
    wadable under the F06-B.2 policy, the Thames drowns),
    `submerge_dwell` 2.0 s (the escape window: a car that regains a
    dry edge inside it keeps driving), `fall_margin` 50 m (sized past
    retail drops — any real landing refreshes the anchor first).
  - `GroundContact` — Airborne/Dry/Submerged, classified app-side
    from `WheelState::surface_drag` (the same coefficient the wading
    force reads) so the contract never borrows the wheel type.
  - `VehicleRecovery` component — anchor = the last pose a grounded
    wheel sat on a *dry* surface (water colliders are solid in this
    engine: drowning is a surface class under the wheels, not a
    missing floor); pre-anchored at spawn; all-wet contacts accrue
    the dwell → `RecoveryCause::Submerged`; an airborne fall past the
    margin below the anchor → `RecoveryCause::OutOfBounds`, latched
    once per fall; a non-finite pose fires at once as
    defence-in-depth (a NaN inside the physics step trips the wheel
    raycast first — the detector only answers between-step writes).
  - `RecoveryEvent` — bounded: a fresh dwell per fire, one per fall.
- `mm2_app::recovery`: `track_recovery` (FixedLast, authority +
  Playing gated — observe-only so pause just freezes the dwell)
  advances every participant's detector; `resolve_recovery` answers
  with `ResetVehicle` to the anchor — `Teleported`, so no checkpoint
  sweep — with the session `SpawnPoint` as the no-anchor fallback.
  Trailers re-seat at authored offsets (the `resolve_stuck`
  pattern); an armed `VehicleStuck` episode disarms; remote
  participants and `Disabled` wrecks are skipped (their authorities /
  the damage outcome own them). **Recovery is not a repair** — damage
  and detached parts persist; only the disabled outcome heals.
- Spawn: `VehicleRecovery` attaches to the player vehicle (any def —
  dev cars included) and every AI opponent, anchored at its spawn
  pose. No authored gate exists to spawn against.
- `mm2_app::smoke`: `rcv=<w>w/<f>f/<r>r` (submerged / out-of-bounds /
  recovered) only when the pipeline saw activity — a dry-ground run
  stays bit-identical. `drive_session` resets the report on teardown.

## Tests

- `mm2_game/tests/recovery.rs` (9): dry-contact anchoring; one fire
  per dwell then a fresh dwell required; dry escape inside the
  window; spawn-straight-onto-water fires with `landing: None`;
  fall-past-margin fires once and latches until grounded; a legit
  drop lands first and re-anchors; non-finite pose fires OOB at
  once; `recovered` clears the episode and re-anchors; garbage dt
  never accrues + zero-dwell edge.
- `mm2_app/tests/recovery.rs` (11): deep-water dunk recovers to the
  last dry anchor end-to-end (real wheel raycast onto a `drag` 0.5
  collider) and stays done; shallow-water (0.119) wading never
  starts the dwell; powering out inside the dwell escapes; falling
  off the world recovers to the anchor; a legit big drop lands
  instead of firing; recovery-is-not-a-repair (damage total
  preserved); remote participant never tracked/resolved; `Disabled`
  wreck skipped; stale-generation event ignored; recovery disarms an
  armed stuck episode; trailer re-seats at its authored offset.

## Evidence

- `cargo test --locked -p mm2_game --test recovery`: 9/9 pass.
- `cargo test --locked -p mm2_app --test recovery`: 11/11 pass.
- Quality gates: pending (fmt/clippy/`cargo test --workspace` run at
  commit time; retail headless smoke on the supplied install below).
- Retail headless smoke — sf/london cruises should show no `rcv=`
  (the scripted driver never leaves dry ground → bit-identical); a
  `--spawn` over the Thames exercises the submerged leg on real
  content.

## Classification / open items

- F05-B.5 is `implemented` (candidate) — pending external gates +
  review.
- Classifications: the whole detector is **designed** (DSN-23) — no
  authored record exists to decode. The original's water/OOB rules
  (reset to shore? last checkpoint? in place? what submersion/OOB
  tests?) stay UNK-13, explicitly open.
- Honest gaps: no rendered/GPU proof of a recovery; the retail
  Thames run exercises the detector headlessly; the non-finite arm
  is unit-tested defence-in-depth, not a pipeline path (physics
  raycasts panic on NaN first); a degenerate spawn straight onto
  water loops recovery↔dwell by design (bounded, counter-visible).
- Still open in F05-B: visual tiers (smoke pivots,
  `TextelDamageRadius`, `DoublePivot`/`MirrorPivot`), damage-driven
  detachment if original, impairment, C&R healing, replication.
