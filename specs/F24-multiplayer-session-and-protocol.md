# F24 - Multiplayer session, protocol, lobby, and authority

## Outcome

Establish host/join and stable authority before adding gameplay replication; modern engine-to-engine multiplayer, not original DirectPlay interoperation.

**Priority:** 6 (lower is earlier, subject to dependencies).
**Input contracts:** F01-B, F02-A
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Do not inherit a networking crate recommendation without verifying current compatibility with the pinned stack. Choose and document one maintained transport; reuse cryptographic/session primitives instead of inventing them.

Source keys: R1, R4; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Specify server/host-authoritative simulation, input tick/sequence, stable network entity IDs, session generation, protocol/content versioning and transport channel needs. Never assume deterministic lockstep across platforms.
2. Implement headless listen/dedicated host and direct host/join client paths plus lobby roster, readiness, city/mode/car/paint/settings validation, start/cancel and disconnect.
3. Separate gameplay fingerprints (tuning, collision, event definitions, relevant mods) from cosmetic texture/audio differences. No automatic transfer of proprietary original assets.
4. Validate identity/authority and bound message sizes, queues, rates and deserialization. Clients may request actions but cannot set authoritative positions, score, damage or unlock state.
5. Keep LAN/direct-address support distinct from Internet reachability. Explicitly define NAT/firewall/relay strategy and costs; do not silently buy services or open public listeners by default.
6. Replicate essential session configuration, seed, clocks and late-join policy. Host migration is optional unless explicitly required; a clean host-loss return is mandatory.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F24-AC01** - Separate host and two client processes establish a lobby, choose compatible cars/settings, ready and start.
- [ ] **F24-AC02** - Mismatched protocol/gameplay content is rejected clearly; allowed cosmetic differences do not falsely block joining.
- [ ] **F24-AC03** - Malformed/oversized/out-of-order/rate-excess messages are bounded and cannot crash or unboundedly allocate the server.
- [ ] **F24-AC04** - Disconnect, failed join and host loss release resources and produce usable UI/CLI errors.
- [ ] **F24-AC05** - A headless server runs without window/audio requirements and binds only the explicitly configured interface.
- [ ] **F24-AC06** - Reports distinguish same-machine, LAN and Internet tests; no fake claim of Internet support from loopback success.

## Edge cases to cover

Duplicate player IDs; reconnect; wrong port; version mismatch; incompatible gameplay mods; host leaves; auth failure; noisy client.

## Suggested small implementation slices

### F24-A

Choose/verify transport and implement protocol IDs, authority boundaries and content fingerprints.

### F24-B

Implement host/join/lobby/readiness/start/disconnect with headless hosting.

### F24-C

Run multi-process handshake, compatibility, malformed-input and host-loss tests.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No original DirectPlay clients, automatic public matchmaking service, or client-authoritative scoring.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
