# Last implementation iteration

- Task ID and title: F16-A.2 — profile application wiring. F16-A.1's
  store passed external review (`fd49a36`); its disclosed remainder was
  that no production consumer existed. This slice binds a driver
  profile at startup, restores its remembered selections, and persists
  the session's selections — the `mm2_game::profile` store API is the
  flow primitive; UI flows stay F17 scope.
- Starting commit: `fd49a360615a68f75369de2c5bcef594c3ab4f00` on
  `ralph/night`; tree was clean.
- Retail install: `/Users/linus/coding/rust-mm2/retail` — untouched;
  profiles live in the OS user-data dir (`ProfileStore::default_root`).

## What changed

- `crates/mm2_app/src/profile.rs` (new): `ActiveProfile` resource
  (store + working copy + `recovered_from_backup`), `ProfileRequest`
  (`Select`/`Create`/`Active`), `resolve`, `choose_launch`,
  `note_session_start`, `store_root`.
- `crates/mm2_app/src/main.rs`: `--profile <id|name>`,
  `--new-profile <name>`, `--sandbox` (requires `--new-profile`),
  `--profile-dir <dir>`, `--no-profile` (conflicts with the rest);
  `--paint` is now `Option<usize>` so "absent" and "explicit 0" are
  distinguishable. Interactive runs with no profile flag bind the
  store's `active` marker; smoke runs (`--headless`/`--frames`/
  `--screenshot`) never bind implicitly but honor an explicit request —
  their records must stay reproducible. Explicit failures exit 2;
  implicit failures warn and run profile-less. Vehicle selection now
  runs `choose_launch`: `--car`/`--paint`/`--pro` override the
  remembered vehicle/paint/rank; a remembered car that fails to load
  retries paint 0 then degrades to the stock default (warned — saved
  prefs never gate launch).
- `crates/mm2_app/src/race.rs`: `EventSetup.key` carries the event's
  stable `EventKey{city,table,stem}` — save identity by authored stem,
  never a table row index.
- `crates/mm2_app/src/session.rs`: `load_session_world` takes an
  optional `ResMut<ActiveProfile>` and calls `note_session_start` only
  after the world, player, race resources and phase transition succeed
  — a failed load records nothing, a cruise does not erase
  `last_event`, a dev-car session does not erase the remembered
  vehicle.
- `crates/mm2_app/src/smoke.rs`: `headless_smoke` accepts a bound
  profile and inserts it as a resource; the record gains `profile=<id>`
  only when one is bound — unbound records are bit-identical.
- Binding a profile recovered from `.tmp`/`.bak` re-saves once so the
  main file heals; selecting marks the profile `active` for later
  runs.

## Design decisions

- `last_event` is recorded but *not* launched — Quick Race is F17
  scope (DRV-8).
- Nothing here writes `progress`/`unlocks` — that is F16-B's
  authoritative result→progress pipeline (AC02/AC03/AC05 open).
- Sandbox profiles persist selections like standard ones; the
  `records_progress()` gate keeps them out of F16-B records.

## Tests (`crates/mm2_app/tests/profile.rs` — 11 new)

Resolution by id and unique name; ambiguous/unknown selectors error;
create binds + marks `active` + honors rank/kind; implicit `active`
bind (empty store → none); corrupt `active` degrades profile-less;
backup recovery heals the main file at bind; flag-over-remembered
precedence incl. difficulty; session-start persistence of vehicle +
`EventKey` through `load_session_world`; dev-car/cruise
non-clobbering; per-profile file isolation; failed load records no
event.

## Commands actually run and results

- `cargo test -p mm2_app --test profile` — 11/11 pass.
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --workspace --all-targets --all-features -- -D
  warnings` — PASS.
- `cargo clippy --locked …` — PASS, 0 warnings.
- `cargo test --workspace` and `cargo test --locked --workspace` — all
  40 suites ok, 0 failures.
- Evidence classification: code gates + synthetic tempdir/Bevy-app
  tests. No retail/GPU/audio evidence applies — no retail smoke was
  run this iteration (store paths are install-independent; the
  profile-free smoke path is unchanged when unbound).

## Still open

- F16-A parent: AC01's restart-isolation evidence needs F16-B's
  result→progress consumption to be observable; UI create/select/
  delete flows with deliberate confirmation are F17 scope.
- F16-B/F16-C, F15-B remainder, F13-B/F14-B remainders, F11-C
  remainder — unchanged.
- Store-level known limits unchanged (documented in F16-A.1's
  iteration notes): `next-id` mark loss degrades non-reuse to the
  file floor; revision ordering is recovery not tamper-proofing;
  case-variant foreign filenames uncounted.
- `sf checkpoint:0`'s idle-player "fell through the world" smoke
  artifact remains pre-existing (unrelated).
