# F15 - AI race opponents

## Outcome

Provide competitive, recoverable AI participants that use original event routes and vehicle identities.

**Priority:** 5 (lower is earlier, subject to dependencies).
**Input contracts:** F09-B, F11-B, F02-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Race opponents are not ambient cars. Inspect opponent definitions, waypoint/route data, difficulty and vehicle tuning variants.

Source keys: R3, R4; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Load the event's actual participant lineup, route intent and applicable opponent tuning through VFS. Do not clone the selected player car for everyone.
2. Separate route planning from driving control. Use lookahead, curvature-aware speed/braking and obstacle anticipation feeding the same normalized input path where practical.
3. Respect mode-specific checkpoint/lap semantics, including route alternatives when verified. AI must earn progress through the same validation rules as players.
4. Implement overtaking/local avoidance and bounded stuck recovery. Any reset/teleport/rubber-band assistance must be explicit, observable, scoped by rules and not falsely recorded as physical racing.
5. Make difficulty affect documented/tunable behavior and appropriate source parameters. Keep traffic-following, police pursuit and race goals separate despite shared navigation.
6. Track per-opponent progress, stuck duration and recovery actions. Use seeded randomness and finite loops; do not promise every chaotic race ends identically.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F15-AC01** - The event's specified opponent roster and distinct vehicles spawn in valid start slots.
- [ ] **F15-AC02** - Opponents navigate at least a complete checkpoint course and circuit using actual trigger validation.
- [ ] **F15-AC03** - A blocked road/temporary collision leads to bounded recovery, not indefinite stationary vehicles.
- [ ] **F15-AC04** - AI does not gain checkpoints/laps from teleports or invalid direction/height.
- [ ] **F15-AC05** - A fixed-seed race soak records finish/DNF/stuck outcomes and finite vehicle state.
- [ ] **F15-AC06** - Difficulty changes have measured effects, with any catch-up assistance disclosed and tested.

## Edge cases to cover

Faster car behind a bus; route junction miss; persistent player obstruction; overturned opponent; shortcut invalidation; competing recovery positions.

## Suggested small implementation slices

### F15-A

Import opponent rosters/route intent and implement a production-vehicle driving controller.

### F15-B

Add avoidance/overtaking/difficulty and bounded recovery under mode rules.

### F15-C

Complete representative races with AI and validate progress, recovery and finish/DNF reporting.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No invisible cars updating progress numerically, straight-line teleport racers, or undisclosed perfect-knowledge cheats.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
