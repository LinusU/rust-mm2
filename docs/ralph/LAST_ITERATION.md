# Last iteration — a decided Cops & Robbers match under packet loss (iteration 16 of the new run)

Selection: iteration 15 (`7f60d2a`) passed gates and review with no blocking findings. The plan's next F27-C items were bot navigation out of the sf spawn corner (unblocks client delivery and a steal), AC05 impairment/late join at process level, and a menu-hosted lobby. I spent the first part on the bot stall (below) and landed the AC05 leg, which does not depend on the bot driving from the client seat.

Change (candidate, not independently checked): `net_drive::a_decided_cops_and_robbers_match_reaches_a_client_on_an_impaired_link` — `MM2_RETAIL`-gated; the decided-match run (retail sf, ffa, `100pts`, seed 1291, host `--bot`, parked client) with the client's link through an armed `ImpairProxy` (30 % loss, 10 % duplicate, 10 % reorder, 40±30 ms, both directions, armed 400 ms after `Start`). Asserts the host decided `PointLimit` for player 0; the client record shows `ref0`, seats 2, `dec1`, `win=p0`, `phase=results`, `landed>0`; and the proxy counters show the recipe bit (`down.dropped>0`, `down.delayed>0`, `up.frames_in>0`). No production code changed. The first iteration-15 test (client picks up the gold) is untouched.

Evidence (local, loopback, headless, `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail`): 4/4 passes, ~73 s each. One run: proxy down 9027 in / 2495 dropped / 584 duplicated / 582 reordered, up 6752 in / 1879 dropped; client `cnr=sent0,landed76,stale133,ref0,seats2,solo2,…,dec1,win=p0,say2`. Not shown: late join, a driving client under loss, undecided-frame loss at finer grain, contested pickups, rendering.

Bot-navigation investigation (measured, no code kept):
- Single-process repro: `--host --cnr ffa --bot --seed 1291` plus a `start` on stdin. The car picks up at tick 763 near (-1291,259) and then loops in escapes at (-1285,260) for thousands of ticks.
- Cause found: the post-pickup road line is `[car, (-1291.4,271.9), …]` — the first leg is 12 m straight through a courtyard wall to the street lane (top-down capture of the spot shows the wall). The graph cannot see walls.
- Tried: a trail of breadcrumbs plus a re-plan from an earlier on-road crumb (when a world ray finds the first leg blocked, or after two escapes). It reached the same delivery later than the unmodified bot (tick 14344 vs 12370, deterministic), and the client-seat run still wandered at a junction near (-1340,400). Reverted; no benefit shown, so no code kept.
- Seeds 93/196 draw a gold at y 22.5; the bot reaches it horizontally at y 14.1 and stalls 8 m below, so `mm2-inspect cnr`'s reach audit (5 m vertical band) over-reports what the router drives to.

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` exit 0; `cargo test --locked --workspace` exit 0, 58 `test result: ok` lines, none failed (without `MM2_RETAIL`, so the new test skips there; the retail runs above are separate).

Status: implemented candidate; not independently checked. Next: bot navigation (walled first legs, junction stalls, elevated sites) for client delivery and a steal; late join at process level; menu-hosted C&R lobby offer.
