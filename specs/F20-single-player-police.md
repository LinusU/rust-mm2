# F20 - Single-player police pursuit and response

## Outcome

Add police as a distinct single-player system, not a synonym for ambient traffic or multiplayer Cops & Robbers.

**Priority:** 9 (lower is earlier, subject to dependencies).
**Input contracts:** F10-B, F05-B, F01-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Verify offense/detection, pursuit escalation, loss of pursuit and applicable penalties from original behavior. Preserve uncertainty rather than inventing a modern wanted-level system.

Source keys: R3, R4, R5; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Import/discover police vehicle content, session density and applicable event restrictions. Reuse navigation/vehicle assembly and siren/light/audio hooks.
2. Implement an explicit detect -> engage -> pursue/intercept -> lost/disengaged/outcome state machine driven by verified rules and bounded observations.
3. Use road-aware pursuit and local avoidance with recovery and limits on active pursuers. Avoid perfect omniscient teleporting police unless a specific original spawn policy is documented and intentionally adapted.
4. Define collision, stopping/busting/outcome semantics with the game mode. Distinguish police gameplay from arbitrary collision damage thresholds.
5. Pause, restart, change city/event and disable police cleanly. Surface/settings/AI authority should be shared with the session, not player-local UI.
6. Provide debug state/target/route visibility and deterministic scenarios for pursuit start, chase, loss, outcome and disabled contexts.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F20-AC01** - Verified triggering behavior starts pursuit and a nontriggering control scenario does not.
- [ ] **F20-AC02** - Police use original vehicles with functioning siren/light states and coherent road pursuit.
- [ ] **F20-AC03** - Losing pursuit or completing an applicable outcome terminates the state according to the rules ledger.
- [ ] **F20-AC04** - Active pursuer count and stuck recovery remain bounded during a long chase.
- [ ] **F20-AC05** - Police density/event restrictions and restart/unload are honored.
- [ ] **F20-AC06** - The report distinguishes observed original semantics from enhanced AI decisions and unresolved rules.

## Edge cases to cover

Player changes level at bridge; pursuit during race finish; disabled police event; overturned pursuer; respawn while chased; multiple offenses one tick.

## Suggested small implementation slices

### F20-A

Verify police rules and implement detection/pursuit state plus content selection.

### F20-B

Add road-aware chase/interception, sirens/lights and bounded disengagement/recovery.

### F20-C

Test trigger/nontrigger/chase/loss/outcome scenarios and session restrictions.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No assumed GTA-style stars, arrest animations, or substitution for the gold-based multiplayer mode.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
