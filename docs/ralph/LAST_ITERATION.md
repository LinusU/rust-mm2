# Last implementation iteration

- Task ID and title: F16-A.1 — versioned profile storage with atomic
  load/save and isolated identities (the storage leg of F16-A).
- Starting commit: `1720a5378e75ac8f7db3bd54cc4c437e7805154a` on
  `ralph/night` — the iteration-14 candidate the external review
  passed (F11-C.1).
- Retail install: `/Users/linus/coding/rust-mm2/retail`
  (`fnv1a64:e91e6cd4b2ae30d9`) — inspected, not written to.

## Why this slice

The plan's selection policy offered F13-B/F14-B remainders, the F15-B
remainder, or the F11-C remainder. Reassessed: the F11-C remainder is
evidence-recording (run-and-record audits), not code; F13-B/F14-B's
open legs are scoped to F15/F17 (results screen, opponent hooks
already landed); the F15-B remainder's named items are research-gated
(catch-up semantics unverified, `weirdPathfinding`/`distancePadding`/
`cornerBrakingThreshold` consumption deferred "once semantics verify",
`avoidOpponents` polarity open). F16-A was the highest-value *ready*
slice — both dependencies (F01-A, F11-A) are checked — and it unblocks
the F16-B → F17-A chain (rewards, then menus/user flow). The retail
`players/` binaries (`player<N>.sav`/`*.cfg`, 17 files) were inspected
for shape only; per the F16 non-goal our own format claims no
compatibility with them.

## What changed

- `crates/mm2_game/src/profile.rs` — new module:
  - `PlayerProfile`: `version` (=`PROFILE_SCHEMA_VERSION` 1, stamped
    on save, rejected on mismatch), `ProfileId` (`driver-<n>`),
    `name` (≤32 chars, no control chars — duplicates legal, the id is
    the identity), `rank` (reuses `Difficulty`, DRV-2), `kind`
    (`Standard`/`Sandbox` — the spec req-5 split, gated by
    `records_progress()`), `revision` (per-save counter),
    `progress` (`Vec<EventRecord>` sorted by key + `unlocks` id set),
    `selections` (last vehicle/paint, `last_event` for DRV-8), and
    `extra` (`#[serde(flatten)]` — unknown top-level fields written
    by a newer build round-trip verbatim, spec req 4).
  - `EventKey{city, table, stem}` keys progress by the event's
    authored file stem — not the table row index — so a mod inserting
    a row cannot silently retarget a saved record (spec req 4).
  - `ProfileStore`: `open`/`list`/`create`/`load`/`save`/`delete`/
    `active`/`set_active`/`default_root`. Ids allocate max-suffix+1 so
    a deleted id is never reused. `save` = tmp write + `sync_all` +
    rotate `.bak` + rename — a crash at any point leaves a complete
    document under one of the three names. `load` falls back to `.bak`
    and reports `ProfileLoad::recovered_from_backup`; corrupt files
    still `list()` by id with their error and are never deleted by a
    read (denominator honesty); a parsed file whose `id` field
    disagrees with its file stem is corrupt, not a different
    identity. `delete` removes only that id's three files and refuses
    the store's last profile (DRV-7). `default_root()` resolves the OS
    user-data dir (macOS `~/Library/Application Support/rust-mm2/
    profiles`, Windows `%APPDATA%`, other Unix `$XDG_DATA_HOME` or
    `~/.local/share`) — original installs stay read-only.
- `config.rs`: `Difficulty` and `EventTableKind` gained serde derives
  (`snake_case` wire names); `EventTableKind` gained `Ord` for the
  sorted progress container.
- `Cargo.toml`/`Cargo.lock`: `serde` + `serde_json` deps on mm2_game
  (both already in the lockfile — no new crates), `tempfile` dev-dep.
- `lib.rs`: `pub mod profile` + re-exports.

## Tests (`mm2_game` profile suite — 15 new)

`tests/profile.rs`: create→mutate→save→load round trip; unknown
fields survive a round trip; two-profile isolation (mutate one, the
other untouched) + listing; duplicate display names → distinct ids;
invalid names (empty/blank/over-cap/control) rejected; deleted ids
never reused; corrupt main → backup recovery flagged; interrupted
write (missing main + stale tmp + valid bak) recovers; fully-corrupt
profile → `Corrupt` with per-file reasons, files preserved, still
listed; `version: 99` rejected; file naming a different id → corrupt;
last-profile delete refused (DRV-7) and still loadable; delete scoped
to one id (main+bak+tmp gone, sibling intact, re-delete →
`UnknownProfile`); `active` marker round trip + stale marker → `None`;
sandbox `records_progress()` gate.

## Commands actually run and results

- `cargo test -p mm2_game --test profile` — 15/15 pass.
- `cargo fmt --all -- --check` — PASS (after `cargo fmt`).
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — PASS, 0 warnings.
- `cargo test --locked --workspace` — all suites ok, 0 failures
  (mm2_game profile suite 15/15; no existing test touched or removed).
- Evidence classification: code gates + synthetic store tests against
  tempdirs. No retail/GPU/audio evidence needed — the store is user
  data, not content; nothing in the original install is read or
  written by this slice.

## Ledger / research updates

- `docs/original-rules.md` — DSN-15 records the profile-store design
  (own format, no retail-save compatibility; atomic save protocol;
  stem-keyed progress; sandbox gate; DRV-7 store enforcement).
- `docs/ralph/PLAN.md` — F16-A → active, F16-A.1 row added, selection
  narrative updated.

## Still open

- F16-A remainder: app wiring — resolve `default_root()`/`--profile`
  at startup, restore `selections` into `SessionConfig`, persist on
  exit. UI create/select/delete flows are F17 scope. AC01's
  restart-isolation evidence additionally needs F16-B's authoritative
  result→progress consumption to exist.
- F16-B (reward/unlock rules, idempotent grants — `unlocks` set and
  `records_progress()` gate are the seams) and F16-C (full matrix)
  unchanged.
- F15-B remainder (catch-up semantics, measured difficulty effects,
  param-tail consumption once verified, `avoidOpponents` polarity,
  fixed-seed soak), F13-B/F14-B remainders, F11-C remainder — all
  unchanged.
- `sf checkpoint:0`'s idle-player "fell through the world" smoke
  artifact remains pre-existing (unrelated to this slice).
