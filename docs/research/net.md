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

## Wire protocol (`PROTOCOL_VERSION = 10`)

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
field exists to forge. v6→v7: `SnapEntry` gained a presentation tail
(F25-B) — `steer` (i16, milliradians, saturating), `spin` (i16, 0.1
rad/s, the mean rate of the grounded wheels), `compression` (u8, the
mean suspension-droop fraction ×255) and `flags` (u8: brake, reverse,
grounded). Pose/velocity were already replicated but a remote copy's
`VehicleState`/`VehicleInput` were dead — wheels stayed at full droop,
never steered or spun, and the brake/reverse glows never lit. The tail
is informational: it drives `car_visual`'s wheel/glow systems on the
copy, never authority pose; application clamps it to the pick's
steer/travel bounds, so a hostile wire value cannot write unbounded
visual state. A client-side remote copy also carries
`mm2_vehicle::RemoteReplica`, which excludes it from the local
`vehicle_simulation` — without it the sim's own dead-input step would
overwrite every replicated field between snapshots (the host's
authoritative seats keep simulating; the marker exists only on
predicted-session copies). Between snapshots `RemoteDrive::spin_rate`
integrates the replicated rate into `WheelState::spin`, so wheels
keep turning at ~20 Hz updates instead of stepping. v7→v8:
`SnapEntry` gained `damage` (u8, F25-B) — the authority's
`VehicleDamage` total as a fraction of the seat's authored
`MaxDamage`, ×255; `0` for intact and for a seat with no authored
`vehcardamage` record (undamageable reads as undamaged, never a
fabricated spec), `255` at/over the bound. On a predicted client the
byte writes through `VehicleDamage::set_replicated` onto every named
seat — remote copies *and* the client's own (the only own-seat field
a snap applies, since nothing local accumulates under prediction) —
reconstituting the total through the copy's own spec without touching
the `ImpactId` watermark, so authority repairs replicate down while a
late duplicate impact still cannot land. It is state replication, not
impact replication: the byte carries no per-impact positions (v10
later added those, below); the
authored `VehicleSmoke` rig now binds at remote spawn and emits off
the replicated total, and `sync_impairment` runs under predicted
sessions so the client's own seat weakens the way the authority's
copy of it does. v8→v9: `Snap` gained `trailers`, a bounded
(`MAX_PLAYERS`) list of `SnapTrailer` rows — pose, velocities, wheel
rate and a grounded bit for each trailered seat's trailer, keyed by
the *towing seat's* wire id rather than a fresh identity. The
authority's `publish_snapshots` emits a row per `Trailer` relation
whose `towing` is a `NetPlayer` seat; on the client a remote trailer
copy blends and spins exactly like its seat (the same `RemoteLerp`/
`RemoteDrive` rig, `RemoteReplica`-excluded from the local sim) and
snaps on its owner's reset epoch — trailers carry no epoch of their
own, they ride the seat's. The predicted client's *own* trailer is
the exception that proves the rule: it is a real jointed body the
local hitch owns, so like the own seat's epoch-equal entries the
authority's lagged view is dropped — only a declared reset (the
epoch-advance row) or a real divergence reseats it. The reseat side
of that contract is the generalized `reseat_towed_trailers`
`ResetVehicle` follower: every tractor reset the stream carries —
the `R` bundle, `--reset-at`, recovery/stuck/disabled resolves,
scripted and opponent re-anchors, the self-right assist, wire
`ResetRequest`s — emits each towed trailer's reseat ahead of the
apply, so a remote participant's rig teleports as one on the
authority exactly like the local `R` bundle. The copy-side contract
matches the seat's: the kinematic trailer carries `DamageSignals`
like the authority's real body and the local trailer (client-side
impact-signal consumers see its contacts), its hitch joint rides as a
`ChildOf` the trailer entity on every role so a leave/re-pick despawn
sweeps it instead of orphaning it on dead bodies, and the row's
grounded bit — the only suspension truth the trailer tail carries —
folds into the copy's `VehicleState`: grounded settles each wheel at
its authored rest sag (`HandlingMetrics`, the same number the local
sim settles to), a clear bit hangs it at full droop. v9→v10: `Snap` gained
`impacts`, a bounded (`MAX_SNAP_IMPACTS` = 64) list of `SnapImpact`
rows replicating the authority's filtered `ImpactEvent` stream —
per-impact `point`, outward `normal`, `severity` and the authority's
`ImpactId` that the v8 damage *fraction* cannot express (F25-B).
`publish_snapshots` drains the stream every run, filters to the
current generation, and emits one row per participant side that maps
to a `NetPlayer` seat — a car-vs-car hit rides as two rows sharing
`id` with mirrored outward normals — sorted strongest-first at the
cap, so a pile-up keeps its worst hits. The rows are presentation
*events*, not state: poses stay latest-wins but impacts ride a
separate bounded pending queue, so a superseded frame's effects are
not silently lost with it. The client dedupes on `(generation, seat,
id)` (bounded FIFO window), drops stale/unspawned/non-finite rows,
skips the receiver's own seat — its predicted physics already
rendered the hit through the local stream — and mints
`netdrive::RemoteImpact` messages resolved to the live remote copy.
`spark_fx::emit_sparks`, `audio::impact_voices` and
`texel_fx::apply_remote_texels` now read both streams, and on the
authority a remote-controlled seat's local-stream impacts render like
an AI car's (it is a locally simulated participant there). The texel
leg (F25-B, same iteration): `spawn_remote` binds a `TexelDamageRig`
on the authored `vehcardamage` gate exactly like a local pick, seeded
`generation | wire` the way the smoke/spark rigs are. On the
authority the seat's local `ImpactEvent`s splat it through
`apply_texel_damage` — whose `Remote` skip narrows to predicted
sessions only, since nothing echoes a wire row back at the process
that authored the hit. On a predicted client a `RemoteImpact`'s
world point converts through the copy's `GlobalTransform` into the
car space `rig.apply` wants; a remote copy's local-stream hits stay
skipped there so nothing double-stamps. Repairs replicate through
the v8 damage byte alone: `apply_snapshots`' `apply_damage` sees the
`>0→0` transition — the authority's `resolve_disabled`
`damage.reset()` + `texel.reset()` pair arriving as state — and runs
`TexelRepair::reset`, so repeated clean bytes never re-blit a splat
the accumulator missed. Byte `0` is reserved for that signal:
`encode_damage` floors any positive total to byte 1 rather than
letting `round` mint a repair the authority never performed. The two
halves of `apply_snapshots` order against each other through the
per-seat `repaired` ledger — the state pass runs first and records
each transition's `(generation, snap tick)`; the pending-impact drain
then drops any row whose emit tick sits at or below it
(`SnapImpact::tick` is the host tick the `ImpactEvent` was emitted,
never after its snap's publish tick, so such a row is provably
pre-repair). A hit the authority wiped therefore drops instead of
splatting *after* the wipe — the authority ordered splat-then-wipe —
whether it rode a superseded frame or arrived late on a reordered
one. Two presentation-only edges remain by construction: a hit
emitted in the same host tick the repair resolved reads as pre-repair
and may drop a legitimate post-repair splat, and a pre-repair hit
whose *damaged* intermediate byte was superseded before it ever
applied is unobservable — the client never saw a transition, so the
row is indistinguishable from a fresh hit on an intact car (a
per-seat repair epoch on the row would close it if it ever matters).
What the wire still does not carry: the
struck side's identity (a replicated row's audio picks the id-0
catch-all) and `surface` (no consumer reads it). v10→v11:
`SnapEntry` gained `breaks` (u32, F25-B) — the seat's
detached-breakaway-part bitmask, bit *i* = `VehicleBreaks` part *i*
in authored order. Authored order is identical on every process
(the gameplay fingerprint gates the handshake), so the mask needs
no names. It is replicated *state* like the v8 damage byte, not an
event: `apply_snapshots` diffs it against every named seat's rig —
own seat included, since a predicted client never runs
`detach_breaks` — and a set bit hides the part's intact node and
spawns its `BangerPool` fragment through the same
`spawn_break_fragment` helper the authority's `detach_breaks`
uses; a cleared bit re-attaches and despawns the fragment — the
authority's `resolve_disabled` repair arriving as state, so a
dropped snap or a late join can never leave a copy's rig diverged.
Two designed deltas from the authority path: the wire carries the
detach *state* but not the per-part launch impulse, so a copy's
fragment inherits the replicated motion and tumbles on its own;
and no `PartDetached` message fires client-side (nothing consumes
it there today — breakaway audio on remote copies stays a named
gap). Parts past bit 31 are unexpressible — far past any authored
count (the retail roster tops out at single digits). On the
authority side of the same change, `detach_breaks` stopped
skipping `PlayerControl::Remote` seats: a remote driver's rig on
the host *is* this authority's participant — simulated like an
AI's — so its parts shed there and the mask publishes with its
entry. `spawn_remote` now binds `VehicleBreaks` on both roles off
the authored inventory, the same absence gate as a local pick.

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
  for *that host's* lifetime — the value `mm2_game`'s
  `Session::wire_generation` tracks (see "Generation namespaces"
  below) — and `session` is the *running* session, snapshotted at
  start so a mid-session `set_session` re-advertisement (the next
  round's config) can never rewrite what the running one is.
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
adopts the lobby's minted generation. A `Start` drained while a
session is still live parks in `pending_start` and begins when
teardown lands back at `Menu`; a `Cancel` matching the running
generation quits the session through the normal `Unloading → Menu`
path. The accepted session loads through `load_session_world` — the
shared world/spawn path — never a parallel multiplayer loader.

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
replication of score/result state (F26 — the v8
`SnapEntry::damage` byte already carries the replicated total, v10
`Snap.impacts` the per-impact spark/audio presentation and texel
splat, and v11 `SnapEntry::breaks` the detached-part mask), and
the in-app *host*
surface. `mm2 --join --headless` parks the same link inside the smoke
harness, which waits on the wire with wall-clock pacing while `Menu`
is parked and reports the lobby's progress as `mp=` on the record
(`mp=gen<N>` once a `Start` minted the session, `mp=lobby(<n>p)`
while waiting, `mp=lobby(0p)` after the link dies).

### Generation namespaces

*Implementation choice.* `Session` carries two generation counters.
`generation()` is the local, process-lifetime counter —
`ObjectId`/`ResultId` mint off it and staleness detection assumes it
never regresses, so a wire value may only move it forward (a lower
`Start` generation clamps to `local + 1`). `wire_generation()` is the
authority's minted value, adopted verbatim by `begin_generation`;
`begin()` sets both to the local mint since a local session is its own
authority. Everything the wire stamps or gates on — `Input`,
`Snap`/`SnapImpact` frames, `ResetRequest`, the reconcile's
`LobbyState::generation` liveness check, and the cross-process
deterministic seeds (`VehicleSmoke`/`VehicleSparks`/`TexelDamageRig`
and the session audio tables, which need every process to pick the
same authored variant) — reads the wire namespace. The two diverge
because the wire number belongs to the *authority*: a fresh host
process restarts its numbering at 1, so a rejoining client can adopt
a wire generation below its local counter without reusing a local
id space.

The same boundary rules apply to the snap stream itself.
`RemoteSnaps`'s `(generation, tick)` watermarks, pending-impact queue,
repair ledger and dedup window all describe the stream of *one*
authority — none of it can safely reach into the next stream, whose
numbering may restart anywhere. `RemoteSnaps::reset()` therefore runs
at the two observable authority boundaries: an accepted `Start` (on
accept, not on begin — impact rows the new stream queues while a
parked session tears down belong to it) and the pump's terminal
`Closed`. The reset folds still-queued impact rows into `dropped`
rather than losing the evidence, and leaves the `stale`/`dropped`
counters themselves intact for the report fold.

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
actually did (frames in/out, payload bytes in/out — each duplicated
copy re-paying — frames that paid a positive delay/jitter hold,
dropped, duplicated, reordered, overflowed) — a leg asserts the
impairment *happened*, not just that the session coped, and the byte
counters are the bandwidth leg of F25-B req 6's budget evidence.
Pending frames are bounded (`MAX_QUEUED` per lane,
excess dropped and counted), the relay never decodes payloads (present
and future messages alike), and `Drop` shuts every relayed socket and
joins every thread.

The harness is what makes the mailbox's ordering rules load-bearing.
Ordered TCP cannot reorder or duplicate in practice, so the guards
exist for the impaired legs and any future unordered transport: an
`Input` only displaces the stored sample when its sender `seq` is
strictly ahead, a `ResetRequest` mailbox keeps the *highest*
generation per slot — a reordered stale ask can never mask a fresher
one — and `RemoteSnaps::push` is latest-wins on the frame's own
`(generation, tick)`, not arrival order: a frame at or behind the
staged-or-applied watermark drops counted (`net=`'s `snap<x>` cell)
instead of displacing a newer pose, while its `impacts` rows still
queue — events outlive the frame that carried them. The watermark is
scoped to the stream's authority: `RemoteSnaps::reset` at an accepted
`Start` or a link `Closed` clears it (and the pending queue, repair
ledger and dedup window), so a *different* authority's restarted
`(generation, tick)` sequence never reads as a straggler of the dead
stream.

## Measured impairment matrix (F25-AC03)

*Measured evidence — in-process apps over real loopback.*
`net_app`'s `the_impairment_matrix_records_each_recipe_cell` runs the
spec's impairment axes as named recipe cells, each over a fresh hosted
session + joined client through a fresh seeded `ImpairProxy`: the lobby
crosses clean, both directions arm only once the session is `Playing`,
a fixed window of paired updates moves real `Input` frames up and real
`Snap` frames down, and a settle tail drains every scheduled release
(deepest owed hold is delay + jitter + `HOLD_CAP`). The cell asserts
floors — frames moved both ways, fresh snapshots still applied, inputs
still drove the remote seat, each armed knob shows in its `LinkStats`
counter, and duplicate/reorder cells land counted stale drops — while
the `matrix cell=` record line carries the measured counters.

One recorded run (dev-world cruise, one client;
`snap=<sent>s/<applied>a/<staled>x`,
`input=<sent>s/<applied>a/<staled>x`; per-direction `LinkStats` are
whole-connection sums — the handful of clean lobby frames that cross
before arming are inside `frames_in`/`bytes_in`):

| cell | recipe (both dirs) | snap s/a/x | input s/a/x | delayed u/d | dropped u/d | dup u/d | reo u/d |
|---|---|---|---|---|---|---|---|
| clean | — | 251/103/148 | 251/250/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| latency | 100 ms + 20 ms jit | 251/88/142 | 251/250/0 | 250/250 | 0/0 | 0/0 | 0/0 |
| jitter | 10 ms + 60 ms jit | 251/71/164 | 251/250/0 | 250/250 | 0/0 | 0/0 | 0/0 |
| loss | 20% | 251/102/102 | 251/250/0 | 0/0 | 50/47 | 0/0 | 0/0 |
| loss-heavy | 60% | 251/69/25 | 251/250/0 | 0/0 | 147/157 | 0/0 | 0/0 |
| duplicate | 50% | 251/102/271 | 251/250/0 | 0/0 | 0/0 | 122/122 | 0/0 |
| reorder | 50% | 251/104/146 | 251/250/0 | 0/0 | 0/0 | 0/0 | 77/81 |
| combined | 40 ms + 30 ms jit + 5% loss + 10% dup + 10% reo | 251/85/164 | 251/250/0 | 284/285 | 11/10 | 24/24 | 21/21 |

What the numbers show:

- **The `clean` row calibrates `snap<x>`.** `publish_snapshots`
  broadcasts once per `Update` while `session.tick` only advances on
  fixed steps (64 Hz here — the test app's default `Time<Fixed>`), so
  roughly two of three publishes re-send the same tick and drop at the
  client's watermark: 148 stale of 251 on a *clean* link is
  publish-cadence dedup, not impairment. A `snap<x>` reading is only
  meaningful against this floor.
- **Duplicate adds ~one stale per copy** (148 → 271 over 122
  duplicated frames): each second copy lands at-or-behind the
  watermark.
- **Reorder moves the floor rather than raising it** (146 vs 148 over
  81 swaps): the stream is already redundancy-dominated — the straggler
  lands behind the successor it deferred to and drops, which is the
  correctness property (a superseded pose never displaces a newer
  staged one), counted rather than silently rolled back.
- **Loss shrinks the stream, not the session**: at 60% down-loss 69
  snapshots still applied and 250 inputs still drove the remote seat —
  latest-wins means a dropped frame costs only its freshness.
- **`inputs_staled` stayed 0 in every cell**: at ~250 sends/s against
  `INPUT_STALE` (250 ms) even 60% loss never starved the mailbox of a
  fresh sample — the counter measures *staleness at apply*, not
  arrivals (arrival volume is `frames_in`/`bytes_in` at the proxy).
- **Delayed arrivals bunch**: under the 100–120 ms hold, applied drops
  below the distinct-tick count (88) because several frames arrive
  between updates and the staged one supersedes uncounted —
  latest-wins again, only the newest pose ever blends.
- No cell produced an `overflowed`, a `resets` correction, or a lost
  session — bounded queues held and no authority reset was needed.

Same-shape floors are asserted for every cell; the per-frame ordering
that makes those floors safe is proven in `impair`'s lane legs (dup
copies emit adjacent, a swap emits the held frame behind its
successor) and `netdrive`'s push legs (at-or-behind watermark → counted
drop). Scope: in-process loopback — the same grid also exists at
process level (next section); LAN and Internet scope stay open.

### Process-level grid

*Measured evidence — real `mm2` OS processes over real loopback.*
`net_drive`'s
`the_process_level_impairment_matrix_records_each_recipe_cell` runs
the identical cell table with a fresh `mm2 --host --headless` +
`mm2 --join --headless --ready` client pair + seeded `ImpairProxy` per
cell: the lobby crosses clean, `Start` lands unimpaired (a one-shot
verb has no retransmit), then both directions arm the recipe for the
driving window. Each client's mid-session `net=` record proves the
convergence floors — predicted seat drove, authority snaps applied,
both peers reconciled (`rem2`/`rem≥1`), remote wheel spin accumulated
— and the armed recipe's own `LinkStats` counters show it really
fired. A `proc-matrix cell=` record line per recipe carries the
measured counters.

One recorded run (dev-world cruise; `snap=<applied>a/<staled>x` and
`in=<sent>s` from each client's `net=` record; `LinkStats` u/d are
upstream/downstream whole-connection sums — the clean lobby phase is
inside `frames_in`/`bytes_in`):

| cell | recipe (both dirs) | snap a/x alice | snap a/x bob | in s a/b | delayed u/d | dropped u/d | dup u/d | reo u/d |
|---|---|---|---|---|---|---|---|---|
| clean | — | 1336/0 | 943/0 | 1400/1000 | 0/0 | 0/0 | 0/0 | 0/0 |
| latency | 100 ms + 20 ms jit | 557/696 | 471/418 | 1398/998 | 1910/1918 | 0/0 | 0/0 | 0/0 |
| jitter | 10 ms + 60 ms jit | 436/893 | 369/578 | 1398/998 | 1924/1927 | 0/0 | 0/0 | 0/0 |
| loss | 20% | 1154/0 | 814/0 | 1398/998 | 0/0 | 388/364 | 0/0 | 0/0 |
| loss-heavy | 60% | 697/0 | 549/0 | 1398/998 | 0/0 | 1137/1143 | 0/0 | 0/0 |
| duplicate | 50% | 1339/561 | 965/355 | 1398/998 | 0/0 | 0/0 | 927/916 | 0/0 |
| reorder | 50% | 1002/391 | 744/240 | 1398/998 | 0/0 | 0/0 | 0/0 | 642/633 |
| combined | 40 ms + 30 ms jit + 5% loss + 10% dup + 10% reo | 494/840 | 431/544 | 1398/998 | 2175/2149 | 83/91 | 158/157 | 167/152 |

What differs from the in-process table, and why:

- **The `clean` row's `snap<x>` floor is 0 here, not ~60%.** The
  headless runner drives `TimeUpdateStrategy::ManualDuration(1/60)` —
  every `app.update()` is exactly one 60 Hz frame, i.e. exactly two
  120 Hz fixed steps — so every published `Snap` carries a fresh tick
  and nothing republishes same-tick. The in-process floor was the
  fixture's ~250 Hz paired-update cadence against its default 64 Hz
  `Time<Fixed>`. At process level `snap<x>` is therefore *purely*
  wire-attributed: every stale drop above was manufactured by the
  recipe (an overtaken release, a second copy, a held swap frame).
- **Delay/jitter cells produce real stale drops, not just holds** —
  ±20 ms or ±60 ms release jitter lets a later-sent frame emit before
  an earlier one, so the straggler lands at-or-behind the watermark
  and drops counted (696/893 on alice). This is the spec's reordered-
  arrival guarantee exercised end-to-end through real sockets, not
  only the dedicated `reorder` knob.
- **Loss shrinks the stream, not the session**: at 60% loss 697/549
  snaps still applied and every client's `in` stream still drove its
  remote seat — latest-wins costs freshness, never convergence.
- **`frames_in` sums both clients' lanes** (~2,411 sent downstream in
  the clean cell ≈ alice's ~1,340 + bob's ~940 + the clean lobby
  phase), while each `snap<a>`/`<x>` pair is one client's own record.
  A snap pushed then superseded by a newer staged pose before the
  client's next update ran is neither applied nor stale — latest-wins
  replacing `latest` is deliberately uncounted — so `applied + staled`
  need not equal that client's pushed count.
