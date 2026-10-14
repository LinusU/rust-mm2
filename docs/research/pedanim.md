# Pedestrian rig files (`anim/`)

Format notes for the pedestrian model/animation corpus, measured on a
retail install (`fnv1a64:e91e6cd4b2ae30d9`) 2026-09-28. Parsers live in
`mm2_formats::ped` (+ `pkg::PkgShaders::parse`); the audit is
`mm2-inspect peds`.

## Corpus census

- 4 expected archetypes: `pedmodel_{man,manw,woman,womanw}` — each ships
  `.mod`, `.skel`, `.csv`, `.rays`, `.shaders`; `woman` additionally
  ships `.remap` (the only remap on retail).
- `pedmodel_wolf.skel` — a 26-bone dog/wolf rig with no companion files
  (partial archetype; no `.mod`, no states — nothing on retail
  references it).
- 67 `pedanim_*.anim` files: 66 parse as binary clips (34 `man` + 33
  `wom` names on disk, minus the misfit); `pedanim_manantrnch.anim` is a
  104-byte ASCII scene list (`4` + `"Banks"/"Pipeline"/"Sun"/"Peds"`
  rows), not a clip — an authored scratch file.
- Non-content: `anim/cvs/*` metadata (3 files + a dir entry),
  `anim/grog.bat` (a `mod/skel/anim` converter driver script), and the
  extensionless `anim/pedmodel_woman` — a second ASCII scene list.

## `.skel` — skeleton (ASCII, recovered)

```
NumBones 19
bone root {
	offset 0.000570 1.147210 -0.000000
	bone spine { … }
}
```

`NumBones <n>` header, then a recursive `bone <name> { offset x y z
…children }` tree (tab-indented, CRLF). All four ped rigs are the same
19-bone humanoid: `root → spine → {neck → head, clavicle_{r,l} →
shoulder → elbow → wrist}` and `root → pelvis → {hip,knee,ankle}_{l,r}`.
The wolf rig is 26 bones (`neck1/neck2`, `tail1..4`, `foot_{l,r}`).
Offsets read as bind-pose local translations (inferred from the tree
shape — e.g. `root` stands ~1.147 m up). `NumBones` matches the parsed
count on all 5 files.

## `pedmodel_*.csv` — animation state model (ASCII, recovered)

```
# anim name,mma name,first frame,last frame,Y AXIS Offset,Y AXIS DISTANCE,X AXIS Offset,X AXIS DISTANCE,default next[
STAND,pedanim_manstand,1,30,0,0,0,0,STAND
STAND_WALK,pedanim_manst2w,1,4,0,0.281,0,0,WALK
```

9 cells: state name, clip stem (`anim/<stem>.anim`), 1-based first/last
frame window, Y/X offset+distance floats (measured: forward/lateral
travel bookkeeping per window — `DISTANCE` equals the clip's
root-channel Z/X travel (man walk `1.409` vs a measured −1.410 drift;
run `2.854` vs −2.8535), and chained rows carry the prior `DISTANCE`
into the next `OFFSET` (0.281 + 1.409 = `WALK_STAND`'s 1.69; the dive
chain carries ±2.2 m laterally). mm2hook's recovered
`pedAnimationSequence` names the pair `FSpeed`/`LSpeed` — movement
input for the controller, not pose data), default-next state name.
Each of the 4 CSVs authors
the same 24-state machine (STAND/STAND2, WALK, RUN, BACKUP, ANTIC,
dive-left/right, ground recovery, transitions between them — the
authored header documents the `from_to` transition convention). All
authored state names and clip stems resolve on retail. The window's
`last frame` equals `frames` or `frames + 1` on every row — a
consistent authored off-by-one (36 rows are `+1`), never more.

## `.remap` — bone remap (ASCII, recovered shape / unknown purpose)

`pedmodel_woman.remap` only:

```
17
1 3 2 5 4 7 6 9 8 11 10 13 12 15 14 17 16
```

A count line then that many indices. 17 entries against a 19-bone rig;
the pairwise swaps read like a channel reorder (the woman rig lists
`clavicle_l` before `clavicle_r` where `man` lists `_r` first) —
consistent with retargeting clips across differently ordered rigs, but
unverified (UNK-41).

## `.rays` — unknown payload (ASCII, recovered shape)

```
19
0.177000 0.095000 0.053000 2 7
… 19 rows of <f32 f32 f32 i32 i32> …
3 2 0 3 3 3 0 0 3 3 0 0 8 6 8 0 8 0 0
… integer grid, 19 ints per row …
```

`n` (= bone count) + `n` vector/int/int rows + a grid of integer rows
(48 on man/woman, 24 on manw/womanw — the `*w` variants ship a half-size
grid). Grid values are small ints (0–17). Name suggests collision/skin
rays; semantics unrecovered (UNK-41).

## `.anim` — binary clip (measured grammar)

```
u32 reserved      ; 0 on all retail clips
u32 frames
u32 floatsPerFrame; 60 on all retail clips
f32 motionHint    ; correlates with locomotion speed — see below
u8  kind          ; 1 on all retail clips
f32 samples[frames * floatsPerFrame]
```

