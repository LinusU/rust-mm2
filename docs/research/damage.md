# Vehicle damage and recovery records

Research for F05-A.1 — the authored-data leg of vehicle damage and
recovery. Everything below is measured on the retail installation at
`/Users/linus/coding/rust-mm2/retail` (`mm2-inspect damage <install>`,
2026-09-22). Format semantics that are not directly measured are
marked **inferred** or **unverified**; nothing here is an
original-behavior claim.

## `tune/vehicle/<id>.vehcardamage`

One flat `vehCarDamage` tune block per player vehicle. 20 records on
retail — every listed car plus `vpvwcup`; `vpmoonrover` ships none
(the undocumented secret car, UNK-3). All 20 carry the same 38
fields; 7 add a 39th, `MirrorPivot` — vpbullet, vpbus, vpcaddie,
vpcop, vpddbus, vpdune, vpmustang99 (authored 0 on all seven).

Damage model fields:

| Field | Retail range | Reading |
| --- | --- | --- |
| `MaxDamage` | 187 500 (`vpcoop`/`vpcoop2k`) – 3 281 300 (`vpsemi`) | Accumulated bound at which the vehicle is destroyed — the DMG-1 meter's empty end. Tracks vehicle mass, so the unit is impulse-scale (**inferred**, UNK-13). |
| `MedDamage` | 80 000 (`vpcoop`/`vpcoop2k`) – 2 343 800 (`vpsemi`) | Mid-tier bound; `MedDamage < MaxDamage` on every record (the meter's yellow band / damaged-visual tier). |
| `ImpactThreshold` | 1500 on 19 records; 100 on `vpcaddie` | Impacts at or below this do not damage — the authored floor that keeps resting contact, curb taps and suspension loads out of the accumulator (F05-AC01). |
| `RegenerateRate` | 0 on every record | Damage healed per second — the authored channel DMG-4's C&R healing would drive; no stock car regenerates. |
| `TextelDamageRadius` | 0.4 (`vpcaddie`/`vpcentury`) – 20.0 (`vp4x4`/`vppanozgt`) | Decal/deformation radius around an impact point (**inferred**). |
| `SmokeOffset` / `SmokeOffset2` | car-space pivots | Two smoke-emitter attachment points; `DoublePivot` (1 on vpddbus/vppanoz/vppanozgt, 0 elsewhere) and `MirrorPivot` (0 where authored) gate their use (**inferred**). |

The remaining ~30 fields are a flat particle spec sharing the root
block — the same vocabulary as `dgBangerData`'s `BirthRule`
(`Position/Var`, `Velocity/Var`, `Life/Var`, `Mass/Var`, `Radius/Var`,
`Drag/Var`, `Damp/Var`, `DRadius/Var`, `DAlpha/Var`, `DRotation/Var`,
`InitialBlast`, `SpewRate`, `SpewTimeLimit`, `Gravity`,
`TexFrameStart/End`, `BirthFlags`) plus `LifeVar`, `DampVar`,
`Height`, `Intensity` and `Color` extras. It is decoded as
`DamageEffect` but embedded flat rather than as a nested sub-block.
`Color` is a packed word preserved verbatim (retail authors
`-167772161` — reads as a negative 32-bit ARGB-ish value). What the
original emits with it is unverified (UNK-13).

Decoder: `mm2_formats::veh::VehCarDamage::from_tune` — expected-root
check, typed scalars/vectors, unknown fields into `warnings`,
`validate()` reporting `DamageIssue::{NonFinite, Negative,
MedAboveMax}`.

## `tune/vehicle/<id>.vehstuck`

One flat `vehStuck` block — 20 retail records, uniform 6-field set:
`Turn` (≈ π, 3.141593, on 10 of 20 — `vpford` 3.098593; 1.57 on 6;
`vpbus` 2.064593, `vpcentury`/`vpsemi` 2.0, `vpddbus` 0.74),
`Rotation` (0 on all), `Translation` (≈ 0.1; `vpcoop` 0.164),
`TimeThresh` (2.0 on 6 records, ~1.0 on the rest — `vpddbus`
1.1714), `PosThresh` (1.25 on all) and `MoveThresh` (1.75 on all —
above `PosThresh` on every record, so not a sub-bound of it). The
names read as a stuck detector: an angular/linear test over a time
window with position and movement bounds — all **inferred** from
names; the original's test combination is unverified (UNK-13).
Decoder: `VehStuck::from_tune`; `validate()` enforces non-negative
time/position/movement bounds while allowing signed angular fields.

MM2Hook's recovered `vehStuck` struct
(`src/modules/vehicle/stuck.h`, 2026-09-22) supports the detector
reading structurally: alongside the authored fields it keeps an
`m_State` machine, an accumulating `m_StuckTime`, an
`m_LastImpactPos` anchor, squared copies of `m_PosThresh`/
`m_MoveThresh` (`ComputeConstants` — both bound distance), and an
`m_InertialCSPtr` pose source. `Update()` is a binary thunk, so the
exact test combination stays inferred — see the F05-B.2 runtime
section below for the implemented interpretation.

## `tune/vehicle/<id>.vehgyro`

One flat `vehGyro` block — 21 retail records (`vpvwcup_angel` ships
one despite having no other damage records). `Drift`, `Spin180`,
`Reverse180` on every record; `Roll` and `Pitch` on 17 of 21 (absent
on vpbug, vpcab, vpford, vpvwcup_angel; authored 0.0 where present).
The names read as assisted air-control/righting rates — semantics
**inferred** (UNK-13). Decoder: `VehGyro::from_tune`; `Roll`/`Pitch`
decode as `Option<f32>` — absent stays `None` (never an invented
zero), while a present-but-non-numeric value is a decode error, the
same rule `MirrorPivot`'s optional integer follows.

MM2Hook's recovered `vehGyro` struct
(`src/modules/vehicle/gyro.h`, 2026-09-22) carries the same five
fields plus three asNode feature gates — `Spinable` (0x10000),
`Driftable` (0x20000) and `Rightable` (0x40000) — which read as
{Spin180, Reverse180} / {Drift} / {Pitch, Roll} groupings. `Update()`
is a binary thunk, so the application semantics stay unrecovered
(UNK-13) — see the F05-B.4 runtime section for the implemented
designed reading.

## Breakaway parts (authored inventory)

`geometry/<id>.pkg` `BREAK<NN>` chunks are the intact representation
of detachable panels; the runtime fragment is bound by name through
either the pkg chunk or a `geometry/<id>_break<NN>.mtx` transform, and
the physics/audio record is `tune/banger/<id>_break<NN>.dgbangerdata`
(the same `<base>_break<N>` → `BREAK<N>` convention the banger audit
resolves fragment references through — WLD-15).

Naming measured on retail: `BREAK0`–`BREAK3` are corner pieces and
`BREAK01`/`BREAK12`/`BREAK23`/`BREAK03` the panels *between* corners
(**inferred** — the pairs read as edge indices). 14 catalog vehicles
ship breakaway parts; the largest sets are vpsemi, vpftruck and
vplafrance (4 corners + 2 edges each), vpcoop (2 corners + 4 edges).
`mm2-inspect damage` inventories all three sources per vehicle and
flags records binding to no geometry — retail carries 10 such dead
authored fragments (vpeagle `break0/1` — no pkg at all;
vpvw_cup/vpvwcup `break01/02`; vpvwcup_angel all four) — findings,
not failures.

`ImpulseLimit2` on the vehicle fragment records is measured
**`Mass × constant`** (2026-09-22, `mm2-inspect banger`): every
part on every catalog vehicle reads ≈31.25 m/s of approach speed
(`limit / mass` — 166/5.3 on vpcoop's corners, 4606/147.4 on
vpsemi's rear) except a "never detach" class at ≈2500 m/s
(vpftruck all six parts, vp4x4 `break0`, vpvw_cup `break2/23`,
vpvwcup_angel `break2/23`). Prop fragment records show the same
structure with more constants (≈500, ≈800, ≈2500 — see the banger
census). The field reads as `mass × detach_speed`, not a free-form
impulse; what the original compares it against stays UNK-22.

## Shared contract (`mm2_game::damage`)

`DamageSpec` distils the authored bounds (`impact_threshold`,
`med_damage`, `max_damage`, `regenerate_rate`) from `VehCarDamage`.
`DamageState` is the authority-owned accumulator: `apply` rejects
non-finite/non-positive/at-or-below-threshold severities outright,
saturates at `max_damage`, and returns the resulting tier
(`DamageTier::{Intact, Damaged, Disabled}` — the DMG-1 green/yellow/
empty bands). `tick` advances the authored regeneration channel
(mechanism only — whether a session allows healing is DMG-4 mode
policy). `repair`/`reset` are named authority operations; the
session's role check decides who may call them (F05 req 6).
`disabled_outcome(mode)` maps the documented RACE-5/DMG-2
consequences: `RestartEvent` for Blitz/Checkpoint/CrashCourse (the
crash-course mapping is designed — F21 territory), `PenaltyReset` for
Circuit, `FreeReset` for Cruise (designed — the help names no
free-roam consequence).

The severity→damage *conversion* is not recovered — `apply` consumes
the linear impulse estimate the impact pipeline reports
(`approach_speed × striker_mass`) as a documented designed policy
(UNK-13 stands). Banger activation shares only the deepest-contact
severity and striker-mass resolution: since F04-C.5 it gates on
striker kinetic energy `½·m·v²` against `ImpulseLimit2` (DSN-10,
docs/research/banger.md).

## Runtime application (F05-B.1)

`VehicleDamage` is a component spawned on the player and every AI
opponent whose `vehcardamage` decoded — authored absence stays
undamageable, never a fabricated spec. It wraps `DamageState` behind
a monotonic `ImpactId` watermark: re-delivered or out-of-order
impacts return `DamageVerdict::Duplicate` instead of double-applying
(F05-AC06, on top of the upstream pair dedup).

`mm2_app::damage::apply_impact_damage` (FixedLast, authority-gated,
Playing-only) consumes the deduplicated `ImpactEvent` stream. Per
impact, each participant's delivered impulse is `severity ×
other_mass` — the vehicle's own mass when the other side is the
static world or has no resolvable mass — then one `DamageEvent`
(object, generation, tick, impact id, applied severity, total, tier)
emits per accepted application. `DamageReport` counts applied/
rejected/duplicate/disabled/recovered — the headless smoke record's
`dmg=` field. Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`):
sf/london `--frames 600` scripted cruises each record
`dmg=2a/0d/0r rej=1 dup=0`.

`resolve_disabled` enforces `disabled_outcome` on `Disabled` events:
Cruise resets the player to the spawn point through `ResetVehicle`
(trailers included) and repairs; Circuit resets in place and adds
`DISABLED_PENALTY_TICKS` (5 s, designed — RACE-5 documents a time
penalty but no magnitude, UNK-13) to the live race clock; Blitz/
Checkpoint/CrashCourse queue the session's own `restart` intent so
the event restarts through the production lifecycle. AI opponents
reset in place and repair under every mode (designed — original
opponent-destruction behavior is unverified); remote participants are
skipped (F25+ authority). The outcome re-checks the live tier, so two
disabling impacts in one tick resolve once.

## Runtime stuck detection and recovery (F05-B.2)

`VehicleStuck` is a component spawned on the player and every AI
opponent whose `vehstuck` decoded — authored absence means no
component, never a fabricated spec (same policy as `VehicleDamage`).
`StuckSpec` carries the six authored fields verbatim; `rotation` and
`translation` decode but are not consumed — the recovered struct
gives their names, not their tests (UNK-13).

The detector is a designed interpretation of the recovered
`vehStuck` struct's fields (the `Update()` body is unrecovered):

- an `ImpactEvent` arms it at the pose the car was in — the
  `m_LastImpactPos` anchor; every new impact re-anchors;
- inside `pos_thresh` of the anchor the episode accrues toward
  `time_thresh`; past `move_thresh` the car escaped and the detector
  disarms — the uniform `move > pos` pair reads as a hysteresis
  band where accrued time holds;
- a pose rotated more than `turn` since the anchor is still
  tumbling — the rotation leg re-anchors the orientation so the
  settle window only counts a stopped car;
- reaching `time_thresh` fires `StuckVerdict::Stuck` once and
  disarms — the `StuckEvent` stream is bounded to one per episode.

`mm2_app::stuck::track_stuck` (FixedLast, authority + `Playing`
gated, drains stale input) arms off the same deduplicated
`ImpactEvent` stream damage reads and advances every armed detector
per tick — `StuckReport` (`vsk=` smoke field) counts armed/
detections/recovered. `resolve_stuck` answers a detection with the
bounded in-place recovery: `ResetVehicle` onto
`upright_recovery_pose` — heading kept, hull dropped onto the
surface it already rests on — local and AI participants identically
(designed, UNK-13), trailers re-seated at their authored offsets;
remote participants are skipped (F25+). `Rotation` 0 and
`Translation` ≈ 0.1 on every retail record support the in-place
reading: nothing authors a positional rescue or a yaw change. The
reset marks the car `Teleported`, so it cannot sweep a checkpoint.
`resolve_disabled` disarms a wreck's detector — the disabled outcome
owns the car, and the arm/observe legs both skip `Disabled` vehicles
(scheduled before the outcome so the skip sees the pre-repair
tier). The authored `TimeThresh` also drives the pre-existing
`vehicle_self_right` assist delay — the modern assist and the
authored detector coexist: the detector only fires post-impact, the
assist covers impact-free rollovers.

Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`): sf/london
`--frames 600` scripted cruises each record `vsk=3a/0d/0r` — three
impacts arm the detector, none persist because the scripted driver
keeps moving; all pre-existing counters stay bit-identical.

## Runtime breakaway detachment (F05-B.3)

`VehicleBreaks` is a component spawned on the player and every AI
opponent whose `VehicleDef.breaks` is non-empty — authored absence
means no component, never a fabricated rig (same policy as
`VehicleDamage`/`VehicleStuck`). `mm2_content::assemble` builds the
rig only from parts carrying *both* sides of the authored inventory:
a `PartRole::Break` model part *and* a decodable
`tune/banger/<id>_<part>.dgbangerdata` record — an unmatched chunk
stays bolted on, an unmatched record stays dead data (the 10 retail
dead fragments never spawn anything), a malformed record lands in
the conversion warnings.

`mm2_app::breakaway::detach_breaks` (FixedLast, authority +
`Playing` gated, drains stale input) reads the same deduplicated
`ImpactEvent` stream: each attached part whose `severity ×
part_mass` exceeds its authored `ImpulseLimit2` detaches — the
`limit / mass` reading the authored data is shaped for (DSN-21;
~31.25 m/s panels, ~2500 m/s never-detach anchors on retail). The
part's `BreakPartVisual` node hides and a fragment body spawns at
the detached mesh's pose (`car_pose × node_local`, read off the
physics pose so a never-propagated `GlobalTransform` cannot lag the
impact): convex hull over the part's own baked verts, the record's
mass/friction/elasticity, the car's velocity at the part centroid
plus the `dir × severity` kick and spin the prop fragments use.
Fragments claim `BangerPool` slots like prop debris — a part leaves
the rig even when the pool bound denies a body (`fragment: None` on
the event). One bounded `PartDetached` fires per part per
attachment (`VehicleBreaks::detach` refuses a second detach —
F05-AC06); the record's `CG`/`Size` anchors are *not* consumed (they
mix car-space and ~zero conventions on vehicle fragments — UNK-13),
the fragment's `CenterOfMass` is the hull's measured centroid.
Remote rigs are skipped (F25+).

`resolve_disabled` calls `restore_rig` wherever it calls
`damage.reset()` — Cruise's FreeReset, Circuit's PenaltyReset and the
AI in-place reset all repair the wreck, so all three restore the rig:
every part re-attaches, its fragment despawns, the node shows again
(F05-AC03). A plain reset or the stuck recovery does not repair —
detached parts stay off until a repair lands. `BreakReport` feeds a
`brk=` smoke field only once the pipeline saw activity.

Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`): sf/london
`--frames 600` scripted cruises on the default vpbug record all
pre-existing counters bit-identical and no `brk=` field — vpbug
authors no break parts (measured absence, correct). A 1200-frame sf
cruise on vpcoop (6 authored parts at ~31.25 m/s) recorded
`impacts=27 dmg=6a/0d/0r vsk=9a/0d/0r` and still no `brk=` — the
scripted driver's contact approach speeds never reach the authored
~70 mph detach speed, which is the behaviour the authored
thresholds encode.

