# F26 - Shared multiplayer world, Cruise, and races

## Outcome

Make multiplayer clients share traffic, relevant objects, race rules and outcomes rather than only vehicle transforms.

**Priority:** 7 (lower is earlier, subject to dependencies).
**Input contracts:** F25-B, F11-B, F04-B, F10-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Use explicit relevancy/authority; clients do not each independently spawn a different interacting traffic world from local RNG.

Source keys: R3, R4, R5; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Server owns traffic, gameplay-relevant props, damage, weather/time and race state. Replicate stable identities/state transitions; cosmetic effects can be local and nonauthoritative.
2. Use interest areas around all participants with hysteresis. Despawning an object for one client must not reset server damage/breakage or erase it for another player.
3. Synchronize countdown, pause policy, checkpoint/lap progress, finish ordering, timeouts and results for supported network Cruise/Blitz/Checkpoint/Circuit modes after verifying original mode/settings availability.
4. Late join or reconnect receives a consistent world snapshot and explicit race participation/spectator policy. Never start a new private race silently on one client.
5. Replicate weather/surface modifiers consistently and enforce gameplay mod fingerprints; no client-controlled award/unlock messages.
6. Provide lobby -> session -> result -> rematch -> lobby flow with clean teardown and bounded resynchronization.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F26-AC01** - Two clients see the same relevant traffic and the same broken/moved gameplay prop state.
- [ ] **F26-AC02** - A late joiner receives current world/session state rather than default noon/intact props.
- [ ] **F26-AC03** - Networked race progress/results agree across host and clients under impaired-network tests.
- [ ] **F26-AC04** - A client cannot award itself a checkpoint, lap, win, repair or score by submitting state.
- [ ] **F26-AC05** - Rematch and city/mode change remove stale entities/messages without disconnecting valid participants unnecessarily.
- [ ] **F26-AC06** - Coverage distinguishes each verified multiplayer mode and unsupported original variants.

## Edge cases to cover

Players far apart; broken prop leaves relevance then returns; finish packet lost; host pause; countdown disconnect; race ends during late join.

## Suggested small implementation slices

### F26-A

Replicate authoritative world actors, prop transitions and environment state with relevancy.

### F26-B

Implement shared Cruise/race lifecycle, progress/results and rematch/late-join policies.

### F26-C

Run multi-process world/race consistency and malicious-state/restart tests.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No per-client independent authoritative traffic or trust in a client-reported finish position.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
