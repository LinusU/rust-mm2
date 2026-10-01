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

## Wire protocol (`PROTOCOL_VERSION = 2`)

Length-prefixed frames: `u32le` length + payload, bounded by
`MAX_FRAME` (256 KiB) checked *before* allocation. Messages are strict
little-endian with `u16`-length-bounded strings; unknown tags, bad
truncation and trailing bytes are all hard errors. v1→v2: `RosterEntry`
gained the driver's `pick` and the `SetVehicle`/`VehicleRefused` pair
landed — an incompatible roster shape, so the version moved.

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
                                     host → Session | Roster (broadcast)
                                          → VehicleRefused { reason }
                                            (to the refused peer only)
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
roster is unchanged. A host with no validator accepts any bounded pick.
A pick identical to the slot's current one is a no-op: no event, no
broadcast — a repeating client cannot flood the lobby with rosters
(this is pick-side only; the disclosed `SetReady` rate-limit gap is
unchanged F24-C/AC03 scope). The gameplay-fingerprint gate means every
peer's catalog is identical, so a pick the host accepts is spawnable
everywhere.

### Dedicated host

`mm2-host` (a second `mm2_app` binary) is the first consumer: a headless
host process that mounts the VFS read-only (`--mm2-path` is required —
an empty directory is a valid content-free mount; `--mods` adds a mod
directory), computes the gameplay fingerprint, binds `--bind` (default
`127.0.0.1:0` — loopback with an ephemeral port; the chosen address is
printed so a harness can dial it), advertises a cruise `SessionConfig`
built from `--dev-world` *or* `--city` (the flags conflict; the default
city is `london` and one whose `city/<name>.psdl` does not resolve
through the mounted VFS is refused), `--pro` (Professional instead of
Amateur), `--weather`/`--time-of-day` (0–3 selectors) and `--seed`
(clock-derived when omitted), validates client vehicle picks against
the mounted `VehicleCatalog`, and prints one
`listening=<addr> fingerprint=… seed=… session="…"` record followed by
one `event=` line per lobby event for harness consumption
(`joined`/`left`/`ready`/`vehicle`/`pick_refused`/`join_failed`). No
window, audio or GPU is required — that is the F24-AC05 binary leg,
exercised so far only on loopback. Named sessions and non-cruise modes
have no flags yet — those arrive with the session legs below.

Deliberately *not* here: start/cancel, session-content join gating,
late-join into a running session, host migration, and the per-tick
dataplane (F25).

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
no-op, validator-less acceptance and host-only-message drops.
`mm2_app`'s `net_host` test additionally runs the `mm2-host` binary as a
separate OS process with two in-process clients joining it, picking and
being refused — partial F24-AC01/AC05 evidence (separate host process,
configured bind, no window/audio; clients still share the test
process). This is *not* fully separate-process, LAN or Internet
evidence; F24-C owns that matrix.
