# Banger records (`tune/banger/*.dgbangerdata`)

Angel Studios' `dgBangerData` is the physics/effects record for every
knockable object: roadside props, breakaway prop fragments, vehicle
wheels/dash parts and editor leftovers. Each record pairs a bounding
`Size`, centre of gravity, mass, elasticity, friction and an impact
threshold (`ImpulseLimit2`) with a `BirthRule` particle spec for the
break effect. Geometry is *not* referenced by name inside the record —
the link is by file stem (`geometry/<stem>.pkg` / `.mtx`) or, for
fragments, `BREAK<NN>` chunks inside the parent's PKG.

Measured on the retail install (VFS fingerprint
`fnv1a64:e91e6cd4b2ae30d9`, 2026-09-20): **999 logical files**, all
parsing cleanly through the generic `tune.rs` block grammar.

## Grammar (verified against every retail file)

```
type: a
dgBangerData {
  AudioId 0
  Size 0.200000 0.500000 0.200000
  CG 0.000000 0.000000 0.000000
  NumGlows 0
  Mass 50.000000
  Elasticity 0.500000
  Friction 0.900000
  ImpulseLimit2 0.000000
  SpinAxis 0
  Flash 264
  NumParts 0
  BirthRule { ... particle fields ... }
  TexNumber 0
  BillFlags 0
  YRadius 0.000000
  ColliderId 0
}
```

- Every retail record carries the `type: a` tag (meaning unknown —
  presumably the record-format version) and the `dgBangerData` root
  block. Every record also carries a birth-rule block, but exactly one
  spells it `asBirthRule` (`sp_tree1_s_break06`) — the parser accepts
  that variant as the birth rule and reports it as a diagnostic rather
  than failing.
- `BirthRule` is decoded into a typed `BirthRule` struct:
  `Position`/`PositionVar`, `Velocity`/`VelocityVar`, `Life`,
  `Mass`/`MassVar`, `Radius`/`RadiusVar`, `Drag`/`DragVar`,
  `DRadius`/`DRadiusVar`, `DAlpha`/`DAlphaVar`,
  `DRotation`/`DRotationVar`, `InitialBlast`, `SpewRate`,
  `SpewTimeLimit`, `Gravity`, `TexFrameStart`/`TexFrameEnd`,
  `BirthFlags` — the particle spec for the break effect. The `*Var`
  fields are inferred variance ranges; runtime semantics unverified.

| Field | Retail values | Status |
| --- | --- | --- |
| `Size`, `CG` | 3-float vectors | verified structural (bounds, centre of gravity) |
| `Mass` | 0.5 – 72,580,104 | verified authored value; units unverified |
| `Elasticity` | mostly 0.5, a few 0.9 / 1.5 | verified authored value (>1 authored once, so not an anomaly) |
| `Friction` | always 0.9 | verified authored value |
| `ImpulseLimit2` | 0 – 1e30 | verified authored value; what it limits is unverified (UNK-22). 0 = activate on any contact (`default` has 0); ~1e30 on 15 records reads as "effectively unbreakable" — inferred |
| `NumParts` | 0–8 | verified: on standalone props equals the count of distinct `BREAK<NN>` chunk indices in the record's own PKG — 0 mismatches on retail |
| `AudioId` | always 0 | parsed; sound-table link unverified |
| `TexNumber` | 0–16 | parsed; inferred particle-texture index |
| `ColliderId` | 0–23, absent on 2 records | parsed; id space unverified |
| `CollisionPrim` | 0/1/2, absent on 18 records | parsed; primitive-code semantics unverified |
| `CollisionType` | 4/16/48, absent on 30 records | parsed; type-code semantics unverified |
| `SpinAxis` | always 0 | parsed |
| `Flash` | integer ids (0 = none) | inferred impact-flash sprite/frame |
| `BillFlags` | integer flags | parsed; billboard-flag semantics unverified |
| `YRadius` | 0 – ~30 | parsed; inferred cylindrical-bound radius |
| `NumGlows` / `GlowOffset` | 0 or paired | verified pairing (two records carry `GlowOffset` without `NumGlows` — anomaly, reported) |

