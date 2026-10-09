# Last iteration — a joined client's car takes the gold (iteration 15 of the new run)

Selection: iteration 14 (`8963672`) passed gates and review with no blocking findings; operator report 7 items 1–11 are all implemented. The next ready networking slice was F27-C's "second driven client". Proximity leaves are mod-only (WLD-26), so I took the client-driven Cops & Robbers leg.

Change (candidate, not independently checked): `net_drive::a_joined_clients_bot_picks_up_the_gold` — `MM2_RETAIL`-gated, retail sf free-for-all, seed 1291, host `--parked`, client `--bot`. Asserts the host's log names `Picked { player: PlayerId(1), recovered: false }`, the client's replica is unrefused (`ref0`), seats 2, undecided (`dec0`), and the client's announcer queued the call (`say>=1`). No production code changed.

Evidence (local, loopback, headless, `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail`): 3/3 passes, ~41 s each.

Measured limit (not fixed): I first tried the full client-driven *delivery* (host parked, client bot, 100 pts). It never finishes: the bot stalls in escape loops on the sf spawn hillside (client at 1291 reached the gold, then looped at e.g. (-1640,52,-25) for 160+ escapes). A sweep over 24 other seeds (client bot) found no pickup within ~140 s, and the host-driven control on four reach-audit seeds did not either — so the cause is the road-graph bot's navigation there, not the client path (a dev-world client bot drives 2.1 km). Seed 1291 stays the one that delivers from the host seat. Improving the bot's spawn-area navigation is a separate task; contested steals need two bots that both get moving.

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --locked --workspace` exit 0, 58 `test result: ok` lines, none failed (without `MM2_RETAIL`; the retail leg ran separately above).

Status: implemented candidate; not independently checked. Next: bot navigation out of the sf C&R spawn area (unblocks client delivery and a contested steal), AC05 impairment/late-join at process level, menu-hosted C&R lobby offer.
