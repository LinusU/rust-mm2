# Last iteration — menu-hosted Cops & Robbers lobby (iteration 19 of the new run)

Selection: iteration 18 (`b5dc910`) passed gates and review with no blocking findings. The plan's queued F27-C item that does not depend on the open bot-navigation findings is "a menu-hosted lobby offer": the main menu had no multiplayer path at all (`Multiplayer` row disabled, hosting only via `--host`), so the Cops & Robbers options screen could start a local match but never a hosted one.

Changes (candidate, not independently checked):
- `menu.rs`: `Action::HostCnr` / row `Host lobby` on `Screen::CnrOptions`; `MenuEffect::Host` (the `Launch` payload, produced by running the same `launch` resolution); `menu_input` opens the lobby, sets the car like a launch, hides the shell; `menu_watch` keeps the shell closed while a non-leaving `HostLink` exists (`LiveSettings` carries the link to stay under the argument lint).
- `net.rs`: `open_menu_host` (validate, `check_session`, gameplay fingerprint, catalog pick validator, fresh seed), `adopt_menu_host`, `spawn_host_text`, `close_menu_host` (removes link + lobby resources + text once leaving at `Menu` under a menu), `MenuHostBind`; `host_input` ignores the key press that opened the lobby (`is_added`).
- `main.rs`: the host systems moved into `add_host_systems`, registered for `--host` and for menu apps, gated `run_if(resource_exists::<HostLink>)`; `--menu-bind` (loopback:0 default, conflicts with `--host`; `--bind` stays a `--host` flag).
- Tests: `app::menu::the_menu_hosts_a_cops_and_robbers_lobby_and_returns_when_it_closes` (mutation-checked: dropping the `menu_watch` guard fails it); existing C&R menu test row list updated; `mm2_host_flag_gates_are_named_exits` gains `--host --menu-bind`. Docs: net.md paragraph, PLAN F27-C slice 5.

Not shown: a client joining a menu-hosted lobby (the machinery is `--host`'s and covered elsewhere), a windowed/rendered capture of the host text, a roster screen, in-menu rematch options. Bot-navigation findings (walled first legs, junction stalls, elevated sites) and a second driven client / steal remain open.

Gates (foreground, final tree): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed (no `MM2_RETAIL`, so retail legs skip).

Status: implemented candidate; not independently checked.
