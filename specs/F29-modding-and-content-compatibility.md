# F29 - End-to-end mod replacement and content compatibility

## Outcome

Keep the user's central requirement intact: replacing original content should be easy, consistent, safe and independent of game code.

**Priority:** 10 (lower is earlier, subject to dependencies).
**Input contracts:** F00-B, F02-A, F03-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

The existing VFS already supports source precedence and logical extension lookup. Extend consumers through it rather than introducing a second mod manager or bypassing it.

Source keys: R1, R3; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Audit every new feature for VFS use: models, textures, pivots, bounds, tuning, surfaces, placement, race rules, audio, UI and localization. No direct original-install opens in feature code.
2. Document source and same-source format precedence, duplicate IDs, unsupported formats and error behavior. Do not silently fallback when a selected override is malformed unless an explicit developer policy allows it.
3. Retain modern-format replacement boundaries; implement formats only when actually supported/tested. Do not claim GLB replacement works because its extension sorts ahead of PKG.
4. Use stable dependency/cache identity and invalidate dependent resources appropriately on restart/reload. Hot reload is optional; restart-based replacement must be reliable.
5. Classify gameplay versus cosmetic mod effects for multiplayer/records. Changes to bounds/tuning/race rules affect compatibility, unlike a base-color texture swap.
6. Keep original data read-only and reject traversal, unsafe symlinks, unbounded decompression, invalid counts and dependency cycles. Do not add native code plugins or automatic downloads.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F29-AC01** - A synthetic mod independently replaces a car texture, handling, a prop and an audio cue through production consumers.
- [ ] **F29-AC02** - Removing the mod restores base behavior; unrelated content is unchanged.
- [ ] **F29-AC03** - Conflicts have deterministic explainable provenance across app/inspector/server.
- [ ] **F29-AC04** - Malformed selected overrides and unsupported encodings report errors instead of fake success.
- [ ] **F29-AC05** - Gameplay fingerprints change for physics/race edits but not approved cosmetic-only replacements.
- [ ] **F29-AC06** - Negative parser/VFS tests cover malicious paths, oversized assets and reference cycles without crashes.

## Edge cases to cover

Same ID in two mods; extension preference versus source priority; palette/material variant cache; missing optional decoder; recursive model references.

## Suggested small implementation slices

### F29-A

Audit new content consumers and extend logical resource/override coverage.

### F29-B

Add end-to-end mod examples, compatibility classification and dependency/cache diagnostics.

### F29-C

Run override/revert/conflict/security tests across vehicle/world/audio/race consumers.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No AI enhancement pipeline, marketplace, cloud mod distribution, or arbitrary native plugin ABI.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
