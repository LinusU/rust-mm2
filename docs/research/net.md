# Networking foundation (F24-A)

Implementation-choice record for the engine-to-engine multiplayer
stack. None of this claims compatibility with the retail game's
DirectPlay sessions — per the project contract that interoperation is
out of scope.

## Authority model

Host-authoritative. One process owns simulation truth (`mm2_game`'s
`AuthorityRole::Authority`); clients send requests and consume state.
Clients never set authoritative positions, scores, damage or unlocks.
`mm2_net` itself carries no game rule types — that boundary stays in
`mm2_game`'s contract (`PlayerId`/`ObjectId`, stamped per session
generation); the wire layer exchanges opaque protocol fields only.

## Transport decision

**Control/session channel: TCP over `std::net`, blocking sockets.**

- The workspace has no async runtime and adding one (tokio + quinn, or
  an engine replication crate) is a large dependency move the contract
  reserves for deliberate decisions — none is needed yet: handshake and
  lobby traffic is low-rate request/response, which TCP serves exactly.
- Blocking sockets on a dedicated thread, bridged to Bevy through
  channels, keeps `mm2_net` free of engine dependencies — a headless
  host needs no window, GPU or audio device.
- Direct-IP/LAN scope first. `listen_loopback` binds `127.0.0.1` and
  tests never open anything wider; Internet reachability (NAT,
  firewalls, relays) is explicitly *not* claimed and needs its own
  design when the time comes. No listeners bind a public interface by
  default.

**Gameplay dataplane: deferred to F25.** Per-tick vehicle state wants
an unreliable/unordered channel (UDP candidate); that choice belongs
with the replication design, not here. Nothing in the control channel
precludes adding a second socket later.

**Authentication/encryption: deferred.** LAN-trust for now; when auth
is added it must come from maintained crypto/session crates, not
hand-rolled primitives.

## Wire protocol (`PROTOCOL_VERSION = 6`)

Length-prefixed frames: `u32le` length + payload, bounded by
`MAX_FRAME` (256 KiB) checked *before* allocation. Messages are strict
little-endian with `u16`-length-bounded strings; unknown tags, bad
truncation and trailing bytes are all hard errors. v1→v2: `RosterEntry`
gained the driver's `pick` and the `SetVehicle`/`VehicleRefused` pair
landed — an incompatible roster shape, so the version moved. v2→v3:
`Start`/`Cancel` (the session lifecycle pair) and
`RejectCode::SessionStarted` landed. v3→v4: `Input`/`Snap` — the
in-session driving transport (F25-A). v4→v5: `SnapEntry` gained
`epoch`, the authority's per-seat reset counter (F25-A.5). v5→v6:
`ResetRequest { generation }` — a client asking the authority to reset
its own seat (F25-B); the roster slot names the seat, so no target
field exists to forge.

Handshake (always the first exchange):

```
client → Hello { protocol, gameplay_fingerprint, build, driver }
host   → Accept | Reject { code, message }
```

The gate (`proto::admit`): exact `PROTOCOL_VERSION` match, and exact
`gameplay_fingerprint` match. Mismatches reject with a named reason —
version drift and content drift are distinguishable to the UI.

Blocking sockets need a bound or a peer that completes TCP and then
goes silent hangs the handshake forever. `send_hello`/`accept_hello`
install `HANDSHAKE_TIMEOUT` (10 s — designed, far above what two small
frames need) before any I/O and clear it once the session is
established, since the control channel may then idle between requests;
`*_within` variants take an explicit bound for callers and tests that
need a different one. The F24-B accept loop therefore never waits on a
single peer indefinitely.

## Content fingerprints

Two fingerprints, different jobs:

- **Catalog** (`mm2_assets::fingerprint::catalog`): FNV-1a-64 over
  resolved paths + provenance + winner file size. Cheap structural
  identity of "what would resolve"; not a byte hash. Shared with
  `mm2-inspect inventory`.
- **Gameplay** (`mm2_content::fingerprint::gameplay`): FNV-1a-64 over
  the resolved *bytes* of every gameplay-relevant logical path —
  `tune/`, `bound/`, `geometry/`, `race/`, `anim/`, `players/`, and
  `city/` except visual-only records (`.sky`, `.ldef`, `.lmap`,
  `.cpvs`, `.pvs`, `.pvshist`, `.ltNN`). Textures, audio and menu art
  never move it, so cosmetic mods cannot block a join; any edit the
  simulation could observe does.

  The classification is an implementation choice pinned to this
  protocol version — when a new consumer domain lands (e.g. pedestrian
  state affecting sim), its family joins the set and the fingerprint's
  meaning moves with `PROTOCOL_VERSION`.