## Runtime gyro maneuvers (F05-B.4)

`VehicleConfig.gyro` carries the decoded record verbatim
(`Option<GyroConfig>`; `None` on authored absence — never a
fabricated assist). `mm2_content::convert` clamps a malformed
negative/non-finite rate to 0 with a warning rather than sinking the
load, mirroring `VehGyro::validate`'s nonnegative rule; `Pitch`/`Roll`
keep the authored sign the decoder allows and are inert when
non-positive.

The application is a **designed** reading (UNK-13 stays open — the
recovered `Update()` is a thunk), shaped by the recovered feature
gates and the Crash Course's own lessons:

- *Spinable*: handbrake + steering while travelling latches a spin
  maneuver — `Spin180` going forward, `Reverse180` (the J-turn)
  backwards. The latch is `VehicleState::gyro_spin`: a per-frame gate
  cannot express a 180 because the car's forward speed collapses to
  zero halfway through (the recovered `mmCarSim` keeps a `SpinState`
  machine for the same reason). While latched the gyro writes the
  authored yaw rate directly — it is the maneuver's yaw authority,
  not a torque fighting the tires, which correctly resist rotation
  once the car has scrubbed its speed. The handbrake hold doses the
  rotation ("a short tap ~90°, a held one ~180°" — the community
  reading of the lesson): releasing either input drops the latch
  partway, ~π completes it (`gyro_completed`), an opposite flick
  re-arms, and a bounded age means a wedged car is never servoed
  forever. `gyro_spins`/`gyro_completed` feed the `gyr=` smoke field
  only once activity exists.
