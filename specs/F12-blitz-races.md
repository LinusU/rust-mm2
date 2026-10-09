# F12 - Blitz races

## Outcome

Make every original Blitz event playable with verified timer, checkpoint and finish rules.

**Priority:** 4 (lower is earlier, subject to dependencies).
**Input contracts:** F11-B, F02-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

The mode name is not a complete rule specification. Confirm countdown, checkpoint order/freedom, timing, difficulty, penalties and completion thresholds from original help/data/behavior.

Source keys: R3, R4, R5; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Write the Blitz rule ledger first and connect each rule to original evidence. Unknown rules may support an explicitly provisional implementation but cannot satisfy full fidelity acceptance.
2. Load authored start/checkpoints/finish/time and environmental overrides for each event; no generated replacement layouts.
3. Implement visible timer, remaining objectives, navigation feedback, warning cues, success/timeout and restart through the shared race runtime.
4. Handle checkpoint acceptance and finishing exactly according to the verified mode rules, including simultaneous timer/finish boundaries and resets.
5. Emit authoritative idempotent results and applicable reward facts. CLI developer unlocks must not silently award legitimate progression.
6. Run bounded scripted/bot-assisted completions/failures in the production trigger and rule systems. A route overlay without a functioning objective is not playable.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F12-AC01** - Every expected Blitz event loads its own authored objectives and settings.
- [ ] **F12-AC02** - A valid sequence finishes; missing/repeated/invalid objectives cannot manufacture success.
- [ ] **F12-AC03** - Timeout and finish-at-boundary behavior are deterministic with a documented rule.
- [ ] **F12-AC04** - Timer/countdown/UI/audio all use the same authoritative race clock.
- [ ] **F12-AC05** - Restart and quit do not preserve prior checkpoint flags or grant duplicate rewards.
- [ ] **F12-AC06** - At least representative original events in both cities are played/rendered, with the complete catalog structurally tested and unplayed entries labeled.

## Edge cases to cover

High-speed crossing; checkpoint overlap; time expires on finish tick; pause; recovery/teleport; difficulty-specific limits.

## Suggested small implementation slices

### F12-A

Verify Blitz rules and bind event-specific data to the shared runtime.

### F12-B

Implement timer/objectives/finish/failure with HUD/navigation feedback.

### F12-C

Run complete-catalog structural checks and valid/invalid playable Blitz scenarios.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No assumption that Blitz needs the same opponents or ordering as Circuit; use verified evidence.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
