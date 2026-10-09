# Last iteration — the menu tests run the production lobby schedule (iteration 22 of the new run)

Selection: iteration 21 (`96f742d`) passed gates and review. Its review's first verification gap: the menu-join test registered only `lobby_input`/`drive_lobby`/`close_menu_join`, so the rest of `add_client_systems` in menu mode (reconcile/apply/send systems) was checked by source reading only. Picked closing that gap for both the client and host sets (F27-C / F24-B).

Changes (candidate, not independently checked):
- New `mm2_app::lobby_systems` with `add_client_systems`/`add_host_systems`, moved from `main.rs` (the `capturing` freeze condition is a parameter). `main.rs` calls them; behaviour for `--join`/`--host` unchanged.
- The three menu lobby tests (`the_menu_hosts_a_cops_and_robbers_lobby_and_returns_when_it_closes`, `a_client_joins_the_lobby_the_menu_opened`, `the_menu_joins_a_lobby_another_menu_opened`) now register the full production sets (the join test also registers the `RemoteImpact`/`BangerStateChanged`/`RaceStarted` messages the binary registers).
- **Defect found by that:** with the full host set, `Esc` out of a menu-hosted lobby panicked (`publish_snapshots`: `Res<HostLink>` missing, `track_reset_epochs`: `ResMut<NetDriveReport>` missing). `close_menu_*` remove the link/resources by deferred commands which a sync point applied mid-schedule, before other lobby systems ran. Fix: every lobby system is in `HostedLobby`/`JoinedLobby`, and the close systems run `.after` that set. Before: host test failed on the first run (system names obtained with `--features bevy/debug`); after: passes.
- Docs: `docs/research/net.md`, PLAN F27-C slice 8.

Not shown: two OS processes joined through the menu rows, a driven menu-joined client, rendered capture, retail data.

Gates (foreground, final tree): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed (no `MM2_RETAIL`).

Status: implemented candidate; not independently checked.
