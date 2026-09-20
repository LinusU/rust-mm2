# Architecture

The engine is a Cargo workspace with strict dependency direction:

```
                ┌─────────────┐
                │  mm2_app    │  Bevy executable (winit/wgpu window,
                │  (binary)   │  input, cameras, city import, glue)
                └──────┬──────┘
                       ▼
                ┌─────────────┐
                │ mm2_content │  VFS scans, catalogs and
                │  (producer) │  tuning→runtime conversion
                └──────┬──────┘
        ┌──────────────┼───────────────┐
        ▼              ▼               ▼
  ┌───────────┐  ┌───────────┐  ┌────────────┐
  │ mm2_game  │  │mm2_vehicle│  │ mm2_assets │
  │ domain    │  │ Avian sim │  │    VFS     │
  │  state    │  └───────────┘  └─────┬──────┘
  └─────┬─────┘                       │
        │                             ▼
        │                       ┌────────────┐
        └──────────────────────►│mm2_formats │
                                │  parsers   │
                                └────────────┘
```

Edges are simplified for readability: `mm2_app` may depend on every
crate, `mm2_content` also reads `mm2_formats` parsers directly plus the
`mm2_game`/`mm2_vehicle` types it fills in, and `mm2_game` holds the
`Mm2Vfs` handle — a type-level edge, not content access. The crate
`Cargo.toml`s are authoritative.

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
- `mm2_game` holds shared domain state and contracts: `WorldMode`, the
  typed `SessionConfig` a session starts from (world, mode/event,
  difficulty, conditions, densities, seed, vehicle, authority — with
  local developer overrides quarantined in `DevOverrides`), the
  `Session` lifecycle state machine (`Menu → Loading → Ready →
  Countdown → Playing → Paused/Results → Unloading → Menu`, `Failed` on
  load errors), `SessionEntity` ownership markers for teardown, and the
  fixed-step session clock. It also owns the shared contracts gameplay
  and a future network layer read: stable `PlayerId`/`ObjectId`/
  `ImpactId`/`ResultId` identity (distinct from Bevy `Entity`, minted by
  the session with its generation), `AuthorityRole` marking who may
  simulate an object, `SurfaceMaterial`/`SurfaceState` (authored codes
  carried uninterpreted, physical material kept separate from visual
  identity), `ImpactEvent`+`ImpactPolicy`/`ImpactDedup` (bounded,
  deduplicated contact semantics — never raw solver spam),
  `VehicleTelemetry`/`WheelTelemetry`/`DamageSignals` (read-only
  per-step snapshots stamped with generation+tick),
  `SessionResult`/`ResultLedger` (stable result identity with
  deduplication), and the shared race contract: `RaceDefinition`
  (checkpoints/finish/start slots/laps/countdown), `Checkpoint`'s swept
  cylinder trigger test, `CheckpointRule` (`AnyOrder` vs `Ordered` —
  mode data, not an imposed rule), `RaceState` (generation-stamped
  countdown/clock resource), `RaceProgress` (per-participant clearing
  state with explicit teleport segment breaks), `RaceStarted` and
  `SessionOutcome` (finish provenance on results). `mm2_game` may
  reference `mm2_formats` types (e.g. authored event-table kinds) and
  carry the `Mm2Vfs` resource handle so app-side systems can reach
  content, but it never lists, reads or parses anything itself —
  VFS-backed scans and file parsing live in `mm2_content`.
  Deliberately small; gameplay systems grow here later.
- `mm2_content` is the content→runtime producer layer. It scans the
  mounted VFS and runs `mm2_formats` parsers to build what the app
  consumes: `VehicleCatalog`/`load_vehicle`/`build_model` for vehicles,
  `convert` for tuning→`VehicleConfig`, and `EventCatalog::scan` for a
  city's authored events. `race_def::race_definition` turns a resolved
  `CatalogEvent` (parsed waypoint/start-grid records retained on the
  catalog entries) into an `mm2_game::RaceDefinition`. It fills in
  `mm2_game` contract types (`EventRef`, `EventTableKind`) and
  `mm2_vehicle` configs, but owns no session state, no Bevy rendering
  and no Avian internals.
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
  handbrake ∈ 0..1 / -1..1) plus an explicit `forced_gear` override for
  consumers that pin the gearbox (clamped to the top gear; `None` resumes
  the automatic selector). Physics systems never read devices; `mm2_app`
  maps keyboard/gamepad into the component and clears it on focus loss or
  while the free camera is active.
- Brake-to-reverse is an explicit `DriveDirection` state with a near-stop
  hysteresis band — no oscillating around a single speed threshold.
- Wheel telemetry is honest about the arcade model: `traction_demand` is
  force utilization against the grip limit, not measured wheel slip (there
  is no wheel-speed state); it is clamped and signed by direction. Each
  `WheelState` also records `contact_entity` — the entity its ray probe
  hit — so surface lookups can resolve what the wheel is driving on.
  `VehicleState.engine_load` reports delivered drive force vs. the
  demandable maximum (0 while airborne), and `vehicle_bundle` carries
  `CollisionEventsEnabled` so chassis contacts reach the impact pipeline.