## Lobby channel (F24-B.1)

`mm2_net::lobby` adds the host/join driver on top of the handshake:

```
client → Hello                     host   → Accept | Reject
       ← Welcome { player_id }            → Session { summary, params }?
       ← Roster { entries }               → SetReady { ready }
                                          → SetVehicle { vehicle, paint }
                                          → Leave
                                          → Input (absorbed, per-tick)
                                          → ResetRequest { generation }
                                            (absorbed, collapsed)
                                     host → Session | Roster (broadcast)
                                          → VehicleRefused { reason }
                                            (to the refused peer only)
                                          → Start { generation, session }
                                          → Cancel { generation }
```

`Session` is sent to a newcomer only when the host has advertised one,
always before its first `Roster` (see "Session advertisement" below).

- `Host` owns the listener and a single event loop; the loop is the
  only roster mutator. Three thread kinds feed it one channel: an
  accept thread (blocking `listener.accept`), a short-lived handshake
  thread per accepted conn (the gate runs under `HANDSHAKE_TIMEOUT`, so
  a stalled peer never blocks accepts), and a reader thread per
  admitted player forwarding `SetReady`/`SetVehicle`/`Leave` and socket
  death.
- The roster is a `BTreeMap<u16, Slot>`; ids mint monotonically from 1
  and a live slot's id is never reused — freed ids can be re-minted
  only after the `u16` counter wraps (~65k joins). `0` is reserved for
  the host player at the app layer. `Welcome` carries the assigned
  slot; `Roster` is a complete snapshot, not a delta, rebroadcast after
  every join/leave/ready/pick change — receivers replace wholesale, so
  no ordering hazards. Each entry carries the driver's `pick`
  (`Option<VehiclePick>`) — see "Vehicle picks" below.
- Bounds: `MAX_PLAYERS = 8` on the wire roster (MP-1's documented
  TCP/IP ceiling) and on the encode/decode of `Roster`; `max_clients`
  in `HostConfig` is the runtime seat count (default 8 — a host that is
  itself a player should pass 7) and a value above `MAX_PLAYERS` is
  rejected at listen as `NetError::Config` — the wire roster cannot
  represent it; `MAX_PENDING` caps in-flight handshakes so a connect
  flood drops at the accept boundary instead of spawning unbounded
  threads. A full roster rejects with `RejectCode::LobbyFull` *before*
  `Accept`, so a refused client sees a named reason through the normal
  handshake verdict.
- Post-handshake discipline: a client may send only
  `SetReady`/`SetVehicle`/`Leave`; anything else drops it as
  `LeaveCause::Malformed`. A dead socket is
  `Lost`; `Leave` is `Quit` — `Client::leave` half-closes and drains so
  a quit isn't RST'd into looking like a drop. Established-lobby writes
  carry `WRITE_TIMEOUT` (10 s) so a peer that stops reading is dropped
  rather than freezing a broadcast. Every removal — quit, drop or a
  failed broadcast write — disconnects the peer socket, so the blocked
  reader thread exits and the client observes the close instead of
  sitting on a dead-but-open connection.
- `Host::shutdown`/`Drop` closes every peer socket (waking the reader
  threads), self-connects to wake the blocking accept, and joins the
  loop. Clients observe the lobby's death as a failed `recv`.

### Session advertisement

*Implementation choice.* `Message::Session` carries a
`SessionAdvertisement { summary, params }`: a bounded display line plus a
bounded opaque blob (`MAX_SESSION_PARAMS = 4 KiB`). `mm2_net` deliberately
does not interpret it — the transport ships the blob; the owner that
built the session (today `mm2_app`, see `mm2_app::net`) defines its
encoding. The host owns the advertisement: `Host::set_session` replaces
it and rebroadcasts to every connected peer; a newcomer gets
Welcome → Session (if set) → Roster, so its first roster never precedes
the session it belongs to. A peer whose session write fails is
disconnected and the corrected roster is rebroadcast. A client sending
`Session` is out-of-turn and dropped like any other host-only message.
The advertisement is informational only — a peer already in the lobby is
not re-validated against a new session's settings; the handshake
fingerprint remains the only compatibility gate.

