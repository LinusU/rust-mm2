# `camTrackCS` — the original chase camera, recovered from `Midtown2.exe`

What the retail executable does with a `tune/camera/<id>_{near,far}.camtrackcs`
record, read from the disassembly (UNK-36 in
[`../original-rules.md`](../original-rules.md)). The authored side (file
format, field set, retail value ranges) is in
[`../../crates/mm2_formats/src/camtrack.rs`](../../crates/mm2_formats/src/camtrack.rs);
this note is the runtime half.

## Method and confidence

- `retail/Midtown2.exe`, x86-32, image base `0x400000`, disassembled with
  `objdump -d -M intel` (no decompiler). Addresses are this executable's;
  they differ from mm2hook's by the per-region shifts described in
  [`vehicle-physics/README.md`](vehicle-physics/README.md).
- Field names are the parser tokens the class registers (read out of the
  `.rdata` strings the registrar pushes), so the **offset ↔ name** table
  below is verified_original. Meanings are read from the arithmetic;
  where a paragraph depends on guessing what an engine global *is* it says
  *inferred*.
- Constants quoted are the `.rdata` floats at the cited addresses.
- Nothing was run: the claims are about what the code computes. Unit
  statements (m/s, rad/s) follow from the cited reads of fields documented in
  [`vehicle-physics/04-chassis-and-assists.md`](vehicle-physics/04-chassis-and-assists.md).

## Class layout (verified_original)

`camTrackCS` has vtable `0x5b3ec4`; its constructor is `0x51d750`, its
class-name getter (`"camTrackCS"`, `0x5d328c`) `0x51fd40`, its field
registrar (vtable slot 6) `0x51fa60`, and its per-frame `Update` (slot 2)
`0x51db30`. The parent `camAppCS` registrar is `0x5229b0`. Object offsets:

| Off | Field | Class | Default (ctor `0x51d750`) |
| --- | --- | --- | --- |
| `0x94` | `TrackTo` (vec3) | camAppCS | |
| `0xa0` | `ApproachOn` | camAppCS | 1 |
| `0xa4` | `AppAppOn` | camAppCS | 1 |
| `0xa8` | `AppRot` | camAppCS | 30 |
| `0xac` | `AppXRot` | camAppCS | 10 |
| `0xb0` | `AppYPos` | camAppCS | 5 |
| `0xb4` | `AppXZPos` | camAppCS | **live value**, rewritten every frame (below) |
| `0xb8` | `AppApp` | camAppCS | 0.7 |
| `0xbc` | `AppRotMin` | camAppCS | 0.01 |
| `0xc0` | `AppPosMin` | camAppCS | 0.25 |
| `0xc4` | `LookAbove` | camAppCS | **live value**, rewritten every frame |
| `0xcc` / `0xd0` | `MaxDist` / `MinDist` | camAppCS | 11 / 7.93 |
| `0xd4` | `LookAt` | camAppCS | 1 |
| `0x10c` | `ReverseOn` | camTrackCS | 1 |
| `0x114` | `Offset` (vec3) | camTrackCS | (0, 1.9, 7.7) |
| `0x120` | `CollideType` | camTrackCS | 0 |
| `0x124` | `MinMaxOn` | camTrackCS | 1 |
| `0x128` | `TrackBreak` | camTrackCS | 0 |
| `0x12c` / `0x130` | `MinAppXZPos` / `MaxAppXZPos` | camTrackCS | 1.8 / 12 |
| `0x134` / `0x138` | `MinSpeed` / `MaxSpeed` | camTrackCS | 5 / 35 |
| `0x13c` / `0x140` | `AppInc` / `AppDec` | camTrackCS | 15 / 10 |
| `0x144` | `MinHardSteer` | camTrackCS | 0.8 |
| `0x148` | `DriftDelay` | camTrackCS | 0.3 |
| `0x14c` | `VertOffset` | camTrackCS | 0.6 |
| `0x150` / `0x154` / `0x158` | `FrontRate` / `RearRate` / `FlipDelay` | camTrackCS | 0.55 / 0.5 / 0.5 |
| `0x15c` / `0x160` / `0x164` | `SteerOn` / `SteerMin` / `SteerAmt` | camTrackCS | 0 / 0.5 / 3.5 |
| `0x168` / `0x16c` / `0x170` | `HillMin` / `HillMax` / `HillLerp` | camTrackCS | −0.56 / 0.56 / 0.05 |
| `0x174` | `RevDelay` | camTrackCS | 2 |
| `0x178` / `0x17c` | `RevOnApp` / `RevOffApp` | camTrackCS | 2 / 4 |
| `0x180` | (not parsed) collision margin | camTrackCS | 0.33 |

