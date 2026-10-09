# Source map and provenance

Prepared September 19, 2026. Sources are research entry points; code presence and docs are not proof of runtime compatibility. URLs here are intentionally usable by the implementation agent. Record newer commit/observed versions during research.

## Repository baseline

**R1** - User repository, inspected through the connected GitHub tool at commit `822511c1b29cd31f5746aa8dc0607bd24c69b422`.

- https://github.com/LinusU/rust-mm2/tree/822511c1b29cd31f5746aa8dc0607bd24c69b422
- https://github.com/LinusU/rust-mm2/blob/822511c1b29cd31f5746aa8dc0607bd24c69b422/crates/mm2_app/src/main.rs
- https://github.com/LinusU/rust-mm2/blob/822511c1b29cd31f5746aa8dc0607bd24c69b422/crates/mm2_content/src/lib.rs

**R2** - README at the same commit. Read as implementation claims that may lag code, not independently run acceptance results.

https://github.com/LinusU/rust-mm2/blob/822511c1b29cd31f5746aa8dc0607bd24c69b422/README.md

## Original content and behavior research

**R3** - Community reverse-engineering format documentation (primary project documentation). Verify uncertain fields with actual user-supplied files.

- https://github.com/Dummiesman/angel-file-formats
- https://github.com/Dummiesman/angel-file-formats/blob/master/Midtown%20Madness%202/Format_Reference.md
- https://github.com/Dummiesman/angel-file-formats/blob/master/Midtown%20Madness%202/File_tree.md
- https://github.com/Dummiesman/angel-file-formats/blob/master/Midtown%20Madness%202/BAI.md
- https://github.com/Dummiesman/angel-file-formats/blob/master/Midtown%20Madness%202/PKG_Groups.md

**R4** - MM2Hook recovered structure and instrumentation research. Some functions remain calls into the original binary; hooks also add fixes/features. Check licensing before code reuse and distinguish stock behavior from extensions.

- https://github.com/Dummiesman/mm2hook
- https://github.com/Dummiesman/mm2hook/tree/master/src/modules/vehicle
- https://github.com/Dummiesman/mm2hook/blob/master/src/modules/mmnetwork/network.h

**R5** - Authoritative project-specific behavior evidence to collect from the user's installed original help/manual/readme, original event/tuning files and a reference playthrough. This pack does not contain or claim to have inspected that private installation. Do not assume exact race/Cops & Robbers/Crash Course rules from this source label. Public manual mirrors attempted during preparation were unavailable; they are not treated as independently verified evidence here.

## Ralph and Devin

**R6** - Geoffrey Huntley's description of the Ralph technique. Used for the fresh-context, repository-persisted, one-task-at-a-time approach; not as a promise of outcomes.

https://ghuntley.com/ralph/

**R7** - Devin CLI command reference, checked during preparation. Used for model discovery and the noninteractive prompt/export invocation; verify the installed CLI before use.

https://docs.devin.ai/cli/reference/commands

**R8** - Devin permission behavior, including direct file writes in sandbox/autonomous mode.

https://docs.devin.ai/cli/reference/permissions

**R9** - Devin sandbox documentation. The launcher is not itself an OS sandbox.

https://docs.devin.ai/cli/sandbox

## Source-use discipline

The feature specifications are proposed engineering requirements written for this project, not copied source text or claims that every rule is already known. No original game assets, screenshots, audio, binaries, font files or upstream implementation code are bundled. Small synthetic test data used for launcher tests is authored in this package.
