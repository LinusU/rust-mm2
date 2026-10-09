# F00 - Baseline, original-content audit, and evidence harness

## Outcome

Establish what actually works at the current commit and what the installed original game requires, before changing another subsystem.

**Priority:** 0 (lower is earlier, subject to dependencies).
**Input contracts:** None; begin here.
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

The reviewed commit is 822511c1b29cd31f5746aa8dc0607bd24c69b422. Its CLI and mm2_content already contain vehicle loading; the README is not a complete inventory. Newer work may exist. Do not reset to the reviewed commit.

Source keys: R1, R2, R3; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Inspect git status, branch, workspace manifests, tests, installed toolchain, current CLI help, and relevant code. Preserve local/newer work. Record the starting commit and enabled Bevy/Avian versions without upgrading them.
2. Locate user-supplied game data through MM2_GAME_DIR or documented local configuration. Keep it read-only and outside public fixtures. Enumerate archives and dependency families through the existing VFS, not an alternative extraction-only path.
3. Build an expected-content inventory independently of successful imports: cities, player vehicles and paints, race/event catalogs, lessons, placement sources, audio families, pedestrian archetypes, and multiplayer rule variants. Preserve rejected entries with reasons.
4. Create an original-rules ledger from local help/manual, authored data, and observable original behavior. Mark each fact verified_original, documented, inferred, designed, or unknown. Never import assumed rules from MM1, MM3, or MM2Hook additions.
5. Run baseline build/lint/tests and a dev-world smoke. Add reusable headless and screenshot/report entry points as needed. Missing data, unavailable GPU/audio, unsupported records, and actual failures must have different statuses.
6. Keep coverage reports versioned by engine commit and installation/content fingerprint. Reuse cached immutable inputs; do not rescan/decompress every archive every physics tick.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F00-AC01** - From the actual current checkout, documented fmt, Clippy, and workspace tests run; failures and prerequisites are recorded, not suppressed.
- [ ] **F00-AC02** - Inventory reports contain expected, discovered, accepted, rejected, and unverified counts; an empty catalog never passes strict validation.
- [ ] **F00-AC03** - A deliberately malformed synthetic asset is reported with logical path, selected source, and parse context without crashing.
- [ ] **F00-AC04** - The dev world starts without original data; a specifically requested missing original city/vehicle exits or enters an explicit failure state.
- [ ] **F00-AC05** - At least one visual smoke and one headless physics smoke are independently distinguishable in the report.
- [ ] **F00-AC06** - Unverified original rules remain visible and are not silently converted into completed compatibility claims.

## Edge cases to cover

Missing archives; localized strings; original/mod conflicts; stale README claims; differing patch/content versions; tests skipped because a GPU or dataset is absent.

## Suggested small implementation slices

### F00-A

Audit the live checkout, environment, and existing tests; write a factual baseline report.

### F00-B

Create the content inventory and original-rules ledger using existing VFS/inspection tools.

### F00-C

Establish reusable synthetic, original-data, and graphical evidence commands with explicit missing-capability outcomes.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

Do not rewrite completed loaders, download game files, invent an exact stock count, or implement unrelated features during the audit.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
