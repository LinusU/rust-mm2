# Replication recipe

The original car model as one self-contained per-step program. Each
block cites the document that derives it; constants are the retail
exe's. Port it literally first, then tune — every number below is what
the original computes, including its quirks (marked **quirk**), so a
faithful port should reproduce MM2's feel before any change is made.

Notation: vectors are world-space unless noted; `R` is the body
rotation (rows = car axes `right`, `up`, `back`); `·` dot, `×` cross;
`dt`, `invdt` as in [01-rigid-body.md](01-rigid-body.md).

## Constants

```
G            = 19.6        # gravity (m/s²), body and wheel loads
PROBE_ABOVE  = 0.3         # ray starts this far above SuspensionLimit
MIN_UP_DOT_N = 0.02        # contact accepted if wheelUp·n >= this
STEEP_NY     = 0.001       # |n.y| below this: no grip
XDOT_CLAMP   = 10          # suspension rate clamp (m/s)
BUMP_STOP_K  = 0.25        # fraction of approach speed removed per step
RELAX_K      = 0.1         # bristle relaxation per metre rolled
DRAG_TORQUE  = 50          # N·m per drive train, always opposing spin
FREE_I_PER_KG= 0.005       # inertia of a train with no engine (× mass)
ENGINE_I_ADD = 0.02        # added to g²·I_engine
DIFF_BIAS_LO = 1.25 ; DIFF_BIAS_HI = 1.03 ; DIFF_BIAS_W = 50   # rad/s
DIFF_SMOOTH  = 0.1
BRAKESTAND   = 0.95 (inputs) ; 1.0 (m/s)
HP_TO_W      = 746 ; MPH_TO_M_PER_MIN = 26.8224
MAX_ANG_VEL  = 4π per axis (rad/s)
```

## Time step and body ([01](01-rigid-body.md))

```
frame:  Seconds = clamp(realFrameDelta, 1e-4, 0.1)
        n = min(3, ceil((Seconds − 0.001)/(1/35)))   ;  dt = Seconds/n       # 1 step of 1/60 at 60 fps
        PreUpdate cars ; repeat n: { Update cars ; collide ; apply pushes } ; PostUpdate cars

body:   state = momentum p, angular momentum L (world), matrix, mass, I = InitBoxMass(Mass, InertiaBox)
        I = m/12·(y²+z², x²+z², x²+y²) ; |v| <= 500 ; |ω_body_axis| <= 4π (explicit mode only)
step:   (mode A) p += impulse + dt·F ; L += angImpulse + dt·T ; ω from L via body axes
        (mode B, when any wheel added an implicit term) linearly-implicit solve — 01 § The step
        pos += netPush + dt·v ; rotate axes by ω·dt ; clear F, T, impulses, implicit sums
gravity: F.y += −19.6·Mass at the start of every car update
```

