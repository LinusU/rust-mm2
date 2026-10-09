# F04 - Knockable and breakable world objects

## Outcome

Make appropriate world objects react to impacts, break or fall, and reset correctly without destabilizing the physics world.

**Priority:** 4 (lower is earlier, subject to dependencies).
**Input contracts:** F03-B, F01-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Research authored banger/bound/breakable metadata and original observations. A part name or visible mesh alone is not a sufficient breakage rule.

Source keys: R3, R4; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Map verified mass, friction, elasticity, impact thresholds and replacement parts into an explicit prop state machine: intact, activated/moving, broken, sleeping/removed.
2. Apply Avian impulses once per meaningful impact/transition. A sleeping or static-to-dynamic object must preserve its transform and not overlap a duplicate collider.
3. Use bounded piece counts, suitable convex/compound collision, sleeping and cleanup. Physics LOD must not erase persistent gameplay state such as a removed obstacle.
4. Emit deduplicated impact/breakage events for audio, particles and later replication. Keep fragments cosmetic unless they genuinely affect gameplay.
5. Associate every state with a stable placement ID and session generation. Reset/restart reconstructs the original state without leaking parts or accumulating bodies.
6. Preserve distinct authored behavior: fixed monuments should not topple because a generic dynamic flag was applied to every PKG.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F04-AC01** - Below-threshold and above-threshold impacts have different tested outcomes.
- [ ] **F04-AC02** - A knockable object becomes dynamic at most once; duplicate contact messages do not duplicate bodies or shards.
- [ ] **F04-AC03** - An appropriate breakable object switches to its authored or documented fallback pieces and produces one logical break event.
- [ ] **F04-AC04** - Repeated collisions and cleanup stay within configured body/effect budgets with finite transforms.
- [ ] **F04-AC05** - Session restart restores the same initial placements and removes all session fragments.
- [ ] **F04-AC06** - Two test clients eventually receive identical authoritative prop state when F26 is integrated; until then the replication contract has deterministic IDs.

## Edge cases to cover

A car rests against an object; repeated contacts; stacked props; sleeping on a moving surface; reset during breakage; missing optional fragments.

## Suggested small implementation slices

### F04-A

Parse/classify authored breakable-prop behavior and define stable state transitions.

### F04-B

Implement activation, impulses, breakage pieces, cleanup, and semantic events.

### F04-C

Run impact-threshold, repeated-collision, reset, and resource-budget tests on synthetic and representative original objects.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No destructible buildings everywhere or unbounded particle/fragment spawning. Do not synchronize every cosmetic shard.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
