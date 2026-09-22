# Last implementation iteration

- Task ID and title: F05-B.2 — authored `vehstuck` detection and
  bounded in-place recovery (second runtime leg of F05-B; continues
  F05-AC05's stuck/rollover half while water/OOB recovery stays open).
- Starting commit: `dcddd3e04650ba928b86baf35ce5c4f3b72670a0` on
  `ralph/night`; tree clean.

## What changed

- `crates/mm2_game/src/stuck.rs` (new)
  - `StuckSpec` — the six authored `vehstuck` fields verbatim
    (`From<&VehStuck>`). `rotation`/`translation` decode but are not
    consumed: MM2Hook's recovered `vehStuck` struct gives their
    names, not their tests (UNK-13).
  - `VehicleStuck` component — spawned only when `vehstuck` decoded
    (authored absence = no component, same policy as
    `VehicleDamage`). Designed interpretation of the recovered
    struct (`stuck.h`: `m_State`, `m_StuckTime`, `m_LastImpactPos`,
    squared thresh copies; `Update()` is a binary thunk):
    `impact(pos, rot)` anchors the episode — every new impact
    re-anchors; `observe` per fixed step — inside `pos_thresh`
    accrues, past `move_thresh` disarms (the uniform `move > pos`
    pair reads as a hysteresis band where accrued time holds),
    rotation past `turn` re-anchors the orientation as still
    tumbling, `time_thresh` accrued fires `StuckVerdict::Stuck` once
    then disarms. Garbage (non-finite pose/dt) is a no-op
    observation.
  - `StuckEvent` (object/generation/tick) — bounded: one per armed
    episode.
- `crates/mm2_vehicle/src/systems.rs` — `upright_recovery_pose`
  extracted from `vehicle_self_right` (flatten heading, drop the
  hull onto the surface it rests on) so the authored recovery and
  the modern assist land a car the same way.
- `crates/mm2_app/src/stuck.rs` (new)
  - `track_stuck` (FixedLast after `apply_impact_damage`, before
    `resolve_disabled`, authority + `Playing` gated, drains stale):
    arms each participant's detector off the deduplicated
    `ImpactEvent` stream at the delivered pose, then observes every
    armed detector once per tick — `StuckEvent` per fired episode.
    Remote participants skipped on both legs; `Disabled` wrecks
    skipped (the damage outcome owns them — scheduled before
    `resolve_disabled` so the check sees the pre-repair tier).
  - `resolve_stuck` (after `resolve_disabled`): each detection →
    `ResetVehicle` onto `upright_recovery_pose` — in place, heading
    kept (the authored reading: `Rotation` 0, `Translation` ≈ 0.1
    on every retail record — no positional rescue or yaw change is
    authored). Local + AI identical (designed, UNK-13); local
    trailers re-seat at authored offsets off the recovered pose;
    remote/unidentified objects skipped. The reset marks
    `Teleported`, so the hop cannot sweep a checkpoint.
  - `StuckReport` (`armed`/`detections`/`recovered`) — session
    scoped, reset on teardown, feeds the `vsk=` smoke field.
- `crates/mm2_app/src/damage.rs` — `resolve_disabled` disarms a
  resolved wreck's `VehicleStuck`: the disabled outcome supersedes
  any armed episode, so it cannot fire a second recovery into the
  pose the outcome lands the car in.
- `crates/mm2_content/src/{convert,assemble}.rs` — `ConvertInput`
  carries the decoded `VehStuck`; authored `TimeThresh` feeds
  `assists.self_right_delay` (the modern assist keeps covering
  impact-free rollovers the detector never arms on).
- `crates/mm2_app/src/session.rs` — player spawn attaches
  `VehicleStuck` when `def.stuck` is authored; `drive_session`
  resets `StuckReport` on teardown (all test harnesses gained the
  resource).
- `crates/mm2_app/src/opponents.rs` — same attachment for AI.
- `crates/mm2_app/src/main.rs` + `smoke.rs` — `StuckEvent` message,
  `StuckReport` resource, both systems in the FixedLast chain;
  `vsk={a}a/{d}d/{r}r` smoke field (only when the pipeline saw
  activity — inactive records stay bit-identical).
- Tests: +9 mm2_game contract (unarmed never fires, fire at
  `time_thresh`, escape disarm, hysteresis band holds, `turn`
  re-anchor, π-turn still counts, re-anchor on new impact, garbage
  observations, verbatim spec) and +11 mm2_app integration
  (arm→detect→recover in place once, drive-away escape, unimpacted
  negative, roofed righting, `turn`-bound wedged, AI recovery, remote
  skip, disabled skip, stale-generation skip, trailer re-seat, real
  4 m roof-drop end-to-end).

## Evidence

- `cargo test -p mm2_game --test stuck` — 9 pass.
- `cargo test -p mm2_app --test stuck` — 11 pass.
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, 734 tests across 54
  binaries, 0 failures (was 714/52; +20 tests, +2 binaries).
- Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`):
  - `mm2 --mm2-path <retail> --city sf --headless --frames 600` →
    `… impacts=9 … dmg=2a/0d/0r rej=1 dup=0 vsk=3a/0d/0r`
  - `--city london --headless --frames 600` →
    `… impacts=7 … dmg=2a/0d/0r rej=1 dup=0 vsk=3a/0d/0r`
  - Three delivered impacts arm the player's detector per city; none
    persist because the scripted driver keeps moving — the detector
    correctly measures escape, not parking. All pre-existing
    counters bit-identical to the F05-B.1 records.
- Evidence level: code gates + synthetic integration + retail
  content-driven headless run. No GPU/rendered evidence needed
  (no visual change); no original-executable behavioral comparison
  (Update body unrecovered — designed interpretation, UNK-13).

## Still open (F05-B/F05-C scope)

- Visual tiers: smoke pivots (`SmokeOffset`/`SmokeOffset2`,
  `DoublePivot`/`MirrorPivot`), `TextelDamageRadius` decals, damage
  effect spec — decoded but unrendered.
- Breakaway lifecycle: REC-1's authored inventory is inventoried only;
  detachment rules (damage- vs impact-driven) stay UNK-13.
- Impairment short of destruction; `vehgyro` consumption;
  water/out-of-bounds recovery (F05-AC05's remaining legs);
  DMG-4 C&R healing (channel exists, no mode drives it);
  replication (authority-gated but untested over the wire).
- `vehstuck` `Rotation`/`Translation` semantics unrecovered —
  decoded verbatim, not consumed.
- Designed (not original-verified): the whole test combination
  (impact anchor + hysteresis + tumbling leg + time window),
  in-place upright recovery, AI stuck recovery, `TimeThresh` →
  `self_right_delay` binding.
- F05-B.2 is candidate pending external check.
