# F31 - Final original-feature coverage and release acceptance

## Outcome

Determine whether this is a complete enhanced recreation using evidence, not a folder full of parsers and menu labels.

**Priority:** 20 (lower is earlier, subject to dependencies).
**Input contracts:** F00-C
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

This is the final aggregate audit. It depends on feature evidence from F01-F30 for completion, but can produce a gap report at any point. Improved handling/rendering/network protocol are deliberate differences, not hidden gaps.

Source keys: R1, R3, R4, R5; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Maintain a complete feature/content matrix against original help/manual/data and reference playthroughs: vehicles/paints, both cities, objects, audio, traffic/pedestrians/police, modes/courses, HUD/options, progression and multiplayer variants.
2. Require full expected denominators and never define completeness as only the assets/events the current importer accepts. Record unsupported, absent, untested and intentionally changed behavior separately.
3. Execute full player journeys: profile -> select -> load -> drive/race -> outcome/reward -> menu -> restart; multi-client lobby -> mode -> score/result -> rematch -> disconnect.
4. Test negative/abnormal paths: corrupt/missing data, mod conflict, invalid profile, pause/focus loss, reconnect, packet loss, checkpoint exploit, duplicate result, and resource cleanup.
5. Maintain deliberate-differences and unresolved-original-rules documents. Exact original executable/save/network compatibility is not implied by complete feature scope.
6. Produce a release handoff with checked commit/artifacts, runnable commands, coverage/evidence locations, performance/platform matrix and outstanding blockers. Never issue a blanket complete claim from build success.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F31-AC01** - Every feature in the audited original scope maps to an implemented path and verified evidence or an explicit remaining gap.
- [ ] **F31-AC02** - All stock vehicle/event/course/placement/audio catalogs have honest expected-versus-tested counts.
- [ ] **F31-AC03** - Single-player and multiplayer end-to-end flows pass using the same production systems as normal gameplay.
- [ ] **F31-AC04** - No placeholder panels, bots pretending to be clients, silent generic fallback cars or unknown-format defaults count as completed features.
- [ ] **F31-AC05** - All required code gates, platform-specific checks and selected original-content playtests are recorded at a reproducible commit.
- [ ] **F31-AC06** - The final report distinguishes code-checked, original-data-verified, manually-playtested and intentionally changed behavior.

## Edge cases to cover

Feature renamed to evade coverage; skipped tests counted as passes; stale captures; partial stock installation; mods mask missing assets; last-minute regression.

## Suggested small implementation slices

### F31-A

Reconcile the original-feature matrix and current evidence to produce a prioritized gap audit.

### F31-B

Close remaining cross-feature integration gaps and execute complete user journeys.

### F31-C

Run final content/platform/network acceptance and produce an honest release/handoff report.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

Do not publish, push to main, bundle original assets, or declare full completion while required evidence remains unverified.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
