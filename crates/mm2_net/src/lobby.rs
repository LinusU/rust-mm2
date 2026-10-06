//! The lobby channel: a host driver and the client join path (F24-B.1).
//!
//! The host is three kinds of thread bridged by one channel:
//!
//! - an **accept thread** polling the non-blocking listener against a
//!   stop flag, forwarding every new connection;
//! - a short-lived **handshake thread** per accepted connection, running
//!   the compatibility gate under [`HANDSHAKE_TIMEOUT`] so a stalled or
//!   flooding peer can never block accepts — in-flight handshakes are
//!   capped by [`MAX_PENDING`];
//! - one **reader thread** per admitted player, forwarding the peer's
//!   lobby messages and reporting the socket's death.
//!
//! The **host loop** is the only code that mutates the roster: it mints
//! player ids, sends `Accept`/`Reject` + `Welcome`, applies `SetReady`
//! and `SetVehicle` (through the consumer's [`HostConfig::pick_validator`]
//! — the roster carries each driver's pick), runs the consumer's
//! start/cancel against the lobby gate, reaps gone peers and broadcasts
//! the whole `Roster` after every change. Consumers drain
//! [`HostEvent`]s — the Bevy bridge (F24-B follow-up) is just a system
//! that forwards them into app state.
//!
//! Start/cancel (F24-B.4): `Host::start` moves the lobby in-session
//! under a fresh generation and broadcasts `Start` carrying the
//! *running* session; `Host::cancel` returns everyone to the lobby.
//! Whether a started session still admits joins is the consumer's
//! per-start [`LateJoin`] choice — MP-5 (documented) closes race lobbies
//! at start but keeps Cruise and Cops & Robbers open, so a late joiner
//! into an open session gets its roster then the running `Start`.
//!
//! The session data plane (F25-A) shares the same ordered socket:
//! a peer's `Input` frames never wake the host loop — they absorb into
//! the [`RemoteInputs`] mailbox, latest-wins per roster slot, so input
//! rate cannot flood the event channel; a peer's `ResetRequest` absorbs
//! the same way (F25-B), collapsing a burst into the one newest ask the
//! consumer drains. Host→client `Snap` snapshots go out through
//! [`HostCtl::broadcast`], which follows the same dead-peer-removal
//! discipline as every other send.
//!
//! Nothing here knows about vehicles, cities or modes: `vehicle` is an
//! opaque id the consumer's validator interprets and `params` an opaque
//! blob — the game-rule types stay in `mm2_game` and the wire carries
//! opaque fields only.

use std::collections::BTreeMap;
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvError, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::NetError;
use crate::conn::{Conn, HANDSHAKE_TIMEOUT, Writer, listen_loopback, recv_hello_within};
use crate::proto::{
    DriveInput, Hello, MAX_PLAYERS, MAX_STRING, Message, RejectCode, RosterEntry,
    SessionAdvertisement, VehiclePick,
};

/// Cap on connections mid-handshake. A connect flood drops at the accept
/// boundary rather than spawning unbounded threads; each in-flight
/// handshake is itself bounded by [`HANDSHAKE_TIMEOUT`]. Sized like the
/// roster — more pending joins than seats can never all fit anyway.
const MAX_PENDING: usize = MAX_PLAYERS as usize;

/// How often the accept thread re-checks the stop flag while no
/// connection is waiting. It bounds both teardown's wait for the
/// accept thread and the extra delay a join sees before its handshake
/// starts — invisible next to a human joining a lobby.
const ACCEPT_POLL: Duration = Duration::from_millis(20);

/// Write bound on established lobby connections. Broadcast sends share
/// the socket with the reader thread, so they get their own deadline:
/// a peer that stops reading stalls a broadcast for this long, then is
/// dropped as `Lost` instead of blocking the loop forever.
const WRITE_TIMEOUT: Duration = HANDSHAKE_TIMEOUT;

/// Bound on `Client::leave`'s post-`Leave` drain. The host closes
/// promptly once it reads the quit (its reader thread exits and drops
/// the socket); this only bounds a host that never does.
const LEAVE_DRAIN_TIMEOUT: Duration = Duration::from_millis(250);

/// The consumer's gate on `SetVehicle` picks (F24-B.3): the wire layer
/// knows nothing about which vehicle ids or paint indices are legal, so
/// the app supplies the check — `mm2_app::net::vehicle_validator` runs a
/// pick against the mounted `VehicleCatalog`. Called on the host loop;
/// `Ok(())` applies the pick to the roster, `Err(reason)` answers the
/// peer with `VehicleRefused` (a refused request, not a violation) and
/// leaves the roster unchanged. The `reason` string is display-ready;
/// it rides a bounded wire field, so the host shortens an over-long
/// reason on a char boundary rather than letting the send fail.
pub type PickValidator = Arc<dyn Fn(&str, u8) -> Result<(), String> + Send + Sync>;

/// Parameters a host listens under.
#[derive(Clone)]
pub struct HostConfig {
    /// The host's `mm2_content::fingerprint::gameplay` value — the
    /// compatibility gate every joining `Hello` is checked against.
    pub gameplay_fingerprint: u64,
    /// Remote-client ceiling. The default [`MAX_PLAYERS`] counts
    /// connections only — a host that is itself a player (MP-1's eight
    /// *total*) should pass `MAX_PLAYERS - 1`; a dedicated headless host
    /// may keep the full eight. Values above [`MAX_PLAYERS`] are
    /// rejected at listen time — the wire roster cannot carry them.
    pub max_clients: u16,
    /// Gate applied to every `SetVehicle` pick on the authoritative
    /// side. `None` accepts any bounded pick — the right default for a
    /// lobby with no content knowledge (tests, a bare transport host);
    /// a real consumer installs its catalog validator.
    pub pick_validator: Option<PickValidator>,
    /// The host process's own vehicle pick when it is also a player
    /// (`mm2 --host`). `None` — the default — means a dedicated
    /// seat-less host; peers then know snapshot player 0 does not
    /// exist. Carried on `Start` so a client can spawn the host's car
    /// (F25-A); the wire roster itself never lists the seat.
    pub host_pick: Option<VehiclePick>,
}

impl HostConfig {
    /// A lobby at the documented player ceiling, no pick validation,
    /// no host seat.
    pub fn new(gameplay_fingerprint: u64) -> Self {
        Self {
            gameplay_fingerprint,
            max_clients: MAX_PLAYERS as u16,
            pick_validator: None,
            host_pick: None,
        }
    }
}

impl std::fmt::Debug for HostConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostConfig")
            .field("gameplay_fingerprint", &self.gameplay_fingerprint)
            .field("max_clients", &self.max_clients)
            .field("pick_validator", &self.pick_validator.is_some())
            .finish()
    }
}

/// Whether a started session still admits joins — the consumer's
/// choice at [`Host::start`], derived from the session's mode. MP-5
/// (documented — `help:Multiplayer Games`) splits the original's
/// behavior by mode: races require everyone in before the host starts;
/// Cruise and Cops & Robbers allow join/leave at any time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LateJoin {
    /// Joins are refused with [`RejectCode::SessionStarted`] for the
    /// session's duration — the race rule.
    Closed,
    /// Joins stay open — the cruise rule. A newcomer is admitted like
    /// any lobby join, then told `Start` with the *running* session and
    /// generation so it enters the session the rest already play.
    Open,
}

/// Why a roster entry went away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaveCause {
    /// The peer sent `Leave` — a deliberate quit.
    Quit,
    /// The socket died (EOF, reset, timed out): a drop, not a quit.
    Lost,
    /// The peer spoke out of turn post-handshake — a protocol violation,
    /// answered by dropping it.
    Malformed,
}

/// One observable lobby transition, drained by the consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// A peer completed the handshake and took a roster slot.
    Joined {
        /// The assigned slot.
        id: u16,
        /// Display name from its `Hello`.
        driver: String,
        /// Build identifier from its `Hello`.
        build: String,
    },
    /// A roster entry went away.
    Left {
        /// The freed slot.
        id: u16,
        /// The departed driver.
        driver: String,
        /// Whether it quit, dropped or violated the protocol.
        cause: LeaveCause,
    },
    /// A peer toggled its readiness flag.
    ReadyChanged {
        /// The slot that changed.
        id: u16,
        /// The new state.
        ready: bool,
    },
    /// A peer's `SetVehicle` pick passed the validator and is now on the
    /// roster (rebroadcast to everyone).
    VehicleChanged {
        /// The slot that picked.
        id: u16,
        /// The picked vehicle id.
        vehicle: String,
        /// The picked paint index.
        paint: u8,
    },
    /// A peer's `SetVehicle` pick failed the validator — the roster is
    /// unchanged and the peer was answered `VehicleRefused`. Logged for
    /// the host; not an error that drops the peer.
    VehicleRefused {
        /// The slot that asked.
        id: u16,
        /// The refused vehicle id.
        vehicle: String,
        /// The refused paint index.
        paint: u8,
        /// The validator's display-ready reason.
        reason: String,
    },
    /// A connection was refused — a failed handshake (version/content/
    /// malformed/stalled, with the wire `Reject` sent where applicable),
    /// a join against a full roster, or a join during a closed session.
    /// `reason` is display-ready.
    JoinFailed {
        /// The refused peer's address.
        peer: SocketAddr,
        /// Why it was refused.
        reason: String,
    },
    /// A `Host::start` request passed the gate: `Start` went out to the
    /// roster (peers whose write failed were reaped `Lost` first, so the
    /// remaining roster is the session's roster) and the lobby is now
    /// in-session.
    Started {
        /// The generation the session was minted under.
        generation: u64,
    },
    /// A `Host::start` request failed the lobby gate — nothing changed.
    /// `reason` is display-ready (`"alice is not ready"`, `"no session
    /// has been advertised"`, `"a session is already running"`).
    StartRefused {
        /// Why the start did not happen.
        reason: String,
    },
    /// A `Host::cancel` ended the in-session phase: `Cancel` went to
    /// every peer, readiness reset for the next round, and joins
    /// re-opened.
    Cancelled {
        /// The generation that ended.
        generation: u64,
    },
}

/// A remote player's newest received [`DriveInput`] plus when it landed.
/// `received` is the staleness clock: sender `seq` values are ticks on
/// the sender's clock, so *arrival* time is what "how old is this
/// sample" is measured against (F25-A).
#[derive(Debug, Clone)]
pub struct StampedInput {
    /// The wire input.
    pub input: DriveInput,
    /// Host-side arrival instant.
    pub received: Instant,
}

/// The mailbox itself: each roster slot's newest input sample plus its
/// newest pending reset request's generation (F25-B).
#[derive(Debug, Default)]
struct PeerMail {
    inputs: BTreeMap<u16, StampedInput>,
    resets: BTreeMap<u16, u64>,
}

/// The host's per-player data-plane mailbox (F25-A). Each admitted
/// peer's reader thread writes its newest `Input` here — one slot per
/// roster id, latest-wins — so a fast sender can never pile up a
/// backlog the consumer must drain, and a slow sender simply leaves a
/// stale sample. `ResetRequest` frames absorb the same way (F25-B): the
/// newest requested generation per slot is what a drain hands the
/// consumer, since a repeated ask is idempotent. Bounded by
/// [`MAX_PLAYERS`]; a departing player's slots are pruned with its
/// roster removal, and lobby teardown clears both maps.
#[derive(Debug, Clone, Default)]
pub struct RemoteInputs {
    inner: Arc<Mutex<PeerMail>>,
}

impl RemoteInputs {
    /// Store `input` as `id`'s newest sample. Never grows past the roster
    /// ceiling — a key the map does not already hold is only admitted
    /// while a slot is free, so junk ids cannot exhaust it.
    ///
    /// "Newest" is judged on the sender's clock: a sample whose `seq` is
    /// not ahead of the stored one's is a duplicate or an arrival-order
    /// regression and is refused (the slot's staleness clock keeps the
    /// fresher sample's landing time). Ordered TCP cannot reorder in
    /// practice, so this guards the impairment harness's reorder/dup
    /// legs and any future unordered transport — spec req 1's sequence
    /// handling.
    fn store(&self, id: u16, input: DriveInput) {
        let mut mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if mail
            .inputs
            .get(&id)
            .is_some_and(|cur| input.seq <= cur.input.seq)
        {
            return;
        }
        if mail.inputs.len() >= MAX_PLAYERS as usize && !mail.inputs.contains_key(&id) {
            return;
        }
        mail.inputs.insert(
            id,
            StampedInput {
                input,
                received: Instant::now(),
            },
        );
    }

