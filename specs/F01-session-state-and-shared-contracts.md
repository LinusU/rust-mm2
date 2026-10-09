# F01 - Session lifecycle, simulation ownership, and shared contracts

## Outcome

Give independently implemented features a small common vocabulary and a predictable session lifecycle, without a general-purpose engine rewrite.

**Priority:** 1 (lower is earlier, subject to dependencies).
**Input contracts:** F00-A
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Reuse mm2_game, mm2_content, mm2_vehicle, the current WorldState, and existing ECS components. Stable content IDs are not Bevy Entity values.

Source keys: R1, R4; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Define typed session configuration for city, mode/event, difficulty, weather/time, densities, seed, selected vehicle/paint, and authority. Keep local developer overrides separate from progression and network legality.
2. Define only the shared contracts now needed: stable player/vehicle/prop IDs, normalized vehicle input, read-only telemetry, meaningful impact/surface events, race result identity, and session ownership. Reuse existing structures instead of parallel versions.
3. Separate simulation state and visual/audio projections. Fixed-step gameplay clocks must be independent of render FPS. Offline local authority and network server authority should call the same game-rule systems.
4. Specify menu -> loading -> ready/countdown -> playing -> paused/results -> unloading/menu. Clean up session-owned entities, audio loops, timers, subscriptions, and caches; never despawn persistent profile/UI state accidentally.
5. Make collision/impact events bounded and meaningful, with participants, point, surface, severity, and stable identity/tick. Do not send every solver contact directly to sound/damage/network consumers.
6. Introduce interfaces where two implemented consumers require them. Avoid a speculative universal event bus, serialization of arbitrary ECS state, or every feature editing main.rs.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F01-AC01** - Two consecutive sessions can be started, quit, and restarted without duplicated cameras, players, physics bodies, UI, timers, or sound emitters.
- [ ] **F01-AC02** - A failed load leaves no active player simulation in an incomplete world.
- [ ] **F01-AC03** - Fixed-tick input produces equivalent gameplay events across different render/update batching, within stated tolerances.
- [ ] **F01-AC04** - A race/session result has a stable unique identity so consumers can deduplicate it.
- [ ] **F01-AC05** - Local and simulated remote player identities coexist without single-player query assumptions.
- [ ] **F01-AC06** - Shared interfaces have small integration tests and a documented ownership/scheduling diagram.

## Edge cases to cover

Pause during load; profile change while assets are pending; restart after failure; disconnect during results; duplicate impact/result delivery; focus loss.

## Suggested small implementation slices

### F01-A

Implement typed session configuration and explicit session ownership/lifecycle transitions.

### F01-B

Expose stable IDs, telemetry, surface/impact/result contracts, and local authority boundaries.

### F01-C

Add repeated start/quit/failure and fixed-tick integration tests; document how feature plugins attach.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

Do not create networking, profiles, or full menus here; provide tested contracts they can use.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