- *Driftable*: `Drift` relieves the slip term of the yaw-stability
  damper (`sim::yaw_damp_factor`) — a car the record says drifts
  holds a controlled slide instead of being straightened by the
  damper. `drift = 0` is the unmodified policy exactly, so
  gyro-absent and zero-drift cars drive identically to before.
- *Rightable*: `Pitch`/`Roll` are per-axis airborne righting rates —
  a critically damped return on the axis each authors, shaped on the
  `air_control` assist. Every retail record authors 0.0 (or omits
  the fields), so the channel is inert on stock content; stock cars
  keep being levelled by the designed `air_control`/`self_right`
  assists, which remain untouched.

The scripted smoke driver never pulls the handbrake, so no maneuver
latches and no `gyr=` field appears on the stock cruise. The `Drift`
relief is live though: A/B-ing the channel on the retail SF cruise
(`--headless --bot --frames 600`, vpbug authors `Drift 0.2`) moves the
endpoint ~1 m with every smoke counter identical — the authored record
subtly changing slide dynamics is the feature, not drift.

Not yet implemented: visual tiers (smoke pivots land in F05-B.6,
`TextelDamageRadius` decals remain), damage-driven detachment if the
original ever uses it (UNK-13),
C&R healing (DMG-4's `RegenerateRate` channel exists, no mode drives
it), replication.

