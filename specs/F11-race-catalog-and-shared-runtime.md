# F11 - Original race catalog and shared event runtime

## Outcome

Load the complete original event catalog and provide reusable countdown, trigger, result and restart infrastructure.

**Priority:** 3 (lower is earlier, subject to dependencies).
**Input contracts:** F00-B, F01-A
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Research race tables, event-specific waypoint/data CSVs, AIMAP overrides, opponent files and props. CSV-like content may not fit a default comma-split parser.

Source keys: R3, R4; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Discover events per city/mode/difficulty from original catalogs and metadata. Resolve all references through VFS and retain missing events in expected coverage.
2. Parse authored coordinates, checkpoint shapes/widths/orientation, start slots, timers/laps, opponent references, mode settings and reward links. Mark uncertain semantics instead of guessing from column names.
3. Define a RaceDefinition and authoritative per-participant progress/result model that mode implementations share. Stable event/participant/result IDs must not depend on load order.
4. Implement swept checkpoint crossing so a fast car cannot skip a trigger between ticks; check direction/height/eligible state where verified rules require it. Handle reset/teleport explicitly.
5. Support countdown and input lock, race clocks, pause policy, finish/failure, restart and resource cleanup. Event props/traffic overrides are session-scoped.
6. Provide CLI event listing/selection and inspection before the full menu exists. Proposed names may be added, but the docs must reflect actual implemented syntax.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F11-AC01** - All expected events are listed with content/difficulty/status and failed references; empty catalogs fail strict audit.
- [ ] **F11-AC02** - Synthetic high-speed, wrong-height, repeated-crossing and teleport tests exercise the same trigger path as gameplay.
- [ ] **F11-AC03** - Countdown unlocks exactly once; pause/restart behavior is deterministic and no old timer survives.
- [ ] **F11-AC04** - Results are emitted once per participant/event generation and include enough authority/provenance for progression.
- [ ] **F11-AC05** - Event props and traffic overrides are removed on unload without deleting shared city assets.
- [ ] **F11-AC06** - A selected original event can be inspected/loaded independently of UI and validates its dependencies.

## Edge cases to cover

Quoted/localized CSV values; alternate difficulty suffixes; checkpoint skipped in one tick; reset near finish; late join; ties; corrupt event reference.

## Suggested small implementation slices

### F11-A

Discover/parse the full original event catalog and its dependent data files.

### F11-B

Implement shared race lifecycle, swept triggers, participant progress and result identity.

### F11-C

Audit event coverage and negative trigger/restart tests; expose usable CLI/inspection entry points.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

Do not impose one checkpoint ordering rule on every mode. F12-F14 specify verified mode-specific behavior.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
