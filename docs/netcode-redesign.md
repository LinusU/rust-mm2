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
  stores nothing; the caller acks only after a successful apply. Sixty-four
  baselines are held per side (at least `REFRESH_TICKS`; eight made every
  delta `MissingBase` once the ack round trip passed about eight snapshots,
  NETPLAY-10D-2) and the acknowledged one outlives newer unacknowledged ones. `Client::lane_ack` sends the ack; `Host::lane_ack`
  reports the newest per seat (an ack for another generation is refused at
  the door as `wrong_generation`).
- **A resting body is refreshed, not omitted (#2118).** The host sends every
  group of every `Playing` snapshot (a seat is thinned only by its congestion
  divisor, at most 3), and the tick inside a group's bytes moves even when no
  body does, so a state is never withheld as unchanged. A lost last change
  therefore costs the receiver the next delivered frame: a delta against the
  acknowledged baseline when that is held, a full state otherwise, and a
  full state at the latest `REFRESH_TICKS` (60) after the last ack. Human
  seats always ride group 0 (`ContactIslands::groups_for`), which `Bulk`
  omission never drops. The bound is pinned over every loss pattern of six
  data frames crossed with six acks by
  `a_body_that_rests_as_its_last_change_is_lost_reaches_the_receiver`. The smoke
  record prints `marks=<seat>:<tick>` (the newest authority tick applied or
  staged per seat) and the `net_contact` fork report echoes it, so a copy
  that sits still while its mark is old is a receive gap and one whose mark is
  current is an apply or presentation gap.
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

### MTU and fragment failure (NETPLAY-10D-1)

Measured with `cargo test -p mm2_net lane_mtu -- --nocapture` (real
`Message::Snap` encodings, the UDP impairment proxy's `largest` counter):

- **Eight-seat correction, quiet:** 957 B encoded, one datagram of 989 B
  (32 B lane header). **With the 64-impact burst:** 4605 B, four fragments;
  the proxy saw no datagram over `MAX_DATAGRAM` (1200 B) and a path limited
  to exactly 1200 B delivers both (`mtu_dropped == 0`). Overhead per
  fragment is 44 B (32 lane + 12 fragment header).
- **Fragmentation amplification:** wire bytes / payload is 1.03 (quiet) and
  1.04 (burst), so bandwidth is not the cost; loss is. A tick of `n`
  fragments survives with `(1 - p)^n` (burst: four fragments, 0.81 at 5 %
  loss), which is the whole-tick column in the cadence table above. At 60 Hz
  the burst offers 240 datagrams/s (about 287 kB/s) against 60 (59 kB/s)
  quiet.
- **Selective oversize drops** (`selective_oversize_drops_cost_only_the_snapshots_that_need_fragments`):
  with the downlink limited to 600 B, a snapshot that fits still arrives; a
  fragmented one delivers nothing (never half-assembled), any short tail
  fragment that crosses is expired by `FRAGMENT_TTL`, and when the limit is
  lifted the same snapshot arrives whole. At the assembler
  (`a_size_threshold_that_eats_full_fragments_starves_assembly_boundedly`):
  200 ticks with every full-size fragment eaten complete only the
  one-datagram group, hold at most `MAX_ASSEMBLIES` ticks, and expire or evict
  the rest.
- **The join already proves the path** (probes and echoes are padded to
  `MAX_DATAGRAM`, `LaneTimeout` names the MTU). A path that degrades
  *after* the join used to lose fragments silently. The established client
  now sends a full-size probe every `PATH_PROBE_INTERVAL` (1 s) whatever
  else it sends, the host echoes it at full size, and
  `Client::lane_path_failure()` returns `NetError::LanePathFailed` once no
  echo has arrived for `PATH_FAILURE_AFTER` (5 s, below the host's 8 s
  liveness reap), clearing again on the next echo
  (`a_path_whose_mtu_collapses_after_the_join_is_a_surfaced_failure_that_heals`,
  both directions). The failure is surfaced, not repaired: no refresh can
  help when no fragment-sized datagram crosses. Acting on it (leaving the
  session with the message, or falling back to TCP-only play) belongs to
  `mm2_app`, which does not poll it yet.
- Usable cadence when it does work: a one-datagram correction (the owner
  group) tracks `1 - loss`; refresh after missing baselines is the existing
  `REFRESH_TICKS` (one second) full state.

### Redundant input history under clustered loss (NETPLAY-10D-3)

Measured with `cargo test -p mm2_net clustered_bursts -- --nocapture` (the
seeded lockstep lane proxy; `drop_next` bursts of 3-6 datagrams at seeded
positions, 400 ticks, a window every second tick, 11-12 bursts per run):

| window | bursts | windows dropped of 200 | commands lost of 400 |
|---|---|---|---|
| 8 (production, 4 transmissions per command) | 11 | 47 | 28 |
| 8 | 12 | 58 | 44 |
| 16 (`MAX_INPUT_WINDOW`, 8 transmissions) | 11 | 46 | 0 |

- **A command is lost iff every distinct datagram that carried it was
  dropped.** The test models that per command and asserts the host's
  `received` and `gap_finalized` counters equal the model exactly, so
  redundancy spans separate packets and time, not window length.
- **Finalization.** Every sequence `1..=400` finalizes exactly once and in
  order; a recovered command is applied as the driver sent it
  (`Source::Command`), a lost one expires into a substitute and is never
  labelled a driver command. The ack walks over lost sequences.
- **Not presentation state.** Input windows are not latest-wins: the host
  counts `windows_received` equal to windows sent minus windows dropped, with
  no overflow or rate limiting, whereas only snapshots coalesce.
- **Quantization.** The pose in a correction is full `f32`; what is
  quantized is the input (`u8` pedals, `i8` steer, `-127..=127`) and the
  presentation tail, which has no authority over replay. Prediction, host and
  replay all run on the decoded wire command, and
  `netcmd::tests::quantizing_a_command_is_a_fixed_point_with_half_a_step_of_error`
  proves every wire byte re-encodes to itself and a driver value is off by at
  most half a step (1/510 pedals, 1/254 steering). The end-to-end
  applied-equals-quantized check stays in `net_matrix`.
- **Evidence level:** loopback with a seeded proxy; says nothing about a real
  path. The 4+ burst cells at window 8 lose commands by design: only the
  16-command window covers a 6-burst at this cadence.

### Not done / open

- (Done in NETPLAY-10D-2, see "Baselined snapshot states per seat" below:
  the app sends each `Playing` snapshot group, split by contact island
  (NETPLAY-10C-2), as a baselined state and the client engages
  `BaselineReceiver`.)
- The simulation has no jitter or reordering beyond the fixed ack RTT, and
  loopback tests prove the code, not WAN behaviour.

## Baselined snapshot states per seat (NETPLAY-10D-2, #2028)

Evidence level: **code checks and synthetic integration** (unit tests and
loopback lane tests through the seeded `UdpImpairProxy`). No WAN, retail or
rendered level.

### What the app sends

`mm2_app::netdrive::publish_snapshots` builds the one frame as before and,
in the `Playing` phase only, sends it per seat:

- **`Playing` frames** take the groups of "Snapshot groups per seat"
  (`ContactIslands::groups_for`: group 0 owner and human islands, group 1
  `Bulk` bot-only islands, empty groups omitted) and send each as a
  baselined state with the group's index as its logical group id, so the
  two groups keep independent baselines. The priorities are the existing
  ones (`Movement` / `Bulk`).
- **Seats without a lane** keep the whole frame over the control stream.
  Every non-`Playing` frame (`Ready`, `Countdown`, `Results` and its
  repeats) keeps the plain `send_snap_groups_prio` path: those frames repeat
  a tick or carry rows every seat must see, which a baselined
  (stale-by-tick) state would not allow.

`HostCtl::send_snap_states_prio(seat, generation, tick, epoch, &[(group,
Snap)], priorities)` encodes each group with that seat's per-group
`BaselineSender` and sends the result as a **state frame**
(`0xFE | group | epoch | state payload`, `frag::encode_state_frame`) in a
fragment group. The senders live in the host's `LaneSeat`, so a client that
rejoins under the same id starts from full states.

### Receiving and acknowledging

The client lane reassembles the group, sees the state frame, applies it with
its per-group `BaselineReceiver`, hands the reconstructed `Snap` to the game
exactly as an unbaselined one and **then** acknowledges it. The ack
(`LaneTag::Ack`) is now `epoch u8 | group u8 | tick u64`; the host keeps the
newest per group (`Host::lane_group_ack`) and feeds it to that group's
sender. Nothing in `mm2_app` changed on the receiving side.

### Failure handling

| Fault | Behaviour |
|---|---|
| baseline lost on the way down | never acked, so never a delta base; the next tick is a full state (`a_lost_baseline_is_never_deltaed_against`) |
| ack lost | the sender keeps the older acked baseline; a delta against it applies because the client holds it; the next ack supersedes (`a_lost_ack_costs_a_full_state_and_the_next_ack_recovers`) |
| acked baseline too old | more than `REFRESH_TICKS` (60) behind: full state (`a_baseline_older_than_the_refresh_bound_is_replaced_by_a_full_state`) |
| baseline evicted at the receiver | `MissingBase`: nothing stored, nothing acked, counted (`StateStats::missing_base`); the sender's refresh recovers (unit tests in `frag`) |
| reset / roster change | `PublishPace::baseline_epoch` moves when the seat set or any seat's wire reset epoch changes; the next state is full for every seat and an old-epoch ack names nothing (`an_epoch_bump_forces_a_full_state_and_retires_old_acks`) |
| generation change | the sender's scope changes: full state; the receiver drops its baselines |
| group not sent a tick | each group has its own baselines (`groups_keep_independent_baselines`) |
| refused send (oversize group) | counted (`snap_states_refused`); the next tick is a fresh full state |

Seeded 20 % loss / 10 % duplication / 10 % reordering, 300 ticks, two
groups (`a_seeded_lossy_run_reconstructs_every_applied_group_exactly`):
every group the client applied equals what was sent for that tick and group;
the host sent 586 deltas against 14 full states and 53.6 kB of state payload
against 293.4 kB as full states (18 %), with no `MissingBase`. The protocol
is **v40** (state frames and the ack's group byte).

### Congestion control takes the round trip from these acks

The 10B `PeerController` takes its round-trip evidence from the group acks
(`SeatOut::on_group_ack`): one sample per tick (the first group acked; the
tick leaves the log), measured from when the tick was queued. Three
decisions, each from a failed `net_contact` run of the first wiring
(`a_sustained_push_is_measured_on_unequal_links`: own-car correction 18-47 m,
first motion 272 ticks late):

- **No passed-over count.** A tick carries only the groups that changed, so
  an unacknowledged older tick is not loss; older ticks are dropped from the
  log without `skipped`.
- **No ack age.** A quiet scene sends no group, so no ack, and the
  controller would read that silence as congestion. `ack_at` stays unset.
- **Tick 0 is no sample.** It is acknowledged in about 2 ms on a 90 ms link
  (cause not established), which as the lowest-RTT baseline made every real
  sample look like a standing queue. And the standing-queue test allows
  four mean deviations of the samples (`rttvar`) above the 50 ms slack, since
  quarter-delay jitter spreads samples above the minimum without any queue.

### Not done / open

- (Done in NETPLAY-10D-2, see "Baselined snapshot states per seat" below:
  the app sends each `Playing` snapshot group, split by contact island
  (NETPLAY-10C-2), as a baselined state and the client engages
  `BaselineReceiver`.)
- The simulation has no jitter or reordering beyond the fixed ack RTT, and
  loopback tests prove the code, not WAN behaviour.

## Baselined snapshot states per seat (NETPLAY-10D-2, #2028)

Evidence level: **code checks and synthetic integration** (unit tests and
loopback lane tests through the seeded `UdpImpairProxy`). No WAN, retail or
rendered level.

### What the app sends

`mm2_app::netdrive::publish_snapshots` builds the one frame as before and,
in the `Playing` phase only, sends it per seat:

- **`Playing` frames** take the groups of "Snapshot groups per seat"
  (`ContactIslands::groups_for`: group 0 owner and human islands, group 1
  `Bulk` bot-only islands, empty groups omitted) and send each as a
  baselined state with the group's index as its logical group id, so the
  two groups keep independent baselines. The priorities are the existing
  ones (`Movement` / `Bulk`).
- **Seats without a lane** keep the whole frame over the control stream.
  Every non-`Playing` frame (`Ready`, `Countdown`, `Results` and its
  repeats) keeps the plain `send_snap_groups_prio` path: those frames repeat
  a tick or carry rows every seat must see, which a baselined
  (stale-by-tick) state would not allow.

`HostCtl::send_snap_states_prio(seat, generation, tick, epoch, &[(group,
Snap)], priorities)` encodes each group with that seat's per-group
`BaselineSender` and sends the result as a **state frame**
(`0xFE | group | epoch | state payload`, `frag::encode_state_frame`) in a
fragment group. The senders live in the host's `LaneSeat`, so a client that
rejoins under the same id starts from full states.

### Receiving and acknowledging

The client lane reassembles the group, sees the state frame, applies it with
its per-group `BaselineReceiver`, hands the reconstructed `Snap` to the game
exactly as an unbaselined one and **then** acknowledges it. The ack
(`LaneTag::Ack`) is now `epoch u8 | group u8 | tick u64`; the host keeps the
newest per group (`Host::lane_group_ack`) and feeds it to that group's
sender. Nothing in `mm2_app` changed on the receiving side.

### Failure handling

| Fault | Behaviour |
|---|---|
| baseline lost on the way down | never acked, so never a delta base; the next tick is a full state (`a_lost_baseline_is_never_deltaed_against`) |
| ack lost | the sender keeps the older acked baseline; a delta against it applies because the client holds it; the next ack supersedes (`a_lost_ack_costs_a_full_state_and_the_next_ack_recovers`) |
| acked baseline too old | more than `REFRESH_TICKS` (60) behind: full state (`a_baseline_older_than_the_refresh_bound_is_replaced_by_a_full_state`) |
| baseline evicted at the receiver | `MissingBase`: nothing stored, nothing acked, counted (`StateStats::missing_base`); the sender's refresh recovers (unit tests in `frag`) |
| reset / roster change | `PublishPace::baseline_epoch` moves when the seat set or any seat's wire reset epoch changes; the next state is full for every seat and an old-epoch ack names nothing (`an_epoch_bump_forces_a_full_state_and_retires_old_acks`) |
| generation change | the sender's scope changes: full state; the receiver drops its baselines |
| group not sent a tick | each group has its own baselines (`groups_keep_independent_baselines`) |
| refused send (oversize group) | counted (`snap_states_refused`); the next tick is a fresh full state |

Seeded 20 % loss / 10 % duplication / 10 % reordering, 300 ticks, two
groups (`a_seeded_lossy_run_reconstructs_every_applied_group_exactly`):
every group the client applied equals what was sent for that tick and group;
the host sent 586 deltas against 14 full states and 53.6 kB of state payload
against 293.4 kB as full states (18 %), with no `MissingBase`. The protocol
is **v40** (state frames and the ack's group byte).

### Congestion control is deliberately not fed by these acks

The 10B `PeerController` was built to take RTT, passed-over-snapshot and
ack-age evidence from snapshot acknowledgements, and until now none arrived.
Wiring the new acks into it (`SeatOut::on_ack`, `ack_at`) was tried and made
the `net_contact` unequal-link cases fail (`a_sustained_push_is_measured_on_unequal_links`:
own-car correction 18-47 m, first motion 272 ticks late): on a jittery link
the smoothed-RTT and passed-over evidence cut the peer's rate and thinned
the corrections the contact deadlines depend on. So the baseline ack updates
the baseline sender and the per-group `LaneAck` only. Feeding the controller
from snapshot acks needs its own change with its own measurement against
those cases.

### Not done / open

- The island criterion is not tuned against play (#2050); a group whose
  membership changes is a larger delta, never a wrong one. The group id is
  the group's index, which is stable because there are at most two.
- No WAN, rendered or retail-city measurement of the saving.

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

## Remote-input prediction policy (NETPLAY-05A-2, #1893)

`mm2_app::netremote::RemotePredictor` fills a remote seat's tick when the
store (`RemoteCommands`) has no command for it. Pure: it reads the store and
returns a `TickCommand`; no body is touched. A stored tick is returned
unaltered through `consume` (once); a missing tick consumes nothing and is
extrapolated from the newest stored command at or before it. Only analog
controls are repeated or scaled *down* toward neutral; `forced_gear` and
`auto_reverse` are copied, so no reset, horn, gear change or handbrake is
invented. A command that arrives after its tick was predicted past is
dropped by the store cursor, never applied late.

Two policies, measured by `tests/net_remote_predict.rs` (`cargo test -p
mm2_app --test network net_remote_predict -- --nocapture`): **hold-last**
repeats the newest command at any age; **age-aware** repeats for `hold`
ticks of age, then fades throttle, brake, steer and handbrake linearly to
neutral over `fade` ticks.

**Method.** The production vehicle step (`VehiclePlugin`, default
`VehicleConfig`, flat ground, 60 Hz) runs a reference car on the true
quantized commands with zero delay, and a second car driven only by the
predictor. After a 150-tick full-throttle run-up the scripts are *steady
cruise*, *sudden braking* (throttle to full brake), *steering reversal*
(+0.8 for 40 ticks, then -0.8 for 40) and *evasive near-miss* (full lock
left 15, right 30, left 15 ticks, then lift). Error is distance to the
reference car's position (max over the scored ticks) plus speed gap.
*Lag*: commands reach the client `d` ticks late, so every tick is
extrapolated from age `d`. *Stall*: lag 3, then the feed stops 5 ticks into
the action and the next 15/30/60 ticks (0.25/0.5/1 s) are scored.

**Results** (max position error in metres, summed over the four scripts):

| policy | lag 3 | stalls (12 cases) |
|---|---|---|
| hold-last | 2.78 | **8.04** |
| age-aware hold 12 / fade 24 | 2.78 | 10.96 |
| age-aware hold 6 / fade 24 | 2.78 | 12.89 |
| age-aware hold 6 / fade 12 | 2.78 | 14.89 |
| age-aware hold 3 / fade 12 | 2.78 | 16.57 |
| age-aware hold 3 / fade 6 | 2.78 | 18.46 |

Per script, lag 3 / 6 / 12 under hold-last gives 1.49 / 2.23 / 3.78 m
(sudden braking), 0.38 / 0.79 / 1.74 m (steering reversal), 0.92 / 1.15 /
1.83 m (evasive). Fading under steady lag is ruinous (age is constant, so
once `age > hold` the throttle decays forever: hold 3 / fade 6 is 17–51 m
at lag 6–12); that is a property of the policy, not of the settings. In
stalls, a 1 s stall on the evasive script is the one place a candidate wins
(hold 3 / fade 6: 1.82 m against 4.35 m), but every candidate loses on
cruise and on braking/reversal at 0.25–0.5 s, which are the likelier
cases, because fading throttle cannot replace the braking or counter-
steer the driver actually applied.

**Choice: hold-last.** No measured age-aware setting beats it in total
(`prediction_policies_are_measured_against_a_zero_delay_reference` asserts
this), and it is the simpler contract. The age-aware code stays as the
measured candidate, unused by the app. Not decided: any stall longer than
the 267 ms redundancy (hold-last grows to 0.8–4.4 m at 1 s on the braking and turning
scripts), and wiring the predictor to a body, which is 05A-3. Evidence
level: synthetic integration on the production step; flat ground, default
car, no collisions, no retail data.

**Live policy (NETPLAY-05K, #1959).** The synthetic choice above is for
flat ground and no collisions. The live contact-body step runs on the
received store with `Policy::AgeAware { hold: 4, fade: 12 }`
(`netremote::LIVE_POLICY`): a client's bodies are always ahead of the
newest frame it holds (lead plus link latency, 9-19 ticks in the pileup),
so every live command is a prediction, never a received one. In the
three-car wall pileup (`net_contact`, unequal links) the pinned cars
steer in a closed loop and saturate left and right in turn; holding
that steering 10-20 ticks past its source drove the contact bodies into
the own car and the replay reproduced the same shove each snapshot (own
error 1.4-4 m that no replay removed). With hold-last the test failed on
roughly half of runs (1.3-20 m); with the fade it passed 19 of 19 in a row
(neutral bodies, the earlier baseline, 6 of 6). The replay is unchanged
(`command_at`: the stored command for its host tick, else what the live
step applied). Evidence level: synthetic integration over loopback with
the seeded lossy proxy; no retail data.

## Dynamic remote body, visual apart (NETPLAY-05A-3, #1894)

Behind `NetplayTimeline`, `spawn_remote` on a predicted client gives each
remote **human** seat two entities. The *contact body*
(`netdrive::PredictedRemoteBody`) is built from the same `vehicle_bundle`
the host steps for that seat: authored mass, inertia, centre of mass,
convex-hull collider, friction and restitution, `RigidBody::Dynamic`,
stepped by `vehicle_simulation`. It has no mesh and none of the
participant markers (`NetPlayer`, `RemotePick`, `Player`), so reconcile,
snapshots, damage and race systems still see one participant per seat.
`netremote::drive_predicted_bodies` (`FixedUpdate`) hands it one command
per fixed step from `RemoteFeed` (store + hold-last predictor) at the
client's `Session::tick`. The *visual* is the existing kinematic
`RemoteReplica` copy: it wears the model and keeps following the delayed
`RemoteLerp` interpolation, and its collider is removed, so each car has
exactly one collider. `sweep_remote_bodies` despawns a body whose visual is
gone (leave, changed pick); both carry the session's `SessionEntity`.
Host-planned bots and trailers keep the legacy kinematic copies; with the
selector off nothing changes.

**Not done here.** (Update, 05K: the host emits frames, the client absorbs
them into `RemoteFeed.store`, and the live step reads them. Pure hold-last
over that store failed the three-car wall pileup in `net_contact`, so the
live step fades stale commands; see "Live policy" below.) The body is not corrected towards the host:
`reconcile_own` restoring and replaying both cars is 05A-4. Evidence level:
synthetic integration (unit tests on `spawn_remote` with a dev car and the
lerp system); no retail data, no rendered capture.

## Send cadence, budgets and priorities (NETPLAY-06D, #1870)

Code: `mm2_net::pace` (pure, clock-injected: `Cadence`, `Budget`,
`Priority`), applied per seat in `mm2_net::rt` (`SeatOut`) and at the
source in `mm2_app::netdrive::publish_snapshots`. Evidence level reached:
**code checks and synthetic integration** over loopback. No WAN, retail or
rendered level. All numbers are *designed*, not recovered from the retail
game; the congestion policy that adapts them to a link is NETPLAY-10 (#1771).

### Rules

- **Cadence.** While `Playing`, `publish_snapshots` sends once per
  `60 / SNAPSHOT_HZ` fixed ticks (`SNAPSHOT_HZ` = 30, resource
  `SnapshotRate`, clamped 1..=120), counted in session ticks, so the render
  `Update` rate neither multiplies nor starves the stream and `FIXED_HZ` is
  untouched. Never deferred: a frame with fresh impact events, a frame
  whose seat set or reset epoch changed (a reset teleport leaves at once),
  and — so a stalled clock cannot silence the stream — a frame whose tick
  has not moved but whose wall interval has elapsed. Ready, Countdown and
  Results keep their per-update send (the Results repeats rely on it).
  `NetDriveReport::snaps_skipped` counts deferred updates.
- **Priority.** Each seat's lane has two bounded queues. `Movement`
  (`Data`, `Ack`, and snapshots sent by `broadcast`/`send_to`/
  `send_snap_groups`) always drains first and is never held by the budget.
  `Bulk` (groups marked so via `HostCtl::send_snap_groups_prio`) is sent
  only while the seat's byte budget has credit.
- **Bounds.** Movement queue 32 datagrams, max age 100 ms; bulk queue 32,
  max age 250 ms. An entry older than its age is discarded unsent and
  counted (`QueueStats::expired`); a full queue drops its oldest
  (`dropped`). `Host::lane_pace(seat)` reports both queues, datagrams and
  bytes sent, bulk deferrals and the deepest budget overdraw.
- **Budget.** Token bucket, 128 KiB/s sustained (1 Mbit/s), 16 KiB burst.
  Movement may overdraw it down to -32 KiB so bulk, not movement, pays for
  congestion.

### Measured (`cargo test -p mm2_net pace -- --nocapture`)

Bulk flood: a producer sends, at 60 Hz for ~2.4 s, a ~4.3 kB bulk group
(4 fragments, ~258 kB/s offered against the 128 KiB/s budget) next to a
movement group each tick.

- Movement delivered **120 / 120** ticks, in order; movement queue never
  dropped or expired anything, `max_wait` ~20-40 µs.
- Bulk: 480 fragments queued, 144 dropped by the 32-entry bound, 336 sent;
  112 head-of-queue deferrals for budget; only 22 / 120 bulk groups arrived
  whole. Bulk `max_wait` 242 ms, under its 250 ms age bound; the queue
  ends empty.
- Bandwidth: 349-353 kB sent in the run against a ceiling of
  `rate * duration + burst + movement overdraw` (about 386 kB); the test
  asserts that ceiling.
- A 40-tick burst of bulk into a stalled budget leaves at most 32 queued
  and drains or expires within the age bound.

App level (`playing_snapshots_leave_at_the_snapshot_rate_not_the_update_rate`):
a host updating every ~1 ms for 600 ms publishes at most 30/s + 2.

### Snapshot groups per seat (NETPLAY-10C-2)

`netdrive::publish_snapshots` no longer broadcasts one `Snap`. For each
rostered seat with an established lane it calls
`HostCtl::send_snap_groups_prio` with groups of the same `(generation, tick)`:

- **Group 0, `Movement`:** the seat's own entry, every island that holds a
  human seat (the seat's own island included), their trailers, the impact
  rows naming them, and the race row.
- **Group 1, `Bulk`:** the islands made only of bots and farther than
  `WORLD_GROUP_RANGE_M` from every human body, with their trailers and
  impact rows. Omitted when empty; the race row is not repeated.
  **`WORLD_GROUP_RANGE_M` is infinite today, so group 1 is never sent**
  (see "Why nothing is `Bulk` yet").

A contact island is the transitive closure of "a body (car or trailer) of
one seat is within `CONTACT_ISLAND_RADIUS_M` (20 m) plus both speeds times
`CONTACT_ISLAND_HORIZON_S` (1 s) of a body of the other" and "the two seats
were participants of one `ImpactEvent` this frame" (distances inferred from
the physics scale, not original values). It depends only on the frame's
data, so it is deterministic, and an island is never split across groups.

**Why a distant human is not `Bulk`.** The first cut put every seat outside
the receiver's island in `Bulk`. `net_contact`'s high-latency wall pile-up
then failed (seats ended 10-170 m apart between the two clients): on a lossy
link the congestion layer sheds `Bulk` outright, so a human who drove off was
never updated. Humans are few and small.

**Why nothing is `Bulk` yet.** With distant *bots* in `Bulk`, a retail
Cops & Robbers match (`net_cnr_bots_mp`, 2 bots, loopback) collapsed the
seat's lane: about tick 2550 the bulk queue stopped draining while movement
was idle, movement datagrams started to expire, and the controller fell from
131072 to 12288 bytes/s (`decreases` 437, 4188 movement datagrams expired,
`adapt.bulk` false) for the rest of the match, so the client's bots froze
(0 sampled ticks in common with the host).
The same run with every row in group 0 keeps 131072 and sheds nothing.

Follow-up #2041 measured it (lane stats logged from the host send loop,
`net_cnr_bots_mp` London, range 300 m): the collapse is **not** the `Bulk`
path. Sending the second group at `Movement` priority collapses the lane
identically (rate 131072 -> 12288 within ~2000 ticks, `bulk_sent` 0). What
tips it is the bytes of a second group in the headless harness, which runs
the simulation about 5x faster than the wall clock the rate is measured in,
so the demand per wall second is already about half the ceiling; the
resulting queue age cuts the rate and `PeerController` then could not climb
back: at the floor the movement queue, throttled by that rate, kept
expiring and ageing past the congested threshold, which reset `clean_since`
for ever. That trap is fixed (expiry and queue age at the floor are not
evidence; a floor peer probes upward, `a_rate_at_the_floor_climbs_again_
though_its_own_queue_keeps_expiring`), but the probes still fail while the
harness demand (about 22 KB/s) exceeds the 12 KB/s floor, so with a finite
range the client's distant bot still freezes late in a London match.
`WORLD_GROUP_RANGE_M` therefore stays infinite. Open: either the harness
must pace the simulation to the wall clock, or the lane needs ack evidence
(`on_ack` is never called) so a probe can be judged by delivery rather than
by its own queue; a bot sampled only when its `Bulk` group arrives also
needs `NetDriveReport::note_bot_poses` to merge the groups of one tick.

The unit tests `contact_islands_*` cover the partition; the lane-level
behaviour of a lost or delayed `Bulk` group is in `lane_pace_tests`.

### Not done / open

- `Playing` snapshot groups now go out with ACKed baselines, one per
  logical group id (0 = owner/human islands, 1 = `Bulk`) per seat
  (NETPLAY-10D-2, "Baselined snapshot states per seat"). The snapshot
  grouping and `Bulk` marking are done: see "Snapshot groups per seat"
  above.
- World catch-up no longer rides TCP for a seat with a lane (NETPLAY-10C-3,
  protocol v39); TCP stays only for seats without one:
  - `Props` (`HostCtl::send_props`) takes the lane's **reliable channel**,
    because the host commits a client's owed set and late-join snapshot
    position when a send succeeds. The channel numbers, repeats and chunks
    it (up to `MAX_RELIABLE_MESSAGE`); a refusal (window full, dead
    channel) is an `Err` and the owed mark stays set for the next frame,
    never a TCP fallback. The client's reliable receiver hands it to the
    inbox like `Cnr`/`Impacts`, so the existing freshness/dedup rules apply
    unchanged.
  - `Traffic` and `Signals` (`send_world_state` / `broadcast_world_state`)
    are state that the next frame supersedes, so they ride `Priority::Bulk`
    fragments on their own lane tags (`WorldCars` 7, `WorldSignals` 8) with
    one assembler per kind (their tick domains differ from `Snap`'s and
    each other's). The client admits a completed frame only as its own kind
    and only under the datagram's generation. They are shed before
    anything else but spend their own allowance (a quarter of the peer's rate,
    `PaceStats::world_sent`) rather than the movement/bulk credit, which a
    busy snapshot stream can hold at zero (a retail two-process run starved
    them completely under the shared bucket); nothing repeats them.
  Tested through the seeded lossy relay (`lane_world_tests`): a late-join
  `Props` run completes once each and in order, cars and lights recover on
  the next frame, a reliable `Impacts` completes once beside four full
  `Props` frames and queued cars, and a seat's movement groups all arrive
  with the pacer's `bulk_sent` advancing and no movement dropped or expired.
  Open: reliable `Props` data shares the *movement-priority* queue with the
  other reliable traffic (so events are never starved by bulk), which means
  a catch-up offered faster than the peer's budget does cost that seat's own
  movement; the producer paces the snapshot (`rows_for_client`) and the
  reliable window bounds it, but a per-seat catch-up byte budget is not yet
  applied. Other seats are unaffected (separate queues; only the host's
  shared uplink couples them).
- The tick-driven cadence path is unit-tested (`Cadence`) and exercised
  live only through the stalled-clock keepalive; a render-rate-vs-fixed-rate
  headless run is open.

## Two cars on one tick (NETPLAY-05A-4, #1895)

`netpredict::reconcile_own` now restores and replays the own car and every
remote contact body (`PredictedRemoteBody`) together. The recorded world
already holds the bodies (`capture_world` takes every `Vehicle`), so the
change is in what is compared, corrected and driven:

- **Common tick.** The snapshot that carries the own assertion
  (`input_ack`, host tick `T`) also carries each remote seat's body state at
  `T`. `apply_snapshots` stages those as `RemoteAssertion`s beside it
  (`OwnStage.remotes`, tagged with the snapshot's generation and tick, and
  used only with that snapshot's assertion). Own state at the ack and remote
  state at `T` are therefore one simulation instant, with no tick
  arithmetic.
- **Compare.** The replay runs when the own car *or any* remote body passes
  a position/orientation/velocity tolerance (the same four tolerances).
  `OwnPrediction.remote_replays` counts replays where a remote body
  diverged; `Outcome::Replayed` carries `remote_error`/`remote_diverged`.
- **Restore + replay.** The corrected capture takes the host state for the
  own car and each remote body; every replayed step drives all of them. The
  own car uses its stored command. A remote body uses
  `RemoteFeed::command_at(wire, client_tick)` for the `predicted_tick` of
  that step: the store's authoritative command if still there (it may have
  arrived after the tick was predicted), else what the live step applied
  (`RemoteFeed.applied`, recorded by `drive_predicted_bodies`, since
  `RemoteCommands::consume` forgets a used tick). The predictor itself is
  not run during a replay and the store's cursor is not moved.
- **Not put back.** Replayed contact bodies keep their replayed state. Only
  the delayed visual (`RemoteReplica`, driven by the snapshot lerp) is put
  back, because the restore writes its old captured pose.
- **Bounded recovery.** The named paths are unchanged and also cover the
  remote bodies: a remote body with an unknown command for any replayed step
  is `CommandGap`; `HistoryExhausted`, `TooDeep` and `ReplayRefused` as
  before. Each adopts the host state of the own car and of every remote body
  that had an assertion.

Still open: (05J/05K: frames are absorbed and read by both the replay and
the live step) and contact-specific
tests and cost numbers are 05A-5. Evidence level: synthetic integration on
the production step (twin-world equality for a two-car replay); no retail
data, no rendered capture.

## Reliable-message channel (NETPLAY-06E-1, #1897)

`mm2_net::reliable` is the transport-independent primitive for the few
messages that must survive a dropped datagram (Start/Cancel, roster and
despawn, reset epochs, outcomes — moved onto it by 06E-2/3). It is a pure
state machine (`ReliableSender`, `ReliableReceiver`, `ReliableChannel`)
driven by a caller-supplied millisecond clock, carried on two new lane
tags and wired into both lane endpoints (`PROTOCOL_VERSION` 35).

- **Wire**: `LaneTag::Reliable` (5) payload `id u64 | body`; `LaneTag::ReliableAck`
  (6) payload `cum u64 | mask u32` — `cum` is the highest id delivered in
  order, bit `i` of `mask` marks id `cum + 2 + i` buffered. Ids start at 1.
  Both ride the movement queue (never starved by the bulk budget) under the
  lane's normal per-datagram authentication, so they carry the session
  generation like any `Data` datagram: a reliable message before the host
  has a running generation is refused as `wrong_generation` and is the
  caller's (06E-2) problem to sequence.
- **Order policy**: one channel per direction per seat, **ordered and
  exactly once**. Lifecycle messages are state machines that must not see
  Cancel before Start.
- **Bounds** (designed numbers): at most `RELIABLE_WINDOW` = 32 ids between
  the oldest unacked message and the next (a span, so selectively acked ids
  cannot widen it past what the receiver buffers); body at most
  `MAX_RELIABLE_BODY` (one datagram); 8 transmissions per message, delays
  100/200/400/800 ms then 800 ms. A refused send is an `Err`
  (`Full`/`TooLarge`/`Dead`) and counted. When the oldest message runs out
  of attempts the channel goes **dead** — an ordered channel cannot skip —
  every unacked message is counted in `given_up`, and later sends return
  `Dead`. The receiver refuses ids beyond its window and, with 64 delivered
  messages unpopped, further ones (backpressure); the sender repeats, so
  nothing is lost silently.
- **Acks** are sent for every reliable datagram including duplicates, so a
  lost ack is repaired by the next repeat.
- **Nonblocking**: `reliable_send` locks only the seat table, pushes to the
  bounded queue and returns; repeats are emitted by the lane's sender
  thread. No `Writer`, no socket on the caller.
- **Untouched**: `LIFECYCLE_RESEND_WINDOW` and its dedup stay as they were;
  nothing is moved onto the channel yet.
- **Tests** (`lane_reliable_tests`): the relay drops and duplicates from the
  first probe, the first reliable datagram is then lost on purpose (both
  directions), streams of 60 messages survive loss/reorder/duplication and
  burst loss both ways exactly once in order, three lost acks converge, a
  black hole gives up bounded (`3 × 8` datagrams, `given_up` 3), window and
  size refusals are counted. The reliable clock is manual in these tests.

### Lobby scope and the roster (NETPLAY-06E-2a, #1903)

- **Admission with no running generation**: a `Reliable`/`ReliableAck`
  datagram naming generation `0` (`rt::LOBBY_SCOPE`) is admitted by the
  host and the client lane even when no session runs (lobby, after
  Cancel). Authentication is unchanged — seat, token, source address and
  replay window. Every other tag, and a reliable datagram naming a
  nonzero generation, still needs the running one (`wrong_generation`).
  A reliable message names the generation it was sent under and repeats
  under it.
- **Roster and removal**: each roster *change* (join, ready, pick,
  removal) queues the final `Message::Roster` on every lane seat's
  reliable channel at generation 0 (`broadcast_roster`); a removal is a
  roster without the departed peer. The client lane moves a delivered
  `Roster` into the inbox like the other host events (`is_host_event`),
  so `Client::recv` returns it and `reliable_recv` never sees it. The TCP copy and the per-keepalive TCP roster
  resend stay (the resend does not touch the reliable window); the app
  replaces its roster wholesale, so the second copy is harmless.
- **Tests** (`lane_lobby_tests`): a lobby-scope message each way with the
  first datagram lost from connection start, a stale nonzero generation
  refused, and a join roster (first datagram lost) plus a removal
  delivered exactly once each (`receiver.delivered`).
- **Leave and ready (NETPLAY-06E-2c, #1905)**: a lane client queues its
  `Leave` on the reliable channel at generation 0 *and* still sends the TCP
  `Leave` (the half-close handshake, and the app's repeat until the host's
  close, stay until the reliable path is proven in play). `set_ready` (the
  rematch / ready-again intent) rides the reliable channel alone, falling
  back to TCP only when the channel refuses it, so reliable copies stay in
  order and a stale flag cannot land after a newer one. The host takes
  `Leave`/`SetReady` off the front of a seat's delivered queue into the
  mailbox's intent queue (`PeerMail::intents`, purged with the seat); the
  lobby loop polls it every 50 ms (`INTENT_POLL`) and handles each exactly
  like the TCP copy: `remove_player` reaps a seat once, a repeat finds
  nothing. Tests (`lane_lobby_tests`): a reliable-only `Leave` after a lost
  first datagram yields one `Left{Quit}` for two copies; three `set_ready`
  flags after a lost first datagram arrive once each, in order.
- Open: retiring the TCP `Leave` and its app retry; the
  vehicle pick still rides TCP only.

### Start and Cancel (NETPLAY-06E-2b, #1904)

- **Carried**: `Start` (the broadcast, and the unicast to a late joiner) and
  `Cancel` are queued on every seat's reliable channel (`queue_lifecycle`)
  *at the lobby scope* (generation 0), beside the TCP copy. The lane
  generation changes under them (`Start` raises it, `Cancel` clears it), and
  generation 0 is always admitted, so neither a repeat nor an ack races that
  change. The channel is ordered and exactly-once, so a `Cancel` is never
  applied before its `Start`. The client lane returns both through
  `Client::recv` (`is_host_event`).
- **Apply once, stale ignored**: the channel delivers each message exactly
  once (`receiver.delivered`, asserted). The TCP copy and its resend window
  still hand `Client::recv` repeats of the same generation; the app keeps
  ignoring a `Start` or `Cancel` whose generation it has already applied
  or never entered, as before (`Client::recv` itself does not filter, so
  the resend tests can still observe the repeats, and a generation-0
  `Start` still reaches the app to be refused).
- **Size (#1930)**: a `Start` body can exceed `MAX_RELIABLE_BODY` (the
  session blob is bounded by `MAX_SESSION_PARAMS`, 4096 bytes). The
  channel now splits any message into at most `MAX_RELIABLE_PIECES` (8)
  datagram-sized pieces, each its own id with a `more` byte in the data
  header (`id u64 | more u8 | body`, protocol v37). Order and exactly-once
  delivery do the reassembly: the receiver concatenates pieces in id order
  and delivers the whole message once, so nothing above the channel sees a
  piece. A message is accepted whole or refused whole (`Full` when its ids
  do not all fit the window, `TooLarge` over `MAX_RELIABLE_MESSAGE`). A
  run of `more` pieces that never terminates is counted `malformed` and
  never delivered. Digest-plus-fetch was not needed.
- **Kept**: `LIFECYCLE_RESEND_WINDOW`, the Keepalive lifecycle state and
  the TCP copy — a seat whose lane never establishes (UDP blocked) still
  depends on them, and what a lane-down seat does instead is not decided
  yet. Nothing is removed; every `Start` now also rides the channel.
- **Tests** (`lane_lobby_tests`): with a datagram dropped, `Start` and then
  `Cancel` are each delivered on the channel exactly once
  (`receiver.delivered`) and returned once, `Start` first; an oversize
  `Start` (a 4096-byte blob, first datagram dropped) is delivered on the
  channel once, whole; `reliable` unit tests cover reassembly across loss,
  reorder and duplicates, atomic refusal and an unterminated run.

## Two-car contact tests and costs (NETPLAY-05A-5, #1896)

`crates/mm2_app/src/netpredict/contact_tests.rs` (unit tests of the
production step; run `cargo test -p mm2_app --lib contact_tests --
--nocapture --test-threads=1` for the tables). A *host* world drives both
cars on the true commands and is the uninterrupted zero-delay reference. A
*client* world drives its own car on the same commands, the other human's
contact body only from hold-last commands `delay` ticks old
(`RemotePredictor`), and corrects both bodies with `reconcile` from host
snapshots that also arrive `delay` ticks late (every tick; `loss` drops a
seeded share). Everything is in-process: the lossy-proxy sockets are not
involved, so "dropped snapshot" here is a seeded draw, not the UDP proxy.
Delays 3/6/12/30 ticks (50 ms-0.5 s). An *impulse* is a one-tick velocity
change over 0.6 m/s; a *contact start* is a `CollisionStart` between the 
cars raised by the live world.

| Scenario (host contact tick) | result with `reconcile` |
|---|---|
| head-on (227), rear-end with sudden lead braking (245), push a stationary car (207), pushed while stationary (207) | At every delay the first local response is the host's tick for both cars, impulses and contact starts equal the host's (no duplicate impulse from the up-to-43 replays), corrections at most 0.13 m, final position error < 0.05 m. The drivers hold their pedals into the contact, so hold-last predicts it exactly. Without `reconcile`, the rear-end shunt lands 3-15 ticks late and the cars end 1.5-3.8 m from the reference. |
| false contact: opponent swerves at tick 200, contact would be ~227 | Delay <= 12: the news arrives first, client never touches, corrections < 0.1 m. Delay 30: the client shoves (6 and 5-11 impulses) until the command arrives; correction 3.4 m on each car, then both end within 0.05 m. Without `reconcile` the cars end 5-87 m from the reference. |
| missed contact: opponent turns in at tick 210, contact at 227 | Delay 3/6 (< the 17-tick lead): first response on the host's tick, corrections 0.01-0.03 m. Delay 12: response 3 ticks late, correction 1.9 m, off by > 0.1 m for 8 ticks. Delay 30: 23 ticks late, correction 7.9-8.1 m, converged 30 / 41 ticks after the host's contact. Final error 0.00 in all cases. Without `reconcile` the pass is never a contact. |
| 40 % of snapshots dropped (delay 6 and 30, all six scenarios) | Same end states (< 0.05 m), no recoveries, contact starts equal to the host's where the contact is predicted. Replays are deeper (a correction of up to 0.5 m in the rear-end case). |

**Infinite-mass shove.** The pre-05A-3 client held the remote human car as a
kinematic copy. In "pushed while stationary" (delay 6) the shoved car peaks
at 17.2 m/s in the host and with the dynamic body, and 20.2 m/s with the
kinematic copy (1.17x), ending 13 m from the reference. The 10.5 vs 5.7 m/s
(1.8x) of `docs/research/net.md` was a different scene (a parked start-grid
slot, with a lighter push); this synthetic pair reproduces the direction,
not that ratio.

**Contacts a replay discovers (#1902).** `ReplayQuarantine` swallows the
`CollisionStart`s a replay raises, so a contact the client only learns of
through a replay (the missed-contact runs at delay 12/30) used to raise no
live contact start: damage, sparks and audio never fired though the car
velocity changed. `ReplayReport::collision_starts` now returns the replayed
starts (a pair already touching in the restored capture is dropped from step
0: the restore does not carry its started-touching flag, so the first step
announced it again). `OwnPrediction` keeps a ledger of the starts the live
world raised (`log_contacts`, from `record_prediction`) and matches each
replayed start against it: the same body pair, not yet behind the ack and
within 4 ticks (`CONTACT_JITTER_TICKS`, designed: the replayed start moves
2-4 ticks per snapshot while the car converges) is the same contact and only
pulls the ledger tick earlier; anything else is written once into the live
`Messages<CollisionStart>` by `reconcile` (`contacts_surfaced`). A start the
replay no longer finds stays raised (it played). The raised event is the
replay's, so it fires when the replay finds it, not at the host's tick. The
contact tests now assert contact starts equal the host's in the missed-contact
scenario at every delay (including the host's second start 2 ticks after the
first, at delay 30). The false-contact scenario at delay 30 still raises a
start the host never had (the client really did touch live); that is the
prediction error, not a replay artefact, and is not corrected here.

**Cost for two cars** (dev profile, Apple Silicon, one run, load not
recorded; treat milliseconds as machine-dependent):

| Item | Value |
|---|---|
| Physics tick, median, 1 car / 2 cars apart | 0.167 / 0.171 ms |
| `capture_world`, 2 cars on a flat plane | 0.003 ms |
| `reconcile` + replay of both cars, depth 3 / 6 / 12 / 30 | 0.29 / 0.53 / 1.04 / 2.56 ms (about 0.085 ms per replayed tick) |
| `Snap`, 1 car / 2 cars (encoded) | 138 / 255 B (+117 B) |
| `RemoteCommands` frame, 1 seat x 16 ticks | 142 B (8.5 kB/s at 60 Hz if sent each tick) |

State memory per capture is the 03F figure (the vehicle part is per car);
the capture here is tiny because the world holds no props. At 60 Hz a
second car adds about 7 kB/s of snapshot plus up to 8.5 kB/s of command
frames, before the 06D cadence reduces both.

Evidence level: synthetic integration on the production step. No retail
data, no sockets, no rendered capture. Open: acceptance-level evidence over
the real transport with a second process.

## NETPLAY-05A integration status (#1889)

All five slices are on main: wire contract (#1892), prediction policy
(#1893), dynamic remote body (#1894), joint replay (#1895), contact tests
and costs (#1896). The measurements are in their sections above; the
two-car CPU/state/bandwidth numbers are in the #1896 cost table.

Advanced (synthetic integration on the production step, no retail data, no
sockets, no render): F25 AC1 partly (a remote human car is a finite-mass
dynamic body on the client), AC2 for head-on, rear-end and push, AC3, AC4
partly, and the two-car causal requirements.

Still open: acceptance evidence over the real transport with a second
process; contact events discovered only through a replay (#1902); more than
two cars and trailers (NETPLAY-05B, #1890).

## Three or more human cars (NETPLAY-05B, #1890; first slice)

`reconcile` and `Request.remotes` were already N-generic (05A-4 builds one
`RemoteDrive` per `PredictedRemoteBody`), so this slice adds no production
code: `netpredict/multi_tests.rs` generalizes the 05A-5 harness to `n` cars
(host world = zero-delay reference, client drives car 0 on true commands and
every other car on hold-last commands, `reconcile` from delayed host
snapshots) with per-seat command delays and seeded snapshot loss. Run
`cargo test -p mm2_app --lib multi_tests -- --nocapture --test-threads=1`.

| Scene | Result |
|---|---|
| side swipe, three-car pileup, pinned against a wall, 4-car ring | Zero delay: both worlds agree to 0.00 m, no replays. Delay 3-12 ticks: hold-last predicts these exactly (drivers hold pedals), 0 replays except the side swipe (1-4 replays). Delay 30: 13-23 replays, depth 30, peak correction 0.35-0.64 m, final error 0.00 m, no recoveries. |
| pileup, seats at 3 and 24 ticks, 30 % snapshot loss | 6 replays (depth 6), peak error 0.03 m, final 0.00 m. |

The side-swipe drift produced no velocity impulse above 0.6 m/s (the
rubbing is gentle), so its evidence is the position agreement, not impulse
counts.

Cost, 2..8 seats (dev profile, one run on a loaded machine, so
milliseconds are indicative only; the 05A-5 table was measured on a quieter
one): physics tick 0.8-1.6 ms, `capture_world` 0.008-0.018 ms, vehicle
capture 552 B/car, replay of a depth-30 correction 10-16 ms at 4 seats.
Wire at 60 Hz without 06D cadence: `Snap` 255 B (2 seats) to 957 B (8), a
16-tick `RemoteCommands` frame covering all remote seats 142 B to 886 B,
i.e. 23.8 kB/s to 110.6 kB/s together. Replay depth is bounded by
`MAX_REPLAY_STEPS` (120); beyond it the named recovery paths adopt the host
state.

Evidence level: synthetic integration on the production step. Open for
#1890: reset epochs and damage/mass
changes tied to a tick, desynchronized re-entry, a genuinely causal
host + two clients test over the real transport with the impairment proxies,
and showing local input stays responsive when the replay budget bites.

## Reset asks, Cops & Robbers frames and impact events on the channel (NETPLAY-06E-3, #1899)

Three one-shot message classes ride the 06E-1 reliable channel when the
seat has an established lane (`PROTOCOL_VERSION` 36):

- **`ResetRequest`** (client → host): `ClientCtl::request_reset` sends it
  reliably, falling back to the control stream when there is no lane or
  the channel refuses it. The lane receive path decodes it as it is
  delivered and absorbs it into the same newest-wins mailbox slot the TCP
  path uses (`PeerMail::absorb_reset`), so the host's `drain_resets` and
  the stale-generation drop are unchanged. The retry ledger in
  `send_reset_request` stays: it repeats an ask the *host declined* (the
  epoch snap is still the acknowledgement), the channel only guarantees
  the ask reaches the mailbox. Only the front of the delivered queue is
  inspected, so any other message class stays queued for `reliable_recv`
  (06E-2 consumers) and the asks behind it are absorbed as soon as it is
  taken.
- **Cops & Robbers frames** (host → client), including the decided
  outcome: `publish_cnr` calls `HostCtl::reliable_broadcast`, which queues
  the frame on every established-lane seat's channel and sends the rest —
  TCP-only seats and any seat whose channel is full or dead — over the
  control stream, so a refusal never drops it. The client stage keeps the
  freshest `(revision, elapsed)`, so a frame seen twice applies once.
- **Impact events** (host → client): the new `Message::Impacts` carries
  `SnapImpact` rows in chunks of `IMPACTS_PER_MESSAGE` (16). The in-frame
  `Snap` rows and their `IMPACT_ROW_REPEATS` stay (the app-level impairment
  harness drops *TCP* frames whole, which only an in-frame repeat
  survives); the client dedups both copies on `(generation, seat, id)`
  (`RemoteSnaps::queue_impacts`).

Client side, a delivered `Cnr`/`Impacts` goes to a bounded
(`REL_INBOX_CAP`) reliable queue of the lane inbox that `Client::recv`
drains before lane `Snap`s; nothing is dropped, because the receiver keeps
what does not fit and the sender repeats.

**Not moved: damage and reset epochs.** Both are *state* in every
`SnapEntry` (`damage` fraction, `epoch`), re-asserted by each snapshot, so a
lost frame is corrected by the next; there is no one-shot damage message.
The one-shot part of damage — texel deformation — is the impact events
above.

**Generation end retires the scoped messages (#1954).** The channel is
ordered, per seat, and survives generations; a datagram for a generation
the receiver is not running is refused without an ack, so a reliable
message minted for a generation that has since ended would eat its 8
attempts and kill the channel. It no longer does: when the generation
changes the sender *retires* its unacked generation-scoped messages
(`ReliableSender::retire`). The id is ordered, so it cannot simply be
dropped (the receiver would wait for it forever): the message becomes an
empty lobby-scope tombstone (generation 0, always admitted) with fresh
attempts, and the receiver consumes it in id order without delivering it
(`ReceiverStats::retired`; no real message has an empty body). The host
retires in `set_generation` (`Start` of generation N retires everything
not N; `Cancel` sets 0 and retires everything scoped); the client retires
through the generation it learns ended (a newer `Start`, or `Cancel`).
Lobby-scope messages (generation 0: `Start`, `Cancel`, roster, `Leave`,
ready) are never touched. The consequence is that a late reset ask, `Cnr`
outcome or impact event minted across a `Cancel` is lost by design (its
generation is over); the in-frame `Snap` rows and the control stream carry
state that outlives it. A datagram of the old generation already queued
for the wire is still refused by the receiver, which costs one datagram,
not the channel. Evidence (synthetic integration):
`generation_scoped_messages_in_flight_across_a_cancel_do_not_kill_the_channel`
(`lane_reliable_tests`, seeded `UdpImpairProxy`, both directions) and
`retiring_keeps_the_order_and_never_delivers_the_message` (`reliable`).

Process-level evidence (`net_lane_loss`, #1950): a host and a client as
separate `mm2` processes, the client's lane routed through a seeded
`UdpImpairProxy` (`mm2 --join --lane-via <dial>/<advertise port>`, a hidden
test seam), 15 % loss/duplication/reordering plus a datagram dropped ahead
of each event: `Start` (generation 1), a reset ask (sent once, granted once),
`cancel` and the client's re-ready, `Start` (generation 2) and the client's
`Leave` (one `event=left cause=quit`). The drop is armed just before the
event, so under live input traffic it may land on another datagram; the
guaranteed first-datagram-lost cases are `lane_lobby_tests`'.

Evidence level: synthetic integration over the seeded UDP impairment proxy
(`lane_reliable_tests`): the first reset ask, the first Cnr outcome frame
and the first impact event are each lost on purpose from connection start
under 15 % loss/duplication/reorder and arrive exactly once; a reset ask
behind another message class is still absorbed; a full window falls back to
TCP with every message delivered once. No retail data, no rendered capture.

## World-actor history for contact replay (NETPLAY-05C, #1891)

A replay used to restore cars and dynamic props and then run against
whatever the rest of the scene looked like *now*. Three things differ
between the replay window and today, and each has a rule
(`mm2_vehicle::physics_world`, `replay::EntryGate`):

| Actor | Body type kept | Rule during the window |
|---|---|---|
| Kinematic traffic / movers | `Kinematic`, never made dynamic | `WorldCapture::movers` records pose and velocity per tick. Before replayed step `S[t] -> S[t+1]` the mover is set to the pose of `S[t]` and the velocity of `S[t+1]` (a `FixedUpdate` driver writes the velocity before the step that integrates it, so the post-step capture holds the velocity that step used). After the last step it sits at its last recorded state. The traffic system itself never runs in a replay. |
| Dynamic props / bangers | `Dynamic` | Restored from `S[from]` as before. A prop **spawned inside the window** (in `RestoreReport::uncaptured`) gets `RigidBodyDisabled` for the replay, is enabled at the first tick the history holds it and posed as recorded there (`ReplayReport::entered`, `Some(tick)`); one in no captured tick stays out for the whole window and rejoins where it is. |
| Despawned since `from` | n/a | Cannot be restored: reported in `RestoreReport::missing`, the replay runs without it. A contact with it inside the window is not reproduced; the host's next snapshot corrects the residue. |
| Repurposed in place (broken banger, recycled traffic car) | n/a | `ActorIncarnation` differs: `Retired`, the replay refuses (03E). Movers and entrants with a changed incarnation are not written through. |
| Static scenery | `Static` | Not recorded: thousands of colliders the session does not change. A static collider added or broken at runtime is **not tracked** (a broken banger is repurposed, so it is caught by the incarnation rule). |

Speculative contact: the prediction's impulse on a prop lives in the
abandoned future, which the restore overwrites with `S[from]`; only the
replay applies the impulse again, so it lands once
(`a_prop_that_spawned_in_the_window_enters_at_its_tick` compares the prop
with the live record). Wire captures carry no movers (`movers: None`) and
never gate kinematic bodies. Damage, Cops & Robbers rules and impact
events stay with the host and NETPLAY-08.

Evidence level: synthetic integration on the production `VehiclePlugin`
step (`crates/mm2_vehicle/tests/world_actors.rs`): a kinematic mover that
crosses the car's path and stops mid-window (replay with history matches
the live record to 1e-3 m / 1e-2 m/s; the same replay with the mover
history stripped diverges by more than 5 cm), a prop that spawns and is
pushed inside the window, and a prop that despawns. Not covered
there: the client's traffic copies (`worldtraffic::apply_traffic`) in a
session, and `RigidBodyDisabled` entrants against the app's own systems;
NETPLAY-05H (below) covers both and found that `netpredict` was not using
the history at all.

## Trailer rigs in the predicted replay (NETPLAY-05D, #1911)

A jointed trailer is a `Vehicle`, so `capture_world` / `replay_from` always
restored and re-simulated it. What was missing in `netpredict` was the
*correction*: the host's state went onto the own car and the remote bodies,
and the trailer stayed at its recorded pose. `Request.trailers` now lists
the real trailers of the own car and of the remote contact bodies
(`TrailerDrive { body, towing, assertion }`); the trailer is then a partner
like a remote body:

| Step | Trailer rule |
|---|---|
| Staging | `apply_snapshots` stages `Snap.trailers` rows beside the remote seats (`OwnStage::trailers`, keyed by the towing seat; `own_wire` names the own seat), gated on the same snapshot as the assertion. |
| Compare | A trailer outside the position/rotation/velocity tolerances triggers a replay like a diverged remote body (`remote_diverged`). |
| Restore | The boundary capture takes the host's trailer row, so the hitch joint never sees a car and a trailer from different ticks. |
| Replay | The trailer has no command: each step copies the tractor's brake and handbrake onto it, as the live `trailer_input` system does. |
| Recovery | The host's trailer row is adopted with the car. |
| Attach / detach in the window | `HitchChanged` makes `replay_from` refuse; the existing `ReplayRefused` recovery adopts the host state for the whole rig and clears the history. |

The predicted client's kinematic trailer copies (`RemoteTrailer`, wire-driven,
no joint) are not partners. So today only the **own** rig's trailer is a real
body in the client's physics; a remote car's trailer is not simulated in the
replay, and a contact with it is not predicted (done in #1916, below).

Tests (`netpredict/trailer_tests.rs`, `cargo test -p mm2_app --lib
trailer_tests -- --nocapture`): car 1 rear-ends the trailer of car 0 (host
trailer takes two impulses). Zero delay: client matches the reference to
0.00 m. Delay 3 / 6 / 12 / 30 ticks: 0 recoveries, final error under 1 mm
for every body, hitch anchor gap at most 0.014 m, peak body error 0.001-0.03 m
(car 1 at delay 30, 0.65 m, before the first replay). Same run at delay 12
with the trailer left out of the request: 107 replays instead of 25 and a
peak trailer error of 0.096 m instead of 0.024 m. A joint removed inside the
window: `Recovered(ReplayRefused)`, car, remote body and trailer all take the
host state.

Evidence level: synthetic integration on the production step. Not covered:
the real transport, a trailer on a retail car, trailer attach by a *new*
tractor mid-session.

## A real trailer for predicted remote cars (NETPLAY-05E trailer, #1916)

Under the `NetplayTimeline` selector `spawn_remote` now gives each
`PredictedRemoteBody` that tows a trailer a second trailer, built by
`spawn_contact_trailer`: a dynamic body from the same `vehicle_bundle` the
host uses, hitched to the **contact body** (not the visual) by the same
`SphericalJoint` limits and `JointCollisionDisabled` as
`car_visual::spawn_trailer`. It is marked `PredictedRemoteTrailer { wire,
visual }` and carries no model.

| Entity | Role |
|---|---|
| Kinematic trailer copy (`RemoteTrailer`) | Delayed visual, driven by `Snap.trailers` through `RemoteLerp`; loses its collider so the rig has one. |
| Contact trailer (`PredictedRemoteTrailer`) | Dynamic collider, replayed with its tractor. `netpredict::trailer_drives` finds it through `Trailer::towing` = the `PredictedRemoteBody`, and pairs it with the staged row for that body's wire id (`OwnStage::trailers` already stages every row). |

`apply_snapshots` ignores the contact trailer (no `RemoteTrailer`, and its
`towing` is not the own seat), `sweep_remote_bodies` despawns it with its
visual (so a leave or pick change removes it, joint included), and the
trailer census does not count it. `HitchChanged` on a remote reseat is the
existing `ReplayRefused` recovery (`a_hitch_change_inside_the_window_...`).
Bot copies and sessions without the selector keep the kinematic copy alone.

Tests: `netpredict/trailer_tests.rs` `remote rig` cases (the own car
rear-ends a remote car's trailer, production `trailer_drives`, zero-delay
host reference): zero delay matches to 0.00 m; delay 3-30 ticks: 0
recoveries, final error 0.00 m, hitch gap 0.014 m, peak error 0.09 m at delay
30. `netdrive` test `a_contact_trailer_is_jointed_to_the_contact_body` checks
the spawned layout and the sweep. The remote's commands in that scene are
constant, so hold-last predicts them exactly and replays only start at delay
30; a varying remote driver is not covered. Evidence level: synthetic
integration; not run against a retail trailer car or the real transport.

## Reset epochs of remote cars in predicted contact (NETPLAY-05E, #1912)

A host reset teleports a seat; its `Snap` row carries the new reset epoch.
The own car already clears its history on one. A remote car's contact body
(`PredictedRemoteBody`) now carries the epoch its recorded history reflects
(`epoch`, stamped 0 at spawn like `ResetEpoch`), and `RemoteAssertion.epoch`
is the seat's epoch in the snapshot (a trailer row takes its towing seat's).
`netpredict::reconcile` compares them on the wrapping `u8` circle:

| Assertion epoch vs the body's | Rule |
|---|---|
| equal | compared and corrected as before |
| ahead (the host reset the seat) | the recorded past of that body predates a teleport: the replay is forced even if the poses happen to be close. The restored world takes the host's post-reset pose at the compared tick and the own car is replayed against it, so a car that already passed the spot on the client meets the reset body where the host did instead of being pushed out of it. The body's epoch advances only once an assertion was *consumed* (`Matched`, `Replayed` or `Recovered`); a skipped snapshot leaves the reset pending for the next one. |
| behind | dropped from the request (`OwnPrediction::stale_remote`): never compared, never written back over the reset |

Snapshots already arrive in order (`RemoteSnaps::push` drops older rows per
seat), so "behind" is a guard rather than an observed path. No restitution, grip or
collision-layer change is involved and no car is ghosted.

Found while measuring: a correction at replay depth 0 (the snapshot acks the
newest tick, e.g. loopback) restored nothing, because there was no step to
run; the history recorded a correction the world never took (a 36 m error on
the reset body that never closed). The corrected capture is now restored
without stepping.

Tests: `netpredict::tests` (forced replay then `Matched`, stale epoch ignored,
epoch wrap, depth-0 correction) and `multi_tests` (`reset_into_the_path`: car 1
is idle off the road and is reset at tick 150 into car 0's path; delays 0/3/12/30
and 30 %/60 % snapshot loss: 0 recoveries, final error 0.00 m, no lift of car 0
over the host's, one reset folded in). The same scene with epochs ignored is
measured for comparison and gives the same result at delay 3-30: the pose
difference of a real teleport already forces the replay, so the epoch's
contribution is the forced replay for close poses, the stale guard and the
pending-until-consumed rule, not a different trajectory in this scene.

Not covered (evidence level: synthetic integration on the production step):
the real transport with the impairment proxies; the vehicle crate's
`lifetime::ResetEpoch` / `SessionEpoch` components are still not wired in the
app (captures are stamped with the own car's `EpochRef` only, and the
`PredictedRemoteBody::epoch` carries the seat's reset epoch instead, which is
what reconcile needs).

### A reset starts a new life on every peer (NETPLAY-08D, #1977)

The host's `vehicle_reset` gives the teleported seat a fresh `VehicleState`
(engine, gearbox, tyre bristles, suspension, pending forces; the gyro
counters are run evidence and survive, `VehicleState::start_new_life`).
Before this change the pose reached the other peers but the old life did
not leave them:

| Where | Before | Now |
|---|---|---|
| Own car on a client, epoch advance in a `Snap` | pose/velocity taken, drivetrain and tyre state kept: a kick on the next tick | `apply_snapshots` queues `start_new_life` for the seat (and the own rig's trailer, which reseats with it); a later same-epoch snapshot never zeroes it, reconciliation restores the real state through the replay |
| Remote contact body, epoch ahead | host pose written over the recorded vehicle state | the corrected capture carries a fresh state (`renew_reset_bodies`) with the host's body state; input and run counters are kept |
| Remote copies (`RemoteReplica`) | state comes from the v7 tail each snapshot | unchanged |

How many ticks before the compared tick the host reset happened is not on
the wire, so a contact body's fresh state is taken at the compared tick: an
inference, not a wire fact. The player's input filter (`BufferedPlayerInput`)
is local to one peer and reaches the host only as quantized commands, so it
cannot diverge between peers and is left alone (whether retail clears it on
a reset is unknown). Tests: `netdrive::tests::an_authority_reset_of_the_own_seat_starts_a_new_life`,
`netpredict::tests::a_reset_remote_body_continues_from_a_fresh_state`.
Evidence level: synthetic integration.

The remainder (NETPLAY-08D-2, #1992), same evidence level:

| Item | Test |
|---|---|
| Damage repaired by the reset | `netpredict::tests::a_reset_that_repaired_the_body_drops_its_recorded_impairment`: the damage byte of the reset snapshot overrides the impairment recorded before the teleport |
| Damage carried over a reset | `netpredict::tests::a_reset_that_kept_the_damage_carries_the_authored_impairment` |
| Late join, disconnect and rejoin | `netdrive::tests::a_joiner_behind_the_hosts_epoch_starts_clean_once` (own seat at epoch 0 meets the host's epoch 3), `netpredict::tests::a_rejoined_body_behind_the_hosts_epoch_is_renewed_once` (contact body) |
| Rematch, city reload, C&R round change | one boundary for the wire (stream reset, new generation, seats respawn at epoch 0, ticks restart): `netdrive::tests::a_new_session_generation_starts_its_seats_clean`, `netpredict::tests::a_new_generation_drops_the_recorded_past` |
| Trailer rig reset over the transport | `net_drive::a_reset_trailer_rig_reseats_*` (three processes, retail `vpsemi`) now also read alice's `--net-trace` and require that the tractor's first sample of the new life is far below its old speed and nothing jumps in velocity or position in the next 120 ticks |

A repair is only known from the damage byte of the snapshot that carries the
reset. When that snapshot does not name the seat's damage (no entry for the
seat), the contact body keeps the impairment it recorded: whether the host
repaired is not on the wire in that case. Not covered: bob's view of alice's
tractor and trailer pose in the trace (the trace records the own car only; bob
is covered by the `trl=` reseat counters), and a run on the UDP lane.

### A replayed gate crossing never awards on a client (NETPLAY-08E, #2093)

A checkpoint is the gate rule's (`advance_race`, `FixedLast`) business, not
the physics schedule's. Two proofs, synthetic integration on the production
systems:

| Claim | Test |
|---|---|
| A client whose own car passes a gate inside two successive replays (60 and 45 steps) keeps its `RaceProgress` (crossings, cleared, state) and `ResultLedger` as they were; the same script on the authority counts the crossing once and records one result; the corrected path does cross the gate (the rule fed that path finishes) | `netpredict::tests::a_replayed_gate_crossing_awards_nothing_on_a_predicted_client` |
| The impairment byte the host re-sends with both corrections is stamped into the recorded past once (`modifier_corrections == 1`) | same test |
| The host's progress row for the crossing and the finish, redelivered on the same tick, a later tick, an older tick, or an older generation, leaves cleared/lap/crossings/route clears, state and the ledger unchanged and applies no second row | `netdrive::tests::a_redelivered_gate_row_is_one_award` |

The client does not run `advance_race` live either (it is authority-gated),
so the first test shows the replay path adds nothing, not that the gate rule
was suppressed inside it. Not covered: a process-level run over the
transport (the CNR authority test covers gold/score, not a checkpoint), and a
row of an older *reset* epoch with a newer tick for a seat that is still
racing (the mirror takes the authority's counters verbatim; whether retail
ever lowers them is unknown). Evidence level: synthetic integration.

### Re-entering seats (NETPLAY-05E, #1933)

A contact body that spawns after the compared tick is not in the recorded
world, so it used to sit at its spawn pose, disabled during the replay and
re-enabled where it was: a stale body (up to 7 m off at a 30-tick delay in
`side_swipe`, with a spurious client impulse) until the first assertion whose
tick the history held. Reconcile now *seeds* such an entrant: when the
snapshot has its row and the feed has a command for every replayed step, its
current vehicle state is put into the corrected capture with the host's body
state of the compared tick, the replay is forced even if the own car agrees,
and the body is stepped on its commands with the rest. An entrant without
commands is left as before (no recovery is forced for it); trailers are not
seeded.

Test: `multi_tests::a_seat_that_enters_after_the_compared_tick_is_seeded_from_the_host`
(car 1 does not exist on the client until tick 100, spawning at the pose the
client last had; delays 0/3/12/30: 0 recoveries, entrant peak error 0.07 m at
delay 30, client impulses equal the host's) and `a_re_entering_seat_survives_lost_snapshots`
(30 %/60 % loss). Evidence level: synthetic integration; not the real
transport.

## Local input under a maximum replay, and the cost to eight rigs (NETPLAY-05G, #1914)

**Where the replay sits.** `reconcile_own` runs in `FixedFirst`, between 
fixed steps. The live input is written afterwards, in `FixedUpdate`
(`input::apply_player_input`, then `netcmd::capture_and_send`, which also
quantizes it for the step), and the step follows. The replay writes stored
commands into `VehicleInput` while it runs, but every tick's live write
lands after it, so the step of the tick that carries a replay uses that
tick's own command: the replay cannot delay or replace the local input, at
any depth. What it spends is wall time before the step, as one block (it is
neither spread across frames nor budgeted per frame: the bound is
`MAX_REPLAY_STEPS`, past which the named recovery adopts the host state at no
replay cost).

Test (`multi_tests::the_local_step_answers_input_at_every_replay_depth`, `cargo
test -p mm2_app --lib multi_tests -- --nocapture`): the client runs in the
production order (reconcile, write input, step, record) and from tick 125
every tick gets a host state half a metre off, so a replay runs on every
tick, including the one that carries a steering change. At replay depths
0, 1, 30 and 120 (65 replays each, no recoveries) and at 130 (past the
history: `Recovered`), the input the step used is the live one on every tick
and the car's yaw-rate response to the steering change is -0.0286 rad/s
(-0.0283 at 130) against -0.0286 with no replay at all. The step itself
measures the same with and without a replay before it (max 1.7 ms against
1.8 ms, dev profile); the replay's own wall time is 0.1 / 2.5 / 12.3 ms at
depth 1 / 30 / 120.

**Cost to eight seats with trailers** (`multi_tests::eight_rigs_cost_one_bounded_replay`;
every seat a car with a jointed trailer, no contact; dev profile, one run on a
loaded machine, indicative only):

| Rigs (bodies) | Physics tick | `capture_world` | History ring (121 captures) | One maximum replay (120 steps) |
|---|---|---|---|---|
| 2 (4) | 0.19 ms | 0.004 ms | ~267 kB | 12.4 ms (0.7 ticks) |
| 4 (8) | 0.21 ms | 0.005 ms | ~534 kB | 15.1 ms (0.9 ticks) |
| 8 (16) | 0.24 ms | 0.007 ms | ~1068 kB | 19.5 ms (1.2 ticks) |

The ring figure is `VehicleCapture` size (552 B) times bodies times 121; it
is an upper-bound estimate of the vehicle state, not a measured heap size.
CPU grows roughly linearly in bodies and is bounded by `MAX_REPLAY_STEPS`;
state grows linearly with bodies; the wire is unchanged by trailers in this
harness (the 8-seat `Snap` / `RemoteCommands` figures are in the 05B
section: 110.6 kB/s together without 06D cadence).

Not covered here: a replay's frame-time cost under the frame loop and
contact-heavy replays at 8 seats (NETPLAY-05I, below), and the real
transport. Evidence level: synthetic integration on the production step.

## Replay under the frame loop and under contact (NETPLAY-05I, #1932)

`multi_tests::replay_cost_in_a_frame_loop_and_under_contact` (`cargo test -p
mm2_app --lib replay_cost_in -- --nocapture`) measures one maximum replay (120
steps, every remote body corrected and replayed, no trailers) at 2, 4 and 8
seats, then at 8 seats in a contact-heavy scene (a stopped car between 
head-on drivers, a car pinned on a wall with a driver on its tail, a
three-car chain shunt; 6 touching pairs at the measured peak). Three runs, dev
profile (`[optimized + debuginfo]`), a machine at load average ~16, so
indicative only:

| Seats | Physics tick (median) | One maximum replay | Fixed steps the frame loop adds |
|---|---|---|---|
| 2 | 0.18 ms | 9.7-10.5 ms | 0 |
| 4 | 0.18 ms | 10.5-10.7 ms | 0 |
| 8 | 0.19 ms | 11.9-12.3 ms | 0 |
| 8, contact | 0.20-0.21 ms | 12.1-15.1 ms | 0 |

Replay cost is mostly a per-step overhead of the world (restore, history,
all bodies), not the solver: contact adds ~3 ms to 8 seats. The trailer rigs
of 05G (19.5 ms at 8 seats) are the costlier case, and the largest measured
overall.

**The frame loop is a model, not a window.** The test cannot open a
window, so Bevy's fixed-step catch-up is replayed in code from the measured
costs (`catch_up`: `Time<Virtual>` advances by the previous frame's wall time,
clamped at 250 ms; the accumulator pays out whole 16.7 ms ticks; the replay
lands in the first step of one frame; render cost is not included, so every
figure is a lower bound):

| Replay block | Extra steps in the next frame | Frames until back to 1 step | Worst frame |
|---|---|---|---|
| 10-12 ms | 0 | 0 | 16.7 ms |
| 20 ms (05G's 8 rigs) | 0 | 0 | 20 ms |
| 40 ms | 1 | 2 | 40 ms |
| 60 ms | 2 | 2 | 60 ms |
| 120 ms | 6 | 2 | 120 ms |
| 250 ms | 14 | 2 | 250 ms |

Findings. (1) Every measured replay, including the 05G 8-rig trailer case,
lands as one frame at most a few ms over budget with no extra step. (2)
Catch-up never spirals: with a physics tick of 0.2-1.3 ms the extra steps
cost far less than the tick they pay for, so the loop is back to one step per
frame two frames after any block up to the 250 ms clamp. (3) Local input is
unaffected, as 05G showed: the live command is written in `FixedUpdate`
after the replay for every step, including the extra ones, so the cost is a
longer frame (an input sampled once per frame is at most one frame's wall
time old on its step), not a lost or replaced command. (4) No budgeting or
spreading is needed at the measured sizes; a replay only becomes a visible
hitch (above ~35 ms) at a per-step cost five times the measured one.
Not measured: render cost in the same frame, a real window loop, the real
transport, and a release build (cheaper). Evidence level: synthetic
integration plus a catch-up model.

## Traffic copies in the predicted replay (NETPLAY-05H, #1915)

05C tested world-actor history on `mm2_vehicle`'s step alone. In the app the
mover is a `TrafficCopy`: a kinematic body that `apply_traffic` spawns and
poses from `Traffic` rows in `Update`, after the fixed step that
`record_prediction` captured in `FixedLast`. Driving that path showed a
defect: `netpredict::replay` called `replay_from` once per command against
a **two-entry scratch ring** (the corrected boundary and the previous
replayed tick). The movers' and entrants' recorded timeline lives in the
permanent ring, which that call never saw, so every step restored the copy to
the state of the step before and left it on its velocity: the copy's later
appearance, stop or turn was ignored, and an actor spawned after the compared
tick was enabled on step 1 instead of at its entry tick (a car that had
already passed the spot hit it: 18.7 m end error against the reference).

Fix: the whole window is one `replay_from_observed` call (`replay_from` plus
an `observe(world, i)` hook after each step, used to capture the tick for the
rewritten history). The permanent ring is taken out, its entry at the
compared tick is overwritten with the host-corrected boundary
(`HistoryRing::replace`, every other entry stays), and it is rewritten with
the replayed captures only when the replay succeeded; any refusal still
clears it and adopts the host state. Nothing about handling, restitution or
collision layers changed.

Tests (`tests/traffic/contact.rs`, `cargo test -p mm2_app --test app
contact:: -- --nocapture`): a `Remote` city client on the synthetic install,
the player vehicle as the own car, rows pushed into `RemoteSnaps` and landed
by the production `apply_traffic`, history recorded in `FixedLast`. A twin
client gets the same rows on the same ticks and a 0.3 m sideways nudge of the
own car at the compared tick (the host knowing something the client did not);
its state after the nudge is the assertion, and the client reconciles with
`netpredict::reconcile` and is compared with the twin 100 ticks later.

| Scene | Replay vs twin | Control |
|---|---|---|
| Oncoming copy appears in the window (entrant), changes velocity, stops 5 ticks before the end; the car hits it | 0.0000 m, `Replayed` | mover history stripped from the ring: 2.31 m |
| Copy appears after the compared tick on a spot the car already passed | 0.0000 m, `Replayed`, no body left disabled | copy present from the start of the window: 20.58 m |

Not covered (evidence level: synthetic integration, no sockets, no retail
data): a copy retired (`COPY_TTL_TICKS`) or recycled under another id inside
the window beyond `missing`/incarnation handling already in 05C; the rows
landing between the compared tick and `now` are not re-applied (the copy's
history is the client's own, so a late row corrects the copy from the next
row on, not retroactively).

Prop contact (#1931): `a_replay_meets_the_prop_as_it_was_recorded` stamps a
dormant banger placement the way the city stamp does, lands `Active` rows
through the production `worldprops::apply_props` (and `drive_prop_lerp`) and
puts the prop in the car's path at a tick inside the window (an entrant: it
turns kinematic and loses its sensor); a last row throws it clear before the
end. Replay vs twin 0.0000 m, `Replayed`; with the mover history stripped the
car ends 5.05 m away (exactly where it ends with no prop at all, so the
control cannot pass by the prop never mattering). Settled (static) props are
not movers and are not covered by this scene. Evidence level: synthetic
integration, no sockets, no retail data.

## Fixed recovery deadline (NETPLAY-05B-4, #1934)

*Designed, not original.* A contact body that disagrees with the host is
normally corrected by replay. If its pose is still disagreeing
`RECOVERY_DEADLINE_TICKS` (60 host ticks, 1 s) after the deadline started,
`reconcile` drops the recorded past and replays forward from the host state
of the own car, the contact bodies and the trailers, and reports
`RecoveryCause::DeadlineExceeded` (`recovered_deadline`,
`kind=recover_deadline`). The own car keeps its lead: it is not set to the
host's state at the ack (which would throw it back by lead x speed, 1.4-5 m
in `net_contact`), and a refused replay falls back to the named recoveries.

*What counts as disagreeing* is the pose only, past
`DEADLINE_POS_TOLERANCE` (0.05 m) or `DEADLINE_ROT_TOLERANCE` (0.02 rad).
A resting body's replay keeps a residue the replay tolerances (0.01 m,
0.001 rad, 0.02 m/s, 0.02 rad/s) call a divergence (the host's suspension and
tyre state is not on the wire): measured on `net_contact` it was 0.011 m,
0.004 rad, 0.22 m/s and 0.14 rad/s at the 99th percentile. Counted against the
deadline it made the clock run out every ~78 ticks on bodies that agreed to a
centimetre. Such a body is still replayed; it just never runs the clock.

*Why the own car was also pulled back after every replay (#2020).*
`reconcile_own` ran unordered in `FixedFirst` against avian's transform
interpolation, whose `complete_*_easing` writes last tick's end into
`Transform`. When it ran later it undid the reconcile's pose in `Transform`;
the replay's own physics steps had advanced avian's `LastPhysicsTick` past the
`Position` write, so `transform_to_position` took the stale `Transform` over
`Position`, and in a sustained contact the car snapped back to its
pre-replay pose every tick (a pinned rammer stayed a steady 1.3 m from the
host, replay gap 1.28-1.30 m). `reconcile_own` now runs after
`TransformEasingSystems::Reset` and before `UpdateStart`.

The deadline starts at the first **usable complete** authoritative state in
which a contact body diverged: usable means it was compared (not
`Skipped`), complete means every contact body and trailer in the recorded
world has its host row (a row dropped as stale-epoch counts as missing).
Neither the contact itself nor a partial snapshot starts it, so lost or late
snapshots postpone the start rather than spend the budget. Once running, a
partial snapshot neither restarts nor stops it; a complete state that agrees,
any recovery, an authority reset and a new generation end the episode. The
count is in `server_tick`, so it does not depend on how many snapshots
arrived. Only a *contact body* divergence engages it; the own car's routine
corrections never do.

Tests: `netpredict/deadline_tests.rs` (persistent divergence recovers at
exactly the deadline and keeps the lead; a velocity residue with the pose
agreeing never runs the clock; a late first snapshot starts it late; incomplete and
skipped snapshots do not start it; an agreeing state gives the next episode a
fresh budget). Evidence level: synthetic integration on the production step.
Over the wire (#1921, bb183f85): `tests/net_contact.rs` now runs the clients on
the datagram lane while the host sends `RemoteCommands`, and asserts
convergence and the response deadline on ordinary links. Integration check of
#1766 (all nine dependencies done): AC1-AC6 are advanced at the evidence levels
named in their sections (synthetic integration and loopback processes). No
real-Internet or human evidence exists; #1240 and #1249 remain the gates for
any Internet-play claim.

## Lifecycle on the lane (NETPLAY-06F, #1872)

*Evidence level: synthetic integration — real `mm2` OS processes over
loopback, dev world, no retail data. Loopback proves the code path; it
says nothing about a real LAN, router or the Internet (OWNER #1249 for
driving feel, #1240 for reachability).* Every client in these legs
negotiated the lane (`transport=lane`); none ran `--tcp-only`.

| Lifecycle | Leg | Result |
|---|---|---|
| Direct host/join, start, drive | `net_drive::two_mm2_processes_drive_one_session_over_loopback` | inputs up, snapshots down, remote seat spawned |
| Rematch (cancel, re-ready, generation 2) | `net_drive::two_mm2_processes_play_a_rematch_without_reconnecting`; lossy: `net_lane_loss` | client ends in `gen2` |
| Start, reset ask, rematch, leave under 15 % loss / 10 % dup / 10 % reorder, a datagram dropped ahead of each event | `net_lane_loss` | each applied once; seat reaped `cause=quit` |
| Late join into a running session | `net_drive::a_client_that_joins_a_running_session_is_handed_the_live_one` | joiner is handed generation 1 |
| Late join behind a lossy lane | `net_lifecycle::a_late_joiner_behind_a_lossy_lane_is_handed_the_live_session` (new) | joiner drives `gen1`, proxy impaired both ways |
| Client vanishes mid-session | `net_lifecycle::a_killed_client_is_reaped_and_a_new_one_joins_the_running_session` (new) | host `cause=lost` (2.3 ms measured on loopback: the kernel closes the sockets); a newcomer then joins generation 1 and drives |
| Client goes silent, sockets open (SIGSTOP) | `net_lifecycle::a_stopped_client_is_reaped_lost_and_a_new_one_joins_the_running_session` (#1967) | host `cause=lost` after ~8 s; a newcomer then joins generation 1 and drives |
| Host vanishes | `net_edge::a_killed_host_*` | clients close / end |

### Host-side liveness for a silent client (#1967)

A client that stops speaking while its sockets stay open (SIGSTOP, a
black-holed route with no FIN/RST) fails no write, so the host used to
keep its seat forever. The host now keeps, per lane seat, when the last
authenticated datagram arrived (`LaneSeat::heard`, set after the token,
address, replay and generation checks, so a forger cannot keep a seat
alive). On the keepalive tick, a seat silent past
`HostConfig::liveness_timeout` (`HOST_LIVENESS_TIMEOUT`, 8 s, designed not
original) is removed as `LeaveCause::Lost` through `remove_player`, which
releases the lane seat and token and rebroadcasts the roster; a running
session continues with the remaining seats. A client's lane send loop
sends a `Probe` (which the host authenticates and echoes) whenever nothing
else left for `KEEPALIVE_INTERVAL`, so an idle lobby seat or a paused
session is never silent. A TCP-only join has no lane and keeps the
write-failure rule. The client-side `LIVENESS_TIMEOUT` (5 s) is unchanged:
it is the client's read timeout on host keepalives.
Tests: `a_silent_lane_is_reaped_as_lost_and_an_idle_one_is_not`
(`lane_lobby_tests`, a black-holed uplink) and
`net_lifecycle::a_stopped_client_is_reaped_lost_and_a_new_one_joins_the_running_session`
(SIGSTOP of a real `mm2 --join`).

### #1767 acceptance mapping

Evidence for each criterion lives in the sections above and the named
tests; none of it is WAN evidence.

1. Datagram drop/reorder/duplicate both directions: `UdpImpairProxy`
   (06B) and `net_lane_loss`.
2. Bulk/reliable delay does not stop movement; budgets measured: 06C
   "Measured" and 06D "Measured" sections.
3. Wrong seat/generation/token rejected, stale bases and partial
   assemblies safe: 06A counters, 06C.
4. Start, reset, leave, rematch survive a lost significant message and
   apply once: `net_lane_loss` and `mm2_net` `lane_lobby_tests` /
   `lane_reliable_tests`. C&R and impact outcomes: 06E-3 section.
5. Direct join, late join, disconnect on loopback: the table above.
   Endpoint/firewall requirements: `docs/multiplayer-reachability.md`.
   The host-side silent-client timeout is above (#1967). **Open:** all real
   machine validation (OWNER #1240, #1249).

## Host-authored event identities and the presentation ledger (NETPLAY-08A, #1974)

**Identity.** A consequential event is named by what the authority
decided: `EventIdentity { generation, epoch, seat, tick, ordinal }`
(`mm2_game::presentation`). `epoch` is the seat's reset epoch when the
event happened, `tick` the fixed session tick, `ordinal` the position among
that seat's events of the tick in emission order (strongest first, the
order `collect_impacts` emits). `SnapImpact` carries `epoch` and `ordinal`
next to `seat`/`tick` (`PROTOCOL_VERSION` 38); `OrdinalCounter` numbers
both the host's rows and a client's local contacts the same way.

**Ledger.** `PresentationLedger` is bounded (`LEDGER_CAPACITY` 256
entries, `LEDGER_HORIZON_TICKS` 600) and holds, per presented event, the
identity each stream gave it. `arrive(Local | Authoritative, ..)` returns
`Present`, `Suppress` or `Stale`:

| arrival order | outcome |
| --- | --- |
| local, then row | local `Present`, row pairs with it and `Suppress`es |
| row, then local | row `Present`, the late local twin `Suppress`es |
| replay re-detects a tick (same identity again, either stream) | `Suppress` (a repeat) |
| row of an epoch older than the seat's current one | `Stale`, never presented |

A speculative local contact of a replaced epoch is cancelled on the seat's
reset (`seat_reset`), so it cannot absorb a post-reset row. Pairing is **one to
one** on `(generation, epoch, seat)`, nearest tick within
`PAIR_TICK_TOLERANCE` (30 ticks, designed, covers prediction lead plus
reconciliation error), then nearest ordinal, then distance. Each pairing
consumes both sides, which replaces the 1.5 s / 3 m matcher of #1248/#1532:
two distinct hits half a metre apart are two presentations. Residual risk,
accepted: if a row is lost for good, the next row of the same seat inside
the tolerance may pair with the orphaned speculative contact — rows ride the
reliable channel (06E-3) so this needs a dead channel.

**Wiring (behind the NETPLAY-02 selector).** With `NetplayTimeline` on,
`ImpactPresentation` (`mm2_app::impact_presentation`) replaces the old
`recent` matcher: `apply_snapshots` syncs the local seats and epochs and
classifies local contacts, `drain_pending_impacts` asks `admit_row` for
own-seat rows, and the local-stream consumers (`impact_voices`,
`emit_sparks`, `apply_texel_damage`) ask `admit_local`, which decides an event
once and memoises it so the consumers agree in any order. With the selector
off the legacy matcher runs unchanged.

**Replay.** Replayed physics must not emit packets, score or a second
effect. The ledger makes the effect half safe: a replay that re-detects a
contact presents nothing (`begin_replay_tick` restarts the ordinal count so
the identity repeats). Today no replay path emits `ImpactEvent`s at all;
`begin_replay_tick` is the hook a replay that does must call. Packets and
score are N08-B/N08-C.

**Lifecycle per effect** (own seat of a predicted client):

- *Damage* is state: the authority's `SnapEntry.damage` fraction replaces the
  local value; `apply_impact_damage` drops on a non-authority. It never
  rides the ledger and cannot double.
- *Sound* (`impact_voices`) and *sparks* (`emit_sparks`): one spawn per
  ledger `Present`, from the local sim if it came first, from the row if it
  did. A `Suppress`/`Stale` spawns nothing. Spawned voices and streaks are
  not retracted when a reset later cancels the speculative entry.
- *Texel deformation* (`apply_texel_damage` / `apply_remote_texels`): as
  sound; a repair byte still erases rows at or before its tick.
- *Fragments* (`detach_breaks`): authority-only; the client mirrors the
  `breaks` bitmask state. Not ledger-gated.
- *Scrape loops* (`ScrapeWatch`): per-pair hold over a spawned clip; a
  suppressed event never reaches it, so it cannot hold a pair for a clip
  that did not play.
- *Camera shake*: no impact-driven camera shake exists in the code today;
  nothing to gate. A future one must ask the ledger like sound.

Evidence level: synthetic integration. `mm2_game::presentation` unit tests
and `mm2_app::impact_presentation` tests assert production identities for
local-first, authoritative-first, replay-first and delayed older-epoch
arrival and for two nearby distinct impacts;
`net_app::the_staged_ledger_presents_each_own_seat_hit_once_in_either_order`
runs it over the real transport. No retail data, no rendered capture.
**Open:** impact-driven camera shake (none exists), Cops & Robbers re-award
and role swaps (N08-B), tick-correct modifiers (N08-C).

## Tick-correct force-affecting modifiers (NETPLAY-08C, #1976)

A replay restores the *recorded* capture of the compared tick, so the engine
impairment, mass and inertia in it are the ones the client predicted then.
The authority's word about them can arrive later and elsewhere (the damage
byte, the Cops & Robbers frame); a replay that kept the recorded values, or
the live world's of today, would rewrite the past with wrong forces.

`netpredict::modifiers` turns that word into `Authored` values;
`OwnPrediction::authored` holds them for one comparison and
`modifier_changes` lists the bodies whose recorded `BodyModifiers` differ.
Such a body is replayed even when every pose still agrees (the forces differ
from the compared tick on), and the corrected capture is stamped with the
authoritative value before `restore_world`, so the rewritten history carries
one consistent past. `ReplayStep::modifiers` / `apply_modifiers`
(`mm2_vehicle`) are the matching per-step primitive.

| Modifier | Source | Tick it takes effect |
| --- | --- | --- |
| Engine impairment (damage) | the snapshot's damage byte, through `damage::impairment_factor` (the same rule `sync_impairment` applies live) | the snapshot's host tick = the compared ack: exact |
| Mass, inertia (gold) | `CnrReplica` through `cnr::authored_load_kg`, applied to the unloaded base (`cnr::unloaded_body`) | the compared ack, the oldest rewritable tick. The frame carries the match clock, not the snapshot tick, so the pickup tick is not on the wire: a documented bounded refresh, lag = replica latency |
| Trailer joint | the capture's own `hitches` | the tick's recorded hitch; the wire carries no joint change, so nothing is overridden |

No impulse is ever reapplied: damage and load are state, written once into
the capture; a contact's effect stays with the presentation ledger (08A) and
the replayed physics, so a speculative false contact's obsolete force
disappears with the rewritten past and a confirmed late one is committed by
the replay once. A non-finite or non-physical modifier is refused
(`BodyModifiers::is_valid`), never applied.

Evidence level: synthetic integration (`netpredict::tests::a_late_impairment_*`,
`a_speculative_impairment_*`, `a_late_gold_load_*`, `modifiers::tests`): the
corrected client equals a twin world that had the damage at the historical
tick. **Open:** a snapshot whose ack was already compared (`Skip::Seen`)
cannot carry a new damage byte into the past; packet/fragment-burst and
both-arrival-order tests over the lossy transport, and a retail-data run.

## Own-car correction smoothing (NETPLAY-07 slice 1, #1768)

`mm2_app::netvisual`, behind the `NetplayTimeline` selector. When
`reconcile_own` replays (or recovers) the own car, the body shift
`before -> after` at the same simulation instant is absorbed into a
`CorrectionOffset` (world translation + shortest-arc rotation about the body
origin) so the *drawn* pose stays where it was, then decays with wall-clock
time (`exp(-dt/0.12 s)`, frame-rate independent, cleared below 1 mm / 0.1
degree). The offset is bounded (1.5 m, 0.35 rad); a shift over 6 m, an
authority reset, a generation change or a non-finite pose clears it so the
view never slides across the city or from a previous life.

- Never touches `Position`/`Rotation`/`Transform` of the body: it edits the
  `GlobalTransform` subtree of the car after propagation (`apply_offsets`) and
  restores it before the next propagation (`undo_offsets`), so Avian's
  transform sync cannot read it back and a static subtree cannot accumulate.
  Body state is therefore identical with smoothing on or off by construction.
- The chase camera (`camera::chase_follow`) targets the drawn pose; a cockpit
  camera is a child and moves with the subtree. No input or steering is
  filtered and nothing is buffered.
- The numbers are designed values (no original network correction exists).

Evidence (synthetic, `cargo test -p mm2_app --lib netvisual`): drawn pose
preserved at the correction and converging; same settle time at 30/60/144 fps
(within 50 ms, under 1 s); 2 cm alternating corrections never exceed 4 cm;
bounds and teleport clear; yaw across +-pi takes the 0.1 rad arc; alternating
yaw settles; non-finite clears; the PostUpdate systems leave `Transform`
unchanged and do not accumulate over repeated frames.

### Slice 2 (#2007): trailers, audit, overlay

- The own trailer is a separate real body, so `reconcile_own` records the
  own trailers' poses around the reconcile and absorbs each shift into the
  trailer's own `CorrectionOffset` (`netvisual::absorb_rig`); a reset or
  generation change clears car and trailer together. Both offsets decay with
  the same constant, so the drawn hitch gap stays the one that was drawn.
- Remote contact bodies (`PredictedRemoteBody`, `PredictedRemoteTrailer`)
  carry no mesh: what is drawn for a remote car is the kinematic replica
  following the host-timestamped snapshot buffer (07B), so a contact-body correction has no
  rendered pose to offset. Not applicable, not skipped.
- Audit: the minimap, the Cops & Robbers markers and the chase/cockpit
  camera (and so the audio listener) read `GlobalTransform` or the camera
  target, i.e. the drawn pose; physics queries read `Position`. The two
  differ by at most `MAX_DISTANCE` for ~0.5 s after a correction.
  Spatial audio emitters on a car's subtree follow the drawn pose only if
  bevy_audio's update runs after `apply_offsets`; that ordering is not
  asserted here.
- Developer overlay: `MM2_NETVISUAL_OVERLAY=1` (with the timeline selector)
  draws the physics pose (axes), the rendered pose (amber sphere) and a line
  between them, and prints `event=netvisual_offset ... dist= angle=
  snapshot_tick=` once per absorbed correction (`CorrectionStamp`).
- Evidence: synthetic unit tests (`netvisual::tests`: trailer pose
  preservation, remote trailer copies excluded, reset/jump/reverse, overlay
  reading). No rendered capture or lossy-transport run was made.

Open (not claimed): the audio emitter ordering above; rendered captures and
30/60/144 FPS
route runs (AC1, AC3); input-to-render latency measurement.

### Frame-rate route evidence (NETPLAY-07D, #2008)

`cargo test -p mm2_app --lib frame_rate -- --nocapture` (`netpredict/frame_rate_tests.rs`).
A host and a client car on the production step run `--net-route`'s `Route`
(slalom and accel); the host car takes an unpredicted 1.2 m/s sideways kick
and 0.5 rad/s spin at ticks 90/210/330; the client reconciles against host
state 6 ticks late (3 ticks one way) with the real `reconcile` and feeds each
replay into a `CorrectionOffset` that decays on a render clock of its own.

| route, render rate | corrections | worst body shift | slowest settle | worst view slip |
|---|---|---|---|---|
| slalom 30 fps | 3 | 0.119 m | 0.567 s | 0.029 m |
| slalom 60 fps | 3 | 0.119 m | 0.567 s | 0.015 m |
| slalom 144 fps | 3 | 0.119 m | 0.560 s | 0.007 m |
| accel 30 / 60 / 144 fps | 3 | 0.095 m | 0.533 / 0.533 / 0.532 s | 0.023 / 0.012 / 0.005 m |
| slalom, no smoothing (any rate) | 3 | 0.119 m | - | 0.119 m |

"View slip" is the largest displacement the drawn pose takes that the car's
motion does not explain: one frame's decay with smoothing, the whole shift
without. The body (`final_error` 0.000 m against the host's record) is
bit-identical with smoothing on or off. Two seats at unequal caps (30 and 144
fps) settle within 0.15 s of each other and take identical corrections.

Input timing: the client consumes command seq `k` on its tick `k` online and
offline alike (the brake response onset is tick 481 in both, with a
correction every tick online), so the network adds no input latency to the
own car. Under the frame model (input sampled once per frame, ticks due by
the frame run before it submits) the press-to-step and press-to-render
-submission times are 17.0 ms mean / 33.3 ms worst at 30 fps, 8.3 / 16.6 ms at
60 fps and 8.7 / 20.7 ms at 144 fps; they coincide because the step and the
submission share a frame.

Evidence level: synthetic integration. Frame times are the model's, not
measured from a window system: driver/display latency is not included. **Open:**
rendered before/after captures from both non-host perspectives (AC1) and the
human comfort judgement stay with OWNER #1249; AC2 (a rendered
physics-versus-drawn overlay) is not produced.

Open (not claimed): trailers/wheels of the
corrected car beyond the carried subtree (trailer is a separate body); the
physics-versus-rendered developer overlay; rendered captures.

## Host-timestamped snapshot buffer (NETPLAY-07B, #2006)

`mm2_app::netinterp::SnapBuffer`, on the collider-less visual copy of a remote
human (the `Predicted` copy whose contact body is a separate dynamic entity).
`apply_snap_frame` pushes each seat row keyed by the host tick time
(`tick / RACE_TICK_HZ`); `drive_remote_lerp` samples it instead of the
arrival-gap `RemoteLerp` blend, so jitter changes buffer occupancy, not the
speed of the drawn body.

- Clock: `base` = floor of `arrival - host_time` (drops at once, creeps up
  1%); render time = `now - base - delay`.
- Delay: `0.05 s + 3 * jitter` (RFC 3550 mean deviation of the arrival
  delay), capped at 0.25 s, slewed at 0.1 s/s (<= 10% playback-speed change).
- Sampling: interpolate between the bracketing samples (short-arc slerp);
  past the newest, extrapolate along the sample velocity for at most 0.1 s
  then hold; before the oldest, hold. At most 32 samples; non-finite and
  non-increasing samples are refused (`SnapBuffer::refused`). A teleport
  (epoch change or > `CORRECTION_SNAP_DIST`) clears the history.
- Never writes a colliding body: the copy has no collider, and the close
  predicted body keeps the NETPLAY-05 policy untouched. Numbers are designed.

Evidence (synthetic, `cargo test -p mm2_app --lib netinterp`): steady stream
plays at the host speed; +-15 ms jitter keeps speed within 3 m/s of 20 m/s;
jitter raises the target delay, a clean stream does not; delay slew bound;
extrapolation bound; capacity and refusal; clear on teleport; short-arc
rotation.

Open: kinematic copies that *are* the collider (host-planned bots, the
non-timeline path) still use the arrival-gap blend, since smoothing their
`Position` would write the colliding body; they need a separate render pose.
Trailer copies keep the arrival-gap blend. No rendered or lossy-transport
run (AC3/AC5 jitter matrix) yet.

## NETPLAY-10A: nonblocking bounded I/O handoff

No simulation or render system waits on a socket or a contended socket
mutex, in either direction:

- **Egress.** Every post-handshake control-stream write goes through a
  `PeerWriter`: the caller queues an encoded frame (bounded,
  `PEER_QUEUE_CAP`) and a per-peer thread writes it, so one peer that stops
  reading cannot delay another. Replaceable state (`Keepalive`, `Session`,
  `Roster`) is coalesced in place (`send_replaceable`); `Start`, `Cancel`,
  `Leave` and everything else is never merged, and a full queue is an
  error, not a drop. `Snap` is not coalesced on TCP because its impact rows
  are events; moving world state off TCP is NETPLAY-10C.
- **Disconnect policy.** A persistently non-reading peer is removed `Lost`
  when its queue fills or when its oldest unwritten frame is older than
  `HostConfig::peer_stall_limit` (`PEER_STALL_LIMIT`, 10 s). A coalesced
  frame keeps its predecessor's age, so fresh state cannot hide a stall.
- **Ingress.** The host loop's wake-up queue and its event queue are
  bounded. A game-thread `HostCtl` call on a full loop queue returns
  `WouldBlock`; reader threads wait (backing a flooding peer up onto its own
  socket). A consumer that leaves `EVENT_QUEUE_CAP` events undrained ends the
  lobby rather than let the queue grow or drop an event.
- **Telemetry.** The pending-I/O queue (`Host::io_queue`,
  `ClientCtl::io_queue`: length, `oldest_age`, `max_wait`, `coalesced`,
  `refused`) is reported separately from the lane's queues
  (`lane_queue`) and from retained history (reliable-channel windows, the
  command ring). A seat reaped as stalled logs its I/O-queue stats on stderr.

Tests: `io_backpressure_tests` (non-reading peer among healthy ones over
real sockets) and the `conn` unit tests. The reliable Start/Cancel/reset/
Leave paths are covered under the seeded lossy proxy by
`lane_reliable_tests` and `lane_lobby_tests`.

## NETPLAY-10B: congestion controller and aggregate uplink budget

Code: `mm2_net::congest` (pure, clock-injected: `PeerController`, `Uplink`,
`Adapt`, `wire_bytes`, `Quantiles`), applied per seat in `mm2_net::rt`
(`SeatOut`, `divide_uplink`). Mechanism after RFC 8085: reduce on congestion
evidence at most once per hold (3.1), raise additively and only while the
rate is in use or the traffic is shed to fit it (3.1.6, 3.2), keep a floor.
All numbers are *designed*, none recovered from the retail game.

### Rules

- **Wire bytes.** Every budget counts `payload + 32 B lane header + 28 B
  IP/UDP` per datagram. Redundant commands, ACKs, reliable retransmissions and
  fragments are datagrams like any other and pay the same.
- **Per peer.** A `PeerController` starts at 128 KiB/s (1 Mbit/s) and floors
  at 12 KiB/s. Congestion evidence: a reliable-channel retransmission; a
  movement datagram discarded unsent (older than its 100 ms bound); the
  oldest queued movement datagram >= 66 ms (two snapshot periods); a smoothed
  RTT more than 50 ms above the lowest seen in 10 s (a standing queue before
  any loss); a snapshot passed over by a newer acknowledgement; the last
  acknowledgement >= 500 ms old while datagrams flow. Bulk expiry is *not*
  evidence: bulk is meant to yield. On evidence the rate is cut to 70 %, at
  most once per 250 ms, and the clean wait before the next step doubles (up
  to 4 s; reset after 10 s clean). After a clean wait the rate rises: x1.25
  while below the last rate that worked (recovery), then +4 KiB/s, never
  past the ceiling. It is raised only if the interval used at least half the
  rate, or the traffic is already degraded (a shed level's demand is below
  its rate by design and only a higher rate lets it rise; a failed probe is
  cut again). Congestion never raises a rate and never queues old snapshots.
- **Aggregate.** `HostConfig::uplink_bytes_per_sec` (default 5 Mbit/s,
  retunable at run time with `Host::set_uplink_cap`) is a token bucket (64 KiB
  burst) every datagram of every seat must pass, and is divided max-min
  fairly: each send tick a seat's bucket refills at
  `max(floor, min(controller rate, fair share))`, a seat asking for less
  than an equal share gets what it asks and the rest is split among the
  others. One constrained peer therefore costs the healthy ones nothing,
  and a cap cut is divided, not queued.
- **Order and bounds.** Movement (snapshots, `Data`, ACKs, reliable) goes
  first and needs positive credit; bulk needs its datagram plus a 2 KiB
  reserve in credit, so catch-up never makes the next snapshot wait. Queues
  hold at most 32 datagrams and 100 ms (movement) / 250 ms (bulk) of age: a
  datagram that cannot be sent in time is discarded, never replaced by an
  older one. The unacknowledged-tick log is capped at 64; the wait samples
  at 4096.
- **How traffic adapts** (`Adapt::for_rate`, rate vs the 128 KiB/s ceiling):

  | rate | snapshot cadence | bulk (low priority) | relevance radius |
  |---|---|---|---|
  | >= 1/2 | 30 Hz | sent | 100 % |
  | >= 1/4 | 30 Hz | shed | 100 % |
  | >= 1/8 | 15 Hz | shed | 70 % |
  | below | 10 Hz | shed | 40 % |

  Coalescing: only the newest snapshot of a seat matters, so a seat that
  cannot keep up is sent every n-th snapshot, not a backlog; replaceable
  control state (`Keepalive`, `Session`, `Roster`) is already coalesced in
  place (10A).
  `Host::peer_adapt(seat)` exposes the level to the producer.
- **Reports.** `Host::lane_pace(seat)` adds `wire_sent`, `rate`, `ceiling`,
  `share`, `decreases`, `movement_deferred`, `uplink_denied`, `adapt` and the
  movement-queue wait p50/p95/p99/max; `Host::uplink()` the cap and the wire
  bytes admitted.

### Measured (`cargo test -p mm2_net congest_sim -- --nocapture`)

Virtual-clock simulation of the production send side against modelled
drop-tail links (150 ms buffer, 20 ms one way), 30 Hz snapshots of 900 B plus
four 1.2 kB catch-up fragments, 120 B reliable every 100 ms, client acks over
256 kbit/s. Host plus 7 peers, 5 Mbit/s host cap, 1 Mbit/s peer links.

| scenario | healthy-peer movement wait p95 / max | state age p50 / p95 | host wire | notes |
|---|---|---|---|---|
| steady | 10-12 ms / <= 33 ms | 33 / 41 ms | 624 of 625 kB/s | Jain fairness 1.000, no loss |
| 5 -> 2 -> 5 Mbit/s | 0 / 0 ms at 2 Mbit | 28 / 35 ms at 2 Mbit | 245 of 250 kB/s at 2 | catch-up yields, movement keeps full cadence |
| one peer on 160 kbit/s | 0 / 0 ms | 31 / 37 ms (healthy) | 612 kB/s | slow peer cut to 15 kB/s, 10 Hz, healthy peers 101 kB/s each |
| all links 1 Mbit -> 200 kbit -> 1 Mbit | 0 ms at the dip | p95 54 ms, p99 150 ms at the dip | 64 kB/s at the dip | fresh state <= 100 ms on every peer within 1 s of widening, queues in bounds within 2 s, full cadence after 3.25 s |

Real sockets: `the_aggregate_uplink_cap_holds_across_two_real_lanes` (two lane
clients, 40 kB/s cap, ~108 kB/s offered) admits 159 kB in 3.7 s against 223 kB
allowed, every admitted byte attributed to a seat, movement wait <= 100 ms.

### Evidence and open

Evidence level: code checks and synthetic integration (virtual clock and one
loopback test). No bandwidth-limited real-socket qualification, no WAN, no
retail or rendered level. Retransmission rate is modelled for the reliable
datagram only; the real channel's retransmits feed the controller in `rt.rs`
but are not exercised under loss here.

Open, filed as tasks: the client does not send `lane_ack` yet, so the RTT and
skipped-snapshot signals are idle in the app (queue age, expiry, retransmits
and the aggregate cap are live); the producer (`publish_snapshots`) still
broadcasts one `Snap` and does not yet honour `peer_adapt` (cadence divisor,
relevance, bulk marking); the 8-peer loopback qualification with a
bandwidth-limited relay is NETPLAY-10E.

## Partial same-tick Snap groups (NETPLAY-10C-1, #2024)

`mm2_net` delivers each completed fragment group as its own `Message::Snap`,
so a tick split into an owner/interaction group and a world group reaches the
app as two `Snap`s with the same `(generation, tick)` and disjoint seats.
`RemoteSnaps` used to keep one `latest` frame behind one frame-wide
watermark, which dropped the second group as stale. It now judges freshness
per row:

- **Freshness is per seat.** `seat_marks` holds the newest `(generation,
  tick)` *applied* for each seat, `trailer_marks` the same per towing seat;
  a row is judged against that and against the rows still staged for the
  seat. (Marks move at apply, after the session's generation gate, so a
  dropped foreign-generation frame never poisons a seat.) A row at or behind
  either is stale; a `Snap` whose every row
  is stale is one counted `snaps_staled` drop (a duplicate of a group, or a
  reordered straggler); a partly stale one keeps its fresh rows. Both maps
  and the per-row blend-arrival map are bounded (`MAX_FRESHNESS_KEYS`, 4 x
  `MAX_PLAYERS`, oldest retired first). A frame with no seat or trailer rows
  keeps the old frame watermark.
- **Groups apply independently.** Staged groups are a list in ascending
  tick; a group of a tick already staged merges into it, and a newer row for
  a seat strips that seat from an older staged group, so a seat sits in at
  most one staged group and the list is bounded by the seat count.
  `apply_snapshots` applies every staged group each run, oldest first. A
  group never waits for its siblings, and a newer tick overtaking a missing
  group of an older one leaves that group's late arrival usable for the seats
  the newer tick did not name. `applied` is the newest tick any group
  reached.
- **Absent is not left.** Nothing derives a departure from a missing entry:
  apply only touches the seats a group names, and roster removal comes from
  the reliable roster messages. Blend intervals are per seat (the gap since
  that seat's own previous row), so a group carrying only some seats blends
  them over their own period.
- **Race row, impacts, trailers** keep their rules: race on
  `race_order_key`, impacts on `(generation, seat, id)`, trailers on
  `trailer_marks`.
- **Own-seat correction.** The assertion (`OwnStage::assertion`) is staged
  from the own seat's row only, so the owner's group alone is a usable
  correction and a group without it never replaces or clears it. The remote
  seats' states and trailers (`remotes`, `trailers`, `remotes_at`) are
  staged only by the group that carries the own row, and travel only with
  the assertion of that same `(generation, tick)` as before; a split tick
  therefore replays without the other group's seats, exactly as if they were
  absent from a snapshot. The own rig's trailer corrects only with the
  group that carries the own seat, and a remote trailer's owner-reset signal
  needs its owner's row in the same group.
- **What the ack acknowledges.** `input_ack` and `input_depth` ride the own
  seat's row, so the lane report (`ClientLead::note_snap`) and the
  assertion's `ack` are those of the owner's group at the owner's tick; a
  world group carries none, and the owner's ack never waits for it. A lost
  world group costs only the remote seats' freshness.

A single full `Snap` (today's host output) is one group naming every seat and
behaves as before. Tests: `netdrive::tests` (`groups_of_one_tick_stage_in_either_order`,
`a_duplicated_group_is_stale_and_a_missing_one_blocks_nothing`,
`a_newer_tick_overtakes_a_missing_group_of_an_older_one`,
`trailer_rows_are_judged_per_owner`, `freshness_state_is_bounded`,
`apply_snapshots_applies_each_group_independently`,
`the_owner_group_stages_its_assertion_alone_and_the_world_group_leaves_it`)
and `net_partial_snaps` (a real lane through the seeded `UdpImpairProxy`:
15 % loss, 20 % duplication, 30 % reordering; the outcome equals an
independent model of the freshness rule). Evidence level: synthetic
integration.


The standing-queue verdict (smoothed RTT above the baseline plus slack) only counts once it has held for 500 ms (`STANDING_QUEUE_HOLD`): a scheduler stall on a loaded machine inflates a few samples and must not cut the rate and starve corrections.
