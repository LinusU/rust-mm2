# F19 - Pedestrians, animation, and avoidance

## Outcome

Restore a populated pedestrian environment using original character/animation data and bounded behavior.

**Priority:** 9 (lower is earlier, subject to dependencies).
**Input contracts:** F09-B, F01-B, F00-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

R3 lists MOD/SKEL/ANIM/shader/remap/state-related families. Their runtime role must be verified; a static billboard crowd is not full pedestrian support.

Source keys: R3, R4, R5; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Parse and assemble required pedestrian mesh, skeleton, remapping, animation and state files through the content/VFS pipeline. Preserve unknown records with explicit coverage.
2. Implement animation sampling/blending and authored states. Verify bind transforms and bone mappings on independent synthetic skeletal fixtures.
3. Use pedestrian lanes/navigation and density/seed controls, with bounded spawning/interest management. Keep pedestrians off ordinary vehicle-only lanes except verified crossings.
4. Implement original-style reactions/avoidance and applicable nonimpact behavior based on evidence. Do not turn the game into pedestrian run-over simulation because generic colliders were convenient.
5. Connect contextual reactions/audio and avoid duplicate sound/state events. Authority should govern gameplay-relevant states; distant animation may remain cosmetic.
6. Support pause/unload/reset without accumulating actors or animation resources. Use LOD/culling without visibly teleporting nearby pedestrians.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F19-AC01** - All expected stock pedestrian archetypes/dependencies are inventoried with supported/missing/unsupported status.
- [ ] **F19-AC02** - Representative original characters animate without broken limbs or incorrect bind transforms.
- [ ] **F19-AC03** - A player approaching triggers an appropriate verified avoidance/reaction rather than an unhandled impact.
- [ ] **F19-AC04** - Synthetic pedestrian-route/crossing cases preserve lane/sidewalk eligibility and terminate recovery.
- [ ] **F19-AC05** - A fixed-seed crowded-scene soak stays finite and within actor/animation budgets.
- [ ] **F19-AC06** - Session restart removes prior pedestrians and restores density/seed behavior.

## Edge cases to cover

Missing animation state; remap mismatch; pedestrian at curb; crowd bottleneck; paused animation; two players approaching; nearby actor culled.

## Suggested small implementation slices

### F19-A

Import pedestrian rigs, animation/state data and synthetic animation tests.

### F19-B

Add sidewalk movement, density management and verified player-avoidance/reaction behavior.

### F19-C

Validate stock archetype coverage, animation/reaction visuals and population stability.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No new violent pedestrian mechanics or a fully simulated city-life system beyond original feature scope.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