Strict grammar — exact byte fit, no trailer. Frame layout recovered
(measured on retail, matching R3 `Pedestrian_animations.md`'s type-1
channel and mm2hook's `crAnimFrame`/`crBone` layout):

- channel 0 (`floats[0..3]`): the **root bone's world-space
  translation** — it stands ~1.147 m up in idle clips and drifts along
  −Z through locomotion windows; the walk clip's total Z travel equals
  the state row's `Y AXIS DISTANCE` to ~1 mm.
- channels 1..`NumBones` (`floats[3+3i .. 3+3i+3]`): **bone `i`'s local
  Euler rotation in radians**, in `.skel` pre-order — verified by the
  standing pose's mirrored left/right values landing exactly on the
  `clavicle/shoulder/elbow/wrist_{r,l}` channel pairs.
- Euler composition is the AGE `Matrix34` convention `Rx·Ry·Rz`
  (row-vector form — `Matrix34::GetEulers` extracts `X = atan2(m12,
  m22)`, `Y = asin(−m02)`, `Z = atan2(m01, m00)`, exactly that
  product), i.e. `Quat::from_euler(EulerRot::XYZEx, x, y, z)` in
  column-vector terms: fixed-axis X first, then Y, then Z. Under this
  order the dive clips' end poses land the body prone along the dive
  direction; the intrinsic-XYZ reading leaves it perpendicular.
  Note the Blender `io_scene_angelstudios` importer maps these floats
  differently — its conversion is already fudged for Blender's Z-up
  space and must not be copied.
- Non-root bones carry no translation channel: their local translation
  is always the `.skel` bind `offset`. Frames interpolate by lerping
  the raw channel floats — mm2hook's `crAnimFrame::Blend(fraction,
  first, second)` is precisely that buffer shape (call sites
  unrecovered).

`motionHint` tracks authored travel: man walk 1.552, run 2.970; woman
walk 1.087 matches its csv `Y AXIS DISTANCE` exactly; negative on
back-up clips; 0 on stands. Close to per-window root travel on most
clips but off ~10% on man walk/run — a plausible per-loop distance
hint, possibly stale; unverified (UNK-41). Largest clip: 53 frames /
3180 floats.

The runtime sampler/state-stepper lives in `mm2_game::ped`
(`PedRig::sample`, `PedAnimator`); playback rate and transition
policies there are designed, not recovered (DSN-64).

## `pedmodel_*.shaders` — standalone PKG shader chunk (verified)

Byte-for-byte the PKG `shaders` chunk grammar: `u32` type word (low 7
bits = paint jobs, bit 7 = byte colours), `u32` shaders-per-paint-job,
then `jobs × per` records. All four retail files: float shaders, empty
texture names, exact fit (man 48×18, manw 24×17, woman 48×17,
womanw 24×16). Parsed by `mm2_formats::pkg::PkgShaders::parse`.

## `.mod` — ASCII skinned mesh (recovered, F19-A.2; binding F19-A.4)

~46–60 KB ASCII; `version: 1.09` on all four retail files. A ten-field
count header (`verts`/`normals`/`colors`/`tex1s`/`tex2s`/`tangents`/
`materials`/`adjuncts`/`primitives`/`matrices`), then resource lists
(`v`/`n`/`c`/`t1`/`t2`/`ts`/`tt` rows), `mtl <name> { … }` shader
groups, the geometry itself, and trailing `mtxv`/`mtxn` matrix-count
rows. Two authoring dialects exist on retail:

```
# packet dialect — pedmodel_man, pedmodel_manw
mtl Businessman1:SKIN {
	packets:	5          # this group owns packets[i..i+5]
	primitives:	44         # sum of owned packets' tri count
	textures:	1
	texture:	0 BEARD64
	illum: diffuse
	ambient/diffuse/specular: r g b
}
packet 15 24 2 {           # declared adjuncts / tris / matrices
	adj	v n c t1 t2 slot   # slot indexes this packet's mtx list
	tri	a b c              # indexes the packet's own adjuncts
	mtx 0 1                # bone matrix indices (< matrices:)
}
mtxv 1 2 …                 # per-matrix contiguous vertex counts
mtxn 1 2 …                 # same partition for normals