## Stem → geometry resolution

The audit classifies each record by stem and resolves it through the
same VFS the game uses:

| Class | Rule | Retail count |
| --- | --- | --- |
| fallback | `default.dgbangerdata` | 1 |
| standalone prop | `geometry/<stem>.pkg` exists | 216 |
| named part | `geometry/<stem>.mtx` or a matching chunk inside `geometry/<base>.pkg` | 477 |
| break fragment | `<base>_break<NN>` → `BREAK<NN>` chunk in `geometry/<base>.pkg`, else `geometry/<stem>.mtx` | 254 resolved |
| dead ref | none of the above resolve | 47 (30 named + 17 fragments) |
| editor backup | `.#*.dgbangerdata.1.2` filenames | 4 |

`NumParts` on the standalone's record equals the number of distinct
`BREAK<NN>` indices embedded in its PKG on every retail prop checked
(sp_benchwood_f: 3 parts ↔ BREAK01–03, vp4x4: 4 ↔ BREAK01/12/23/03).
Fragments are break-level records sharing the parent PKG — they do not
have a PKG of their own, and four authored records still declare a
nonzero `NumParts` (sp_tree1_s_break01/06 = 8, sp_tree2_s_break01/02 =
2) — reported as issues since no child geometry exists for them.

Vehicle bangers (`vp*`, wheel/dash stems) resolve to `.mtx` transforms
inside the vehicle geometry set; `vpvwcup_angel*` records are authored
dead refs (no such PKG/chunks on retail — likely a cut car).

## Known authored anomalies (all reported, none fatal)

- `sp_tree1_s_break06` — `asBirthRule` block name, `NumParts=8` on a
  fragment, and no `BREAK06` chunk (sp_tree1_s has BREAK01–05 only).
- `sp_lightfwy_f_break03/04` — `GlowOffset` without `NumGlows`; also
  dead refs (sp_lightfwy_f carries BREAK01–02 only).
- `vpvw_cup`/`vpvwcup` — `break01`/`break02` records exist but the
  PKGs hold `BREAK2`/`BREAK23` (single+double-digit indices);
  `vpvwcup_angel*` records (9) are entirely dead.
- `sp_roundbout_l_break01/02` — dead; parent has no BREAK chunks.
- `vpeagle_*` — dead; `geometry/vpeagle.pkg` does not exist.
- `vpcaddie59_dash_*` — dead `.mtx` refs.
- Absurd masses/limits are authored, not parse noise: `cp_chinagate_f`
  mass 1.48 M, `giz_chinagate_f` 72.6 M, `giz_bridge*_l`
  `ImpulseLimit2` = 1e30 (effectively immovable — consistent with
  bridge gates that should never break).

## Audit results (`mm2-inspect banger`, retail 2026-09-20)

- **999/999 records parse** — 1 expected fallback + 998 extras, zero
  unsupported, zero parse failures.
- 47 dead geometry refs, 54 issues total (47 dead + 4 fragment
  `NumParts` + 2 glow-count mismatches + 1 `asBirthRule`).
- Every standalone prop's `NumParts` matches its PKG's BREAK-index
  count (0 mismatches).
- `--strict` exits 2 on the 54 issues; missing `default.dgbangerdata`
  or a parse failure would count as failures.
