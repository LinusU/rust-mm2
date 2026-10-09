# Last iteration — the menu joins a lobby (iteration 21 of the new run)

Selection: iteration 20 (`37e3aef`) passed gates and review; operator report 7 items 1–11 are all landed as candidates. The menu could host a Cops & Robbers lobby but only `--join` could enter one, so the in-game flow had no client half. Picked the F27-C / F24-B "real lobby menu surface" gap: a menu `Join lobby`. (A bot-navigation fix remains open but has no new evidence to act on.)

Changes (candidate, not independently checked):
- `menu.rs`: root row `Join lobby` → `Screen::JoinLobby { addr }`, a text field like `NewProfile` (ASCII graphic, ≤64 chars). `Enter` parses a `SocketAddr` (else status `type an address like 192.168.1.20:7777`), emitting `MenuEffect::Join { addr, vehicle }`. `menu_input` runs `net::open_menu_join`; failure → `cannot join <addr>: <why>` on the open field; hosting/joining while a lobby is up is refused. `menu_watch` holds the shell closed while a `LobbyLink` exists and shows `SessionNote::lobby` on reopen. `menu_graphics.rs` draws the field and hint.
- `net.rs`: `open_menu_join` (the `--join` fingerprint handshake and pick offer), `adopt_menu_join`, `spawn_lobby_text` (shared with `main`), `close_menu_join` (after `drive_lobby` queues the exit and the session is at `Menu`; despawns the text, removes link/state/snaps/seq/report, leaves the reason in `SessionNote::lobby`). `lobby_input` ignores the frame the link was added.
- `main.rs`: client systems moved into `add_client_systems`, gated `run_if(resource_exists::<LobbyLink>)`, registered for `--join` and menu apps (`close_menu_join` added); `--join` behaviour otherwise unchanged.
- `session.rs`: `SessionNote::lobby`.
- Test `menu::the_menu_joins_a_lobby_another_menu_opened`: two in-process menu apps; bad address and closed port refused on the field; a real join is rostered with its pick, offered the match, refused while unready, readied by `Enter`, then both reach `Playing` under the host's generation (joiner `Remote`); host quit returns the joiner to its lobby; `Esc` leaves and the menu returns with `left the lobby`. Passed 3/3. Mutation: dropping the joined hold in `menu_watch` fails it.
- Docs: `docs/research/net.md` (menu-joined lobby paragraph), PLAN F27-C slice 7.

Not shown: two OS processes joined through the menu rows, a driving menu-joined client, a rendered capture of the field/lobby text, retail data. The dial blocks a frame up to `HANDSHAKE_TIMEOUT` (10 s) on a silent peer; unreachable/mismatched lobbies answer at once. Bot-navigation findings and a second driven client / steal remain open.

Gates (foreground, final tree): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed (no `MM2_RETAIL`, retail legs skip).

Status: implemented candidate; not independently checked.