# flat dialect — pedmodel_woman, pedmodel_womanw
mtl A:SKIN {
	adjuncts:	3        # this group's slice of the global adj list
	primitives:	1        # slice of the global tri list
	…
}
adj	v n c t1 t2            # five fields — no matrix slot
tri	a b c                  # indexes the global adjunct list
```

Adjuncts pair a vertex with a normal plus colour/tex indices — they are
the skinned face-corners, shared between triangles.

Measured invariants (all four files):

- `adjuncts:` == `normals:` == the number of *distinct* (vertex,
  normal) tuples across all `adj` rows. Flat-dialect rows are all
  distinct, so the header equals the row count (696/723); packet
  dialect shares corners (manw: 279 rows → 252 distinct).
- `matrices:` == skeleton `NumBones` (19 on the four humanoids).
- `primitives:` == total `tri` rows; per-material
  `packets:`/`adjuncts:`/`primitives:` exactly partition the packet
  blocks / global adjunct / triangle lists in order.
- `mtxv`/`mtxn` have exactly `matrices` entries and sum to
  `verts`/`normals` — they partition the vertex/normal arrays
  contiguously by bone. Packet `adj` matrix slots resolve through the
  packet `mtx` list to the same bone `mtxv` assigns: the two
  skinning records agree 100% on retail (all 1946 adjuncts — man 248,
  manw 279, woman 696, womanw 723 — including every adjunct's `mtxn`
  normal bucket).
- `v` rows are **bone-local**, not model-space: every authored vertex
  lies within ~0.55 m of the origin while the rig stands ~1.15–1.8 m
  tall. Applying `T_world(bone) · v` at the bind pose — the bone being
  the `mtxv` bucket (or the packet `mtx` slot's entry) — reassembles
  each mesh as a feet-on-the-ground standing figure (man y ≈ 0–2.0,
  ankles at ~0.01, head ~1.82; woman y ≈ 0–1.87). Rigid skinning is
  therefore the posed bone transform applied directly — no inverse-bind
  matrix, matching the AGE `crModel`/`crBone` convention.
- The flat dialect's adjuncts bind through `mtxv` alone (no
  per-adjunct slot exists) — the vertex's bucket index *is* the bone
  in `.skel` pre-order. `mtxn` binds normals the same way.
- `materials:` == `mtl` blocks == `.shaders` shaders-per-paint-job
  (man 18, manw 17, woman 17, womanw 16) — R3 documents the group order
  must match the shader order.
- `colors:` is 1 on all four (every adjunct's `c` index is 0); `t1`
  carries real per-adjunct UV indices (≤ tex1s−1); `tex2s:`/
  `tangents:` are 0 and `t2`/`ts`/`tt` never appear.

Unrecovered: whether `mtxv` is authoritative over packet `mtx` lists
or vice versa (they agree on every retail adjunct, so the question is
moot on stock data — `mm2_game::ped::PedSkin` prefers the packet slot
and falls back to `mtxv`), `stp` rows (seen in R3 docs, absent on
retail), and the optional 4th `packet` header int (UNK-41).

## Cross-checks that hold on retail (`mm2-inspect peds`)

- Every state-model clip stem resolves to a present `.anim`; no missing
  references.
- Referenced clips carry `floatsPerFrame == 3 × (bones + 1)` on all 4
  complete rigs, and every authored state window samples to finite
  poses through `mm2_game::ped::PedRig` (the audit exercises
  first/mid/clamped-last frames per window).
- `.rays` row count == `NumBones` on all 4.
- Every parsed `.mod` assembles into a `mm2_game::ped::PedSkin` against
  its own rig, deforms to a plausible feet-on-the-ground standing figure
  at the bind pose, and deforms to finite world geometry at every
  sampled state-window pose (292 deform samples: 4 bind + 288 window).
- All 66 binary clips parse; 18 are unreferenced by every state model
  (dive/ground/run-back variants — preserved, reported).
- `--strict` exits 0 on the retail corpus.

## Triangle winding and the on-screen lab (F19-A.5)

Measured on retail: of the 963 non-degenerate `.mod` triangles at the
bind pose, 941 (97.7 %) have a counter-clockwise geometric normal on the
same side as the authored corner normals, so the authored `tri` order is
the front face with no flip — the same identity convention the vehicle
importer uses (`-Z` forward, `+Y` up, no mirror). `mm2-inspect peds`
prints the count and raises an issue when an archetype is mostly the
other way round. The 22 disagreeing triangles are unexplained (strip
winding is still unrecovered, UNK-41).

`--ped-lab` (`mm2_app::pedestrian`) puts one figure per stock archetype
in front of the player, each in a different paint job, stepping through
nine authored states in place (root horizontal drift removed). Retail
`pedmodel_*.shaders` author colour only, so the figures are flat-coloured
clothing, not textured. A rendered capture of the line-up on retail
`sf` was inspected locally (not committed: original content).

## Sidewalk movement (F19-B.1)

`mm2_game::pedwalk` is the domain half of pedestrian movement: a
`SidewalkNet` over the BAI sidewalk curves, a seeded `plan_pedestrians`,
and `SidewalkNet::advance`. The runtime leg (F19-B.2) fields it:
`mm2_app::crowd` builds the crowd on a city session's first `Playing`
frame and recycles it with the players' interest bubbles; `--ped-lab`
remains the pose/animation diagnostic view.

Walk speed comes from the animation, not a constant:
`PedAnimState::locomotion_speed` = the state's `Y AXIS DISTANCE` over the
time one pass of its window takes at the 30 fps playback policy. Retail
man `WALK`: 1.409 m over its 20-frame window ≈ 2.1 m/s; woman `WALK`:
1.087 m ≈ 1.6 m/s. A walker moved at this speed does not skate.

`mm2-inspect nav` now prints a sidewalk census per city. Measured on
retail (fingerprint `e91e6cd4b2ae30d9`):

| join radius | London joined ends (of 2160) / regions | SF joined ends (of 1516) / regions |
| --- | --- | --- |
| 1 m | 176 / 994 | 20 / 750 |
| 3 m | 589 / 792 | 260 / 632 |
| 5 m | 1171 / 505 | 1093 / 278 |
| 8 m (default) | 1551 / 325 | 1222 / 224 |
| 12 m | 1791 / 212 | 1284 / 210 |

Corner gaps are spread over the whole range rather than clustered, so
no radius recovers the original's rule (UNK-42); even at 12 m London's
largest connected region is 99 of 1080 curves. The vehicle-lane guard
(a join is refused when the line between the two ends crosses a vehicle
lane at the same level) removes 224 candidate joins at 8 m on London and
30 on SF. A 2 km soak of one walker per curve stays on the net (0
escapes). Pedestrians therefore walk their own street and a few corners,
and never cross a street — crosswalks are an open question, not a
feature.

## Reaction chains (F19-B.3, measured)

`mm2-inspect peds` follows each dive row through `default next` on the
four stock archetypes: `ANTIC_{L,R}DIVE` and `WALK_{L,R}DIVE` all chain
(`*_GROUND{L,R}` → `GROUND_STAND{L,R}`) into `STAND`. Lateral travel at
the end of the last dive row is ±4.38 m (man, manw) and ±4.99 m (woman,
womanw); playing time 3.13 s from `ANTIC_*` and 3.60–3.63 s from
`WALK_*` at the designed 30 fps (the 44-frame ground recovery is
1.47 s of it). Left dives are `+` in the csv; in the rig frame the
clips' root-channel X drift goes negative for left (the figure faces
−Z, so +X is its right). The first dive clip's own root drift is
shorter than its csv distance (man `ANTIC_LDIVE`: 0 → −1.23 against
2.2) while `LDIVE_GROUNDL` starts at −2.22 — the clips do not chain
continuously, so the runtime applies the csv distances (linear per row,
DSN-89) and holds them through the ground rows. The sensing rule that
picks these states is recovered in part, see "Car sensing" below (UNK-43).

## Car sensing (F19-B.7, read from `Midtown2.exe`)

Addresses are this executable's. The walker class's per-frame update is
`0x54b9a0`; its reaction state is the word at +0x10 (0 walking, 1 wary,
2 diving), its target-car index +0x38, squared distance +0x34. Anim
names are resolved at `0x54b200..` (`WALK_STAND`, `STAND_ANTIC`,
`ANTIC*`, `ANTIC_{L,R}DIVE`, `WALK_{L,R}DIVE`, `RUN`, `BACKUP`, ...).

- **Which cars**: the loop at `0x54ba4a` walks the linked list headed
  at the global `0x6b1f60` (the manager at `0x6b1df0` +0x170) through
  each node's +0x2c, keeping the nearest by horizontal (x,z) squared
  distance. A node's car actor is at +0x14 and its player index word at
  +0x8 — the index into the four-slot player table at `0x6b1df0` +0xb0
  (stride 0x30) that the accessor `0x534ad0` returns (it asserts
  `Returning a NULL Player. Idx: %d`, `0x5d652c`). The walker therefore
  senses **player-driven cars only**: AI opponents sit in separate
  managers (the opponent accessors `0x534920`/`0x534970`) and ambient
  traffic has no player slot, so neither is ever a node.
- **Range and speed**: nothing happens at squared distance ≥ 1225
  (35 m) (`0x54bc1a`); the car's speed (its vtable +8) must exceed 1.0
  (`0x54bc65`, `0x5af364`).
- **Corridors**: `0x54e660` — along-axis distance `L` of the walker
  from the car, signed by a direction flag at car-sim +0x304, must lie
  in (0.25·field(+0x224), 20) (`0x5af584` = 0.25, `0x5d716c` = 20);
  lateral offset must lie inside ±(0.5·field(+0x21c) + 2) (`0x5af360` =
  0.5, `0x5af578` = 2). `0x54e8a0` is the same shape with `L` in
  (0.25·field(+0x224), 35) (`0x5d7170` = 35) and lateral
  ±(0.5·field(+0x21c) + 4). Failing, the distance output is 10000
  (`0x461c3c00`). The two fields are the car's box **full** size — the
  length at +0x224, the width at +0x21c — the same `(2,1,3)` default
  as the `InertiaBox` at +0x228.
- **Time value**: `0x54e630` = (per-car factor at entry +0x24) ×
  (`L` − 2) (`0x5af578` = 2). The factor is `1.0 / speed`: `0x53f400`
  stores `1.0 / sim+0x248` once the speed exceeds `0x5b0d38` = 0.001,
  else 0. The value is therefore seconds, `(L − 2)/speed`.
- **Decision**: near corridor passes → value < 0.75 (`0x5b0854`) ⇒ dive,
  else < 2.3 (`0x5b486c`) ⇒ wary; far corridor passes → value < 2.3 ⇒
  wary (never a dive); else walking.
- **Wary (`0x54d0e0`)**: turns the walker to face the car (`atan2` of
  car − walker) and plays `ANTIC`; a sidewalk probe (`0x468e30`) decides
  between walking on along the curve and stopping.
- **Dive (`0x54d8b0`)** (2026-10-14, #1285): on entry the walker's
  heading is set from the car's axis (`0x54dc09`) and a side value
  (dot of the car's axes with walker − car) is stored at walker +0x30.
  The side (`0x54dc3f`): the car's steering field +0x1554 (`+1` right)
  > 0.85 (`0x5b1e34`) ⇒ `*_RDIVE`, < −0.85 (`0x5b4880`) ⇒ `*_LDIVE`
  (both strict, so exactly 0.85 falls through), else the stored side
  value at walker +0x30 > 0 ⇒ `*_LDIVE`, ≤ 0 ⇒ `*_RDIVE`. `WALK_*DIVE`
  when the walker's current clip is `WALK`, else `ANTIC_*DIVE`; the
  state ids are the strings resolved at `0x54b200..` (`WALK_RDIVE`,
  `ANTIC_RDIVE`, ...).
- **Bound in the port**: `ReactPolicy` sense range 35 m, min speed
  1 m/s, alert 2.3 s, dive 0.75 s; both corridor shapes with their
  recovered margins and the nose fraction (`NEAR_RANGE`, `FAR_RANGE`,
  `NEAR_MARGIN`, `FAR_MARGIN`, `NOSE_FRACTION`, `TIME_OFFSET` in
  `mm2_game::pedreact`); player-driven filtering (`Player` without
  `PlayerControl::Ai`); and the steering-based dive side
  (`DIVE_STEER`). Not modelled: the +0x304 direction flag's second sign
  convention (the port senses forward motion), and whether a diver can
  land in traffic stays unknown. Also not yet bound: the loop's
  *nearest-car* selection — the port keeps the most urgent threat among
  the player cars rather than the literally nearest node it walks
  (designed deviation, #1428).

## Car contact (F19-B.8, read from `Midtown2.exe`)

Question: does a retail car physically hit a walker? Answer from the
walker side: nothing in the code reads a contact.

- The walker update `0x54b9a0` dispatches on a reaction word at +0x10
  that only takes 0 (walking), 1 (wary), 2 (diving); the car enters only
  through the sensing tests of "Car sensing" above. A standing walker
  (`0x54bb12..0x54bb87`) wakes into `WALK` when a car comes within twice
  a car-component radius (vtable +0x18) — a proximity wake-up, not a hit.
- No string names a hit, splat, roadkill, damage or score for a walker.
  `Ped %d biffed.` (`0x5d6f20`, pushed at `0x544193`) is the exception
  handler arm of the per-ped loop at `0x544130`; the sibling arms print
  `Vehicle/Opp/Cop/Hookman %d biffed.` for their own loops, so it is a
  fault report.
- `pedRagdollMgr` (`0x5db18c`, checked in the singleton constructor
  `0x57b630`, global `0x6b3720`, vtable `0x5b535c`) is the walker
  animation state-graph player: `0x57b070` advances a time word by the
  frame delta and, past a per-state threshold, enters the next state
  (`0x57b2d0`). It drives the csv chain, not rigid-body ragdolls.
- The walker instance (constructor `0x551b50`, vtable `0x5b490c`,
  size 0x3c) is a level-instance object, and the vehicle broad phase
  **does not filter it out** (traced 2026-10-12, task #1302):
  - `0x6b1f60` is not an instance list; every use (`0x53d3b4`,
    `0x54aa36`, `0x54ba40`, ...) reads the player vehicle's position
    through `[0x6b1f60]+0x14 -> +0xb8 -> +0x90`. The list the broad phase
    walks is the per-room instance list of the level object at
    `0x654d7c` (head `[room entry+4]`, next `[inst+0x10]`, previous
    `[inst+0xc]`, room `[inst+6]`), maintained by `MoveToRoom` `0x465470`.
  - The walker is put in it: the walker actor update `0x54b9a0` calls
    `0x465470` for its sub-instance (`0x54b9f9`, `0x54bf30`, vtable
    `0x5b52f0`, built by `0x57b300`), and the actor's room sync at
    `0x5516c2..0x5516ea` and the walker slot `0x552d40` (+0x28) call it for
    the `0x5b490c` instance itself (`0x5516ea`, `0x552d63`).
  - Per frame `dgPhysManager` (`0x468850` family) calls
    `GatherCollidables` `0x4694d0` (at `0x468ac3`) for each declared
    mover. `DeclareMover` is `0x468360`; its third argument is the mover's
    flag word (`[entry+0x8c]`), `0x1b` for every vehicle call site (player
    `0x41419b`, `0x53d43e`, `0x53df70`, `0x54aae1`, ...). The gather
    keeps an instance when (`flags & 0x18` and vtable slot +0x50 is true)
    or (`flags & 0x2` and slot +0x54 is true), then a sphere test
    `0x4692d0` (vtable +0x4 centre, +0x18 radius) and a cap of 32.
  - The base slots are `0x43fc30` (`[inst+8] bit 4`) and `0x43fc40`
    (`[inst+9] bit 0`); the base constructor `0x4631e0` sets the word at
    +8 to `0x0200`, so a plain instance passes neither. **The walker
    constructor sets `or byte [this+8], 0xd8` (`0x551d01`), which turns
    bit 4 on, and it inherits `0x43fc30`**: with a vehicle flag word of
    `0x1b` the walker passes the filter. By contrast the `0x5b52f0`
    sub-instance overrides slot +0x50 with `0x57b500` (`xor al,al`) and has
    `[+8]=0x0200`, so it never passes.
  - Narrow phase `0x469610(mover, other)` takes each side's vtable +0x64
    (walker: `0x552d00`, which returns `[0x553d40(this)+0xb8]`; `0x553d40`
    indexes the `aiVehicleManager`'s per-model records at
    `[0x6b218c] + 0x18 + idx*0xbc`, `idx` = the walker instance's byte +5)
    and, if either is null, reports `CollideInstances: Attempting to
    collide instances without bounds` (`0x5c5eb4`, branch `0x469a0b`) and
    returns. **The walker's field is non-null** (traced 2026-10-12, task
    #1354): the model record's +0xb8 is written exactly once, in the
    get-or-create `0x553d60` (after the record's `[+0x28]` = `0x4a1110`
    loads the model's data file), by `0x553f3e`, storing a freshly
    constructed 0x318-byte bounds object (`0x5770e0` new + `0x46a260`
    ctor, vtable `0x5b0d98`); the record destructor (`0x554160`/`0x4a0cf0`),
    the record's own methods (all reads), the manager reset loop `0x553fa0`
    and the manager methods never clear it. A load failure blacklists the
    name and returns 0, leaving the walker's +5 = 0 → record 0 (created by
    the first successful load), also non-null in any scene with a vehicle
    model. So the narrow phase is not short-circuited on the walker side and
    yields a contact when the car and walker shapes overlap. A successful
    narrow phase sets `[other+9] |= 0x80` when either side is the player
    mover (`0x4697d6`, `0x4697e1`); the only reader (`0x55b154`) is vehicle
    code and turns it into a car field (`+0x153c` = 2.0 vs 1.0), nothing in
    the walker code reads it.
  So the static answer is: the broad phase pairs a car with a walker
  instance and the narrow phase is live (its bounds guard resolves
  non-null), so retail cars collide with pedestrians when the shapes
  overlap. What is still not shown is the *response*: there is no
  walker-side state for being struck, no hit/splat/score string and no
  animation change, so a contact does not knock the walker down in the
  retail code — only the car reacts (`0x55b154`: `+0x153c` = 2.0 vs
  1.0). Open: OWNER play observation (drive into a walker in a Cruise
  city and describe blocking, knock-down, sound, score).

**The port now runs the contact (F19-B.8b, 2026-10-13).** Each walker
gets one kinematic body — a torso capsule from its bind-pose skin
bounds (radius half the fore-aft depth, bounded 0.2–0.35 m; the T-pose's
~1.9 m arm span is left to the cosmetic mesh) — on a `Pedestrian`
collision layer that meets only the default layer the cars ride on, not
the static world, the scenery movers or other walkers. The authored
walk/dive still writes the walker's `Transform` each `Update`, and
Avian's transform→position sync carries the body with it at zero
velocity, so the body is never a force on the car and the car can never
move the walker: a car that catches a walker is blocked at the body
(bounded, no interpenetration) and the walker plays only the authored
stop/look/dive chain — no struck state, animation, score or sound (F19
req 4, PED-2). `mm2_app`'s `tests/crowd.rs`
(`a_car_catching_a_walker_contacts_its_body_and_its_authored_avoidance_still_runs`)
drives a real dynamic car into a walker and contrasts it with a body-less
figure: the car comes to rest one chassis (2.2 m) and one torso radius
(0.25 m) short of the walker with the body, and drives straight through
without it. The car-side response field `+0x153c` (2.0 vs 1.0,
`0x55b154`) is not remapped — vehicle feel is owner-owned — and is
recorded here rather than implemented.

Conclusion: no walker-side reaction to a contact is evidenced, but the
car-side broad phase does not exclude walkers and the walker's narrow-phase
bounds guard resolves non-null (above), so retail cars do collide with
pedestrians when the shapes overlap. The port implements that as a
designed bounded contact (DSN-89, F19-B.8b): the car is blocked, the
walker is not moved and no walker-side hit state exists, per F19 req 4.

### Which spatial queries see a walker (#1390)

The `Pedestrian` layer meets the default layer only, but a
`SpatialQueryFilter` defaults to every layer, so each query was classified
(designed; handling is owner-owned, so nothing a car's physics reads may
end on a capsule):

- **Must not see walkers**, via `GameLayer::probe_mask` / `probe_filter`:
  the wheel ground probes, the original step's suspension and air-levelling
  rays (`mm2_vehicle::VehicleProbeLayers`, installed by the app next to
  `VehiclePlugin` because the vehicle crate cannot name `GameLayer`); the
  chase camera's min/max clamp and `collide_eye` (a walker crossing the
  eye-to-car ray does not pull the camera in); the recovery anchor and
  landing-ceiling rays (a walker beside the hull must not read as ground
  or as a ceiling).
- **Deliberately see walkers**: the original step's contact re-cast in
  `original_contacts.rs` (it recovers the impact point of a contact the
  solver already has and must land on its partner, a walker included),
  and `banger.rs`'s strike-bound overlap (non-banger hits are skipped).
- **Already blind**: police line of sight (`World` only); the opponent
  wall feelers, the recovery ground test and traffic draping, whose
  predicates accept static or listed ground colliders only (a kinematic
  walker is neither); `seat_level`'s `ground` predicate likewise.
- Tests: `mm2_app` `tests/layers.rs`
  (`probe_filter_ignores_a_walker_on_the_ray_but_sees_the_world`,
  `vehicle_probe_layers_skip_a_walker_and_the_car_itself`).

## Runtime crowd and retail soak evidence (F19-B.2/B.3)

The windowed app and the headless evidence path (`--headless`, the
`peds=` smoke-record fields) run the same four systems
(`mm2_app::crowd`): recycle/refill → react → walk → animate. Density
resolves from the player's pick, else the event's authored `Peds` dial,
else the session default (0.5 → 24 walkers under the 48-walker cap);
the draw stream is seeded from the session seed, so a restart with the
same seed fields the same crowd (F19-AC06). Each walker's body is now a
kinematic torso capsule (F19-B.8b) that blocks a car but is never moved
by one; the walker's own response to a car is still only the authored
stop/look/dive chain.

Measured on retail (fingerprint `e91e6cd4b2ae30d9`), 3600 frames
(60 s of game time) per city, headless, default density, driver
`hold` — `cargo test -p mm2_app --test app pedestrian_retail -- --nocapture`
(spawns `mm2 --headless --city <city> --frames 3600`):

| city | live/target | spawned / recycled | corner hops / turn-arounds | wary / dive / rejoin | ms/frame (3 runs) |
| --- | --- | --- | --- | --- | --- |
| San Francisco | 24 / 24 | 134 / 110 | 25 / 0 | 0 / 0 / 0 | 5.2–8.2 |
| London | 24 / 24 | 108 / 84 | 64 / 22 | 1 / 1 / 1 | 8.5–18.4 |

The census fields are identical run to run (fixed seed); only the
wall-clock column moves with the machine's load. The run stays finite
and inside the actor budget in both cities, the crowd recycles as the
driving player leaves its bubble, and London's run caught a real
wary→dive→rejoin cycle (an ambient car bore down on a walker, which
dove clear through the authored chain and walked back to its curve —
the F19-AC03 movement/reaction half at original-content level;
F19-C owns the rendered inspection). Single no-crowd baseline runs on
the same machine measured 13.2 ms/frame (SF) and 11.1 ms (London);
the machine is shared, so wall-clock figures are order-of-magnitude
evidence, not a benchmark (F30-A owns the frame budget proper).

## Stock-archetype coverage and rendered validation (F19-C)

`mm2-inspect peds` now gives every archetype a runtime coverage
status (F19-AC01): each discovered `pedmodel_*` stem is loaded
end-to-end through `mm2_content::PedArchetype::load` — the same call
the crowd spawner and the `--ped-lab` line-up make — so "supported"
claims the runtime path works, not just that files parse. On retail
(fingerprint `e91e6cd4b2ae30d9`):

```
coverage (F19-AC01), by the runtime loader:
  pedmodel_man: supported — runtime loads 24 clips, 48 paint jobs
  pedmodel_manw: supported — runtime loads 24 clips, 24 paint jobs
  pedmodel_woman: supported — runtime loads 24 clips, 48 paint jobs
  pedmodel_womanw: supported — runtime loads 24 clips, 24 paint jobs
  pedmodel_wolf (extra): missing — anim/pedmodel_wolf.mod absent
