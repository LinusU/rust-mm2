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
| `crates/mm2_app/policies/opponent_{amateur,professional}.mm2p` | The shipped policies, compiled into the binary; a mod overrides them at `ai/opponent_*.mm2p` |

### What the policy is given

A *guide* and nothing else about where to go. For a race opponent the
guide is its authored `.opp` route re-pathed through the road graph
(`opponents::driving_route`). Nothing in the policy knows that: a guide
can equally be a nav-graph path to a Cops & Robbers objective, or the
straight segment to a target. Route information is how a caller says
where to drive, not something the driver needs to exist.

A race guide is built in three steps, and much of what once looked like
bad driving was one of them going wrong:

1. **The roads.** Two consecutive rows of the record name two
   intersections; the leg is the road they share, whichever way its
   traffic flows (`NavGraph::race_leg`). Where the record skips an
   intersection, the chain of roads between the two, along the race's
   checkpoints (`NavGraph::race_path`). Only then the directed router,
   and last the authored straight line.
2. **The line.** Down the lanes that run the leg's way, heights from
   the lanes, the few-metre jink at each junction smoothed out
   (`Guide::from_route_on`).
3. **The world.** Once the collision world exists, every point more
   than a metre off the ground under it is moved onto it
   (`Guide::regrounded`) — a course crosses parks and terraces where
   there are no lanes to take a height from — and the line is moved
   sideways off whatever stands within a car's width of it and cannot
   be pushed aside (`Guide::cleared`): a parked car in the kerb lane, a
   traffic light on a corner the line cuts.

The third step costs about 10 ms for an 8 km line, and the game does
one opponent's a step during the countdown. `mm2_gym audit` walks every
line and lists where it still cannot be driven as drawn.

One observation is 132 floats in the car's own heading frame, scaled
to about unit size (`observe.rs` is the reference):

| Block | Size | Contents |
| --- | ---: | --- |
| car | 11 | size, mass, power, top speed, steering lock, grip, the driver's throttle ceiling |
| motion | 15 | velocity, spin, tilt, steering angle, wheel contact, reverse, last action |
| guide | 40 | offset and heading against the line, then the line at twelve distances from 5 m to 340 m — braking distance for the fastest cars |
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

### What it costs to run

One decision is 45 µs of network on one core — 100,000 multiply-adds —
and some thirty short rays into the collision world. A car decides
twenty times a second, so a field of eight spends under 1 % of a core
on being driven. Nothing here needs a GPU, a thread of its own or an
inference library: the weights are a 400 kB file compiled into the
binary and the forward pass is forty lines of Rust.

### What stays conventional

- **Routes.** Authored, as the original's are.
- **Progress and finishing.** An opponent finishes on its own course
  and the finish line, not on checkpoint cylinders its line may miss —
  the original's rule (`0x53d6c0`). `RouteGateLine` binds gates and the
  finish to arc positions on the driven line.
- **Backing up.** A car that has held the throttle for 1.5 s without
  moving a metre is reversed for about a second, nose swung toward its
  line — the original's stuck remedy. A driver rewarded for progress
  almost never discovers reversing on its own. Stopped at the same
  thing again it reverses on full lock, one side and then the other,
  to come at it from a new angle.
- **Re-anchoring.** The bounded last resort: 9 s inside one 8 m bubble
  teleports the car back onto its route, disclosed in the race record
  (`opp_rec=`). Stuck again at the same place, it is put down past
  whatever is there instead — a parked car one racer wrecks settles
  into a static body on the line of every racer behind.
  A car that drives for 20 s without getting further along its line
  than it has been — lost, a street away — is re-anchored the same
  way.

## Training

The policy is trained by reinforcement learning (PPO) in
`neural::env::DrivingEnv`: one race event's world — city collision and
props through the production importer, the event's barricades, parked
cars and drawbridges — with the production vehicle simulation and
nothing else. No audio, HUD, pedestrians or rendering; one worker steps
about 2,000–5,000 decisions a second on one core.

