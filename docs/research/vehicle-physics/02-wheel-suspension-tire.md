# Wheels: ground probe, suspension and tyre forces (`vehWheel`)

Everything here is **verified_original** — read from the retail
`Midtown2.exe` with the Kuna decompiler and checked against the
disassembly wherever an x87 comparison decides a branch — unless a line
says otherwise. Addresses are in *our* exe's numbering; mm2hook's
headers name the same vehicle functions `0x10` higher and globals
`0x1000` higher (the shift differs elsewhere in the exe — README). Conventions, units and the frame order live in
[README.md](README.md).

A `vehWheel` is `0x26C` bytes. A car has exactly four
(`whl0` front-left, `whl1` front-right, `whl2` rear-left, `whl3`
rear-right; left is `x < 0`). The wheel does three jobs every step, in
this order:

1. **Contact** (`0x4d29f0`, called by the owning drivetrain before it
   integrates wheel spin): cast the suspension ray, run the suspension,
   read the surface, compute the contact-patch velocities.
2. **Spin** is integrated by the drivetrain, not the wheel
   ([03-powertrain.md](03-powertrain.md)); the wheel only receives its
   new `RotationRate`.
3. **Forces** (`vehWheel::Update`, `0x4d34d0`, a child update of the
   drivetrain after the spin step): tyre forces from a two-axis
   stick–slip "bristle" model, a friction circle, surface drag, and the
   suspension force; all applied to the body at the contact point.

## Fields

`float[i]` in the decompiler means byte offset `4·i`. Names in the
first column are ours where mm2hook's guess was wrong.

