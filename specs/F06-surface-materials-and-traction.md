# F06 - Surface materials, traction, and contact classification

## Outcome

Provide one surface identity/state path used consistently by tires, effects, audio and weather.

**Priority:** 3 (lower is earlier, subject to dependencies).
**Input contracts:** F00-B, F01-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Investigate city material mappings/definitions and bounds metadata. Visual texture identity, tire grip and chassis friction are different concepts.

Source keys: R3, R4; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Retain source surface IDs through geometry/collider import and query hits. Correctly map a contact triangle/subshape to its material without guessing from a texture filename.
2. Represent base material plus environment modifiers such as wetness/snow separately. Defaults must be explicit and reported for unknown surfaces.
3. Combine per-wheel/axle tire parameters with surface properties in the real Avian force path. Do not modify only displayed grip values or multiply modifiers twice.
4. Expose stable contact descriptors for tire squeal/skid effects, impacts, dust/spray, and route costs where actually needed.
5. Do not make every nonrendered surface noncolliding or every water surface solid. Use explicit material/semantic flags and recovery policies.
6. Keep material lookup/cache read-only in the hot loop where possible. Mods can replace surface configuration independently of cosmetic texture packs.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F06-AC01** - A synthetic mesh with multiple material triangles reports the correct material for each wheel/contact.
- [ ] **F06-AC02** - Two distinct surfaces and a controlled wetness change produce measurable traction differences in a suitable test.
- [ ] **F06-AC03** - Changing only a diffuse texture does not silently change physics material identity.
- [ ] **F06-AC04** - Unknown IDs are visible in diagnostics and use a documented conservative policy.
- [ ] **F06-AC05** - Tire, collision audio and spray/dust consumers see consistent contact classification.
- [ ] **F06-AC06** - Surface state survives session configuration changes and is authority-owned when networked.

## Edge cases to cover

Contacts on triangle boundaries; compound shapes; wheel airborne; blended/unknown materials; mod overrides; water and bridge overlaps.

## Suggested small implementation slices

### F06-A

Import surface IDs and authored material properties into a shared typed representation.

### F06-B

Connect contact queries to tire forces and surface/effect/audio telemetry.

### F06-C

Test multi-material geometry, measurable traction changes, overrides, and unknown-material diagnostics.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No AI-generated PBR material maps or full weather rendering in this feature.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
