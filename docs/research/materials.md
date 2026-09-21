# Surface-material tables (`city/materials.mtl`, `city/materials.csv`)

The authored surface-material system: `materials.csv` maps a texture
stem to a physics-material name, and `materials.mtl` defines each named
material's physical properties. A surface query in the original
presumably walks texture name → `materials.csv` → `materials.mtl`
(lookup chain inferred; see UNK-23).

Measured on the retail install (fnv1a64:e91e6cd4b2ae30d9, 2026-09-21):

- **One global pair** `city/materials.{csv,mtl}` — no per-city copies;
  both cities share the tables. No other `.mtl` files exist anywhere in
  the VFS.
- `city/<city>/{floors,walls}.csv` are a *different* table sharing the
  texture-stem key space: `neighborhood,name,tile[,type,…]` rows that
  group texture names under named areas — an editor/lookup table, not
  the physics-material map; not parsed by this slice.
- `materials.csv`: 3423 rows — 137 named-material mappings, 3286 `none`.
- `materials.mtl`: 8 material blocks: `deepwater`, `_default`, `grass`,
  `water`, `dirt`, `sand`, `cobblestone`, `wood`.

## `materials.mtl` grammar (verified on retail)

Line-oriented block syntax:

```text
mtl <name> {
    elasticity: 0.500000
    friction: 0.650000
    ...
}
```

- `mtl <name> {` opens a block; the `{` may sit on the next line (no
  retail block does this — the parser accepts both).
- Fields are `key: v1 v2 …`; the `:` is optional — whitespace separates
  otherwise. A lone `}` closes the block; `//` starts a comment (none
  in retail).
- Every retail block carries the same ten fields in authored order:
  `elasticity`, `friction`, `effect`, `sound`, `drag`, `width`,
  `height`, `depth`, `ptxindex` (two integers), `ptxthreshold` (two
  floats).

Field semantics (R3-documented names; runtime consumption unverified —
UNK-23):

- `elasticity`, `friction`, `drag` — scalar physical coefficients.
  `friction` is the traction input F06 needs.
- `effect` — a word; `none` on every retail block (other values
  unknown).
- `sound` — one integer, a sound-class selector: `deepwater`/`dirt`/
  `sand`/`cobblestone`/`wood`/`_default` = 0, `water` = 1,
  `grass` = 2. The class table itself is unrecovered.
- `width`/`height`/`depth` — a ripple/depression volume? `deepwater`
  carries `depth: 100.0` (infinite), `water` `0.1`; non-water
  materials carry small `width`/`height`/`depth` — semantics
  unverified.
- `ptxindex` (two ints) / `ptxthreshold` (two floats) — particle
  specifiers: which wheel-level particle effects play over the surface
  and at what slip thresholds. Values seen: `-1` (none), `1`, `2`,
  `4`, `5`, `6`, `7` in the first slot; `-1`, `2`, `5`, `6`, `7` in
  the second. The particle index space is unrecovered.

Retail values:

| material | elasticity | friction | drag | depth |
| --- | --- | --- | --- | --- |
| deepwater | 0.50 | 0.65 | 0.50 | 100.0 |
| _default | 0.90 | 0.90 | 0.0 | 0.0 |
| grass | 0.90 | 0.90 | 0.0 | 0.1 |
| water | 0.19 | 0.68 | 0.119 | 0.1 |
| dirt | 0.0 | 0.75 | 0.0 | 0.0 |
| sand | 0.90 | 0.90 | 0.0 | 0.1 |
| cobblestone | 0.90 | 0.90 | 0.0 | 0.0 |
| wood | 0.03 | 0.95 | 0.0 | 0.0 |

## `materials.csv` grammar (verified on retail)

Two-column table under a `texture,physics` header, no quoting:

```text
texture,physics
r1_l,cobblestone
s_ocean,deepwater
vp4x4_mud_sd,none
```

- Column 1 is a texture stem (no extension); column 2 a material name
  or the `none` keyword = "no named material".
- Names are not all texture *files*: ~136 rows are semantic-only stems
  that resolve to no `texture/` file (`vp_59cad_blue_bk1`, … — likely
  bound-material/shader names used by PKG surfaces rather than TEX
  files; unverified).

## Coverage against the PSDL texture tables

`mm2-inspect materials <install>` cross-checks each city's PSDL
texture-name table against the map. Two normalization rules matter:

- **Frame stems**: animated textures are stored as `<stem>-NNNN` frames
  (`s_thames-0001`…`s_thames-0030`, `s_pond-…`, `s_ocean-…`) and the
  PSDL table references single frames (`s_thames-0009`,
  `s_ocean-0007`). `materials.csv` keys the *base* stem
  (`s_thames,deepwater`), so a lookup must fall back to the
  four-digit-stripped base — `mm2_formats::tex::frame_base_stem`.
  The visual importer's `load_image_sequence` expands the same
  convention in the other direction.
- **Blank slots**: each PSDL table carries authored empty names (6 per
  city) — counted separately; no coverage is expected of them.

Measured (2026-09-21):

- london: 469 names — 152 named-material (cobblestone×141, grass×10,
  deepwater×1), 308 `none`, 6 blank, 3 not in map.
- sf: 457 names — 148 named-material (cobblestone×136, grass×10,
  deepwater×1, water×1), 301 `none`, 6 blank, 2 not in map.
- Not-in-map names are real textures with no authored row: `sliver`,
  `sf_win_brickyel01_2s_4_l`, `sf_base_tan09_1s_5_l` (london);
  `sliver`, `gw_stc_offwhite_marswin_f` (sf — note the `gw_` prefix;
  the map has `sw_stc_offwhite_marswin_f`). What material the original
  applies to them — `_default`, none — is unverified (UNK-23).

## Audit findings on retail data

- Two rows reference materials `materials.mtl` does not define:
  `transbay_ramp_f → ash` (line 2563) and `s_grass2mud → mud`
  (line 2824) — dead authored references; `mm2-inspect materials
  --strict` fails on them. What the original does with them (error,
  `_default`, undefined-behavior) is unknown.
- `_default`, `dirt` and `wood` are defined but referenced by no
  `materials.csv` row — `_default` is presumably the fallback for
  unmapped names (unverified); `dirt`/`wood` may serve bound materials
  outside the texture map.

## What is *not* in these tables

- Collidability flags — `none` does not mean non-solid (roads are
  `cobblestone`, buildings `none`, and both are solid). Collision is a
  separate attribute (F06 spec R9).
- Environment modifiers — wetness/snow is not in this data (F06 spec
  R3); `vp4x4_snow_*` rows are vehicle-paint names, all `none`.

## Consumers (inferred, UNK-23)

The texture-name → material chain is authored fact; *which* consumer
performs it is not. Candidates seen in the data:

- PSDL room geometry via the texture table (roads → cobblestone,
  grass → grass, water planes → water/deepwater).
- PKG bound materials via semantic-only stems (`vp_*` rows).
- INST/banger colliders — no material link found; likely `_default`.

## Runtime wiring (implemented, F06-A — provisional policy)

`mm2_content::surface::load_surface_tables` resolves the global pair
through the VFS (absent pair → `None`; a present-but-broken or
half-present pair is an error, never a partial classification) into a
`SurfaceTables` session resource — the index space
`SurfaceMaterial::Authored(i)` refers to (`MaterialSet::defs` order).

`mm2_app::city::emit_psdl` classifies each PSDL texture-table name
through the tables (`slot_for`, with the frame-stem fallback) and
splits each room's collider per authored material index instead of
emitting one trimesh per room: every collider entity carries a
`SurfaceMaterial` component, so wheel raycasts and the impact pipeline
read the surface identity off `WheelState.contact_entity` /
contact-entity queries unchanged. Visual mesh grouping stays keyed by
texture — a cosmetic texture swap does not move collision tris between
material groups.