| Off | Name | Meaning / source |
| --- | --- | --- |
| `0x18` | carSim | owning `vehCarSim*` |
| `0x1c` | ics | body `phInertialCS*` |
| `0x20` | flags | bit `4` = "non-rolling wheel" (brake = full static friction, slip = ground speed). No retail init path sets it — every `vehWheel::Init` call passes 0. |
| `0x24` | WheelMatrix | world matrix of the wheel (rows X,Y,Z,pos) |
| `0x54`/`0x58` | TireDispLimitLat/Long | file |
| `0x5c`/`0x60` | TireDampCoefLat/Long | file |
| `0x64`/`0x68` | TireDragCoefLat/Long | file |
| `0x6c` | SteeringLimit | file (rad) |
| `0x70` | CamberLimit | file (visual only) |
| `0x74` | WobbleLimit | file (visual only) |
| `0x78`/`0x7c` | BrakeCoef / HandbrakeCoef | file |
| `0x80` | SteeringOffset | file — the Ackermann factor |
| `0x84` | SuspensionLimit | file — bump travel above rest (m) |
| `0x88` | SuspensionExtent | file — droop travel below rest (m) |
| `0x8c` | SuspensionFactor | file — progressivity, clamped ≥ 0.75 |
| `0x90` | SuspensionDampCoef | file |
| `0x94` | wheelCount | set to the car's `WheelCount` (4); unused by the physics |
| `0x98`.. | ray segment / intersection | `0xc0` = world contact point, `0xcc` = hit normal, `0xd8` = hit fraction `t`, `0x15c` = segment |
| `0x16c` | grounded | probe hit and accepted |
| `0x170` | vLat | contact-patch velocity · lateral dir (`+` = right) |
| `0x174` | vFwd | contact-patch velocity · forward dir |
| `0x178` | vN | contact-patch velocity · ground normal |
| `0x17c` | vSlip | `ω·r + vFwd` (longitudinal slip velocity) |
| `0x180` | latDir | world, `= n × rearDir` (points right) |
| `0x18c` | n | ground normal |
| `0x198` | rearDir | world, `= normalize(wheelX × n)` (points back) |
| `0x1a4` | lastHit | copy of the contact point |
| `0x1b0` | Center | wheel pivot in model space (`geometry/<car>_whlN.mtx`) |
| `0x1bc` | Radius | `|max.y − min.y| / 2` of the pivot bounds |
| `0x1c0` | Width | `max.x − min.x` of the pivot bounds |
| `0x1c4` | L | static normal load (N) — **the tuning reference** |
| `0x1c8` | bump | surface bump displacement (m) |
| `0x1cc` | matDrag | surface `drag` |
| `0x1d0` | matFric | surface `friction` × weather × handling remap |
| `0x1d4` | depth | current sink depth (m) |
| `0x1d8`/`0x1dc` | matHeight/matWidth | bump amplitude / wavelength |
| `0x1e0` | bumpPhase | |
| `0x1e4` | spinAngle | visual |
| `0x1e8` | brakeTorque | set by `SetInputs` (N·m) |
| `0x1ec`/`0x1f0` | brakeMax / handbrakeMax | N·m, from `SetNormalLoad` |
| `0x1f4` | steerAngle | rad, set by `SetInputs` |
| `0x1f8` | x | suspension compression (m; `+` = up/compressed, `−Extent`..`Limit`) |
| `0x1fc` | Fs | suspension force (N) |
| `0x200` | N | normal force used by the tyre = `Fs` |
| `0x204` | xdot | compression rate (m/s, clamped ±10) |
| `0x208` | D | implicit suspension coefficient (N·s/m) |
| `0x20c` | ks | spring rate at rest (N/m) |
| `0x210` | k2 | progressivity (1/m) |
| `0x214` | cs | damper (N·s/m) |
| `0x218`/`0x21c` | wobble / camber | visual |
| `0x220` | slipVisual | 0..1, for skid marks/sound |
| `0x224` | majorSlip | `slipVisual ≥ 0.5` |
| `0x226` | groundedThisStep | counted by `vehCarSim::OnGround` |
| `0x227` | bottomedOut | counted by `vehCarSim::BottomedOut` |
| `0x228`/`0x22c` | dLat / dLong | tyre bristle displacement (m) |
| `0x230`/`0x234` | FLat / FLong | tyre force (N) |
| `0x238` | τreact | `FLong · Radius` — read by the drivetrain next step |
| `0x23c` | ω | `RotationRate` (rad/s; rolling forward is **negative**) |
| `0x240`/`0x244` | sLat / sLong | slip ratios, clamped to ±1 |
| `0x248`/`0x24c` | kLong / cLong | tyre long stiffness / damping |
| `0x250` | OptimumSlipPercent | file |
| `0x254`/`0x258` | StaticFric / SlidingFric | file |
| `0x25c`/`0x260` | kLat / cLat | |
| `0x264` | 1/opt² | |
| `0x268` | material | `lvlMaterial*` under the contact |

Constructor defaults (`0x4d2180`) — what a field holds when the file
omits it, and what the **right-side** wheels keep for the fields
`CopyVars` does not copy (see below): `SteeringLimit 0.39`,
`CamberLimit −1`, `WobbleLimit 0`, `BrakeCoef 1`, `HandbrakeCoef 1`,
`SteeringOffset 0`, `SuspensionLimit 0.1`, `SuspensionExtent 0.2`,
`SuspensionFactor 1`, `SuspensionDampCoef 0.1`, `TireDispLimit* 0.075`,
`TireDampCoef* 0.25`, `TireDragCoefLat 0.05`, `TireDragCoefLong 0.02`,
`OptimumSlipPercent 0.14`, `StaticFric 2.0`, `SlidingFric 1.9`,
`Radius 0.3`, `Width 0.1`, `L 5000`.

### Front/back files and the copy to the right side