- No cell produced an `overflowed`, a lost session, or a failed
  process — every client exited `status=pass` at its frame cap and
  every host `quit` reported the authority-side counters
  (`in<N>a`/`snap<N>s` nonzero).

Scope: still loopback on a synthetic dev world — no LAN or Internet
leg, no rendered observation, no retail install. The cells also leave
`dsyn`/`tsyn`/`imp`/`rb` honest: the dev car binds no damage record,
tows nothing and authors no breakable parts; `imp` shows single-digit
applied rows (spawn-landing impacts replicated through the real
session), not a driven collision.

## Data-plane budget and bounds (F25-B req 6)

*Implementation choice + measured.* Payload sizes are fixed by the
v11 encode (4-byte length prefix excluded everywhere):

| frame | payload bytes |
|---|---|
| `Input` | 21 (tag 1, generation 8, seq 8, 4 channels) |
| `ResetRequest` | 9 (tag 1, generation 8) |
| `Snap` header | 20 (tag 1, generation 8, tick 8, three counts) |
| per `SnapEntry` | 66 (player 2, pos/rot/vel/angvel 52, epoch 1, steer 2, spin 2, compression 1, flags 1, damage 1, breaks 4) |
| per `SnapTrailer` | 57 (owner 2, pos/rot/vel/angvel 52, spin 2, flags 1) |
| per `SnapImpact` | 46 (seat 2, id 8, tick 8, point 12, normal 12, severity 4) |

