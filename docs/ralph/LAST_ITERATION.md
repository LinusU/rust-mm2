# Last iteration — F18-A.6 review repair: zero-room drop + recorded test counts

Repair iteration on `ralph/night` (candidate 7cb64d5). The external
review verified the F18-A.6 implementation end-to-end on retail
(`wtr=-1.9/3r` sf / `wtr=-3.8/3r` london, `pvs=off` under
`--headless --no-pvs`, room-scoped `rcv=2w/0f/2r` kills inside
london room 345) but blocked on two recorded-evidence defects in
committed docs, not code defects.

## Root cause

1. **Recorded test counts were wrong.** LAST_ITERATION/PLAN claimed
   "Tests +8" including "`tests/recovery.rs` ×5" with a dedicated
   "resource-absent unchanged" scenario. The actual diff adds 12
   tests: `water.rs` unit tests ×4 (uncounted entirely), `pvs.rs`
   ×1, `tests/recovery.rs` ×4 (not ×5 — no dedicated
   resource-absent test exists; that path is covered only
   implicitly by the pre-existing recovery tests that run without
   `CityWater`), `tests/session.rs` ×1, `tests/import_pipeline.rs`
   ×2.
2. **Documented failure policy was not enforced.** The water.rs
   module doc, PLAN F18-A.6 row and DSN-34 all state that refs
   resolving to no rooms yield no resource, but `load_city`
   inserted `Some(CityWater)` unconditionally after a successful
   parse with a finite level — an all-unresolvable record produced
   a resource with `rooms: []`/`skipped: n`. Functionally harmless
   (`is_deadly` false everywhere) but the docs did not match the
   code.

## What changed

- `city.rs`: `load_city` now warns (`water refs resolved no rooms;
  deadly water off`, with the skipped count) and yields `None` when
  `CityWater::build` resolves zero rooms — the documented policy,
  making resource presence mean "deadly water is live" and treating
  an all-unresolvable record identically to a missing/unparseable
  one. The alternative fix (correcting the three doc statements to
  bless the empty resource) was rejected: presence-as-configured is
  the cleaner contract for future consumers, and the guard makes
  the already-committed claims true rather than rewriting them.
- `water.rs` module doc: the failure-policy paragraph now says the
  drop happens in `load_city` and that a present `CityWater` always
  lists ≥1 deadly room.
- `tests/import_pipeline.rs`: +1 regression —
  `vfs_to_city_rejects_a_water_record_with_no_resolvable_refs`
  asserts all-bad refs (`0`/`7`/`-2`) and a ref-less record both
  yield `water: None` through the production `load_city` path.
- PLAN.md: F18-A.6 row corrected to +13 tests (12 in the candidate
  + 1 repair regression) with the real breakdown and the phantom
  "resource-absent" claim re-attributed to the pre-existing
  recovery tests; narrative records the repair. DSN-34 and the
  water.rs policy statements now describe enforced behaviour — no
  wording change needed there beyond the load_city clarification.
  The 7cb64d5 commit message's "all-unresolvable refs yield no
  resource" is now accurate post-repair (the guard it described is
  the missing piece this iteration adds).

## Verification (this tree)

- `cargo fmt --all -- --check` — clean; `cargo clippy --workspace
  --all-targets --all-features -- -D warnings` — clean; `cargo test
  --workspace` — 67 suites, 0 failures (2026-09-23, this tree).
- Targeted: `cargo test -p mm2_app --test import_pipeline` — the
  new regression passes with the guard; it asserts `water.is_none()`
  on the path that previously returned `Some`, so it fails without
  it. The existing bind/missing-record tests still pass.
- Behaviour on authored data is unchanged: both retail cities ship
  all-resolvable records (`wtr=<level>/3r`), so the guard never
  fires on retail; it only rejects mod/corrupt records that bind
  nothing.

## Not done / open

- Unchanged from the candidate: `.water`'s original consumer and
  exact room/level composition are unrecovered (UNK-24); the
  room-scoped bound, `max(level, room-top)` elevated-water reading,
  contact-point exposure and the airborne arm are designed policy
  (DSN-34).
- `.cpvs` variant selection, `.ldef`/`.pvshist`/`.lmap` consumers,
  precipitation/wetness/audio and condition replication remain open
  (F18-B/C scope).
- The review's noted evidence gaps stand: the exact staged-spawn
  coordinates of the claimed `rcv=3w/0f/3r` run weren't recorded
  (reviewer reproduced `rcv=2w/0f/2r` — same overlay, different
  spawn depth), and install fingerprint `fnv1a64:e91e6cd4b2ae30d9`
  is self-reported.