- `mm2-inspect scan` parses all 995 `.dgbangerdata` names (the 4
  `.1.2` backups don't carry the extension).

## Placement binding (`mm2-inspect banger-bind`, retail 2026-09-20)

The binding rule is by name: a placed name `N` binds iff
`tune/banger/<N>.dgbangerdata` resolves through the VFS. The audit walks
every placement source that can stamp a world object — INST files,
`*.pathset` files under `city/` and `race/`, `propdefs.csv`,
`proprules.csv`, `props.csv` group tables, and the PSDL `prop_rule`
bytes — and cross-checks each placed name against the banger stems and
`geometry/<N>.pkg` separately, so dead placement refs stay visible.

**Verified measurements (129 source files, 0 failures):**

| Source | Result |
| --- | --- |
| `city/london.inst` | 1997 placements / 221 names — **0 bound** |
| `city/sf.inst` | 3763 placements / 165 names — **0 bound** |
| `city/{london,sf}/props.pathset` | london 17/17 prop names bound; sf 30/30 bound (31 decal paths separate) |
| `city/{london,sf}/propdefs.csv` | london 16 defs → 12 pkg names bound; sf 33 defs → 27 bound |
| `city/{london,sf}/props.csv` groups | london 19 entries: 18 bound + `sp_bollard_pedsafe_l` dead; sf 16/16 |
| `city/props.csv` root group table | 3/3 bound (`sp_barricadeconcl_f`, `sp_barricadeconcr_f`, `sp_jumptrailer_f`) |
| `race/<city>/*.pathset` overlays | every resolved prop name bound; `blitz10`/`blitz11` truncated → unsupported |
| `city/*_ai.inst`, `*.sdl_ai.inst` | supplemental files stamping only `sp_stop_f` (40–67 instances each, all `modifiers=0x0200`) — bound |
| PSDL `prop_rule` reachability | london 17 rule numbers / 415 rooms → 15 defs → 11 files, all bound; sf 20 / 397 rooms → 25 defs → 20 files, all bound |
| dev/backup sources (`city/phys`, `sfai`, `variant`, `sf/bak`, `race0`) | dead refs kept in the denominator: 45 `*_m` phys names, `r_concrete`, `prop_sp_barricadeconcr_f`, `xcp_banrred_f` |

Union per stock city: london 277 placed names → 55 bound / 221 unbound /
1 dead; sf 232 → 67 bound / 165 unbound / 0 dead.

**The binding conclusion is verified, not inferred:** INST placement is
the static-architecture channel — not one of its 386 distinct names has
a banger record. World knockables reach the world through the *pathset*
stamping channel (`props.pathset`, race overlays) and the *prop-rule*
channel (`proprules.csv` → `propdefs.csv` files selected per PSDL room
edge); every name those channels can produce binds. The `*_ai.inst`
stop-sign supplements are the only INST files that place a bound name.

**Reverse coverage:** of 994 records (999 minus `default` and the 4
`.#*` backups), 269 are reachable through audited placements. 562 sit on
105 `vp*`/`va*` owner PKGs — vehicle bangers bind through the vehicle
pipeline (`partName`), not world placement. The remaining 163 records on
87 owners are authored-but-never-placed on retail: unused prop variants
(`sp_fruitcart_l` + 7 fragments, `sp_newsgroup*`, `sp_oneway*`,
`sp_office_door_f`), mode/marker objects (`pt_check`, `pt_finish`,
`pt_red`/`pt_blue`, `wpobj_gold`, `tptpole_bnd`), animated `giz_*`
records whose paths exist only in overlays the audit names separately,
and building ornaments (`kp_harrod_*`, `sp_awning_*`, `ep_transam_door_f`).

## Recovered runtime structure (MM2Hook / R4)

From `Dummiesman/mm2hook` `src/modules/banger/` — struct layouts
recovered from the original binary. These establish the *shape* of the
runtime model; field-level semantics are still inference.

- `dgBangerDataManager::AddBangerDataEntry(name, partName)` — records
  are registered under a name *plus* part name, matching the
  stem/named-part audit classification.
- `dgBangerInstance` — placement-side object storing a packed banger
  type/variant index into the manager's table.
- `dgUnhitBangerInstance` — the dormant placement state, in Y-axis-angle
  and full-matrix variants (the two placement forms seen in INST and
  pathset stamping).
- `dgHitBangerInstance` — the struck-but-now-static state; implies the
  dormant → hit transition is a distinct instance class, not a flag.
- `dgBangerActive` — the dynamic state: physics body state, `phSleep`
  sleep flag, `Target` back-pointer to the dormant instance, an
  `asParticles` effect, and a `Timer` read as the despawn/settle
  timeout.
- `dgBangerActiveManager` — a fixed pool of **32** active objects:
  the original bounds simultaneous dynamic bangers, consistent with an
  oldest-first reclaim policy (reclaim order itself unverified).
- `lvlInstance` flags `INST_BANGER`, `INST_STATIC`, `INST_LANDMARK`,
  `INST_VISIBLE` — instance flags exist, but the retail INST
  `modifiers` word is *not* this flag word (its low bits are paint
  variants, 0x0100 lands on monuments). How a stamped prop's instance
  acquires `INST_BANGER` is unknown — likely the binding lookup itself.

The implied state machine: **dormant placement (`dgUnhitBangerInstance`)
→ dynamic (`dgBangerActive`, pooled ×32) on impact → settled/hit
(`dgHitBangerInstance`) or despawned on `Timer`.** Which threshold gates
the first transition (`ImpulseLimit2` against what quantity), whether
fragments spawn at activation or at a later break threshold, and what
`phSleep`/`Timer` exactly do remain UNK-22.

## Implemented runtime slice (F04-A/B — provisional)

`mm2_game::banger` + `mm2_app::banger` implement the recovered state
machine's first half on Avian. This is an *implementation choice*, not
verified original behaviour — every provisional point is still UNK-22.

- **Binding consumption.** `stamp_pathset` resolves each prop name
  through a per-load `BangerDefs` cache
  (`tune/banger/<name>.dgbangerdata`, lowercased). A bound name with a
  collider stamps one session-owned entity — collider, authored
  physicals (`Mass`, `Friction`, `Restitution`, `CenterOfMass` from
  `CG`), `Banger` state, `ObjectIdentity`, `AuthorityRole` — with the
  mesh parts as children that follow the body. Unbound names and bound
  names without collision stamp as the ordinary static render/collider
  pair. Records that resolve but fail decode are counted
  (`banger_failed`) and stamp unbound. Verified on retail: sf
  `props.pathset` 925/925 stamps bound carrying 3,092 collidable BREAK
  pieces, london 1188/1188 carrying 2,710, zero decode failures
  (`bng=925d/0a/0s/0b`, `bng=1188d/0a/0s/0b` in headless smoke; pieces
  are the prepared fragments, not spawned bodies).
- **Dormant → Active.** `activate_bangers` (FixedLast) reads
  `CollisionStart` edges — a second consumer alongside
  `collect_impacts`, since bangers need every approaching contact, not
  only deduplicated reportable ones. Approach speed is the deepest
  manifold contact's pre-solver `normal_speed` (shared
  `deepest_contact` helper with the impact pipeline). The provisional
  estimate is `approach_speed × striker_mass` compared against
  `ImpulseLimit2`; a qualifying edge flips `RigidBody` to dynamic once,
  applies one impulse leaving the prop at the striker's approach speed
  plus a spin kick derived from the authored `Size` bounds (solid-cuboid
  inertia estimate), and emits `BangerStateChanged`.
