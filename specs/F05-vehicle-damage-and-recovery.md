# F05 - Vehicle damage, breakaway parts, and recovery

## Outcome

Restore original-content vehicle damage/recovery features while preserving the intentional modern handling policy.

**Priority:** 5 (lower is earlier, subject to dependencies).
**Input contracts:** F02-B, F01-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Research vehCarDamage and visual part roles. Derive health/damage rules from data and observed behavior; not every impact should subtract the same constant.

Source keys: R3, R4; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Load relevant damage thresholds and part associations through the VFS. Separate mechanical/functional damage, cosmetic damage and immutable source metadata.
2. Convert meaningful impact severity into damage using a documented model. Stable contact resting, tiny wheel contact impulses and normal suspension loads must not repeatedly damage the car.
3. Show authored intact/damaged/breakaway parts correctly, emit smoke/sparks and sound events where applicable, and prevent detached visual pieces from remaining duplicated on the intact rig.
4. Apply documented impairment/disabled-state rules; expose compatibility facts separately from enhanced policies. Race modes decide whether recovery/respawn incurs a penalty or is disallowed.
5. Make reset, stuck recovery, rollover recovery, water/out-of-bounds and respawn safe for all vehicle sizes and trailers. Do not award race checkpoints through teleporting.
6. Damage and recovery are authority-owned state, with bounded events and predictable replication; clients never authoritatively grant themselves repairs or invulnerability.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F05-AC01** - A stationary car and normal curb/suspension contact do not accumulate unexplained damage.
- [ ] **F05-AC02** - Increasing controlled impact severity produces coherent damage and visual-state changes.
- [ ] **F05-AC03** - A detached part appears once, cleanup is bounded, and reset restores the original rig.
- [ ] **F05-AC04** - Disabled/repair/recovery behavior matches the explicit session policy and does not bypass race rules.
- [ ] **F05-AC05** - Water, rollover, stuck and out-of-bounds recovery are tested with small, heavy and articulated vehicles.
- [ ] **F05-AC06** - Duplicate impact/reset events do not double-apply damage or create extra vehicles.

## Edge cases to cover

Glancing scrape versus head-on hit; very heavy vehicle; destroyed trailer; damage during finish; reset crossing a checkpoint; missing damaged geometry.

## Suggested small implementation slices

### F05-A

Define damage/recovery rules and parse the required authored parameters/part mappings.

### F05-B

Implement impact-driven state, visual/effect hooks, impairment, and safe recovery.

### F05-C

Validate severity, reset, mode penalties, rig restoration, and abnormal-location cases.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No soft-body simulation requirement or unsupported promise of exact original crash trajectories.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
