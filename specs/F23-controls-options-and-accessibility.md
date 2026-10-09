# F23 - Controls, options, device handling, and accessibility

## Outcome

Make the game reliably controllable and configurable beyond the initial keyboard bindings.

**Priority:** 8 (lower is earlier, subject to dependencies).
**Input contracts:** F01-A, F16-A
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Preserve normalized input and the existing gamepad path. Audit original option categories, but adapt them appropriately to modern cross-platform devices.

Source keys: R1, R5; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Add rebinding, conflict display, deadzones, sensitivity/axis inversion, analog throttle/brake/steering and selectable transmission policy. Keep device mappings out of physics systems.
2. Implement graphics/window/resolution/scaling, audio bus levels, camera, gameplay assistance and accessibility settings with validation and persistence.
3. Handle focus loss, hot-plug/disconnect, paused/menu navigation, controller ownership and mouse capture without stuck throttle or driving while entering text.
4. Provide keyboard-only/gamepad-only primary navigation, clear focus, scalable text, volume/subtitle controls and reduced-motion options where practical.
5. Audit wheel/force-feedback support by actual platform/device capability. Report unsupported hardware honestly; do not promise device testing not performed.
6. Apply settings transactionally where necessary with safe display recovery. Keep developer options and cheats separate from ordinary gameplay/online legality.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F23-AC01** - A remapped binding persists after restart and affects the normalized input path.
- [ ] **F23-AC02** - Focus loss and controller disconnect clear inputs without leaving throttle/steering stuck.
- [ ] **F23-AC03** - Menu/text input never drives the car; gamepad focus remains usable after hot-plug.
- [ ] **F23-AC04** - Invalid settings are rejected/recovered without a black-screen trap or corrupt profile.
- [ ] **F23-AC05** - Audio and graphics controls have actual observable effects rather than only updating UI values.
- [ ] **F23-AC06** - Device/platform verification is recorded per capability, with untested wheel/feedback support explicitly labeled.

## Edge cases to cover

Duplicate bindings; mixed keyboard/gamepad activity; trigger range conventions; invalid display mode; gamepad reassignment; minimized multiplayer client.

## Suggested small implementation slices

### F23-A

Implement persistent input/settings schema and rebind/device normalization.

### F23-B

Wire real options, UI focus/device transitions and accessibility controls.

### F23-C

Test restart, focus loss, disconnect/reconnect, invalid settings and supported device paths.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No claim of universal racing-wheel/force-feedback compatibility without hardware evidence.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