    /// `id`'s newest sample, if one ever arrived.
    pub fn latest(&self, id: u16) -> Option<StampedInput> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .inputs
            .get(&id)
            .cloned()
    }

    /// Record `id`'s `ResetRequest` — the newest requested generation
    /// wins like a sample does: a burst between drains collapses to one
    /// ask, which is all an idempotent reset ever needed. "Newest" here
    /// is the *highest* generation — a sender's generation never
    /// regresses, so an arrival-order regression (the impairment
    /// harness's reorder leg) cannot let a stale session's ask mask a
    /// fresher one. Same admission bound as the input slots.
    fn request_reset(&self, id: u16, generation: u64) {
        let mut mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if mail.resets.len() >= MAX_PLAYERS as usize && !mail.resets.contains_key(&id) {
            return;
        }
        let slot = mail.resets.entry(id).or_insert(generation);
        *slot = (*slot).max(generation);
    }

    /// Take every pending reset request — `(roster id, requested
    /// generation)` pairs, id-sorted. The consumer owns deciding which
    /// requests are still meaningful (a stale generation is not).
    pub fn drain_resets(&self) -> Vec<(u16, u64)> {
        let mut mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut mail.resets).into_iter().collect()
    }

    /// Drop `id`'s slots — called with every roster removal so a recycled
    /// wire id never inherits its predecessor's stream or pending ask.
    fn remove(&self, id: u16) {
        let mut mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.inputs.remove(&id);
        mail.resets.remove(&id);
    }

    /// Occupied input slots — for the consumer's diagnostics; never
    /// exceeds [`MAX_PLAYERS`]. Pending reset asks are counted
    /// separately: [`Self::is_idle`] reports whether the mailbox holds
    /// anything at all.
    pub fn input_len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .inputs
            .len()
    }

    /// Whether the mailbox holds nothing — no stored input sample and
    /// no pending reset ask. A peer that only ever asked still leaves
    /// mail behind.
    pub fn is_idle(&self) -> bool {
        let mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.inputs.is_empty() && mail.resets.is_empty()
    }

    /// Drop every slot — the lobby teardown's final state.
    fn clear(&self) {
        let mut mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.inputs.clear();
        mail.resets.clear();
    }
}

/// A running lobby host: owns the listener and event loop, hands the
/// consumer a channel of [`HostEvent`]s. Dropping the `Host` shuts the
/// lobby down (peer sockets closed, threads reaped).
pub struct Host {
    addr: SocketAddr,
    events: Receiver<HostEvent>,
    control: Sender<LoopMsg>,
    inputs: RemoteInputs,
    handle: Option<JoinHandle<()>>,
}

impl Host {
    /// Listen on an explicit address. [`Self::listen_loopback`] is the
    /// safe default; a LAN/interface bind is the caller's deliberate
    /// choice, never the default.
    pub fn listen(addr: SocketAddr, config: &HostConfig) -> Result<Self, NetError> {
        Self::spawn(TcpListener::bind(addr)?, config)
    }

    /// Listen on `127.0.0.1` with an ephemeral port — tests and local
    /// play. No public interface is ever bound by default.
    pub fn listen_loopback(config: &HostConfig) -> Result<Self, NetError> {
        Self::spawn(listen_loopback()?, config)
    }

    fn spawn(listener: TcpListener, config: &HostConfig) -> Result<Self, NetError> {
        if config.max_clients > MAX_PLAYERS as u16 {
            return Err(NetError::Config(
                "max_clients exceeds the wire's MAX_PLAYERS bound",
            ));
        }
        let addr = listener.local_addr()?;
        let (tx, rx) = mpsc::channel();
        let (events_tx, events) = mpsc::channel();
        let config = config.clone();
        let control = tx.clone();
        let inputs = RemoteInputs::default();
        let loop_inputs = inputs.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let accept = spawn_accept(listener, tx.clone(), stop.clone(), adopt)?;
        let handle = thread::spawn(move || {
            run(accept, stop, config, tx, rx, events_tx, loop_inputs);
        });
        Ok(Self {
            addr,
            events,
            control,
            inputs,
            handle: Some(handle),
        })
    }

    /// The address peers dial — `127.0.0.1:<ephemeral>` for a loopback
    /// host.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// A control handle for driving the lobby from another thread —
    /// `Host` itself is `!Sync` (its event channel is a `Receiver`), so
    /// a consumer that wants a second thread calling `start`/`cancel`/
    /// `shutdown` (the dedicated host's stdin reader, a UI thread)
    /// hands this off instead.
    pub fn ctl(&self) -> HostCtl {
        HostCtl {
            control: self.control.clone(),
        }
    }

    /// The data-plane mailbox every admitted peer's `Input` frames and
    /// `ResetRequest`s land in — the host-side consumer's view of the
    /// session data plane (F25-A/F25-B). Latest-wins per roster slot;
    /// see [`RemoteInputs`].
    pub fn remote_inputs(&self) -> RemoteInputs {
        self.inputs.clone()
    }

    /// Advertise (or replace) the session this lobby will run. The
    /// advertisement is broadcast to every connected player and sent to
    /// each newcomer between `Welcome` and the roster, so a client always
    /// sees the current session alongside its first roster. A payload
    /// the wire cannot carry is refused here rather than silently
    /// dropped by the loop.
    ///
    /// What the advertisement *says* is the consumer's business — the
    /// `mm2_app` bridge maps `SessionConfig` onto the opaque `params`
    /// blob; the lobby only bounds and carries it.
    pub fn set_session(&self, session: SessionAdvertisement) -> Result<(), NetError> {
        Message::Session(session.clone()).encode()?;
        self.ctl().send(LoopMsg::SetSession(session))
    }

    /// Request the session start. The request is asynchronous — the
    /// verdict arrives as [`HostEvent::Started`] or
    /// [`HostEvent::StartRefused`], never as this call's return value.
    ///
    /// The gate (designed — MP-8 gives the host the start control but
    /// does not evidence the original's refusal conditions): a session
    /// must have been advertised, and every *connected* player must be
    /// ready and have a vehicle pick — a driver cannot spawn into a
    /// session with no committed car. An empty roster passes: a remote
    /// client's state cannot gate a host that is itself the only
    /// player (wire ids start at 1; the app-layer host player is not
    /// on the wire roster).
    ///
    /// `late_join` is the started session's join policy — see
    /// [`LateJoin`] for the MP-5 split. On success the lobby mints a
    /// fresh generation, broadcasts `Start` carrying the running
    /// session (which `set_session` may later replace for the *next*
    /// round without touching the running one), and refuses or admits
    /// joins per the policy.
    pub fn start(&self, late_join: LateJoin) -> Result<(), NetError> {
        self.ctl().start(late_join)
    }

    /// End the in-session phase: broadcast `Cancel`, re-open joins and
    /// reset every readiness flag for the next round (designed — a new
    /// start wants fresh consent; picks are kept). A no-op while the
    /// lobby is not in-session. Confirmed by [`HostEvent::Cancelled`].
    pub fn cancel(&self) -> Result<(), NetError> {
        self.ctl().cancel()
    }

    /// Wait for the next lobby event, unbounded — for consumers that
    /// live entirely on lobby traffic (the dedicated host). `Err` means
    /// the loop is gone.
    pub fn recv(&self) -> Result<HostEvent, RecvError> {
        self.events.recv()
    }