### Vehicle picks

*Implementation choice.* `Message::SetVehicle { vehicle, paint }` is the
client→host pick request; the pick lands in `RosterEntry::pick` and rides
every roster snapshot. `vehicle` is opaque to the wire — `mm2_app`
carries catalog ids (`vpbug`) and uses the empty string for the
synthetic dev car (`VehicleSelection::id = None`).

Validation runs on the authoritative side: `HostConfig::pick_validator`
is a consumer-supplied gate (`mm2_app::net::vehicle_validator` builds it
from the mounted `VehicleCatalog` — exact lowercase catalog id, the
entry must be `Ready`, paint bounded by the metadata `Colors` list with
a one-job floor; the dev car is always legal at paint 0). A refused
pick gets `Message::VehicleRefused { reason }` back to that peer alone —
a bad pick is a refused request, not a protocol violation, and the
roster is unchanged. The validator's reason is consumer text carried in
a `MAX_STRING` field, so the host shortens an over-long reason on a char
boundary before the send — an id-echoing validator fed a long wire-legal
id must not fail the encode and read as a dead socket, which would turn
a refused pick into a dropped peer. A host with no validator accepts any
bounded pick.
A pick identical to the slot's current one is a no-op: no event, no
broadcast — a repeating client cannot flood the lobby with rosters
(this is pick-side only; the disclosed `SetReady` rate-limit gap is
unchanged F24-C/AC03 scope). The gameplay-fingerprint gate means every
peer's catalog is identical, so a pick the host accepts is spawnable
everywhere.

### Session lifecycle (start/cancel)

*MP-5 (documented — `help:Multiplayer Games`) is the original-rules
anchor:* race joiners must be in before the host starts; Cruise and
Cops & Robbers allow join/leave at any time; a leaver's vehicle
disappears for everyone. MP-8 (documented — `help:Multiplayer Lobby
Screen`) gives the host the start control. What the original does not
evidence is the exact refusal conditions of a blocked start — the gate
below is a *designed* policy, recorded as such.

- `Host::start(late_join)` / `HostCtl::start` is the consumer's
  asynchronous request; the verdict arrives as `HostEvent::Started` or
  `HostEvent::StartRefused { reason }`. The gate, in order: the lobby
  must not already be in-session, a session must have been advertised
  (`Start` carries it — there is nothing to start otherwise), and
  every *connected* player must be `ready` and have a pick — a driver
  cannot spawn into a session with no committed car. An empty wire
  roster passes: a remote client's state cannot gate a host that is
  itself the only player (the app-layer host player, wire id 0, is not
  on the roster). The first blocker names the refusal reason
  (`"alice is not ready"`).