(Defaults are what the constructor `0x51d750..0x51da57` stores; a record
that omits a field keeps them. `AppRot` etc. are the parent's fields, set
by the same constructor.) `0x108` is the owning vehicle's player/driver
object, whose `+0xb8` is the car simulation (`vehCarSim`); `0x90` is the
car's world matrix, `0x1c` the current camera matrix, `0x4c` the desired
camera matrix (its position row is `0x70..0x78`), `0x40..0x48` the current
eye.

## Per-frame pipeline (verified_original order)

`Update` (`0x51db30`) calls, in order:

1. `0x51dc50` — reverse-gear detection (`PreUpdate`).
2. `0x51dec0` — ground-slope "hill" bias (`HillMin/Max/Lerp`).
3. `0x51e3e0` — the desired eye / aim point, TrackBreak/air state.
4. `0x51eb20` — speed-dependent follow rate (`MinSpeed..MaxSpeed`, `AppInc/Dec`).
5. `0x522040` — `camAppCS::Approach`: eye follows the desired position.
6. `0x51eca0` — `MinMaxOn != 0`: vertical ground/ceiling clamp.
7. `0x51eeb0` — `CollideType` 1 / 2: occlusion pull-in.
8. camera-matrix publish (virtual `+8` on the sub-object at `0x1bc`).

