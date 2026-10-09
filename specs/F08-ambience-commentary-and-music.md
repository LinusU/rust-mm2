# F08 - City ambience, commentary, UI audio, and music compatibility

## Outcome

Complete the soundscape beyond the player engine, with authentic content-driven events and honest legacy-music support.

**Priority:** 10 (lower is earlier, subject to dependencies).
**Input contracts:** F07-A, F01-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

MM2Hook exposes DirectMusic-related systems, but this does not establish that every music asset is a ordinary WAV or that a MIDI player covers the required behavior. Inventory actual data first.

Source keys: R3, R4; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Load spatial city ambience/pathsets, environmental zones, traffic/pedestrian reactions, commentary/announcer triggers and menu sounds through the VFS.
2. Implement dialogue priority, interruption, repeat suppression, seeded selection where appropriate, subtitles where practical, and session cleanup. Race-start speech must not replay every update.
3. Separate vehicle/effects, ambience, voice, UI and music volume controls with sensible ducking and bounded concurrency. Localize voice references where source data supports it.
4. Determine which music/event formats the installation uses. Implement required playback semantics or expose an explicit unsupported state with tracked work; a constant replacement loop is not original music compatibility.
5. Treat any optional external decoder library as an audited dependency with license/platform checks. Do not introduce a Windows-only runtime requirement into the native engine.
6. Record codec/asset-family coverage and capture evidence. A successful parse without audible event-driven output is not completion.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F08-AC01** - A known ambience source attenuates with distance and the correct scene/session controls its lifetime.
- [ ] **F08-AC02** - Race, collision/reaction and menu events trigger appropriate clips once under a deterministic test script.
- [ ] **F08-AC03** - Dialogue priorities prevent incoherent overlap; muting a bus affects only the intended category.
- [ ] **F08-AC04** - All expected audio families have supported/unsupported/missing counts and sample decode outcomes.
- [ ] **F08-AC05** - A sound capture distinguishes ambience, voice, UI and music behavior; any unimplemented legacy-music semantics remain explicitly incomplete.
- [ ] **F08-AC06** - Two session restarts do not leave old loops or queued dialogue active.

## Edge cases to cover

Overlapping zones; missing localized clip; long queued speech; rapid UI navigation; restart during dialogue; absent music renderer on a platform.

## Suggested small implementation slices

### F08-A

Inventory ambience/voice/UI/music content and implement missing decoders or format diagnostics.

### F08-B

Add spatial ambience, prioritized commentary/UI events and supported music scheduling.

### F08-C

Audit audio-family coverage and capture full session audio with correct cleanup/mixing.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No unlicensed replacement recordings, speech generation, or silent claim that DirectMusic is solved by generic audio output.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