- `Start { generation, session }` is host-authoritative and
  self-contained: `generation` mints from 1 and climbs monotonically
  for the lobby's lifetime — the value `mm2_game`'s
  `Session::generation` / `ObjectId::generation` namespace to — and
  `session` is the *running* session, snapshotted at start so a
  mid-session `set_session` re-advertisement (the next round's config)
  can never rewrite what the running one is.
- `late_join` is the per-start join policy — `LateJoin::Closed` (the
  MP-5 race rule: joins get `RejectCode::SessionStarted` before
  `Accept`, like a full lobby) or `LateJoin::Open` (the MP-5 cruise
  rule: a newcomer is admitted normally, gets Welcome → Session →
  Roster, then a unicast `Start` with the *running* session and
  generation so it enters the session the rest already play).
- The roster stays the shared truth through a session: departures,
  readiness and pick changes still apply and rebroadcast (MP-5's
  "a leaver's vehicle disappears for everyone" at the lobby level; an
  open session's late joiner picks its car after joining).
- `Host::cancel` / `HostCtl::cancel` ends the in-session phase:
  `Cancel { generation }` broadcasts, readiness resets (designed — a
  fresh start wants fresh consent; picks are kept), joins re-open, and
  `HostEvent::Cancelled` confirms. Outside a session it is a no-op.
- `Host::ctl()` hands out `HostCtl` — a cloneable `Send`/`Sync`
  handle driving `start`/`cancel`/`shutdown` from a thread other than
  the event-draining one (`Host` is `!Sync`), which is how `mm2-host`'s
  stdin reader reaches the loop.
- Client-sent `Start`/`Cancel` are out-of-turn and drop the peer
  `Malformed`, like every other host-only message.

### Dedicated host

`mm2-host` (a second `mm2_app` binary) is the first consumer: a headless
host process that mounts the VFS read-only (`--mm2-path` is required —
an empty directory is a valid content-free mount; `--mods` adds a mod
directory), computes the gameplay fingerprint, binds `--bind` (default
`127.0.0.1:0` — loopback with an ephemeral port; the chosen address is
printed so a harness can dial it), advertises a `SessionConfig`
built from `--dev-world` *or* `--city` (the flags conflict; the default
city is `london` and one whose `city/<name>.psdl` does not resolve
through the mounted VFS is refused), `--pro` (Professional instead of
Amateur), `--weather`/`--time-of-day` (0–3 selectors) and `--seed`
(clock-derived when omitted), validates client vehicle picks against
the mounted `VehicleCatalog`, and prints one
`listening=<addr> fingerprint=… seed=… session="…"` record followed by
one `event=` line per lobby event for harness consumption
(`joined`/`left`/`ready`/`vehicle`/`pick_refused`/`join_failed`/
`started`/`start_refused`/`cancelled`). No
window, audio or GPU is required — that is the F24-AC05 binary leg,
exercised so far only on loopback. Named sessions have no flag yet.

`--event <table>:<row>` hosts an authored event instead of cruise — the
same selector grammar `mm2 --event` and `mm2-inspect` share, now owned
by `mm2_game::EventRef::parse`/`EventTableKind::parse_token`. The gate
is the same path a session load takes (`mm2_app::race::event_race_setup`:
catalog scan → dependency-checked `EventCatalog::resolve` →
`race_definition` build): an unknown row, missing records or a
definition that fails to build exit 2 at flag time, so the host never
advertises a session it cannot run. Crash Course rows refuse through
`RaceBuildError::CrashCourseUnsupported` exactly as `mm2` refuses them
(F21). *Designed:* advertising is limited to what the shared loader can
build; a lobby-level check cannot prove the per-client render path.

The operator's control surface is stdin: `start`, `cancel`, `quit`,
one per line. `start`'s late-join policy is the session mode's (MP-5):
an event lobby starts `LateJoin::Closed` — the first shipped consumer
of the race rule — and a cruise lobby `LateJoin::Open`. A
closed stdin means unattended operation, not shutdown; `quit` is the
clean-exit command (`HostCtl::shutdown`, exit 0 after the loop ends).

### Dedicated client

`mm2-join` (a third `mm2_app` binary) is the lobby's client-side
counterpart: a headless process that mounts the VFS read-only with the
same policy as `mm2-host` (`--mm2-path` required, `--mods` optional),
computes the gameplay fingerprint, joins `--connect` and prints one
`connected=<addr> id=<n> driver="…" fingerprint=…` record followed by
`event=` lines per lobby transition (`roster`/`pick_refused`/`started`/
`cancelled`/`join_failed`/`session_refused`/`closed`). stdin drives
`vehicle <id>[:<paint>]`/`ready`/`unready`/`quit`; `--vehicle` and
`--ready` do the same once at startup. The send side reaches the
blocked `recv` through `Client::ctl()` — a cloneable handle mirroring
`HostCtl` that serializes `SetReady`/`SetVehicle`/`Leave` on a shared
`Mutex<Writer>` (wire frames are two `write_all` calls; unsynchronized
senders would interleave them). No window, GPU or audio is touched.

`mm2-join` is also the first consumer of the **client-side session
gate**, `mm2_app::net::check_session`: every `Session` advertisement
and the running session inside `Start` is decoded (`net::accept`) and
then proven runnable against *this* install — a `City` world psdl must
resolve, an `Event` must survive the same `race::event_race_setup`
path the host's flag-time gate runs, and a `race` customization pick
is bounded where it applies (`laps ≤ CUSTOMIZE_LAP_MAX` on `Ordered`
rules, `opponents ≤` the authored roster). What the gate deliberately
does not recheck: bounds `accept`'s structural decode +
`SessionConfig::validate` already cover, and picks the runtime ignores
(`race` on Cruise, `laps` on a non-`Ordered` rule). An unrunnable
session gets `event=session_refused`, a clean `Leave` and exit 1 — a
client never holds a lobby seat for a session it cannot spawn. The
fingerprint handshake means an honest host shares this content
already; the gate is defense-in-depth for one that does not. `quit`
exits 0; a refused join or a lost host exits 1; usage/mount failures
exit 2 — the same code shape as `mm2-host`.

Deliberately *not* here: a started session is reported, not spawned —
world/race replication is F26 and the per-tick dataplane is F25 — plus
host migration and the Bevy-side host/client surface.

### In-app client bridge (F24-B.7)

`mm2 --join <addr>` joins a lobby inside the real application —
windowed or `--headless`. `mm2_app::net::LobbyLink` owns the
connection: `Client::recv` blocks, so it lives on a pump thread that
forwards each inbound frame as a `LobbyEvent` on a channel, ending
with a terminal `Closed`. The Bevy thread never touches the socket;
outbound intents (`SetReady`, `SetVehicle`, `Leave`) ride the same
`ClientCtl` `mm2-join` uses. `drive_lobby` drains
the channel once per update into `LobbyState` (roster, advertised
session, running generation, latest notice, a parked start, a queued
exit) — display/decision state only; the `Session` resource stays the
single gameplay authority.

A host `Start` is the point: the advertisement is decoded and gated by
the same `accept` + `check_session` pair `mm2-join` runs, the accepted
config is stamped `Remote` authority with this process's local facts
(roster-echoed vehicle pick, `mods_active`, launch-time dev overrides —
none of which ever ride the wire), and `Session::begin_generation`
adopts the lobby's minted generation so `ObjectId`/`ResultId`
generation fields agree across peers. The wire value may only move
the local counter forward — staleness detection on generation-keyed
ids assumes it never regresses. A `Start` drained while a session is
still live parks in `pending_start` and begins when teardown lands
back at `Menu`; a `Cancel` matching the running generation quits the
session through the normal `Unloading → Menu` path. The accepted
session loads through `load_session_world` — the shared world/spawn
path — never a parallel multiplayer loader.

Exit ownership follows the surface: with a `LobbyLink` present,
`drive_session`'s `Menu` quit arm does not write `AppExit` (a quit
returns to the lobby), and `drive_lobby` writes the eventual exit —
`0` after our own `Leave` is acknowledged (bounded by a five-second
watchdog against a host that never closes), `1` on a lost host or a
refused session. The stock `F4` restart binding is `Local`-authority
only — a remote session's restarts belong to the host's
`Cancel`/`Start` pair. A networked-authority session is also
record-ineligible (`Ineligible::Networked`): a local prediction must
not mint single-player unlocks.

The lobby surface is deliberately minimal: `Enter` toggles ready,
`Esc` leaves, and a `LobbyText` status line shows the advertised
session, roster readiness and the latest notice. A real lobby menu
(pick/browse screens) is future work, as is every piece still absent
underneath: remote vehicle spawning, roster picks as gameplay spawns,
replication of position/score/damage (F25/F26), and the in-app *host*
surface. `mm2 --join --headless` parks the same link inside the smoke
harness, which waits on the wire with wall-clock pacing while `Menu`
is parked and reports the lobby's progress as `mp=` on the record
(`mp=gen<N>` once a `Start` minted the session, `mp=lobby(<n>p)`
while waiting, `mp=lobby(0p)` after the link dies).

## Impairment harness (F25-B)

*Implementation choice.* `mm2_net::impair::ImpairProxy` is a framed TCP
relay a test inserts between a peer and a host: the peer dials the
proxy's loopback address, the proxy dials the real target, and every
frame crosses a per-connection, per-direction *lane* that applies an
`Impair` recipe — `delay` (a release instant per frame), `jitter`
(uniform extra release, so jitter past a neighbour's release is a real
reorder), `loss` (whole-frame drop), `duplicate` (an adjacent second
copy) and `reorder` (a frame defers to its successor — the pair swaps;
a successor that never comes emits at a bounded `HOLD_CAP`). Decisions
come from a per-lane SplitMix64 stream seeded at construction, so a
fixed seed replays an identical impairment pattern over identical
traffic; `set` retunes a live direction so a scenario handshakes clean
and impairs only the data plane. `LinkStats` reports what the recipe
actually did (frames in/out, dropped, duplicated, reordered,
overflowed) — a leg asserts the impairment *happened*, not just that
the session coped. Pending frames are bounded (`MAX_QUEUED` per lane,
excess dropped and counted), the relay never decodes payloads (present
and future messages alike), and `Drop` shuts every relayed socket and
joins every thread.

The harness is what makes the mailbox's ordering rules load-bearing.
Ordered TCP cannot reorder or duplicate in practice, so both guards
exist for the impaired legs and any future unordered transport: an
`Input` only displaces the stored sample when its sender `seq` is
strictly ahead, and a `ResetRequest` mailbox keeps the *highest*
generation per slot — a reordered stale ask can never mask a fresher
one.

## Two-process driving (F25-B)

`crates/mm2_app/tests/net_drive.rs` is the data plane's first
evidence above the in-process level: three real `mm2` OS processes —
one `--host --headless` (the authority, playing seat 0) and two
`--join --headless --ready` clients — driving one dev-world cruise
over real loopback sockets. Each process's `smoke=headless-physics`
record carries the `net=` counters, so the assertions read what the
wire actually moved rather than an in-process mailbox's contents.

- **Clean leg** (`two_mm2_processes_drive_one_session_over_loopback`):
  each client's mid-session record shows inputs streamed up
  (`in<N>s`), authoritative snapshots applied (`snap<N>a`), the other
  participants reconciled as remote copies (`rem2` while both peers
  are still connected) and its own predicted seat driven (`moved=`).
  A client's frame-cap exit lands on the host as `cause=quit` — the
  link `Drop` sends a deliberate `Leave`, not a lost socket — and the
  host's `quit` record carries the authority side: remote inputs
  applied to their seats (`in<N>a`) and snapshots broadcast
  (`snap<N>s`).
- **Impaired leg**
  (`an_impaired_two_process_session_still_converges`): the same
  session with every client connection relayed through one
  `ImpairProxy`. The lobby phase crosses clean (recipes are armed
  only after the `ready` echoes prove the verbs delivered, and the
  downstream recipe arms after `Start` has landed — a one-shot verb
  has no retransmit), then the whole driving phase — `Input`s up,
  `Snap`s down — rides a seeded recipe of 40 ms delay + 30 ms jitter
  + 5% loss + 10% duplication + 10% pairwise reorder in both
  directions. Both clients still converge: predicted driving keeps
  the seat moving while stale, duplicated and reordered frames drop
  at the mailbox instead of wedging the session. `LinkStats` asserts
  the recipe genuinely fired in each direction.

Scope stays honest: this is loopback on a synthetic dev world — no
LAN leg, no rendered observation, no retail install. One recipe over
a live session is not the recorded delay/jitter/loss matrix
F25-AC03 names; that grid remains open.

## Evidence level

Mixed loopback. `mm2_net` tests bind `127.0.0.1:0` and run real
client/server socket pairs in one process — handshake
accept/version/content/malformed/oversize/truncation/silent-peer legs
plus lobby legs: slot assignment, multi-client roster broadcast, ready
rebroadcast, quit-vs-drop causes, fresh ids on rejoin, lobby-full and
incompatible-peer rejection, out-of-turn-message drops, handshake-flood
bounding, session ordering/rebroadcast, host-shutdown disconnect, and
the pick legs — validator-applied pick riding the roster, refusal
reaching only the picker with the roster untouched, identical-pick
no-op, validator-less acceptance and host-only-message drops —
plus the lifecycle legs: gated start (named refusals for unready/
unpicked/missing-session/already-running, empty-roster pass), `Start`
carrying generation + session, `Closed` refusing a join with
`SessionStarted`, `Open` handing a late joiner the running `Start`
(including the running-vs-readvertised session distinction),
`Cancel` re-opening the lobby with readiness reset and a fresh
generation on the next start, mid-session roster liveness,
client-sent lifecycle messages dropping `Malformed`, and the
`HostCtl` cross-thread driver. The impairment harness legs add the
lane unit checks (delay holds, full-loss drops, duplication,
pairwise reorder, held-frame deadline, queue overflow, per-seed
determinism), real-socket relay legs (upstream/downstream recipes,
runtime retune, stats aggregation across connections), and a lobby
leg that replays a reordered input stream into the real mailbox and
finds the freshest `seq` still held.
`mm2_app`'s `net_host` test additionally runs the `mm2-host` binary as a
separate OS process with two in-process clients joining it, picking and
being refused — partial F24-AC01/AC05 evidence (separate host process,
configured bind, no window/audio; clients still share the test
process). Its second leg drives stdin `start`/`cancel`/`quit`: a gated
refusal, a generation-1 start with the session decoding back through
`net::accept`, a mid-session joiner receiving the running `Start`
(open-policy MP-5 cruise), a cancel re-opening the lobby into a
generation-2 start, and `quit` exiting the process cleanly. The third
leg hosts a synthetic authored event (`--city testcity --event race:0`
over a fixture install): the advertised session decodes back to the
`SessionMode::Event` config, a started lobby refuses a late joiner
`SessionStarted` (MP-5's race rule through the real consumer), and
`cancel` re-opens joins; the fourth runs the flag-time refusals —
malformed selector, out-of-range row and a resolve-Ready-but-unbuildable
event (`NumLaps 0`) all exit 2. A retail leg is on record for the
event gate: `mm2-host --city sf --event race:0` against the retail
installation resolves and builds through `event_race_setup` and
advertises `sf, race:0, amateur`; `--event crash:0` refuses
`CrashCourseUnsupported` at flag time and `--event race:99` refuses
the missing row — both exit 2.
`mm2_app`'s `net_join` suite adds the client side: two separate
`mm2-join` OS processes against a separate `mm2-host` pick vehicles,
ready, observe `Start` and leave cleanly (the first *separate client
process* evidence for AC01 — still loopback); a fingerprint-mismatched
join reports `join_failed`/exit 1 on the client and `join_failed` on
the host; an in-process host feeding advertisements a client's fixture
cannot run (dev-world event, unresolvable city) produces
`session_refused`/exit 1 and a clean `quit` leave; and the `race:0`
fixture event round-trips through a real `mm2-host` + `mm2-join` pair
into `started`. Direct `check_session` legs cover the accept/refuse
matrix: runnable dev-world/event sessions pass; missing world, out-of-
and unbuildable-table events refuse; `laps`/`opponents` bounds refuse
where the pick applies and pass where the runtime ignores it. A retail
leg is on record for the pair: `mm2-host --city sf --event race:0` +
`mm2-join --vehicle vpbug --ready` on the retail installation — stock
pick validated, `started generation=1 session="sf, race:0, amateur"`
observed client-side, `cause=quit` leave, both processes exit 0.
`mm2_app`'s `net_app` suite covers the in-app bridge: in-process legs
run `drive_lobby`/`drive_session` against a real loopback `Host`
(join broadcast surfacing the ad/roster at `Menu`, `Start` →
`begin_generation` → `Loading` under the minted generation with
`Remote` authority, `Cancel` returning through `Unloading → Menu`
with no `AppExit`, a `Start` mid-session parking until teardown
lands, a dead host tearing down to a nonzero exit with a named
notice, a `Leave` reaching the host as `Quit` and exiting 0, a
`Menu` quit staying inside the lobby, an unrunnable ad refusing with
a clean leave, the `F4` local-authority gate, and the generation
clamp); `an_impaired_link_still_converges_the_data_plane` runs the
whole session through a live `ImpairProxy` — the lobby handshake
crosses clean, then `Input`s, `Snap`s and a `ResetRequest` all ride a
seeded delay/jitter/duplicate/reorder recipe, and the hosted car
still drives on the freshest `seq` while the ask is granted exactly
once; `headless_lobby` joins an in-process host and proves the
`Start` → `load_session_world` path loads the wired world for real;
and the process legs run `mm2 --join --headless` against a separate
`mm2-host` — `start` produces `world=dev-world mp=gen1 status=pass`,
`cancel` produces `mp=lobby(1p) phase=menu`, a host quit produces
`lost the host`/`status=fail` exit 3, a refused connection exits 1,
and `--join` × session-shaping flags are usage exit 2.
`mm2_app`'s `net_drive` suite is the first two-process *driving*
evidence: a separate `mm2 --host --headless` authority plus two
separate `mm2 --join --headless` clients drive one dev-world cruise —
cleanly, then with every client connection relayed through an armed
`ImpairProxy` (see "Two-process driving" above) — while each
process's record counters prove inputs flowed up, snapshots flowed
down and remote copies spawned. This
is *not* LAN or Internet evidence — every socket so far is
`127.0.0.1`, the world is synthetic, and nothing rendered — F24-C owns
the reachability matrix.
