# F28 - City-specific transport, animated objects, and boundaries

## Outcome

Audit and restore the city-specific moving/interactive features that generic static geometry and car traffic do not cover.

**Priority:** 11 (lower is earlier, subject to dependencies).
**Input contracts:** F09-B, F03-B, F01-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Treat cable cars, trains, bridges, animated set pieces, unusual movement paths, water/recovery areas and hidden/event objects as an inventory to verify, not an assertion that every example exists in each city.

Source keys: R3, R4, R5; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Build a city-by-city list of expected special actors and their evidence/content sources. Distinguish static landmarks from moving/interactive content.
2. Import special routes, schedules, pivots and animation/control data actually present. Use domain-specific controllers rather than driving a train through the generic car-following logic.
3. Implement motion and collision coherently for moving platforms/transport: relative contact velocity, safe riders/player interaction, correct level/route and finite loops.
4. Handle city boundaries, water and recovery zones with the shared recovery/race policy, avoiding invisible walls or generic respawn planes replacing known content semantics.
5. Include stable IDs/authoritative state for network relevancy and late join. Cosmetic animation may be locally reconstructed only when it cannot diverge gameplay.
6. Reuse object/resource cleanup and session seeding; avoid bespoke global singleton state per landmark.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F28-AC01** - Each expected special actor is inventoried, implemented or explicitly unresolved with a source reference.
- [ ] **F28-AC02** - Representative authored moving actors traverse their routes/pivots with correct collision and no teleport discontinuity.
- [ ] **F28-AC03** - Player interaction with a moving collider remains physically stable and material/contact data is consistent.
- [ ] **F28-AC04** - Water/boundary recovery follows the session/race policy and cannot manufacture event completion.
- [ ] **F28-AC05** - Restart and late join restore a coherent actor phase/state.
- [ ] **F28-AC06** - Both cities' special-content coverage is reported, without inventing actors absent from the installation.

## Edge cases to cover

Wraparound path; stopped moving platform; player trapped between moving/static shapes; route crossing; actor outside one client's relevance.

## Suggested small implementation slices

### F28-A

Audit special city actors/boundaries and parse their actual content/control data.

### F28-B

Implement verified movement/interaction/recovery controllers with stable state.

### F28-C

Validate original special-content coverage, collision interaction and restart/network phase.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No new moving landmarks or city redesign solely to make the scene busier.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
