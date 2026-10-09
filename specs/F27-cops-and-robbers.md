# F27 - Multiplayer Cops & Robbers

## Outcome

Implement the recognizable original gold/robbery competition and every audited stock variant, not merely police-car chasing.

**Priority:** 7 (lower is earlier, subject to dependencies).
**Input contracts:** F25-B, F11-A, F01-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Verify pickup, carrier, transfer/steal/drop, delivery/base, teams, scoring, gold weight, match ending and reset rules from local original help and behavior. Names/enums in hooks alone do not prove full rules.

Source keys: R3, R4, R5; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. First build a rule matrix per original variant/settings combination, with evidence and unresolved entries. Distinguish the cops/robbers/team competition from single-player police AI.
2. Load original mode spawn/base/gold locations and permitted settings from content where present. Any recovered code-defined rule must be sourced and isolated, not masquerade as a parsed file.
3. Implement an authoritative gold-object state machine with one valid ownership state, stable object ID and match generation. Handle contested pickups/transfers deterministically.
4. Apply verified carrier penalties/weight or handling changes once and undo them exactly on transfer, delivery, reset or disconnect. Avoid mutating the immutable vehicle definition permanently.
5. Implement team selection, role assignment, gold indicators, carrier/score HUD, delivery feedback, victory/end/rematch and relevant audio. Clients display state; they cannot declare a steal or score.
6. Support impaired-network, late-join/disconnect and recovery edge cases through explicit rules. If a rule is unknown, mark that variant provisional and keep its compatibility check incomplete.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F27-AC01** - A host and at least two separate clients complete a verified pickup -> carry -> contested transfer/steal -> delivery -> score cycle.
- [ ] **F27-AC02** - Two simultaneous pickup requests produce exactly one carrier, never duplicated gold or double score.
- [ ] **F27-AC03** - Carrier disconnect/destruction/recovery and gold out-of-bounds produce the verified outcome without losing the objective permanently.
- [ ] **F27-AC04** - Gold handling modifiers are applied/removed exactly once and do not leak into a later race.
- [ ] **F27-AC05** - Teams/scores/winner agree across clients under packet loss/reordering and late join.
- [ ] **F27-AC06** - Every audited stock variant has its own acceptance status; a single chase sandbox cannot satisfy mode completion.

## Edge cases to cover

Gold pickup and delivery same tick; two cars collide over gold; disconnecting carrier; stale transfer message; simultaneous winning scores; round restart.

## Suggested small implementation slices

### F27-A

Verify the full Cops & Robbers rule/settings matrix and original placement dependencies.

### F27-B

Implement authoritative gold/teams/scoring/handling state plus playable HUD/lobby flow.

### F27-C

Run multi-client full rounds, contested events, impairment and all verified variant tests.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No invented capture-the-flag/pursuit rules presented as MM2. No bots substituted for remote clients in completion evidence.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
