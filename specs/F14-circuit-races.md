# F14 - Circuit races

## Outcome

Make every original Circuit event work with valid lap counting, opponents and completion.

**Priority:** 4 (lower is earlier, subject to dependencies).
**Input contracts:** F11-B, F02-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Course order, lap requirements, event boundaries and difficulty overrides must be verified from original sources, not inferred from names alone.

Source keys: R3, R4, R5; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Import each authored circuit, start grid, lap settings and relevant traffic/props. Use the shared event runtime and checkpoint data.
2. Track ordered progress and lap validation according to verified rules. Crossing the finish line repeatedly, backward, via reset or without the required gates must not accumulate laps.
3. Present lap/current position/timing and navigation cues from authoritative state. Define start-lap handling and tie resolution explicitly.
4. Integrate F15 opponents with sensible lap navigation, recovery, overtaking and race completion. Do not let a stuck competitor prevent bounded result handling forever.
5. Produce exactly one result per participant and a consistent overall result; connect rewards/profile state through F16.
6. Keep online Circuit settings and lap clocks portable to F26 without serializing render-only objects.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F14-AC01** - Every expected original circuit loads with its own lap/route configuration.
- [ ] **F14-AC02** - A complete valid lap counts once; missing gates, reverse traversal, repeated finish hits and teleports do not count.
- [ ] **F14-AC03** - Multiple laps and multiple participants remain independent and yield consistent finish ordering.
- [ ] **F14-AC04** - With F15, a real original circuit can be raced against opponents rather than only driven alone.
- [ ] **F14-AC05** - Restart restores grid, counters, clocks and event objects without duplication.
- [ ] **F14-AC06** - The complete-catalog audit and representative city/difficulty playtests are recorded separately.

## Edge cases to cover

Finish-line spawn; overlapping start/finish volumes; skipped gate; last-lap tie; DNF participant; reset on finish.

## Suggested small implementation slices

### F14-A

Bind original circuit/lap rules to the shared checkpoint runtime.

### F14-B

Implement lap validation, participant ranking, HUD and opponent hooks.

### F14-C

Validate catalog coverage plus multi-lap/opponent/restart and exploit-negative tests.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No universal lap count or identical circuit layout generated for every event.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
