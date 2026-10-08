# Moving scenery (`<city>_{sailboat,ferry,train}*.pathset`)

Beside the drawbridges and parked cars (`drawbridge.md`, `parked.md`)
the original creates three more per-city object managers
(`0x413230`): sailboats (`0x415250`, default model `giz_sailboat01_f`),
the Underground (`0x4155d0`, `va_ug_l`) and ferries (`0x415790`,
`giz_carferry01_f`, created only when the session is not networked).
All use the shared `<city>_<object>[_<event stem>].pathset` lookup.
Everything below is **verified_original** — read from `Midtown2.exe`.

Retail files: `london_sailboat` (tugs, water taxis, ducks — 16
paths), `london_ferry` (6, two of them moored two-point paths),
`london_ferry_crash9/11/12` (Crash Course), `london_train` (8 lines);
`sf_sailboat` (sailboards, ducks, default sailboats — 16),
`sf_ferry` (1).

## The path follower (`0x579dd0`–`0x57a4e0`)

Every object rides a follower over its path's points:

- a **closed** Catmull-Rom loop: each segment is a Hermite cubic
  (basis `[2,−2,1,1; −3,3,−2,−1; 0,0,1,0; 1,0,0,0]` at `0x4c0860`)
  from `P[i]` to `P[i+1]` with tangents `(P[i+1] − P[i−1]) / 2` and
  `(P[i+2] − P[i]) / 2`, indices wrapping;
- the segment's length is estimated as `|P[i] − mid| + |mid − P[i+1]|`
  with `mid` the curve at `t = 0.5`;
- the follower holds seconds `u` into the segment and
  `t = speed · u / length`; advancing adds `dt` to `u` and crosses at
  most one segment boundary per call (backwards too, for negative
  `dt`);
- position is the cubic, direction its derivative; a **two-point
  path** returns its first point facing the second — a parked object.

The object's matrix takes the direction as its local +Z row and
orthonormalises with +Y up (`0x4bee80`): it faces +Z along travel.

## Sailboats (`0x578370`)

Model: the path name when `geometry/<name>.pkg` exists, else the
default. Speed: the path's spacing (metres) ± 1 — a uniform draw
between `spacing − 1` and `spacing + 1` (manager fields 4.0/1.0 at
`0x57826d`; the 4.0 base is only used by a re-randomise call). The
mesh is drawn raw at the curve point.

## Ferries (`0x579400`, `0x5791e0`)

Model as above. Speed 0.75 m/s with zero spread (`0x579307`).
Height: the curve point plus the model's `dgBangerData` `CG.y`
(`[record + 0x20]`) — which puts each hull's bottom on the water.
Each ferry carries the `ferry` object sound (§ Object audio).

## The Underground (`0x578da0`, `0x5788d0`–`0x578c70`)

Per path, a train of three cars (`0x5788f6`), each its own follower at
40 m/s (`0x5db034`), car *i* advanced `0.44·i` s (`0x5db030`) — 17.6 m
apart. Car height is the straight interpolation of the segment's end
point heights (level with the authored rails, not the curve's
overshoot) plus `CG.y`. The shuttle:

| Phase | Behaviour |
| --- | --- |
| waiting | 10 s (`0x5db038`), then accelerate |
| accelerating | speed fraction +0.51/s (`0x5db03c`) to 1 — cars advance `dt · fraction` |
| running | full speed until the end check, then brake |
| braking | fraction −0.51/s to 0, then wait and reverse direction |

The fraction starts at 1, so the first departure skips the ramp. End
check: running forward, the rear car's segment index reaches
`count − 3`; backward, the last car's index falls to 1. Reversal
negates `dt` (`0x5af408` = −1). The `subwaycar` object sound rides
the middle car (§ Object audio).

## Object audio (`Aud3DAmbientObject`, `0x515080`–`0x515bc0`)

Drawbridge leaves, ferries and trains each own a positional sound
object bound to `aud/ambient/<name>.csv` (`aud\ambient` + `csv`,
`0x515742`): `drawbridge`, `ferry`, `subwaycar`. The table's
`sample type` dispatches through the jump table at `0x515314`:

| Type | Behaviour |
| --- | --- |
| 0 | loop, played while the row is active and the emitter's speed lies in `min speed..max speed` (`0x515390`); deactivating stops it at once (`0x5155a0`) |
| 1 | timed one-shot at a random 0.75–1 gain and pan (`0x5154b0`) |
| 2 | timed one-shot: while active and in the speed window, a run-out timer fires the row unless it still plays, then redraws the timer in `oneshot time limit low..high` (`0x5153e0`, `0x515440`) |
| 3 | parameter updates only — fired solely by an explicit trigger no retail object issues |

The `active` column is each row's initial state; owners flip rows
with `0x515580`/`0x5155a0` (or all rows with index −1):

