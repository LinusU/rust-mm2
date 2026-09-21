# Last implementation iteration

- Task ID and title: F16-A.1 repair — external review rejected
  `62fbb89` because `existing_ids` defined store membership solely by
  `<id>.json` main files, so a `.bak`/`.tmp` orphaned by an interrupted
  save or delete freed a still-live `driver-<n>` id for reallocation
  (the "deleted id is never reused" invariant broke on exactly the
  crash states the module advertises handling).
- Starting commit: `62fbb89d7cb56f74847f7b5105eac917c5afac8f` on
  `ralph/night` — the failed F16-A.1 candidate; tree was clean.
- Retail install: `/Users/linus/coding/rust-mm2/retail` — untouched;
  this slice is user-data storage with no original-data interaction.

## Root cause

`ProfileStore::existing_ids` (filename scan) matched only
`driver-*.json`. Two reachable states then violated id non-reuse:

1. A `save` crash between `rename(main, .bak)` and `rename(tmp, main)`
   leaves a complete `.bak` and no main — the profile vanished from
   `list()` while `load()` still recovered it, and `create()` handed
   its id to a new profile whose `.bak` then held the previous owner's
   data.
2. A `delete` crash or IO error after `remove_file(main)` left the
   same orphan-`.bak` shape.

## What changed (`crates/mm2_game/src/profile.rs`)

- `existing_ids` now collects stems from all three file names
  (`.json`, `.json.bak`, `.json.tmp`) with dedup — an orphan still
  owns its id for allocation, delete membership and DRV-7 counting.
- `list()`/`summarize` recover metadata from the newest surviving copy
  (`read_candidates` — max `revision` over main/tmp/bak): an orphan or
  corrupt-main profile lists with `meta` from the surviving file plus
  an error noting the recovery, instead of reporting as nonexistent.
- `load` keeps the highest surviving `revision` across all three
  files — a complete `.tmp` from an interrupted save is always a newer
  attempt than the main it never replaced (fixes the review gap where
  the freshest save was left on the floor; gives `revision` a real
  consumer). `ProfileError::Corrupt` gained a `tmp` reason slot.
- `ProfileId` boundary: `load`/`delete`/`set_active` reject ids that
  are not safe file stems (`Invalid`), and `active` treats a malformed
  marker as `None` — constructed `ProfileId::from("../x")` can no
  longer probe paths outside the store root.
- `set_active` now writes through `File` + `write_all` + `sync_all`
  before the rename, matching the doc claim; `save`/`delete`/
  `set_active` fsync the directory after the renames (`sync_dir`,
  Unix-only, no-op elsewhere) so the final rename/removal is durable.
- `delete` unlinks the main file last — a crash mid-delete leaves an
  intact profile or a recoverable backup, never a half-removed id.
- `validate` rejects unsorted or duplicate `progress.events` keys —
  `event_mut` binary-searches the Vec, so a hand-edited file that
  broke the order is corrupt rather than silently splitting records.

## Tests (`tests/profile.rs` — 16 → 20)

- Extended `interrupted_write_recovers_the_backup`: the orphan still
  lists (recovered meta + error) and `create` allocates `driver-1`,
  not `driver-0` — the review's blocking scenario.
- `a_complete_tmp_is_newer_than_the_main_it_never_replaced` — a
  parseable `.tmp` outranks a healthy older main on `revision`.
- `an_orphaned_backup_still_owns_its_id` — post-main-unlink state
  lists, loads from `.bak`, blocks reallocation, still deletable.
- `malformed_ids_are_rejected_before_any_path_probe` — `../escape`
  gets `Invalid` from load/set_active/delete and `None` from `active`.
- `unsorted_or_duplicate_event_records_are_corrupt` — hand-edited
  out-of-order `events` → `Corrupt`.
- `fully_corrupt_profile_reports_and_preserves_files` pattern updated
  for the new `tmp` field.

## Commands actually run and results

- `cargo test -p mm2_game --test profile` — 20/20 pass.
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — PASS, 0 warnings.
- `cargo test --locked --workspace` — 39 suites ok, 0 failures.
- Evidence classification: code gates + synthetic store tests against
  tempdirs. No retail/GPU/audio evidence applies.

## Still open

- F16-A remainder: app wiring (`default_root()`/`--profile` at
  startup, restore `selections`, persist on exit). AC01's
  restart-isolation evidence additionally needs F16-B's result→progress
  consumption. UI create/select/delete flows are F17 scope.
- F16-B/F16-C, F15-B remainder, F13-B/F14-B remainders, F11-C
  remainder — unchanged.
- Known limit (unchanged): `revision` ordering is the recovery rule,
  not a tamper check — a hand-edited file with a forged high revision
  wins recovery. Store files are user data; documented policy, not a
  security boundary.
- `sf checkpoint:0`'s idle-player "fell through the world" smoke
  artifact remains pre-existing (unrelated).