## Water / out-of-bounds recovery (F05-B.5, designed — DSN-23)

No authored record tunes water or out-of-bounds rescue — the
damage-family census covers `vehcardamage`/`vehstuck`/`vehgyro` only
and DMG-2 covers destruction — so `mm2_game::recovery` is a designed
policy end to end (the original's rules stay UNK-13):

- `VehicleRecovery` rides on every local/AI participant (no authored
  gate to spawn against), pre-anchored at the spawn pose. Its
  **anchor** is the last pose a grounded wheel sat on a non-water
  surface — water colliders are solid in this engine, so "in the
  water" is a surface class under the wheels (`WheelState::
  surface_drag`, the same coefficient the F06-B.2 wading term reads),
  not a missing floor.
- The **submerged** leg fires when every grounded wheel reads `drag`
  at/above `water_min_drag` (0.3 — splits retail `deepwater` 0.5 from
  shallow `water` 0.119, which stays wadable) for `submerge_dwell`
  (2.0 s — the escape window; a car that regains a dry edge inside it
  keeps driving). Recovery is to the anchor — back on shore, never in
  place on the water.
- The **out-of-bounds** leg fires once per airborne fall more than
  `fall_margin` (50 m — sized past retail drops, since any real
  landing refreshes the anchor first) below the anchor; a non-finite
  pose fires at once as defence-in-depth (a pose gone NaN inside the
  physics step trips the wheel raycast first — the detector can only
  answer poses written between steps).
- `mm2_app::recovery::resolve_recovery` answers through the shared
  `ResetVehicle` (`Teleported` — no checkpoint sweep), trailers
  re-seat at authored offsets, an armed stuck episode disarms, and
  recovery is not a repair: damage and detached parts persist.
  Remote participants' detectors belong to their authority (F25+);
  `Disabled` wrecks belong to the damage outcome.

## Engine smoke visual tier (F05-B.6, designed gate — DSN-24)

MM2Hook's recovered `vehCarDamage` struct names the embedded particle
spec `EngineSmokeRule` and carries the pieces that consume it: the two
authored pivots, a `m_CurrentPivot` alternation cursor, an
`ImpactsTable[12]` + `fxTexelDamage` for texel damage, and
`asLineSparks` for impact sparks. `vehCarDamage::Update()` is a binary
thunk, so the emission gate and cadence are designed policy; the
pivots, particle field values and atlas/tile vocabulary are authored.

Implemented in `mm2_game::effects` + `mm2_app::damage_fx`:

- **Pivots (authored + designed gate reading).** `SmokeOffset` always
  emits. `MirrorPivot != 0` derives the second pivot by mirroring the
  first about x = 0 (designed reading — all 7 retail `MirrorPivot`
  fields author 0 anyway). Otherwise a non-zero `SmokeOffset2` is the
  second pivot. `DoublePivot != 0` emits every pivot per burst
  (`vpddbus`/`vppanoz`/`vppanozgt`); single-pivot rigs alternate via
  the cursor.
- **Spec (authored, verbatim).** `ParticleSpec` carries every
  `DamageEffect` field. Consumed: `PositionVar`, `Velocity`(±var),
  `Life`(±var), `Radius`(±var), `Drag`(±var), `DRadius`(±var),
  `DAlpha`(±var), `Gravity`, `TexFrameStart`/`End`, `Color` (packed
  ARGB — retail decodes as `0xF6000000`-class near-opaque black).
  Carried unconsumed: `Position` (pivots own placement), `Mass`,
  `Damp`, `DRotation` (0 on retail), `InitialBlast`, `SpewRate`/
  `SpewTimeLimit` (driver fields — 0 on every retail damage record;
  the designed policy owns cadence), `BirthFlags`, `Height`,
  `Intensity`.
- **Emission policy (designed).** Smoke is the damaged-tier signal:
  rate 0 below `MedDamage`, ramping `rate_at_med` → `rate_at_max`
  (4 → 24 puffs/s) to `MaxDamage`. Emission accumulates fractionally
  per frame and draws per-puff jitter from a per-emitter `NavRng`
  seeded by the vehicle's object id — replicable by construction.
  `max_live` (48/vehicle) bounds the pool; expired puffs despawn on
  their authored `Life`.
- **Field readings (designed).** `Gravity` is a signed +Y rise rate
  (authored 8.7–18 ⇒ smoke rises); `Drag` decays velocity
  exponentially; `DRadius` grows the sprite; `DAlpha` drains the
  `Color` alpha byte per second (retail ≈ −83 ⇒ ~3 s fade inside the
  ~1.5 s life). These are defensible readings, not recovered
  semantics — the original integrator is unrecovered.
- **Sprite (implementation choice).** `texture/fxpt2` resolves
  through the VFS — a measured 2×2 puff-tile atlas; `TexFrameStart`/
  `TexFrameEnd` index its tiles like mm2hook's `asSparkPos::
  TexCoordOffset`. One unlit, blended, camera-facing quad per puff,
  tinted by `Color` with per-puff alpha. The texture binding itself
  is designed — the original's atlas choice is unrecovered, but every
  retail damage record authors frames inside a 2×2 tile space.
- **Ownership (same family rules).** Emission rides whatever
  `VehicleDamage` the entity carries; `PlayerControl::Remote` skips
  (its authority renders its own), pause freezes emission and
  integration, puffs stamp `SessionEntity` for teardown, and the
  headless record reports `ptx=<emitted>e/<expired>x` on activity.

Not implemented in this slice: `TextelDamageRadius` decals /
`ImpactsTable` deformation, `asLineSparks` impact sparks, and
impairment short of destruction (implemented next — below).

## Engine impairment (F05-B.7, designed — DSN-25)

MM2Hook's `mm2.ini` documents a `PhysicalEngineDamage` option —
"damage affects engine torque … when the engine spews smoke" the
vehicle has "less acceleration and less top speed". That is evidence
the original *couples* the smoke tier to engine output, not a
recovered shape: the magnitude, ramp and onset are unrecovered
(`vehCarDamage::Update()` is a thunk, UNK-13), so the implemented
policy is designed:

- `mm2_game::damage::ImpairmentPolicy` — `power_at_med` 0.8,
  `power_at_max` 0.4. `factor(total, spec)` is 1.0 below
  `MedDamage`, steps to `power_at_med` on reaching it, then ramps
  linearly to `power_at_max` at `MaxDamage`. The factor drops below
  1 at exactly the bound the DSN-24 smoke gate emits on, so the
  documented "when it spews smoke" coupling holds by construction.
  Degenerate specs (`MedDamage >= MaxDamage`, non-finite fields)
  and non-finite totals degrade to full output — impairment can
  never stall or over-drive a car.
- `mm2_vehicle::EngineImpairment` is the physics-side input: a
  per-vehicle component scaling only the drivetrain's drive output
  (`wheel_drive_available` — both forward and reverse drive).
  Foot brakes, `engine_brake_nm`, steering and tire forces are not
  engine output and stay unscaled. The sim sanitises the factor
  (non-finite → 1.0, clamps 0..1); absence of the component is
  full output.
- `mm2_app::damage::sync_impairment` (FixedLast, after
  `resolve_disabled`) mirrors each local/AI participant's authored
  total into the component: present exactly while `factor < 1`,
  removed the same tick a repair returns the state to `Intact`.
  Remote participants are skipped (their authority impairs its own
  sim, F25+); a pause freezes the factor with the rest of the sim.
  `DamageReport` counts `impaired`/`restored` episodes — the
  headless record's `imp=` field, emitted only on activity.

Scaling drive torque produces both documented symptoms — weaker
acceleration and a lower drag-equilibrium top speed. Whether the
original scales torque, power, top speed directly, or something
else entirely stays unverified.

## Impact sparks (F05-B.8, designed — DSN-26)

MM2Hook recovers a per-vehicle `asLineSparks* Sparks` on
`vehCarDamage`, `Init`'d alongside the break groups and fired from
the car's `ImpactCB` via
`RadialBlast(count, Vector3 &position, Vector3 &velocity)` — a
radial burst at the impact point, one `m_Spark` trail per spark. The
record's `SparkMultiplier`/`SparkFade` are runtime fields
uninitialised by `Init` and `SparkMultiplier` is not authored on any
retail tune record, so nothing authored bounds the burst shape; the
exact count/velocity/cadence/texture choice are unrecovered
(`vehCarDamage::Update()` is a thunk, UNK-13). The implemented
policy is therefore designed on top of two recovered facts: the
per-vehicle renderer and the impact feed.

- `mm2_game::effects` — `SparkPolicy` + `VehicleSparks` ride on
  every rigged participant (inserted where `vehcardamage` decodes,
  same gate as `VehicleSmoke`; absent where the record is absent).
  Each deduplicated `ImpactEvent` delivers a burst at its authored
  contact `point`: `min_burst` 2 + `sparks_per_speed` 0.8/m/s of
  `severity`, ceiling `max_burst` 16, per-vehicle `max_live` 128.
  Velocities are `rebound_dir` — the participant's own side of the
  contact normal — plus a ±`spread` 0.8 lateral jitter
  renormalized, at `speed` 7 ±3 m/s, so the striker's sparks blow
  back toward it while the struck side's rebound away. Lives
  `life` 0.5 ±0.2 s under `gravity` 9.8 m/s². A per-vehicle
  `NavRng` seeded from the `ObjectId` keeps bursts replicable; a
  non-finite or non-positive `severity` bursts nothing, a
  degenerate normal falls back to straight up, and
  `live >= max_live` truncates the burst. `Spark::advance(dt)`
  integrates gravity/position and reports expiry at `life`;
  `streak()` is the speed-scaled streak length (floor `length`
  0.04 m so a stalled spark still reads as a fleck) and `alpha()`
  a linear `1 − age/life` burn-down — all designed, no authored
  counterpart.
- `mm2_app::spark_fx` — `emit_sparks` (`Update`, chained with
  `advance_sparks`) drains the `ImpactEvent` buffer as an
  independent broadcast reader of the stream the `FixedLast`
  producers `collect_impacts`/`apply_impact_damage` publish — a
  frame-rate reader still sees every buffered message. Remote
  participants are skipped (their
  authority renders its own sparks, F25+), a non-`Playing` session
  drains without emitting, and the deferred-spawn live count is
  tracked per-emitter inside the system so a burst-heavy frame
  cannot overrun `max_live`. Each `Spark` spawns a
  session-stamped entity — a velocity-aligned crossed-quad streak
  (two quads sharing the velocity axis, width 0.03 m) in an unlit
  `AlphaMode::Add` material cloned per spark so `alpha()` animates
  one streak. `advance_sparks` reposes each streak along its live
  velocity, writes the linear fade into the material, and despawns
  on `advance`'s expiry.
- Texture: `spark.tga` — the install's only spark-named texture,
  an 8×8 fleck — resolves through the VFS onto the shared material.
  A missing texture warns and falls back to an untextured additive
  material (the standard missing-texture policy — emission
  continues); the `SparkFx` resource absent entirely (a world that
  never loaded) emits nothing. The original's `Init` texture name
  is unrecovered, so the binding is a designed choice on a
  recovered asset.
- `SparkFxReport` counts bursts/emitted/expired — the headless
  record's `spk=<b>b/<e>e/<x>x` field, emitted only on activity so
  impact-free runs stay bit-identical.

`TextelDamageRadius`/`ImpactsTable` texel damage remains
unconsumed — its consumer is `fxTexelDamage`, a separate system
from `asLineSparks`.

## Open questions

- The original accumulation model: what quantity `MaxDamage`
  integrates (impact impulse? energy? a contact callback count?) and
  whether `ImpactThreshold` compares the same unit — UNK-13.
- Whether `MedDamage` gates visual state, impairment or both. Both
  implemented legs key on it as designed policy (smoke DSN-24,
  impairment DSN-25); MM2Hook's `mm2.ini` option
  `PhysicalEngineDamage` documents that damage affects engine
  torque — "when the engine spews smoke" the vehicle has "less
  acceleration and less top speed" — supporting a
  smoke↔impairment coupling in the original, but its shape/values
  are unrecovered.
- `TextelDamageRadius`'s consumer — mm2hook binds it to
  `fxTexelDamage::ApplyDamage(position, maxDist)` driven by the
  recovered `ImpactsTable[12]` of impact positions; decal projection
  vs vertex deformation and the per-impact table's fill rules stay
  unrecovered. The original `asLineSparks` burst semantics
  (count/velocity/cadence/texture binding) likewise — a designed
  radial-rebound policy is implemented (DSN-26).
- `MirrorPivot` semantics — the implemented mirror-about-x reading
  is designed; every retail value is 0, so no authored case
  distinguishes readings yet.
- Which parts detach at which damage level; whether break detachment
  is damage-driven or impact-driven (the implemented reading is
  impact-driven against `severity × part_mass`, DSN-21 — the authored
  `ImpulseLimit2 = Mass × {31.25, 2500}` structure supports a
  speed-threshold semantic but the original's comparison is
  unrecovered); fragment-vs-intact rig swap rules (intact-hide +
  spawned-fragment is implemented; whether the original swaps a
  damaged variant model is unrecovered).
- `DoublePivot` semantics beyond the implemented emit-all-pivots
  reading (designed — the three retail `DoublePivot` cars also carry
  non-zero `SmokeOffset2`, so alternation-vs-parallel is the only
  distinguishable leg); `Color` packing byte order (read as ARGB —
  consistent on all retail values); `Height`/`Intensity` roles in
  the effect spec; the original emission cadence, pivot switch
  timing and whether `SpewRate`/`SpewTimeLimit`/`InitialBlast` ever
  drive damage smoke (all 0 on retail damage records — the designed
  policy owns cadence).
- `.vehstuck`'s exact test combination — the implemented
  interpretation (impact anchor + hysteresis + tumbling leg + time
  window) is designed; `Rotation`/`Translation`'s roles are
  unrecovered. `.vehgyro`'s application semantics — the implemented
  reading (latched rate-actuator spins, drift damper relief,
  per-axis airborne righting) is designed (DSN-22); the original's
  trigger conditions, application mechanism and whether `Drift`
  relieves damping or feeds friction multipliers stay unrecovered.
- The original's water/out-of-bounds recovery rules — no authored
  records found yet; DMG-2 covers destruction only. A designed
  dry-anchor + dwell/margin detector is implemented (DSN-23, section
  above); whether the original resets to shore, to the last
  checkpoint, or in place, and what its submersion/OOB tests are,
  stays unverified.
