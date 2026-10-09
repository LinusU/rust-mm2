# F18 - Weather, lighting presets, and time of day

## Outcome

Make session weather/time settings visible, audible and physically meaningful where the original rules require it.

**Priority:** 5 (lower is earlier, subject to dependencies).
**Input contracts:** F06-B, F01-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Confirm actual stock presets, combinations and effects from local help/data. Weather and time of day are independent session dimensions; continuous simulation is not assumed.

Source keys: R3, R4, R5; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Load/represent supported stock preset IDs and authored sky/lighting/fog/particle references with explicit mappings into modern Bevy rendering.
2. Implement precipitation and environmental appearance with bounded particles and correct camera-relative behavior. Rain/snow should not simply pass through covered interiors without a declared approximation.
3. Connect surface wetness/snow to F06 traction and F07/F08 audio/effects without duplicate multipliers. Keep cosmetic and gameplay preset parameters distinguishable.
4. Implement day/dusk/night lighting, appropriate vehicle/street illumination and exposure choices as required by verified presets. No ray-tracing-only dependency; macOS rasterized path remains supported.
5. Initialize presets deterministically from SessionConfig and replicate authoritative state/seed where relevant. Late joiners must not start with different physics weather.
6. Expose weather/time selection in CLI/menu and preserve settings/profile policy. Unsupported combinations must be rejected or documented rather than silently mapped to clear noon.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F18-AC01** - Every audited stock preset/allowed combination can be selected and visibly distinguishes the intended environment.
- [ ] **F18-AC02** - A controlled surface test demonstrates required traction changes and unchanged dry-state behavior.
- [ ] **F18-AC03** - Precipitation/effect counts remain bounded and unload on session exit.
- [ ] **F18-AC04** - Weather/time state survives restart and is consistent for server/late-joining clients after networking integration.
- [ ] **F18-AC05** - Screenshots and audio captures cover contrasting time/weather presets on the supported rendering path.
- [ ] **F18-AC06** - Unknown preset/assets produce explicit diagnostics and do not appear as successful default loading.

## Edge cases to cover

Covered roads/interiors; dry bridge over wet road; night headlights; frame-rate-independent particles; repeated preset changes; late join.

## Suggested small implementation slices

### F18-A

Inventory presets and implement lighting/sky/fog/time-of-day selection.

### F18-B

Add precipitation, surface/audio effects and shared session weather state.

### F18-C

Validate preset coverage, traction causality, bounded resources and representative visual/audio output.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No AI material generation, mandatory path tracing, or dynamic seasonal weather system.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
