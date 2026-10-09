# Last iteration — a client's match view under loss and reordering (iteration 4 of the 2026-10-09 run)

Selection: iteration 3 (`d5e0163`) passed gates and review (no blocking findings). No operator-report item is open. Took the next open F27-C leg that needs no bot navigation: F27-AC05 ("teams/scores/winner agree across clients under packet loss/reordering"), in-process. Existing evidence covered only the *decided* frame under loss and raw-wire stage behaviour; nothing ran the production `publish_cnr` → real socket → `apply_cnr` → `CnrReplica` chain through a lossy link for *undecided*, changing state. Also took the review's non-blocking nit: `until_wire` had no deadline against a stream that keeps sending other messages.

Changes (candidate, not independently checked):
- Test only: `network::net_app::a_clients_replica_converges_on_the_hosts_match_through_loss_and_reordering`. Host app + joined client app, client dialled through a seeded `ImpairProxy`. Lossy phase (35 % loss, 15 % dup, 20 % reorder, 8±8 ms) with scripted pickups/drops/deliveries and a 60-step burst of back-to-back state changes: replica freshness never steps back, the stage counts stale frames (so the recipe reached it). Then the link levels the replica; a total blackout swallows one last change; replica provably behind (wrong carrier); healed link → replica equals the host's whole view only when the 120-tick cadence frame lands (index asserted within 115..=130); carrier, each score and both team totals agree.
- Mutation checks: disabling the stage's stale test fails the `stale() > 0` guard; pushing the cadence out of reach fails the heal. 6/6 repeat runs green (~2.7 s). (An earlier draft of the test passed under the stale-test mutation — its recipe never produced adjacent match frames — so it was rebuilt with the burst and the `stale() > 0` guard.)
- `until_wire` now fails after `WAIT` instead of looping on a chatty stream.
- Docs: `docs/research/net.md`, PLAN F27-C slice 12. No production code changed.
- Recorded consequence: a lost *last* change of an undecided steady state is stale on a client for up to one cadence period (120 match ticks). Not changed.

Not shown: loss on the real TCP transport (proxy drops whole frames), a second concurrent client, late join under this recipe in-process, driven cars, a rendered scoreboard. F27-AC01/AC02/AC03/AC04/AC05 stay open at process level.

Gates (final tree): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed (no `MM2_RETAIL`). Status: implemented candidate; not independently checked.
