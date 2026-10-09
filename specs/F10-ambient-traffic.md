# F10 - Original-style ambient traffic and signals

## Outcome

Populate both cities with traffic that follows authored lanes, negotiates intersections, reacts to the player and remains physically credible.

**Priority:** 4 (lower is earlier, subject to dependencies).
**Input contracts:** F09-B, F02-A, F01-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Ambient vehicle data may differ from player tuning. Discover it explicitly rather than converting all traffic to the selected player car.

Source keys: R3, R4; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Import ambient models/variants, spawn weights/densities, route speed overrides and intersection/signal rules. Use actual traffic vehicles from user content.
2. Implement bounded seeded spawning on valid offscreen/occluded/cleared lanes; despawn with distance/hysteresis while preserving relevant interactions. Use the union of player interest areas in multiplayer.
3. Implement speed following, curvature-aware braking, following distance, turn choice, right-of-way/signal behavior, obstruction response and stuck recovery. Reactions must be bounded, not an aggressive unconditional teleport.
4. Use a deliberate movement/physics approach: simplified distant traffic is allowed, but nearby cars must collide coherently with players. Transition to dynamic behavior without duplicating bodies or injecting extreme energy.
5. Coordinate lights and intersections from one authoritative controller. Keep ambient routing separate from race opponents and pursuit logic.
6. Expose density/seed/settings in SessionConfig and diagnostics for spawned, culled, stuck, colliding and active cars. Avoid spawning cars inside the player or visibly popping on an empty nearby road.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F10-AC01** - Both cities show multiple original ambient vehicle types driving in the correct authored direction.
- [ ] **F10-AC02** - A synthetic intersection test covers crossing flows, signals/right-of-way, queues and bounded recovery.
- [ ] **F10-AC03** - The player can collide with nearby traffic without driving through it or triggering a physics explosion.
- [ ] **F10-AC04** - Spawn/despawn tests reject occupied space and preserve active interactions near any player.
- [ ] **F10-AC05** - A fixed-seed headless soak reports finite states, bounded entity count and stuck-car outcomes.
- [ ] **F10-AC06** - Race-specific traffic density/speed overrides and session restart are honored.

## Edge cases to cover

Player blocks intersection; multi-car pileup; red light under load; dynamic/kinematic handover; two players far apart; lane closure; signal unloaded/reloaded.

## Suggested small implementation slices

### F10-A

Build ambient-vehicle catalog/spawn policy and a minimal lane-following controller.

### F10-B

Add intersections/signals, player collisions, following/obstruction response and bounded recovery.

### F10-C

Validate original traffic in both cities, multiplayer-ready interest rules, density settings and long-running stability.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No promise of exact original traffic trajectories. Do not count a circle of cars in a synthetic arena as original city traffic.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
