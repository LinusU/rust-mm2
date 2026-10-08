# Last iteration — report 7 item 5: Cops & Robbers arrow and map markers (iteration 5 of the new run)

Selection: report 7 outranks everything; items 1-4 are implemented and the previous gate/review passed with no blocking findings, so item 5 is next.

Cause: the premise in the report was slightly off — `hudmap.rs` had *no* gold/bank/hideout roles (its `MarkerRole` was Player/Opponent/Gate/Finish; the three in `cnr.rs` are the 3D world markers). Both instruments were race-only: `spawn_nav_arrow` ran in the event arm and `update_nav_arrow` read `RaceState`; `spawn_hud_map` drew dots only from a `RaceDefinition`. A C&R match has no race, so neither existed.

Change:
- `cnrhud::objective` + `CnrScene` (SystemParam): one answer off the host's match or the client's replica — gold position (carrier's car while a rival holds it, none while the local car does), hideout/bank sites, and the arrow target (gold, or the local side's `delivery_target` site while carrying; none after the match ends or while unseated).
- `navarrow`: `spawn_nav_arrow_pkg` + `GENERIC_ARROW_PKG`; `update_nav_arrow` falls back to the C&R objective with the local car's bearing. `session.rs` spawns the arrow (and its 3D view) after `start_match`.
- `hudmap`: `MarkerRole::{Gold,Hideout,Bank}` with the authored `hudmap_square` paint jobs 6/7/8 (shader records `GOLD_DOT`/`BANK_DOT`/`HIDEOUT_DOT`, read with `mm2-inspect pkg --shaders`); `spawn_hud_map` takes a `cnr` flag; `drive_hud_map` places them from the objective every frame. Host and client share the path (the client reads `CnrReplica`).
- Ledger DSN-101 (designed): arrow family, chasing the carrier's car, hiding while carrying are implementation choices.

Tests: `cnrhud::tests` +4 — objective for resting/carried (robber → hideout, cop → bank, rival carrier followed, carrier out of view), unseated/decided match, and `update_nav_arrow` end to end with no race (visible, bearing = gold, swings to hideout on carry, `H` gate hides, no match hides).

Retail evidence (sf, Apple M1, local, uncommitted in `/tmp/rm5`): `--city sf --cnr cops --frames 240 --screenshot` shows the green 3D arrow at top-centre and the scoreboard; headless smoke `arr=hudarrow01/ahead map=inset/north/z1195/hudmap_sf.pkg/6t/4m` (markers 1 → 4). `--pause-map` capture shows the gold, bank and hideout dots at their sites on the full-screen map. No before capture: the before state is structural (no arrow node, 1 map marker), not a rendering difference.

Not done / open: the dots use the authored `IconScale`, so they are small on the full-screen map (same as race gate dots); not enlarged. The arrow's pointing direction was only checked by the unit test and the screenshot showing it live, not against a driven approach to the gold. Remote cars still have no opponent tri on the C&R map (opponent pool is sized from the race roster, 0 here) — a candidate follow-up. Two-process client check not run; the client path is the same `CnrScene` reading `CnrReplica` (the replica branch is exercised by the existing scoreboard test, not by a new arrow test).

Status: candidate; not independently checked. Next is item 6 (trees standing in a London intersection).

Gates: see below.
Gates (foreground): fmt PASS; clippy `--locked --workspace --all-targets --all-features -D warnings` PASS; `cargo test --locked --workspace --no-fail-fast` exit 0 (73 `test result` lines, none failed).
