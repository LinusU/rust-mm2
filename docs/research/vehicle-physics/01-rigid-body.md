# The body under the car: time step, integrator, gravity, collisions

**verified_original** throughout (retail `Midtown2.exe`, Kuna decompile
cross-checked with the disassembly), unless a line says otherwise.
Conventions in [README.md](README.md).

## The time step

### Frame time — `datTimeManager::Update` (`0x4c6330`)

Called once per frame at the top of the main loop (`0x401a00`):

```
raw     = QueryPerformanceCounter delta (s)           # ActualSeconds 0x5cd824
Seconds = clamp(raw, 1e-4, 0.1)                        # 0x5cd820 ; limits 0x5cd838 / 0x5cd834
InvSeconds = 1 / Seconds                               # 0x5cd828
```

The game calls `datTimeManager::RealTime(0)` once at start-up
(`0x40170a`), which removes the frame-time cap; `FixedFrame` exists but
nothing calls it, and the main loop has no frame limiter. **The
simulation runs on the real, variable frame delta.** Below 10 fps the
clamp makes the game run in slow motion.

### Physics samples — `dgPhysManager::Update` (`0x4688a0`)

The physics manager splits each frame into up to three equal samples:

```
n  = min(MaxSamples, ceil((Seconds − 0.001) / SampleStep))
dt = Seconds / n        # SetTempOversampling: Seconds/InvSeconds are the sub-step inside the loop
```

`MaxSamples` and `SampleStep` default to 6 and 1/60 in the manager's
constructor but are overwritten at game start (`0x412c72`/`0x412c87`)
with **3** and **1/35 s**:

| Frame time | fps | Samples | Physics step |
| --- | --- | --- | --- |
| ≤ 29.6 ms | ≥ 33.8 | 1 | the frame time (16.7 ms at 60 fps, 6.9 ms at 144) |
| 29.6–58.1 ms | 17.2–33.8 | 2 | half the frame (30 fps → 2 × 16.7 ms) |
| > 58.1 ms | < 17.2 | 3 | a third (10 fps → 3 × 33.3 ms) |
| ≤ 1 ms | > 1000 | 0 | **no physics that frame** |