A port to a different engine can use its collision geometry, but retaining
its generic contact solver is an adaptation: the original contact response
accumulates impulses for the next integration, not immediate velocity changes
([01, contact solver](01-rigid-body.md#contact-solver-0x46cce0)). Preserve the
force lag, 2 g gravity, absence of body damping beyond `vehAero`, and implicit
suspension. Matching those formulas alone does not prove matching trajectories.

The current rust-mm2 port retains the authored four-wheel-car hull and uses
raw material products (`frictionA·frictionB`, `min(elasticityA·elasticityB, 1)`).
For marked static-world contacts it suppresses Avian's constraint solve by
removing manifold points temporarily, restores them before sleeping, then
computes source effective-mass impulses for the next integration and positional
pushes for this step. Dynamic contacts remain an adaptation. The swept midpoint
used to reconstruct a contact arm is **inferred** from planar measurements:
the sweep must hit the same collider with a normal dot product at least 0.999,
or the Avian manifold point is retained. This is not a recovered general
collision-detection algorithm.

## Load time

```
for each wheel w (whl0..3; front block → whl0, copied to whl1; back block → whl2, copied to whl3,
                  except HandbrakeCoef/WobbleLimit which stay at 1.0/0 on whl1/whl3 — quirk):
    w.center, w.r, w.width from geometry/<car>_whlN.mtx
    w.L = 4.9·Mass·|w.center.z − CoG.z| / |w.center.z|                 # 02 § Static load
    SetNormalLoad(w, w.L)                                               # 02 § SetNormalLoad
body.InitBoxMass(Mass, InertiaBox)                                      # 01
gear ratios, auto shift points                                          # 03 § Ratios
engine constants (ωidle/opt/max, P, A=(√5+1)/2, B=(√5−1)/2)            # 03 § Engine
axle k, c (from authored TorqueCoef/DampCoef)                                                 # 04 § vehAxle
trains: type 0 → drive {whl2,whl3}, free {whl0},{whl1}
        type 1 → drive {whl0,whl1}, free {whl2},{whl3}
        type 2 → drive {whl0,whl1,whl2,whl3}
gear = first (index 2), gearChanged = 1                                 # Reset
engine.ωe = 0 ; engine.RPM = engine.rpmAtShift = IdleRPM                # cold reset, 03
engine.inGearChange = 1 ; engine.gclTimer = GCL
```

In rust-mm2, wheel geometry is measured from the available mesh, with MTX
bounds as a fallback: radius is the vertical half extent, while width is the
full lateral extent used by the inner-edge steering pivot. Importing mesh
width as a half extent was a port error.

## Per step, per car

```
# ---- inputs (05/06 produce SteeringInput, Throttle, Brake, Handbrake, gear requests)
apply drivable-mode overrides                                           # 04 § Drivable modes

# ---- vehCar::Update
body.force += (0, −G·Mass, 0)

# ---- vehCarSim::Update
Speed = |body.v · R.back|
s = SSS(Speed)·SteeringInput                       # SSS = 1 for every retail car
for w in fronts: SetInputs(w, s, Brake, max(−Handbrake, 0))
bR = (Brake > 0.95 and Throttle > 0.95 and Speed < 1) ? 0 : Brake
hb = max(Handbrake, 0)
SetInputs(whl2, −s, bR, hb·(SteeringInput > 0 ? 1 − SteeringInput : 1))
SetInputs(whl3, −s, bR, hb·(SteeringInput < 0 ? 1 + SteeringInput : 1))
body.integrate()                                   # 01 — consumes everything accumulated since the last call
worldMatrix = body.matrix with pos += R·CoG

engine.update()                                    # 03 § vehEngine::Update
transmission.update()                              # 03 § vehTransmission::Update
aero.update()                                      # 04 § vehAero
for train in (freeL, freeR, drive) or (drive):     # 03 § The step
    train.update()   # brakes+engine+τreact → contacts → bias → ω → wheels' Update (02 § Step 3)
axles.update()                                     # authored anti-roll, after tyre squash

# ---- after vehCarSim
gyro.update()                                      # 04 § vehGyro
stuck.update()                                     # 04 § vehStuck
```

### `SetInputs(w, steer, brake, handbrake)`

```
a = −steer·w.SteeringLimit
w.steerAngle  = (1 − a·w.SteeringOffset·sign(w.center.x))·a
w.brakeTorque = handbrake·w.handbrakeMax + brake·w.brakeMax
```

### `train.update()`

```
coef = (ω != 0) ? BrakeDynamicCoef : BrakeStaticCoef
B = DRAG_TORQUE + Σ w.brakeTorque·coef
if engine attached:
    g = ratio[gear]
    T = g·engine.T + g·engine.I·(g·ω + engine.ω)·invdt
    I = g²·engine.I + ENGINE_I_ADD
else:
    T = 0 ; I = Mass·FREE_I_PER_KG
T −= Σ w.τreact
if ω != 0: canStop = |T| <= B ; T += sign(ω)·B
else:      T = T >= 0 ? max(T − B, 0) : min(T + B, 0) ; canStop = false
for w in wheels: contact(w)
bias update (wheelCount ≥ 2 and |ω| ≥ 0.001, else bias = 1):
    maxB = |ω| >= 50 ? 1.03 : (1.25·(50−|ω|) + 1.03·|ω|)/50
    target = bias + Σpairs (wL.τreact − wR.τreact)/(AngInertia·ω)
    bias = 0.1·(9·bias + clamp(target, 1/maxB, maxB))
ωn = ω − dt·T/(I + dt·AngInertia)
if canStop and sign(ωn) != sign(ω): ωn = 0
if engine attached:
    e = −g·ωn
    if e < 0: ωn = 0
    elif e > engine.ωmax: ωn = −engine.ωmax/g ; engine.ω = engine.ωmax
    else: engine.ω = e
ω = ωn
wheels[even].ω = bias·ω ; wheels[odd].ω = ω/bias   (single wheel: ω)
for w in wheels: forces(w)
```

### `contact(w)`

```
M = (w.SteeringLimit != 0) ? RotY(w.steerAngle) about the wheel's inner edge : I ; M.pos = w.center
W = M·worldMatrix ; up = W.row1
a = w.Limit + PROBE_ABOVE ; len = w.Extent + w.r + a
hit = raycast(W.pos + a·up → W.pos − (w.Extent + w.r)·up)
if not hit or up·hit.n < MIN_UP_DOT_N:
    w.grounded = false ; suspension(w, −w.Extent, false, 0) ; w.N = w.Fs ; return
n = hit.n ; p = hit.point
rear = normalize(W.row0 × n) ; lat = n × rear      # if |W.row0 × n|² < 0.02: treat as not grounded
v = body.velocityAt(p)
w.vLat = v·lat ; w.vFwd = −v·rear ; w.vN = v·n
material: matFric, matDrag, depth (sinks toward mat.depth at |ω|·r·0.1 /s while majorSlip), bump
if |n.y| < STEEP_NY: matFric = 0
matFric *= WeatherFriction ; if matFric < 1: CarFrictionHandling remap (AI: 2.0 while touching the player → halves grip)
x = (a + w.r) − ((len·hit.t − bump) + depth)
suspension(w, x, true, up·n) ; w.N = w.Fs
```

### `suspension(w, xin, hit, cosθ)`

```
xo = w.x
if hit: w.x = max(xin, −Extent) ; atExt = xin < −Extent
else:   w.x = −Extent ; atExt = true
w.xdot = clamp((w.x − xo)·invdt, ±XDOT_CLAMP)
prog = 1 + k2·w.x
w.Fs = (cs·w.xdot + ks·w.x)·prog + w.L
if w.Fs < 0:
    w.Fs = 0 ; w.x = (xo·cs − dt·ks·Extent)/(dt·ks + cs) ; w.xdot = (w.x − xo)·invdt ; w.D = 0 ; return
if w.x > Limit:
    j = (w.vN < 0) ? −w.vN / (nᵀ·Winv(p)·n) : 0          # body inverse mass at p
    Fb = invdt·j·BUMP_STOP_K
    body.pushOut((w.x − Limit)·cosθ·n)
    w.x = Limit ; w.xdot = clamp((Limit − xo)·invdt, ±XDOT_CLAMP)
    w.Fs = (cs·w.xdot + ks·Limit)·(1 + k2·Limit) + w.L + Fb
    w.D = (w.vN != 0) ? −Fb/w.vN : 0 ; return
w.D = atExt ? 0 : prog·cs + dt·ks/cosθ
```

### `forces(w)`

```
if not w.grounded: zero dLat, dLong, FLat, FLong, τreact, slip visuals ; return
if w.D > 0: body.addImplicit((w.x < Limit ? ks·w.xdot·dt : 0)·n, p, w.D·n·nᵀ)     # 01
sLat  = ratio(w.vLat, w.vFwd) ; vSlip = w.ω·w.r + w.vFwd ; sLong = ratio(vSlip, w.vFwd)
    where ratio(a, b) = a == 0 ? 0 : (|a| <= |b| ? a/|b| : sign(a))
μs = StaticFric·matFric ; uL = sign(vSlip)·N/kLong ; uT = sign(vLat)·N/kLat
per axis X ∈ {Long (sLong, vSlip, dLong, uL), Lat (sLat, vLat, dLat, uT)}:
    step = dt·v_X ; μX = μs ; limX = μs·uX
    if |sX| <= opt or not holds(dX, limX, step): μX = curve(sX) ; limX = μX·uX
μc = max(|sLong|,|sLat|) < opt ? μs : (dominant axis's μ) ; cap the other axis's lim at μc·u
r = |w.ω·w.r|·dt·RELAX_K
per axis: cand = dX + step
    if (step >= 0 ? cand <= limX : cand >= limX): dX = cand ; velX = step ; stickX = true
    else: dX = step >= 0 ? max(dX − r, limX) : min(dX + r, limX) ; velX = 0 ; stickX = false
FLat = −kLat·dLat − velLat·invdt·cLat ; FLong = −kLong·dLong − velLong·invdt·cLong
Fmax = μc·N
if FLat² + FLong² > Fmax²:
    if (stickLong or |sLong| <= opt) and (stickLat or |sLat| <= opt): scale (FLat, FLong) to Fmax
    else: (FLat, FLong) = −Fmax·(vLat, vSlip)/|(vLat, vSlip)|
w.τreact = FLong·w.r
drag (water only): DLat = −|vLat|·DragLat·N·vLat·matDrag ; DLong = −(1+depth)·|vFwd|·DragLong·N·vFwd·matDrag
F = Fs·n + (FLat + DLat)·lat − (FLong + DLong)·rear
body.force += F ; body.torque += (p − body.pos) × F

curve(s): s=|s| ; f = μs·s·(2·opt − s)/opt² ; return s <= opt ? f : max(f, μk)    (μk = SlidingFric·matFric)
holds(d, lim, step): step > 0 ? (d < lim and lim − d >= step) : (d > lim and lim − d <= step)
```

### `engine.update()`, `transmission.update()`

See [03-powertrain.md](03-powertrain.md#vehengineupdate-0x4d8f20) and
[§ Transmission](03-powertrain.md#vehtransmissionupdate-0x4cf5f0) —
both are short enough to port line by line from there.

### `aero.update()`

```
ωl = Rᵀ·body.ω
for a in x,y,z:
    α = −sign(ωl[a])·AngCDamp[a] − ωl[a]·AngVelDamp[a] − |ωl[a]|·ωl[a]·AngVel2Damp[a]
    if |α|·dt > |ωl[a]|: α = −ωl[a]·invdt
    if |body.ω[a]| < 1: α *= |body.ω[a]|                 # world component — quirk
    τl[a] = α·body.inertia[a]
body.torque += R·τl
body.force  += −Speed·Drag·body.v − Speed²·Down·R.up
```

### `gyro.update()`

```
allDown = (count of wheels grounded this step) == 4      # integer division — quirk
ωd = drive train ω
if Drift > 0:  body.torque += I.y·Drift·s·|s|·allDown·ωd · R.up        (s = SSS·SteeringInput)
if |Handbrake| > 0.01 and (Spin180 > 0 or Reverse180 > 0):
    k = ωd >= 0 ? Reverse180 : Spin180
    body.torque += I.y·k·SteeringInput·allDown·ωd · R.up
if |Brake| > 0.01 and (Pitch > 0 or Roll > 0) and not allDown:          # 0 in retail
    w = Brake²
    body.torque += I.x·Pitch·w·R.back.y · R.right − I.z·Roll·w·R.right.y · R.back
```

## Inputs

### Human ([05](05-player-input.md#replication-recipe))

```
once per frame, before physics:
    t = clamp(speed, SpeedBaseLow, SpeedBaseHi) / (SpeedBaseHi − SpeedBaseLow)          # quirk: no −Lo
    DeltaOut, DeltaIn, Exponent = lerp(asnode Lo, Hi, t)
    keyboard: target ∈ {−1, 0, +1} (left wins)
              rate = (|target| >= |s| and sign(s) == sign(target)) ? DeltaOut : DeltaIn
              s = approach(s, target, rate·dt) ; steer = sign(s)·|s|^Exponent
    throttle, brake, handbrake ∈ {0, 1} for keys (pedals swapped in auto-reverse)
    quantise: steer → trunc(·127)/127, others → trunc(·255)/255
    automatic + AutoReverse: gear ≥ 2, speed < 5, brake > 0.8, throttle < 0.1 → swap pedals, reverse
                             gear 0 and (swapped) throttle < 0.8 → unswap, first
    post-race: throttle 0, brake 1, steer −1 ; dead car: all 0
    SpeedMPH < 4 and throttle == 0 → handbrake = 1
```

### AI ([06](06-ai-input.md))

```
steer = clamp(1.8992 · bearingToNextNode, ±1) ; handbrake = speed > 30 and |raw| > 1
throttle = maxThrottle, brake 0 — or throttle 0, brake = clamp(v(v−vc)/(23.76·d), 0, 1) when > threshold,
           and then angularMomentum *= 0.85 that frame
CarFrictionHandling = touching the player ? 2 : 1
backup (vehStuck state 2): reverse, throttle 0.85, steer = clamp(−2.857·bearing), ≤ 66 frames, momenta ×0.25 on exit
police: linearMomentum *= 1.03 per frame while throttle == 1 and speed < 50
```

## Known frame-rate dependence

Several terms are per-step rather than per-second, so the original's
feel depends on its step length ([01-rigid-body.md](01-rigid-body.md)):
the drivetrain's `dt·AngInertia`, the 10%-per-step diff smoothing, the
bump stop's quarter-speed-per-step, the `dt·ks/cosθ` implicit term, the
bristle relaxation (per metre, so fine) and the finite-difference
suspension rate. The AI's momentum factors (×0.85 while braking, ×0.95
totalled, ×1.03 police boost) and its 66-frame backup are **per frame**.
Run the port at the original's step — one 1/60 s step per 60 Hz frame —
to match it.
