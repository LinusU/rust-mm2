# Architecture

The engine is a Cargo workspace with strict dependency direction:

```
                ┌─────────────┐
                │  mm2_app    │  Bevy executable (winit/wgpu window,
                │  (binary)   │  input, cameras, city import, glue)
                └──────┬──────┘
        ┌──────────────┼───────────────┐
        ▼              ▼               ▼
  ┌───────────┐  ┌───────────┐  ┌────────────┐
  │ mm2_game  │  │mm2_vehicle│  │ mm2_assets │
  │ domain    │  │ Avian sim │  │    VFS     │
  │  state    │  └─────┬─────┘  └─────┬──────┘
  └───────────┘        │              │
                       │              ▼
                       │        ┌────────────┐
                       │        │mm2_formats │
                       │        │  parsers   │
                       │        └────────────┘
```

Dependency rules:

- `mm2_formats` depends on **nothing** project-local. Pure byte-level parsing;
  no Bevy, no Avian, no filesystem assumptions. All parsing errors are
  structured (`thiserror`) — malformed external data must never panic.
- `mm2_assets` depends on `mm2_formats` only (for archive mounting). It owns
  the virtual filesystem and knows nothing about rendering.
- `mm2_vehicle` depends on Bevy + Avian but **not** on `mm2_formats`/
  `mm2_assets`. Driving math lives in `sim.rs` as pure functions so it can be
  unit-tested without a physics world.
- `mm2_game` holds shared domain state (`WorldMode`, markers). Deliberately
  small; gameplay systems grow here later.
- `mm2_app` is the only place where everything is allowed to meet. Bevy
  conversion of parsed formats (TEX → `Image`, PSDL/PKG → `Mesh`) lives here,
  not in the parser crates.

## The logical asset pipeline

```
logical asset  ──►  resolver (Vfs)  ──►  selected source  ──►  importer  ──►  runtime asset
 "texture/foo"      priority+ext        archive / dir / mod    TEX/PNG/…     bevy::Image
```

- Game code never opens physical filenames. It asks the `Vfs` for a *logical
  stem* plus a preference-ordered extension list
  (`resolve_preferred("texture/foo", &["png","ktx2","tex"])`).
- A source is anything that can list logical paths and read bytes:
  `ArchiveSource` (DAVE `.ar`), `DirSource` (loose files, override dirs,
  mods). New source types plug in without changing callers.
- Resolution is deterministic: highest `priority` wins; equal priorities are
  won by the later mount. Within extension alternatives, priority is checked
  first (a mod's `foo.png` beats the archive's `foo.tex`), then extension
  preference (inside one source, `foo.png` is preferred over `foo.tex`).
- Every resolved asset reports provenance (`ResolvedSource`: kind, physical
  path or archive+offset) for diagnostics and debugging.

## VFS mount order (mm2_app)

| tier          | source                        | priority          |
|---------------|-------------------------------|-------------------|
| mods          | `--mods <dir>` subdirs        | `300 + index`     |
| loose files   | files inside the install dir  | `10`              |
| archives      | `*.ar` in the install dir     | `0`               |

`mm2_assets::priority` defines the named tiers (`ARCHIVE`/`LOOSE`/`OVERRIDE`/
`MOD`); the exact ordering between the four retail archives is intentionally
*not* baked into the library — the app mounts them in sorted order and later
mounts win ties.

## Vehicle simulation

- One dynamic `RigidBody` (chassis) + four raycast wheels. Wheel probes use
  `SpatialQuery::cast_ray` against the world, excluding the car itself.
- Forces (spring/damper suspension, longitudinal/lateral tire forces, drag,
  downforce, yaw-stability and air-control torques) are applied through
  Avian's `Forces` query data inside `PhysicsSchedule`
  (`PhysicsStepSystems::First`), at the fixed 120 Hz timestep. Rendering rate
  does not affect handling; `TransformInterpolation` smooths visuals.
- All handling numbers live in `VehicleConfig` (serde-friendly): mass,
  center of mass, wheelbase/track, per-wheel position/radius/flags,
  suspension, engine torque curve, gearbox, tire grip curves, steering,
  brakes, aero and assist strengths. Systems contain no magic constants.
- Input is a normalized `VehicleInput` component (throttle/brake/steering/
  handbrake ∈ 0..1 / -1..1). Physics systems never read devices; `mm2_app`
  maps keyboard/gamepad into the component.

## City import (current, early)

`mm2_app::city` converts parsed data into Bevy assets at load time:

- PSDL room geometry → `Mesh` (positions/normals/UVs from attribute data;
  textured with resolved TEX → `StandardMaterial`).
- INST placements → PKG prop meshes instanced per placement.
- Room/attribute semantics that are not yet understood are preserved in the
  parsed structures but skipped for rendering.

Known gaps are tracked in `docs/research/psdl.md`; the importer is
intentionally simple until the format knowledge improves — robustness of the
VFS/parsers takes priority over visual completeness.