So on the hardware of its day — vsync'd at 60–85 Hz, or 30–60 fps on a
slow machine (**inferred**) — the original stepped at **1/60–1/85 s**,
and it is not frame-rate independent: several vehicle terms are per step
([08](08-replication-recipe.md#known-frame-rate-dependence)). Faithful
reproduction at 60 fps is a single 1/60 s step per frame.

### The frame

```
datTimeManager::Update
game nodes update: mmPlayer reads input → vehCarSim inputs; AI; DeclareMover(player car, priority 4, all collisions)
game-frame node (0x403000): level, AI map, then dgPhysManager::Update:
    PreUpdate every mover                        (vehCar::PreUpdate 0x42c470 — drivable-mode overrides)
    repeat n samples (Seconds = frame/n):
        S1  Update every mover                   (vehCar::Update 0x42c680 — see README § pipeline)
        S2  collide every mover                  (terrain, mover-vs-mover, gathered static collidables)
        S3  UpdateMtx every mover                (MoveICS: apply position pushes)
    Seconds = frame
    PostUpdate every mover                       (render matrices)
```

Each phase runs over **all** movers before the next starts. Inside
`vehCar::Update` the car integrates first and computes its new forces
afterwards, so:

- a sample integrates forces computed from the **previous** sample's
  state, plus this sample's gravity, plus the previous sample's
  collision impulses;
- an input change is read by `SetInputs` in sample *k* but the forces
  it causes are integrated in sample *k*+1 — one physics step of input
  latency on top of the frame's;
- the contact solver in S2 sees the post-integration state of the same
  sample; its impulses wait for the next integration.

The player car's collider is created without a `phSleep`
(`vehCar::Init`, `0x42bead`), so **the car never sleeps**; only
bangers and AI bodies use `phSleep`.

## The rigid body — `phInertialCS` (`0x1b4` bytes)

| Off | Field | Notes |
| --- | --- | --- |
| `0x08` | active | 0 = frozen (only phSleep clears it) |
| `0x0c`/`0x10` | mass / 1/mass | |
| `0x14`/`0x20` | I (x,y,z) / 1/I | principal moments about the **body** axes |
| `0x2c` | maxSpeed | 500 m/s (constructor; nothing overrides it for cars) |
| `0x30` | maxAngSpeed (x,y,z) | per body axis; 5 rad/s default, **4π for cars** (`vehCarSim::Init`) |
| `0x3c` | p | linear momentum — the state variable |
| `0x48` | L | angular momentum, world frame — the state variable |
| `0x54` | matrix | rows: X, Y, Z axes (world), position = centre of mass |
| `0x84` | v | `p/m` |
| `0x90` | ω | world frame |
| `0x9c`/`0xa8` | F / T | accumulators |
| `0xb4`/`0xc0` | F2 / T2 | second accumulators (implicit-force path) |
| `0xd0`/`0xdc` | impulse / angular impulse | from collisions |
| `0xe8` | netPush | position correction (metres) |
| `0xf4` | netTurn | `ω·dt` scratch |
| `0x100`/`0x10c` | totalPush / lastTotalPush | push applied this / last step |
| `0x11c` | useImplicit | set by `ApplyForceWithJacobian` |
| `0x120`/`0x150`/`0x180` | A / B / C | implicit Jacobian sums |
| `0x1b0` | timeAlreadyIntegrated | always 0 in retail (its only writer is dead) |

### Mass properties — `InitBoxMass` (`0x4760c0`)

```
Ix = m·(y² + z²)/12 ;  Iy = m·(x² + z²)/12 ;  Iz = m·(x² + y²)/12
```

`x, y, z` are the full edge lengths of `InertiaBox` — a solid box
(default mass 2000, box 2 × 1 × 3). `Ix` is pitch, `Iy` yaw, `Iz` roll.

### The step — `0x477de0` → `Integrate` (`0x478050`)

`0x477de0` folds `F2/T2` into `F/T`, integrates by `dt = Seconds`, then
clears `F2/T2`, copies `totalPush` to `lastTotalPush` and clears it.

`Integrate` has two modes.

**Mode A** (no implicit force this step — e.g. a car with no wheel on
the ground):

```
p += impulse + dt·F ;  v = p/m
L += angImpulse + dt·T
w_i = (row_i · L) / I_i      for the body axes, using the matrix before this step
w_i = clamp(w_i, ±maxAngSpeed_i)        (if any clamped: L = Σ w_i·I_i·row_i)
ω = Σ w_i·row_i
```

Momentum is the state, so gyroscopic coupling comes for free. There is
**no damping of any kind** in the integrator.

**Mode B** (one or more wheels called `ApplyForceWithJacobian` this
step — the normal state of a car on the ground): a linearly-implicit
velocity update that treats the wheels' suspension terms implicitly.

```
s  = dt/m ;  T1 = (I3 + s·A)⁻¹ ;  dp = impulse + dt·F
Iw = Rᵀ·diag(I)·R
M2 = Iw + dt·C − s·dt·B·T1·Bᵀ
Q  = angImpulse + dt·T − s·B·(T1·dp)
Δω = M2⁻¹·Q
Δv = T1·(dp + dt·Bᵀ·Δω)/m                (see sign note)
v += Δv ; p = m·v ; ω += Δω ; L = Iw·ω
```

with the sums accumulated by `ApplyForceWithJacobian(f, point, J)`
(`0x478930`): `F2 += f`, `T2 += r × f`, `A += J`, `B += [r]×·J`,
`C += [r]×·J·[r]×ᵀ` (`r = point − position`). The wheels pass
`J = D·n·nᵀ` — their implicit suspension coefficient along the ground
normal ([02](02-wheel-suspension-tire.md#implicit-suspension-term)).
Mode B does **not** apply the per-axis angular-speed clamp.

*Sign note (inferred):* the consistent elimination of that coupled
system back-substitutes `Δv = T1·(dp − dt·Bᵀ·Δω)/m`; the code adds.
The coupling term is small (`dt·D·r`); copy the `+` to be bit-faithful.

**Both modes then:**

```
if |v| > maxSpeed: scale v and p to maxSpeed
position += netPush ; position += dt·v                # symplectic: new velocity
netTurn  += dt·ω
if |netTurn|² > 1e-15: rotate every axis row by |netTurn| about netTurn (right-handed)
F = T = impulse = angImpulse = 0 ; totalPush += netPush ; netPush = 0 ; netTurn = 0
useImplicit = 0 ; A = B = C = 0
```

The matrix is never re-orthonormalised.

### Forces at a point

The wheels add straight to the accumulators: `F += f`,
`T += (p − position) × f`. There is no separate helper. Point velocity
`v + ω × (p − position)` is `0x478f30`; the wheel uses `0x4790e0`,
which additionally removes the velocity a positional push would fake
(`lastTotalPush`).

## Gravity

`dgPhysEntity::Update` (`0x46a110`) — the first thing `vehCar::Update`
does each sample — adds `F.y += mass · −19.6` (global `0x5c5c1c`,
read-only). The integrator has no gravity of its own. A car falls at
**19.6 m/s²**. The same helper is called for bangers (`0x440040`),
trailers (`0x4d7af0`) and AI physics bodies (`0x553650`).

## Collisions

Detection (bound shapes against the world and each other) is outside
this document; the **response** is:

### Material pair

```
friction   = matA.friction · matB.friction
elasticity = min(matA.elasticity · matB.elasticity, 1.0)      # 4.0 under the /flubber cheat
```

A car's hull (`vehBound`) has a single material whose friction and
elasticity are the file's `BoundFriction` / `BoundElasticity`
(defaults 0.3 / 0.2; retail 0.2–0.9 / 0–0.5). World materials come
from `materials.mtl` (`_default` 0.9 / 0.9). Car against car is the
product of the two hulls (e.g. 0.5·0.5 = 0.25 friction).

### Contact solver (`0x46cce0`)

For each impact of a pair (N impacts, each weighted `1/N`):

```
relVel = (vA + ωA×rA) − (vB + ωB×rB)          # n points from B to A
K = K_A + K_B                                  # K_X = 1/m·I3 + [r]× Iw⁻¹ [r]×ᵀ  (0x478cc0)
if n·relVel <= 0.01:                           # approaching (or resting)
    x = K⁻¹·(−relVel)                          # impulse that stops the contact point dead
    xn = x·n ; xt = x − xn·n
    if |xt| >= μ·xn:                           # outside the friction cone: slide on its edge
        w = n + μ·xt/|xt| ;  d = n·(K·w)
        x = |d| >= 1e-5 ? w·(−n·relVel / d) : 0
    x *= 1 + elasticity
else:
    x = 0
penetration push (tolerance 0 at run time): split by |K_A n| : |K_B n|,
    never pushing a body downward if the other can take it
A: impulse += x/N, angImpulse += rA × x/N, netPush ⊕= pushA
B: the opposite
```

`⊕=` is `CalcNetPush` (`0x4786f0`): pushes along the same direction
take the **maximum**, not the sum, so several contacts do not stack.
Impulses wait in the accumulators for the next integration; pushes are
applied at the end of the sample (`MoveICS`, `0x478670`). There is no
time step in the contact path (impulses are momentum) and no velocity
clamp beyond the integrator's.

### `SetHackedImpactParams` is dead

`0x4cc070` (bound elasticity 0, friction 2, brake 1) has no callers or
pointers in the shipped exe. `RestoreImpactParams` (`0x4cc040`) runs
from `vehCarSim::Reset`.

## Not resolved

- Jointed colliders (the semi's trailer hitch) route impacts through
  `0x46e4d0`; not decoded.
- Collision detection itself (bound vs. world polygons, hull vs. hull).
- Whether anything renormalises the body matrix outside the integrator
  (nothing found).
