# F22 - Driving HUD, maps, cockpit, mirrors, and cameras

## Outcome

Complete the driving interface and navigation experience using original content with readable modern presentation.

**Priority:** 8 (lower is earlier, subject to dependencies).
**Input contracts:** F02-B, F11-B, F01-A
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Audit original HUD/map/camera controls and content. The current telemetry text and chase/free camera are useful developer tools, not the full original interface.

Source keys: R3, R4, R5; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Display speed/units, gear/RPM where applicable, damage, race timer, position/laps/objectives and warnings from authoritative telemetry rather than duplicated calculations.
2. Import original map geometry/imagery and coordinate alignment. Support audited map size/zoom/rotation modes, player/goal/participant markers and checkpoint guidance with correct city transform.
3. Implement original cockpit/dashboard content and moving needles/steering wheel where required. Preserve part/pivot conventions and vehicle-specific camera position.
4. Add appropriate chase/near/far/look controls, rear view/mirrors and camera obstacle handling as verified. Keep free camera explicitly developer-only during competitive/progression sessions.
5. Use interpolation consistently so camera, chassis and wheels do not jitter at different update times. Adjust framing/clipping for tall/long/small vehicles and reset transitions.
6. Keep HUD independent of networking authority and usable at common aspect ratios/high DPI. Add readable contrast/text sizes and alternatives to color-only status.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F22-AC01** - HUD values agree with production telemetry and race state across pause/reset/finish.
- [ ] **F22-AC02** - Known world positions map to correct HUD coordinates/orientations in both cities.
- [ ] **F22-AC03** - Checkpoint/opponent markers update correctly without revealing invalid/stale participants.
- [ ] **F22-AC04** - Representative dashboards animate correct instruments/steering without pivot drift.
- [ ] **F22-AC05** - Camera transitions, wall proximity, mirrors and reset are visually inspected on atypical vehicle sizes.
- [ ] **F22-AC06** - Keyboard/gamepad bindings and scaling work without clipped unreadable UI or accidental driving in free-camera mode.

## Edge cases to cover

Map mirrored; units conversion; parked/free camera; rear-view active during reset; wide aspect ratio; missing optional dashboard; late-join markers.

## Suggested small implementation slices

### F22-A

Implement race/driving HUD and correctly aligned original city maps/markers.

### F22-B

Add cockpit/dashboard/mirror and audited camera controls with interpolation/occlusion handling.

### F22-C

Validate map coordinates, telemetry, per-vehicle camera framing and multi-resolution presentation.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

Do not replace the original map with an unrelated web map or claim cockpit support from an exterior camera.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
