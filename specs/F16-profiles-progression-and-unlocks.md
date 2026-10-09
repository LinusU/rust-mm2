# F16 - Profiles, persistent progress, vehicle and paint unlocks

## Outcome

Persist real player identity, settings/results and original progression rules safely across game restarts.

**Priority:** 5 (lower is earlier, subject to dependencies).
**Input contracts:** F01-A, F11-A
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Import unlock/reward tables and distinguish developer unrestricted access from normal-profile progression. Vehicle and paint unlocks are separate coverage items.

Source keys: R3, R4, R5; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Provide stable profile IDs, names, create/select/delete flows, per-profile progress and settings. Use an OS-appropriate user data directory; never write saves into original game content.
2. Version the save schema, validate loaded data, write atomically and keep a recoverable previous version. Interrupted writes or an invalid profile must not silently erase all progress.
3. Apply authored race/Crash Course rewards and difficulty criteria from verified evidence. Unlocks must be driven by idempotent authoritative result IDs, not UI navigation or race launch.
4. Track completed events, relevant best records and unlock conditions. Preserve unknown future fields or migrate explicitly; do not make content-array positions permanent save IDs.
5. Separate a dev/sandbox profile and cheats/forced unlocks from legitimate progression. Define how modded gameplay content affects record/unlock eligibility.
6. Read saved preferences before starting a session and avoid one profile leaking selected car/results into another. Do not imply compatibility with original save files unless implemented and tested.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F16-AC01** - Create two profiles, complete an eligible event in one, restart the application and observe isolated persisted progress.
- [ ] **F16-AC02** - A successful eligible result unlocks the expected vehicle/paint once; duplicate result delivery does not re-award it.
- [ ] **F16-AC03** - Aborted, failed, developer-unlocked or invalid events do not accidentally grant normal-profile rewards.
- [ ] **F16-AC04** - Corrupt/truncated saves and interrupted writes produce a recoverable error without destroying valid backup data.
- [ ] **F16-AC05** - Reward coverage enumerates every expected authored unlock rule, including Crash Course links.
- [ ] **F16-AC06** - Deleting a profile requires deliberate UI confirmation and does not delete other profiles or original data.

## Edge cases to cover

Duplicate display names; Unicode; missing mod content; schema upgrades; result received twice; quit during save; developer profile mistaken for normal.

## Suggested small implementation slices

### F16-A

Implement versioned profile storage and atomic load/save with isolated identities.

### F16-B

Import reward/unlock rules and apply idempotent authoritative results.

### F16-C

Validate restart, multiple-profile isolation, full unlock-rule coverage and corrupt-save recovery.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No cloud accounts, Steam achievements, or claim of original-save compatibility without a dedicated importer.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
