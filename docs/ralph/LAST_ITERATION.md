# Last iteration — parked-car agreement across peers (new-run iteration 4)

Baseline `cd7ad71`. Selection: report 6 follow-up 3, the one F26-A
question answerable without a wire change: do seed-rolled kerbside cars
agree across peers in networked races?

## Finding

Yes by construction: `spawn_parked_cars` rolls from a local
`ParkedRng::new(config.seed)`, the seed rides the advertised session
config to every client (`net.rs` config round trip test), and nothing
else consumes the stream. Test
`parked::peers_sharing_a_session_seed_roll_identical_bays` pins the pure
function of the seed.

## Evidence level / still open

Synthetic unit test only. Initial placement agrees; once a host-side
physics body is knocked, copies on clients diverge — replicating that
(plus drawbridge, movers, sounds) remains F26-A. Two-process legs for
follow-ups 1 and 2 remain open. Status: implemented (candidate), not
independently checked.
