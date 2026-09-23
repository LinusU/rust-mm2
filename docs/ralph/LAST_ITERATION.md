# Last iteration — F18-A.6: `.water` deadly-water consumption + F18-A.5 repairs

Iteration on `ralph/night` (base 3969fff). Selected the F18-A
remainder's `.water` leg and repaired both non-blocking defects the
external review found on F18-A.5 (`--no-pvs` ignored headless; the
PVS camera query unfiltered by `Camera3d`).

The `.water` refs' meaning was verified before designing the consumer
— a scratch probe over the retail PSDLs showed all six refs resolve to
flat attribute-less perimeter rooms at the water plane only under the
1-based room-id reading (same space as `PerimeterPoint.room`,
`road_rooms` and the `.cpvs` lists): london 345/351/356 are Thames
reaches at −4.0 under the authored −3.8 level, sf 228/399/401 ocean
reaches at −2.0 under −1.9. The 0-based reading lands sf's 401 on a
road tunnel at y≈30 — refuted. London's authored BAI lanes run to
−22, refuting a global "below level = deadly" rule: the level applies
inside listed rooms only.

## What changed

- `mm2_app::water` (new): `CityWater` — authored level plus each ref
  resolved to its room's XZ perimeter and a deadly bound of
  `max(level, room-top)` (honours mm2kiwi's "deadly water at any
  height" for elevated mod water; 0.1 m slack). Non-finite level or
  all-unresolvable refs → no resource; skipped refs are counted.
- `city.rs`: `room_poly` extracted `pub(crate)` (shared by `pvs.rs`
  and `water.rs`); `load_city` resolves the sibling `<stem>.water`
  through the VFS beside the `.cpvs` block — missing/unparseable → no
  `CityWater` (never fabricated), mod cities without one unaffected.
- `recovery.rs`: `track_recovery`'s `ground_contact` overlays
  `CityWater` on the wheel-`drag` classification — a grounded wheel
  whose contact point is exposed is water whatever the collider's
  material says; a car under a listed room's bound with no contact
  (clipped through the plane) is `Submerged` and accrues the dwell
  rather than free-falling to OOB. Decks above the bound inside the
  same XZ stay dry; resource-absent paths are bit-identical.
- `session.rs`: inserts `CityWater` on city load, removes it in
  `drive_session` teardown — session-scoped like `CityNav`/`CityPvs`.
- `smoke.rs`: the record gains ` wtr=<level>/<rooms>r` (`+<n>s` when
  refs skipped; absent without a record — dev-world stays identical).
- Repair 1 (F18-A.5 review): `DevOverrides::no_pvs` replaces the
  `PvsEnabled` resource — `main.rs` stores `cli.no_pvs` into
  `SessionConfig::dev` so `load_session_world` applies it on both the
  windowed and headless paths (the old resource was inserted only
  windowed, after the headless early-exit); `menu_mode` excludes it.
- Repair 2 (F18-A.5 review): `apply_city_pvs`'s camera query gains
  `With<Camera3d>` — a menu `Camera2d` surviving a transition frame
  can no longer contribute a source room (was bounded to over-show).

## Verification (this tree)

- `cargo fmt --all -- --check`, `cargo clippy --workspace
  --all-targets --all-features -- -D warnings`, `cargo test
  --workspace` — all pass (67 suites, 0 failures).
- Tests +8: `pvs.rs` `system_ignores_2d_cameras` (the repair);
  `tests/recovery.rs` ×5 over a synthetic deadly-water city (exposed
  contact → water event despite dry material; airborne under a listed
  bound → submerged + recovery; deck above bound stays dry; non-listed
  room untouched; resource-absent unchanged); `tests/session.rs`
  teardown removes `CityWater`; `tests/import_pipeline.rs` ×2
  (`load_city` binds level/rooms/skipped; missing record → none).
- Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`):
  - sf/london report `wtr=-1.9/3r` / `wtr=-3.8/3r` — all refs resolve.
  - `--headless --no-pvs` now prints `pvs=off` (repair 1 verified
    end-to-end; previously the flag was silently ignored headless).
  - staged `--spawn` inside london room 345 / sf room 228 below the
    bound → `rcv=3w/0f/3r` with `never grounded` on both cities — the
    airborne deadly-room overlay fires on authored data where no wheel
    contact exists.

## Not done / open

- `.water`'s original consumer and its exact room/level composition
  are unrecovered (UNK-24): the room-scoped bound, `max(level,
  room-top)` elevated-water reading, contact-point exposure and the
  airborne arm are designed policy (DSN-34), not original claims.
- `.cpvs` variant selection, `.ldef`/`.pvshist`/`.lmap` consumers,
  precipitation/wetness/audio and condition replication remain open
  (F18-B/C scope — UNK-24).
- The scratch probe used to verify ref indexing was removed after the
  runs; the findings live in `docs/research/environment.md`.