A `Snap` is `20 + 66·seats + 57·trailers + 46·impacts` — worst case
`MAX_PLAYERS` 8 seats and trailers plus `MAX_SNAP_IMPACTS` 64 rows =
3,948 B, far under `MAX_FRAME` (256 KiB). The matrix runs measured
the v10 shape: `Input` payloads averaged 21 B
(`bytes_in`/`frames_in` ≈ 20.9) and the one-seat dev-world `Snap`
82 B (20 + 62 — 86 B under v11); the two-process leg's three-seat
snaps were 206 B (218 under v11).

**Update rates.** Both directions send once per app `Update` while the
session is live — the wire rate is the update-loop rate, not the fixed
simulation clock: vsync-bounded windowed (~60–144 Hz), unbounded in
headless runs (the matrix's test apps publish ~250 snaps/s). Same-tick
republishes are deliberate redundancy — `tick` dedups them at the
receiver (the clean-row floor above). Consequences worth recording:

- per-client downstream at a 60 Hz update rate, 8 seats: ≈33 KB/s of
  `Snap` payload (548 B × 60); the ~250 Hz headless cadence multiplies
  that ≈4× (≈134 KB/s, ~1 Mbps) and a full-impact burst snap is still
  ≤3,948 B.
- per-client upstream: 25 B on the wire per update — ≈1.5 KB/s at
  60 Hz, ≈6 KB/s headless.
- a faster update loop buys smoother *redundancy*, not fresher poses —
  poses only change per fixed tick; whether to throttle publishes to
  the tick rate is an open efficiency question, not a correctness one
  (the dedup floor above is the cost).

**Receiver bounds.** Latest-wins everywhere a backlog could form: one
staged `Snap` per client (`RemoteSnaps`), one input sample per seat
(the mailbox, `INPUT_STALE` 250 ms before the seat coasts), the reset
mailbox keeps the highest generation, pending replicated impact rows
cap at 256 with a 512-entry dedup window, `CORRECTION_SNAP_DIST`
(20 m) bounds a blend-vs-snap decision, and `RESET_REQUEST_COOLDOWN`
(1 s) bounds ask rate. The proxy's `MAX_QUEUED` (4096/lane) is harness
state, not protocol.

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
  (`in<N>s`), authoritative snapshots applied (`snap<N>a` — the
  cell's third counter, `snap<x>`, is pose frames dropped stale at
  the push watermark: 0 on a clean link), the other
  participants reconciled as remote copies (`rem2` while both peers
  are still connected), the v7 presentation tail driving the copies'
  wheel spin (`rspn` — accumulated radians the client integrated into
  remote wheels), the v8 damage counter (`dsyn` — replicated totals
  written onto live `VehicleDamage` components; `dsyn0` on these legs
  is honest, the dev car binds no authored damage record), the v9
  trailer counter (`tsyn` — `Snap.trailers` rows applied to a live
  trailer; `tsyn0` likewise, the dev car tows nothing), the v10
  impact counter (`imp<s>s/<a>a/<d>d` — rows broadcast on the
  authority, applied/dropped on the client; `imp0s/0a/0d` on these
  legs is honest, the clean cruise never collides), the v11
  breakaway counter (`rb<d>d/<r>r` — detached-mask transitions the
  client reconciled / restored; `rb0d/0r` is honest too, the dev
  car authors no breakable parts), and its own
  predicted seat driven (`moved=`).
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
  the recipe genuinely fired in each direction — frame counts, the
  `bytes_*` payload volume and the `delayed` hold count — and the
  client records' `snap<x>` cells show the push watermark counted
  the stragglers the recipe produced.

Scope stays honest: this is loopback on a synthetic dev world — no
LAN leg, no rendered observation, no retail install. The recorded
delay/jitter/loss matrix exists at both levels now — in-process (see
"Measured impairment matrix" above) and process-level over this
harness ("Process-level grid", same cell table). LAN and Internet
scope remain open.

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
and `--join` × session-shaping flags are usage exit 2. The suite's
`the_impairment_matrix_records_each_recipe_cell` leg adds the
F25-AC03 grid: eight named recipes (clean/latency/jitter/loss/
loss-heavy/duplicate/reorder/combined) each run a fresh in-process
host + client over a seeded `ImpairProxy` and record the measured
`LinkStats`, `snap s/a/x` and `input s/a/x` counters — the table
under "Measured impairment matrix" is a transcribed run.
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
