# Last implementation iteration

- Task ID and title: F16-A.1 repair #2 — external review rejected
  `08ceb8e` because the "deleted id is never reused" invariant was
  still falsifiable on a clean public-API path: `create` allocated
  `max(existing file suffix)+1` and `delete` removed every file a
  profile owned, so deleting the *highest-numbered* profile erased
  all trace of its id and the next `create` reissued it — resolving a
  stale `active` marker (or any saved reference) to a different
  person. The prior repair had only closed the crash-state vector
  (orphan `.bak`/`.tmp` owns its id); `deleted_ids_are_never_reused`
  deleted a non-max id, so the hole was untested.
- Starting commit: `08ceb8e21022c2ea55cbf8bc3b9b6bbf0310038c` on
  `ralph/night` — the failed F16-A.1 repair candidate; tree was clean.
- Retail install: `/Users/linus/coding/rust-mm2/retail` — untouched;
  this slice is user-data storage with no original-data interaction.

## Root cause

`existing_ids` derives store membership solely from surviving
filenames, and nothing persisted an allocation high-water mark. A
delete of the maximum id is therefore indistinguishable from that id
never existing: `create A(driver-0)`, `create B(driver-1)`,
`set_active(driver-1)`, `delete(driver-1)` (legal — two ids),
`create C` → `driver-1` again, and `active()` resolved B's stale
marker to C.

## What changed (`crates/mm2_game/src/profile.rs`)

- `create` now allocates through `allocate_id`: `max(next-id mark,
  max surviving file suffix + 1)`. The `next-id` file is a small
  marker written by `write_marker` — the same tmp sibling +
  `sync_all` + rename + directory fsync shape as the `active` marker
  and profile saves — and is advanced *before* the new profile's
  first `save`. Order matters: a crash between mark and save wastes
  a suffix; the reverse could reissue a live id. Losing or
  corrupting the mark degrades to the file-scan floor (a deleted
  highest id could then reissue — documented limit; no live profile
  is ever displaced).
- `set_active`/`active` and `next-id` share new `write_marker`/
  `read_marker` helpers; marker reads are now bounded
  (`MAX_MARKER_BYTES` = 4 KiB) like profile documents
  (`MAX_FILE_BYTES`), closing the unbounded `read_to_string` review
  gap.
- `summarize` distinguishes a superseded main ("main file holds an
  older revision") from a missing or unreadable one — the old
  message claimed "missing" whenever a `.tmp`/`.bak` won on
  `revision`, even with a healthy main.
- Module doc and DSN-15 (`docs/original-rules.md`) updated: the
  allocation rule and `load`'s three-copy revision recovery now
  match the code.

## Tests (`tests/profile.rs` — still 20; extended two)

- `deleted_ids_are_never_reused` gains the review's scenario: delete
  the highest id (`driver-2`) after `set_active`, assert the next
  `create` allocates `driver-3`, and assert the stale marker reads
  `None` rather than attaching to the new profile.
- `a_complete_tmp_is_newer_than_the_main_it_never_replaced` now also
  asserts the listing reports "older revision" (superseded), not
  "missing".

## Commands actually run and results

- `cargo test -p mm2_game --test profile` — 20/20 pass.
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — PASS, 0 warnings.
- `cargo test --locked --workspace` — all suites ok, 0 failures.
- Evidence classification: code gates + synthetic store tests against
  tempdirs. No retail/GPU/audio evidence applies.

## Still open

- F16-A remainder: app wiring (`default_root()`/`--profile` at
  startup, restore `selections`, persist on exit). AC01's
  restart-isolation evidence additionally needs F16-B's result→progress
  consumption. UI create/select/delete flows are F17 scope.
- F16-B/F16-C, F15-B remainder, F13-B/F14-B remainders, F11-C
  remainder — unchanged.
- Known limits (documented, user-data-dir adversary model): losing
  the `next-id` mark degrades non-reuse to the surviving-files floor;
  `revision` ordering is the recovery rule, not a tamper check — a
  hand-edited file with a forged high revision wins recovery;
  case-variant foreign filenames (`DRIVER-9.JSON`) are not counted
  yet collide on case-insensitive filesystems.
- `sf checkpoint:0`'s idle-player "fell through the world" smoke
  artifact remains pre-existing (unrelated).
