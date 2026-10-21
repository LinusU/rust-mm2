# Netcode redesign: shared contract, plan and baseline

Owner: NETPLAY-01 (#1762) maintains this note; later NETPLAY tasks append
their evidence. It is a design and evidence record, not a claim that
Internet play is solved. Loopback passes, this document and a green task
queue are **not** observed Rocket League parity: that claim additionally
needs reachability evidence (OWNER #1240) and the real-Internet
driving/feel session (OWNER #1249).

## Problem

On 2026-10-20 the owner reported multiplayer unplayable for non-hosts:
their own car repeatedly teleports while the host drives well. Every
participant must get immediate local control, stable presentation and
convincing car-to-car contact (Cops & Robbers needs real physical
contact).

## Audit (code at main `e05819b`, re-checked on this branch)

Known **static-code defects** (read from the source, not all separately
measured):

- `netdrive::DriftWatch` compares the *current* predicted own pose with
  a *historical* host pose: >15 m for 30 snapshots pulls 15%, >20 m
  snaps the physics body. The two poses are not the same simulation
  instant, so a fast car looks "divergent" by construction.
- `SettleWatch` reseats the own seat when it rests apart from the host.
- Client remote cars are Kinematic/`RemoteReplica`; host counterparts
  are dynamic, so a client cannot predict contact.
- `DriveInput` carries no simulation-tick contract and snapshots carry
  no applied-input acknowledgement, so no replay is possible.
- Unpaced processes tick at unrelated rates, so the host replays a
  client's inputs against a different alignment than it predicted.

## Direction (shared contract)

Host authority; immediate local prediction on the existing production
vehicle/Avian simulation at the fixed 60 Hz step; historically aligned
reconciliation and replay; coherent predicted interactions; presentation
smoothing separate from simulation correction. Handling and tuning stay
owner-owned and untouched. Normal corrections never pull today's body
toward an old host pose, never disable collisions and never trust
client-submitted positions or scores.

Invariants:

- Command `C[n]` controls one 1/60 s step `S[n-1] -> S[n]`; `S[n]` is
  captured after the whole vehicle, Avian, custom-contact and post-step
  work of that tick. Snapshot `server_tick = n` means that boundary.
- Client command sequence, client predicted tick and server tick stay
  distinct until a mapping relates them explicitly.
- An input acknowledgement means the command's effect is in completed
  authoritative state (or its slot was finalized by the documented
  missing-input policy), not merely received.
- State includes session generation, vehicle lifetime/reset epoch and
  tick-associated damage, gold-mass and trailer changes.
- Restore/replay must not advance the live session clock or unrelated
  world systems a second time.
- Unfinished NETPLAY-04..08 behaviour sits behind the one development
  selector defined by NETPLAY-02; NETPLAY-09 validates and enables it.

## Task plan

NETPLAY-01 (#1762, this harness and baseline) -> NETPLAY-02/03 (start in
parallel; 02 defines the tick/state contract and the selector) ->
NETPLAY-04 (#1765, historical prediction and replay replacing the drift
snaps) -> NETPLAY-05..08 (predicted interaction, presentation smoothing,
and the remaining slices; see their Rally tasks) -> NETPLAY-09 (validates
the integrated path, enables it for menu/CLI host and join and headless
runs, and turns the strict checker below into a regression gate) ->
NETPLAY-10 (#1771, congestion and freshness). Human gates: OWNER #1240
(reachability) and #1249 (driving feel). This note does not duplicate
the other tasks' scopes; consult Rally for them.

## Primary sources

- Psyonix, GDC 2018, *It IS Rocket Science*: fixed-frame input, history
  comparison, actor replay. <https://media.gdcvault.com/gdc2018/presentations/Cone_Jared_It_Is_Rocket.pdf>
- Ubisoft, GDC 2017, *Replicating Chaos*: trajectory debugging and
  collision timeline trade-offs. <https://media.gdcvault.com/gdc2017/Presentations/Matt_Delbosc_Replicating_Chaos.pdf>
- iRacing braking/bump-draft cases: <https://support.iracing.com/support/solutions/articles/31000164646-2022-season-2-release-notes-2022-03-08-01-.>
- Rocket League input-buffer diagnostics: <https://www.epicgames.com/help/c-37599050/c-trending_0/a16015734>,
  <https://www.epicgames.com/help/c-37599050/c-42869540/a11386071>
- Input-cadence/stall bugs: <https://www.rocketleague.com/news/tournaments-update-follow-up>
- Server health despite low ping: <https://www.rocketleague.com/news/rocket-league-server-performance-update>

These inform a rust-mm2 design decision; they are not a claim to copy
another game's implementation.

## The harness

```sh
scripts/netcode-baseline.sh              # baseline/report mode, ~4 min
MM2_NET_STRICT=1 scripts/netcode-baseline.sh   # also gate on Limits::STRICT
```

It runs `crates/mm2_app/tests/net_baseline.rs`: a real `mm2 --host` and
two `mm2 --join` processes (dev-world cruise, headless, seed 11, loopback
UDP, no impairment proxy), each driving a scripted route through the
production `VehicleInput` path (`--net-route accel|slalom`, `--ram` for
contact; scripts supply inputs, never poses). Each process writes a
per-update JSONL trace of its own car (`--net-trace`); `mm2_app::netprobe`
reduces it to metrics. Output: `<scenario>-<seat>.jsonl` and
`<scenario>.json` under `MM2_NET_REPORT_DIR` (default
`target/captures/netcode-baseline`), recording seed, frame cap (2400
client updates, about 40 simulated seconds per seat), transport and
args. Children are `Proc`s killed by PID on drop; every wait is bounded
by the client frame budget. Perturbations are separately injectable:
`--paced` (wall-clock 60 Hz; omit on one process for unequal frame
pacing), `--net-stall EVERY:MS` (whole-process stall, bounded to 2 s).

Measurement definition. A *correction* is the part of a tick's
displacement the car's own velocity does not explain:
`|p[n] - p[n-1] - (v[n-1] + v[n]) / 2 * dt|`. High-speed travel is
therefore not a teleport (unit-tested at 70 m/s), and a sample pair
across an authority-reset epoch change is skipped and counted as a
respawn. Reported: the full residual distribution *and* the
nonzero-only distribution (zero samples do not hide a bad correction),
counts and per-minute rates above 0.1/0.25/0.5/2 m, cumulative
correction per minute, and yaw flips (opposite yaw steps over 2 degrees
within 1 s). `Limits::STRICT` (0.5 m largest correction, 2/min above
0.25 m, 2 yaw flips/min) is a placeholder for NETPLAY-09's targets.

Fault injection. `--net-fault kicks|yaw` injects 0.6 m sideways kicks or
alternating 5-degree yaw kicks every 15 applied snapshots *inside the
real own-seat reconcile code*. The checker, which reads only the
trajectory, flags both (`the_checker_flags_injected_*`); both runs were
also observed to fail through `own_correction kind=fault` records.

### Not yet measured (extension points, not guesses)

The current protocol has no input tick, applied-input acknowledgement,
host buffer occupancy, clock lead, snapshot age, replay depth or history
misses, so those columns do not exist; `Sample` already carries
`snap_tick` and `epoch`. Still open for later tasks: sampled-input-to-
physics and -to-render-submission timing, separate render-correction
versus simulation-correction series, per-peer wire load, tick deadline
misses, causal-versus-predetermined multiplayer modes (the routes here
are predetermined; no client sees another's script, but no live
prediction of remote drivers exists yet to test), burst-loss and shared-
uplink recipes, and slow-host-physics injection. Render stalls cannot be
injected headless; whole-process stalls are.

## Baseline (current production code, this branch)

Run 2026-10-20 on one Apple-silicon dev machine under other agents'
load; headless, so no render or camera discontinuity was measured. The
owner's exact session (RTT, machine, logs) is **not** reproduced and is
not invented here. "max" is the largest single-tick correction; the car
is alice's (slalom) unless noted.

| scenario | seat | max correction | >0.25 m / min | cumulative m/min | yaw flips | logged own corrections |
|---|---|---|---|---|---|---|
| clean (all paced) | host | 0.19 m | 0 | 0.6 | 0 | - |
| clean | alice | **22.2 m** | 6.0 | 44.5 | 1 | 1 snap, 3 pull |
| clean | bob (accel) | 0.19 m | 0 | 0.6 | 0 | - |
| client stall (100 ms / 120 updates) | alice | 2.5 m | 6.0 | 15.1 | 0 | 4 pull |
| contact (alice `--ram`) | alice | **22.1 m** | 28.5 | 102.4 | 0 | 1 snap, 18 pull |
| unequal pacing (host unpaced) | alice | **90.5 m** | 34.5 | 1994.9 | 7 | 23 snap |
| unequal pacing | bob | **97.4 m** | 21.0 | 1345.1 | 1 | 14 snap |

Measured failures: on clean loopback with no delay the slalom client's
own car is teleported 22 m by a physics-body snap; when processes tick
at unrelated rates both clients are snapped 14-23 times in 40 s by up to
~97 m; ramming adds repeated pulls. A bob that only accelerates in a
straight line is not corrected, so the defect depends on the route
(turning and braking), which is consistent with the static defect that a
current pose is compared with an old host pose. The host seat is always
clean. These are the numbers NETPLAY-04 must beat; they fail
`Limits::STRICT` by orders of magnitude.

Caveats: single run per scenario (no variance bound yet); alice's 22 m
snap in the clean case occurs in all runs seen so far (three) but the
sample is small; the two-client, one-host topology and dev-world map
(open ground, no buildings) are not the owner's CnR city session.

## Final quality targets

To be set by NETPLAY-09 from this harness; until then the placeholder is
`Limits::STRICT` above. The existing 40 m moving sanity ceiling and the
50-58 s settling waits in `net_drive.rs` are **not** driving-quality
evidence and are not accepted as such.

## Vehicle state capture/restore (NETPLAY-03A, #1778)

`mm2_vehicle::capture` holds the typed DTO (`VehicleCapture`, serde, no
`Entity` ids, no ECS reflection) plus `capture_vehicle` / `restore_vehicle`
over a `World`. Take captures between fixed steps, never inside one.
`restore_vehicle` validates (wheel count, model, finite values) before it
writes anything. `VehicleCapture::deviation` compares two captures:
discrete state exactly, floats by largest absolute difference.
Tests: `crates/mm2_vehicle/tests/capture.rs`, production `VehiclePlugin`
schedules at `FIXED_HZ` 60, generic and original model both.

Classes: **T** transmitted (the authority must send it, or the receiver
cannot step the car identically), **H** locally historical (the owner of
the simulation keeps it; a rewind restores it from its own history),
**R** safely reconstructed (recomputed before anything reads it), **I**
immutable (config; never captured). The wire choice of T versus H is
NETPLAY-03D's; this table is the closure of what a step reads.

| Field | Class | Why |
|---|---|---|
| `Position`, `Rotation`, linear/angular velocity | T | the body |
| `Mass`, `AngularInertia` (principal, frame) | T | Cops & Robbers gold changes them mid-match |
| `EngineImpairment` factor | T | damage is tick-associated state |
| `VehicleInput` (throttle, brake, steering, handbrake, forced gear) | T | the command in effect; replayed commands come from the input log |
| `PreStepVelocity` | H | rewritten by `vehicle_simulation` before any read; captured for an exact restore |
| `VehicleState`: `steer_angle`, `direction`, `gear`, `engine_load`, `shifting`, `upended_for`, `gyro_spin` | T | steering/drivetrain/shift/assist latches read by the next step |
| `VehicleState`: `rpm`, `grounded`, `forward_speed` | H | outputs of the last step; the generic model reads them back (rpm response, traction) |
| `VehicleState`: `gyro_spins`, `gyro_completed` | H | run evidence, never read by the sim; kept so a restore does not rewrite history |
| `WheelState` contact point/normal, `compression`, `suspension_force`, velocities, slip, forces, `cornering`, `traction_demand`, `spin` | H | suspension/contact history the next step differences against |
| `WheelState` `surface_grip`, `surface_drag`, `slip_visual` | R | rewritten by the tyre path each step; captured for exactness |
| `WheelState::contact_entity` | R | an `Entity`; re-derived by the next wheel ray cast, dropped from the DTO |
| `OriginalState::powertrain` (gear, engine omega, clutch, torque, rpm, time in gear, gear-change latch, lag left, rpm at shift) | T | the private latch is captured too (`PowertrainHidden`) |
| `OriginalState`: `drive_omega`, `drive_bias`, per-wheel `x`, bristles, `reaction`, `omega` | T | tyre stick-slip and wheel spin are state in the original model |
| `OriginalState`: `pending_force`, `pending_torque`, `angular_momentum`, `integrated_angular_velocity` | H | force/torque accumulation carried across the step boundary |
| `OriginalState`: `player_speed`, `steering_speed` | H | one and two samples old speeds the player rules read |
| `OriginalState`: implicit Jacobians A/B/C | H | coupled suspension solve of the previous step |
| `OriginalState`: `push`, `last_push`, `applied_push` | H | bump-stop corrections, read by wheel point velocities |
| `OriginalState`: `collision_impulse`, `collision_angular_impulse` | H | static-world contact momentum consumed by the next solve |
| `OriginalState`: `step_start_position`, `step_start_rotation` | R | rewritten at the start of every step |
| Avian force accumulators | R | emptied by `original_step::integrate` every step; empty at a boundary |
| `OriginalContactSnapshots` | R | only live between the solver's prepare and restore inside one step |
| `Transform` | R | rewritten from the restored pose |
| `EpochRef` (session generation, reset epoch) | T | opaque to the vehicle crate; the replay layer rejects stale epochs |
| `VehicleConfig`, `OriginalHandling`, collider, `StrikeBound` | I | never captured |
| Avian contact manifolds, solver warm start, broad phase, sleep timers | see below | physics world; NETPLAY-03B (`mm2_vehicle::physics_world`) |
| Trailer hitch joint, other dynamic bodies | see below | NETPLAY-03B; `HumanDriver`, `RemoteReplica`, input filters in the app stay NETPLAY-03C/D |

Findings:

- **Reset mismatch (AC2).** `vehicle_reset` rebuilds `VehicleState::new`
  and zeroes the body velocities; a pose-only snap lands the same pose
  and velocities but keeps tyre bristles, wheel spin, engine/gear,
  suspension and force history. Same pose, same commands: after 60
  ticks the largest capture difference is ~157 (generic) and ~4968
  (original, a wheel force in N). A reset equals a freshly spawned car
  exactly (0 deviation) once one step has rewritten `PreStepVelocity`,
  which `vehicle_reset` leaves stale (harmless; nothing reads it before
  the step writes it).
- **Replay (AC1).** Capture at T, run steer/brake/forced-gear/handbrake/
  coast to T+N, restore T, replay the same commands: the generic and the
  original path both come back with a maximum float deviation of **0**
  (bit-identical) against an uninterrupted run, at every segment end and
  on a second rewind. The test tolerance is `1e-3`, kept loose on
  purpose so a legitimate libm/platform difference is not a false alarm.

Determinism limits (what 03A does **not** prove): one machine, one
build, a flat ground plane. Bit identity relies on the fixed step being
exactly one tick per update and on Avian's solver being reproducible for
the same inputs; it was observed with Avian's contact warm-start cache
left as the replay found it, which is only safe on that fixture. Contacts
with props, walls, other cars and sleeping bodies are NETPLAY-03B, below.
Cross-platform float determinism is not claimed anywhere.

## Avian 0.8 physics-world restore (NETPLAY-03B, #1779)

`mm2_vehicle::physics_world`: `capture_world(&mut World, EpochRef) ->
WorldCapture` and `restore_world(&mut World, &WorldCapture) ->
RestoreReport`. Local history (class H): it names `Entity` ids of one
world and never goes on the wire. Call between fixed steps. Tests:
`crates/mm2_vehicle/tests/physics_world.rs`, the production
`VehiclePlugin` schedules at 60 Hz, one step per `update()`.

### What Avian 0.8 keeps between steps, and what we do with it

| Avian state | Strategy | Why |
|---|---|---|
| `Position`/`Rotation`/velocities of dynamic bodies | **restored** (vehicles through `restore_vehicle`, props through `BodyState`) | the body |
| `Sleeping`, `SleepTimer` | **restored**; see sleeping below | a prop at rest is part of the world |
| `ContactGraph` manifolds (points, `normal_impulse`, warm-start normal/tangent impulses, feature ids) and the contact recycling cache | **restored** per collider pair when the manifold layout is unchanged, otherwise **invalidated** (cold start, recycling cache dropped) | the next solve warm-starts from them; a pair born in the abandoned future must not warm-start the replay |
| `ConstraintGraph`, `PhysicsIslands`, `BodyIslandNode`, `JointGraph` | **left to Avian**, rebuilt lazily | the narrow phase removes/creates pairs and Avian's own `SleepBody`/`WakeBody` commands split or merge islands; there is no public way to rewrite them consistently. Joints (the trailer hitch `SphericalJoint`) carry no solver state in 0.8: anchors and limits are component fields, re-solved from the restored poses |
| `ColliderTrees`, `MovedProxies`, `ColliderAabb`/`EnlargedAabb` | **left to Avian**, refreshed through change detection | writing `Position`/`Rotation` marks the collider moved after `LastPhysicsTick`, so the next step re-fits its AABB and tree proxy. Not snapshotted |
| `LastPhysicsTick` | **never written** | it is a Bevy change-detection `Tick`, not a simulation tick; assigning a simulation tick to it would hide the restore from Avian's change detection. `restore_leaves_avians_change_tick_and_the_cars_awake` pins it |
| `Time<Physics>`, `Time<Substeps>` clocks | **not restored** | nothing a step reads depends on their elapsed value; the simulation tick belongs to the replay layer |
| `CollidingEntities`, collision start/stop events | **not restored** | derived from the contact graph; a replay re-emits events from the live graph. Gameplay that reacts to them (impacts, damage, pickups) must be suppressed or deduplicated by the replay layer: NETPLAY-03E |
| Force accumulators, `OriginalContactSnapshots` | n/a | empty at a step boundary (see the 03A table) |

The Lightyear reference at `8abe00453b52` (`crates/integration/avian/src/rollback.rs`)
was read as an inventory. It rolls back `ContactGraph`, `ConstraintGraph`,
`JointGraph`, `PhysicsIslands`, `BodyIslandNode`, `Sleeping`, `SleepTimer`,
both clocks, and snapshots `ColliderTrees` + `MovedProxies` + `ColliderAabb`
+ `EnlargedAabb` + `ColliderTreeProxyKey` wholesale, then re-registers
colliders whose proxy keys went stale and rebuilds `CollidingEntities`.
That makes the whole persistent Avian state a function of the tick. We
chose the smaller surface: restore what the solve reads and the poses,
invalidate pairs we cannot restore, and let Avian's own change detection
and narrow phase rebuild the rest. The measurements below are what that
costs; if a scenario later shows the broad phase or island graph
diverging, snapshotting them is the next step, not a rewrite.

### Sleeping

Avian wakes a sleeping body when its `Position`, `Rotation`,
velocities or `SleepTimer` *change-detect* (even to an equal value), so
`restore_world` only writes what differs. A body captured awake and found
asleep is woken (`WakeBody`). A body captured asleep and found awake is put
to sleep (`SleepBody`) when its island holds nothing else. If a stale
contact still links it to an awake island (the trailer rig reached the prop
after the capture), sleeping it now would sleep the cars too, so the
restore parks it in `PendingSleeps`; `finish_sleep_restore`, a system at
the end of the `PhysicsSchedule` (before Avian advances `LastPhysicsTick`),
writes the captured state back and sleeps it once the narrow phase has
dropped the stale contact, unless it touches a live dynamic body. The
result is bit-exact: sleep flags match the reference on every tick.
`SleepingDisabled` stays on every car; cars never sleep. A prop that is
jointed to a dynamic body is not handled (none exist).

### Measured tolerances

Scene: an original-model human car (child sphere collider + sensor on its
own collision layer, driving into a static wall until pinned), a generic
car towing a jointed trailer into a sleeping prop, ground. `TOLERANCE` is
`1e-3` per float in the field's unit (m, m/s, rad, N).

| Rewind (ticks) | Crosses | Largest deviation over 40+ replayed ticks |
|---|---|---|
| 19→20 | loaded tyres, depth 1 | 0 (bit-identical) |
| 60→120 | brake, forced gear change | 0 |
| 100→120 | prop asleep throughout | 0 |
| 150→200 | wall contact onset | 0 |
| 236→300 | multi-step wall pin | 0 |
| 299→300, 300→380 | pinned, depth 1 and 80 | 0 |
| 190→250 | **new** car–prop contact onset after the rewind | position 4e-5 m, velocity 3.5e-3 m/s over 140 ticks; per-wheel force scalars up to ~65 N on the impact tick |
| 204→, 261→, 308→ (generic car, ramp jump) | before takeoff, mid flight, after landing | 0 (bit-identical) |
| 306→, 339→, 363→ (original-handling car, ramp jump) | before takeoff, mid flight, after landing | 0 |

The two ramp rows come from `tests/replay_jump.rs` (#1799): both models
drive full throttle up a 0.2 rad static ramp, leave the lip (all wheels
off the ground for 84 and 37 ticks), land on the ground plane and brake.
Each flight is rewound to before takeoff, mid-flight and after landing;
the whole world is compared with the uninterrupted run at every replayed
tick (about 200 per rewind) with `TOLERANCE`. Measured deviation 0 in all
six cases. Contact onsets with the ramp lip and the landing are
static-geometry contacts, so they fall in the exact class, not the bounded one.

The 190→250 row is a floor, not a defect to be tuned away: the replay
creates the contact pair again in a world whose contact slots and solver
ordering were built in a different order than the reference's, so a
chaotic impact differs at the 1e-6 level and the wheel forces carry that
noise for a few ticks. Positions and velocities do not drift (the test
bounds them at 1e-3 m and 2e-2 m/s). Exactness is therefore claimed for
replays whose contact pairs already exist at the restore point; a new
dynamic contact onset is bounded, not exact. `skipping_the_world_restore_makes_the_replay_diverge`
shows the same replays without the world restore are off by 2.5 to 7
(wheel force scalars) on the first tick, and wake the sleeping prop.

Changed body membership: a dynamic body spawned after the capture is
reported in `RestoreReport::uncaptured` and left alone (the replay layer
despawns it; the continuation then matches to `TOLERANCE`); a captured
body that no longer exists is reported in `missing` and skipped; a vehicle
whose capture no longer fits is reported in `refused` and written
nowhere.

Not covered, and not claimed: other platforms, `CollidingEntities` and
collision events (above), bodies with joints other than the trailer
hitch, more than one prop island, and the cost of `capture_world`
(NETPLAY-03F).

## Controlled fixed-step replay (NETPLAY-03C, #1780)

`mm2_vehicle::replay`: `replay_from(&mut World, &HistoryRing, from_tick,
&[ReplayStep]) -> Result<ReplayReport, ReplayError>`. It restores the
`WorldCapture` recorded for `from_tick` (03B) and then runs the
`FixedPostUpdate` schedule once per step, with the generic `Time` set to one
fixed step as `FixedMain` does. That is Avian's outer preparation and
write-back sets around the `PhysicsSchedule`, which in turn holds the
vehicle step, world queries, cached-force integration, collision and
substeps, original static-contact processing and the push-out, in the live
order. Nothing else runs: no `Update`/`Last` (no render frame), no
`FixedPreUpdate`/`FixedUpdate`/`FixedLast` (session clock, race, audio,
network, bots, world systems).

Running only the inner `PhysicsSchedule` was tried first and rejected: it
matched the reference bit for bit through the replayed window, then the
first *live* step afterwards was off by ~11000 N on a wheel force, because
Avian's outer `Prepare` work had not run for the replayed ticks. With
`FixedPostUpdate` the live run continues bit-identical to an uninterrupted
one (`live_ticks_after_a_replay_match_an_uninterrupted_run`, rewinds of 1
and 10 ticks across the wall contact onset).

| Concern | What the replay does |
|---|---|
| `Time<Physics>`, `Time<Substeps>`, generic `Time` | saved before, written back after; `Time<Virtual>`/`Time<Fixed>` are never touched |
| Messages written inside a tick (`CollisionStart`/`CollisionEnd`, plus any type an app registers with `quarantine_message_in_replay::<M>()`) | the live queue is swapped for a scratch queue for the replay and put back untouched; scratch contents are counted in `ReplayReport::suppressed_messages` and dropped, so live readers never see them |
| Systems in `FixedPostUpdate`/`PhysicsSchedule` with a presentation or irreversible effect | gate with `.run_if(not_replaying)` (`ReplayContext` is active exactly while a replay runs); the native skid-timing capture in `mm2_app::audio` is the one such system today |
| Bound | `MAX_REPLAY_STEPS` = 120 ticks (2 s); `HistoryRing` holds `MAX_HISTORY` = 121 captures, evicts the oldest and drops everything from a re-recorded tick on |
| Preconditions | checked before any write: no steps, too many steps, re-entrancy, paused physics, zero timestep, missing history |

**Missing history is a distinct operation.** `ReplayError::MissingHistory`
writes nothing. The caller takes `recover_missing_history`, which adopts the
authority's complete `VehicleCapture` (`Recovery::AdoptedAuthoritative`):
tyres, engine, suspension and force history all come from it, no replay runs
and a refused capture leaves the car as it was. `VehicleState::new` is built
only by a spawn or by a genuine reset (`ResetVehicle` → `vehicle_reset`);
nothing in the replay or recovery path constructs it.

Scope notes: a vehicle with no entry in a `ReplayStep` keeps the command it
has; the caller supplies commands for every car it wants driven.
Quarantine covers message queues only: a system that mutates a resource or a
component outside the captured set from inside `FixedPostUpdate` must gate
itself. Wiring the ring and replay into the client's correction path is
NETPLAY-04; deduplicating gameplay reactions to the replayed contacts
(damage, pickups) beyond suppressing the events is NETPLAY-03E.

## Authoritative-state wire round trip (NETPLAY-03D, #1781)

`mm2_vehicle::wire`: `capture_authoritative(&mut World, generation,
reset_epochs, tick) -> AuthoritativeState` on the host and
`apply_authoritative(&mut World, &AuthoritativeState, session_generation,
expected_reset_epoch)
-> ApplyReport` on a peer. The DTO is serde and format-agnostic; the byte
encoding is the transport's choice (NETPLAY-02 owns the message layout; this
module does not depend on it). The tests encode with `serde_json`, which
round-trips every f32 exactly, and the decoded value must equal the sent one.

**Identities.** A vehicle is a `VehicleSlot(u16)` (seat or trailer slot), a
dynamic prop a `PropSlot(u16)`, both components the session stamps
identically on every peer. Every vehicle carries its own `EpochRef`
(session generation and reset epoch). No `Entity`, Avian proxy or collider
id is in the snapshot. A vehicle or dynamic body without a slot makes
`capture_authoritative` fail (`UnslottedVehicle`/`UnslottedProp`) rather than
be skipped. A peer maps slots to its own entities at apply time; snapshot
slots it lacks are reported (`unknown_*`), local vehicles the snapshot does
not mention are left alone (`unmentioned_vehicles`).

**One tick, not a history.** A snapshot is the post-step state of one host
tick: the full `VehicleCapture` of each vehicle and `PropWire` (pose,
velocities, sleep flag and timer) of each prop. It is about 9 kB of JSON for
three vehicles and one prop; the binary size is the transport's. The 121-tick
`HistoryRing` and the contact graph stay local. Bounds: `MAX_VEHICLES` 64,
`MAX_PROPS` 512.

**Field classification on the wire.** The 03A table's T and H classes both
travel inside the vehicle's `VehicleCapture`: a receiver's own H history
(suspension, tyre and solver history) describes a different timeline, so
adopting the authority's H as well is what makes a receiver with divergent
history converge, and splitting T from H on the wire would only save bytes
at the cost of exactness. R fields are written but re-derived by the next
step; I fields never travel. Contact manifolds, warm-start impulses and the
recycling cache (03B "restored/invalidated") are **not** sent.

**Solver-cache invalidation on a correction.** `apply_authoritative` builds a
`WorldCapture` with no contact pairs and calls `restore_world`, so every
contact pair in the receiver is cold-started (impulses zeroed, recycling
cache dropped) and counted in `ApplyReport::world.pairs_invalidated`
(`pairs_restored` is always 0). Islands, broad phase and AABBs are rebuilt by
Avian through change detection from the written poses, as in 03B. A prop
captured asleep is put to sleep, deferred one step if a stale contact still
links it to an awake island (03B `PendingSleeps`).

**Validation, before the first write.** Counts, unique slots, every
vehicle's generation equal to the snapshot's (`MixedGeneration`), finite
floats (`NonFinite`), and a reset epoch not older than the peer's own for
that slot (`StaleResetEpoch`: a late packet cannot resurrect a pre-reset
car). A vehicle whose wheel count or model disagrees is refused by
`restore_vehicle` and reported in `ApplyReport::refused`. A snapshot from another
session generation than the one passed to `apply_authoritative` is refused
(`StaleGeneration`).

### Measured convergence

Receiver: a second world with another spawn order (so no `Entity` matches),
37 despawned and 11 live junk entities ahead of the scene, and its own
25-tick-longer history under different commands, so its cars, tyres, prop
and contact graph disagree with the host's at T (asserted: > 1.0 from the
host before the apply, 0 after). Both worlds then run the host's commands
for 60 ticks and are compared every tick on the full capture.

| T | Situation | Worst float, any field | Worst position / velocity |
|---|---|---|---|
| 120 | cars driving, prop asleep, no contact beyond the ground | 0 (bit-identical) | 0 / 0 |
| 236 | car A at the wall, rig reaching the prop | 64 N (wheel force scalar, tick 262) | 4.9e-4 m / 8.3e-3 m/s |
| 300 | car A pinned, rig at the prop | 8.1 N | 5.1e-3 m / 9.6e-3 m/s |

Accepted bounds in `tests/wire.rs`: with no contact in play 1e-3 per float
(`TOLERANCE`, as 03A/03B); with cold-started wall or prop contacts 1e-2 m,
5e-2 m/s, 130 N. The T=236 row measured 2.97e-2 m/s (83 N) on the Linux CI
runner against 8.3e-3 m/s on macOS: a cold-started contact onset is
solver-order noise and differs across platforms. The contact rows are the same floor 03B measured for a
contact onset in a world whose solver ordering differs: bounded, not exact.
Exactness is claimed only when no contact pair other than the ground exists.
Without the apply, the worlds stay > 1.0 apart (`applying_nothing_leaves_the_worlds_apart`).

Not covered: other platforms, a real transport (loss, ordering, delta
snapshots: NETPLAY-02/04), bodies other than the trailer joint, and the cost
of `capture_authoritative` (NETPLAY-03F). Applying a correction on a peer
while it predicts ahead (rewind to T, replay local inputs) composes this
with 03C's `replay_from`; that wiring is NETPLAY-04.

## Historical lifetimes of collision dependencies (NETPLAY-03E, #1782)

`mm2_vehicle::lifetime`. A `WorldCapture` names `Entity` ids that must
mean the same actor when it is restored up to `MAX_REPLAY_STEPS` (120)
ticks later. Rather than keeping despawned entities alive for the length
of the history, the lifetime is made detectable: a capture records what an
id meant, and a restore never writes through an id that now means something
else. `check_lifetimes(world, &capture)` lists the `LifetimeIssue`s;
`restore_world` reports them (`RestoreReport::issues`, `retired`) and
`replay_from` fails with `ReplayError::StaleHistory` *before writing*, so
the caller adopts the authority's capture (`recover_missing_history`).

| Case | Mechanism | Result |
|---|---|---|
| Traffic recycle/despawn, slot reused | Bevy entity generation: the stale id never resolves to the new occupant | `missing`, nothing written to the new car; replay proceeds |
| Entity repurposed in place (broken or changed banger, fragment reuse, recycled traffic car, a body swapped under a moving collider) | `ActorIncarnation` component, bumped by whoever repurposes (`banger::shatter_placement` does) and captured per body, vehicle and contact pair | `Retired`: not written; pairs with that body cold-start; replay refuses |
| Trailer attach/detach | the `SphericalJoint` set `(joint, body1, body2)` is captured | `HitchChanged`; replay refuses |
| New session generation | `SessionEpoch` resource vs `EpochRef::generation` | restore writes nothing; replay refuses; `restore_vehicle` returns `StaleEpoch` |
| Vehicle reset | `ResetEpoch` component vs `EpochRef::reset_epoch` (a capture *older* than the live epoch is stale; a wire snapshot ahead of the peer is not) | `StaleEpoch` refusal; replay refuses; an authoritative capture stamped with the new epoch is adopted |

Bounds: a `u32` incarnation needs 2^32 bumps of one entity inside the
121-tick history to alias; only `SphericalJoint` hitches are tracked; the
checks are skipped when `SessionEpoch` / `ResetEpoch` are absent. Wiring
`SessionEpoch`/`ResetEpoch` and the remaining repurposing sites (fragment
pool, traffic recycle-in-place, if any exist) into the app is NETPLAY-04's
correction path. The city is loaded as one unit, so there is no runtime
chunk streaming to account for. Tests: `crates/mm2_vehicle/tests/lifetime.rs`.

## State-copy cost in a loaded world (NETPLAY-03F, #1783)

Harness: `crates/mm2_vehicle/tests/profile.rs`. Run
`cargo test -p mm2_vehicle --test profile -- --ignored --nocapture` (dev
profile: workspace crates at opt-level 1, dependencies including Avian at 3;
Apple Silicon macOS, one run, machine load not recorded). The always-on test in
the same file runs the scene small and asserts the ring holds exactly
`MAX_HISTORY` captures and a 60-tick replay from it completes unrefused.
A counting global allocator measures the capture's real heap bytes.

**Evidence level: synthetic integration.** The world is not the retail city:
it has 6000 static building colliders (a city-sized broad phase), 8 cars
(two models), 4 jointed trailers (12 vehicles captured), 200 dynamic props
(172 asleep at the measured instant, the rest woken by the cars), a wall,
and 228 touching contact pairs. Cars drive down a street at 0.7-1.0
throttle with small steering. A retail-city run (real collider counts and
mesh complexity, bot/traffic/prop systems) was **not** done: `mm2_vehicle`
cannot load a city and the replay entry point is not wired into `mm2_app`
until NETPLAY-04. It remains open as the original-content level.

| Operation | mean | p99 | max |
|---|---|---|---|
| Ordinary tick (`app.update()`, physics only) | 0.54 ms | 0.70 ms | 0.72 ms |
| `capture_world` (every tick, 12 vehicles + 200 props + 228 pairs) | 0.056 ms | 0.079 ms | 0.097 ms |
| `capture_authoritative` (wire snapshot, host) | 0.036 ms | 0.048 ms | 0.077 ms |
| Restore + 1 replayed tick | 0.84-0.87 ms | 0.98 ms | 0.98 ms |
| Replay 6 ticks (100 ms) | 3.6 ms | | 3.6 ms |
| Replay 20 ticks | 11.1 ms | | 11.1 ms |
| Replay 60 ticks (1 s) | 31.0 ms | | 31.1 ms |
| Replay 120 ticks (`MAX_REPLAY_STEPS`, 2 s) | 56.8 ms | | 57.6 ms |

Replay costs about 0.47-0.60 ms per tick (the live tick cost) plus a
~0.3 ms restore, and is linear in depth. At 60 Hz the frame budget is
16.7 ms: a correction of up to ~25 ticks fits in one frame's budget on this
machine; the 2 s worst case (57 ms) is three-plus frames and should be spread
or capped by NETPLAY-04, not run unconditionally.

**Load sensitivity.** The table was measured before
rebasing onto 03E (machine load not recorded, so not claimed idle). Re-run after the rebase on the same machine at load average
~13 (other agents building): ordinary tick 1.4 ms, `capture_world` 0.126 ms,
restore+1 tick 1.8 ms, replay 20 ticks 22 ms, 120 ticks 122-126 ms. Every
figure scaled by about 2.5x together, so the ratios hold: a capture costs
~9-10% of a tick and replay is ~tick-cost per step. Treat the absolute
milliseconds as machine-dependent and the ratios as the finding.

**Memory.** One capture is 156,264 B (144,584 B before NETPLAY-03E added its lifetime records) in this world (heap bytes, measured),
so a full ring of 121 captures (2 s, the stress depth) is 18.0 MiB and a
normal depth of 20 ticks is 3.0 MiB. Eight human cars with trailers is the
state-copy worst case the task names; the capture grows with props and
touching pairs, not with the number of static colliders (none are copied).

**Decision.** Copying every dynamic body and touching contact pair each tick
costs 0.056 ms, about a tenth of the physics tick, and is viable as it
stands: no change-aware or shared history is needed on this evidence, so the
whole-world capture per tick is *with* a demonstrated budget. No blocking
children were filed. Not measured: the retail city (above), more than 200
props or a crowd of bodies in contact, other platforms and release builds,
and the cost of the client's own non-physics replay-side work (NETPLAY-04).
The "normal and stress history depths" are taken as 20 and 120 ticks;
NETPLAY-09 owns the final choice and can re-run this harness at its values.

## NETPLAY-03 integration status (#1764)

All six slices (03A-03F) are on main. This section maps the parent
acceptance criteria to the evidence that exists and names what stays open.
Evidence level reached: **synthetic integration** (production `VehiclePlugin`
and Avian schedules at 60 Hz in `mm2_vehicle`); no original-content or
rendered level.

| AC | Evidence | Status |
|---|---|---|
| 1 capture, advance, restore, replay, compare | `tests/capture.rs` (generic and original, steer/brake/forced gear/handbrake route, deviation 0 on this machine, tolerance `1e-3`); `tests/replay.rs`, `tests/physics_world.rs` (two cars, trailer, wall, sleeping prop, several rollback depths) | met |
| 2 original handling, contact, trailer, multi-tick comparison | original path in `capture.rs`; wall contact onset and trailer rig in `replay.rs`/`physics_world.rs`, compared at every replayed tick | met except jump/landing, see below |
| 3 reset versus pose-only snap | `capture.rs::a_host_reset_and_a_pose_only_snap_are_not_the_same_state` | met |
| 4 no second render frame or duplicate side effects, bounded | `replay.rs::a_replay_runs_the_physics_schedule_and_nothing_else`, `replay_work_is_bounded`, `the_history_ring_is_bounded_and_rewrites_the_future`; memory in the cost section | met for the primitive; gameplay-reaction dedup is NETPLAY-08 |
| 5 determinism and limits | bit-identical on one Apple Silicon machine; the wire convergence bound needed a looser contact-onset velocity on the Linux CI runner (`dd34fc6`). No cross-platform bit identity is claimed | measured on two platforms, limits stated |

Open, not claimed:

- A replay across an **airborne jump and landing** is not in the replay or
  world-restore suites (`drive.rs` covers a landing live, not a rewind
  through one).
- The loaded-city profile uses a synthetic world, not the retail city.
- Release builds and non-Apple/Linux platforms are unmeasured.
- Wiring the ring and replay into the client correction path is NETPLAY-04.

## Own-car prediction history and replay (NETPLAY-04, #1765)

`mm2_app::netpredict`, behind the one `NetplayTimeline` selector
(`--netplay-timeline` / `MM2_NETPLAY_TIMELINE=1`). With the selector off the
legacy `DriftWatch`/`SettleWatch` path is unchanged and still the default;
NETPLAY-09 enables the new path and then deletes them.

- **Alignment is the ack, not a tick.** `record_prediction` (`FixedLast`)
  stores `capture_world` after every command under that command's `seq`.
  A snapshot's own-seat `input_ack` names the last command inside the host
  state, so host `S[server_tick]` and the client state recorded for
  `seq == input_ack` are the same instant. No present-versus-past compare.
- **Prediction consumes the quantized command.** `netcmd::capture_and_send`
  writes `decode_tick_command(command)` back into `VehicleInput`, so
  prediction, host and replay run on identical numbers.
- **Correction.** `reconcile_own` (`FixedFirst`, exclusive) takes the newest
  staged assertion (`RemoteSnaps` is latest-wins, so a burst is coalesced to
  one comparison and one replay), compares position/orientation/linear and
  angular velocity (tolerances in `netpredict`, designed values), and on a
  difference restores the recorded world with the host body state put in,
  replays `ack+1..=newest` one fixed step at a time through
  `mm2_vehicle::replay::replay_from`, and rewrites the recorded history with
  the re-simulated states. Acked commands are trimmed after use and never
  replayed twice. Remote replicas are put back where they were (they are
  driven by the snapshot lerp, contact between predicted cars is NETPLAY-05).
- **Not routine "resets".** Each non-routine case has its own counter:
  `Skip::{NoAck,Seen,Future,PreReset,Corrupt,OtherGeneration}` leave the car
  alone; `RecoveryCause::{HistoryExhausted,TooDeep,CommandGap,ReplayRefused}`
  adopt the host body state (marked `Teleported`) and restart the history;
  an authority reset (new reset epoch) clears the history and sets a floor
  so the pre-reset commands are never compared or replayed.
- **Headless.** `smoke.rs` now carries the selector in `RunBudget`, registers
  `netcmd`/`netpace`, and wall-paces a selected headless run: the timeline
  aligns on a real 60 Hz tick, so a fast-forwarding host is outside its
  contract. This means `baseline_unequal_frame_pacing` (host without
  `--paced`) is wall-paced under the selector and no longer measures a
  fast-forwarding host; a genuine render-rate mismatch scenario is open.
- `net_baseline` now passes `--netplay-timeline` to all three processes, and
  the `Driver::Route` exemption from the dev-world speed bar is removed.

Evidence (synthetic integration, `cargo test -p mm2_app --lib netpredict`):
agreeing snapshot is a no-op; a divergent state at ack 8 replayed through 22
pending turn/brake commands equals an independent twin world run live from
the same host state (1e-3 m) and the rewritten history then matches; duplicate
and older acks skip; NaN/future/zero/other-generation skip; reset floor;
exhausted history and a missing command take named recoveries.

Evidence (original-process level, headless, loopback, `net_baseline`
`unequal-pacing`, 40 s, two clients, dev-world): alice 0 corrections > 0.25 m,
cumulative 1.29 m/min, 0 yaw flips, peak 30 m/s; bob 0, 0.46 m/min, 0, 33 m/s
(before: 18/min over 0.25 m, 1332 and 1228 m/min, snaps of 50-100 m).
The clients logged ~600 small replays each in 40 s; the correction *count*
is therefore high but the size is tiny (total translation above). The cause
of the steady sub-metre replays (the wire carries only the body, so wheel
and drivetrain continuation comes from the client's own recorded state) is
not yet isolated.

Open, not claimed: 0/50/100/150 ms RTT runs with mismatched frame pacing
(AC4) — no impairment proxy in the process harness; the correction-work
budget spread over frames (a deep replay runs in one frame, bounded by
`MAX_REPLAY_STEPS`); presentation smoothing of the replay shift (NETPLAY-07);
contact (NETPLAY-05); gameplay-reaction dedup (NETPLAY-08); wire carry of
the full vehicle continuation state; retail-city cost.

## Snapshot fragmentation and ACKed baselines (NETPLAY-06C, #1869)

Code: `mm2_net::frag` (pure, clock-injected) wired into the datagram lane in
`rt.rs`/`lobby.rs`. Evidence level reached: **code checks and synthetic
integration** (unit tests, loopback lane tests, a seeded loss simulation).
No WAN, retail-data or rendered level.

### What exists

- **Fragments** (`LaneTag::Fragment`): `tick u64 | group u8 | groups u8 |
  index u8 | count u8` (12 B) then data, so a fragment carries
  `MAX_PAYLOAD - 12 = 1156` data bytes inside the 1200 B datagram budget
  (32 B lane header included). Bounds: 8 groups per tick, 8 fragments per
  group, 4 incomplete ticks in flight, 8 ticks of lag.
- **Groups are the dependency boundary.** The transport never cuts inside a
  group; `HostCtl::send_snap_groups(player, generation, tick, &[Snap..])`
  lets the producer put the owner's car and its contacts in one group and an
  unrelated part of the world in another. The client delivers each group as
  a `Snap` the moment *its* fragments are whole, so a lost world fragment
  cannot withhold the owner correction (`a_lost_fragment_withholds_only_its_own_group`,
  `snapshot_groups_arrive_independently_and_the_client_acks_a_baseline`).
  `broadcast`/`send_to` of an over-budget `Snap` now fragments it as one
  group instead of falling back to TCP (`oversize_tcp` stays 0 for `Snap`).
- **Expiry never blocks newer ticks.** An assembly older than 250 ms
  (`FRAGMENT_TTL`) is dropped; a newer tick evicts the oldest incomplete
  one; stale, duplicate, malformed and inconsistent fragments are refused
  and counted. `Client::lane_assembly()` reports `expired`, `evicted`,
  refusals and the longest completed assembly.
- **ACKed baselines** (`LaneTag::Ack`, `epoch u8 | tick u64`, generation in
  the lane header): `BaselineSender::encode` sends a delta only against a
  baseline that is acknowledged, still held, in the same (generation, epoch)
  scope and at most `REFRESH_TICKS` (60, one second) behind; otherwise a full
  state, which is also the refresh on a missing baseline.
  `BaselineReceiver::apply` rejects a delta whose base it does not hold
  (never received, evicted, other epoch/generation) with `MissingBase` and
  stores nothing; the caller acks only after a successful apply. Eight
  baselines are held per side and the acknowledged one outlives newer
  unacknowledged ones. `Client::lane_ack` sends the ack; `Host::lane_ack`
  reports the newest per seat (an ack for another generation is refused at
  the door as `wrong_generation`).
- **Lossless deltas.** `delta_encode` is a byte-run diff, so a continuation
  field (velocity, accumulator, `input_ack`) is reproduced exactly; there is
  no field list that could omit one. The simulation asserts byte equality of
  every reconstructed state.

### Measured (`cargo test -p mm2_net eight_seat -- --nocapture`)

60 Hz, 3600 ticks, seeded; eight seats as group 0 = the owner's car (one
`Snap` entry) and group 1 = the other seven. Loss hits every fragment and
every ack independently; an ack returns after 4 ticks. Sizes are the real
`Message::Snap` encoding.

- Serialized continuation size: full owner group **138 B** (1 fragment);
  delta payload **~72 B/tick** on average (about 52 %; only pose, velocity,
  rotation, rpm and counters move). Full world group of seven cars **840 B**
  (1 fragment), ~394 B/tick as deltas. With a 64-impact collision burst the
  world group is **4296 B = 4 fragments**, ~2.36 kB/tick as deltas (impacts
  are fresh events each tick, so they do not delta; the simulation keeps the
  group length constant, which a real burst does not).
- Fragments per correction: owner **1**; whole tick **2** quiet, **3.6**
  with the burst.

Complete usable corrections per second (owner group / world group / whole
tick), quiet world:

| loss | owner Hz | world Hz | whole-tick Hz |
|---|---|---|---|
| 0 % | 60.0 | 60.0 | 60.0 |
| 1 % | 59.3 | 59.3 | 58.6 |
| 5 % | 57.0 | 56.8 | 53.9 |
| 10 % | 54.1 | 53.7 | 48.3 |
| 20 % | 48.1 | 48.0 | 38.5 |

With the 64-impact burst (world group 4 fragments): owner 60.0 / 59.5 /
57.1 / 54.0 / 48.0 Hz, world 60.0 / 58.1 / 52.5 / 45.9 / 34.0 Hz, whole tick
60.0 / 57.6 / 49.9 / 41.2 / 27.1 Hz for 0 / 1 / 5 / 10 / 20 % loss. The
owner correction tracks `1 - loss` independently of the world's size; the
whole tick decays with fragment count. Baseline, fragment and ack loss are
all in the model; a delta was refused for a missing base once in the 20 %
quiet run (never applied wrongly), because the sender only deltas against
acknowledged baselines.

### Not done / open

- The app does not yet produce grouped or delta snapshots: `mm2_app`
  still broadcasts one `Snap`, which now fragments as a single group. Choosing
  the owner/contact grouping and engaging `BaselineSender` per seat belongs
  with the send-cadence and prioritisation work (NETPLAY-06D) and the app
  correction path (NETPLAY-04).
- The simulation has no jitter or reordering beyond the fixed ack RTT, and
  loopback tests prove the code, not WAN behaviour.

## Remote controls on the wire (NETPLAY-05A-1, #1892)

Per-seat `input_ack` in `Snap` says how far the host has *finalized* a
seat's command stream; it is not a coherent record of what the host
*applied* to each car on each tick. To predict another human's car, a
client needs those applied commands at their authoritative ticks. Protocol
v34 adds `Message::RemoteCommands` for that (`mm2_net::proto`); nothing
reads it yet — no body behaviour changes in this slice.

**Which commands.** The commands the host actually applied to each seat's
car (`SeatBuffer::consume`'s result), not raw arrivals: a gap or starved
tick carries the substitute the host used, tagged with its `Source`
(`Command` / `HeldLast` / `Neutral`), so a predictor can tell the
driver's input from a host fallback. The receiving client's own seat is
not sent.

**Canonical tick.** The frame has `tick` (host session tick at capture).
Each per-seat run has `first_tick`; command `i` was applied on host tick
`first_tick + i`. A run may not end after `tick` and may not overflow
(rejected on encode and decode), runs hold `1..=MAX_REMOTE_RUN` (16) ticks,
a frame at most `MAX_REMOTE_SEATS` (8) runs, one per player. Each run
carries the seat's reset `epoch`.

**Age.** `age = frame.tick - command_tick` at the frame's capture, and
`RemoteCommands::age(player, now)` for the newest stored command at the
client's current notion of host tick. The wire carries ticks, not
wall-clock ages: age is derived, so it cannot disagree with the tick.

**Loss.** No ack. Every frame repeats each seat's last 16 applied ticks
(about 267 ms), so a tick is lost only if every frame carrying it is. The
frame rides the datagram lane when it fits one datagram (it is never fragmented) and the
control stream otherwise. `lane_impair_tests::remote_commands_survive_loss_reorder_and_duplication_without_an_ack`
runs 90 ticks through the seeded UDP proxy at 30% loss, 25% duplication
and reorder with 10–50 ms delay: every tick arrives intact and is consumed
once, in order.

**Client store** (`mm2_net::netcmd::RemoteCommands`, bounded to 64 ticks
per seat): keyed by seat and tick. The first copy of a tick is kept and
never altered — an identical repeat is a counted duplicate, a differing
one a counted conflict and dropped. `consume(player, tick)` hands a tick
out once and advances a cursor, so a consumed (or older) tick can never be
consumed or re-stored again. A newer reset epoch (wrapping) clears the seat;
an older one is dropped. Frames for another generation are dropped.

**Not decided here** (05A-2): what a predictor does when a tick is missing
(hold-last versus an age-aware policy), and where the host takes the
frame's runs from each tick. Evidence level: synthetic integration; no
retail data or rendered evidence is involved.
