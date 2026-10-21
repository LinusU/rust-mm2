# Netplay fixed-tick timeline contract (NETPLAY-02)

Status: contract defined by NETPLAY-02A (#1773), wire codec in
`mm2_net::proto` at **protocol v31** (v31 adds the Cops & Robbers lobby bot count to the session advertisement `params`; v29 added the command window and
`input_ack`; v30 adds `SnapEntry.input_depth`). The host buffer (02B), client capture
(02C), lead controller (02D) and impairment matrix (02E) implement against
this page. Where a rule here is a *design choice* rather than a measurement
of the retail game, it says so; nothing below is an original-game rule.

## Selector

One switch stages every incomplete NETPLAY-04..08 behaviour: the
`mm2_app::netplay_timeline::NetplayTimeline` resource, set by the
`--netplay-timeline` flag or `MM2_NETPLAY_TIMELINE=1`. Default **off**; with
it off, sessions use the latest-only `Message::Input` path unchanged.
NETPLAY-09 turns the completed path on for ordinary host/join and retires
the switch. Do not add a second switch.

## Timeline

- Physics runs at the production fixed 1/60 s step (`FIXED_HZ`); it is never
  retuned. Wall time schedules whole ticks.
- **Command `C[n]` controls exactly one step `S[n-1] -> S[n]`.** `S[n]` is
  captured after the complete step: vehicle, Avian, custom contacts and
  post-step work.
- **Snapshot `server_tick = n`** (`Message::Snap.tick`) means state `S[n]`,
  the post-step boundary. Snapshots are state, never mid-step.
- Three counters stay distinct:
  - **seq** — the client's command counter, one per local fixed tick,
    starting at 1 per session generation; `InputWindow.first_seq + i`.
  - **predicted tick** — the client's own simulated tick (its lead over the
    server is `predicted - server_tick`, owned by 02D).
  - **server tick** — the canonical host tick. Remote state and commands are
    mapped through server ticks, never another peer's local frame number.
  The mapping seq -> server tick is established by the host when it consumes a
  command (02B): the command consumed for step `S[n-1] -> S[n]` is acked
  against tick `n`. A client keeps the (seq, predicted tick) pairs of its
  unacked commands; after fallback, catch-up or rebase it re-derives the
  pairing from acks, so seq stays valid when ticks are skipped or repeated.

## Controls: raw or processed

A `TickCommand` carries the **quantized pre-physics controls** the existing
`DriveInput` carried (throttle, brake, steer, handbrake, auto-reverse
preference, forced gear). Human steering smoothing, pedal swapping and
auto-reverse run **exactly once, on the simulating side, inside the vehicle
step** — in client prediction, in the host's simulation and in replay alike.
The quantization (u8 pedals, i8 steer) is applied once, at capture, and the
quantized value is what every path consumes, so prediction, host and replay
see identical numbers. Render rate (30/60/144 FPS) must not change which
commands exist: capture is per fixed tick, not per rendered frame.

## Acknowledgement

`SnapEntry.input_ack` (per seat) is the highest command seq whose slot is
**included in the completed state that entry was captured from**. It is
either (a) consumed by a step, or (b) explicitly finalized by the
missing-input policy below. A received-but-unconsumed command is never
acked. `0` means nothing finalized (and is what the host's own seat and bots
carry). Acks are monotonic per (seat, generation, epoch); a lower value than
already seen is stale and ignored.

## Missing input

The host advances at fixed dt regardless, consuming **one slot per seat per
fixed step** in `FixedUpdate`, ahead of the physics schedule (design choice,
implemented in 02B as `mm2_net::cmdbuf`). When a seat's next-seq command is
absent the tick is driven by a substitute: the last real command held for
`HOLD_LAST_TICKS` (6, 100 ms) consecutive substituted ticks, then neutral.
Two cases differ in what is *finalized*:

- **Gap** — a later seq is buffered, so the missing one is lost: the slot is
  finalized with the substitute and the ack advances over it. A late arrival
  for that seq is dropped as old and is never applied twice.
