# Last iteration — F05-B.3: authored breakaway detachment

Iteration 30 on `ralph/night`, continuing from `c1247b9`
(externally checked F05-B.2). Task id: `F05-B.3`.

## Slice choice

F05-B's remaining ready runtime leg after B.1 (damage accumulation +
disabled outcomes) and B.2 (`vehstuck` detection + in-place
recovery): connect the already-inventoried authored breakaway parts
(REC-1) to runtime detach state. F05-AC02/AC03 evidence: parts
detach on meaningful impacts, appear once, clean up bounded, and
repair restores the intact rig.

## What changed

**`mm2_game::breakaway`** (new domain contract):
`BreakPartSpec` (part stem + distilled `BangerDefinition`),
`VehicleBreaks` component — parts in model order, `attached` +
`fragment` state per part. `detachable(approach_speed)` returns the
attached parts whose `approach_speed × part_mass` exceeds their
authored `ImpulseLimit2`; `detach` is one-shot per attachment (the
bounded-event guarantee); `restore` re-attaches everything and
drains fragment entities. `PartDetached` message:
object/generation/tick/part-stem/fragment-object/estimate — bounded
one per part per attachment.

**`mm2_content::assemble`**: `VehicleDef.breaks` resolves only parts
carrying both sides of the authored inventory — a `PartRole::Break`
model part *and* a decodable `tune/banger/<id>_<part>.dgbangerdata`
record. Malformed records warn into `ConversionReport`; unmatched
chunks stay bolted on, unmatched records stay dead data — no
fabricated detach specs (same authored-absence policy as
damage/stuck).

**`mm2_app::breakaway`** (new runtime): `BreakPartVisual` tags each
`PartRole::Break` render node (car_visual arm) with its local pose,
a convex hull over the part's baked verts and the vertex centroid.
`detach_breaks` (FixedLast, chained `track_stuck → detach_breaks →
resolve_disabled`, authority + `Playing` gated, drains stale) reads
the same deduplicated `ImpactEvent` stream as damage: each
detachable part hides its intact node and spawns a fragment body at
`car_pose × node_local` — the part's own hull, the record's
mass/friction/elasticity, car velocity at the part centroid plus
the `dir × severity` kick and `angular_kick` spin the prop
fragments use. Fragments claim shared `BangerPool` slots through
`claim_slot` (made `pub(crate)` with `BangerMut`); a pool-bound part
still leaves the rig (`fragment: None` on the event). A rig part
with no render node cannot detach — a spec/model inconsistency is
not silently repaired. Remote participants skipped (F25+).

**`mm2_app::damage::resolve_disabled`**: calls `restore_rig` at each
`damage.reset()` site — Cruise FreeReset, Circuit PenaltyReset, AI
in-place — so every repair path re-attaches parts, despawns
fragments and shows the intact nodes (F05-AC03). Plain reset and
stuck recovery do not repair: detached parts stay off. `BreakReport`
(`brk=` smoke field, reset on teardown) counts detached/restored
and only appears once the pipeline saw activity.

## The threshold reading (DSN-21)

Measured on every retail vehicle fragment record (`mm2-inspect
banger`, install `fnv1a64:e91e6cd4b2ae30d9`): **`ImpulseLimit2 =
Mass × constant`** — ≈31.25 m/s (70 mph) on all ordinary panels
(166/5.3 vpcoop corners … 4606/147.4 vpsemi rears), ≈2500 m/s
(effectively never) on heavy-rig anchors (vpftruck all six, vp4x4
`break0`, vpvw_cup/vpvwcup_angel `break2/23`). Prop fragment
records share the structure with more constants (≈500/800/2500).
The field is authored as `mass × detach_speed`; the implemented
comparison (`severity × part_mass > limit`, where `severity` is the
contact's approach speed) makes `limit / mass` read directly as the
authored detach speed. What the original compares the field
against stays UNK-22, and whether detachment is impact- or
damage-driven stays UNK-13 — the reading is designed but shaped by
the authored data, not invented. An earlier `severity × other_mass`
draft would have shed every panel on a ~2 mph touch and was
corrected against this measurement before commit.

The fragment record's `CG`/`Size` anchors are not consumed: on
vehicle fragments they mix car-space anchors with ~zero conventions
(UNK-13). The fragment's `CenterOfMass` is the hull's measured
centroid.

## Evidence

- `cargo test -p mm2_game --test breakaway`: 6/6 — spec carry,
  per-part limits incl. at-limit boundary, dedup on re-detach,
  garbage speeds, bounded detach, restore drains fragments.
- `cargo test -p mm2_app --test breakaway`: 10/10 — detach-once
  (node hides, `PartDetached`, pooled `Active` dynamic fragment),
  below-limit stays, per-part thresholds, repair restores rig +
  retires fragment, plain reset keeps parts off, remote rig never
  detaches, no authored rig → nothing, pool-bound part still
  detaches without a body, missing node → no detach, stale
  generation ignored.
- `cargo fmt --all -- --check`: pass. `cargo clippy --workspace
  --all-targets --all-features -- -D warnings`: pass. `cargo test
  --workspace`: 750 tests / 56 binaries, 0 failures.
- Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`):
  - sf `--frames 600` (default vpbug): `impacts=9 … dmg=2a/0d/0r
    rej=1 dup=0 vsk=3a/0d/0r` — all counters bit-identical to the
    F05-B.2 baseline; no `brk=` field (vpbug authors no break parts
    — measured absence).
  - london `--frames 600`: `impacts=7 … dmg=2a/0d/0r rej=1 dup=0
    vsk=3a/0d/0r` — bit-identical, no `brk=`.
  - sf `--frames 1200 --car vpcoop` (6 authored parts at ~31.25
    m/s): `impacts=27 … dmg=6a/0d/0r rej=3 dup=0 vsk=9a/0d/0r` —
    still no `brk=`; the scripted driver's contact approach speeds
    never reach the authored ~70 mph detach speed, matching the
    behaviour the thresholds encode.
- No rendered/GPU evidence required for the state-machine half; the
  fragment visual spawn path is exercised in the synthetic suite
  (mesh children re-spawned under the fragment body) but no
  screenshot proof of a retail detachment exists — the scripted
  driver cannot reach ~31 m/s approach speed into scenery.

## Classification / open items

- Designed (DSN-21): the `severity × part_mass` vs `ImpulseLimit2`
  detach rule, intact-hide + pooled-fragment lifecycle, repair-only
  restore, velocity-inherited kick/spin, hull-centroid COM.
- Original data: `BREAK<NN>` chunks, `_break<NN>.mtx` transforms,
  `<id>_break<NN>.dgbangerdata` records, the `Mass × {31.25, 2500}`
  limit structure — all consumed verbatim.
- Unknown (UNK-13/UNK-22): what the original compares the limit
  against, damage-vs-impact-driven detachment, the record `CG`/`Size`
  conventions on vehicle fragments, whether the original swaps a
  damaged variant model rather than hiding.
- F05-AC03 partially evidenced: part appears once + bounded cleanup
  + repair restore are synthetic-tested; no retail detachment
  observation yet (scripted driver can't reach authored thresholds).
- Still open in F05-B: visual tiers (smoke pivots,
  `TextelDamageRadius`, `DoublePivot`/`MirrorPivot`), damage-driven
  detachment if original, impairment, `vehgyro`, water/OOB
  recovery, C&R healing, replication.
