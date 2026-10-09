# F21 - London cab and San Francisco stunt Crash Courses

## Outcome

Implement both original Crash Course tracks with their authored lessons, exams, conditions, feedback and rewards.

**Priority:** 9 (lower is earlier, subject to dependencies).
**Input contracts:** F11-B, F16-B, F02-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Do not treat these as generic Blitz races. Audit every lesson/exam and its source/configuration, including any rules embedded in original logic rather than obvious data files.

Source keys: R3, R4, R5; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Create an independently audited course/lesson catalog for both cities with prerequisites, start conditions, vehicles, environment props, objectives, limits, feedback and rewards.
2. Build small explicit objective evaluators for evidenced lesson rules: gates/zones, timing, maneuvers, sequences, required states and failure conditions as actually encountered. No universal timer-only approximation.
3. Allow data-driven composition where appropriate but keep hardcoded recovered rules small, named, sourced and testable. Do not invent original file layouts to force everything into a fake parser.
4. Integrate instruction/voice/subtitle, progress, pass/fail, retry and next-lesson flow with the shared session/menu/profile systems.
5. Restore course-specific objects/traffic constraints on retry and cleanly leave the course. Prevent normal-race results from accidentally satisfying lesson rewards.
6. Track every lesson/exam individually; structural catalog success is different from executing its distinctive pass/fail mechanics.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F21-AC01** - Both expected course catalogs enumerate all lessons/exams and known dependencies without filtering unsupported ones.
- [ ] **F21-AC02** - Each lesson has positive and negative tests for its distinctive rule evaluator.
- [ ] **F21-AC03** - Representative lessons and each exam can actually be played through with correct feedback and retry.
- [ ] **F21-AC04** - Passing unlocks the correct next step/reward once; failure/quit/duplicate results do not.
- [ ] **F21-AC05** - Retry restores required world/vehicle state without duplicated objects or stale objective counters.
- [ ] **F21-AC06** - Course coverage identifies implemented, original-verified, synthetic-only and unresolved lessons separately.

## Edge cases to cover

Lesson-specific vehicle switch; exam sequence; time limit boundary; resetting mid-maneuver; missing commentary; prerequisite mismatch.

## Suggested small implementation slices

### F21-A

Audit both course catalogs and formalize each evidenced lesson/exam rule.

### F21-B

Implement objective evaluators and playable instruction/pass/fail/retry flow.

### F21-C

Validate every lesson rule and both course progressions/rewards with coverage evidence.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No made-up replacement lessons presented as original Crash Course compatibility.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
