# Last iteration — late join under loss (iteration 18 of the new run)

Selection: iteration 17 (`bbe3a52`) passed gates and review with no blocking findings. The review's non-blocking items were handled first: the cosmetic misplaced doc comment (`the_vehicle_validator_gates_picks_by_catalog`'s doc had ended up above the new `start_pick` test) is back where it belongs; the refused-pick corner of `start_pick` was traced and documented. Then the plan's next F27-C item that does not depend on the open bot-navigation findings: late join under the impairment recipe (F27-AC05).

Refused-pick corner (traced, not changed): a pick the host's validator refuses leaves the roster entry `pick: None`; `desired_remotes` only builds bodies for picked seats, so the host spawns no body for that seat and there is no pose to mismatch. The client seats the refused offer locally and gets `VehicleRefused` as a lobby notice. No test; noted in `docs/research/net.md`.

Changes (candidate, not independently checked):
- New process test `net_drive::a_late_joiner_reads_the_cops_and_robbers_verdict_on_an_impaired_link` (`MM2_RETAIL`, retail sf, ffa, seed 1291, `100pts`): host `--bot` alone; the parked client connects through a seeded `ImpairProxy` after the host's `Picked`; the recipe (30 % loss, 10 % dup, 10 % reorder, 40±30 ms, both directions) arms 400 ms after the host logs `event=vehicle id=1`. Asserts host `Ended(PointLimit)`/`Delivered` for player 0, the proxy counters (dropped/delayed/up traffic > 0), and the client's `status=pass, ref0, seats2, dec1, win=p0, phase=results, landed>0`.
- Doc fixes: validator test doc comment; mid-carry test doc points at the lossy variant. PLAN F27-C slice 4; net.md evidence paragraph.
- No production code changed.

Evidence (local, loopback, headless, `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail`): 3/3 passes (~74 s each). Measured run: host `Picked` tick 763, `Delivered`/`Ended` tick 12370; client `status=pass … dec1 win=p0 phase=results stale133`; proxy dropped 2354/8575 down, 1667/6144 up. Not shown: loss on the join handshake itself (Start/pick are one-shot verbs, outside the recipe by construction), a driving or delivering joiner, contested pickups, a rendered match.

Gates: see the end of this file.

Gates (foreground, final tree): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed (without `MM2_RETAIL`, so the retail legs skip there; the new test was run separately with it, 3/3).

Status: implemented candidate; not independently checked. Next: bot navigation (walled first legs, junction stalls, elevated sites) for client delivery and a steal; a menu-hosted C&R lobby offer.
