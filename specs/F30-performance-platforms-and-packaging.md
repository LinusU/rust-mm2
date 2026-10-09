# F30 - Performance, native platforms, and reproducible builds

## Outcome

Keep the expanded game usable and measurable on the intended native platforms, especially the user's Apple Silicon development machine.

**Priority:** 12 (lower is earlier, subject to dependencies).
**Input contracts:** F01-C, F02-B, F03-B, F10-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

Do not replace the portable rasterized renderer with a mandatory GPU-specific path. Pin working dependency/toolchain versions and verify actual available platform capabilities.

Source keys: R1; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Profile city loading, frame time, physics time, allocations, draw calls, GPU memory, audio voices, actor counts and network bandwidth under representative scenes. Measure before optimizing.
2. Apply spatial culling/LOD, resource sharing, bounded streaming/interest updates and physics sleeping while preserving gameplay state. Do not hide missing actors behind aggressive culling to improve numbers.
3. Retain fixed-step simulation and visually coherent interpolation; report overload behavior instead of silently dropping arbitrary gameplay time.
4. Provide reproducible build/package instructions for macOS, Windows and Linux, native dependencies, application asset discovery, user-save paths and CLI/server entry points.
5. Add CI checks/tests appropriate to each platform; keep original-data/GPU/manual testing explicitly separate from hosted CI success.
6. Build clean artifacts without copyrighted original content, private paths, credentials or large local captures. No automatic releases/pushes without operator approval.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F30-AC01** - A reproducible benchmark report records hardware, commit, scene/content fingerprint, settings and percentile frame timings.
- [ ] **F30-AC02** - A populated driving/race/network soak stays within declared budgets and shows no unbounded entity/voice/queue growth.
- [ ] **F30-AC03** - A clean checkout builds/tests with documented dependencies; platform claims are backed by actual target tests or labeled untested.
- [ ] **F30-AC04** - Packaged app locates its own synthetic assets regardless of current working directory and accepts an external original installation.
- [ ] **F30-AC05** - Save/config/log/cache paths are writable user locations, not original-content or installed-binary directories.
- [ ] **F30-AC06** - Release candidates preserve scene correctness and game-rule tests after optimization.

## Edge cases to cover

Low GPU capability; no audio device; huge mod texture; slow storage; minimized client; fixed-step overload; non-ASCII installation path.

## Suggested small implementation slices

### F30-A

Create reproducible performance/soak measurements and identify actual bottlenecks.

### F30-B

Implement targeted optimizations and clean native packaging/runtime asset paths.

### F30-C

Validate platform matrix, populated-session stability and distributable artifact contents.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No promised FPS on untested hardware, compulsory path tracing, or premature engine migration.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