Fallback policy (implementation choice, *not* verified original
behavior — the original's `_default` semantics are unknown): `none`
rows, blank slots, unmapped names and dead csv→mtl refs all carry
`SurfaceMaterial::Unspecified`; unmapped names land in
`CityReport.surfaces.unmapped` and table issues in `.issues`. A
missing pair leaves every collider `Unspecified` (the pre-F06 single
collider per room); a broken pair warns and does the same with
`.failure` set.

`SurfaceTables::issues()` = `validate()` on both halves +
`undefined_refs` — the same counts the audit reports.

## Tire-path consumption (implemented, F06-B — provisional policy)

The tire force path now consumes the authored `friction` field:

- `SurfaceTables::tire_surface(i)` reads `defs[i].friction` and
  normalizes it against the `_default` block's `friction`, producing
  `mm2_vehicle::TireSurface { grip, .. }` — so `_default` lands exactly on
  `1.0` and other materials scale relative to it. Retail numbers
  (`_default` friction = 0.90): `cobblestone`/`grass`/`sand` → 1.0,
  `water` → ~0.76, `deepwater` → ~0.72, `dirt` → ~0.83, `wood` →
  ~1.06. Missing/negative/non-finite values and out-of-range indices
  sanitize to the neutral `1.0` (and to the raw authored value when
  `_default` itself is absent — conservative, not verified).
- `mm2_app::city` attaches `TireSurface` beside `SurfaceMaterial` on
  every collider at spawn; unmarked colliders are the neutral
  reference (no component), so `Unspecified` surfaces drive
  unmodified.
- `mm2_vehicle::vehicle_simulation` multiplies material grip ×
  `TireConditions.traction` (the separate environment term — a
  quarantined `--traction` dev override today, owned by F18 weather
  later) into one `surface_grip` applied to the lateral force, the
  longitudinal limit, the TC cap and the friction ellipse — exactly
  once (F06 spec R4).
- Wheel telemetry and impact events report the environment term in
  `SurfaceState.traction`; the material term stays with the
  `TireSurface`/`WheelState.surface_grip` physics view.

## Drag and elasticity consumption (implemented, F06-B.2 — provisional
policy)

The remaining scalar fields now reach real consumers:

- `TireSurface` gains `drag`: the def's `drag` consumed **raw**
  (`_default` authors `0.0`, so there is no reference to divide by).
  `vehicle_simulation` applies a viscous force per grounded wheel —
  `-v_plane × drag × load` opposing the wheel's motion in the contact
  plane, kept outside the friction ellipse because it is fluid
  resistance on the wheel, not a tire force. Retail: only `water`
  (0.119) and `deepwater` (0.5) carry nonzero `drag`, so dry surfaces
  are untouched. `WheelState.surface_drag` reports the coefficient
  per wheel (0 airborne/unmarked).
- `SurfaceTables::contact_restitution` maps the def's `elasticity`
  into `0..MAX_SURFACE_RESTITUTION` (0.1 — the same conservative cap
  `convert` applies to `vehCarSim.BoundElasticity`: MM2's elasticity
  drove its own impact solver, so it is scaled rather than applied
  verbatim) and `load_city` attaches it as Avian `Restitution` on
  each named-material collider. Retail: `_default`/grass/sand/
  cobblestone 0.9 → 0.09, deepwater 0.5 → 0.05, water 0.19 → 0.019,
  wood 0.03 → 0.003, dirt 0.0 → 0.0. Unmarked colliders keep Avian's
  default. The collider's `Friction` deliberately stays at Avian's
  default — authored `friction` is a tire-grip coefficient, and
  applying it to chassis/prop contact would fight the
  `MAX_COLLIDER_FRICTION` scrape policy.

Retail evidence (install `fnv1a64:e91e6cd4b2ae30d9`, 2026-09-21):
London's `deepwater` colliders are the Thames rooms (~337–364 of
`london.psdl`, surface at y = −4.0 — measured via `emit_psdl` +
`load_surface_tables`). `mm2 --city london --spawn=-80,2,805,0
--headless` drops the car onto that surface, where full throttle
reaches only `peak=1.1m/s moved=11m` in 10 s (dry-land baseline
`peak=29.0m/s moved=84m`, unchanged). The authored `elasticity`
restitution changed two recorded banger scenarios on
named-material streets — the SF perpendicular restage
(`vpddbus --spawn=-141.9,1.5,-608.5,115`) now records
`bng_ev=0a/2s/1b` at 10 000 ticks (was `0a/5s/2b`: fragments bounce
more on the cobblestone/grass colliders there) and the London
reclaim ring (`vpbug --spawn=802,6,-905,180`) records
`bng_ev=3a/3s/0b` (same 3 activations, all now settled — was
`3a/0s`). Both are the feature working, not regressions; the
pre-restitution records are superseded.

This is an *implementation choice*, not verified original behavior:
how the original combines `friction`/`elasticity`/`drag` with tire
parameters is unknown (UNK-23), `_default`-as-divisor is a
convenient normalization, and the restitution cap is a conservative
scaling — none are discovered rules. `sound`, `effect`,
`width`/`height`/`depth` and `ptx*` still have no runtime consumer.
