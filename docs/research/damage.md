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
the same impulse estimate the impact pipeline reports
(`approach_speed × striker_mass`, shared with banger activation) as a
documented designed policy (UNK-13 stands).

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

Not yet implemented: visual tiers (smoke pivots, `TextelDamageRadius`
decals, `DoublePivot`/`MirrorPivot` semantics), damage-driven
detachment if the original ever uses it (UNK-13), impairment short of
destruction, `vehgyro` consumption, water/out-of-bounds recovery,
C&R healing (DMG-4's `RegenerateRate` channel exists, no mode drives
it), replication.

## Open questions

- The original accumulation model: what quantity `MaxDamage`
  integrates (impact impulse? energy? a contact callback count?) and
  whether `ImpactThreshold` compares the same unit — UNK-13.
- Whether `MedDamage` gates visual state, impairment or both.
- Which parts detach at which damage level; whether break detachment
  is damage-driven or impact-driven (the implemented reading is
  impact-driven against `severity × part_mass`, DSN-21 — the authored
  `ImpulseLimit2 = Mass × {31.25, 2500}` structure supports a
  speed-threshold semantic but the original's comparison is
  unrecovered); fragment-vs-intact rig swap rules (intact-hide +
  spawned-fragment is implemented; whether the original swaps a
  damaged variant model is unrecovered).
- `TextelDamageRadius`'s consumer (decal projection vs vertex
  deformation), `DoublePivot`/`MirrorPivot` semantics, `Color`
  packing, `Height`/`Intensity` roles in the effect spec.
- `.vehstuck`'s exact test combination — the implemented
  interpretation (impact anchor + hysteresis + tumbling leg + time
  window) is designed; `Rotation`/`Translation`'s roles are
  unrecovered. `.vehgyro` assist application (torques?
  angular-velocity targets? per-axis gains?).
- Water/out-of-bounds recovery rules — no authored records found yet;
  DMG-2 covers destruction only.
