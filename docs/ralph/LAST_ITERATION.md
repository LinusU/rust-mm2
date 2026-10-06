# Last iteration — reconcile the field-races rule with the networked
# Results deferral (new-run iteration 3)

Baseline `9ac6f17`. Selection: report 6 follow-up 2 (the host deferral vs
`field_races`), the smallest open networking item with no wire change.

## What landed

- Decision recorded in `docs/original-rules.md` DSN-11: one rule. The
  hosted `Playing → Results` deferral only delays the phase edge the
  snapshot stream needs; `field_races` already covers `Playing`, so the
  simulated field records real finish times through the hold exactly as
  behind a single-player results screen. A client reaches `Results` on
  its own seat's replicated terminal edge.
- Test `race::the_field_keeps_racing_through_the_wire_deferral`: host
  driver finishes first (holds `Playing`), an AI opponent finishes
  during the hold, the wire seat's finish releases `Results`, a
  straggler finishes behind it with a later time; ledger holds 4.

## Evidence level

Synthetic in-process test on the production `advance_race`. NOT done:
two-process proof of the results overlay on both sides, progress rows
for late finishers over the wire, graphics/audio/original content.

## Gates

fmt pass; clippy `-D warnings` clean; `cargo test --locked --workspace`
exit 0, no failures.

## Still open from report 6

Follow-up 2's two-process leg (overlay both sides), follow-up 3 (F26-A
replication of drawbridge/movers/parked cars/sounds), follow-up 1's
two-process leg. Status: implemented (candidate), not independently
checked.
