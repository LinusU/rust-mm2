# Last implementation iteration

- Task ID and title: F10-B.6 — kinematic→dynamic collision handover
  (the F10 spec's "transition to dynamic behaviour without
  duplicating bodies or injecting extreme energy"; the
  dynamic/kinematic-handover edge case; F10-AC03's
  collision-fidelity leg). Selected per the selection policy's
  named F10-B remainder "box-yield/right-of-way/collision" —
  B.5 landed the yield leg, this slice lands the handover leg.
- Starting commit: `cd93ae78930c853b0858d8251a12ffa627f0c100` on
  `ralph/night`; tree was clean, previous external review verdict
  pass (F10-B.5), so this is feature work, not a repair.

## What changed

- `mm2_game::traffic` — new `KnockPolicy` with `min_impulse`
  4000 N·s. Designed value: the original's ambient crash rules
  (whether/how ambients physically react to impact) are
  unverified (UNK-12).
- `mm2_app::contracts` — `impulse_estimate` (deepest-contact
  approach speed × striker `ComputedMass`, 1 kg fallback) moved
  out of `banger.rs` so the handover and banger activation share
  one estimate rather than diverging.
- `mm2_app::traffic` — `AmbientCar` gains `drive: AmbientDrive`
  (`Lane` | `Knocked`). `spawn_ambient_car` adds authored `Mass`
  and `CenterOfMass` (finite validated, bounded fallback) and
  `CollisionEventsEnabled`, so wreck-vs-car and prop-fragment
  strikes register contact events even when the ambient side is
  the passive collider.
- New `knock_ambient` — a third `CollisionStart` consumer
  alongside `collect_impacts` and `activate_bangers`, scheduled
  before `drive_ambient` in both the app and the headless/smoke
  chain. Same authority/phase gate as the driver, and it drains
  the reader while inactive so buffered edges never flush as a
  stale burst on resume. On a qualifying contact it mutates the
  same entity: `drive = Knocked`, `LinearVelocity += normal *
  sign * severity` (bounded at the approach speed — the energy
  the hit carried, nothing amplified), `junctions.depart`,
  `traffic.knocked += 1`, `insert(RigidBody::Dynamic)`. No
  duplicate body, no teleport.
- `drive_ambient` skips `Knocked` cars entirely (no cursor
  advance, gate, pose rewrite or stuck window — the solver owns
  them) and `bound_for` maps `Lane`-mode cars only, so a wreck
  inside the junction box is not shielded from occupancy the way
  a waiting approach is — it holds the yielded approaches like
  any other occupant. The recovery is the ordinary distance
  recycler.
- Smoke record: `kn=` appended to `traf=` only when nonzero, so
  knock-free records stay bit-identical to earlier ones.

## Evidence

- `cargo test -p mm2_app --test traffic` — 22 pass (+3:
  `a_hard_hit_hands_the_follower_to_dynamics` — a 1300 kg striker
  on the lane converts the follower to `RigidBody::Dynamic` on
  the same entity, `knocked=1`, cursor frozen across 60 further
  updates, velocity bounded, striker physically shoved, exactly
  one knocked wreck; `a_light_touch_leaves_the_car_lane_
  following` — a 50 kg striker stays under the floor, the car
  remains kinematic and drives past;
  `a_knocked_wreck_occupies_the_junction_box` — a wreck nominally
  bound for the junction still occupies it and holds the green
  approach at its line through a whole green, releasing on
  despawn).
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets
  --all-features -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, all suites,
  0 failures.
- Retail headless smoke (install `fnv1a64:e91e6cd4b2ae30d9`):
  - `--city sf --frames 600` → `status=pass … traf=16/16 sp=23
    rec=7 dead=0 uns=0 q=0 jq=5 stuck=0 kn=1`.
  - `--city london --frames 600` → `status=pass … traf=16/16
    sp=20 rec=4 dead=0 uns=0 q=0 jq=4 stuck=0 kn=1`.
  - Every counter holds its F10-B.5 value; the scripted `hold`
    drive knocks one ambient car in each city — the handover
    fires on real data, and the wreck counts in `traf=` until
    the recycler collects it.

## Still open

- All collision constants are designed — `min_impulse` 4000 N·s,
  the `approach_speed × striker_mass` estimate (shared with
  DSN-10, provisional for bangers too), and the point-normal
  velocity kick. The original's ambient crash behaviour is
  unverified (UNK-12): no evidence yet that retail ambients
  deform, shove or despawn on impact.
- Approximations kept explicit: one kick at the deepest-contact
  normal rather than per-manifold impulses; a contact whose
  striker lacks `CollisionEventsEnabled` never fires the event
  (ambient cars now carry it, so wreck-on-wreck and
  fragment-on-wreck register); `knock_ambient` matches collider
  entities to ambient roots directly — a future child-collider
  hierarchy would need `collect_impacts`-style body resolution.
- The wreck persists until the distance recycler collects it —
  visible pop near the player remains possible; dynamic wrecks
  get no damage model, no deformation, no audio (none exists
  yet anywhere).
- F10-AC03 is advanced, not closed: player-hit feel/damage,
  lane-change passing, and rendered/manual collision evidence
  are unrecorded. AC05's fixed-seed soak still owes a run over
  the new behaviour; F10-C multiplayer union-of-interest,
  signal-prop rendering and original timing all stay open.