## Session lifecycle and how features attach

`mm2_game::Session` is the phase state machine; `mm2_app::session` drives
it inside the app. The cycle is:

```text
  Menu ─begin(config)─► Loading ─load_session_world─► Ready ─► Playing
   ▲      (drive_session)        (run_if loading)                │
   │                                    │                        │ quit/restart
   │                                    ▼                        ▼  (drive_session)
   │                                  Failed ◄─────────────► Unloading
   │                                    │                        │ (despawn_session_entities
   └─────────────── Menu ◄──────────────┴────────────────────────┘  → drive_session)
        restart = begin again
```

Scheduling in `mm2` (`main.rs`):

| Schedule | Systems |
|---|---|
| `Update` | `load_session_world.run_if(session::loading)` (spawns the whole session; `Ready → Countdown` when an `Event` mode's `RaceDefinition` built, else `Ready → Playing`; `Failed` on error), `session_control_input` (`Esc` quit → exit, `Backspace` restart), `race::update_checkpoint_markers` (hides cleared gates, reveals the finish once all gates clear), `scripted::scripted_drive` (only while the `ScriptedDrive` resource `--bot` inserts exists — owns `VehicleInput` after `vehicle_input`, steering at the live race objective for evidence runs), `despawn_session_entities.run_if(session::unloading)` chained before `drive_session` (despawn flushes, then the driver observes the empty world → `Menu`) |
| `FixedUpdate` | `advance_session_tick` — the gameplay clock, `Playing` only |
| `FixedLast` | `collect_impacts` → `publish_vehicle_telemetry` → `race::reanchor_teleported_participants` → `race::advance_race` (one chain, post-solver). Impacts/telemetry publish `Playing` only; `reanchor_teleported_participants` consumes the `Teleported` marker `vehicle_reset` stamps on every reset/teleport so a `Position` jump can never sweep checkpoints (AC02); `advance_race` drives the shared race lifecycle when a `RaceState` exists — countdown, swept triggers, once-only results |

Ownership rules a feature must follow:

- Every entity a session spawns carries `SessionEntity(generation)`;
  `Unloading` despawns those roots wholesale. Persistent UI/profile/dev
  state must never carry the marker.
- Session-scoped resources are cleared in `drive_session`'s teardown
  (`ImpactFilter::reset` — the dedup map is keyed by `Entity`, which the
  next session may recycle — and `SpawnPoint.trailers`) or rewritten by
  the next spawn (`SpawnPoint.position/yaw`).
- Restart always goes `Unloading → Menu → begin` — `begin` bumps the
  generation, so stale ids (`ObjectId`/`ImpactId`/`ResultId`) and stale
  `Entity` handles from the old session are detectable.
- A failed load is a session too: `Failed` keeps the error visible,
  spawns no player simulation, and quit/restart still tear it down.

How a feature plugin attaches:

- Contract *types* (identity, telemetry, impact, surface, result) live in
  `mm2_game`; the *producers* that turn engine/solver data into them live
  in `mm2_app::contracts` or the feature's app-side module, because only
  `mm2_app` may see both. Producers that turn *file content* into runtime
  data (catalogs, parsed event records) live in `mm2_content`, never in
  `mm2_game`.
- Gameplay systems go in `FixedUpdate`/`FixedLast` and gate on
  `session.is_playing()` so the fixed clock defines their view of time;
  presentation reads `VehicleTelemetry`/events in `Update` and never
  touches the mutable simulation state.
- New session-owned spawns take the `SessionEntity(session.generation())`
  stamp at spawn time; new session-scoped resources register their reset
  in `drive_session`'s `Unloading` branch.

## Contract bridge (`mm2_app::contracts`)

`mm2_game` owns the contract *types*; `mm2_app::contracts` owns the
*producers* that turn engine data into them, running in `FixedLast` after
the physics step and only while the session is `Playing`:

- `collect_impacts` reads Avian `CollisionStart` edges + `Collisions`
  manifolds, resolves participants to stable `ObjectId`s (unmarked static
  world → `ObjectId::WORLD`), and emits bounded, deduplicated
  `ImpactEvent`s — severity from pre-solver normal approach speed, one
  event per physical impact via `ImpactDedup`, capped per tick by
  `ImpactPolicy` with drops counted in `ImpactFilter`. Emitted impacts
  also accumulate `DamageSignals` on the participants.
- `publish_vehicle_telemetry` inserts the read-only `VehicleTelemetry`
  snapshot on each simulated vehicle, stamped with session generation,
  fixed-step tick and authority role. Presentation (the HUD) reads this
  snapshot, not the mutable `VehicleState`.

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
