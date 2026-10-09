# F25 - Networked vehicle movement and collisions

## Outcome

Make vehicles driven by different players interact coherently over an imperfect network.

**Priority:** 6 (lower is earlier, subject to dependencies).
**Input contracts:** F24-B, F02-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Use production Avian vehicle simulation under host/server authority. Snapshot interpolation and optional bounded client prediction are architecture choices, not promises of bit-identical physics.

Source keys: R1, R4; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Transmit normalized inputs with bounded tick/sequence handling. Server simulates authoritative movement; remote clients interpolate snapshots with explicit spawn/despawn/reset generations.
2. Provide responsive local prediction/reconciliation where feasible, reusing vehicle simulation and controlling replayed side effects. Document the selected collision/prediction policy and known tradeoffs.
3. Replicate vehicle identity/paint, transform/velocities, wheel/engine state needed for presentation and relevant damage/reset state. Do not serialize the entire ECS or arbitrary pointers.
4. Handle player-player and player-world collisions without double-authority impulses or client self-teleporting. Bound correction magnitudes and recover from dropped/late snapshots.
5. Keep audio/effects deduplicated across predicted and authoritative events. Reset, direction changes and trailers must not leave stale replicas or extrapolate indefinitely.
6. Build a network impairment harness for delay, jitter, loss, duplication and reordering with deterministic test seeds. Document bandwidth/update-rate budgets and packet-size limits.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F25-AC01** - Two independently controlled clients drive visible distinct cars in the same city with one authoritative state source.
- [ ] **F25-AC02** - Player-player collision and reset converge to the server state without permanently diverged replicas.
- [ ] **F25-AC03** - A documented latency/jitter/loss matrix measures corrections and stale-state behavior instead of only testing perfect loopback.
- [ ] **F25-AC04** - Replay/reconciliation does not duplicate sounds, damage, checkpoints or spawned fragments.
- [ ] **F25-AC05** - Disconnect/despawn, respawn generations, reverse and representative trailer/extra-wheel presentation stay correct.
- [ ] **F25-AC06** - Headless soak shows bounded queues, memory and bandwidth with finite vehicle states.

## Edge cases to cover

Input arrives twice; snapshot from prior spawn; client falls behind; dynamic collision during correction; intermittent loss; late-loaded model; trailer reset.

## Suggested small implementation slices

### F25-A

Implement authoritative input transport and remote snapshot spawn/interpolation.

### F25-B

Add local responsiveness/reconciliation and explicit collision/reset/presentation policy.

### F25-C

Run impaired-network multi-process driving/collision/side-effect tests and report measured behavior.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No replacing actual remote cars with local bots or synchronizing only menu names.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
