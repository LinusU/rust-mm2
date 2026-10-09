# Last iteration — a client joins the menu-hosted lobby (iteration 20 of the new run)

Selection: iteration 19 (`992144a`) passed gates and review. Operator report 7 items 1–11 are all landed as candidates, so the plan's next ready F27-C work applies. The reviewer's first verification gap was that no test had a client join a menu-hosted lobby; the previous handoff listed the same gap. This slice closes it at synthetic-integration level.

Changes (candidate, not independently checked):
- `crates/mm2_app/tests/menu.rs`: `a_client_joins_the_lobby_the_menu_opened`. A raw `mm2_net::Client` joins the loopback listener `Host lobby` opened, using the install's own gameplay fingerprint. It checks:
  - it receives the `cops & robbers` session ad and a roster naming it;
  - the host's `LobbyState` rosters it;
  - `Enter` with the joiner unready is refused (`bob is not ready`) and the session stays at `Menu`;
  - after `SetVehicle("vpt")` and `SetReady`, `Enter` reaches `Playing` and the client reads `Start` with the host's generation and the same session;
  - the host's quit gives the client `Cancel{generation}`, and `Esc` removes the `HostLink`.
  The waits are bounded by a 10 s wall-clock deadline (the lobby loop is its own thread), not by frame counts.
- No production code changed. PLAN F27-C slice 6 added.

Not shown: a second `mm2` process joining a menu-hosted lobby (net_drive), a client driving in that session, a rendered capture of the host text, a roster screen, in-menu rematch options, retail data. Bot-navigation findings (walled first legs, junction stalls, elevated sites) and a second driven client / steal remain open.

Gates (foreground, final tree): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed (no `MM2_RETAIL`, retail legs skip). The new test passed 3/3 in isolation.

Status: implemented candidate; not independently checked.
