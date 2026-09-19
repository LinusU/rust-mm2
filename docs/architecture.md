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
  unit-tested without a physics world. `config.rs` owns `VehicleConfig`
  (serde/TOML, validated before spawn); the optional `--vehicle-config` file
  swaps the whole tuning without recompiling.
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
| app assets    | bundled `assets/` dir         | `200`             |
| loose files   | files inside the install dir  | `100`             |
| archives      | `*.ar` in the install dir     | `0`               |

`mm2_assets::priority` defines the named tiers (`ARCHIVE`/`LOOSE`/`OVERRIDE`/
`MOD`); the exact ordering between the four retail archives is intentionally
*not* baked into the library — `mount_install` mounts them in sorted order
and later mounts win ties. The same `mount_install`/`mount_mods` functions
serve `mm2_app` and `mm2-inspect`, so both see identical resolution.

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
  maps keyboard/gamepad into the component and clears it on focus loss or
  while the free camera is active.
- Brake-to-reverse is an explicit `DriveDirection` state with a near-stop
  hysteresis band — no oscillating around a single speed threshold.
- Wheel telemetry is honest about the arcade model: `traction_demand` is
  force utilization against the grip limit, not measured wheel slip (there
  is no wheel-speed state); it is clamped and signed by direction.

## City import

`mm2_app::city` converts parsed data into Bevy assets at load time. The
format work is in two layers: `emit_psdl` (pure — PSDL → mesh groups,
collider triangles, spawn point, `CityReport`) and `load_city` (ECS —
entities, materials, props).

- **Attributes**: every PSDL attribute family is decoded semantically —
  roads (inline + counted forms), sidewalks, divided roads (flat/elevated/
  wedged/invisible dividers), crosswalks, road/generic/roof fans, slivers,
  facades and facade bounds. Malformed vertex/height references reject the
  whole attribute and are counted in `CityReport.rejected`; tunnels are the
  only deliberately unsupported family (counted, not emitted).
- **Coordinates**: authored `z` mirrors to Bevy `-z` and emitted winding is
  flipped to compensate — verified against retail perimeter/fan orientation
  and road-normal checks (see `docs/research/psdl.md`).
- **Render meshes**: one mesh per (room, texture) — spatially bounded and
  named `city-room<n>-tex<i>`.
- **Collision**: one static trimesh per room from driveable surfaces
  (roads, sidewalks, fans, crosswalks, roofs) plus facade bounds and
  invisible dividers; slivers/facades stay render-only.
- **Spawn**: the road-attribute room nearest the bounds centre; spawn sits
  `1.5 m` above the highest road point there. The player only spawns when
  the city loaded successfully (`load_city` returns `Err` → `Failed`
  state), so the dynamic body never falls through a half-built world.
- **Textures**: `MaterialCache` resolves `texture/<name>` through the VFS
  (`png`/`ktx2`/`tga`/`tex` preference), decodes TEX with all mip levels
  preserved, honours TEX clamp flags (repeat otherwise) and picks
  `AlphaMode::Mask` only for pixels carrying real transparency. The same
  `load_image` path serves the dev world.
- **Props**: `city/*.inst` placements load `geometry/<name>.pkg`; the best
  LOD chunk is rendered per placement and a convex hull of its vertices is
  the collision approximation (lamppost-scale objects; not for large
  monuments). Non-triangle strips are counted in the report.
- **Report**: `CityReport` distinguishes emitted / suppressed /
  approximated / unsupported / rejected content, missing textures and prop
  failures — printed once at load, never per-frame.
