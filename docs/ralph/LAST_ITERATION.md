# Last implementation iteration

- Task ID and title: F16-C.1 — F16-C evidence legs: AC01 two-profile
  restart isolation and AC04's interrupted-save recovery through the
  app bind path, plus the two non-blocking review repairs and an AC05
  coverage-accounting fix in `mm2-inspect events`. Selected as the
  plan's first-listed next slice (F16-C evidence run); no external
  review blockers were outstanding.
- Starting commit: `6d4b90baff7645d6f9596770bf21499efb76fe34` on
  `ralph/night`; tree was clean.

## What changed

- `crates/mm2_app/tests/progression.rs` (AC01):
  `two_profiles_isolate_progress_across_a_restart` — A and B share one
  synthetic install. A binds, drives the authored checkpoint course to
  a *real* finish through `advance_race` → `record_session_results`
  (the 900-frame throttle loop is now the shared `drive_to_finish`
  helper), earns both authored grants. The app and store handle drop;
  a reopened `ProfileStore` on the same directory is the restart. A's
  record + 2 unlocks + persisted selections survive; B binds fresh
  with zero progress/unlocks and default selections (spec req 6 — no
  leak of A's remembered `vpt`), and A's earned `vpreward` still
  reports `VehicleGateNote::Locked` for B through the production
  garage surface while reporting open for A. B then drives the same
  event to its own finish — record and grants land on B alone; A's
  progress and selections are untouched.
- `crates/mm2_app/tests/profile.rs` (AC04):
  `an_interrupted_save_recovers_through_the_bind` — the `.tmp`-orphan
  leg the `.bak` test didn't cover: a flushed-but-never-renamed
  revision-3 `.tmp` (crash between flush and rename) beats the stale
  revision-2 main, `resolve` reports `recovered_from_backup`, the
  bind-time heal re-saves it (revision 4), and a fresh load is clean.
- Review repairs (non-blocking findings from the F16-B.3 review):
  `Unlock::Paint`'s doc comment no longer claims the variant index
  base is unverified — it records the measured zero-based reading
  (DSN-16); VEH-5's nonzero-`UnlockFlags` list gains the omitted
  `vpeagle` (unlisted, `0/1` — verified on the roster audit).
- `tools/mm2_inspect` `events` (AC05 accounting): the reward-table
  block now prints `N authored rows → E event-bound + M milestone
  rules (<family sizes>), K diagnostics`, states its denominator
  explicitly, prints the block even when *every* authored row became
  a diagnostic (previously hidden by the non-empty-rule condition),
  and pushes a strict failure if accounted ≠ authored.

## Tests

- `cargo test -p mm2_app --test progression --test profile` — PASS
  (11 + 12, both new tests green).
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, 44 suites, 0 failures.
- Retail audit (`mm2-inspect events`, install
  `fnv1a64:e91e6cd4b2ae30d9`): london and sf each report `10 authored
  rows → 4 event-bound + 6 milestone rules (Blitz=10 Checkpoint=12
  Circuit=10 CrashCourse=13), 0 diagnostics`; `mm2-inspect cars`
  confirms `vpeagle` `0/1` for the VEH-5 correction.

## Still open

- F16-AC01's process-level leg: two real app launches completing an
  event. Headless `--bot` finishes are deliberately ineligible for
  records, so an interactive/operator run is needed — the
  synthetic-integration test above is the recorded evidence, honestly
  scoped.
- F16-AC06 (deliberate-delete UI confirmation) is F17 scope — no
  delete flow exists beyond `ProfileStore::delete`.
- F16-AC05's coverage enumeration is now printed per city by
  `mm2-inspect events` (retail: 10/10 authored rows accounted, 0
  diagnostics per city); promotion to `checked` awaits the external
  gate/review of this commit.
- `UnlockScore`/`UnlockFlags` semantics stay UNK-6; `vpmoonrover`
  roster path stays UNK-3; gate enforcement stays F17.
- Candidate pending external check.