- **drawbridge** (`bridgemove` loop, `bridgebell` type 2 with a `0,0`
  window — so the bell re-rings back to back): both rows on when a
  leaf starts opening or closing (`0x5774c6`, `0x577503`), off when it
  comes to rest (`0x5775b1`, `0x57764e`); each leaf has its own.
- **ferry** (`ferryengine` loop, `ferryhorn` every 5–10 s): authored
  active, never switched.
- **subwaycar** on the middle car (`0x59d490`): moving trains switch
  to row 0 (`LondonTube`), stopped ones to row 1 (`NOTHING` — the
  silent sentinel); the subclass switches on a speed threshold of 1.

The table's `audible area` gates it on where the camera is. Each
frame the camera update reads the `Subterranean` flag (0x02) of the
PSDL room the camera stands in into the audio manager
(`0x4057c6`–`0x405809` → `0x50f9a0`/`0x50f9c0`, `[mgr + 0x24]`); an
area-1 table plays only while it is set, area 2 only while it is
clear, area 0 anywhere (`0x515bc0` before starting, `0x5151a0` while
playing). `subwaycar` is area 1: the Tube rumble is heard only from
inside the tunnels. Entering a tunnel also switches an effect on the
playing samples (`0x515ad0`, parameters `[mgr + 0xa0]` = 0.5 and 0.96
— an echo by the look of it, unverified) — not reproduced. The room lookup is designed (DSN-67).

An emitter plays only while the listener is nearer than the table's
`Max distance` (150 m bridge and Tube, 225 m ferry): once the squared
distance reaches the squared max it is stopped — every sample, a
playing one-shot included (`0x512070`, `0x5156f0`) — and its
rows' timers run only while it plays (`0x515230` is the playing
object's update). Inside that range the volume factor is
`1 − (d² − min²) / (max² − min²)`, 1 within `Min distance`
(`0x511eb0`, set up from the squared distances at `0x512040`;
`0x512260` turns the attenuation into the factor). Every frame rows of
types 0, 2 and 3 take `sample volume × factor` (`0x515330`, also the
pan); type-1 rows instead fire at `sample volume × U(0.75, 1)` with a
random pan in `−1..1` and never follow the distance (`0x5154b0`).
The runtime pans with Bevy's spatial audio and sets the volume itself
(`PAN_ONLY_EDGE`); type-1 voices play centred — the random pan is not
reproduced.

## City ambience (`<city>ambientcontainer.csv`, `0x404046`, `0x5137a0`)

When the city loads, and while the audio detail flag `0x800` of
`[0x6b0650]` is set (it is in the default `0xc73`), the original reads
`aud/ambient/londonambientcontainer.csv` for London or
`sfambientcontainer.csv` for San Francisco (`0x40405b`/`0x404077`; no
other city name is tried). Each name the container lists becomes one
`Aud3DAmbientObject` bound to `aud/ambient/<name>.csv` (`0x513880`) —
`londonriver` and `tubevoices`; `birdies`, `buoyseals`, `horns_gulls`
and `trolleycable`. `waves.csv` ships but no container lists it, and
nothing else names it: dead data.

These tables carry `VECTORPOINTS`, read into the object's point list
(`0x5123e0`). The object stands at one point at a time: while idle,
every update re-picks the point nearest the listener (`0x512530` →
`0x512580`, first of equals) and starts the object once in range;
playing, it keeps that point until the listener's squared distance
reaches the squared max, when it stops and the next idle update picks
again. Everything else — rows, timers, falloff, `audible area` — is
the object audio above: `tubevoices` is area 1, so the station
announcements are heard only from inside the tunnels; `birdies` holds
only type-1 rows, so its gulls fire at random gain regardless of
distance once within 250 m. Row timers start at 0, so every one-shot
sounds the moment the listener comes in range.

## Implementation

`mm2_game::movers` (`PathFollower`, `TrainMotion`, `mover_rotation`,
the constants) and `mm2_app::movers` (spawning and the fixed-step
driver); `mm2_game::object_audio` (the table rules) and
`mm2_app::object_sound` (voices and the ambience emitters). Each object is a kinematic body
whose origin is the curve point; the driver poses it where its path
is now and sets the
velocities that reach the next step's pose, so a car resting on a
ferry rides it. Sailboat speed draws are seeded from the session
(designed).

## Continuity check (F28-AC02/AC03)

Measured, not recovered: stepping every retail sailboat, ferry and
train path for 400 s at the fixed 120 Hz rate (`mm2_app` test
`retail_every_mover_path_steps_continuously`), no step exceeds 3.6× its
nominal `speed · dt` — sailboats 3.3× (32 paths), ferries 2.1× (25),
Tube cars 3.6× (8, all in the first step after the 10 s wait). The
Hermite parameter is not arc length and the closed-loop tangents at a
line's first segment wrap to its last point, so a segment can run
fast; a teleport would be tens of metres. Whether the original shows
the same first-segment surge is not verified. Through the production
`drive_movers` system and a real Avian world, a body resting on a
moving platform rides it, a platform does not move before the
countdown, and a train turns round without a jump (synthetic boxes).
