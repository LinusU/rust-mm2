# Last iteration — late join into a Cops & Robbers match (iteration 17 of the new run)

Selection: iteration 16 (`5e29dd9`) passed gates and review with no blocking findings. Operator report 7's items 1–11 are all implemented candidates; the plan's next F27-C item was late join at process level (F27-AC05), which does not depend on the open bot-navigation findings.

Finding: a first test run was rejected — `rejected: the session has already started`. `net::late_join_policy` kept a started C&R lobby `Closed` (an implementation choice: "no join-time unicast of the match"), against MP-5 (Cruise and C&R allow join at any time). The repeating whole-view frame (`cnrnet`) already is the late joiner's way in and `enroll_cnr_participants` already seats cars as they appear.

Changes (candidate, not independently checked):
- `late_join_policy`: `CopsAndRobbers` → `LateJoin::Open` (ledger DSN-102; `only_cruise_stays_joinable` renamed/updated; `mm2_host` comment).
- Defect exposed by the first admitted late joiner on a retail city and fixed: `net::start` seated a late joiner in the **dev car** whenever its `Start` outran the echo of the pick it offered on joining, while the host simulated its `vpbug` copy. Wheelbase/ride height differ (measured: contact points 1.55×2.55 m vs 1.70×2.69 m, origin 0.88 m above the host's), so the first settled divergence teleported the predicted car 0.8 m into the ground and it fell forever (`status=fail … fell through the world`; reproduced in plain Cruise on retail sf, 5/5 fail before). Fix: `net::start_pick` — the echoed roster pick, else the pick we offered (`offered_selection(SelectedCar)`). Unit test `a_start_seats_the_echoed_pick_else_the_offered_one`.
- New process test `net_drive::a_client_that_joins_a_cops_and_robbers_match_mid_carry_reads_the_verdict` (`MM2_RETAIL`, retail sf, ffa, seed 1291, `100pts`): host `--bot` alone, parked client connects after the host's `Picked`; asserts host `Ended(PointLimit)`/`Delivered` for player 0 and the client's `ref0, seats2, dec1, win=p0, phase=results, landed>0`, `status=pass`.
- Docs: `docs/research/net.md` (policy sentence + "A late join mid-carry" evidence), ledger DSN-102, PLAN F27-C slice 3.

Evidence (local, loopback, headless, `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail`): the new test passes (75 s) after the fix; before it, the same test failed with the client at y −224639 (`fell through the world`). Not shown: late join under loss, a driving or delivering joiner, contested pickups, a rendered match. The dev-world late-join legs (flat ground, same dev car both sides) cannot see the pick defect.

Gates: see the end of this file.

Gates (foreground, final tree): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed (without `MM2_RETAIL`, so the retail legs skip there). Retail runs on the final tree: `mid_carry` (74 s), `two_retail_processes_decide_a_cops_and_robbers_match` (74 s) and `a_late_joiner_lights_the_scene_like_the_host_on_a_retail_city` pass. Single observed run of the new test after the fix plus the one earlier run (75 s); no repeat-count claim.

Status: implemented candidate; not independently checked. Next: bot navigation (walled first legs, junction stalls, elevated sites) for client delivery and a steal; late join under the impairment recipe; a menu-hosted C&R lobby offer.