coverage summary: expected 4 — supported 4, missing 0, unsupported 0
```

`--strict` now also fails when any expected archetype is not
supported by the runtime (it still exits 0 on retail); an absent
expected archetype counts as missing, so the denominator never
shrinks. The `inventory` command's pedestrian family accepts on the
same runtime load instead of presence alone (retail: 4 accepted of 4
expected, `pedmodel_wolf` rejected as the authored partial it is).

Rendered inspection (F19-AC02, F19-AC03's visual half): the
`--ped-lab` line-up was captured on retail `sf` (default spawn
`-1319.9,68.6,219.6`), at fixed `--cam` poses and frame counts that
pin each authored hold (`LAB_HOLD_SECS` = 3 s per state, 60 fps):
STAND at frame 90, WALK at 270, ANTIC at 990, the `WALK_LDIVE` chain
mid-pose at 1350 (its request fires at t=21 s; the chain plays 3.6 s).
In every capture all four archetypes stand on the ground the ray
found, with coherent limbs, feet planted (or deliberately airborne in
the dive), authored paint-job colours, and no bind-transform breakage;
the dive frame shows the lateral lunge with arms flung out and the
figure displaced sideways off its slot — the authored chain's ±4.38 m
carry, not a transform glitch. Captures live under
`$CARGO_TARGET_DIR/captures/f19-c/` (not committed: original
content). A plain windowed run (no `--ped-lab` — the lab suppresses
the crowd by design) captured the fielded crowd with walkers on the
sidewalks beside vehicle lanes, its record `peds=24/24 psp=32 prec=8
phop=17 prx=4/4`.

Population stability (F19-AC05/AC06): the retail soak is now also run
twice per city by `pedestrian_retail.rs`
(`retail_session_restart_restores_the_same_seeded_crowd`), and every
crowd counter of the second fresh process matches the first verbatim
(London `peds=24/24 psp=108 prec=84 phop=64 pturn=22 pwary=1 pdive=1
prej=1`, SF `peds=24/24 psp=134 prec=110 phop=25 pwary=0 pdive=0`) —
a session restart restores the density/seed behaviour on retail data.

## Crosswalk crossings (F19-B.6, measured geometry, designed use)

`cargo run -p mm2_app --example crosswalk_probe -- <install> london|sf`
reduces each PSDL `Crosswalk` rectangle (attribute `0x04`, four corner
refs in strip order) to the midpoints of its two short ends and measures
the distance to the nearest BAI sidewalk curve end:

| city | crosswalks | length (m) min / median / max | end → nearest curve end (m) min / median / max | both ends within 4 m / 6 m |
| --- | --- | --- | --- | --- |
| London | 697 | 7.0 / 16.0 / 30.0 | 1.6 / 2.5 / 4.3 | 693 / 697 |
| SF | 648 | 7.9 / 20.0 / 35.2 | 1.1 / 2.9 / 4.3 | 634 / 648 |

Every crosswalk is authored against curve ends at both of its ends, and
the nearest vertex is always an end vertex (the sidewalk curves stop at
the junction mouth). That evidences the *sites*. It does not evidence
the original's walker behaviour: the attribute holds only corner refs,
`Midtown2.exe` strings name only `lvlAiMap::GetSidewalkVertexMulti`, and
neither the `.rays` rows nor mm2hook's pedestrian classes were
available to read. Ruled out as evidence: the crosswalk texture slot
(a texture, no pedestrian data) and the join-radius census above (gaps
spread 1–12 m).

The runtime therefore lets a walker take a crosswalk only where both
ends verify against curve ends (DSN-106, designed); on retail every
rectangle verifies and the 60 s soak shows `pcross=4` (London) and
`pcross=15` (SF) crossings taken, reproduced verbatim by a second fresh
process.

Still open for F19, deliberately: the original's crossing behaviour
(UNK-42 — sites measured, rule unrecovered; ordinary joins crossing
vehicle lanes stay refused),
the rest of the original car-sensing rule (UNK-43 — the corridor
geometry is still the designed constant-velocity test, DSN-89), pedestrian audio, and any
human play-test judgement of the figures' feel (owner evidence).

## Pedestrian audio (F19-B.9, evidence only)

What is established (AUD-15): `Midtown2.exe` builds the names
`%s_fpedvoice%s%d` / `%s_mpedvoice%s%d` under `aud\creaturedata` and
reads `numfemalepedvoicefiles` / `nummalepedvoicefiles`; the retail
`default_{f,m}pedvoice<N>.csv` tables each author a single
`Min speed,Max speed,min time in range,max time out of range` band
`0,500,0.01,0` of `*SCREAM*` samples. Retail pedestrians therefore have
scream-class voices; no footstep or chatter table exists.

Ghidra trace (F19 #1320, `Midtown2.exe` VAs): the `pedvoice` format
strings are used only by `0x53cd70` (female, `%s_fpedvoice%s%d` /
`%s_fpedvoice%s`) and `0x53cde0` (male). Each has a single caller, the
walker constructor `0x54b0b0`, which picks the male or female loader from
the byte at walker+0x1a and loads the `default` table into an audio
emitter embedded at walker+0xc0 (position pointer walker+0x60, near/far
0 and 40.0 m, `+0x100 = 8`). The loader `0x510d00` registers the table in
a global table array and stores its index at emitter+0x64. The walker
update `0x54b9a0` touches the emitter once per frame, through virtual slot
3 (`0x510930`), which only releases a finished sample instance; no
reaction (1 wary / 2 diving) path calls an audio routine. The file-name
suffix is built by `0x50fa90` from a rolling counter. The counts
(`numfemalepedvoicefiles`, `nummalepedvoicefiles`) are read in `0x412730`.

Activation (F19 #1345, same Ghidra method): the sample is started by the
emitter method `0x510cd0(arg)`, not by slot 3. It checks the table index
(emitter+0x64), calls `0x512370` (which, when the sound manager's enabled
flag at `DAT_006b0474+0x25` is set and emitter+0x44 is -1, takes an
instance index via `0x512210` -> `0x50f6c0` and calls the virtual activate
slot `0x510980`), then `0x5128e0(arg)` which walks the table's bands with
the in-range test `0x514f30` and picks a sample with `0x514c90` (random
draw that skips the previous pick, `DAT_006b0558`). `0x510cd0` has four
call sites, all with `this = walker+0xc0` (`LEA ECX,[ESI+0xc0]`), all in
walker animation code: `0x54cd90` when the current animation is `BACKUP`
(`DAT_006b20a0`, then sets `BACKUP_WALK`); `0x54d8b0` in the away-facing
branch before entering the animation at `0x5d7288` (name not decoded);
and `0x54d8b0` twice immediately after choosing a dive animation
(`ANTIC_LDIVE`/`ANTIC_RDIVE`/`WALK_LDIVE`/`WALK_RDIVE`). Two sites
push 0 (`0x54ce12`, `0x54de04`'s decompile); the two dive sites push `EBX`
(decompiled as 0, not traced further). So the retail
scream is tied to a walker starting a dive or backing away.

What is not: the meaning of the `0,500` speed band (the decompiled loop
does not read `arg`), the numbered-file choice, and the `EBX` argument
at the dive sites.

The port binding (F19-B.9, #1366; DSN-109) keys a deduplicated one-shot
to the evidenced transitions and marks every unrecovered part as
designed: `crate::audio::pedestrian_voices` voices a `PedScream` that
`crate::crowd::react_pedestrians` writes on `Order::Alert` (the port's
wary/back-away stop, the nearest analogue of retail's `BACKUP`) and
`Order::Dive`, from the base `default_{f,m}pedvoice1.csv` tables (the
numbered-file choice stays unrecovered), with the archetype stem
(`...woman...`) picking the gender (retail's walker byte is unrecovered),
a seeded `NavRng` draw over every authored sample that skips the previous
pick (the `DAT_006b0558` behaviour), the authored row volume, and the
`0,500` band taken as always covering. A per-walker clip window paces
repeats (the dedup) and the live voices are bounded.

Still open: an OWNER retail play observation (dive in front of a car with
the volume up) would settle which sample a real dive draws, and whether
the sound manager ever drops the start (`DAT_006b0474+0x25`).
