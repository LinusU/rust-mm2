# Last iteration — `Esc` cancels a menu join in flight (iteration 2 of the 2026-10-09 run)

Selection: iteration 1 (`9b155f1`) passed gates and review with no blocking findings. Operator report 7 is fully implemented, so no report item was ready. Repaired the review's cosmetic nit first (in `tests/menu.rs` the `the_menu_joins_a_lobby_another_menu_opened` doc comment had been merged onto `settle_dial`; moved back). Then took the one defect that slice recorded as "known behaviour": `Esc` out of the address field did not cancel a dial, so a host answering later pulled the player into a lobby they had left (F27-C, F27-AC05 late-join/leave edge).

Changes (candidate, not independently checked):
- `net::MenuDial::cancel` + `MenuJoinError::Cancelled`: a cancelled dial's `poll` answers `Cancelled` whatever the thread found; `menu_dial` removes the resource (dropping the receiver, so the late link is dropped) and leaves the status alone.
- `menu::menu_input`: `Back` on `Screen::JoinLobby` with a dial pending cancels it; status `cancelled joining <addr>`.
- Tests: `escaping_the_address_field_cancels_the_join_in_flight` (silent host; dial gone after `Esc`, a new join accepted, peer's later hang-up changes nothing) and `a_host_that_answers_after_escape_is_not_adopted` (gated relay to a real menu-hosted lobby: the host's reply crosses only after `Esc`; no `LobbyLink`, and the relay sees the abandoned link's socket close). Both fail with the `cancel()` call removed (mutation-checked: "a join the player cancelled was adopted").
- Docs: `docs/research/net.md`, PLAN F27-C slice 10.

Not shown: the `HANDSHAKE_TIMEOUT` expiry path (the silent-host test ends via the peer hanging up), a rendered capture of either status line, two OS processes through the rows, real network/Windows.

Gates (final tree): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed (no `MM2_RETAIL`). Status: implemented candidate; not independently checked.