- **Starved** — nothing at or past the next seq is buffered, so lost cannot
  be told from late: the substitute drives the tick but **nothing is
  finalized and the ack holds**. The command is applied in order whenever it
  arrives, and a stalled peer cannot run the host's cursor ahead of the
  client's. (This refines the earlier draft, which finalized every empty
  slot.)

Every slot reports its `Source` (`Command`, `HeldLast`, `Neutral`) so a
fallback is never mistaken for driver input. The cursor starts at the first
command a seat is seen to send; the buffer holds at most `BUFFER_CAP` (32)
unconsumed commands and drops the excess as overflow.

## Window semantics (`Message::InputWindow`)

- `1..=MAX_INPUT_WINDOW` (16) consecutive commands, oldest first; command `i`
  has seq `first_seq + i`. The window is redundancy: each packet resends recent
  unacked commands so a short burst of loss costs nothing.
- Codec-level rejection (encode and decode): empty window, more than 16
  commands, `first_seq == 0`, `first_seq + len` overflowing `u64`, truncated
  body, non-0/1 booleans, trailing bytes. A peer sending such a frame is
  dropped as malformed. An old or new `Hello.protocol` is refused in the
  handshake with `VersionMismatch` naming the required version.

## Edge policy (host side, implemented in 02B)

Counters per seat (`SeatStats`): received, duplicates, old, overflow,
wrong_generation, stale_epoch, rate_limited, finalized, fallback, underflow,
gap_finalized, occupancy/max_occupancy, last/max command age.

| Case | Policy |
| --- | --- |
| Duplicate seq | Ignored. |
| Reordered | Gap-filling seqs within the unconsumed range are accepted by seq. |
| Old (already consumed/finalized) | Dropped, counted. |
| Future (beyond buffer bound) | Excess beyond the per-seat bound is dropped, counted; never moves another seat. |
| Generation change | The app tells the mailbox the running generation each fixed step; `generation` must equal it; a mismatch is dropped, never revives an old session. Per-seat state resets on a new generation. |
| Reset epoch change | Commands under a stale `epoch` are dropped; the buffer restarts at the new epoch. |
| Seq overflow | Unreachable in practice (u64 at 60 Hz); the codec still rejects windows that would wrap. |
| Start / countdown | Commands before the seat is simulating are dropped, not queued; consumption starts at the first simulated tick. |
| Escape capture | The client keeps emitting a command every tick; with input captured by UI it is neutral. Ticks are never skipped. |
| Host's own seat | Not a command stream; it feeds its own controls directly and carries `input_ack = 0`. |
| Disconnect | Seat buffer discarded; simulation continues on the documented fallback until the seat is removed. |
| Flood | A per-seat token bucket charged by window length: `COMMAND_RATE_HZ` 1920/s (2x a full window every tick), `COMMAND_BURST` 128. An unaffordable window is dropped whole and counted; other seats are untouched. |

## Client capture and send (02C, `mm2_app::netcmd`)

- `capture_and_send` runs in `FixedUpdate` after `input::apply_player_input`
  and captures exactly one `TickCommand` per fixed tick from the settled
  `VehicleInput` (quantized by `encode_input`, the same quantizer the legacy
  path and the host decode use). Render rate cannot change which commands
  exist. While `vehicle_input` holds the car neutral (menu, focus loss,
  countdown lock) the command is neutral; ticks are never skipped.
- `CommandHistory` keeps `HISTORY_CAP` (256) records `(seq, predicted tick,
  epoch, command)`, trimmed by `ack(seq)` (stale acks ignored) and reset on a
  new wire generation. Overflow evicts the oldest and is counted.
- Every `SEND_INTERVAL_TICKS` (2) ticks the newest up-to-16 commands sharing
  the newest reset epoch go out as one `InputWindow`.
- **Nonblocking contract:** systems never touch the socket or the writer
  mutex. They `try_send` into a depth-`HANDOFF_DEPTH` channel; a worker thread
  owns the blocking write. A full queue drops the window (counted) and a
  worker that fell behind sends only the newest queued window (counted as
  coalesced). Tested with a sink parked mid-write.
