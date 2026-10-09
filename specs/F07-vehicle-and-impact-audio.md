# F07 - Engine, tire, brake, horn, and collision sound

## Outcome

Make driving sound right through original user-supplied samples and production simulation telemetry.

**Priority:** 4 (lower is earlier, subject to dependencies).
**Input contracts:** F02-B, F01-B, F06-A
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Choose one audio output/mixer integration compatible with the pinned Bevy stack after auditing existing dependencies. Do not require Kira merely because it was once discussed; avoid two competing audio engines.

Source keys: R3, R4; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Inventory original player/opponent audio definitions, engine sample banks, horn and impact formats. Decode actual formats including compression when required; extension recognition is not playback support.
2. Drive engine loop selection, pitch and crossfades from engine RPM/load/gear and documented mappings. Account for idle, acceleration, coasting, shifts and reverse. Do not tie pitch directly to throttle or world speed alone.
3. Drive tire squeal/skid/rolling sounds from grounded wheel slip/load and surface identity. A brake key pressed at rest must not squeal; an airborne spinning wheel must not play road contact audio.
4. Drive impacts and scrapes from deduplicated semantic collision events, surface pairs and severity. Bound voice count/cooldowns and distinguish a sustained scrape from repeated crashes.
5. Implement spatial attenuation/panning and appropriate listener behavior for chase/cockpit/free cameras. Avoid doubled local and replicated audio for the same event.
6. Add bus controls for vehicles, effects and overall audio; handle missing device, pause, focus settings, session unload and reinitialization. Headless tests must not require an audio device.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F07-AC01** - Valid original samples decode/play or produce specific unsupported-format errors; synthetic PCM fixtures verify the mixer path.
- [ ] **F07-AC02** - A scripted idle -> accelerate -> coast -> shift -> reverse sequence creates the expected loop/pitch/event changes.
- [ ] **F07-AC03** - Stationary braking and airborne wheel motion do not emit road squeal.
- [ ] **F07-AC04** - A sustained wall scrape does not create an unbounded stack of impact voices.
- [ ] **F07-AC05** - A recorded/offline mix or an actual listened-to capture demonstrates audible output; spawning sound entities alone is insufficient.
- [ ] **F07-AC06** - Restart/unload stops old engine loops; no-device execution degrades without breaking gameplay.

## Edge cases to cover

Loop boundaries/clicks; signed RPM/reverse; disabled vehicle; repeated contact ticks; device loss; muted output; cameras far from local car.

## Suggested small implementation slices

### F07-A

Import vehicle/impact audio definitions and implement bounded decoding/voice lifecycle.

### F07-B

Connect RPM/load, wheel contact/slip, horn and impact events to spatial playback.

### F07-C

Validate audible captures, no-device mode, loop cleanup, and negative sound-trigger scenarios.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No generated replacement engine sounds or AI audio service. City commentary/music is F08.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
