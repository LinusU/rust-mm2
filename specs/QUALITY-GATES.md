# Verification and acceptance contract

## Four different kinds of evidence

| Level | What it demonstrates | What it does not demonstrate |
|---|---|---|
| Code checks | Formatting, linting, compilation and automated tests passed at a specific state. | That original content loaded, a sound was audible, or gameplay felt right. |
| Synthetic integration | Controlled fixtures exercised the actual parser/importer/simulation/rule path. | Every original variant or artifact is supported. |
| Original-content validation | A named installation fingerprint and expected catalog were actually exercised. | Every scene was visually inspected or every event played by a person. |
| Rendered/audio/playtest evidence | A specific scene, capture or interactive scenario was observed. | Universal content/platform/Internet support. |

Missing GPU/data/device/network access is not a pass. It may leave code useful and externally checked while verification remains open. A feature is complete only when its applicable spec acceptance and original-coverage requirements are satisfied.

## Baseline external code gates

Run from the actual repository, using the committed lockfile:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace
```

CI (`.github/workflows/ci.yml`) runs these on every pushed task branch, and Rally lands a change only when CI is green on that exact commit. The workflow is owner-controlled configuration, not an agent-editable way to waive failures. If the real project's supported feature combinations differ, document and deliberately approve a corrected gate rather than suppressing a failure inside a task.

Use targeted tests while implementing, then full gates at a checkpoint. Keep one build pipeline per target directory. Agent exit code zero is insufficient. Do not remove tests, narrow the expected stock roster, add broad ignores or make a checker skip unsupported content to manufacture success.

## Feature acceptance

Each feature has AC IDs, observable scenarios and edge cases. For a slice, record which ACs it advances and which remain open. For final feature completion, all applicable ACs need evidence, including negative cases.

Examples of required negative tests:

- parser truncation, invalid counts/indices, traversal, reference cycles and decompression limits;
- engine/audio state at rest and airborne, repeated collisions, reset and resource cleanup;
- high-speed/wrong-height/repeated checkpoint crossings, teleport/reset, time-boundary finishes and duplicated results;
- profile isolation, interrupted save, invalid settings and focus/device loss;
- duplicate/out-of-order/stale network messages, contested gold pickup, carrier disconnect and malformed clients.

## Expected denominators

Inventory before filtering by parser success. Store expected, found, attempted, passed, failed, unsupported and unverified counts. A strict command must fail on an empty required catalog or missing required original data. New original cases must not disappear because a filename matcher ignores them.

Manual or original-data tests may be opt-in for public CI; opt-in commands must report unavailable prerequisites explicitly. Do not hide a skip as success in aggregate reports.

## Repeatable runtime evidence

Capture engine commit, content fingerprint, mods, platform/GPU/audio capabilities, settings, seed, selected vehicle/event, command, duration/ticks and result. Keep large/proprietary captures outside public git. Screenshots must await actual capture completion, not a magic delay and hopeful exit.

Network acceptance requires separate host/client processes and an impairment matrix, with loopback/LAN/Internet scope stated. Bots, screenshots of two cars, or a lobby roster alone are not multiplayer proof.

## Independent review

After code gates, use a fresh review session to inspect the actual diff, relevant spec and evidence. Reviewer searches for silent fallbacks, bypassed checks, unsupported completion claims, duplicate loaders and lifecycle/authority errors. The reviewer reports without editing implementation code.

Rally lands a change after green CI and a passing review. That does not prove a feature meets all original rules, enforce a security sandbox, automatically merge branches, or guarantee the reviewer is correct. Feature-level completion remains evidence-driven and operator-auditable.