- Counters on `CommandStream`: `generated_slots`, `queued_slots`,
  `transmitted_slots` (written by the worker), `handoff_dropped`,
  `coalesced`, `send_errors`.
- The legacy `Message::Input` stream still runs beside it; with the selector
  on the host ignores it for controls (02B). Acks (`CommandHistory::ack`) are
  fed from `SnapEntry::input_ack` on the client by `netpace` (02F, with the
  selector on); the ring is also bounded by `HISTORY_CAP`.

## Lead controller (02D, `mm2_app::netlead`)

Design choices, not original rules; deterministic (wall nanoseconds are passed
in), so the tests drive it on a synthetic clock.

- **Signal: host buffer depth.** The host reports each seat's unconsumed
  command count in every snapshot entry (`SnapEntry.input_depth`, v30; `0`
  for the host seat and bots). `LeadController` smooths it (EWMA, 1/4) and
  steers it to `target_depth` = 2 ticks, with `min_depth` 1 / `max_depth` 6
  as emergency bounds far below `BUFFER_CAP` (32). Depth does not depend on
  path symmetry; `RTT / 2` (minimum filter + 4 mean deviations + explicit
  measurement uncertainty) only seeds `initial_lead_ticks` before the first
  report.
- **Actuator: whole ticks over wall time.** `Pacer::advance` turns a frame's
  wall time into a whole number of 1/60 s ticks; the tick length is never
  retuned. The controller's rate (ppm of wall time) is bounded to +-4% and
  slews at most 4%/s. A dead band (3/4 tick) around the target lets the rate
  decay to zero, so +-100 ppm drift does not hunt; outside it the rate is
  proportional to the error, and in an emergency it is at full authority.
- **Stale ack.** No ack advance for 500 ms stops the rate at zero (speeding
  up into a dead link only builds a backlog); the filter is reseeded from
  the first report after the ack moves.
- **Catch-up and rebase.** After a stall the pacer releases at most 4 ticks
  per call and keeps at most 8 ticks of backlog; the rest of the wall time is
  dropped and reported as `Rebase::Stall`. A frame of >= 1 s (sleep/resume)
  drops everything (`Rebase::Sleep`). Neither skips a seq (seq is per
  simulated tick), but `LeadController::rebase` clears the depth filter and
  the `SeqMap` (seq -> server tick = seq + base, base = `server_tick - ack`
  re-derived from every ack), so the pairing is never stale.
- **History size.** `history_ticks` = confirmation age + lead + snapshot
  interval + jitter margin + 1 must fit `HISTORY_CAP` (256).
- Tests (`netlead::tests`): a 1 ms loopback model with a FIFO uplink and a
  one-command-per-tick host covers startup, delay 50 -> 150 -> 50 ms,
  asymmetric 100/5 ms paths, +-100 ppm host and client drift, 100/150/250 ms
  stalls and a 30 s sleep, asserting depth bounds, settling time, zero host
  overflow and bounded underflow counts.
- **Wired (02F, `mm2_app::netpace`).** Behind the selector, a client with
  a command stream attached to a transport in a playing session stops using
  Bevy's fixed-time accumulator. Each frame `Pacer::advance` turns the
  *real* frame time (not `Time<Virtual>`, whose 250 ms clamp would hide a
  sleep/resume) plus `LeadController::update`'s rate into whole ticks, and
  the `FixedMain` schedule runs that many times at the unchanged `Time<Fixed>`
  step. Bevy's own `FixedMainLoop` set is skipped only while engaged, so
  selector off, a host and a single-player session keep the legacy path.
  `drive_lobby` records the local seat's `input_ack` / `input_depth` /
  server tick from each `Snap`; the next frame feeds them to
  `LeadController::observe` and trims `CommandHistory::ack`. A `Rebase`
  from the pacer calls `LeadController::rebase`; a new wire generation
  restarts controller and pacer. The `CommandStream` capture is unchanged.
  `initial_lead_ticks` (the RTT seed) is still unused: the client has no RTT
  measurement to give it; feedback alone steers.