- **Pool.** `BangerPool.max_active = 32` — the R4-recovered
  `dgBangerActiveManager` size. At capacity the oldest activation
  (lowest `(activated_tick, object_slot)`) settles with
  `BangerCause::Reclaimed` before the new one takes the slot. Reclaim
  order is provisional — R4 recovers the size, not the order.
- **Active → Settled.** A dynamic banger Avian puts to sleep turns back
  into a static collider at its rest pose — the `dgHitBangerInstance`
  state. `Settled` is terminal for the session (no
  `dgHitBangerInstance`→anything transition is recovered); session
  teardown/restamp restores the original placement.
- **Authority.** Both systems are gated like the race driver:
  `Predicted` sessions drain edges but never transition banger state —
  replication (F26) delivers authoritative `BangerStateChanged`.
- **Messages.** `BangerStateChanged` carries `ObjectId` + session
  generation + fixed tick + phase + cause (`Impact{severity,estimate}` /
  `Slept` / `Reclaimed`) — the semantic stream audio/particle/
  replication consumers read instead of watching physics.
- **Dormant → Broken (F04-B.1).** On a qualifying impact a bound prop
  carrying collidable `BREAK<NN>` pieces does not go dynamic itself: the
  parent's collider and render children are removed, its phase becomes
  `Broken`, and each prepared piece spawns as its own dynamic entity —
  own `ObjectId`, same session owner, own collider and render children.
  `pkg_to_parts` splits `BREAK<NN>` chunks out of the intact LOD set at
  load; each piece resolves `tune/banger/<parent>_break<NN>` for its
  physicals and falls back to the parent's `BangerDefinition` when no
  fragment-specific record exists. Pieces claim `BangerPool` slots like
  any activation, so a break at capacity reclaims oldest-first and
  `max_active = 0` spawns nothing. Exactly one `BangerStateChanged`
  (`phase: Broken`) is emitted for the parent — fragment entities carry
  `Banger` too and settle through the normal active → settled path.
  Whether the original spawns fragments at the *same* threshold as
  activation (implemented choice: yes — one `ImpulseLimit2` gate for
  both) is UNK-22; a prop with no collidable pieces takes the ordinary
  activation path, which matches authored data like
  `sp_barricadeconc[lr]_f` (`NumParts` = 0 — they tip, not shatter).
