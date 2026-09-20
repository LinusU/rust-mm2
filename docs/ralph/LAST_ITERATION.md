# Last implementation iteration

- Task ID and title: F06-A.2 — surface identity through collider
  import (the runtime-identity leg of F06-A).
- Starting commit and resulting commits: started at
  `a74afc07cdacb02a9584f6c20750a66d34bd1e75` (externally checked
  F06-A.1 handoff; branch `ralph/night`, clean tree).
- Why this slice: F06-A.1 parsed and audited the authored tables but
  nothing consumed them — the plan's next-slice policy lists the F06-A
  runtime legs first. This is the smallest real consumer: preserve the
  authored material index on collision geometry so the already-wired
  contact pipeline (`WheelState.contact_entity` → `SurfaceMaterial` →
  `SurfaceState`) reports real surface identity instead of the
  `Unspecified` default everywhere. The traction leg (F06-B) now has
  a classified input to consume.
- Retail install: `/Users/linus/coding/rust-mm2/retail`
  (`fnv1a64:e91e6cd4b2ae30d9`).

## What changed

- **`mm2_content::surface` (new):**
  - `load_surface_tables(vfs)` — resolves `city/materials.{mtl,csv}`
    through the VFS. Absent pair → `Ok(None)`; a half pair is
    `Err(Missing)` (a broken table, not an absent feature); UTF-8 or
    parse failure on either half is `Err` — never a silently partial
    classification.
  - `SurfaceTables` (`Resource`) — `set` (`MaterialSet`, the authored
    index space `SurfaceMaterial::Authored(i)` refers to) + `map`
    (`MaterialMap`). `slot_for` classifies one texture name:
    `Material(i)` / `Default` (`none` row) / `Blank` / `Unmapped`
    (absent name or dead csv→mtl ref), with the `<stem>-NNNN`
    frame-base fallback shared with the audit. `resolve_psdl` maps a
    whole texture table and collects the unmapped-name set.
    `issues()` = `validate()` + `undefined_refs`.
- **`mm2_app::city`:**
  - `emit_psdl(psdl, surfaces: Option<&SurfaceTables>)` — collider
    accumulation is now `BTreeMap<Option<u16>, ColliderBuilder>` per
    room, keyed by the authored material index the emitting
    attribute's texture slot resolves to (`collider_rel`/`collider_at`
    helpers mirror the mesh-group `builder`/`builder_at` keying).
    `RoomCollider.surface` carries `Authored(i)`/`Unspecified`.
    `None` tables preserve the pre-F06 single `Unspecified` collider
    per room.
  - `load_city` loads the pair, warns + all-`Unspecified` on failure,
    spawns `SurfaceMaterial` on every collider entity, and returns it
    in `LoadedCity.surfaces`.
  - `CityReport.surfaces: SurfaceReport` — `loaded`, `failure`,
    named/none/blank counts, `unmapped` name set, `issues`; included
    in the `Display` summary.
  - `session.rs` inserts `SurfaceTables` as a session resource on load
    and removes it on teardown (same discipline as `CityNav`).
- **`mm2_game::surface` docs** — `Authored(i)` now names the
  `MaterialSet::defs` index space; semantics still unverified
  (UNK-23).
- **Fallback policy (implementation choice, not verified original
  behavior):** `none` rows, blank slots, unmapped names and dead refs
  all carry `Unspecified`; unmapped names/issues are reported, not
  hidden (F06-AC04's runtime leg). Where two visual faces share one
  physical wall (facades, dividers, tunnels), the interior-facing
  texture slot supplies the collider material — marked unverified.

## Tests

- `mm2_content/tests/surface.rs` — 5 tests: complete-pair
  classification of every slot kind (named/`none`/blank/unmapped/
  dead-ref/frame-stem, including the 4-digit rule), `resolve_psdl`
  slot+unmapped output, absent pair → `None`, half pair → `Missing`,
  unparseable half → `Parse` error.
- `mm2_app/tests/import_pipeline.rs` — 3 new tests:
  `emit_psdl` groups one room into three colliders (road→Authored,
  sidewalk-slot→Authored, unmapped fan→Unspecified) with correct
  report counts, and reverts to one `Unspecified` collider without
  tables; `load_city` spawns `SurfaceMaterial` on every collider
  entity and returns the tables; a broken pair warns → all
  `Unspecified` + `failure` recorded.
- `mm2_app/tests/surface.rs` (new) — real Avian `cast_ray` into each
  region of a synthetic multi-surface city reports that region's
  authored surface: road→`Authored(cobblestone)`,
  sidewalk→`Authored(grass)`, unmapped fan→`Unspecified`. F06-AC01's
  collider leg on the production import path.

## Commands actually run and results

- `cargo fmt --all -- --check` PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` PASS.
- `cargo test --locked --workspace` — all groups, 0 failures.
- `mm2 --mm2-path <retail> --city london --headless` — `status=pass`
  (updates=600, ticks=1200, impacts=3 dropped=0, peak 29.0 m/s, moved
  84 m): real city loads with the pair present; the import report
  logs `surfaces: 152 named, 308 none, 6 blank, 3 unmapped (2
  issues)` — matching the measured PSDL classification; 1341 rooms →
  1559 (room, surface) colliders.
- `mm2 --mm2-path <retail> --city sf --headless` — `status=pass`
  (impacts=3, peak 37.2 m/s, moved 171 m): `surfaces: 148 named, 301
  none, 6 blank, 2 unmapped (2 issues)`; 1171 rooms → 1538 colliders.

## What this proves / does not prove

- Proves: authored surface identity now reaches collider entities on
  the real import path and survives into contact queries — a wheel
  ray into a region reports that region's material (AC01, collider
  leg). Unknown/unmapped content is classified, reported and given an
  explicit conservative surface rather than silently merged into a
  named material (AC04, runtime leg). Cosmetic texture regrouping is
  independent of physical grouping.
- Does not prove: any traction difference — no force-path consumer
  reads `friction`/`elasticity`/`drag` yet (F06-B); the original's
  `_default` policy for unmapped names, dead refs and `sliver` stays
  unverified (UNK-23); the chosen wall/interior texture rule is a
  provisional pick. F06-AC02/03/05/06 all remain open (wetness/snow
  state, texture-swap physics invariance at runtime, tire/audio/dust
  consumers, session+network authority).
- Acceptance IDs: F06-A's identity leg is implemented end-to-end on
  synthetic and retail content; F06-AC01 partially (collider+ray leg,
  wheel integration unchanged), AC04 advanced (runtime classification
  + reporting leg). F06 overall stays `active`.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