## Impairment matrix (02E, `mm2_app` `tests/net_matrix.rs`)

Loopback only, through the seeded `ImpairProxy`; **no real-Internet claim**.
Each cell runs the production path: a client app under the real fixed
schedule (`capture_and_send` + worker handoff) -> proxy lane -> `HostLink`
mailbox -> `consume_seat_commands` -> `Snap.input_ack`/`input_depth` back
through the other lane into `CommandHistory::ack`. Cells: two independently
impaired seats (20 / 150 ms RTT), 20/80 and 80/20 ms splits, RTT 50 -> 150 ->
50 ms (the drop in delay makes later frames overtake earlier ones), 3-6
consecutive dropped input packets spanning a brake/steering press and
release, 90 dropped snapshots (return-path ack burst), a 15-tick renderer
stall caught up in one frame, a silent peer, and a session restart.

Every cell asserts, against what the host consumed: a Command tick finalizes
exactly the next seq and applies exactly that seq's quantized controls; one
slot per tick (`finalized + fallback`); the ack equals finalized + gap
finalized and converges on the client; no control sticks and a substitute
never outlives the hold; zero host overflow; host occupancy under a
per-cell bound well below `BUFFER_CAP`; client history under `HISTORY_CAP`;
stale (swapped) snapshot acks never move the history back. Counters printed
per cell (`--nocapture`): generated / queued / transmitted / coalesced /
handoff-dropped, host received / duplicate / old / overflow / finalized /
fallback / underflow / gap, occupancy, ack and depth feedback.

The 02E cells above step each client one fixed tick per host step (no
controller). 02F adds a paced rig (`Rig::paced`): the client's frame carries
the wall time (own clock error in ppm; time owed from a freeze), and the
pacer decides the ticks. Cells, all asserting zero host overflow, the
shared invariants and the depth band: steady 20 ms RTT settles within 300
ticks and holds the target +-1; 4- and 8-packet starved bursts (the
8-packet backlog peaks above `max_depth`) drain back inside the band within
600 ticks; +-100 ppm leaves the rate at (near) zero, no rebase and the
client within `max_depth` ticks of the wall; 100 / 150 / 250 ms stalls never
release more than `max_catchup_ticks` per frame, never run more ticks than
the wall time bought and account every missing tick to dropped wall time or
controller slowing (at most `max_adjust_ppm`); a 30 s sleep/resume releases no ticks, counts one sleep
rebase and recovers. Loopback only, no real-Internet claim. Still open:
prediction replay (NETPLAY-04), so no slot is "replayed".

The 02F cells are deterministic: a paced rig's proxies run on a virtual clock
(`ImpairProxy::loopback_manual`), so delay, jitter and the reorder hold are
measured against time that only moves in `ImpairProxy::advance`. Each host
step offers its frames at the current virtual time, advances one tick, and
waits on exact counts before it ends: windows the proxy read (per wire tag,
`ImpairProxy::tagged`) against windows the client's worker sent, `Snap`s read
against `NetDriveReport::snaps_sent`, then windows absorbed by the host
mailbox (`RemoteInputs::windows_received`) and `Snap`s taken by the client's
receiver thread against what the lanes wrote. Counting by tag keeps the
host's timer-driven keepalives, which the virtual clock does not schedule,
out of the barriers. Machine load therefore changes how long a step takes,
never what the controller sees: the per-cell occupancy, rate and depth
reports are byte-identical from run to run, and the cells assert the
original bounds (`max_occupancy <= LeadConfig::max_depth`, steady mean within
the target +-1) with no load allowance. A step still never runs faster than a
tick of wall time, because the host's flood bound refills in real time.
Delivery takes at least one step (a frame released at the advance reaches
the far side before the next step), so the cells' RTT is the lane delay plus
one tick each way. The 02E cells keep the wall-clock proxy.
With the selector on, the host's `apply_remote_inputs` stands down and
`consume_seat_commands` owns remote controls; the stalled-seat watch counts a
window as a sign of life.