- **Deliberately deferred:** `BirthRule` particles, `AudioId`/`Flash`/
  `TexNumber` effects, decals, prop-rule-channel stamping, the
  `dgBangerActive` `Timer` despawn, and replication. `NumParts` is
  carried on `BangerDefinition` but does not bound the prepared piece
  set — every collidable `BREAK<NN>` chunk is prepared regardless,
  since the audit verified the two always agree on standalone props;
  whether `NumParts` gates runtime spawning is still UNK-22.

## Observed retail strikes (F04-C.1)

Headless `hold`-driver strikes on the retail install, targeted with
`--spawn x,y,z[,yaw]` (dev pose) and read off the `bng=`/`bng_ev=`
counters in the `smoke=headless-physics` record. Targets were chosen
by expanding `props.pathset` stamps per `stamped_transforms` and
cross-referencing BAI road centre-lines (`mm2-inspect dump
city/<city>.bai`) for placements lying on a road.

- **Activation + settle (London roam, verified ×2):** `vpbug` at
  `--spawn 0.4,5.5,-720,0 --frames 1500` drove road454 into the
  `sp_bollard_black_l` row stamped across it at (−8.3…4.9, 5.0,
  −742…−744) → `bng=1187d/0a/1s/0b bng_ev=1a/1s/0b`. Re-run
  bit-identical. The sibling row across road467 gives the same
  record: `--spawn 112.3,5.5,-745,0 --frames 1500` →
  `bng=1187d/0a/1s/0b bng_ev=1a/1s/0b`. One activation event, one
  settle, dormant count −1, on flat ground.
- **Break (SF roam):** `vpsemi` at `--spawn=-169,35.5,744,172`
  struck the `sp_wrongwayfw` freeway sign stamped at
  (−178.9, 34.9, 784.8) beside road4 → `bng=924d/3a/0s/1b
  bng_ev=0a/0s/1b`. One `Broken` event, 3 `Active` fragments —
  exactly the authored `NumParts 3` / BREAK01–03 chunks. Fragments
  emit no spawn event by contract.
- **Below threshold (SF roam):** `vpsemi` at
  `--spawn=-180,35.5,776,187` grazed the same sign at ~3 m/s →
  `impacts=1`, `bng_ev=0a/0s/0b`. Contact registered, no transition.
  Independently: `vpbug` at `--spawn=-1641.6,36.7,389,0` reached an
  `sp_cone_f` at 7.7 m/s — `impacts=1`, no transition, and the
  dormant cone stopped the car dead (`moved=7m`), the static-collider
  side of the threshold.
