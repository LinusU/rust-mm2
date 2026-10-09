# F03 - Ramps, poles, trees, decals, and all world-object placement

## Outcome

Populate both original cities and event layouts from all applicable authored placement sources, not only INST.

**Priority:** 3 (lower is earlier, subject to dependencies).
**Input contracts:** F00-B, F01-A
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

An INST/PKG importer already exists. Investigate pathsets, PSDL roadside paths, propdefs/proprules, decals, and race-specific placement through R3 and the local installation.

Source keys: R1, R3; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Inventory which placement mechanism supplies each object family. Parse missing formats in mm2_formats; resolve linked PKGs/textures/transforms through the VFS and shared content bridge.
2. Support verified path spacing, orientation, scaling, seeds/rules, material/paint references, and coordinate transforms. Do not substitute a random scatter of trees/poles for authored placement.
3. Give each placement a deterministic ID including content/session identity and source record. Track city ownership separately from race-only overlays so restart/unload does not duplicate content.
4. Classify fixed, movable, breakable, decal, emissive, and decorative/noncolliding objects explicitly. Use real ramps as collision surfaces; trees and light poles need content-appropriate behavior, not indiscriminate dynamic bodies.
5. Share immutable model/material resources. Keep spatially bounded batches and collision shapes; do not merge the entire city into one giant render/physics entity.
6. Report unsupported placement records, unresolved dependencies, and approximated semantics. Preserve raw records for future work rather than filtering failures out of counts.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F03-AC01** - Synthetic pathset tests verify spacing, rotation, nonunit scale, repeated/shared models, and deterministic IDs.
- [ ] **F03-AC02** - Known sampled locations in both cities contain the expected authored ramps/poles/trees or an explicit unresolved-source report.
- [ ] **F03-AC03** - The player drives onto an actual ramp collider; decorative decals do not create collision walls.
- [ ] **F03-AC04** - Entering and exiting a race twice adds/removes only its event objects and produces no duplicates.
- [ ] **F03-AC05** - Texture and compatible model overrides affect placed props using existing source precedence.
- [ ] **F03-AC06** - Strict placement audit covers every expected source family rather than claiming completion from INST counts alone.

## Edge cases to cover

Empty pathsets; zero-length segments; corner orientation; mirrored/nonuniform transforms; duplicate references; race overlays; unknown rule indices.

## Suggested small implementation slices

### F03-A

Inventory placement sources and add parsers/typed records for missing authored placement formats.

### F03-B

Instantiate city/race objects through shared assets with stable IDs, correct transforms, and collision classification.

### F03-C

Validate sampled original locations, all source records, race cleanup, and mod replacement end-to-end.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No procedural city redesign. Prop damage/dynamics belong to F04; static placement must still be correct before that feature.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
