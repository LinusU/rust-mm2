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

## Runtime consumption — what is not known

Parsed, not yet simulated. Everything below is UNK-22:

- What `ImpulseLimit2` is compared against (contact impulse? impact
  speed × mass?) and what crossing it does — break vs. tip vs. nothing.
- Whether `NumParts` also bounds spawned fragments at runtime or is
  purely an authoring echo of the PKG's BREAK count.
- `ColliderId`/`AudioId`/`TexNumber` id spaces (collider table? audio
  table? which texture atlas?).
- `SpinAxis`, `BillFlags`, `Flash`, `YRadius` semantics.
- The `type: a` tag — version field or class marker.
- `BirthRule` field semantics beyond "particle spec for the break
  effect"; when it fires relative to breakage.
- How the fallback `default.dgbangerdata` is selected (globally? per
  unstamped geometry?).
- Whether fragment `NumParts>0` records (the 4 flagged) mean
  fragments-of-fragments the data can't back, or inert leftover fields.