    /// Wait for the next lobby event, at most `timeout`.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<HostEvent, RecvTimeoutError> {
        self.events.recv_timeout(timeout)
    }

    /// Drain a pending event without blocking.
    pub fn try_recv(&self) -> Result<HostEvent, mpsc::TryRecvError> {
        self.events.try_recv()
    }

    /// Shut the lobby down: peer sockets close (their clients see the
    /// connection die), the accept thread wakes and exits, the loop
    /// joins. Also runs on `Drop`.
    pub fn shutdown(&mut self) {
        let _ = self.control.send(LoopMsg::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// A cloneable handle for driving the host loop from a thread other
/// than the one draining [`HostEvent`]s — see [`Host::ctl`]. Sending
/// fails `Err` only when the loop is gone; each request's verdict still
/// arrives on the event channel.
#[derive(Clone)]
pub struct HostCtl {
    control: Sender<LoopMsg>,
}

impl HostCtl {
    fn send(&self, msg: LoopMsg) -> Result<(), NetError> {
        self.control.send(msg).map_err(|_| {
            NetError::Io(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "host loop is not running",
            ))
        })
    }

    /// [`Host::start`], from any thread.
    pub fn start(&self, late_join: LateJoin) -> Result<(), NetError> {
        self.send(LoopMsg::Start { late_join })
    }

    /// [`Host::cancel`], from any thread.
    pub fn cancel(&self) -> Result<(), NetError> {
        self.send(LoopMsg::Cancel)
    }

    /// Ask the host loop to shut down — like [`Host::shutdown`] but
    /// without joining its thread (the owner joins on `Drop`). The
    /// event channel then closes, ending a consumer's `recv` loop.
    pub fn shutdown(&self) -> Result<(), NetError> {
        self.send(LoopMsg::Shutdown)
    }

    /// Broadcast `msg` to every rostered player — the host→client
    /// data-plane send (F25-A: `Snap` snapshots). Shares the loop's
    /// removal discipline: a peer whose write fails is reaped `Lost`
    /// and the survivors get a corrected roster. A payload the wire
    /// cannot carry is refused here (encode-checked) rather than
    /// silently dropped by the loop.
    pub fn broadcast(&self, msg: &Message) -> Result<(), NetError> {
        msg.encode()?;
        self.send(LoopMsg::Broadcast(msg.clone()))
    }
}

/// The joining side of a lobby. `join` completes the handshake and the
/// slot assignment; the roster and later lobby traffic arrive through
/// [`Client::recv`].
#[derive(Debug)]
pub struct Client {
    conn: Conn,
    player_id: u16,
}

impl Client {
    /// Connect to `addr`, run the handshake and read the host's
    /// `Welcome` carrying this client's roster slot. `Accept` was only
    /// the compatibility verdict, so the `Welcome` wait gets its own
    /// [`HANDSHAKE_TIMEOUT`] bound — a host may not stall it forever.
    pub fn join(addr: SocketAddr, hello: &Hello) -> Result<Self, NetError> {
        let mut conn = Conn::connect(addr)?;
        crate::send_hello(&mut conn, hello)?;
        conn.set_timeout(Some(HANDSHAKE_TIMEOUT))?;
        let welcome = conn.recv();
        conn.set_timeout(None)?;
        let player_id = match welcome? {
            Message::Welcome { player_id } => player_id,
            _ => return Err(NetError::Unexpected("expected Welcome")),
        };
        Ok(Self { conn, player_id })
    }

    /// The roster slot the host assigned.
    pub fn player_id(&self) -> u16 {
        self.player_id
    }

    /// Toggle this client's readiness flag; the host rebroadcasts the
    /// roster to everyone.
    pub fn set_ready(&mut self, ready: bool) -> Result<(), NetError> {
        self.conn.send(&Message::SetReady { ready })
    }

    /// Set this client's vehicle pick. The host checks it against the
    /// consumer's pick validator: a legal pick lands on the roster and
    /// is rebroadcast to everyone; a refused one comes back to this
    /// client alone as `Message::VehicleRefused`. `vehicle` is the
    /// consumer's content id — `mm2_app` uses catalog ids (`vpbug`) and
    /// the empty string for the synthetic dev car.
    pub fn set_vehicle(&mut self, vehicle: &str, paint: u8) -> Result<(), NetError> {
        self.conn.send(&Message::SetVehicle(VehiclePick {
            vehicle: vehicle.to_string(),
            paint,
        }))
    }

    /// Send an arbitrary lobby message. The host accepts `SetReady`,
    /// `SetVehicle`, `Leave`, `Input` and `ResetRequest` post-handshake
    /// — anything else is a protocol violation that gets this client
    /// dropped.
    pub fn send(&mut self, msg: &Message) -> Result<(), NetError> {
        self.conn.send(msg)
    }

    /// A clean quit: the host reports `Quit`, not a dropped connection.
    /// After sending `Leave` the socket is half-closed and drained
    /// briefly — a socket dropped with unread inbound data resets (RST),
    /// which would make a deliberate quit indistinguishable from a
    /// crash. Consumes the client.
    pub fn leave(mut self) -> Result<(), NetError> {
        // Bound the drain before `Leave` goes out: the host closes as
        // soon as it reads `Leave`, and macOS refuses `setsockopt`
        // (EINVAL) on a socket whose write half is shut and whose peer
        // has closed — setting it after the half-close races that close.
        self.conn.set_read_timeout(Some(LEAVE_DRAIN_TIMEOUT))?;
        self.conn.send(&Message::Leave)?;
        self.conn.shutdown_write();
        // Read until the host closes (its reader thread exits on `Leave`)
        // or the drain bound expires — either way no inbound data is
        // left pending at drop, so the close stays a clean FIN.
        while self.conn.recv().is_ok() {}
        Ok(())
    }

    /// The next host→client message — a `Roster` broadcast today, more
    /// lobby traffic as later F24-B legs land. `Err` means the
    /// connection to the host is gone.
    pub fn recv(&mut self) -> Result<Message, NetError> {
        self.conn.recv()
    }

    /// Bound `recv`/`set_ready` waits — tests install a short backstop so
    /// a broken peer cannot hang the suite.
    pub fn set_timeout(&self, timeout: Option<Duration>) -> Result<(), NetError> {
        self.conn.set_timeout(timeout)
    }

    /// A send-side handle for threads other than the one blocked in
    /// [`Client::recv`] — the client's counterpart of [`Host::ctl`],
    /// so a consumer that reads lobby traffic on one thread can answer
    /// it from another (a stdin driver, a Bevy system). The handle
    /// serializes sends so two callers cannot interleave a frame, and
    /// installs [`WRITE_TIMEOUT`] — socket options are shared, so the
    /// bound then also covers this `Client`'s own sends: a peer that
    /// stops reading bounds a send rather than stalling it forever.
    pub fn ctl(&self) -> Result<ClientCtl, NetError> {
        let writer = self.conn.writer()?;
        writer.set_write_timeout(Some(WRITE_TIMEOUT))?;
        Ok(ClientCtl {
            writer: Arc::new(Mutex::new(writer)),
        })
    }
}

/// A cloneable `Send`/`Sync` handle to a joined [`Client`]'s send side —
/// see [`Client::ctl`]. It carries only the legal client→host set
/// (`SetReady`, `SetVehicle`, `Leave`, `Input`, `ResetRequest`), so a
/// caller cannot send a host-only message and get the client dropped
/// `Malformed`.
#[derive(Debug, Clone)]
pub struct ClientCtl {
    writer: Arc<Mutex<Writer>>,
}

impl ClientCtl {
    fn send(&self, msg: &Message) -> Result<(), NetError> {
        // A poisoned mutex only means a send panicked mid-write — the
        // socket is still usable, so recover the guard rather than
        // propagate a poisoning that says nothing about the wire.
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        writer.send(msg)
    }

    /// [`Client::set_ready`], from any thread.
    pub fn set_ready(&self, ready: bool) -> Result<(), NetError> {
        self.send(&Message::SetReady { ready })
    }

    /// [`Client::set_vehicle`], from any thread.
    pub fn set_vehicle(&self, vehicle: &str, paint: u8) -> Result<(), NetError> {
        self.send(&Message::SetVehicle(VehiclePick {
            vehicle: vehicle.to_string(),
            paint,
        }))
    }

    /// Send one driver input sample (F25-A) — the client→host
    /// data-plane verb. Unlike the lobby verbs this is fire-and-forget
    /// by design: the newest sample is the only one that matters, and
    /// the host's mailbox keeps exactly that.
    pub fn send_input(&self, input: DriveInput) -> Result<(), NetError> {
        self.send(&Message::Input(input))
    }

    /// Ask the authority to reset this client's own seat (F25-B) —
    /// the predicted-session answer to the local `R` bundle, which a
    /// `Remote` session must never fire itself. Fire-and-forget like
    /// `Input`: the answer, if granted, is the next `Snap` carrying the
    /// seat's bumped `epoch` — no reply message exists. `generation`
    /// must be the session the sender is driving in; the host drops a
    /// request minted against anything else.
    pub fn request_reset(&self, generation: u64) -> Result<(), NetError> {
        self.send(&Message::ResetRequest { generation })
    }

    /// A clean quit from another thread: the host records
    /// [`LeaveCause::Quit`] and closes the socket, which ends the
    /// owner's blocked [`Client::recv`] — unlike [`Client::leave`] this
    /// cannot drain, so the owner finishes the close by reading the
    /// socket to its end (or dropping the client).
    pub fn leave(&self) -> Result<(), NetError> {
        self.send(&Message::Leave)
    }
}

/// Everything the host loop can be woken by — one channel for the accept
/// thread, the handshake threads, the per-player readers and control.
enum LoopMsg {
    /// Accept thread → loop: a fresh connection to handshake.
    Accepted(Conn),
    /// Handshake thread → loop: gated and admitted pending a seat.
    Handshaken { conn: Conn, hello: Hello },
    /// Handshake thread → loop: refused or failed before `Accept`.
    JoinFailed { peer: SocketAddr, reason: String },
    /// Reader thread → loop: a lobby message from a player.
    PeerMessage { id: u16, msg: Message },
    /// Reader thread → loop: the player's socket ended.
    PeerGone { id: u16, cause: LeaveCause },
    /// `Host::set_session` — the session this lobby advertises.
    SetSession(SessionAdvertisement),
    /// `Host::start`/`HostCtl::start` — request session start under the
    /// given late-join policy.
    Start { late_join: LateJoin },
    /// `Host::cancel`/`HostCtl::cancel` — end the in-session phase.
    Cancel,
    /// `HostCtl::broadcast` — a host→client data-plane send
    /// (F25-A snapshots).
    Broadcast(Message),
    /// `Host::shutdown`.
    Shutdown,
}

/// Whether the lobby is taking joins/picks or a session is running.
/// The roster itself stays live either way — mid-session departures,
/// readiness and pick changes still apply and rebroadcast (MP-5: a
/// leaver's vehicle disappears for everyone; a cruise session's late
/// joiner picks its car after joining).
enum Phase {
    /// Lobby state: joins admitted up to the seat cap, `start` allowed.
    Lobby,
    /// A session is running. `session` is the advertisement that
    /// *started* it — which may differ from the lobby's currently
    /// advertised next session — so a late joiner into an `Open`
    /// session is told the running one, not the pending change.
    InSession {
        /// The generation the running session was minted under.
        generation: u64,
        /// The session that started, as the peers received it.
        session: SessionAdvertisement,
        /// The join policy the consumer chose at `start`.
        late_join: LateJoin,
    },
}

/// A rostered player's host-side state.
struct Slot {
    driver: String,
    build: String,
    ready: bool,
    pick: Option<VehiclePick>,
    writer: Writer,
}

/// Run the accept loop on its own thread until `stop` is set. The
/// listener is polled in non-blocking mode so teardown needs nothing but
/// the flag: a blocking `accept` can only be woken by a connection, and
/// the self-connect that used to wake it fails when the host is out of
/// ephemeral ports (`EADDRNOTAVAIL`), leaving `Host::shutdown` hung in
/// the join forever.
///
/// Only the stop flag and a gone loop end the thread. A connection that
/// fails `adopt` is dropped alone, and every accept error backs off one
/// `ACCEPT_POLL` and retries: the errors an owned listening socket can
/// raise are transient (an aborted or reset peer, an interrupt, fd or
/// buffer exhaustion), and quitting on one would leave the lobby
/// silently refusing every later join.
fn spawn_accept(
    listener: TcpListener,
    tx: Sender<LoopMsg>,
    stop: Arc<AtomicBool>,
    adopt: impl Fn(TcpStream) -> io::Result<Conn> + Send + 'static,
) -> io::Result<JoinHandle<()>> {
    listener.set_nonblocking(true)?;
    Ok(thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    // A peer that lands after teardown began is dropped at
                    // the door, never forwarded to a loop that is leaving.
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(conn) = adopt(stream) else {
                        continue;
                    };
                    if tx.send(LoopMsg::Accepted(conn)).is_err() {
                        return;
                    }
                }
                // `WouldBlock` is the idle case; anything else is
                // transient too (see above).
                Err(_) => thread::sleep(ACCEPT_POLL),
            }
        }
    }))
}

/// Turn an accepted socket into a `Conn`. BSD stacks (macOS) hand an
/// accepted socket the listener's non-blocking mode; a `Conn` relies on
/// blocking reads and writes under deadlines. `from_stream` fails when
/// the peer already reset (`peer_addr` reports `NotConnected`).
fn adopt(stream: TcpStream) -> io::Result<Conn> {
    stream.set_nonblocking(false)?;
    Conn::from_stream(stream)
}