- **Activation without settle (SF roam):** `vpbug` at
  `--spawn=-1641.6,36.7,410,0` hit an `sp_cone_f` (limit 8500) at
  ~32 m/s → `bng=924d/1a/0s/0b bng_ev=1a/0s/0b` at 800 frames and
  still `1a/0s` at 5000 ticks — the punted cone never reached Avian
  sleep on the sloped streets, matching the fragment observation
  below.
- **Unbroken attempt (London `circuit:7`):** the `sp_sawhrslt_f`
  wall (ImpulseLimit2 34982) on road216 near (−695, 235) was
  reachable at only ~8 m/s semi — below threshold, no transition.
- **Open observation:** the sign fragments stayed `Active` over 3000
  ticks on the sloped elevated freeway (never reached Avian sleep);
  slope tumbling suspected, a flat-ground retail break would settle
  whether this is a settle-path issue. The London bollard settled
  normally on flat ground.
- **Hull clearance governs what can be struck (F04-C.1 repair):**
  the vehicle collider's underside is deliberately raised
  (`clear_underside`: ≥0.25 m floor plus ~25° approach / ~15°
  breakover ramps) and wheels are raycast, not colliders — a prop
  shorter than the local hull floor passes underneath without any
  contact. `vpbus` (nose floor ≈1 m) drove straight through
  `sp_bollard_black_l` rows and `sp_cone_l` clusters registering no
  contact at all; the same rows stop or activate under `vpbug`,
  whose hull sits lower. `ImpulseLimit2` is irrelevant when no
  contact occurs. Whether the original lets wheels/low bumpers
  strike kerb-height props (cones 0.85–1.14 m) is an open fidelity
  question — with the current hull a tall vehicle can never touch
  them.
- **Retracted claim:** an earlier record cited `vpbug
  --spawn 762,0.5,-424,0` activating the `sp_bollard_black_l` at
  (759.7, −427). It is not reproducible and was physically
  impossible: that spawn drives −Z ~7 m into static geometry, and a
  1000 kg vpbug cannot reach the required >10.86 m/s in that
  distance. The two road-row commands above are the corrected
  evidence.
- **Targeting notes:** `hold` waits ~2 s then drives straight at
  full throttle — no steering, and vehicles drift on cambered roads.
  Spawns that are not on collision geometry fail `never grounded`.
  Signs with thin collision (e.g. `sp_noenter_f`, Size 0.06 m) are
  impractical to hit deliberately.

## Runtime consumption — what is not known

Parsed, bound and provisionally simulated. Everything below is UNK-22:

- What `ImpulseLimit2` is compared against (contact impulse? impact
  speed × mass?) and what crossing it does — break vs. tip vs. nothing.
  The implemented `approach_speed × striker_mass` estimate is a
  stand-in; original evidence could change both the quantity and the
  comparison.
- Whether `NumParts` bounds runtime fragment spawning or is purely an
  authoring echo of the PKG's BREAK count — the implementation prepares
  every collidable `BREAK<NN>` chunk regardless of `NumParts`, since the
  audit verified the two always agree on standalone props.
- Whether fragments spawn at the activation threshold or a later break
  threshold — implemented as the same `ImpulseLimit2` gate, unverified.
- `ColliderId`/`AudioId`/`TexNumber` id spaces (collider table? audio
  table? which texture atlas?).
- `SpinAxis`, `BillFlags`, `Flash`, `YRadius` semantics.
- The `type: a` tag — version field or class marker.
- `BirthRule` field semantics beyond "particle spec for the break
  effect"; when it fires relative to the dormant→active transition.
- How the fallback `default.dgbangerdata` is selected (globally? per
  unstamped geometry?).
- How a stamped placement acquires `INST_BANGER`, and the exact
  dormant→active→hit/despawn transition conditions and pool-reclaim
  order inside `dgBangerActiveManager` (pool of 32 verified; order not).
  The `Timer` despawn is recovered but unimplemented — the slice settles
  instead of despawning.
- Whether fragment `NumParts>0` records (the 4 flagged) mean
  fragments-of-fragments the data can't back, or inert leftover fields.
