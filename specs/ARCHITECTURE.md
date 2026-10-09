# Shared architecture and integration contracts

These are desired boundaries. Reuse current types whenever they satisfy the role; do not introduce a duplicate type just because a name below differs.

## Dependencies and ownership

`mm2_formats`: bounded parsing of byte/text formats, engine-independent records, offsets/line errors and preserved unknowns. No filesystem, Bevy or Avian assumptions in parsing.

`mm2_assets`: logical resource identity, VFS, mounted source ordering, bytes/provenance, safe decompression and content fingerprints. It is not a game-state manager.

`mm2_content`: semantic assembly, catalogs, dependency resolution, original-to-runtime conversion, typed definitions and conversion evidence. Existing vehicle/model code is the starting point for shared content facilities.

`mm2_vehicle`: Avian-backed motion from validated runtime configuration/input. No direct archive parsing. Per-vehicle/axle identity and enhanced assists remain explicit.

`mm2_game`: session authority/lifecycle, stable IDs, rules, participants, progression results and reusable domain contracts. Keep unrelated presentation details out.

`mm2_app`: Bevy integration, rendering/audio/UI/input device mapping and application shell. Split feature plugins/modules as real complexity grows; do not put the whole game in main.rs or make the city renderer the owner of every material/audio/race.

Additional AI/audio/network modules or crates are appropriate when they have a clear owner and tested consumers. This is not permission to design a universal engine framework before implementing gameplay.

## Shared contracts to establish in F01

| Contract | Required semantics |
|---|---|
| ContentId / resolved resource | Stable logical identity, selected-source provenance and generation/hash. Never a platform path or array index as permanent game identity. |
| SessionConfig | City, mode/event, difficulty, vehicle/paint, weather and time separately, densities, seed and authority role. |
| PlayerId / NetworkObjectId | Distinct from Bevy Entity and stable over the relevant session; generation prevents stale respawn messages. |
| VehicleInput | Normalized throttle/brake/steering/handbrake plus explicit gear/control commands; no device keys in simulation. |
| VehicleTelemetry | Authoritative or explicitly predicted pose/velocity, RPM/load/gear, wheel contact/slip/material and damage signals. |
| ImpactEvent | Meaningful contact severity, participants, location, surface, tick/identity; bounded and deduplicated, not raw contact spam. |
| SurfaceState | Base physics material plus environment modifiers; keep visual texture identity independent. |
| RaceDefinition / Progress / Result | Mode-specific authored rules and per-participant progress; stable result IDs, authority and generation. |
| ProfileProgress | Versioned persistent IDs, completed results/unlocks/preferences; no reference to transient ECS entities. |

## Simulation and presentation

Physics/rule clocks run on explicit fixed steps; render interpolation never advances authoritative game rules. Audio/effects consume semantic telemetry/events. Replaying predicted simulation cannot award rewards, duplicate sounds or trigger another breakage event.

Document schedule ordering where forces, spatial queries, contact processing, rules and transform interpolation interact. Do not hide ordering problems behind broad ambiguity suppression. Version-specific Avian APIs must be checked against the actual dependency.

World entity ownership should distinguish persistent app/profile resources, city resources, event overlays and per-round actors. Reset and unload operate on these owners, not an indiscriminate despawn of everything.

## Multiplayer direction

Use one host/server authority for vehicles, relevant traffic/props, race/Cops & Robbers rules and environmental traction. Offline gameplay uses the same authority-owned rules locally. Do not require bit-identical cross-platform physics.

Stable IDs, bounded input sequences, snapshots, interpolation and an explicit local-prediction policy are preferred over serializing the ECS. Validate messages and content compatibility. Cosmetic mods do not automatically break network compatibility; gameplay tuning/collision/rule differences do.

Choose the transport against current official documentation and pinned stack during F24. This pack intentionally does not prescribe an unverified current networking-crate version. Never auto-provision a paid relay or public service.

## Asset/rendering direction

Original resource -> VFS selection -> parser -> semantic content assembly -> runtime visual/audio/physics projection. An HD texture override should not require a PKG edit. Model parts, transforms, paint variants, physics bounds and animation associations survive import.

Original diffuse art gets conservative modern materials. Do not synthesize PBR maps, introduce an enhancement pipeline, regenerate all art or require a ray-tracing path. Rendering improvements cannot mask incorrect geometry or missing object state.

## Evidence and traceability

Conversion reports keep source values separate from adapted values. Coverage preserves rejected records. Actual interface/format discoveries go into brief repository research notes, with the source commit/file/observation and confidence. Behavioral acceptance tests must exercise the production path, not a second test-only simulator.