fn run(
    accept: JoinHandle<()>,
    stop: Arc<AtomicBool>,
    config: HostConfig,
    tx: Sender<LoopMsg>,
    rx: Receiver<LoopMsg>,
    events: Sender<HostEvent>,
    inputs: RemoteInputs,
) {
    let mut players: BTreeMap<u16, Slot> = BTreeMap::new();
    // The session this lobby advertises; `None` until the consumer sets
    // one — clients then simply never see a `Session` message.
    let mut session: Option<SessionAdvertisement> = None;
    // Lobby vs running session; the session generation counter mints
    // 1 on the first start and climbs monotonically for the lobby's
    // lifetime — peers namespace ids by it (`mm2_game`'s
    // `Session::generation`/`ObjectId::generation` match).
    let mut phase = Phase::Lobby;
    let mut generation: u64 = 0;
    let mut pending = 0usize;
    // Wire ids mint from 1; 0 is reserved for the host player at the app
    // layer so the wire roster and the displayed roster share numbering.
    let mut next_id: u16 = 1;

    while let Ok(msg) = rx.recv() {
        match msg {
            LoopMsg::Shutdown => break,
            LoopMsg::Accepted(conn) => {
                if pending >= MAX_PENDING {
                    // Beyond the in-flight bound: drop at the door; the
                    // peer's connect just sees a closed socket.
                    continue;
                }
                pending += 1;
                let tx = tx.clone();
                let fingerprint = config.gameplay_fingerprint;
                thread::spawn(move || {
                    let mut conn = conn;
                    let peer = conn.peer_addr();
                    let msg = match recv_hello_within(&mut conn, fingerprint, HANDSHAKE_TIMEOUT) {
                        // The deadline stays installed; the loop answers
                        // the verdict itself once it knows a seat is free.
                        Ok(hello) => LoopMsg::Handshaken { conn, hello },
                        Err(e) => LoopMsg::JoinFailed {
                            peer,
                            reason: e.to_string(),
                        },
                    };
                    let _ = tx.send(msg);
                });
            }
            LoopMsg::JoinFailed { peer, reason } => {
                pending = pending.saturating_sub(1);
                let _ = events.send(HostEvent::JoinFailed { peer, reason });
            }
            LoopMsg::Handshaken { mut conn, hello } => {
                pending = pending.saturating_sub(1);
                // A join during a closed session is refused before
                // `Accept`, like a full lobby — the peer gets a named
                // reason through the normal handshake verdict. MP-5:
                // race joiners must be in before the host starts.
                if matches!(
                    phase,
                    Phase::InSession {
                        late_join: LateJoin::Closed,
                        ..
                    }
                ) {
                    let _ = conn.send(&Message::Reject {
                        code: RejectCode::SessionStarted,
                        message: "the session has already started".to_string(),
                    });
                    let _ = events.send(HostEvent::JoinFailed {
                        peer: conn.peer_addr(),
                        reason: "the session has already started".to_string(),
                    });
                    continue;
                }
                if players.len() >= config.max_clients as usize {
                    let _ = conn.send(&Message::Reject {
                        code: RejectCode::LobbyFull,
                        message: "lobby is full".to_string(),
                    });
                    let _ = events.send(HostEvent::JoinFailed {
                        peer: conn.peer_addr(),
                        reason: "lobby is full".to_string(),
                    });
                    continue;
                }
                let id = alloc_id(&players, &mut next_id);
                let Ok(mut writer) = conn.writer() else {
                    continue;
                };
                if writer
                    .send(&Message::Accept)
                    .and_then(|()| writer.send(&Message::Welcome { player_id: id }))
                    .is_err()
                {
                    continue;
                }
                // A newcomer always sees the current session before its
                // first roster. A dead write here drops the conn
                // silently, like a failed `Welcome` above.
                if let Some(ad) = &session
                    && writer.send(&Message::Session(ad.clone())).is_err()
                {
                    continue;
                }
                if conn.set_timeout(None).is_err() {
                    continue;
                }
                // The session clear also lifted the write bound; keep
                // sends bounded so a peer that stops reading is dropped
                // rather than freezing the broadcast.
                let _ = writer.set_write_timeout(Some(WRITE_TIMEOUT));
                players.insert(
                    id,
                    Slot {
                        driver: hello.driver.clone(),
                        build: hello.build.clone(),
                        ready: false,
                        pick: None,
                        writer,
                    },
                );
                let _ = events.send(HostEvent::Joined {
                    id,
                    driver: hello.driver,
                    build: hello.build,
                });
                broadcast_roster(&mut players, &events, &inputs);
                // The newcomer's own first roster send may have removed
                // it as `Lost` — a departed slot gets no reader.
                if players.contains_key(&id) {
                    spawn_reader(conn, id, tx.clone(), inputs.clone());
                }
                // A join into an open in-progress session is dropped
                // straight into it: after its first roster the newcomer
                // gets the *running* session's `Start` — which may
                // differ from the lobby's currently advertised next
                // session — under the running generation. MP-5's
                // join-at-any-time rule, made explicit.
                if let Phase::InSession {
                    generation,
                    session: started,
                    late_join: LateJoin::Open,
                } = &phase
                {
                    let msg = Message::Start {
                        generation: *generation,
                        session: started.clone(),
                        host_pick: config.host_pick.clone(),
                    };
                    let failed = players
                        .get_mut(&id)
                        .is_some_and(|slot| slot.writer.send(&msg).is_err());
                    if failed && remove_player(&mut players, id, LeaveCause::Lost, &events, &inputs)
                    {
                        broadcast_roster(&mut players, &events, &inputs);
                    }
                }
            }
            LoopMsg::PeerMessage {
                id,
                msg: Message::SetReady { ready },
            } => {
                if let Some(slot) = players.get_mut(&id) {
                    slot.ready = ready;
                    let _ = events.send(HostEvent::ReadyChanged { id, ready });
                    broadcast_roster(&mut players, &events, &inputs);
                }
            }
            LoopMsg::PeerMessage {
                id,
                msg: Message::SetVehicle(pick),
            } => {
                let verdict = match &config.pick_validator {
                    Some(validate) => validate(&pick.vehicle, pick.paint),
                    None => Ok(()),
                };
                match verdict {
                    Ok(()) => {
                        if let Some(slot) = players.get_mut(&id) {
                            // An unchanged pick is a no-op — no event,
                            // no broadcast: a repeated request cannot
                            // flood the lobby with rosters.
                            if slot.pick.as_ref() != Some(&pick) {
                                let VehiclePick { vehicle, paint } = pick.clone();
                                slot.pick = Some(pick);
                                let _ =
                                    events.send(HostEvent::VehicleChanged { id, vehicle, paint });
                                broadcast_roster(&mut players, &events, &inputs);
                            }
                        }
                    }
                    Err(reason) => {
                        // The validator is consumer code — its reason is
                        // not guaranteed to fit `VehicleRefused`'s wire
                        // field (an id-echoing validator plus a long
                        // wire-legal id overflows it). Bound it before
                        // the send so an encode failure cannot read as a
                        // dead socket and drop a live peer.
                        let reason = bound_reason(&reason);
                        if let Some(slot) = players.get_mut(&id) {
                            let _ = events.send(HostEvent::VehicleRefused {
                                id,
                                vehicle: pick.vehicle.clone(),
                                paint: pick.paint,
                                reason: reason.clone(),
                            });
                            // A dead refusal write means the peer is
                            // gone — the same removal discipline as a
                            // failed broadcast.
                            if slot
                                .writer
                                .send(&Message::VehicleRefused { reason })
                                .is_err()
                                && remove_player(
                                    &mut players,
                                    id,
                                    LeaveCause::Lost,
                                    &events,
                                    &inputs,
                                )
                            {
                                broadcast_roster(&mut players, &events, &inputs);
                            }
                        }
                    }
                }
            }
            LoopMsg::PeerMessage { .. } => {}
            LoopMsg::SetSession(ad) => {
                session = Some(ad.clone());
                // Removals under the session send change the roster too —
                // survivors get the corrected snapshot after the ad.
                if !broadcast(&mut players, &Message::Session(ad), &events, &inputs) {
                    broadcast_roster(&mut players, &events, &inputs);
                }
            }
            LoopMsg::Broadcast(msg) => {
                // The data-plane send (`Snap`) follows the same removal
                // discipline as every other broadcast: a peer that fails
                // the write is reaped `Lost` and survivors get the
                // corrected roster.
                if !broadcast(&mut players, &msg, &events, &inputs) {
                    broadcast_roster(&mut players, &events, &inputs);
                }
            }
            LoopMsg::Start { late_join } => {
                // The start gate (designed — see `Host::start`): the
                // lobby must be pre-session, a session must have been
                // advertised, and every connected player must be ready
                // and picked. The first offender names the reason.
                let refusal = match phase {
                    Phase::InSession { .. } => Some("a session is already running".to_string()),
                    Phase::Lobby if session.is_none() => {
                        Some("no session has been advertised".to_string())
                    }
                    Phase::Lobby => players
                        .values()
                        .find(|slot| !slot.ready)
                        .map(|slot| format!("{} is not ready", slot.driver))
                        .or_else(|| {
                            players
                                .values()
                                .find(|slot| slot.pick.is_none())
                                .map(|slot| format!("{} has not picked a vehicle", slot.driver))
                        }),
                };
                match refusal {
                    Some(reason) => {
                        let _ = events.send(HostEvent::StartRefused { reason });
                    }
                    None => {
                        // Saturating: at the `u64` ceiling the mint
                        // stays monotonic rather than wrapping into a
                        // regression the clients' never-regress clamp
                        // would silently adopt.
                        generation = generation.saturating_add(1);
                        // The gate guarantees `session` is `Some`.
                        let started = session.clone().unwrap();
                        phase = Phase::InSession {
                            generation,
                            session: started.clone(),
                            late_join,
                        };
                        let msg = Message::Start {
                            generation,
                            session: started,
                            host_pick: config.host_pick.clone(),
                        };
                        // Peers that fail the `Start` write are reaped
                        // `Lost` under the usual discipline and the
                        // survivors get the corrected snapshot — then
                        // `Started` reports the roster the session
                        // actually began with.
                        if !broadcast(&mut players, &msg, &events, &inputs) {
                            broadcast_roster(&mut players, &events, &inputs);
                        }
                        let _ = events.send(HostEvent::Started { generation });
                    }
                }
            }
            LoopMsg::Cancel => {
                if let Phase::InSession { generation, .. } = phase {
                    phase = Phase::Lobby;
                    broadcast(
                        &mut players,
                        &Message::Cancel { generation },
                        &events,
                        &inputs,
                    );
                    // Back in the lobby a fresh start wants fresh
                    // consent — readiness resets (designed); picks stay.
                    for slot in players.values_mut() {
                        slot.ready = false;
                    }
                    let _ = events.send(HostEvent::Cancelled { generation });
                    // Departures under the `Cancel` send and the
                    // readiness reset both changed the roster.
                    broadcast_roster(&mut players, &events, &inputs);
                }
            }
            LoopMsg::PeerGone { id, cause } => {
                if remove_player(&mut players, id, cause, &events, &inputs) {
                    broadcast_roster(&mut players, &events, &inputs);
                }
            }
        }
    }

    // Teardown: close every peer socket (wakes the reader threads, whose
    // sends into the dead channel just fail), drop the input mailbox and
    // stop the accept thread, which sees the flag within one
    // `ACCEPT_POLL`.
    for slot in players.values_mut() {
        slot.writer.disconnect();
    }
    inputs.clear();
    stop.store(true, Ordering::Relaxed);
    let _ = accept.join();
}

/// Mint the lowest-unused-slot id: monotonic until wraparound, never 0,
/// never an occupied slot. Terminates because the roster is bounded.
fn alloc_id(players: &BTreeMap<u16, Slot>, next_id: &mut u16) -> u16 {
    loop {
        let id = *next_id;
        *next_id = next_id.wrapping_add(1).max(1);
        if !players.contains_key(&id) {
            return id;
        }
    }
}

/// A `VehicleRefused` reason that fits its `MAX_STRING` wire field.
/// `PickValidator` is consumer code — nothing guarantees its `Err`
/// string fits — and an over-long reason failing the encode would turn
/// a refused pick into a dropped peer. Shortens on a char boundary and
/// marks the cut.
fn bound_reason(reason: &str) -> String {
    const MARK: &str = "...";
    if reason.len() <= MAX_STRING {
        return reason.to_string();
    }
    let mut end = MAX_STRING - MARK.len();
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{MARK}", &reason[..end])
}

/// Remove one player and close its socket, emitting `Left`. Shared by
/// every removal path so a departed peer never leaves a live socket —
/// the disconnect wakes the reader thread still blocked in `recv` on
/// the same socket (otherwise the thread, the fd and the client's
/// dead-but-open connection all leak). The peer's input-mailbox slot
/// goes with it, so a recycled wire id never inherits a dead driver's
/// last input. `false` when the slot was already gone.
fn remove_player(
    players: &mut BTreeMap<u16, Slot>,
    id: u16,
    cause: LeaveCause,
    events: &Sender<HostEvent>,
    inputs: &RemoteInputs,
) -> bool {
    let Some(slot) = players.remove(&id) else {
        return false;
    };
    slot.writer.disconnect();
    inputs.remove(id);
    let _ = events.send(HostEvent::Left {
        id,
        driver: slot.driver,
        cause,
    });
    true
}

/// Send `msg` to every player once. A failed write means the peer is
/// gone: it is removed as `Lost` (disconnecting the socket so its
/// blocked reader thread wakes). Returns `true` when every player
/// received the message; on `false` the roster changed and callers whose
/// message no longer fits should resend a corrected snapshot
/// (`broadcast_roster` does this for `Roster` itself).
fn broadcast(
    players: &mut BTreeMap<u16, Slot>,
    msg: &Message,
    events: &Sender<HostEvent>,
    inputs: &RemoteInputs,
) -> bool {
    let mut failed = Vec::new();
    for (id, slot) in players.iter_mut() {
        if slot.writer.send(msg).is_err() {
            failed.push(*id);
        }
    }
    let removed = !failed.is_empty();
    for id in failed {
        remove_player(players, id, LeaveCause::Lost, events, inputs);
    }
    !removed
}

/// Send the complete roster to every player, rebuilt and resent after
/// each removal — a survivor must never see a snapshot still listing a
/// departed peer. Each pass removes at least one player, so the resend
/// loop terminates.
fn broadcast_roster(
    players: &mut BTreeMap<u16, Slot>,
    events: &Sender<HostEvent>,
    inputs: &RemoteInputs,
) {
    loop {
        let msg = Message::Roster {
            players: players
                .iter()
                .map(|(&id, s)| RosterEntry {
                    player_id: id,
                    driver: s.driver.clone(),
                    build: s.build.clone(),
                    ready: s.ready,
                    pick: s.pick.clone(),
                })
                .collect(),
        };
        if broadcast(players, &msg, events, inputs) {
            return;
        }
    }
}

