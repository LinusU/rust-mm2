# F13 - Checkpoint races

## Outcome

Make original Checkpoint races playable with correct checkpoint eligibility, finishing and competition rules.

**Priority:** 4 (lower is earlier, subject to dependencies).
**Input contracts:** F11-B, F02-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Verify whether each relevant mode/configuration permits checkpoint freedom, enforces order, or has other conditions. Do not infer this from a generic racing template.

Source keys: R3, R4, R5; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Record original Checkpoint race rules, difficulty settings, recovery penalties, objective ordering and finish conditions in the evidence ledger.
2. Load each event's authored course, participants, start slots and environmental overrides through the shared runtime.
3. Track per-participant objective progress; distinguish location proximity from a valid crossing. Prevent repeated or wrong-level crossings from advancing state.
4. Provide checkpoint indicators, position/leaderboard semantics appropriate to the mode, finish/failure and result flow. For nonordered objectives, do not compute race rank from a nonexistent fixed sequence.
5. Integrate race opponents from F15 for the final single-player experience. Rule/unit testing without opponents is an intermediate slice, not complete racing.
6. Expose multiplayer-compatible results/state without clients deciding winners. Save profile progression only through verified authoritative results.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F13-AC01** - Every original Checkpoint event is discoverable and loads all required course/participant dependencies.
- [ ] **F13-AC02** - Tests cover the verified ordering/freedom rule, invalid crossings, missing objectives and finish eligibility.
- [ ] **F13-AC03** - Two participants can have independent progress without one player's trigger updating everyone.
- [ ] **F13-AC04** - With F15, opponents start, make legitimate progress and can finish; player results compare against actual participants.
- [ ] **F13-AC05** - Recovery/restart/quit do not bypass objectives or duplicate progression.
- [ ] **F13-AC06** - Representative original races in both cities are playable and the full catalog has an honest structural/behavioral coverage matrix.

## Edge cases to cover

Different route choices; near-simultaneous finish; wrong direction or level; opponent recovery; restart during countdown.

## Suggested small implementation slices

### F13-A

Verify and implement per-event Checkpoint objective/finish semantics.

### F13-B

Add participant progress, rank/result presentation and opponent integration hooks.

### F13-C

Validate the full catalog and playable races with actual opponents and progression deduplication.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

Do not call an empty time trial a completed competitive Checkpoint race.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
