# Last iteration — F18-A.5: authored room-PVS render culling

Iteration on `ralph/night` (base 68f30bf). Selected the F18-A
remainder's `.cpvs` leg — the format and `IsRoomVisible` semantics
were already verified (WLD-23) and city meshes are emitted per
`(room, texture)`, so authored room culling only needed a room tag
and a source resolver. Investigated the F15-B opponent tail fields
first (`avoidOpponents`/`weirdPathfinding`): mm2hook's recovered
`OpponentData`/`RegisterRoute` clarify the vocabulary but the column
polarity stays ambiguous between two plausible assignments — left
documented, not guessed.

## What changed

- `mm2_app::pvs` (new): `CityRoom(u32)` component (authored room id =
  `Psdl::rooms` index + 1), `PvsEnabled` resource mirroring retail
  `sm_EnablePVS`, and `CityPvs` — the parsed `Cpvs` plus per-room
  perimeter cells built from the loaded `Psdl`.
- `apply_city_pvs` (Update, after the camera systems — windowed and
  headless): resolves the union of rooms whose authored XZ perimeter
  contains the active camera `Transform` or the player vehicle's
  physics `Position` — retail `sdlPage16::PointInPerimeter` is the
  same 2-D test, and `FindRoomId`'s `previousRoom` is a recovered
  search hint, so a rescan gives the same answer without ordering
  state. The source lists are decoded once on change and unioned —
  stacked/overlapping rooms or a boundary-lagged chase camera can
  only over-show, never hide an authored-visible room. The player leg
  keeps the street under the car a source and is a headless run's
  only position. `Visibility` is rewritten only when the source set
  changed, the toggle flipped, or new `CityRoom` entities spawned
  (`Added` — the first update precedes the deferred session spawns).
- `city.rs`: every per-room render group spawns `CityRoom` +
  explicit `Visibility::Inherited` (`Mesh3d` requires `Transform`
  only — without it the cull query matches nothing, which the first
  retail run caught: `pvs=687r/0h/0`); `load_city` resolves the
  sibling `<stem>.cpvs` through the VFS — a missing/unparseable table
  yields no `CityPvs` (unculled, logged, never fabricated), so mod
  cities without one are unaffected. `authored_z`/`point_in_poly`
  went `pub(crate)` for reuse.
- `session.rs`: inserts `CityPvs` (honouring `PvsEnabled`) on city
  load, removes it in `drive_session` teardown — session-scoped like
  `CityNav`. Colliders are physics and never culled.
- `main.rs`: `--no-pvs` CLI flag (retail's `EnablePVS(false)`
  counterpart); `apply_city_pvs` registered behind
  `resource_exists::<CityPvs>`.
- `smoke.rs`: headless runs `chase_follow` + `apply_city_pvs` so the
  camera path is exercised identically; the record gains
  ` pvs=<room>r/<hidden>h/<tagged>` (` pvs=off` when disabled; absent
  without a table — dev-world records stay bit-identical).

## Verification (this tree)

- `cargo fmt --all -- --check`, `cargo clippy --locked --workspace
  --all-targets --all-features -- -D warnings`, `cargo test --locked
  --workspace` — all pass (67 suites, 0 failures).
- Tests +5 (`pvs.rs`): tiled-room source resolution + culling,
  overlapping-room list union, unresolved/disabled bypass + miss
  counting, the full apply path (camera move re-applies,
  `culled`/`tagged` counters), player-position fallback with no
  camera.
- Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`):
  - sf `--frames 900 --spawn=-1319.5,80,214` → `pvs=982r/6378h/7324`,
    the source room tracking the driven car.
  - london `--frames 600` → `pvs=68r/7422h/8254`.
- Rendered A/B (frozen `--cam`, Metal/Apple M1, ImageMagick AE; local
  captures, not committed):
  - same-command baseline: ~1.5 kpx noise.
  - street level: 360 px over 3.7 MPX — capture noise, clean.
  - 75 m aerial: ~28.6 kpx real difference, all in distant skyline
    rooms the authored table marks invisible — the table's own
    conservatism at a non-gameplay viewpoint; retail draws the same
    holes from the same table.
  - a camera embedded under the freeway (~6% diff) is likewise an
    atypical position resolving an authored boundary.

## Not done / open

- `.cpvs` variant selection stays open (UNK-24): the numbered
  `<stem>_N.cpvs` files' per-weather switching is a measured
  hypothesis — the base table is consumed, variants are not
  selected.
- `FindRoomId`'s exact search order is unrecovered (thunk); the
  rescan-union is a designed policy (DSN-33) that can only over-show.
- Props/decals aren't `CityRoom`-scoped yet — only per-room city
  render groups cull; a prop whose room is hidden stays drawn (same
  conservatism direction as the union).
- F15-B's opponent tail columns (`avoidOpponents` polarity) remain
  unresolved — documented, not wired.
