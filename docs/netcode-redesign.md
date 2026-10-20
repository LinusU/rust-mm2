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
