# Multiplayer mode coverage report (F26-AC06)

Which multiplayer modes the checkout verifies, by which test and at what
evidence level, and which original variants it does **not** support. It
follows the F21-C.1 pattern (`CourseCatalog::coverage()`, DSN-104): every
mode of the original list is a row — nothing is filtered — and a row is
only as strong as its *weakest* leg. Rule semantics stay in
`docs/original-rules.md`; this page maps rule rows to tests.

Evidence levels are those of `specs/QUALITY-GATES.md`. All two-process legs
are **loopback** (`mm2-host` / `mm2-join` as separate OS processes); none
says anything about LAN or Internet (see `docs/multiplayer-reachability.md`).
Tests marked *retail* skip without `MM2_RETAIL`; CI has no retail data.
Test files are under `crates/mm2_app/tests/`.

## Original modes (MP-4)

MP-4 lists Cruise, Blitz, Checkpoint, Circuit and Cops & Robbers.

| Mode | Status | Strongest verifying legs | Weakest point |
| --- | --- | --- | --- |
| Cruise | verified | `net_drive::two_retail_processes_stand_the_same_scenery_in_a_cruise` (retail, two-process); `net_drive::two_retail_processes_replicate_the_hosts_traffic` (retail); late join: `net_drive::a_client_that_joins_a_running_session_is_handed_the_live_one`, `net_app::a_late_joiner_converges_on_props_the_host_broke_before_it_arrived` | Networked traffic is DSN-71's enhanced policy, not an original claim. Cruise cops are not fielded over the network (COP-13). |
| Checkpoint | verified | `net_drive::two_retail_processes_roll_the_same_parked_cars_in_a_race`, `net_drive::two_retail_processes_stand_the_same_scenery_in_a_race`, `net_drive::a_remote_drivers_breakdown_crosses_two_processes` (+ `…survives_an_impaired_link`) (all retail, two-process); results: `net_app::race_progress_and_results_agree_under_each_impairment_recipe` (synthetic, impaired link); session gate: `net_check::check_accepts_a_retail_session` | Results leg uses a one-checkpoint synthetic definition (`wire_race_def`), not an authored course. |
| Blitz | partly verified | Breakdown: `net_drive::a_remote_drivers_breakdown_crosses_two_processes_in_a_blitz` (retail `blitz:0`, two-process, wreck knob `--wreck-at`/`--wreck-seat`). Event edit: `net_app` session-edit parse of `event=blitz:N`. | No two-process Blitz *finish/timeout/results* leg: the countdown clock and time-out outcome (BLZ-5) are exercised only through the single-player runtime and the shared race wire, not a networked Blitz run. Open follow-up. |
| Circuit | gate only | `net_check::check_accepts_runnable_sessions` / `check_bounds_customization_picks` (fixture `circuit:1` resolves and builds; `circuit:0` refused for `NumLaps` 0) | No networked Circuit run at any level: ordered gates and laps over the wire have no process or in-app leg (single-player ordered laps: `race::ordered_multi_lap_participants_stay_independent_and_order`). Circuit's wreck outcome (time penalty + reset, RACE-5) is not exercised remotely. Open follow-up. |
| Cops & Robbers, Free-for-all | verified | `net_drive::two_retail_processes_play_a_started_cops_and_robbers_match`, `…_decide_a_cops_and_robbers_match`, `a_joined_clients_bot_picks_up_the_gold` (retail, two-process, `--cnr ffa`); verdict on loss: `a_decided_cops_and_robbers_match_reaches_a_client_on_an_impaired_link`; late join: `a_client_that_joins_a_cops_and_robbers_match_mid_carry_reads_the_verdict`, `a_late_joiner_reads_the_cops_and_robbers_verdict_on_an_impaired_link` | Drop/steal/respawn mechanics are unknown (CNR-5/UNK-10): designed, not original. |
| Cops & Robbers, Cops vs. Robbers | partly verified | `net_drive::two_retail_processes_play_a_started_cops_and_robbers_match` (`--cnr cops`, retail, two-process: started match only). Variant wiring on the wire: `net_app::a_cops_and_robbers_frame_lands_as_a_replica_and_stale_or_foreign_ones_do_not` | Only a *started* match is exercised across processes; a decided cops-vs-robbers verdict is covered by `mm2_game::gold` unit tests and the in-app frame test. |
| Cops & Robbers, Robbers vs. Robbers | in-process only | `mm2_game::gold` unit tests (team totals, `RobbersVsRobbers` limit); wire encoding `mm2_app::cnrnet` | No two-process leg for the team variant. Open follow-up. |

## Cross-mode lifecycle legs (apply to every row above)

| Concern (F26 AC) | Leg |
| --- | --- |
| Rematch without reconnecting (AC05) | `net_drive::two_mm2_processes_play_a_rematch_without_reconnecting`, `net_app::a_rematch_can_change_the_session_without_dropping_the_peer`, `net_app::a_rematch_does_not_carry_the_last_rounds_broken_props_to_the_client` |
| City/mode change (AC05) | `net_drive::a_session_edit_typed_on_the_host_reaches_the_next_round`, `net_join::a_client_process_leaves_cleanly_when_the_changed_session_is_unrunnable` |
| Leaver's seat is freed (MP-5) | `net_drive::a_seat_freed_by_a_leaver_is_not_resurrected_for_the_next_joiner`, `net_edge::a_rejoined_client_gets_a_fresh_slot` |
| Host loss | `net_edge::a_killed_host_ends_a_driving_client` (the client ends; there is no host migration — below) |
| Impaired links (AC03) | `net_drive::the_process_level_impairment_matrix_records_each_recipe_cell` |

## Unsupported original variants

Marked unsupported on purpose; none is silently substituted.

| Original variant | Ledger row | Status |
| --- | --- | --- |
| MSN Zone and IPX transports | MP-1 | **Unsupported.** One TCP listener, exact `host:port` (reachability runbook). |
| Serial / modem 1v1 | MP-1 | **Unsupported.** |
| Host migration — "a new host is designated" on host disconnect | MP-5 | **Unsupported.** A lost host closes every client (`net_edge::a_killed_host_*`). |
| Max 10 concurrent hosted sessions per LAN | MP-2 | Not enforced; there is no discovery, so the limit has nothing to bound. |
| Host-chosen connection type, password | MP-3 | Not modelled (TCP only; no password field in the lobby protocol). |
| Eject via F6 / lobby Eject button | MP-7 | Not covered by any test in this report; treat as unsupported until a task lands one. |
| Cops & Robbers drop/steal/respawn/out-of-bounds specifics | CNR-5, UNK-10 | **Unknown originally**; the implemented rules are designed (see the C&R rows in the ledger). |
| Circuit laps/opponents option semantics | UNK-38 | Unknown; unverified over the network. |

Not original variants but out of scope here: ambient traffic, cops and AI
opponents in networked races (MP-4: none — the host fields none).

## Follow-ups this report leaves open

F26-AC06 asks that coverage *distinguish* each mode and the unsupported
variants; this page does so, but three rows are below "verified":

- a two-process networked Blitz finish/time-out leg;
- a networked Circuit run (ordered gates, laps, remote wreck);
- a two-process Robbers vs. Robbers match.

Unsupported-variant rows MP-7 (eject) and MP-2/MP-3 were classed from the
absence of code and tests found while writing this report, not from a
dedicated audit.
