# Last iteration — world-actor clock audit across peers (new-run iteration 5)

Baseline `d52fc0e`. Selection: report 6 follow-up 3, the remaining
half — do drawbridge leaves and movers (boats, ferries, Underground
trains) agree across peers? Audit only; no production change (a fix
needs a wire or clock-seek design, not a one-iteration patch).

## Finding (read from source, not run)

- `drive_drawbridges` / `drive_movers` step a *local* accumulator by
  `Time<Fixed>` delta while the session is `Countdown`/`Playing`/
  `Results`. Their comments say "deterministic from session start", true
  per process only.
- Nothing in `mm2_net` or `netdrive.rs` carries a world clock, leaf
  phase or mover position (grep: no mover/drawbridge/train references in
  the net layer). A client's countdown and race clock are overwritten
  from `Snap.race`, but the movers keep their own count from the moment
  the client's session entered `Countdown`.
- So timed leaves and every mover are only phase-aligned with the host
  to within the peers' session-start skew, and drift if either side
  stalls. Proximity leaves are worse: triggered from local `Player`
  positions, so a leaf may open on one peer only.
- Parked cars (previous iteration) are the one actor that does agree.

## Proposed fix (open, recorded in PLAN F26-A)

Derive world time from the host's race clock (already on the wire in
`Snap.race`): give `Follower`/`TrainMotion`/`LeafMotion` a seek-to-time,
and have clients re-seek on each fresher race row. Proximity triggers
become host-authoritative (a leaf-open bit or the leaf timer in the
snapshot). Needs a two-process leg.

Status: finding recorded, nothing implemented. Gates not rerun (docs only).
