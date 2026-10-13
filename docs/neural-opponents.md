# Neural opponent drivers

Race opponents are driven by a small neural network: a policy that
looks at the car's line, the world around it and the vehicles nearby
and outputs steering and pedals. Everything else about an opponent —
which route it takes, how it is credited with progress, what happens
when it is hopelessly stuck — stays conventional code around it.

This page describes how the pieces fit, how a policy is trained and how
to judge one. The original game's own controller is documented in
[research/opponent-ai.md](research/opponent-ai.md); this one is
designed, not recovered.

## The pieces

| Where | What |
| --- | --- |
| `mm2_app::neural::guide` | The line to follow: a polyline with arc lengths, tracked by a windowed search so a route that revisits a street cannot confuse it |
| `mm2_app::neural::observe` | The observation layout and the action mapping |
| `mm2_app::neural::policy` | The network: a `tanh` MLP evaluated on the CPU from a `.mm2p` file |
| `mm2_app::neural::unstick` | Backing a stalled car out of a dead end, around the policy |
| `mm2_app::neural::drive` | The policies as a session resource, and per-car state |
| `mm2_app::neural::env` | The training environment |
| `crates/mm2_app/examples/mm2_gym.rs` | The environment as a process: serve, benchmark, evaluate, audit |
| `training/` | The PPO trainer (Python, needed only to train) |
| `assets/ai/opponent_{amateur,professional}.mm2p` | The shipped policies |

### What the policy is given

A *guide* and nothing else about where to go. For a race opponent the
guide is its authored `.opp` route re-pathed through the road graph
(`opponents::driving_route`). Nothing in the policy knows that: a guide
can equally be a nav-graph path to a Cops & Robbers objective, or the
straight segment to a target. Route information is how a caller says
where to drive, not something the driver needs to exist.

One observation is 126 floats in the car's own heading frame, scaled
to about unit size (`observe.rs` is the reference):

| Block | Size | Contents |
| --- | ---: | --- |
| car | 11 | size, mass, power, top speed, steering lock, grip, the driver's throttle ceiling |
| motion | 15 | velocity, spin, tilt, steering angle, wheel contact, reverse, last action |
| guide | 34 | offset and heading against the line, then the line at ten distances from 5 m to 170 m |
| rays | 24 | free distance to the world: three down the corridor the car is about to sweep, then a fan, close-set ahead and wide to the sides and rear |
| floor | 6 | where the ground ahead is against where level ground would be — a quay, a crest, a raised bridge leaf |
| neighbours | 36 | the four nearest other vehicles: position, relative velocity, heading, size |

Rays ignore vehicles (those are the neighbour block), are carried along
any ground they meet so a rising road is not a wall, and treat deadly
water as open space.

The three outputs are steering, a pedal axis (throttle positive, brake
negative, capped at the authored `maxThrottle`) and the handbrake. The
policy decides at 20 Hz — every third physics step — and holds its
action in between.

### What stays conventional

- **Routes.** Authored, as the original's are.
- **Progress and finishing.** An opponent finishes on its own course
  and the finish line, not on checkpoint cylinders its line may miss —
  the original's rule (`0x53d6c0`). `RouteGateLine` binds gates and the
  finish to arc positions on the driven line.
- **Backing up.** A car that has held the throttle for 1.5 s without
  moving a metre is reversed for about a second, nose swung toward its
  line — the original's stuck remedy. A driver rewarded for progress
  almost never discovers reversing on its own.
- **Re-anchoring.** The bounded last resort: 15 s inside one 8 m bubble
  teleports the car back onto its route, disclosed in the race record
  (`opp_rec=`).

## Training

The policy is trained by reinforcement learning (PPO) in
`neural::env::DrivingEnv`: one race event's world — city collision and
props through the production importer, the event's barricades, parked
cars and drawbridges — with the production vehicle simulation and
nothing else. No audio, HUD, pedestrians or rendering; one worker steps
about 2,000–5,000 decisions a second on one core.

Every car in the environment is an agent on one of the event's real
opponent lines. An agent that finishes, wrecks or stalls is replaced
at once somewhere else, so the field never empties. Around the agents
are *drones* — parked in the road or driving at traffic pace — and
every so often a group of agents is restarted together from the race's
real grid, with a drone standing in for the player.

The reward is metres of progress along the guide, less the speed lost
in impacts, less a little for sawing at the wheel, with a penalty for an
episode that ends stalled, off its route, upended or in the water.

### Running it

```sh
cargo build --release -p mm2_app --example mm2_gym
cd training
export MM2_RETAIL=<original install>

# Start, or resume, a run. One mm2_gym process per worker.
uv run python -m mm2rl.train --run pro --workers 18

# Stop it: saves a checkpoint, loses at most the rollout in flight.
uv run python -m mm2rl.ctl stop pro

# How it is doing.
uv run python -m mm2rl.ctl status
```

A run lives in `training/runs/<name>/` (not checked in): `latest.pt`
to resume from, `latest.mm2p` the exported policy, numbered snapshots
every 25 M steps, `log.jsonl`, and `lines.json` — per opponent line,
how the last ten minutes' episodes ended and where on the line the
failures were. Resuming takes any `--workers` count; the run keeps its
own copy of the environment binary, so rebuilding the repository does
not disturb it.

Changing the observation layout (`OBS_VERSION`) or what the
environment does to the car invalidates trained weights: retrain.

## Judging a policy

Three instruments, cheapest first.

```sh
# 1. Every field of a city from its grid, in the lean environment.
target/release/examples/mm2_gym --mm2-path $MM2_RETAIL --city sf \
    eval --policy training/runs/pro/latest.mm2p

# 2. Where a line cannot be driven as drawn (through a wall, over a
#    void, into water) — route defects, not driving mistakes.
target/release/examples/mm2_gym --mm2-path $MM2_RETAIL --city london audit

# 3. Every race in the real game, player parked.
scripts/opponent_sweep.sh target/release/mm2 out/ 4
```

The sweep is the one that counts: `opp=<finished>/<fielded>`,
`opp_rec=` re-anchors, and per car in `opps=` its checkpoints, finish
time, back-ups (`e`), re-anchors (`r`) and longest stall (`w`). Try a
policy that is not yet shipped with `mm2 --opponent-policy <file>`.
