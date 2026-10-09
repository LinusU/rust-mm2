# Last iteration — the menu join dials off the render thread (iteration 1 of the 2026-10-09 run)

Selection: iteration 22 (`b7511e6`) passed gates and review with no blocking findings. Operator report 7 items 1–11 are all implemented, so no report item was ready. Picked the one concrete defect slice 7 of F27-C had recorded: the menu's *Join lobby* dial ran inline in `menu_input`, so a host that accepts and never speaks froze the window for up to `HANDSHAKE_TIMEOUT` (10 s). Also fixed the review's cosmetic nit (the `never_capturing` helper had split `menu_app`'s doc comment from its function).

Changes (candidate, not independently checked):
- `net::begin_menu_dial` runs the VFS-bound checks in-frame (driver bound, pick, fingerprint — `plan_menu_join`) and moves `LobbyLink::join` + the pick offer to an `mm2-menu-dial` thread behind a `MenuDial` resource. `open_menu_join` (no other consumer) is gone; `MenuJoinError::Lost` names a dial thread that died without answering.
- `menu::menu_dial` (registered before `menu_watch` in the binary and in `tests/menu.rs`) adopts the answer: a link closes the shell and adopts the lobby, a failure goes to the status line with the address field still open. A link answered outside the `Menu` phase is dropped with a warning.
- `menu_input`: while a dial is pending, launch/host/join answer `still joining a lobby` instead of queueing; status reads `joining <addr>…`.
- Tests: new `a_silent_host_does_not_freeze_the_menu_join` (accepting listener that never speaks: `Enter` returns at once, frames keep running, a second `Enter` is refused, dropping the peer lands `cannot join …`); the two existing join tests wait on `settle_dial`. Docs: `docs/research/net.md`, PLAN F27-C slice 9.

Known behaviour, documented: `Esc` out of the field during a dial does not cancel it (bounded thread; a late success still adopts the lobby).

Not shown: a rendered capture of the `joining …` status, two OS processes through the rows, retail data. The new test was not mutation-checked against the old inline dial.

Gates (foreground, final tree): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` exit 0; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed (no `MM2_RETAIL`).

Status: implemented candidate; not independently checked.