/// One thread per player: forward `SetReady`/`SetVehicle` to the loop,
/// absorb `Input` into the shared mailbox (data-plane traffic — the loop
/// is not woken per sample), report `Leave` as a clean quit, and drop
/// the peer on a protocol violation or a dead socket.
fn spawn_reader(conn: Conn, id: u16, tx: Sender<LoopMsg>, inputs: RemoteInputs) {
    thread::spawn(move || {
        let mut conn = conn;
        loop {
            let msg = match conn.recv() {
                Ok(Message::Leave) => LoopMsg::PeerGone {
                    id,
                    cause: LeaveCause::Quit,
                },
                Ok(Message::Input(input)) => {
                    // Absorbed, not forwarded: input arrives per session
                    // tick — far above the event channel's cadence — and
                    // only the newest sample matters anyway.
                    inputs.store(id, input);
                    continue;
                }
                Ok(Message::ResetRequest { generation }) => {
                    // Data-plane like `Input`: newest-wins in the
                    // mailbox, and the generation it was minted against
                    // decides whether the consumer honors it — the
                    // lobby loop neither needs nor wants a wake per ask.
                    inputs.request_reset(id, generation);
                    continue;
                }
                Ok(msg @ (Message::SetReady { .. } | Message::SetVehicle(_))) => {
                    LoopMsg::PeerMessage { id, msg }
                }
                Ok(_) => LoopMsg::PeerGone {
                    id,
                    cause: LeaveCause::Malformed,
                },
                Err(_) => LoopMsg::PeerGone {
                    id,
                    cause: LeaveCause::Lost,
                },
            };
            let terminal = matches!(msg, LoopMsg::PeerGone { .. });
            if tx.send(msg).is_err() || terminal {
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hello;
    use crate::proto::{SNAP_NO_SURFACE, SnapEntry};

    const FP: u64 = 0xaaaa;
    const WAIT: Duration = Duration::from_secs(5);

    fn host() -> Host {
        Host::listen_loopback(&HostConfig::new(FP)).unwrap()
    }

    fn join(addr: SocketAddr, driver: &str) -> Client {
        let client = Client::join(
            addr,
            &hello("test-build".to_string(), driver.to_string(), FP),
        )
        .unwrap();
        client.set_timeout(Some(WAIT)).unwrap();
        client
    }

    /// Read until a roster of exactly `n` players arrives — earlier
    /// rosters from prior transitions may still be queued on the socket.
    fn recv_roster(client: &mut Client, n: usize) -> Vec<RosterEntry> {
        for _ in 0..8 {
            match client.recv().unwrap() {
                Message::Roster { players } if players.len() == n => return players,
                Message::Roster { .. } => continue,
                other => panic!("expected a Roster, got {other:?}"),
            }
        }
        panic!("no {n}-player roster arrived");
    }

    #[test]
    fn a_join_takes_a_slot_and_sees_itself_in_the_roster() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        assert_eq!(alice.player_id(), 1);

        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Joined { id, driver, build }) => {
                assert_eq!(id, 1);
                assert_eq!(driver, "alice");
                assert_eq!(build, "test-build");
            }
            other => panic!("expected Joined, got {other:?}"),
        }
        assert_eq!(
            recv_roster(&mut alice, 1),
            [RosterEntry {
                player_id: 1,
                driver: "alice".to_string(),
                build: "test-build".to_string(),
                ready: false,
                pick: None,
            }]
        );
    }

    #[test]
    fn a_second_join_broadcasts_the_grown_roster_to_everyone() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        let mut bob = join(host.addr(), "bob");
        assert_eq!(bob.player_id(), 2);

        let expected = vec![
            RosterEntry {
                player_id: 1,
                driver: "alice".to_string(),
                build: "test-build".to_string(),
                ready: false,
                pick: None,
            },
            RosterEntry {
                player_id: 2,
                driver: "bob".to_string(),
                build: "test-build".to_string(),
                ready: false,
                pick: None,
            },
        ];
        // The newcomer and the incumbent both see the same roster.
        assert_eq!(recv_roster(&mut bob, 2), expected);
        assert_eq!(recv_roster(&mut alice, 2), expected);

        for (id, driver) in [(1, "alice"), (2, "bob")] {
            match host.recv_timeout(WAIT) {
                Ok(HostEvent::Joined {
                    id: got,
                    driver: name,
                    ..
                }) => {
                    assert_eq!((got, name.as_str()), (id, driver));
                }
                other => panic!("expected Joined, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_ready_change_rebroadcasts_to_everyone() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        let mut bob = join(host.addr(), "bob");
        host.recv_timeout(WAIT).unwrap();
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 2);
        recv_roster(&mut bob, 2);

        alice.set_ready(true).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::ReadyChanged { id: 1, ready: true }) => {}
            other => panic!("expected ReadyChanged, got {other:?}"),
        }
        for client in [&mut alice, &mut bob] {
            let roster = recv_roster(client, 2);
            assert!(roster[0].ready);
            assert!(!roster[1].ready);
        }
    }

    #[test]
    fn a_clean_leave_and_a_drop_report_different_causes() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        let bob = join(host.addr(), "bob");
        host.recv_timeout(WAIT).unwrap();
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 2);

        bob.leave().unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Left {
                id: 2,
                driver,
                cause: LeaveCause::Quit,
            }) => assert_eq!(driver, "bob"),
            other => panic!("expected a Quit Left, got {other:?}"),
        }
        assert_eq!(recv_roster(&mut alice, 1).len(), 1);

        let carol = join(host.addr(), "carol");
        host.recv_timeout(WAIT).unwrap();
        drop(carol); // no Leave — a dead socket
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Left {
                driver,
                cause: LeaveCause::Lost,
                ..
            }) => assert_eq!(driver, "carol"),
            other => panic!("expected a Lost Left, got {other:?}"),
        }
        assert_eq!(recv_roster(&mut alice, 1).len(), 1);
    }

    #[test]
    fn a_rejoin_mints_a_fresh_slot() {
        let host = host();
        let alice = join(host.addr(), "alice");
        assert_eq!(alice.player_id(), 1);
        alice.leave().unwrap();
        host.recv_timeout(WAIT).unwrap(); // Joined
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Left {
                cause: LeaveCause::Quit,
                ..
            }) => {}
            other => panic!("expected a Quit Left, got {other:?}"),
        }

        // Slots are not recycled — a rejoining driver cannot inherit a
        // stale id a peer still remembers.
        let alice2 = join(host.addr(), "alice");
        assert_eq!(alice2.player_id(), 2);
    }

    #[test]
    fn a_full_lobby_rejects_with_lobby_full() {
        let config = HostConfig {
            gameplay_fingerprint: FP,
            max_clients: 1,
            pick_validator: None,
            host_pick: None,
        };
        let host = Host::listen_loopback(&config).unwrap();
        let _alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap();

        let err =
            Client::join(host.addr(), &hello("b".to_string(), "bob".to_string(), FP)).unwrap_err();
        assert!(matches!(
            err,
            NetError::Rejected {
                code: RejectCode::LobbyFull,
                ..
            }
        ));
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::JoinFailed { reason, .. }) => {
                assert!(reason.contains("full"), "got {reason}");
            }
            other => panic!("expected JoinFailed, got {other:?}"),
        }
    }

    #[test]
    fn an_incompatible_peer_never_touches_the_roster() {
        let host = host();
        let err = Client::join(
            host.addr(),
            &hello("b".to_string(), "mallory".to_string(), FP + 1),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            NetError::Rejected {
                code: RejectCode::ContentMismatch,
                ..
            }
        ));
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::JoinFailed { reason, .. }) => {
                assert!(reason.contains("content"), "got {reason}");
            }
            other => panic!("expected JoinFailed, got {other:?}"),
        }
        // And nothing else: no Joined, no roster.
        assert!(host.try_recv().is_err());
    }

    #[test]
    fn out_of_turn_messages_drop_the_peer() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 1);

        // `Accept` is host→client traffic; a client sending it violates
        // the protocol and is disconnected.
        alice.send(&Message::Accept).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Left {
                id: 1,
                cause: LeaveCause::Malformed,
                ..
            }) => {}
            other => panic!("expected a Malformed Left, got {other:?}"),
        }
        // The peer's socket is closed: its next read fails.
        assert!(alice.recv().is_err());
    }

    #[test]
    fn a_handshake_flood_is_bounded_at_the_door() {
        let host = host();
        // Occupy every in-flight handshake slot with peers that connect
        // and never speak — each holds its slot until HANDSHAKE_TIMEOUT,
        // so once the pending cap saturates the door stays closed for the
        // rest of the window.
        let _stallers: Vec<TcpStream> = (0..MAX_PENDING)
            .map(|_| TcpStream::connect(host.addr()).unwrap())
            .collect();
        // A real join during the flood is refused at the accept
        // boundary, not queued behind ten seconds of dead handshakes.
        // Attempts made before the pending count saturates can still
        // slip through, so retry until one is refused — once saturated,
        // every attempt must fail fast.
        let mut joined = Vec::new();
        let refused = (0..100).any(|_| {
            match Client::join(
                host.addr(),
                &hello("b".to_string(), "alice".to_string(), FP),
            ) {
                Ok(client) => {
                    joined.push(client);
                    thread::sleep(Duration::from_millis(10));
                    false
                }
                // Refused at the accept boundary (Io — the conn is
                // dropped mid-handshake) or at the roster cap itself:
                // either way the join cannot hang behind the flood.
                Err(NetError::Io(_))
                | Err(NetError::Rejected {
                    code: RejectCode::LobbyFull,
                    ..
                }) => true,
                Err(other) => panic!("unexpected join failure: {other:?}"),
            }
        });
        assert!(refused, "no join was refused while the flood held");
    }

    /// The iteration-010 review's zombie-drop defect: a broadcast write
    /// failure removed the peer without disconnecting it — the `Writer`
    /// drop left the reader thread blocked in `recv` on the same
    /// socket, leaking the thread and the fd and leaving the client on
    /// a dead-but-open connection. Here the roster fails *encode* (nine
    /// entries over `MAX_PLAYERS`), so no bytes and no FIN reach the
    /// peer — the assertions below can only pass if the removal path
    /// itself disconnects the socket.
    #[test]
    fn a_failed_broadcast_disconnects_the_peer() {
        use std::io::Read;

        let listener = listen_loopback().unwrap();
        let addr = listener.local_addr().unwrap();
        let mut peer = TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();
        let conn = Conn::from_stream(server).unwrap();
        // Nine slots sharing this one socket — the oversize roster fails
        // every send before a byte is written.
        let mut players = BTreeMap::new();
        for id in 1..=9u16 {
            players.insert(
                id,
                Slot {
                    driver: format!("p{id}"),
                    build: "b".to_string(),
                    ready: false,
                    pick: None,
                    writer: conn.writer().unwrap(),
                },
            );
        }
        // The production reader shape: blocked in `recv` for as long as
        // the socket lives.
        let (gone, check) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut conn = conn;
            let _ = conn.recv();
            let _ = gone.send(());
        });
        let (events, rx) = mpsc::channel();
        let inputs = RemoteInputs::default();

        broadcast_roster(&mut players, &events, &inputs);

        assert!(players.is_empty());
        for _ in 0..9 {
            match rx.recv_timeout(WAIT) {
                Ok(HostEvent::Left {
                    cause: LeaveCause::Lost,
                    ..
                }) => {}
                other => panic!("expected a Lost Left, got {other:?}"),
            }
        }
        // The blocked reader woke — its thread and fd are reaped.
        check
            .recv_timeout(WAIT)
            .expect("the departed peers' reader never woke");
        reader.join().unwrap();
        // And the peer is told: its socket reads EOF, not silence. No
        // roster byte was ever sent, so only `disconnect` can do this.
        peer.set_read_timeout(Some(WAIT)).unwrap();
        let mut buf = [0u8; 8];
        assert_eq!(peer.read(&mut buf).unwrap(), 0);
    }

    /// The iteration-010 review's config-validation defect: a
    /// `max_clients` above the wire's `MAX_PLAYERS` bound admitted a
    /// roster `Roster::encode` cannot represent — every broadcast then
    /// failed `OversizeRoster` and mass-dropped the lobby as `Lost`,
    /// with no diagnostic pointing at the config. It is rejected at
    /// listen instead.
    #[test]
    fn an_over_cap_max_clients_is_rejected() {
        let over = HostConfig {
            gameplay_fingerprint: FP,
            max_clients: MAX_PLAYERS as u16 + 1,
            pick_validator: None,
            host_pick: None,
        };
        match Host::listen_loopback(&over) {
            Err(NetError::Config(msg)) => assert!(msg.contains("max_clients"), "got {msg}"),
            Err(e) => panic!("expected NetError::Config, got {e}"),
            Ok(_) => panic!("an over-cap max_clients listened successfully"),
        }
        // The bound itself still listens.
        Host::listen_loopback(&HostConfig {
            gameplay_fingerprint: FP,
            max_clients: MAX_PLAYERS as u16,
            pick_validator: None,
            host_pick: None,
        })
        .unwrap();
    }

    #[test]
    fn host_shutdown_disconnects_everyone() {
        let mut host = host();
        let mut alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 1);

        host.shutdown();
        // The client's socket is dead: its next read errors instead of
        // idling on a lobby that no longer exists.
        assert!(alice.recv().is_err());
    }

    /// The accept thread exits on the stop flag alone. A blocking accept
    /// could only be woken by a self-connect, and when that connect
    /// failed (ephemeral ports exhausted) `Host::shutdown` hung in the
    /// join forever; here no wake connection is ever made, which is
    /// exactly that failure.
    #[test]
    fn the_accept_thread_stops_without_a_wake_connection() {
        let listener = listen_loopback().unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let accept = spawn_accept(listener, tx, stop.clone(), adopt).unwrap();

        // A real peer goes through first, so the thread is known to be
        // live in its accept loop when the flag flips.
        let _peer = TcpStream::connect(addr).unwrap();
        match rx.recv_timeout(WAIT).unwrap() {
            LoopMsg::Accepted(_) => {}
            _ => panic!("expected the peer to be forwarded"),
        }

        stop.store(true, Ordering::Relaxed);
        let (done_tx, done) = mpsc::channel();
        thread::spawn(move || {
            let _ = accept.join();
            let _ = done_tx.send(());
        });
        done.recv_timeout(WAIT)
            .expect("the accept thread ignored the stop flag");
    }

    /// A connection that fails setup costs only itself — a peer that
    /// resets before its accept is the real case — and the next peer
    /// still gets through. Accept errors themselves (aborted
    /// connections, fd exhaustion) need SO_LINGER or rlimit control std
    /// does not offer, so they share the retry path untested.
    #[test]
    fn a_connection_that_fails_setup_does_not_stop_accepts() {
        let listener = listen_loopback().unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let failed = AtomicBool::new(false);
        let accept = spawn_accept(listener, tx, stop.clone(), move |stream| {
            if failed.swap(true, Ordering::Relaxed) {
                adopt(stream)
            } else {
                Err(io::ErrorKind::NotConnected.into())
            }
        })
        .unwrap();

        let _first = TcpStream::connect(addr).unwrap();
        let second = TcpStream::connect(addr).unwrap();
        match rx.recv_timeout(WAIT).unwrap() {
            LoopMsg::Accepted(conn) => assert_eq!(conn.peer_addr(), second.local_addr().unwrap()),
            _ => panic!("expected the second peer to be forwarded"),
        }

        stop.store(true, Ordering::Relaxed);
        accept.join().unwrap();
    }

    fn ad(tag: &str) -> SessionAdvertisement {
        SessionAdvertisement {
            summary: format!("summary {tag}"),
            params: format!("params {tag}").into_bytes(),
        }
    }

    #[test]
    fn a_newcomer_sees_the_session_before_its_first_roster() {
        let host = host();
        host.set_session(ad("one")).unwrap();
        let mut alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap(); // Joined

        match alice.recv().unwrap() {
            Message::Session(got) => assert_eq!(got, ad("one")),
            other => panic!("expected Session, got {other:?}"),
        }
        assert_eq!(recv_roster(&mut alice, 1).len(), 1);
    }

    #[test]
    fn a_session_change_rebroadcasts_to_everyone() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        let mut bob = join(host.addr(), "bob");
        host.recv_timeout(WAIT).unwrap();
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 2);
        recv_roster(&mut bob, 2);

        host.set_session(ad("two")).unwrap();
        for client in [&mut alice, &mut bob] {
            match client.recv().unwrap() {
                Message::Session(got) => assert_eq!(got, ad("two")),
                other => panic!("expected Session, got {other:?}"),
            }
        }

        // Replacement, not accumulation: a third advertisement lands
        // alone, and a newcomer sees only the latest.
        host.set_session(ad("three")).unwrap();
        match alice.recv().unwrap() {
            Message::Session(got) => assert_eq!(got, ad("three")),
            other => panic!("expected Session, got {other:?}"),
        }
        let mut carol = join(host.addr(), "carol");
        match carol.recv().unwrap() {
            Message::Session(got) => assert_eq!(got, ad("three")),
            other => panic!("expected Session, got {other:?}"),
        }
    }

    #[test]
    fn a_lobby_without_a_session_sends_none() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        // No `Session` was ever set — the first message is the roster.
        match alice.recv().unwrap() {
            Message::Roster { .. } => {}
            other => panic!("expected Roster, got {other:?}"),
        }
    }

    /// `set_session` checks the payload against the wire bound before
    /// handing it to the loop — an over-cap params blob is a caller
    /// error, not a broadcast that silently fails every player.
    #[test]
    fn an_oversize_session_is_refused_before_the_wire() {
        let host = host();
        let ad = SessionAdvertisement {
            summary: "s".to_string(),
            params: vec![0; crate::MAX_SESSION_PARAMS + 1],
        };
        match host.set_session(ad) {
            Err(NetError::Proto(crate::proto::ProtoError::OversizeSessionParams(n))) => {
                assert_eq!(n, crate::MAX_SESSION_PARAMS + 1);
            }
            other => panic!("expected OversizeSessionParams, got {other:?}"),
        }
        // The loop never saw the bad payload: a join right after gets
        // only its roster, no Session.
        let mut alice = join(host.addr(), "alice");
        match alice.recv().unwrap() {
            Message::Roster { .. } => {}
            other => panic!("expected Roster, got {other:?}"),
        }
    }

    /// A host whose consumer knows its content: `vpbug` with paints 0-3
    /// is the entire legal set here.
    fn catalog_host() -> Host {
        Host::listen_loopback(&HostConfig {
            pick_validator: Some(Arc::new(|vehicle: &str, paint: u8| {
                match (vehicle, paint) {
                    ("vpbug", 0..=3) => Ok(()),
                    ("vpbug", _) => Err("paint out of range".to_string()),
                    _ => Err(format!("unknown vehicle id {vehicle:?}")),
                }
            })),
            ..HostConfig::new(FP)
        })
        .unwrap()
    }

    #[test]
    fn a_vehicle_pick_lands_on_the_roster() {
        let host = catalog_host();
        let mut alice = join(host.addr(), "alice");
        let mut bob = join(host.addr(), "bob");
        host.recv_timeout(WAIT).unwrap();
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 2);
        recv_roster(&mut bob, 2);

        alice.set_vehicle("vpbug", 2).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::VehicleChanged {
                id: 1,
                vehicle,
                paint: 2,
            }) => assert_eq!(vehicle, "vpbug"),
            other => panic!("expected VehicleChanged, got {other:?}"),
        }
        let pick = Some(VehiclePick {
            vehicle: "vpbug".to_string(),
            paint: 2,
        });
        // The rebroadcast reaches the picker and every peer alike.
        for client in [&mut alice, &mut bob] {
            let roster = recv_roster(client, 2);
            assert_eq!(roster[0].pick, pick);
            assert_eq!(roster[1].pick, None);
        }
        // And a join after the pick sees it in its first roster.
        let mut carol = join(host.addr(), "carol");
        let roster = recv_roster(&mut carol, 3);
        assert_eq!(roster[0].pick, pick);
    }

    #[test]
    fn a_refused_pick_leaves_the_roster_and_informs_the_peer() {
        let host = catalog_host();
        let mut alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 1);

        alice.set_vehicle("vpbug", 9).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::VehicleRefused {
                id: 1,
                vehicle,
                paint: 9,
                reason,
            }) => {
                assert_eq!(vehicle, "vpbug");
                assert_eq!(reason, "paint out of range");
            }
            other => panic!("expected VehicleRefused, got {other:?}"),
        }
        // The refusal reaches the picker alone as a message, not a drop.
        match alice.recv().unwrap() {
            Message::VehicleRefused { reason } => assert_eq!(reason, "paint out of range"),
            other => panic!("expected VehicleRefused, got {other:?}"),
        }
        // The roster was untouched — alice can still pick legally.
        alice.set_vehicle("vpbug", 0).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::VehicleChanged { id: 1, .. }) => {}
            other => panic!("expected VehicleChanged, got {other:?}"),
        }
    }

    /// A pick identical to the slot's current one must not emit an
    /// event or a broadcast — otherwise a repeating client floods the
    /// lobby with rosters. Deterministic leg: a duplicate pick
    /// followed by `SetReady` produces exactly one broadcast (the
    /// ready one) — any dup-pick roster would carry `ready = false`
    /// and arrive first.
    #[test]
    fn an_identical_pick_is_a_noop() {
        let host = catalog_host();
        let mut alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 1);

        alice.set_vehicle("vpbug", 1).unwrap();
        host.recv_timeout(WAIT).unwrap(); // VehicleChanged
        recv_roster(&mut alice, 1);

        alice.set_vehicle("vpbug", 1).unwrap(); // identical — swallowed
        alice.set_ready(true).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::ReadyChanged { id: 1, ready: true }) => {}
            other => panic!("expected ReadyChanged, got {other:?}"),
        }
        match alice.recv().unwrap() {
            Message::Roster { players } => {
                assert!(players[0].ready, "a dup-pick broadcast arrived first");
            }
            other => panic!("expected the ready Roster, got {other:?}"),
        }
        // And no host event named the duplicate pick.
        assert!(host.try_recv().is_err());
    }

    /// The iteration-014 review's refusal-encode defect: the validator's
    /// reason rides a `MAX_STRING` field, but an id-echoing validator
    /// fed a long wire-legal id produced an over-long reason whose
    /// encode failed before any I/O — the send error read as a dead
    /// socket and the *live* picker was dropped `Lost`. The reason is
    /// bounded before the wire now: the peer gets its (shortened)
    /// refusal and stays connected.
    #[test]
    fn an_overlong_refusal_reason_still_reaches_the_peer() {
        let host = Host::listen_loopback(&HostConfig {
            pick_validator: Some(Arc::new(|vehicle: &str, _| {
                if vehicle == "vpbug" {
                    Ok(())
                } else {
                    Err(format!("unknown vehicle id {vehicle:?}"))
                }
            })),
            ..HostConfig::new(FP)
        })
        .unwrap();
        let mut alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 1);

        // A wire-legal id (exactly MAX_STRING bytes) whose echo pushes
        // the reason past the field's bound.
        let id = "x".repeat(MAX_STRING);
        alice.set_vehicle(&id, 0).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::VehicleRefused {
                id: 1,
                vehicle,
                reason,
                ..
            }) => {
                assert_eq!(vehicle, id);
                assert!(reason.len() <= MAX_STRING, "reason: {reason}");
            }
            other => panic!("expected VehicleRefused, got {other:?}"),
        }
        match alice.recv().unwrap() {
            Message::VehicleRefused { reason } => {
                assert!(reason.len() <= MAX_STRING, "reason: {reason}");
                assert!(reason.ends_with("..."), "reason: {reason}");
            }
            other => panic!("expected VehicleRefused, got {other:?}"),
        }
        // The peer survived — a refused pick is not a drop. alice can
        // still pick legally, and no Left was emitted.
        alice.set_vehicle("vpbug", 0).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::VehicleChanged { id: 1, .. }) => {}
            other => panic!("expected VehicleChanged, got {other:?}"),
        }
        assert!(host.try_recv().is_err());
    }

    /// `VehicleRefused` is host→client traffic; a client sending it (or
    /// any other host-side message) speaks out of turn and is dropped.
    #[test]
    fn a_client_sending_vehicle_refused_is_dropped() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 1);

        alice
            .send(&Message::VehicleRefused {
                reason: "i am the host now".to_string(),
            })
            .unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Left {
                id: 1,
                cause: LeaveCause::Malformed,
                ..
            }) => {}
            other => panic!("expected a Malformed Left, got {other:?}"),
        }
        assert!(alice.recv().is_err());
    }

    /// A lobby with no validator installed accepts any wire-bounded
    /// pick — the default for a transport-level host with no content.
    #[test]
    fn a_host_without_a_validator_accepts_any_pick() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 1);

        alice.set_vehicle("anything", 250).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::VehicleChanged {
                id: 1,
                vehicle,
                paint: 250,
            }) => assert_eq!(vehicle, "anything"),
            other => panic!("expected VehicleChanged, got {other:?}"),
        }
    }

    /// A host with an advertised session — `start` needs one to carry.
    fn sessioned_host() -> Host {
        let host = host();
        host.set_session(ad("go")).unwrap();
        host
    }

    /// Join a host that advertises a session — the `Session` message
    /// lands before the first roster.
    fn join_sessioned(host: &Host, driver: &str) -> Client {
        let mut client = join(host.addr(), driver);
        match client.recv().unwrap() {
            Message::Session(_) => {}
            other => panic!("expected Session, got {other:?}"),
        }
        client
    }

    /// Read until `Start` arrives, skipping the roster traffic that
    /// precedes it.
    fn recv_start(client: &mut Client) -> (u64, SessionAdvertisement) {
        for _ in 0..8 {
            match client.recv().unwrap() {
                Message::Start {
                    generation,
                    session,
                    ..
                } => return (generation, session),
                Message::Roster { .. } => continue,
                other => panic!("expected Start, got {other:?}"),
            }
        }
        panic!("no Start arrived");
    }

    /// Drive a rostered player to a start-legal state: pick + ready.
    fn ready_up(host: &Host, client: &mut Client, vehicle: &str) {
        client.set_vehicle(vehicle, 0).unwrap();
        client.set_ready(true).unwrap();
        host.recv_timeout(WAIT).unwrap(); // VehicleChanged
        host.recv_timeout(WAIT).unwrap(); // ReadyChanged
    }

    #[test]
    fn a_start_broadcasts_the_session_under_a_fresh_generation() {
        let host = sessioned_host();
        let mut alice = join_sessioned(&host, "alice");
        let mut bob = join_sessioned(&host, "bob");
        host.recv_timeout(WAIT).unwrap();
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 2);
        recv_roster(&mut bob, 2);
        ready_up(&host, &mut alice, "vpbug");
        ready_up(&host, &mut bob, "vpsemi");

        host.start(LateJoin::Closed).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Started { generation: 1 }) => {}
            other => panic!("expected Started{{1}}, got {other:?}"),
        }
        for client in [&mut alice, &mut bob] {
            let (generation, session) = recv_start(client);
            assert_eq!(generation, 1);
            // `Start` is self-contained — the session it carries, not
            // whatever the client last saw advertised.
            assert_eq!(session, ad("go"));
        }
    }

    /// The start gate (designed): lobby phase, an advertised session,
    /// and every connected player ready and picked — the first blocker
    /// is named in the refusal, and a refused start changes nothing.
    #[test]
    fn the_start_gate_names_the_blocker() {
        let host = sessioned_host();
        let mut alice = join_sessioned(&host, "alice");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 1);

        host.start(LateJoin::Closed).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::StartRefused { reason }) => {
                assert_eq!(reason, "alice is not ready");
            }
            other => panic!("expected StartRefused, got {other:?}"),
        }

        alice.set_ready(true).unwrap();
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 1);
        host.start(LateJoin::Closed).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::StartRefused { reason }) => {
                assert_eq!(reason, "alice has not picked a vehicle");
            }
            other => panic!("expected StartRefused, got {other:?}"),
        }

        // A refused start left the lobby untouched: the pick still
        // lands and the completed roster starts.
        alice.set_vehicle("vpbug", 0).unwrap();
        host.recv_timeout(WAIT).unwrap();
        host.start(LateJoin::Closed).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Started { generation: 1 }) => {}
            other => panic!("expected Started, got {other:?}"),
        }

        // A second start while in-session is refused.
        host.start(LateJoin::Closed).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::StartRefused { reason }) => {
                assert_eq!(reason, "a session is already running");
            }
            other => panic!("expected StartRefused, got {other:?}"),
        }
    }

    #[test]
    fn a_start_without_a_session_is_refused() {
        let host = host();
        host.start(LateJoin::Open).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::StartRefused { reason }) => {
                assert_eq!(reason, "no session has been advertised");
            }
            other => panic!("expected StartRefused, got {other:?}"),
        }
    }

    /// An empty wire roster passes the gate — a remote client's state
    /// cannot gate a host that is itself the only player (the app-layer
    /// host player is not on the wire roster). A dedicated host driving
    /// an empty start is the consumer's own choice.
    #[test]
    fn an_empty_lobby_may_start_a_session() {
        let host = sessioned_host();
        host.start(LateJoin::Closed).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Started { generation: 1 }) => {}
            other => panic!("expected Started, got {other:?}"),
        }
    }

    /// MP-5's race rule: a join into a `Closed` session is refused at
    /// the handshake verdict with a named reason.
    #[test]
    fn a_closed_session_refuses_joins() {
        let host = sessioned_host();
        host.start(LateJoin::Closed).unwrap();
        host.recv_timeout(WAIT).unwrap(); // Started — empty roster passes

        let err = Client::join(
            host.addr(),
            &hello("b".to_string(), "alice".to_string(), FP),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            NetError::Rejected {
                code: RejectCode::SessionStarted,
                ..
            }
        ));
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::JoinFailed { reason, .. }) => {
                assert!(reason.contains("started"), "got {reason}");
            }
            other => panic!("expected JoinFailed, got {other:?}"),
        }
    }

    /// MP-5's cruise rule: a join into an `Open` session is admitted
    /// normally and then told the *running* session's `Start` — which
    /// keeps meaning the started session even after a mid-session
    /// re-advertisement changes the lobby's *next* one.
    #[test]
    fn an_open_session_drops_a_joiner_straight_into_it() {
        let host = sessioned_host();
        let mut alice = join_sessioned(&host, "alice");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 1);
        ready_up(&host, &mut alice, "vpbug");
        host.start(LateJoin::Open).unwrap();
        host.recv_timeout(WAIT).unwrap(); // Started
        assert_eq!(recv_start(&mut alice), (1, ad("go")));

        // A mid-session re-advertisement is the *next* session's — it
        // still broadcasts, but the running session is unaffected.
        host.set_session(ad("next")).unwrap();
        match alice.recv().unwrap() {
            Message::Session(got) => assert_eq!(got, ad("next")),
            other => panic!("expected Session, got {other:?}"),
        }

        let mut bob = join(host.addr(), "bob");
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Joined { id: 2, .. }) => {}
            other => panic!("expected Joined, got {other:?}"),
        }
        // The lobby sequence first — the *advertised* (next) session —
        // then the running session's Start.
        match bob.recv().unwrap() {
            Message::Session(got) => assert_eq!(got, ad("next")),
            other => panic!("expected Session, got {other:?}"),
        }
        recv_roster(&mut bob, 2);
        assert_eq!(recv_start(&mut bob), (1, ad("go")));
    }

    #[test]
    fn a_cancel_returns_everyone_to_a_fresh_lobby() {
        let host = sessioned_host();
        let mut alice = join_sessioned(&host, "alice");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 1);
        ready_up(&host, &mut alice, "vpbug");
        host.start(LateJoin::Closed).unwrap();
        host.recv_timeout(WAIT).unwrap(); // Started
        recv_start(&mut alice);

        host.cancel().unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Cancelled { generation: 1 }) => {}
            other => panic!("expected Cancelled{{1}}, got {other:?}"),
        }
        match alice.recv().unwrap() {
            Message::Cancel { generation: 1 } => {}
            other => panic!("expected Cancel, got {other:?}"),
        }
        // Readiness reset for the next round; the pick survives.
        let roster = recv_roster(&mut alice, 1);
        assert!(!roster[0].ready);
        assert_eq!(
            roster[0].pick.as_ref().map(|p| p.vehicle.as_str()),
            Some("vpbug")
        );

        // The lobby re-opened: a join works; the guest leaves again
        // and a second start mints the next generation.
        let bob = join_sessioned(&host, "bob");
        host.recv_timeout(WAIT).unwrap(); // Joined
        bob.leave().unwrap();
        host.recv_timeout(WAIT).unwrap(); // Left
        alice.set_ready(true).unwrap();
        host.recv_timeout(WAIT).unwrap(); // ReadyChanged
        host.start(LateJoin::Closed).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Started { generation: 2 }) => {}
            other => panic!("expected Started{{2}}, got {other:?}"),
        }
    }

    #[test]
    fn a_cancel_outside_a_session_is_a_noop() {
        let host = sessioned_host();
        let mut alice = join_sessioned(&host, "alice");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 1);

        host.cancel().unwrap();
        alice.set_ready(true).unwrap();
        // No Cancelled event precedes the ReadyChanged — the first
        // event after a no-op cancel is the readiness change itself.
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::ReadyChanged { id: 1, ready: true }) => {}
            other => panic!("expected ReadyChanged, got {other:?}"),
        }
        // And no stray Cancel reached the wire — the next message is
        // the ready roster.
        match alice.recv().unwrap() {
            Message::Roster { players } => assert!(players[0].ready),
            other => panic!("expected the ready Roster, got {other:?}"),
        }
    }

    /// `Start`/`Cancel` are host→client traffic; a client sending
    /// either speaks out of turn and is dropped like any other
    /// host-only message.
    #[test]
    fn client_sent_lifecycle_messages_drop_the_peer() {
        let host = sessioned_host();
        let mut alice = join_sessioned(&host, "alice");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 1);
        alice
            .send(&Message::Start {
                generation: 1,
                session: ad("x"),
                host_pick: None,
            })
            .unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Left {
                id: 1,
                cause: LeaveCause::Malformed,
                ..
            }) => {}
            other => panic!("expected a Malformed Left, got {other:?}"),
        }

        let mut bob = join_sessioned(&host, "bob");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut bob, 1);
        bob.send(&Message::Cancel { generation: 1 }).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Left {
                id: 2,
                cause: LeaveCause::Malformed,
                ..
            }) => {}
            other => panic!("expected a Malformed Left, got {other:?}"),
        }
    }

    /// F26-AC04: world state flows host → client only. A client that
    /// submits a props frame — a prop pose, a broken/settled claim — is
    /// a protocol violation and is dropped like any other lifecycle
    /// spoof, never absorbed as truth.
    #[test]
    fn a_client_cannot_assert_world_props() {
        let host = sessioned_host();
        let mut mallory = join_sessioned(&host, "mallory");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut mallory, 1);
        mallory
            .send(&Message::Props {
                generation: 1,
                tick: 1,
                table: crate::proto::SiteTable::default(),
                rows: vec![crate::proto::SnapProp {
                    site: 0,
                    fragment: crate::proto::SNAP_NO_FRAGMENT,
                    phase: 3,
                    pos: [0.0; 3],
                    rot: [0.0, 0.0, 0.0, 1.0],
                }],
            })
            .unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Left {
                id: 1,
                cause: LeaveCause::Malformed,
                ..
            }) => {}
            other => panic!("expected a Malformed Left, got {other:?}"),
        }
    }

    /// The roster stays the shared truth through a session (MP-5's
    /// leaver rule): mid-session pick changes and departures still
    /// land and rebroadcast.
    #[test]
    fn the_roster_stays_live_mid_session() {
        let host = sessioned_host();
        let mut alice = join_sessioned(&host, "alice");
        let mut bob = join_sessioned(&host, "bob");
        host.recv_timeout(WAIT).unwrap();
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 2);
        recv_roster(&mut bob, 2);
        ready_up(&host, &mut alice, "vpbug");
        ready_up(&host, &mut bob, "vpsemi");
        recv_roster(&mut alice, 2);
        recv_roster(&mut bob, 2);

        host.start(LateJoin::Open).unwrap();
        host.recv_timeout(WAIT).unwrap(); // Started
        recv_start(&mut alice);
        recv_start(&mut bob);

        bob.set_vehicle("vpcaddie", 1).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::VehicleChanged {
                id: 2,
                vehicle,
                paint: 1,
            }) => assert_eq!(vehicle, "vpcaddie"),
            other => panic!("expected VehicleChanged, got {other:?}"),
        }
        let roster = recv_roster(&mut alice, 2);
        assert_eq!(
            roster[1].pick,
            Some(VehiclePick {
                vehicle: "vpcaddie".to_string(),
                paint: 1,
            })
        );

        bob.leave().unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Left {
                id: 2,
                cause: LeaveCause::Quit,
                ..
            }) => {}
            other => panic!("expected a Quit Left, got {other:?}"),
        }
        assert_eq!(recv_roster(&mut alice, 1).len(), 1);
    }

    /// `Host` is `!Sync`; `HostCtl` is the cross-thread driver a
    /// control thread uses while the consumer owns the event channel —
    /// the dedicated host's stdin shape.
    #[test]
    fn a_control_handle_drives_the_loop_from_another_thread() {
        let host = sessioned_host();
        let ctl = host.ctl();
        let driver = thread::spawn(move || ctl.start(LateJoin::Closed));
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Started { generation: 1 }) => {}
            other => panic!("expected Started, got {other:?}"),
        }
        driver.join().unwrap().unwrap();
    }

    /// `Client::ctl` is the client's cross-thread send handle: a
    /// `set_ready`/`set_vehicle` sent through it lands on the roster
    /// exactly like the `Client` methods while the owner sits in
    /// `recv` — the dedicated client's stdin shape.
    #[test]
    fn a_client_ctl_drives_the_lobby_from_another_thread() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap(); // Joined
        recv_roster(&mut alice, 1);

        let ctl = alice.ctl().unwrap();
        let driver = thread::spawn(move || {
            ctl.set_vehicle("vpbug", 1)?;
            ctl.set_ready(true)
        });
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::VehicleChanged {
                id: 1,
                vehicle,
                paint: 1,
            }) => assert_eq!(vehicle, "vpbug"),
            other => panic!("expected VehicleChanged, got {other:?}"),
        }
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::ReadyChanged { id: 1, ready: true }) => {}
            other => panic!("expected ReadyChanged, got {other:?}"),
        }
        driver.join().unwrap().unwrap();
        // Pick and ready each rebroadcast a roster, in wire order.
        let roster = recv_roster(&mut alice, 1);
        assert_eq!(
            roster[0].pick,
            Some(VehiclePick {
                vehicle: "vpbug".to_string(),
                paint: 1,
            })
        );
        let roster = recv_roster(&mut alice, 1);
        assert!(roster[0].ready);
    }

    /// `ctl.leave()` is a clean quit the host reports `Quit` — and it
    /// ends the owner's blocked `recv`, which is the point of the
    /// handle: the control thread can end the session without the
    /// reader cooperating.
    #[test]
    fn a_ctl_leave_quits_and_ends_the_recv_loop() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap(); // Joined
        recv_roster(&mut alice, 1);

        let ctl = alice.ctl().unwrap();
        thread::spawn(move || ctl.leave()).join().unwrap().unwrap();

        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Left {
                id: 1,
                cause: LeaveCause::Quit,
                driver,
            }) => assert_eq!(driver, "alice"),
            other => panic!("expected a Quit Left, got {other:?}"),
        }
        // The host closed the socket: the owner's recv drains the last
        // roster (the removal rebroadcast) then errors — it does not
        // sit blocked on a dead lobby.
        while alice.recv().is_ok() {}
    }

    /// Poll the mailbox until a sample with `seq` (or newer) lands for
    /// `id` — the reader absorbs frames asynchronously, and a slower
    /// frame may still be newest when the poll starts.
    fn wait_input(inputs: &RemoteInputs, id: u16, seq: u64) -> StampedInput {
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            if let Some(input) = inputs.latest(id)
                && input.input.seq >= seq
            {
                return input;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no input landed for slot {id}"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn drive_input(seq: u64) -> DriveInput {
        DriveInput {
            generation: 1,
            seq,
            throttle: 200,
            brake: 40,
            steer: -90,
            handbrake: 0,
        }
    }

    /// `Input` frames are data-plane traffic: they land latest-wins in
    /// the mailbox — newest `seq` replaces the older sample — while the
    /// lobby itself is undisturbed (the sender still reads `SetReady`
    /// afterwards rather than having been dropped as a violator).
    #[test]
    fn input_frames_land_latest_wins_in_the_mailbox() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap(); // Joined
        recv_roster(&mut alice, 1);

        let ctl = alice.ctl().unwrap();
        ctl.send_input(drive_input(10)).unwrap();
        assert_eq!(wait_input(&host.remote_inputs(), 1, 10).input.seq, 10);
        ctl.send_input(drive_input(11)).unwrap();
        assert_eq!(wait_input(&host.remote_inputs(), 1, 11).input.seq, 11);

        alice.set_ready(true).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::ReadyChanged { id: 1, ready: true }) => {}
            other => panic!("expected ReadyChanged, got {other:?}"),
        }
    }

    /// "Newest" is the sender's clock, not arrival order: a sample
    /// whose `seq` is not ahead of the stored one — a duplicate or a
    /// reordered delivery — is refused rather than regressing the slot.
    /// Ordered TCP produces neither in practice; the impairment proxy's
    /// reorder/dup legs (`impair` tests) are where this bites, and a
    /// future unordered transport would need it.
    #[test]
    fn an_older_seq_never_regresses_the_mailbox() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap(); // Joined
        recv_roster(&mut alice, 1);

        let ctl = alice.ctl().unwrap();
        ctl.send_input(drive_input(10)).unwrap();
        ctl.send_input(drive_input(9)).unwrap();
        ctl.send_input(drive_input(10)).unwrap();
        // The ReadyChanged event lands after the reader thread absorbed
        // all three inputs — socket order is the happens-before.
        alice.set_ready(true).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::ReadyChanged { id: 1, ready: true }) => {}
            other => panic!("expected ReadyChanged, got {other:?}"),
        }
        assert_eq!(
            host.remote_inputs().latest(1).unwrap().input.seq,
            10,
            "the seq-9 sample and the seq-10 duplicate were refused"
        );
        // A genuinely newer sample still lands.
        ctl.send_input(drive_input(11)).unwrap();
        assert_eq!(wait_input(&host.remote_inputs(), 1, 11).input.seq, 11);
    }

    /// A departed player's input slot is pruned with the roster slot —
    /// a recycled wire id can never inherit a dead driver's throttle.
    #[test]
    fn a_departed_players_input_is_pruned() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap(); // Joined
        recv_roster(&mut alice, 1);

        alice.ctl().unwrap().send_input(drive_input(1)).unwrap();
        wait_input(&host.remote_inputs(), 1, 1);

        alice.leave().unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Left {
                id: 1,
                cause: LeaveCause::Quit,
                ..
            }) => {}
            other => panic!("expected a Quit Left, got {other:?}"),
        }
        assert!(host.remote_inputs().latest(1).is_none());
    }

    /// `HostCtl::broadcast` is the snapshot channel: the `Snap` frame
    /// lands on every peer's stream.
    #[test]
    fn a_broadcasted_snapshot_reaches_every_peer() {
        let host = sessioned_host();
        let mut alice = join_sessioned(&host, "alice");
        let mut bob = join_sessioned(&host, "bob");
        host.recv_timeout(WAIT).unwrap();
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 2);
        recv_roster(&mut bob, 2);

        let snap = Message::Snap {
            generation: 1,
            tick: 42,
            entries: vec![SnapEntry {
                player: 1,
                pos: [1.0, 2.0, 3.0],
                rot: [0.0, 0.0, 0.0, 1.0],
                vel: [4.0, 0.0, 0.0],
                angvel: [0.0, 5.0, 0.0],
                epoch: 0,
                steer: 0,
                spin: 0,
                compression: 0,
                flags: 0,
                damage: 0,
                breaks: 0,
                prog_state: 0,
                prog_ticks: 0,
                prog_lap: 0,
                prog_next: 0,
                prog_cleared: 0,
                prog_crossings: 0,
                prog_route_clears: 0,
                rpm: 0,
                surf_skid: SNAP_NO_SURFACE,
                skid_slip: 0,
                skid_speed: 0,
                surf_roll: SNAP_NO_SURFACE,
            }],
            trailers: Vec::new(),
            impacts: Vec::new(),
            race: None,
        };
        host.ctl().broadcast(&snap).unwrap();
        for client in [&mut alice, &mut bob] {
            match client.recv().unwrap() {
                Message::Snap {
                    generation: 1,
                    tick: 42,
                    entries,
                    ..
                } => assert_eq!(entries[0].player, 1),
                other => panic!("expected Snap, got {other:?}"),
            }
        }
    }

    /// The mailbox is bounded by the roster cap: stuffing it with ids
    /// beyond `MAX_PLAYERS` neither grows it unboundedly nor accepts
    /// samples for unjoined ids.
    #[test]
    fn the_mailbox_never_exceeds_the_player_cap() {
        let inputs = RemoteInputs::default();
        for id in 1..=(MAX_PLAYERS as u16 + 20) {
            inputs.store(id, drive_input(u64::from(id)));
        }
        assert_eq!(inputs.input_len(), MAX_PLAYERS as usize);
        assert!(inputs.latest(MAX_PLAYERS as u16 + 20).is_none());
    }

    /// A `ResetRequest` rides the same socket and lands in the mailbox
    /// beside the input stream — the reader absorbs it without waking
    /// the lobby loop, so the sender is never dropped as a violator.
    #[test]
    fn reset_requests_absorb_into_the_mailbox() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap(); // Joined
        recv_roster(&mut alice, 1);

        alice.ctl().unwrap().request_reset(7).unwrap();
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            let drained = host.remote_inputs().drain_resets();
            if !drained.is_empty() {
                assert_eq!(drained, [(1, 7)], "the peer's ask, keyed by its slot");
                break;
            }
            assert!(std::time::Instant::now() < deadline, "no request landed");
            thread::sleep(Duration::from_millis(2));
        }
        // Drained means drained — a second take is empty.
        assert!(host.remote_inputs().drain_resets().is_empty());
        // The lobby is undisturbed: the sender still talks afterwards.
        alice.set_ready(true).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::ReadyChanged { id: 1, ready: true }) => {}
            other => panic!("expected ReadyChanged, got {other:?}"),
        }
    }

    /// Requests collapse newest-wins per slot like input samples — a
    /// burst is one ask, since a reset is idempotent — and a drained
    /// slot stays empty. The admission bound is the roster cap.
    #[test]
    fn reset_requests_collapse_newest_wins_and_stay_bounded() {
        let inputs = RemoteInputs::default();
        inputs.request_reset(1, 3);
        inputs.request_reset(1, 9);
        inputs.request_reset(2, 4);
        assert_eq!(inputs.drain_resets(), [(1, 9), (2, 4)]);
        assert!(inputs.drain_resets().is_empty());
        for id in 10..=(10 + MAX_PLAYERS as u16 + 20) {
            inputs.request_reset(id, 1);
        }
        let drained = inputs.drain_resets();
        assert_eq!(drained.len(), MAX_PLAYERS as usize);
        assert!(!drained.iter().any(|(id, _)| *id == 10 + 28));
    }

    /// Newest-wins for asks means newest *generation*: a stale
    /// generation arriving behind a fresher ask — the impairment
    /// proxy's reorder case — can never mask it. `is_idle` counts
    /// pending asks too, not just input slots.
    #[test]
    fn reset_requests_keep_the_newest_generation() {
        let inputs = RemoteInputs::default();
        assert!(inputs.is_idle());
        inputs.request_reset(1, 9);
        inputs.request_reset(1, 3);
        assert_eq!(inputs.drain_resets(), [(1, 9)], "the stale ask lost");
        assert!(inputs.is_idle());
        inputs.request_reset(1, 4);
        assert!(!inputs.is_idle(), "a pending ask is mail");
        assert_eq!(inputs.drain_resets(), [(1, 4)]);
        assert!(inputs.is_idle());
    }

    /// A departed player's pending request is pruned with the roster
    /// slot — a recycled wire id never inherits a dead driver's ask.
    /// Ordered on the socket: the reader stores the request before it
    /// reports the `Leave`, and the `Left` event means the removal ran.
    #[test]
    fn a_departed_players_reset_request_is_pruned() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap(); // Joined
        recv_roster(&mut alice, 1);

        alice.ctl().unwrap().request_reset(3).unwrap();
        alice.leave().unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Left {
                id: 1,
                cause: LeaveCause::Quit,
                ..
            }) => {}
            other => panic!("expected a Quit Left, got {other:?}"),
        }
        assert!(
            !host
                .remote_inputs()
                .drain_resets()
                .iter()
                .any(|(id, _)| *id == 1),
            "a dead driver's pending ask must not survive the roster slot"
        );
    }
}