Frame time everywhere is `datTimeManager::Seconds` (`0x5cd820`, the
engine's fixed 1/60 s step).

### 1. `MinSpeed`/`MaxSpeed` drive the follow rate, not the boom (verified_original)

`0x51eb20`: with `speed` = the car's `|forward velocity|` in **m/s**
(`carsim+0x248`, copied to `+0x240` by `0x51dc50`):

```text
t       = clamp((speed − MinSpeed) / (MaxSpeed − MinSpeed), 0, 1)   (t = 0 if MaxSpeed ≤ MinSpeed)
target  = MaxAppXZPos + (MinAppXZPos − MaxAppXZPos) · t            (target = MaxAppXZPos when MinAppXZPos = 0)
target  = 20  if ReverseOn == 0 and carsim+0x304 == 0              (gear 0 = reverse)
AppXZPos ← AppXZPos ± AppInc·dt (rising) or ± AppDec·dt (falling), clamped so it never passes target
```

So `AppXZPos` is a live, slewed **follow rate (1/s)**: loose at speed
(`MinAppXZPos`, 0.65–5 in the stock roster), tight when stopped
(`MaxAppXZPos`, 8–29.2), eased in at `AppInc` and out at `AppDec` per second.
`MinSpeed`/`MaxSpeed` are **m/s** (stock: 0–5 and 6.95–35). Neither field
extends the boom length. This confirms the Open1560-MM1 reading recorded
in UNK-36 and refutes DSN-56's "interpolates rest→`MaxDist` on planar
speed".

### 2. The eye follows the desired position (verified_original)

`0x522040` (`camAppCS::Approach`, only when `ApproachOn != 0` and not the
reset frame; otherwise the current matrix is copied from the desired one):

- `TrackTo` is **car-local**: the aim point `T` (`0xe4`) is `TrackTo ·
  carMatrix` (rows 0–2 and the translation row of `*(0x90)`).
- Each axis of the eye moves toward the desired position (`0x70..0x78`)
  through `0x522860`: distance `d = |target − eye|`; a soft knee
  `d ← d²/AppPosMin` when `d < AppPosMin`; with `AppAppOn` the per-axis rate
  state is low-passed `s ← s + (d − s)·AppApp`; the eye then steps
  `s · rate · dt` toward the target, clamped not to overshoot. `rate` is
  `AppXZPos` for X and Z and `AppYPos` for Y.
- After the approach, if `MaxDist ≠ 0` (`0x522630`) the eye–aim distance is
  hard-clamped into `[MinDist, MaxDist]` (skipped when `MaxDist ≤ MinDist`
  or the engine's `0x522b60` early-out is true; its meaning is unresolved).
  **This clamp is not gated by `MinMaxOn`.**
- The orientation matrix is built from the eye toward `T + (0, LookAbove, 0)`,
  with Euler angles approached at `AppRot` (yaw) / `AppXRot` (pitch),
  wrapped to ±π, `AppRotMin` as their knee, and `LookAt` blending the look
  direction; `LookAt` is 1.0 in 47 of the 49 stock records, 0.993 in two.
  The orientation half is *inferred* from the arithmetic (it was read less
  closely than the position half).
- `LookAbove` is **not** read from the record at runtime: `0x51e3e0`
  overwrites it each frame with `(Offset.y − 0.8) · VertOffset`.

### 3. The desired eye (verified_original arithmetic, inferred naming)

`0x51e3e0`. Let `b` be the car's +Z (rearward) axis flattened onto the
ground plane and normalised, `r = up × b` (so `+X` is the car's right when
the car points along −Z), and `up` the engine's world up. The desired eye is

```text
eye = T + r · Offset.x + b · max(Offset.z, 0.01) + (0, Offset.y, 0)
```

with `T` the unbumped aim point. **`Offset` is therefore a yaw-only,
gravity-aligned frame offset added to the aim point**; car pitch and roll
never tilt the boom. `+Z` is behind the car, `+Y` up, `+X` right. After the
eye is placed `T.y` is raised by 0.4 m (`0x5af42c`) for the look target.

States (`0x190` grounded, `0x194` broken, `0x198` re-anchor, from
`0x51dc50`/`0x51dec0`): `grounded` toggles with ≥ 3 wheels in contact (via
`0x4cbaf0`) for more than 0.1 s, in air for more than 0.1 s the reverse.
`TrackBreak == 2` always, or `== 1` when the body's up axis `y < 0.35`
(`0x5b1c60`; roughly > 70° of tilt), sets "broken" and skips the rest of
the pre-update. While airborne the eye keeps its offset from the previous
aim point instead of re-deriving it from the car's heading. The exact role
of the `0x2250000` (`0x5b3f08`) speed² test is unresolved (see below).

### 4. Hill bias (verified_original arithmetic)

`0x51dec0` builds the mean ground contact point of the front and rear wheel
pairs (`carsim+0x584/0x7f0` front, `+0xa5c/0xcc8` rear, averaging both
wheels when both touch), takes the slope direction between them, low-passes
it with `HillLerp` (snapping when the reset flag `0xc8` is set), measures its
elevation angle `θ` from the horizontal, shapes it through a
`sin`/`cos` ease on `|θ|/(π/4)`, and stores a pitch bias at `0x1b4`:
`−HillMin·(1−2u)` for `u < 0.5`, `−HillMax·(2u−1)` for `u ≥ 0.5`
(`u` the eased value). `0x51e3e0` adds this bias to the camera's
height/pitch slot (`[0x184]+0xc`) and fades it out as the reverse swing
approaches π (below). `HillMin/HillMax` are radians in the stock data
(−1…0.007 / up to 1).

### 5. Reverse swing (verified_original arithmetic)

`ReverseOn` (1 by default and in the 38 stock records that author it; the
other 11 omit it and keep the default 1): when `carsim+0x304`
(`Transmission.CurrentGear`, 0 = reverse) is 0, the car is not handbraking
hard and the throttle exceeds 0.05 (`carsim+0x2bc`, `0x5afc1c`), a timer
counts to **2.0 s (hard-coded, `0x5af578`)**, then a `reversing` flag
(`0x248`) is set and a side sign (`0x250` = −1 if steer input > 0.1, else +1)
chosen. The camera then orbits the aim point by an angle ramping toward
`±π` at **`RevOnApp` rad/s**, and back to 0 at **`RevOffApp` rad/s**
once the car leaves reverse (the angle is rotated about `up` with `0x4bce50`).
`ReverseOn == −1` pins the angle at π whenever the flag is set. The
blend factor `|angle|/π` scales the hill bias out.
**`RevDelay` (`0x174`) is never read** — the 2.0 s is a constant, so editing
the field changes nothing in the retail build.

### 6. `MinMaxOn` is a vertical clamp (verified_original)

`0x51eca0` (when `MinMaxOn ≠ 0`; `0x51db30` passes the pre-update camera
matrix by value): casts two world-collision segments through the engine's
intersect call (`0x468e30`) at the eye's X/Z, vertical, **±5 m**
(`0x5af418`) around the eye's Y — first the upward one, then the downward
one. An upward hit counts when its returned normal component (`+0x10` in
the result block) is `< 0.7` and sets an upper bound `hit.y − 0.5`; a
downward hit counts when that component is `> 0.7` (a floor-like surface) and
sets a lower bound `hit.y + 0.5`. `eye.y` (`+0x44`) is then clamped to the
upper bound, then raised to the lower bound (the floor wins when they
cross). So `MinMaxOn` keeps the eye above the road and below overhead
structure; it is **not** a gate on `MinDist`/`MaxDist`. The result block's
field layout is read from use only (*inferred*): `+0x04` is taken as the hit
height and `+0x10` as the normal component tested against 0.7.

### 7. `CollideType` (verified_original)

`0x51eeb0`: **1** and **2** are the only values with code; 0 and any other
value do nothing. Every stock record authors **1**.

- **1 — hard pull-in.** Five segments from the aim point toward the eye
  (the centre line plus four offset by the camera's right/up axes scaled
  from the live camera's frustum globals `0x682104+0x12c/0x134/0x138`,
  `+0.33`), each of length `MaxDist`. The nearest hit whose normal faces
  the camera (`dot < −1e−5`) gives a distance `d`; if
  `d·MaxDist + near − margin` (`margin` = `0x180`, 0.33) is less than the
  current eye–aim distance, the eye is moved **along the aim→eye line to that
  distance, instantly**. This is a pull-in, never a clip or a re-position.
- **2 — smoothed pull-in.** One segment aim→eye. On a hit, the squared
  distance state `0x264` eases to the hit at ±30·dt per second (`0x5b0a18`)
  and the eye is placed at `T + dir·√state`; with no hit the state relaxes
  back out the same way until the flag `0x258` clears. Not authored by any
  stock record.

### Fields with no reader in `camTrackCS` code (verified_original, scope-limited)

Searching the member functions above and everything they call for
non-store reads of these object offsets finds none for `MinHardSteer`
(`0x144`), `DriftDelay` (`0x148`), `FrontRate` (`0x150`), `RearRate`
(`0x154`), `FlipDelay` (`0x158`), `SteerOn` (`0x15c`), `SteerMin`
(`0x160`), `SteerAmt` (`0x164`) and `RevDelay` (`0x174`). They are
parsed, defaulted and saved but the retail camera never consults them
(`SteerOn` is 0 in all 49 stock records anyway). The search covered
`0x51d000..0x525000`; a read through a computed pointer elsewhere is not
excluded.

## Retail records, by the numbers (verified_original, 49 `camtrackcs`)

| Field | Stock values |
| --- | --- |
| `CollideType`, `MinMaxOn`, `ApproachOn`, `AppAppOn` | 1 in all 49 |
| `SteerOn` | 0 in all 49 |
| `TrackBreak` | 0 ×38, 1 ×11 |
| `ReverseOn` | 1 ×38; the other 11 omit it (constructor default 1) |
| `MinAppXZPos` / `MaxAppXZPos` | 0.65–5 / 8–29.2 |
| `MinSpeed` / `MaxSpeed` | 0–5 / 6.95–35 (m/s) |
| `AppInc` / `AppDec` | 1–15 / 3.35–10 |
| `HillMin` / `HillLerp` | −1…0.007 / 0.05–1 |
| `VertOffset` | −0.89…1 |
| `MinDist` / `MaxDist` | 0–9.43 / 4.25–21.9 |

## `CameraNear` is overwritten after every load (verified_original, UNK-37)

The `CameraNear` token is registered by the shared camera base class
(registrar `0x521e80`: `BlendTime` `+0x80`, `BlendGoal` `+0x84`,
`CameraFOV` `+0x88`, `CameraNear` `+0x8c`, `CameraFar` `+0x90`). The
loader, vtable slot 10 (`0x4a1110`, shared by `camPovCS` and
`camTrackCS`), runs the registrar's parse (slot 6) and, **only when the
parse succeeds**, calls vtable slot 7 (`call [eax+0x1c]` at `0x4a119a`)
before returning. Each camera class's slot 7 is a one-store method:

| Class | Vtable | Slot 7 | Effect |
| --- | --- | --- | --- |
| `camPovCS` | `0x5b3e80` | `0x51d6f0` | `mov [ecx+0x8c], 0x3dcccccd` — `CameraNear = 0.1` |
| `camTrackCS` | `0x5b3ec4` | `0x51dad0` | `mov [ecx+0x8c], 0x3f000000` — `CameraNear = 0.5` |

So the authored `CameraNear` of a loaded record is never used: `camPovCS`
always renders with a 0.1 m near plane, `camTrackCS` with 0.5 m. The
constructor default (`0x51d3f0`, `[+0x8c] = 3.0`) is also overridden by the
same virtual once a file loads. The near plane reaches the renderer
through `0x4b1630` (FOV, near, far); the per-frame readers are
`0x51fed8`, `0x520031` and `0x5217d9` (the blend, which interpolates
`+0x88`/`+0x8c` between two cameras).

Retail sweep (`mm2_app` test `retail_authored_camera_near_values_are_what_the_loader_overwrites`):
44 `camPovCS` records parse — `CameraNear` 0.1 ×29, 0.391–1.726 ×11 (non-`_dash`
records: `vppanoz`, `vpcoop`, `vpcoop2k`, `vpcab`, `vpcop`, `vpmustang99`,
`vpauditt`, `vpcaddie`, `vpsemi`, `vppanozgt`, `vpddbus`), and **3.0 on four
`_dash` records** (`vpcentury`, `vpcoop2k`, `vpsemi`, `vpvw_dune`) — all
run at 0.1, which is why those dashes were never clipped in the retail
game. Three `*_pov.campovcs` files hold a `PovCamCS` block (a different
class, not covered here). 49 `camtrackcs`: 0.5 ×44, 1.0 ×5 (`vpbug_ind`,
`vpbus_ind`, `vpdune`, `vpeagle_far`, `vpsemi_far`) — those run at 0.5.

What stays *designed*: the rear-view mirror strip's near plane (the
original's mirror presentation is unrecovered, DSN-50/UNK-29), which keeps
the authored `camPovCS` value because a high clip usefully hides the
vehicle's own bodywork.

## Still unknown

- The airborne/broken-state machine at `0x190/0x194/0x198`: the speed²
  threshold `2 250 000` (`0x5b3f08`) is compared against `carsim+0x60..0x68`
  squared; what that vector is (velocity in other units vs. a position) is
  not resolved here.
- The helper at `0x522b60` that can veto the `MinDist/MaxDist` clamp.
- `LookAt` (`0xd4`) mixing beyond "blend factor, 1 = full look-at".
- How `camTrackCS` is *selected* (near ↔ far swap, blend `BlendTime`/`BlendGoal`
  between lenses); those two records' fields are not registered by
  `0x51fa60` or `0x5229b0` and belong to the camera manager.
- The meaning of `carsim+0x304` as "gear" rests on the field layout in
  [`vehicle-physics/03-powertrain.md`](vehicle-physics/03-powertrain.md)
  (transmission block, `CurrentGear` at `+0x24`); the sum was not
  re-derived here.
