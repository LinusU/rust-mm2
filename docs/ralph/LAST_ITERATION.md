# Last iteration — contested pickup and dropped carrier over the wire (iteration 3 of the 2026-10-09 run)

Selection: iteration 2 (`1951689`) passed gates and review with no blocking findings; operator reports 1–11 are all implemented, so no report item was ready. Took the F27-C leg the plan listed as open and not blocked by bot navigation: F27-AC02 (simultaneous pickup → exactly one carrier) and AC03 (carrier disconnect) through the *real host/wire path*, which the rule-level tests (`gold.rs`, `cnr.rs` rig with `PlayerControl::Local`) never touched. Remote seats, wire-id attribution (`participant_id`), the socket-drop retire of a seat and the published frames peers decode were untested together.

Changes (candidate, not independently checked):
- Test only: `network::net_app::two_remote_cars_reaching_the_gold_together_make_one_carrier_and_a_dropped_carrier_frees_it`. Host app with two raw `Client` peers + production `cnr_host_step`/`reconcile_gold_load`/`publish_cnr`. Two remote cars equidistant in reach on one step: one `Picked` (lower id), score sums to 25, load (250 kg) on the winner only, no stacking over 5 more steps, both peers decode the same single carrier. Carrier's TCP drops: `Dropped{Disconnected}` at its last position, one `Picked{recovered}` by the other peer, load moved (not stacked), leaver's points stay (`connected=false`), the survivor decodes the new carrier.
- Mutation checks: reversing the tie-break in `resolve_pickups` fails "the lower id wins a tie"; skipping `game.leave` in `cnr_host_step` fails "the carrier's departure drops the gold". 6/6 repeat runs green (~0.09 s).
- Docs: `docs/research/net.md`, PLAN F27-C slice 11. No production code changed.

Not shown: cars driven into reach (positions are placed; no physics runs in the test app), a contested steal by ramming, three OS processes, packet loss on the contest, a rendered match, retail data. F27-AC01/AC02/AC03/AC04 stay open at process level.

Gates (final tree): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed (no `MM2_RETAIL`). Status: implemented candidate; not independently checked.