`vehCarSim::FileIO` (`0x4ccc60`) loads the `WheelFront` block into
`whl0` and `WheelBack` into `whl2` only. `vehCarSim::Init` then calls
`vehWheel::CopyVars` (`0x4d4100`) `whl0 → whl1` and `whl2 → whl3`. It
copies the suspension (four fields), `SteeringLimit`, `CamberLimit`,
`SteeringOffset`, `BrakeCoef`, all six tyre displacement/damping/drag
fields, `OptimumSlipPercent`, `StaticFric` and `SlidingFric` — **but not
`HandbrakeCoef` or `WobbleLimit`**. The right-hand wheels therefore keep
the constructor's `HandbrakeCoef = 1.0` whatever the file says (retail
authors 2.0 on most cars), so the rear-right handbrake torque is half
the rear-left's. Both exceed the lock threshold on every retail car
(see [03-powertrain.md](03-powertrain.md#brakes)), so the asymmetry is
mostly invisible; mm2hook patches the `WobbleLimit` half of the same
omission.

## Load-dependent constants

### Static load — `ComputeConstants` (`0x4d23e0`)

```
inv_opt2 = 1 / OptimumSlipPercent²
L = 19.6 · Mass · 0.5 · |Center.z − CoG.z| / (2·|Center.z|)
  = 4.9 · Mass · |Center.z − CoG.z| / |Center.z|
SetNormalLoad(L)
```

`19.6` is the global at `0x5c5c1c` (stored as `−19.6`, read-only) —
the same constant the body's gravity uses, so the four preloads sum to
the car's weight at **2 g** (README § Gravity) — as long as `CoG.z` lies
between symmetric axles (the Moon Rover's do not; 07 § Derived numbers). `CoG` is the file's
`CenterOfGravity` (model space; the body's centre of mass sits at
`−CoG`). The split assumes the axles are symmetric about the origin:
with `CoG.z = 0` every wheel gets `Mass·19.6/4` regardless of where the
axles really are. Without a car (`carSim == 0`) the load is
`4.9 · ics.mass`.

`AddNormalLoad(ΔL)` (`0x4d2480`, used by trailers) is
`SetNormalLoad(max(L + ΔL, 1))` — the clamp floor is `1.0` and anything
below it becomes `1.0`.

### `SetNormalLoad(L)` (`0x4d24b0`)

```
if SuspensionFactor < 0.75: SuspensionFactor = 0.75        # written back
a   = 1 / ((Limit + Extent) · Extent)
ks  = (Factor·Extent + Limit) · a · L                       # 0x20c
k2  = (Factor − 1) · a · L / ks  = (Factor−1)/(Factor·Extent+Limit)   # 0x210
cs  = 2 · sqrt(ks · L) · SuspensionDampCoef                 # 0x214
m_q = L / 19.6                                              # quarter mass
kLong = 2·L / TireDispLimitLong ;  cLong = 2·sqrt(kLong·m_q)·TireDampCoefLong
kLat  = 2·L / TireDispLimitLat  ;  cLat  = 2·sqrt(kLat ·m_q)·TireDampCoefLat
brakeMax     = BrakeCoef     · StaticFric · Radius · L      # 0x1ec
handbrakeMax = HandbrakeCoef · StaticFric · Radius · L      # 0x1f0
```

What these mean:

- **The spring is shaped by `L`.** Its force (below) is exactly `0`
  at full droop (`x = −Extent`) and exactly `L` at rest (`x = 0`) for
  any `Factor`; at full bump (`x = Limit`) it is
  `L·(1 + Factor·Limit/Extent)`. With `Factor = 1` it is linear with
  rate `L/Extent` — **`SuspensionExtent` is the static sag**. A car
  whose preload split matches its real weight split therefore rests
  with its wheels exactly at the modelled pivots.
- **Suspension damping is `cs = 2·sqrt(ks·L)·ζ'`.** Using the load
  `L = 19.6·m_q` rather than the mass makes the true damping ratio
  `ζ = √19.6 · ζ' ≈ 4.43 · SuspensionDampCoef` (0.44 at the common
  0.1; 0.044 for the London Cab's 0.01).
- **The tyre is a stiff spring per axis** whose force at full
  displacement (`DispLimit/2` of travel) equals the load: `k·d = L` at
  `d = DispLimit/2`. Its damping is `TireDampCoef` of critical for the
  quarter mass.
- **Brake torque scales with load and grip.** `BrakeCoef = 1` is a
  brake exactly as strong as the tyre's static grip at rest on a
  friction-1 surface (`StaticFric·r·L`).

Worked example, `vpbug` (1000 kg, `CoG.z = 0`): `L = 4900 N`;
`ks = 32 667 N/m`, `cs = 2 530 N·s/m` (ζ ≈ 0.44); `kLat = kLong =
78 400 N/m`, `cLat = 2 214 N·s/m`; front `brakeMax = 3 087 N·m`, rear-left
`handbrakeMax = 9 878 N·m`, rear-right `4 939 N·m`.

## Step 1 — contact (`0x4d29f0`)

Signature: `contact(T_dt, out unused0, out wBreak, out unused1)` where
`T_dt` is the owning drivetrain's net torque (only its sign is used,
for `wBreak`). The two `unused` outputs are written and then
immediately overwritten with `0` (see the drivetrain doc).

### Wheel matrix and steering pivot

```
car = carSim ? carSim.WorldMatrix : ics.matrix
if SteeringLimit != 0:
    side = sign(Center.x)                      # +1 right, −1 left, 0 centre
    M = RotY(steerAngle)  ;  M.pos = Center    # RotY: row0=(c,0,−s) row2=(s,0,c)
    h = side · Width · 0.5
    M.pos.x += −h + h·M.row0.x ;  M.pos.y += h·M.row0.y ;  M.pos.z += h·M.row0.z
else:
    M = identity ; M.pos = Center
WheelMatrix = M · car                           # row-vector convention
```

The steered wheel pivots about its **inner edge** (half a width
inboard of the centre — the local point `x = −side·Width/2` is the one
that stays fixed), so steering swings the wheel centre and contact
point slightly inward and fore/aft on a circle of radius `Width/2`.
Positive `steerAngle` turns the wheel **left**; `SetInputs` negates the
input, so steering input `+1` is a right turn (below).

### The ray

```
up    = WheelMatrix.row1
a     = Limit + 0.3                    # 0x5afc54
start = WheelMatrix.pos + a·up
end   = WheelMatrix.pos − (Extent + Radius)·up
len   = Extent + Radius + a
hit   = world probe(start→end, room, carSim.instance, flags 0x20)   # 0x468e30
```

A hit is accepted only if `up · hitNormal ≥ 0.02` (`0x5af35c`).
Accepted: `n = hitNormal`, `p = hitPoint`,

```
rearDir = normalize(WheelMatrix.row0 × n)    # skipped (not grounded) if |·|² < 0.02
latDir  = n × rearDir                          # points right
steep   = |n.y| < 0.001                        # near-vertical surface → no grip (below)
```

### Not grounded

```
grounded = 0
suspension(x = −Extent, hit = false)          # see below; the wheel droops
N = Fs (= 0 once fully drooped)
wBreak = sign(T_dt) · −1e10
```

### Grounded

```
v     = ics.velocityAt(p)                    # 0x4790e0, includes push-correction velocity
vLat  = v · latDir
vFwd  = −(v · rearDir)
vN    = v · n
vSlip = flags&4 ? vFwd : ω·Radius + vFwd     # uses last step's ω here; Update recomputes it
```

**Surface.** The material is looked up from the hit's room/instance
(fallback: the global default material). With a material:

```
bump     = bumpDisplacement(sqrt(vLat² + vFwd²))      # 0x4d3430
matDrag  = mat.drag      (+0x30)
matFric  = mat.friction  (+0x2c)
target   = majorSlip ? mat.depth (+0x3c) : 0
depth   → target at rate |ω|·Radius·0.1 per second    # sinks only while slipping
matHeight = mat.height (+0x38) ;  matWidth = mat.width (+0x34)
```

without one: `bump = 0, matDrag = 0, matFric = 1, depth = 0`. Then:

```
if steep: matFric = 0
matFric *= WeatherFriction                       # global 0x5ce6b8, 1.0 dry
if carSim and matFric < 1:                       # CarFrictionHandling remap
    h = carSim.CarFrictionHandling
    matFric = (h >= 1) ? matFric / h : (1−h)·(1−matFric) + matFric
μs = StaticFric · matFric
```

`CarFrictionHandling < 1` pulls slippery surfaces toward full grip,
`> 1` makes them slipperier. Retail files author `1.0`, but the AI
controller overwrites it every frame with **2.0 while the car is
touching the player** ([06](06-ai-input.md#physics-interventions--the-complete-list)),
and because retail road friction is `0.9` (`_default`) — below 1 — that
halves an AI car's grip on ordinary tarmac while it is being shoved. A
`StaticFric 3.0` tyre otherwise has `μs = 2.7` on tarmac.

**Bumps** (`0x4d3430`), only if `mat.height != 0`:

```
bumpPhase += (rand()/32768 + 0.618) · dt · speed       # rand(): MSVC LCG
bumpPhase  = fmod(bumpPhase, matWidth)
bump = sin(2π·bumpPhase/matWidth) · matHeight · min(speed, 1)
```

**Suspension input.** The compression the ray implies:

```
x_probe = (a + Radius) − ((len·t − bump) + depth)
suspension(x_probe, hit = true, cosθ = up·n)
N = Fs
```

`x = 0` means the wheel's lowest point sits on the ground at its
modelled position.

**Spin breakpoint (`wBreak`).** The contact routine also reports the
spin rate at which this tyre would pass its optimum slip in the
direction the drivetrain torque is pushing — rolling rate
`ωr = −vFwd/r`, `ωr·(1 ∓ opt)` and `(1 − clamp(sLong, ±opt))·ωr` —
or `±1e11` if none lies ahead. The drivetrain's search over these is
inverted (it can never select a real breakpoint), so they have **no
effect**; a replication can skip them.

### Suspension (`0x4d2710`)

```
suspension(x_in, hit, cosθ):
    x_old = x ; bottomedOut = 0
    if hit: x = max(x_in, −Extent) ; atExtent = (x_in < −Extent)
    else:   x = −Extent            ; atExtent = true
    xdot = clamp((x − x_old)·invdt, −10, 10)
    prog = 1 + k2·x
    Fs   = (cs·xdot + ks·x)·prog + L
    if Fs < 0:                                  # unloaded: droop with first-order lag
        Fs = 0
        x  = (x_old·cs − dt·ks·Extent) / (dt·ks + cs)
        xdot = (x − x_old)·invdt ; D = 0
        return
    if x > Limit:                               # bump stop
        j  = impulseToStop(ics, n, vN, p)       # 0x46d290: −vN / (nᵀ·W(p)·n) if vN < 0 else 0
        Fb = invdt · j · 0.25                   # remove a quarter of the approach speed
        Db = −Fb / vN
        ics.pushOut((x − Limit)·cosθ · n)       # 0x4786f0, positional correction
        bottomedOut = 1 ; x = Limit
        xdot = clamp((x − x_old)·invdt, −10, 10)
        Fs = (cs·xdot + ks·x)·(1 + k2·x) + L + Fb
        D  = Db
        return
    D = atExtent ? 0 : prog·cs + dt·ks/cosθ     # implicit coefficient for the integrator
```

`W(p)` is the body's inverse effective-mass matrix at `p`
([01-rigid-body.md](01-rigid-body.md)). The rate is a finite difference
of the probe, so a step in the road (kerb) produces a damping spike
clamped at 10 m/s.

## Step 3 — forces (`vehWheel::Update`, `0x4d34d0`)

Runs after the drivetrain has written the new `ω`.

### Not grounded

All tyre state is zeroed: `slipVisual, majorSlip, dLat, dLong, τreact,
FLat, FLong = 0`; skip to the visual tail. **The bristle displacement
is forgotten the moment a wheel leaves the ground.**

### Implicit suspension term

```
if D > 0:
    Fpred = (x < Limit) ? ks·xdot·dt · n : 0
    ics.addImplicit(Fpred, p, D·n·nᵀ)          # 0x478930, see 01-rigid-body.md
```

The suspension is therefore integrated semi-implicitly by the body
solver; `Fs` itself is applied explicitly below.

### Slip ratios

```
sLat  = vLat == 0 ? 0 : (|vLat| <= |vFwd| ? vLat/|vFwd| : sign(vLat))
vSlip = flags&4 ? vFwd : ω·Radius + vFwd          # with the NEW ω
sLong = vSlip == 0 ? 0 : (|vSlip| <= |vFwd| ? vSlip/|vFwd| : sign(vSlip))
```

`sLat` is the tangent of the slip angle, saturating at 45°; `sLong` is
the slip ratio, saturating at ±1.

### The friction curve — `ComputeFriction(s)` (`0x4d25c0`)

```
s  = |s| ; μs = StaticFric·matFric ; μk = SlidingFric·matFric
f  = μs · s · (2·opt − s) / opt²             # parabola: 0 at s=0, μs at s=opt
if s <= opt:   visual = 0.5·s/opt ;            return f
if f > μk:     visual = ((μs − f) + 0.5·(f − μk))/(μs − μk) ; return f
               visual = 1 ;                    return μk
```

Past the optimum it falls back down the same parabola to the sliding
floor, which it reaches at `s = opt·(1 + sqrt(1 − μk/μs))`; with the
Beetle front (`opt 0.16`, `μk/μs = 0.9`) that is `s = 0.21`, i.e. by
12° of slip the front tyres are fully sliding.

### Choosing the coefficient for each axis

`uX = sign(vX) · N / kX` is the displacement per unit of μ for axis X;
the displacement **limit** is `μ·uX`.

```
for axis in (Long, Lat):                     # uses sLong/vSlip, sLat/vLat
    d_step = dt · v_axis
    μ_axis = μs ; lim_axis = μs·u_axis ; vis_axis = <unset>
    if |s_axis| <= opt:
        μ_axis = ComputeFriction(s_axis, &vis_axis) ; lim_axis = μ_axis·u_axis
    else:   # kinematically past optimum — but the bristle may still hold
        holds = (d_step > 0) ? (disp < lim && lim − disp >= d_step)
                             : (disp > lim && lim − disp <= d_step)
        if holds: vis_axis = 0                       # stay on static μs
        else:     μ_axis = ComputeFriction(...) ; lim_axis = μ_axis·u_axis
```

Then the **combined** coefficient, used for the friction circle:

```
m = max(|sLong|, |sLat|) ; longDominant = |sLong| >= |sLat|
if m < opt:       μc = μs
elif longDominant: μc = μLong ; if μLat > μLong: limLat  = μLong·uLat
else:              μc = μLat  ; if μLong > μLat: limLong = μLat·uLong
```

### Bristle displacement update

```
r = |ω·Radius| · dt · 0.1                         # relaxation per step (0x5ce6bc)
for axis:
    cand = disp + d_step
    if within(cand, lim):          # d_step >= 0 ? cand <= lim : cand >= lim
        disp = cand ; vel = d_step ; stick_axis = true
    else:                          # sliding: hold at the limit, relax toward it
        disp = d_step >= 0 ? max(disp − r, lim) : min(disp + r, lim)
        vel = 0 ; stick_axis = false
FLat  = −kLat ·dLat  − (velLat ·invdt)·cLat
FLong = −kLong·dLong − (velLong·invdt)·cLong
```

While sticking the tyre is a pure spring–damper — static friction: a
parked car holds on a slope, and small transients are absorbed by the
displacement. Any sustained slip velocity, as in a steady corner,
walks the displacement to its limit within a few steps; from then on
the force is `k·lim = μ(s)·N` along the axis with the damper off, so
steady-state grip follows the slip curve with a short spring lag in
front of it (the lag is the "relaxation length" `μ·N/k ≈ μ·DispLimit/2`,
~17 cm on a Beetle front tyre).

### Friction circle

```
Fmax = μc · N
if FLat² + FLong² > Fmax²:
    if (stickLong or |sLong| <= opt) and (stickLat or |sLat| <= opt):
        scale both by Fmax/|F|                         # still "gripping": shrink
    else:
        s = Fmax / |(vLat, vSlip)|                      # sliding: oppose slip velocity
        FLat = −s·vLat ; FLong = −s·vSlip
```

Visual slip: `slipVisual = 0` if both axes stick; the sliding axis's
`vis` if one slides; `max(visLat, visLong)` if both slide.
`majorSlip = slipVisual ≥ 0.5` (drives depth sinking, skid FX).

`τreact = FLong · Radius` — the drivetrain subtracts it from its torque
**next step** ([03-powertrain.md](03-powertrain.md)).

### Surface drag (only on surfaces with `drag ≠ 0`)

```
DLat  = −|vLat|·TireDragCoefLat ·N·vLat ·matDrag
DLong = −(1 + depth)·|vFwd|·TireDragCoefLong·N·vFwd·matDrag     # along forward
```

Retail `drag` is `0` on every dry material — only `water` (0.119) and
`deepwater` (0.5) produce it — so tyre drag exists only in water.

### Applying the force

```
F = Fs·n + (FLat + DLat)·latDir + (FLong + DLong)·(−rearDir)
ics.force  += F
ics.torque += (p − ics.position) × F
```

Everything acts **at the contact point**, so lateral grip rolls the
body about its centre of mass with the full centre-of-mass height as
lever, and the suspension force acts along the ground normal, not the
suspension axis.

### Visual tail

`camber = sign(Center.x)·x·CamberLimit` (if `CamberLimit > 0`),
`spinAngle += dt·ω`, the visual wheel is moved up its own axis by
`x − visualDisp` where `visualDisp = clamp(r·0.05·Fs/L, 0, 0.3·r)`
(`0x4d4020`, a tyre "squash"), then rotated by `spinAngle` and
`wobble`. None of this feeds back.

## Inputs — `SetInputs(steer, brake, handbrake)` (`0x4d3f70`)

```
a = −steer · SteeringLimit
steerAngle = (1 − a · SteeringOffset · sign(Center.x)) · a
brakeTorque = handbrake·handbrakeMax + brake·brakeMax
if flags & 4: brakeTorque = StaticFric · N · matFric · Radius
```

`SteeringOffset` is an Ackermann gain: turning right (`a < 0`), the
right (inner) wheel gets `|a|·(1 + |a|·off)` and the left
`|a|·(1 − |a|·off)`. At the Beetle's 0.4 rad lock and `off = 0.25` that
is 0.44 vs 0.36 rad. What `vehCarSim` passes in (speed-sensitive
scaling, rear counter-steer, the handbrake split) is in
[04-chassis-and-assists.md](04-chassis-and-assists.md#inputs-to-the-wheels).

## Summary for a re-implementation

- One raycast per wheel from `Limit + 0.3` above the hub to
  `Extent + Radius` below it; compression from the hit distance.
- Spring `Fs = (cs·ẋ + ks·x)(1 + k2·x) + L` with `L` the static share of
  `m·19.6`, `ks = L/Extent` for `Factor = 1`, rate by finite
  difference; droop with a first-order lag when unloaded; quarter-speed
  bump stop with positional push-out.
- Tyre: two independent displacement springs `k = 2L/DispLimit`,
  damped at `TireDampCoef` of critical, whose displacement saturates
  at `μ(s)·N/k`, `μ(s)` the parabola-then-floor curve of the slip ratio
  of that axis; friction circle on `μc·N`.
- Grip coefficients are large (`μ ≈ 2.7` on tarmac) **and** act on a
  load computed at 19.6 m/s².
- Everything is applied at the contact point; the longitudinal force's
  moment about the axle goes back into the drivetrain one step late.
