# F09 - BAI roads, intersections, and reusable navigation

## Outcome

Build a reliable shared navigation representation from original road and lane data, retaining elevation and room relationships.

**Priority:** 3 (lower is earlier, subject to dependencies).
**Input contracts:** F00-B, F01-A
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

R3 documents BAI road/lane curves, intersections and active-road sets. Use it as research guidance, not a guarantee about every flag or runtime rule.

Source keys: R3; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Parse BAI and relevant AIMAP/event overrides into typed, validated structures; retain directed lanes, road/room IDs, intersections, connectivity, width, speed and density semantics.
2. Preserve authored travel direction rather than globally swapping lanes by city name. Keep pedestrian/special-transport routes distinguishable from ordinary road traffic.
3. Provide lane sampling, nearest-valid-lane queries, turn connections, reachable route search, and debug overlays. Distinguish overlapping bridges/tunnels by height and room/topology.
4. Separate route choice from vehicle control. Traffic, race opponents, police and pedestrians may reuse navigation but must retain different goals and decision policies.
5. Use deterministic seeded route selection and bounded pathfinding. Detect disconnected components, invalid references and impossible turns; do not repair them by linking arbitrary nearby points.
6. Retain content fingerprints and stable IDs for replay/network validation. Avoid requiring bit-identical cross-platform floating point for navigation.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F09-AC01** - Synthetic straight, curved, one-way, intersection, dead-end and multilevel cases yield legal directed routes.
- [ ] **F09-AC02** - Road IDs/room references are validated and unsupported records appear in the audit.
- [ ] **F09-AC03** - Overlapping bridge and ground lanes do not become connected merely by horizontal proximity.
- [ ] **F09-AC04** - Debug overlays for sampled original roads in both cities match the imported geometry and intended travel direction.
- [ ] **F09-AC05** - Route generation terminates under disconnected/bad input and emits a specific failure.
- [ ] **F09-AC06** - Two consumers can share the graph without changing each other's destination/route state.

## Edge cases to cover

Short/zero-length lanes; left/right naming ambiguity; disconnected road islands; closed event roads; overpass nearest-lane error; special transport lanes.

## Suggested small implementation slices

### F09-A

Parse and validate BAI/AIMAP route and intersection data with independent fixtures.

### F09-B

Implement directed route queries, elevation-aware sampling, and visual diagnostics.

### F09-C

Validate both city graphs and route constraints without inventing missing connectivity.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No generic navmesh replacement of the authored road network. Traffic AI and racers are separate consumers.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