Every car in the environment is an agent on one of the event's real
opponent lines. An agent that finishes, wrecks or stalls is replaced
at once somewhere else, so the field never empties — a quarter of the
time a little way before a spot where an episode lately failed, so the
one hairpin and the one bridge ramp of a city are practised far more
often than chance would bring a car to them. Around the agents
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
failures were. `uv run python -m mm2rl.hot runs/<name>/lines.json`
ranks the events by it; a stretch of line that collects failures out
of proportion has more than once been a line drawn wrong rather than
a corner driven badly. Resuming takes any `--workers` count; the run keeps its
own copy of the environment binary, so rebuilding the repository does
not disturb it.

Changing what the environment does to the car invalidates trained
weights: retrain. A change to the observation layout (`OBS_VERSION`)
need not: `mm2rl.reshape` carries a checkpoint over, new columns
starting at zero, and training goes on from where it was.

### The two policies

Professional is the policy trained on the reward above and nothing
else: as fast as it can get its car round the course.

Amateur is a second network, not the first held back. It is
fine-tuned from the professional one with two more terms in its
reward — a cost on speed above 40 m/s and on cornering harder than
9 m/s² — so it learns a driver's worth of caution: it lifts earlier,
corners inside the tyres, hits less, and is beaten on pace rather than
by being made to brake at random.

```sh
uv run python -m mm2rl.train --run amateur --init runs/pro/latest.pt \
    --configure '{"rewards": {"speed": 0.002, "speed_free": 40.0,
                              "lateral": 0.002, "lateral_free": 9.0}}'
```

A run does not remember `--configure`; give it again when resuming.
The two costs are the dial. Tighter ones (31 m/s and 6 m/s² at twice
the cornering weight) made a driver whose winners took 1.4 times the
professional's time; these give about 1.15. Ship a policy by copying
its `latest.mm2p` over the file in `crates/mm2_app/policies/`.

## Results

Every race of both cities at both difficulties, the player parked on
the grid, until the field is home or twelve minutes are up
(`scripts/opponent_sweep.sh`), 2026-10-14:

| Opponents | Difficulty | Finished | Races with every car home | Re-anchors | Mean speed |
| --- | --- | ---: | ---: | ---: | ---: |
| Scripted (before) | Amateur | 149/256 (58 %) | 13/44 | 1071 | 12.4 m/s |
| Scripted (before) | Professional | 128/261 (49 %) | 8/44 | 1311 | 12.0 m/s |
| Neural | Amateur | 256/256 (100 %) | 44/44 | 233 | 25.3 m/s |
| Neural | Professional | 261/261 (100 %) | 44/44 | 436 | 28.0 m/s |

An Amateur race is won in a median 1.12 times the Professional
winner's time over the same course, the middle of the field 1.06; the
Amateur field also hits less and is re-anchored half as often. Speed is
distance over time for every car on the course, stalls and all; the
fastest drive `sf race11`'s 8 km at 40 m/s.

The professional policy is 178 M decisions of training (about 400 M
counting the runs it continues), the amateur one 20 M more from it —
about ten hours of one 18-core machine in all.

Where it is still weak: London's tight, tree-lined blocks and the
debris other cars leave in them (`london circuit8`, `race6` and
`circuit4` take the most re-anchors, up to a dozen a car). Cops &
Robbers police still drive the scripted chase (DSN-66): the policy
drives any guide, but a chase is a moving target and was not trained.

## Judging a policy

Three instruments, cheapest first.

```sh
# 1. Every field of a city from its grid, in the lean environment.
target/release/examples/mm2_gym --mm2-path $MM2_RETAIL --city sf \
    eval --policy training/runs/pro/latest.mm2p

# 2. Where a line cannot be driven as drawn (through a wall, over a
#    void, into water) — route defects, not driving mistakes.
target/release/examples/mm2_gym --mm2-path $MM2_RETAIL --city london audit

# 3. Every race in the real game, player parked, until the field is
#    home or twelve minutes are up.
scripts/opponent_sweep.sh target/release/mm2 out/ 4
```

The sweep is the one that counts: `opp=<finished>/<fielded>`,
`opp_rec=` re-anchors, and per car in `opps=` its checkpoints, finish
time, back-ups (`e`), re-anchors (`r`) and longest stall (`w`). Try a
policy that is not yet shipped with `mm2 --opponent-policy <file>`.
