# F17 - Main menu and complete session flow

## Outcome

Provide a working front end from profile selection to gameplay to results and back, with original feature coverage and modern usability.

**Priority:** 5 (lower is earlier, subject to dependencies).
**Input contracts:** F16-A, F11-A, F01-A, F02-A
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Audit the installed original menu/help tree. Reuse original art only from the user installation; a modern layout is allowed, but feature omissions must stay visible in coverage.

Source keys: R1, R5; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Implement profile selection/creation; Cruise/Quick Race/event selection; city, vehicle, paint and difficulty selection; weather/time/density settings; options; and multiplayer entry points as their backends become available.
2. Display genuine catalog data, unlock state and dependency/load errors. Vehicle display stats and measured handling statistics must be labeled distinctly.
3. Connect buttons to actual domain/session operations. Unimplemented capabilities must be disabled with a reason, not navigate to fake-success screens.
4. Implement loading/progress/failure, countdown, pause/resume/restart/quit, results, rewards and return-to-menu using F01/F16. Avoid UI owning physics/race authority.
5. Support mouse, keyboard and gamepad focus navigation, back/cancel, readable scaling, and restoration of sensible selections. Escape/back must not lose profile state.
6. Provide deterministic UI-flow tests plus rendered screenshots at normal and high-DPI sizes. Do not rely solely on component presence or snapshot images without interaction.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F17-AC01** - A user creates/selects a profile, chooses a car and real event, plays, receives a result/reward and returns to the menu.
- [ ] **F17-AC02** - Restarting the application preserves profile/unlock/settings changes.
- [ ] **F17-AC03** - Keyboard/gamepad alone can navigate primary flows with visible focus and working back/cancel.
- [ ] **F17-AC04** - A missing asset produces an actionable loading failure and a safe route back to the menu.
- [ ] **F17-AC05** - Every audited original menu capability is functional or explicitly tracked as incomplete; no clickable dead-end placeholders count as done.
- [ ] **F17-AC06** - Repeated session transitions do not duplicate players, cameras, audio loops or UI roots.

## Edge cases to cover

No profile; no MM2 data; locked paint; empty modded catalog; disconnect from lobby; hot-plug controller; window resized during loading.

## Suggested small implementation slices

### F17-A

Implement profile/mode/content selection screens against real domain catalogs.

### F17-B

Wire complete load/play/pause/results/reward/menu transitions and accessible navigation.

### F17-C

Execute end-to-end user journeys, audit original menu capability coverage, and inspect rendered layouts.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No placeholder multiplayer button described as working multiplayer; no web UI service required for a native game.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
