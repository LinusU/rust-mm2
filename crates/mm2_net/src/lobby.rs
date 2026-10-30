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

use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvError, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::NetError;
use crate::cmdbuf::{Consumed, SeatBuffer, SeatStats};
use crate::congest::HOST_UPLINK_BYTES_PER_SEC;
use crate::conn::{
    Conn, HANDSHAKE_TIMEOUT, PEER_STALL_LIMIT, PeerProbe, PeerQueueStats, PeerWriter,
    listen_loopback, recv_hello_within,
};
use crate::proto::{
    DriveInput, FIRST_RESERVED_PLAYER_ID, Hello, MAX_PLAYERS, MAX_STRING, Message, RejectCode,
    RosterEntry, SessionAdvertisement, TickCommand, VehiclePick,
};
use crate::rt::{
    ClientLane, ClientLaneGuard, HostLane, Inbox, LANE_ECHO_TIMEOUT, LaneCounts, LaneTable,
    QueueStats, lane_payload,
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

/// How long host teardown waits, in total, for peers' queued frames to
/// leave before closing their sockets. A designed bound: a stalled peer
/// costs teardown this much once, not once per peer.
const TEARDOWN_FLUSH: Duration = Duration::from_secs(1);

/// How often the lobby loop looks for reliably delivered `Leave`/`SetReady`
/// while no channel message wakes it (designed: a human-scale latency bound).
/// Capacity of the host loop's wake-up queue (control calls, peer messages,
/// handshake verdicts). Callers on the game thread get an error rather than
/// a wait when it is full; reader threads wait, which backs a flooding peer
/// up onto its own socket. A designed bound.
const LOOP_QUEUE_CAP: usize = 1024;

/// Capacity of the lobby event queue the consumer drains. Events are a few
/// per peer per second at most; a consumer this far behind is wedged, and
/// the lobby shuts down rather than grow ([`EventTx`]). A designed bound.
const EVENT_QUEUE_CAP: usize = 4096;

const INTENT_POLL: Duration = Duration::from_millis(50);

/// How often the host loop sends every rostered peer a
/// [`Message::Keepalive`] (F24-AC04), so an idle lobby is never silent
/// for longer than this. Designed, not recovered from the original.
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(1);

/// How long after a `Start`/`Cancel` the host keeps re-sending that
/// lifecycle frame on the keepalive tick (F31-B.4 / #1489). Unlike the
/// repeat-safe `Roster` and advertised `Session`, `Start` and `Cancel`
/// change the receiver's phase, so a client that misses the single
/// frame never enters or never leaves the session; the re-send gives a
/// lossy link several chances to deliver one. A `Start` also stops
/// early once every rostered peer is demonstrably in the session (its
/// first `Input`/`ResetRequest` for the generation), so an acknowledged
/// start does not resend the (possibly large) session blob; the window
/// is what bounds a peer that never answers. Admitting a late joiner
/// into an `Open` session re-arms the window, since that peer's own
/// `Start` is otherwise the one remaining one-shot.
pub const LIFECYCLE_RESEND_WINDOW: Duration = Duration::from_secs(5);

/// How long a joined [`Client`] may hear nothing from the host — not
/// even a keepalive — before `recv` fails with a timeout: a host that
/// keeps its socket open but stopped speaking (stopped process, wedged
/// host, black-holed link) is a lost host. Several keepalive intervals,
/// so a loaded host is not mistaken for a dead one. Designed.
pub const LIVENESS_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the host lets a seat's datagram lane stay silent — no
/// authenticated datagram of any kind, the client's idle probes included
/// (sent every [`KEEPALIVE_INTERVAL`]) — before it removes the seat as
/// [`LeaveCause::Lost`]. A client that stopped (stopped process,
/// black-holed route) keeps its sockets open, so no write fails and
/// nothing else would ever reap it. Several keepalive intervals, a little
/// above the client's own [`LIVENESS_TIMEOUT`] on the host. Designed, not
/// original. Applies to lane clients; a TCP-only join has no lane to hear.
pub const HOST_LIVENESS_TIMEOUT: Duration = Duration::from_secs(8);

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
    /// How long a seat's lane may stay silent before the host removes it
    /// as lost; [`HOST_LIVENESS_TIMEOUT`] by default. A test seam.
    pub liveness_timeout: Duration,
    /// How long a seat's oldest unwritten control frame may wait before
    /// the host disconnects the seat as not reading;
    /// [`PEER_STALL_LIMIT`] by default (NETPLAY-10A). A test seam.
    pub peer_stall_limit: Duration,
    /// The host's aggregate send budget over every seat's lane, in wire
    /// bytes per second (payload, lane header, UDP and IP);
    /// [`HOST_UPLINK_BYTES_PER_SEC`] (5 Mbit/s) by default
    /// (NETPLAY-10B). Retunable with [`Host::set_uplink_cap`].
    pub uplink_bytes_per_sec: u32,
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
            liveness_timeout: HOST_LIVENESS_TIMEOUT,
            peer_stall_limit: PEER_STALL_LIMIT,
            uplink_bytes_per_sec: HOST_UPLINK_BYTES_PER_SEC,
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

/// The loop's bounded event sender. A full queue means the consumer has
/// stopped draining; it is never waited on (that would stall every peer)
/// and an event is never silently dropped — the overflow is flagged and
/// the loop ends the lobby, which the consumer sees as a closed channel.
struct EventTx {
    tx: SyncSender<HostEvent>,
    overflowed: AtomicBool,
}

impl EventTx {
    fn bounded() -> (Self, Receiver<HostEvent>) {
        let (tx, rx) = mpsc::sync_channel(EVENT_QUEUE_CAP);
        (
            Self {
                tx,
                overflowed: AtomicBool::new(false),
            },
            rx,
        )
    }

    /// Queue `event`; `false` when the consumer is gone or too far behind.
    fn send(&self, event: HostEvent) -> bool {
        match self.tx.try_send(event) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.overflowed.store(true, Ordering::Relaxed);
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    fn overflowed(&self) -> bool {
        self.overflowed.load(Ordering::Relaxed)
    }
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
pub(crate) struct PeerMail {
    /// NETPLAY-06A: the UDP lane's seat table (see [`crate::rt`]).
    pub(crate) lane: LaneTable,
    inputs: BTreeMap<u16, StampedInput>,
    resets: BTreeMap<u16, u64>,
    buckets: BTreeMap<u16, Bucket>,
    rate_limited: u64,
    /// NETPLAY-02B: the generation the host is simulating (`None` while no
    /// session runs), the per-seat command buffers keyed by roster id, and
    /// the newest generation each seat's windows named — whether or not
    /// they were accepted — for [`RemoteInputs::entered`].
    running: Option<u64>,
    seats: BTreeMap<u16, SeatBuffer>,
    window_gens: BTreeMap<u16, (u64, Instant)>,
    /// `InputWindow` frames absorbed per seat, accepted or not — the count
    /// a harness waits on to know a delivered frame reached the mailbox.
    windows: BTreeMap<u16, u64>,
    /// NETPLAY-06E-2c: `Leave`/`SetReady` the reliable channel delivered,
    /// in arrival order, for the lobby loop ([`RemoteInputs::take_intent`]).
    intents: VecDeque<(u16, Message)>,
    /// NETPLAY-10A: each rostered seat's pending-I/O queue, for
    /// [`Host::io_queue`].
    io: BTreeMap<u16, PeerProbe>,
}

/// Bound on [`PeerMail::intents`]: the channel's own windows bound what
/// can arrive between two loop passes; this is the backstop.
const MAX_LOBBY_INTENTS: usize = 64;

impl PeerMail {
    /// Queue a reliably delivered lobby intent for the loop.
    pub(crate) fn absorb_lobby_intent(&mut self, id: u16, msg: Message) {
        if self.intents.len() < MAX_LOBBY_INTENTS {
            self.intents.push_back((id, msg));
        }
    }

    /// Record `id`'s reset ask under the mailbox lock — the TCP path and
    /// the lane's reliable channel (NETPLAY-06E-3) share it, so a request
    /// delivered by both is one newest-wins slot.
    pub(crate) fn absorb_reset(&mut self, id: u16, generation: u64) {
        if self.resets.len() >= MAX_PLAYERS as usize && !self.resets.contains_key(&id) {
            return;
        }
        let slot = self.resets.entry(id).or_insert(generation);
        *slot = (*slot).max(generation);
    }

    /// The body of [`RemoteInputs::store_window_at`], on an already-held
    /// mailbox lock so the UDP lane can authenticate and absorb a datagram
    /// atomically with respect to a seat's removal.
    pub(crate) fn absorb_window(
        &mut self,
        id: u16,
        generation: u64,
        epoch: u8,
        first_seq: u64,
        cmds: &[TickCommand],
        now: Instant,
    ) {
        let mail = self;
        if mail.windows.len() < MAX_PLAYERS as usize || mail.windows.contains_key(&id) {
            *mail.windows.entry(id).or_default() += 1;
        }
        if mail.window_gens.len() < MAX_PLAYERS as usize || mail.window_gens.contains_key(&id) {
            mail.window_gens.insert(id, (generation, now));
        }
        if mail.seats.len() >= MAX_PLAYERS as usize && !mail.seats.contains_key(&id) {
            return;
        }
        let running = mail.running;
        let seat = mail.seats.entry(id).or_insert_with(|| SeatBuffer::new(now));
        if running != Some(generation) {
            seat.note_wrong_generation();
            return;
        }
        seat.accept(epoch, first_seq, cmds, now);
    }
}

/// Sustained per-seat `Input` budget, samples per second. The client
/// sends one per ~60 Hz session tick; double that leaves headroom for
/// bursty delivery without letting a flood through. A designed bound,
/// not an original rule (F25-AC06).
pub const INPUT_RATE_HZ: u32 = 120;

/// How many `Input` samples a seat may land back-to-back before the
/// sustained [`INPUT_RATE_HZ`] applies — a stalled link's catch-up burst.
pub const INPUT_BURST: u32 = 16;

const TOKEN: u64 = 1000;

/// One seat's token bucket, in milli-tokens so the refill stays integer.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    milli_tokens: u64,
    at: Instant,
}

impl Bucket {
    fn full(at: Instant) -> Self {
        Bucket {
            milli_tokens: u64::from(INPUT_BURST) * TOKEN,
            at,
        }
    }

    /// Refill to `now`, then take one token if there is one.
    fn take(&mut self, now: Instant) -> bool {
        let micros = now.saturating_duration_since(self.at).as_micros();
        let refill = micros.saturating_mul(u128::from(INPUT_RATE_HZ)) / 1000;
        let cap = u64::from(INPUT_BURST) * TOKEN;
        self.milli_tokens = u64::try_from(u128::from(self.milli_tokens).saturating_add(refill))
            .unwrap_or(u64::MAX)
            .min(cap);
        self.at = self.at.max(now);
        if self.milli_tokens >= TOKEN {
            self.milli_tokens -= TOKEN;
            true
        } else {
            false
        }
    }
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
    pub(crate) inner: Arc<Mutex<PeerMail>>,
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
        self.store_at(id, input, Instant::now());
    }

    /// [`Self::store`] against an explicit arrival instant, so the rate
    /// bound is testable without sleeping.
    ///
    /// Each seat has a token bucket ([`INPUT_RATE_HZ`], [`INPUT_BURST`]):
    /// a sample arriving with the bucket empty is dropped and counted in
    /// [`Self::rate_limited`] — a flooding peer costs one counter bump per
    /// frame, never a queue, and its newest admitted sample stays put.
    fn store_at(&self, id: u16, input: DriveInput, now: Instant) {
        let mut mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if mail.buckets.len() >= MAX_PLAYERS as usize && !mail.buckets.contains_key(&id) {
            return;
        }
        if !mail
            .buckets
            .entry(id)
            .or_insert_with(|| Bucket::full(now))
            .take(now)
        {
            mail.rate_limited += 1;
            return;
        }
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
                received: now,
            },
        );
    }

    /// `Input` frames dropped by the per-seat rate bound, over the
    /// mailbox's lifetime (all seats).
    pub fn rate_limited(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .rate_limited
    }

    /// The newest sign of life from `id`'s data plane — a `Input` sample
    /// or an `InputWindow` — as the generation it named and when it
    /// landed. The stalled-seat watch measures silence against this, so
    /// it works the same on the latest-only and the fixed-tick path.
    pub fn last_activity(&self, id: u16) -> Option<(u64, Instant)> {
        let mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let sample = mail
            .inputs
            .get(&id)
            .map(|s| (s.input.generation, s.received));
        let window = mail.window_gens.get(&id).copied();
        match (sample, window) {
            (Some(a), Some(b)) => Some(if b.1 >= a.1 { b } else { a }),
            (a, b) => a.or(b),
        }
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

    /// Whether `id` has shown it entered `generation` — its newest
    /// `Input` sample or pending `ResetRequest` was minted against it.
    /// The host loop reads this to stop resending a session's `Start` to
    /// a peer that is demonstrably in the session (F31-B.4 / #1489); a
    /// sample from a previous generation does not count, so a peer still
    /// driving the old session keeps being told the new one.
    fn entered(&self, id: u16, generation: u64) -> bool {
        let mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.window_gens.get(&id).is_some_and(|w| w.0 == generation)
            || mail
                .inputs
                .get(&id)
                .is_some_and(|s| s.input.generation == generation)
            || mail.resets.get(&id).is_some_and(|g| *g == generation)
    }

    /// Record `id`'s `ResetRequest` — the newest requested generation
    /// wins like a sample does: a burst between drains collapses to one
    /// ask, which is all an idempotent reset ever needed. "Newest" here
    /// is the *highest* generation — a sender's generation never
    /// regresses, so an arrival-order regression (the impairment
    /// harness's reorder leg) cannot let a stale session's ask mask a
    /// fresher one. Same admission bound as the input slots.
    fn request_reset(&self, id: u16, generation: u64) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .absorb_reset(id, generation);
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
        mail.buckets.remove(&id);
        mail.seats.remove(&id);
        mail.window_gens.remove(&id);
        mail.windows.remove(&id);
        mail.intents.retain(|(seat, _)| *seat != id);
        mail.io.remove(&id);
        mail.lane.remove(id);
    }

    /// Register `id`'s pending-I/O queue for [`Host::io_queue`].
    fn register_io(&self, id: u16, probe: PeerProbe) {
        let mut mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.io.insert(id, probe);
    }

    /// The seats whose pending-I/O queue has gone unread past `limit`.
    fn stalled_io_seats(&self, limit: Duration) -> Vec<(u16, PeerQueueStats)> {
        let mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.io
            .iter()
            .map(|(&id, p)| (id, p.stats()))
            .filter(|(_, s)| s.oldest_age > limit)
            .collect()
    }

    /// The seats whose lane has been silent past `bound`.
    fn silent_lane_seats(&self, bound: Duration) -> Vec<u16> {
        let mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.lane.silent(Instant::now(), bound)
    }

    /// The oldest reliably delivered `Leave`/`SetReady` not yet handled.
    fn take_intent(&self) -> Option<(u16, Message)> {
        let mut mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.intents.pop_front()
    }

    /// Tell the mailbox which session generation the host is simulating
    /// (`None` when no session runs). A change discards every seat's
    /// buffer: per-seat state never survives a generation change, and
    /// windows for any other generation are dropped on arrival.
    pub fn set_running_generation(&self, generation: Option<u64>) {
        let mut mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if mail.running != generation {
            mail.running = generation;
            mail.seats.clear();
        }
    }

    /// Absorb one `InputWindow` for `id` (NETPLAY-02B). Dropped, counted,
    /// when no session runs, the generation is not the running one, the
    /// seat's flood bound is spent or the roster ceiling is reached.
    fn store_window(
        &self,
        id: u16,
        generation: u64,
        epoch: u8,
        first_seq: u64,
        cmds: &[TickCommand],
    ) {
        self.store_window_at(id, generation, epoch, first_seq, cmds, Instant::now());
    }

    /// [`Self::store_window`] against an explicit arrival instant.
    fn store_window_at(
        &self,
        id: u16,
        generation: u64,
        epoch: u8,
        first_seq: u64,
        cmds: &[TickCommand],
        now: Instant,
    ) {
        let mut mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.absorb_window(id, generation, epoch, first_seq, cmds, now);
    }

    /// Consume `id`'s slot for the fixed step about to run — exactly one
    /// per seat per tick (see [`SeatBuffer::consume`]). `None` when no
    /// session runs or the seat has no buffer.
    pub fn consume_command(&self, id: u16) -> Option<Consumed> {
        self.consume_command_at(id, Instant::now())
    }

    /// [`Self::consume_command`] against an explicit instant.
    pub fn consume_command_at(&self, id: u16, now: Instant) -> Option<Consumed> {
        let mut mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.running?;
        let seat = mail.seats.entry(id).or_insert_with(|| SeatBuffer::new(now));
        Some(seat.consume(now))
    }

    /// `id`'s finalized-command ack for the running generation (`0` = none).
    pub fn command_ack(&self, id: u16) -> u64 {
        let mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.seats.get(&id).map_or(0, SeatBuffer::ack)
    }

    /// `id`'s unconsumed buffered commands, saturated at 255 (`0` = none or
    /// no buffer): the depth the client's lead controller steers.
    pub fn command_depth(&self, id: u16) -> u8 {
        let mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.seats
            .get(&id)
            .map_or(0, |s| u8::try_from(s.stats().occupancy).unwrap_or(u8::MAX))
    }

    /// `InputWindow` frames from `id` the mailbox has absorbed over its
    /// lifetime, whether accepted or dropped (0 for an unknown seat).
    pub fn windows_received(&self, id: u16) -> u64 {
        let mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.windows.get(&id).copied().unwrap_or(0)
    }

    /// `id`'s command-buffer counters, if it has a buffer.
    pub fn seat_stats(&self, id: u16) -> Option<SeatStats> {
        let mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.seats.get(&id).map(SeatBuffer::stats)
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
        mail.inputs.is_empty() && mail.resets.is_empty() && mail.seats.is_empty()
    }

    /// Drop every slot — the lobby teardown's final state.
    fn clear(&self) {
        let mut mail = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.inputs.clear();
        mail.resets.clear();
        mail.buckets.clear();
        mail.seats.clear();
        mail.window_gens.clear();
        mail.windows.clear();
        mail.running = None;
        mail.io.clear();
        mail.lane.clear();
    }
}

/// A running lobby host: owns the listener and event loop, hands the
/// consumer a channel of [`HostEvent`]s. Dropping the `Host` shuts the
/// lobby down (peer sockets closed, threads reaped).
pub struct Host {
    addr: SocketAddr,
    events: Receiver<HostEvent>,
    control: SyncSender<LoopMsg>,
    inputs: RemoteInputs,
    lane: Arc<HostLane>,
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
        let (tx, rx) = mpsc::sync_channel(LOOP_QUEUE_CAP);
        let (events_tx, events) = EventTx::bounded();
        let config = config.clone();
        let control = tx.clone();
        let inputs = RemoteInputs::default();
        let loop_inputs = inputs.clone();
        let lane = HostLane::bind(addr.ip(), inputs.clone(), config.uplink_bytes_per_sec)?;
        let loop_lane = lane.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let accept = spawn_accept(listener, tx.clone(), stop.clone(), adopt)?;
        let handle = thread::spawn(move || {
            run(
                accept,
                stop,
                config,
                tx,
                rx,
                events_tx,
                (loop_inputs, loop_lane),
            );
        });
        Ok(Self {
            addr,
            events,
            control,
            inputs,
            lane,
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
            lane: self.lane.clone(),
        }
    }

    /// The UDP port of this host's datagram lane (NETPLAY-06A) — the one
    /// extra port a firewall or port forward must open beside the TCP
    /// listener. It is bound on the listener's IP; clients learn it from
    /// their `LaneGrant`, never from the address they dialed.
    pub fn lane_port(&self) -> u16 {
        self.lane.port()
    }

    /// Lifetime counters of the host's lane: accepted datagrams and every
    /// kind of drop.
    pub fn lane_counts(&self) -> LaneCounts {
        self.lane.counts()
    }

    /// Whether `id`'s lane has been established (the host has answered a
    /// valid probe from it).
    pub fn lane_established(&self, id: u16) -> bool {
        self.lane.is_established(id)
    }

    /// The pending-I/O queue of `id`'s control stream (NETPLAY-10A):
    /// frames handed to the seat's writer thread and not yet written,
    /// with the age of the oldest. Distinct from [`lane_queue`](Self::lane_queue)
    /// and from any retained history, which are reported separately.
    pub fn io_queue(&self, id: u16) -> Option<PeerQueueStats> {
        let mail = self.inputs.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.io.get(&id).map(PeerProbe::stats)
    }

    /// The outbound queue's bounds, age and drop counters for `id`'s
    /// established lane.
    pub fn lane_queue(&self, id: u16) -> Option<QueueStats> {
        self.lane.established_out(id).map(|o| o.stats())
    }

    /// Both priority queues' bounds, ages, drops and the budget's use for
    /// `id`'s established lane (NETPLAY-06D).
    pub fn lane_pace(&self, id: u16) -> Option<crate::PaceStats> {
        self.lane.established_out(id).map(|o| o.pace_stats())
    }

    /// Change the aggregate send budget over every seat's lane (wire
    /// bytes per second): the host learned of, or was told, a different
    /// uplink (NETPLAY-10B). Each seat's share follows within one send
    /// tick.
    pub fn set_uplink_cap(&self, bytes_per_sec: u32) {
        self.lane.set_uplink_cap(bytes_per_sec);
    }

    /// The aggregate uplink's current cap and the wire bytes it has
    /// admitted so far (every seat, every datagram kind).
    pub fn uplink(&self) -> (u32, u64) {
        self.lane.uplink_stats()
    }

    /// How traffic to `id` should adapt to the seat's congestion rate
    /// (NETPLAY-10B); `None` without an established lane.
    pub fn peer_adapt(&self, id: u16) -> Option<crate::Adapt> {
        self.lane.established_out(id).map(|o| o.adapt())
    }

    /// [`HostCtl::reliable_send`], from the `Host` itself.
    pub fn reliable_send(
        &self,
        id: u16,
        generation: u64,
        body: &[u8],
    ) -> Result<(), crate::ReliableSendError> {
        self.lane.send_reliable(id, generation, body)
    }

    /// The next message delivered on any seat's reliable channel, with
    /// its seat: exactly once, in the sender's order per seat.
    pub fn reliable_recv(&self) -> Option<(u16, Vec<u8>)> {
        self.lane.recv_reliable()
    }

    /// Counters of `id`'s reliable channel (both halves).
    pub fn reliable_stats(&self, id: u16) -> Option<crate::ReliableStats> {
        self.lane.reliable_stats(id)
    }

    /// Run reliable retransmission on a manual clock that only
    /// [`Host::advance_reliable_clock`] moves (lockstep tests). Call it
    /// before the first reliable send.
    pub fn use_manual_reliable_clock(&self) {
        self.lane.clock().set_manual();
    }

    /// Advance the manual reliable clock and wake the sender thread.
    pub fn advance_reliable_clock(&self, by: std::time::Duration) {
        self.lane.clock().advance(by);
        self.lane.poke();
    }

    /// [`HostCtl::lane_ack`], from the `Host` itself.
    pub fn lane_ack(&self, id: u16) -> Option<crate::LaneAck> {
        self.lane.latest_ack(id)
    }

    /// [`HostCtl::lane_group_ack`], from the `Host` itself.
    pub fn lane_group_ack(&self, id: u16, group: u8) -> Option<crate::LaneAck> {
        self.lane.group_ack(id, group)
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
        self.lane.shutdown();
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
    control: SyncSender<LoopMsg>,
    lane: Arc<HostLane>,
}

impl HostCtl {
    /// [`Host::peer_adapt`], from the control handle a producer holds.
    pub fn peer_adapt(&self, id: u16) -> Option<crate::Adapt> {
        self.lane.established_out(id).map(|o| o.adapt())
    }

    /// [`Host::lane_pace`], from the control handle a producer holds.
    pub fn lane_pace(&self, id: u16) -> Option<crate::PaceStats> {
        self.lane.established_out(id).map(|o| o.pace_stats())
    }

    /// [`Host::set_uplink_cap`], from the control handle.
    pub fn set_uplink_cap(&self, bytes_per_sec: u32) {
        self.lane.set_uplink_cap(bytes_per_sec);
    }

    fn send(&self, msg: LoopMsg) -> Result<(), NetError> {
        // Never waits: a full queue is an explicit error to the caller
        // (the loop is wedged or flooded), not a stall of the game thread.
        self.control.try_send(msg).map_err(|e| match e {
            TrySendError::Full(_) => NetError::Io(io::Error::new(
                io::ErrorKind::WouldBlock,
                "host control queue full",
            )),
            TrySendError::Disconnected(_) => NetError::Io(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "host loop is not running",
            )),
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
    ///
    /// **Datagram lane (NETPLAY-06A):** a `Snap` that fits one datagram
    /// goes to every seat whose lane is established through its bounded
    /// nonblocking queue — no socket or `Writer` is touched on the
    /// calling thread — and over TCP only to the seats without one. A
    /// `Snap` over the datagram budget is fragmented as one group
    /// (NETPLAY-06C, [`crate::frag`]) and reassembled whole by the
    /// client; [`send_snap_groups`](Self::send_snap_groups) lets the
    /// producer draw the group boundaries.
    pub fn broadcast(&self, msg: &Message) -> Result<(), NetError> {
        let bytes = msg.encode()?;
        if let Message::RemoteCommands { generation, .. } = msg
            && bytes.len() <= crate::rt::MAX_PAYLOAD
        {
            // Never fragmented: a frame that does not fit one datagram
            // stays on the control stream like any non-`Snap` message.
            let outs = self.lane.established_outs();
            if !outs.is_empty() {
                let ids: Vec<u16> = outs.iter().map(|(id, _)| *id).collect();
                for (_, out) in outs {
                    out.push_data(*generation, bytes.clone());
                }
                return self.send(LoopMsg::BroadcastExcept(msg.clone(), ids));
            }
        }
        if let Message::Snap {
            generation, tick, ..
        } = msg
        {
            let outs = self.lane.established_outs();
            if !outs.is_empty() {
                let frags = if bytes.len() > crate::rt::MAX_PAYLOAD {
                    Some(crate::frag::split(*tick, &[&bytes]).map_err(oversize_snap)?)
                } else {
                    None
                };
                let ids: Vec<u16> = outs.iter().map(|(id, _)| *id).collect();
                let now = Instant::now();
                for (_, out) in outs {
                    out.note_tick(*tick, now);
                    match &frags {
                        Some(f) => out.push_fragments(*generation, f.clone()),
                        None => out.push_data(*generation, bytes.clone()),
                    }
                }
                return self.send(LoopMsg::BroadcastExcept(msg.clone(), ids));
            }
        }
        self.send(LoopMsg::Broadcast(msg.clone()))
    }

    /// Send `msg` to the one rostered player `player_id` — the
    /// per-client data-plane send (F26-A.1 relevancy). Same removal
    /// discipline as [`broadcast`](Self::broadcast): a failed write
    /// reaps the peer `Lost`. A `player_id` that is not on the roster
    /// (it left since the caller looked) is skipped, not an error.
    /// A `Snap` for a seat with an established lane takes the datagram
    /// lane like [`broadcast`](Self::broadcast) does.
    pub fn send_to(&self, player_id: u16, msg: &Message) -> Result<(), NetError> {
        let bytes = msg.encode()?;
        if let Message::RemoteCommands { generation, .. } = msg
            && bytes.len() <= crate::rt::MAX_PAYLOAD
            && let Some(out) = self.lane.established_out(player_id)
        {
            out.push_data(*generation, bytes);
            return Ok(());
        }
        if let Message::Snap {
            generation, tick, ..
        } = msg
            && let Some(out) = self.lane.established_out(player_id)
        {
            out.note_tick(*tick, Instant::now());
            if bytes.len() > crate::rt::MAX_PAYLOAD {
                let f = crate::frag::split(*tick, &[&bytes]).map_err(oversize_snap)?;
                out.push_fragments(*generation, f);
            } else {
                out.push_data(*generation, bytes);
            }
            return Ok(());
        }
        self.send(LoopMsg::SendTo(player_id, msg.clone()))
    }

    /// Send one tick's `Snap` to `player_id` as independently deliverable
    /// **groups** over the datagram lane (NETPLAY-06C): each message is
    /// a complete `Snap` for the same `generation` and `tick`, and the
    /// client receives each the moment all of *its* fragments arrive, so
    /// losing a fragment of one group never withholds another. The
    /// producer draws the boundaries along physical dependency — e.g. the
    /// owner's car and its contacts in one group, an unrelated part of
    /// the world in another — because only it knows what may be applied
    /// without the rest. Returns `Ok(false)` when the seat has no
    /// established lane (nothing is sent; use [`send_to`](Self::send_to)).
    pub fn send_snap_groups(
        &self,
        player_id: u16,
        generation: u64,
        tick: u64,
        groups: &[Message],
    ) -> Result<bool, NetError> {
        self.send_snap_groups_prio(player_id, generation, tick, groups, &[])
    }

    /// [`send_snap_groups`](Self::send_snap_groups) with a send
    /// [`Priority`](crate::Priority) per group (NETPLAY-06D): `Movement`
    /// groups drain first and are never held by the peer's byte budget;
    /// `Bulk` groups go from spare budget and are discarded unsent once
    /// obsolete. A group past the end of `priorities` is `Movement` when
    /// `priorities` is empty (the plain call) and `Bulk` otherwise.
    pub fn send_snap_groups_prio(
        &self,
        player_id: u16,
        generation: u64,
        tick: u64,
        groups: &[Message],
        priorities: &[crate::Priority],
    ) -> Result<bool, NetError> {
        let mut encoded = Vec::with_capacity(groups.len());
        for g in groups {
            match g {
                Message::Snap {
                    generation: gg,
                    tick: t,
                    ..
                } if *gg == generation && *t == tick => encoded.push(g.encode()?),
                _ => {
                    return Err(NetError::Unexpected(
                        "snapshot group must be a Snap of the tick",
                    ));
                }
            }
        }
        let Some(out) = self.lane.established_out(player_id) else {
            return Ok(false);
        };
        out.note_tick(tick, Instant::now());
        let slices: Vec<&[u8]> = encoded.iter().map(Vec::as_slice).collect();
        let frags = crate::frag::split(tick, &slices).map_err(oversize_snap)?;
        if priorities.is_empty() {
            out.push_fragments(generation, frags);
        } else {
            out.push_fragments_prio(generation, frags, priorities);
        }
        Ok(true)
    }

    /// The newest baseline acknowledgement `player_id`'s client sent on
    /// its lane (NETPLAY-06C), for a [`BaselineSender`](crate::BaselineSender).
    pub fn lane_ack(&self, player_id: u16) -> Option<crate::LaneAck> {
        self.lane.latest_ack(player_id)
    }

    /// The newest baseline acknowledgement `player_id`'s client sent for
    /// logical snapshot `group`.
    pub fn lane_group_ack(&self, player_id: u16, group: u8) -> Option<crate::LaneAck> {
        self.lane.group_ack(player_id, group)
    }

    /// The seats whose datagram lane is established — the seats
    /// [`send_snap_states_prio`](Self::send_snap_states_prio) can reach.
    pub fn lane_players(&self) -> Vec<u16> {
        self.lane
            .established_outs()
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// [`broadcast`](Self::broadcast) to only the seats **without** an
    /// established lane (they are on the control stream); a producer that
    /// sends each lane seat its own grouped states uses this for the rest.
    pub fn broadcast_unlaned(&self, msg: &Message) -> Result<(), NetError> {
        let ids = self.lane_players();
        if ids.is_empty() {
            self.send(LoopMsg::Broadcast(msg.clone()))
        } else {
            self.send(LoopMsg::BroadcastExcept(msg.clone(), ids))
        }
    }

    /// Send one tick to `player_id` as **baselined groups** (NETPLAY-10D-2).
    /// Each `(group, Snap)` is a complete `Snap` of `generation`/`tick`
    /// (like [`send_snap_groups_prio`](Self::send_snap_groups_prio)) with a
    /// stable logical `group` id below [`MAX_GROUPS`](crate::MAX_GROUPS):
    /// the group's state is sent as a delta against the newest baseline the
    /// client acknowledged for that group, or in full when there is none,
    /// it is more than [`REFRESH_TICKS`](crate::REFRESH_TICKS) behind or
    /// `epoch` changed. The client reconstructs the `Snap` and acknowledges
    /// it; a lost group, ack or baseline therefore only costs a larger
    /// state, never a wrongly applied one. Bump `epoch` to force full
    /// states (a roster change, a teleport). `Ok(false)` when the seat has
    /// no established lane.
    pub fn send_snap_states_prio(
        &self,
        player_id: u16,
        generation: u64,
        tick: u64,
        epoch: u8,
        groups: &[(u8, Message)],
        priorities: &[crate::Priority],
    ) -> Result<bool, NetError> {
        let mut encoded = Vec::with_capacity(groups.len());
        for (group, g) in groups {
            match g {
                Message::Snap {
                    generation: gg,
                    tick: t,
                    ..
                } if *gg == generation && *t == tick && usize::from(*group) < crate::MAX_GROUPS => {
                    encoded.push((*group, g.encode()?))
                }
                _ => {
                    return Err(NetError::Unexpected(
                        "snapshot state group must be a Snap of the tick with a valid group id",
                    ));
                }
            }
        }
        if (1..encoded.len()).any(|i| encoded[..i].iter().any(|(g, _)| *g == encoded[i].0)) {
            return Err(NetError::Unexpected("duplicate snapshot state group id"));
        }
        let Some(out) = self.lane.established_out(player_id) else {
            return Ok(false);
        };
        let Some(frames) = self
            .lane
            .encode_states(player_id, generation, epoch, tick, &encoded)
        else {
            return Ok(false);
        };
        out.note_tick(tick, Instant::now());
        for (i, (group, _)) in encoded.iter().enumerate() {
            out.note_group_frame(tick, i as u8, *group);
        }
        let slices: Vec<&[u8]> = frames.iter().map(Vec::as_slice).collect();
        let frags = crate::frag::split(tick, &slices).map_err(oversize_snap)?;
        if priorities.is_empty() {
            out.push_fragments(generation, frags);
        } else {
            out.push_fragments_prio(generation, frags, priorities);
        }
        Ok(true)
    }

    /// Host-side baselined-state counters (full against delta, bytes).
    pub fn lane_states(&self) -> crate::StateStats {
        self.lane.state_stats()
    }

    /// [`Host::lane_counts`], from any thread.
    pub fn lane_counts(&self) -> LaneCounts {
        self.lane.counts()
    }

    /// Send `msg` to every client, each at most once, riding the reliable
    /// channel for every seat whose lane is established (NETPLAY-06E-3)
    /// and the control stream for the rest — a TCP-only seat, a body over
    /// [`MAX_RELIABLE_MESSAGE`](crate::MAX_RELIABLE_MESSAGE), and any seat
    /// whose channel refuses it (full or dead)
    /// fall back to TCP, counted in the returned [`ReliableFanout`], so a
    /// refusal never drops the message. `generation` is the session the
    /// lane authenticates it under. The receiver dedups (`Cnr` by
    /// freshness, `Impacts` by `(generation, seat, id)`), so a seat that
    /// somehow sees both copies applies it once.
    pub fn reliable_broadcast(
        &self,
        generation: u64,
        msg: &Message,
    ) -> Result<ReliableFanout, NetError> {
        let bytes = msg.encode()?;
        let mut fanout = ReliableFanout::default();
        let mut by_reliable = Vec::new();
        if bytes.len() <= crate::rt::MAX_RELIABLE_MESSAGE {
            for (id, _) in self.lane.established_outs() {
                match self.lane.send_reliable(id, generation, &bytes) {
                    Ok(()) => {
                        fanout.reliable += 1;
                        by_reliable.push(id);
                    }
                    Err(_) => fanout.fell_back += 1,
                }
            }
        }
        self.send(LoopMsg::BroadcastExcept(msg.clone(), by_reliable))?;
        Ok(fanout)
    }

    /// Send a world-prop `Props` frame to `player_id` (NETPLAY-10C-3).
    /// The host commits its per-client owed and late-join snapshot state
    /// when this returns `Ok`, so a seat with a lane gets it on the
    /// lane's reliable channel (numbered, repeated until acknowledged,
    /// chunked when over one datagram) and **never** on the control
    /// stream: a refusal (window full, dead channel) is an `Err` the
    /// caller retries next frame, so a large catch-up cannot queue behind
    /// or ahead of anything on TCP. A seat without a lane takes the
    /// control stream. The reliable channel is independent of the
    /// `Movement` queue, so a catch-up never holds a healthy peer's
    /// corrections.
    pub fn send_props(
        &self,
        player_id: u16,
        generation: u64,
        msg: &Message,
    ) -> Result<(), NetError> {
        if !matches!(msg, Message::Props { .. }) {
            return Err(NetError::Unexpected("send_props takes a Props frame"));
        }
        let bytes = msg.encode()?;
        if self.lane.established_out(player_id).is_some() {
            return self
                .lane
                .send_reliable(player_id, generation, &bytes)
                .map_err(|_| {
                    NetError::Io(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "reliable channel refused the props frame",
                    ))
                });
        }
        self.send(LoopMsg::SendTo(player_id, msg.clone()))
    }

    /// Send a world-state frame (`Traffic` or `Signals`) to `player_id`
    /// (NETPLAY-10C-3): as [`Priority::Bulk`](crate::Priority) fragments on
    /// the lane when the seat has one — state that self-corrects on the
    /// next frame, so nothing repeats it and the pacer may shed it — and
    /// over the control stream otherwise.
    pub fn send_world_state(&self, player_id: u16, msg: &Message) -> Result<(), NetError> {
        let Some((kind, generation, tick)) = crate::rt::WorldKind::of(msg) else {
            return Err(NetError::Unexpected("not a world-state frame"));
        };
        if let Some(out) = self.lane.established_out(player_id) {
            let bytes = msg.encode()?;
            let frags = crate::frag::split(tick, &[&bytes]).map_err(oversize_snap)?;
            out.push_world(kind, generation, frags);
            return Ok(());
        }
        self.send(LoopMsg::SendTo(player_id, msg.clone()))
    }

    /// [`send_world_state`](Self::send_world_state) to every client.
    pub fn broadcast_world_state(&self, msg: &Message) -> Result<(), NetError> {
        let Some((kind, generation, tick)) = crate::rt::WorldKind::of(msg) else {
            return Err(NetError::Unexpected("not a world-state frame"));
        };
        let outs = self.lane.established_outs();
        if outs.is_empty() {
            return self.send(LoopMsg::Broadcast(msg.clone()));
        }
        let bytes = msg.encode()?;
        let frags = crate::frag::split(tick, &[&bytes]).map_err(oversize_snap)?;
        let ids: Vec<u16> = outs.iter().map(|(id, _)| *id).collect();
        for (_, out) in outs {
            out.push_world(kind, generation, frags.clone());
        }
        self.send(LoopMsg::BroadcastExcept(msg.clone(), ids))
    }

    /// Send `body` to `player_id` on the reliable channel (NETPLAY-06E-1):
    /// numbered and repeated until acknowledged, delivered exactly once
    /// and in order, at most
    /// [`MAX_RELIABLE_MESSAGE`](crate::MAX_RELIABLE_MESSAGE) bytes (split
    /// across datagrams when over
    /// [`MAX_RELIABLE_BODY`](crate::MAX_RELIABLE_BODY)). Takes no `Writer`
    /// lock and touches no socket; a full window,
    /// an oversize body or a dead channel is an `Err`, counted in
    /// [`Host::reliable_stats`].
    pub fn reliable_send(
        &self,
        player_id: u16,
        generation: u64,
        body: &[u8],
    ) -> Result<(), crate::ReliableSendError> {
        self.lane.send_reliable(player_id, generation, body)
    }
}

/// What [`HostCtl::reliable_broadcast`] did, per seat.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReliableFanout {
    /// Seats the message went to on the reliable channel.
    pub reliable: u16,
    /// Lane seats whose channel refused it (full or dead) and got it
    /// over the control stream instead. TCP-only seats are not counted.
    pub fell_back: u16,
}

/// The joining side of a lobby. `join` completes the handshake and the
/// slot assignment; the roster and later lobby traffic arrive through
/// [`Client::recv`].
#[derive(Debug)]
pub struct Client {
    conn: Conn,
    player_id: u16,
    /// The datagram lane, when this client joined with one (NETPLAY-06A):
    /// replies are then read through the merged [`Inbox`], and `backlog`
    /// holds what the host sent while the lane was being negotiated.
    lane: Option<LaneLink>,
    backlog: VecDeque<Message>,
    /// Newest `Start` generation seen (0: none) and newest `Cancel`
    /// generation seen or synthesized — what lets a host-state
    /// keepalive stand in for a `Cancel` every copy of which was lost.
    started: u64,
    cancelled: u64,
}

impl Drop for Client {
    fn drop(&mut self) {
        // A lane client's TCP pump thread holds a clone of the socket, so
        // dropping this handle would not close it; shut it down so the
        // pump (and the host's view of the peer) ends now.
        if self.lane.is_some() {
            self.conn.shutdown_both();
        }
    }
}

fn oversize_snap(_: crate::SplitError) -> NetError {
    NetError::Unexpected("snapshot group exceeds the fragment budget")
}

/// A client's datagram lane plus the merged receive queue it feeds.
#[derive(Debug)]
struct LaneLink {
    lane: ClientLaneGuard,
    inbox: Arc<Inbox>,
    /// `recv` wait bound in milliseconds (0: unbounded) — the lane-mode
    /// stand-in for the socket read timeout, which the TCP reader thread
    /// owns.
    wait_ms: AtomicU64,
}

/// How a joined client's data plane travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// TCP only: `InputWindow` and `Snap` share the ordered control
    /// stream, with its head-of-line blocking. Not equivalent to a lane.
    Tcp,
    /// The UDP datagram lane is established.
    Lane,
}

/// Where a lane client sends its datagrams when a relay sits on the path
/// (see [`Client::join_lane_routed`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneRoute {
    /// The relay's client-facing address: every datagram goes here, and
    /// the host's echoes are accepted only from it.
    pub dial: SocketAddr,
    /// The port the host is told to expect datagrams from — the relay's
    /// host-facing socket.
    pub advertise_port: u16,
}

impl Client {
    /// Connect to `addr`, run the handshake and read the host's
    /// `Welcome` carrying this client's roster slot. `Accept` was only
    /// the compatibility verdict, so the `Welcome` wait gets its own
    /// [`HANDSHAKE_TIMEOUT`] bound — a host may not stall it forever.
    ///
    /// This is the **TCP-only** join: no datagram lane is negotiated and
    /// [`Client::transport`] says [`Transport::Tcp`]. A caller that wants
    /// real-time quality uses [`Client::join_lane`] and surfaces its
    /// failure instead of quietly continuing here.
    pub fn join(addr: SocketAddr, hello: &Hello) -> Result<Self, NetError> {
        let (conn, player_id) = Self::join_tcp(addr, hello)?;
        Ok(Self {
            conn,
            player_id,
            lane: None,
            backlog: VecDeque::new(),
            started: 0,
            cancelled: 0,
        })
    }

    fn join_tcp(addr: SocketAddr, hello: &Hello) -> Result<(Conn, u16), NetError> {
        let mut conn = Conn::connect(addr)?;
        crate::send_hello(&mut conn, hello)?;
        conn.set_timeout(Some(HANDSHAKE_TIMEOUT))?;
        let welcome = conn.recv();
        conn.set_timeout(None)?;
        conn.set_read_timeout(Some(LIVENESS_TIMEOUT))?;
        let player_id = match welcome? {
            Message::Welcome { player_id } => player_id,
            _ => return Err(NetError::Unexpected("expected Welcome")),
        };
        Ok((conn, player_id))
    }

    /// [`Client::join`] plus the UDP datagram lane (NETPLAY-06A), waiting
    /// up to [`LANE_ECHO_TIMEOUT`] for the host's first echo. A blocked or
    /// unreachable UDP path is [`NetError::LaneTimeout`] — a connection
    /// failure naming the port to open, never a silent TCP fallback.
    pub fn join_lane(addr: SocketAddr, hello: &Hello) -> Result<Self, NetError> {
        Self::join_lane_within(addr, hello, LANE_ECHO_TIMEOUT)
    }

    /// [`Client::join_lane`] with an explicit echo deadline.
    pub fn join_lane_within(
        addr: SocketAddr,
        hello: &Hello,
        echo_timeout: Duration,
    ) -> Result<Self, NetError> {
        Self::join_lane_routed(addr, hello, echo_timeout, None)
    }

    /// [`Client::join_lane_within`] with the datagram path steered through
    /// a relay: the client sends its datagrams to `route.dial` instead of
    /// the host's granted lane port, and tells the host to expect them from
    /// `route.advertise_port`. A harness seam for the datagram-boundary
    /// impairment proxy (`UdpImpairProxy`), which cannot sit in front of
    /// the host otherwise because the host authenticates the source
    /// address. `None` is the ordinary direct path.
    pub fn join_lane_routed(
        addr: SocketAddr,
        hello: &Hello,
        echo_timeout: Duration,
        route: Option<LaneRoute>,
    ) -> Result<Self, NetError> {
        let (mut conn, player_id) = Self::join_tcp(addr, hello)?;
        // The UDP socket binds the IP the host sees this connection from,
        // so the host's source-address check holds on multi-homed hosts.
        let socket = UdpSocket::bind(SocketAddr::new(conn.local_addr()?.ip(), 0))?;
        conn.set_timeout(Some(HANDSHAKE_TIMEOUT))?;
        conn.send(&Message::LaneBind {
            port: route.map_or(socket.local_addr()?.port(), |r| r.advertise_port),
        })?;
        // The host's `Session`/`Roster` may arrive before the grant; keep
        // them for `recv` rather than losing the lobby snapshot.
        let mut backlog = VecDeque::new();
        let (lane_port, token) = loop {
            match conn.recv()? {
                Message::LaneGrant { port, token } => break (port, token),
                Message::Keepalive { .. } => {}
                msg if backlog.len() < 64 => backlog.push_back(msg),
                _ => return Err(NetError::Unexpected("expected LaneGrant")),
            }
        };
        conn.set_timeout(None)?;
        conn.set_read_timeout(Some(LIVENESS_TIMEOUT))?;
        let inbox = Inbox::new();
        let lane = ClientLane::establish(
            socket,
            route.map_or(SocketAddr::new(addr.ip(), lane_port), |r| r.dial),
            player_id,
            token,
            inbox.clone(),
            echo_timeout,
        )?;
        spawn_tcp_pump(conn.try_clone()?, inbox.clone());
        Ok(Self {
            conn,
            player_id,
            lane: Some(LaneLink {
                lane: ClientLaneGuard(lane),
                inbox,
                wait_ms: AtomicU64::new(0),
            }),
            backlog,
            started: 0,
            cancelled: 0,
        })
    }

    /// Whether this client's data plane is the datagram lane or plain
    /// TCP. UI and logs should say which: the two are not equivalent
    /// quality.
    pub fn transport(&self) -> Transport {
        if self.lane.is_some() {
            Transport::Lane
        } else {
            Transport::Tcp
        }
    }

    /// Fragment reassembly counters, expiry included (NETPLAY-06C).
    pub fn lane_assembly(&self) -> Option<crate::AssemblerStats> {
        self.lane.as_ref().map(|l| l.lane.0.assembly_stats())
    }

    /// A surfaced connection failure when the lane's full-size path died
    /// after the join ([`NetError::LanePathFailed`]); `None` while healthy
    /// or without a lane. Callers poll it and tear the session down (or
    /// fall back to TCP-only play) instead of waiting on snapshots that
    /// cannot arrive.
    pub fn lane_path_failure(&self) -> Option<NetError> {
        self.lane.as_ref().and_then(|l| l.lane.0.path_failure())
    }

    /// Test seam: probe only when idle, as the keepalive did before the
    /// path probe, so exact datagram counts are not perturbed.
    #[cfg(test)]
    pub(crate) fn set_idle_probe_only(&self) {
        if let Some(l) = &self.lane {
            l.lane.0.set_idle_probe_only();
        }
    }

    /// Test seam: tighten the path probe cadence and failure bound.
    #[cfg(test)]
    pub(crate) fn set_path_bounds(&self, probe_every: Duration, fail_after: Duration) {
        if let Some(l) = &self.lane {
            l.lane.0.set_path_bounds(probe_every, fail_after);
        }
    }

    /// [`ClientCtl::reliable_send`], from the `Client` itself.
    pub fn reliable_send(
        &self,
        generation: u64,
        body: &[u8],
    ) -> Result<(), crate::ReliableSendError> {
        self.lane
            .as_ref()
            .ok_or(crate::ReliableSendError::NoLane)?
            .lane
            .0
            .send_reliable(generation, body)
    }

    /// The next message the host delivered on the reliable channel:
    /// exactly once, in order. `None` when nothing is waiting or on a
    /// TCP-only client.
    pub fn reliable_recv(&self) -> Option<Vec<u8>> {
        self.lane.as_ref()?.lane.0.recv_reliable()
    }

    /// Counters of the reliable channel (both halves); `None` on a
    /// TCP-only client.
    pub fn reliable_stats(&self) -> Option<crate::ReliableStats> {
        self.lane.as_ref().map(|l| l.lane.0.reliable_stats())
    }

    /// See [`Host::use_manual_reliable_clock`]; a no-op without a lane.
    pub fn use_manual_reliable_clock(&self) {
        if let Some(l) = &self.lane {
            l.lane.0.clock().set_manual();
        }
    }

    /// See [`Host::advance_reliable_clock`]; a no-op without a lane.
    pub fn advance_reliable_clock(&self, by: std::time::Duration) {
        if let Some(l) = &self.lane {
            l.lane.0.clock().advance(by);
            l.lane.0.poke();
        }
    }

    /// [`ClientCtl::lane_ack`] from the `Client` itself.
    pub fn lane_ack(&self, generation: u64, epoch: u8, tick: u64) -> bool {
        self.lane
            .as_ref()
            .is_some_and(|l| l.lane.0.send_ack(generation, epoch, tick))
    }

    /// [`ClientCtl::lane_states`], from the `Client` itself.
    pub fn lane_states(&self) -> Option<crate::StateStats> {
        self.lane.as_ref().map(|l| l.lane.0.state_stats())
    }

    /// The lane's counters (accepted datagrams and every kind of drop);
    /// `None` on a TCP-only client.
    pub fn lane_counts(&self) -> Option<LaneCounts> {
        self.lane.as_ref().map(|l| l.lane.0.counts())
    }

    /// Occupancy, drop and age counters of the bounded queue the lane's
    /// receive thread feeds `Snap`s through; `None` on a TCP-only client.
    pub fn lane_inbox_stats(&self) -> Option<QueueStats> {
        self.lane.as_ref().map(|l| l.inbox.lane_stats())
    }

    /// The roster slot the host assigned.
    pub fn player_id(&self) -> u16 {
        self.player_id
    }

    /// Toggle this client's readiness flag; the host rebroadcasts the
    /// roster to everyone.
    pub fn set_ready(&mut self, ready: bool) -> Result<(), NetError> {
        let msg = Message::SetReady { ready };
        if let Some(link) = &self.lane
            && reliable_lobby_send(&link.lane.0, &msg)
        {
            return Ok(());
        }
        self.conn.send(&msg)
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
        if let Some(link) = &self.lane {
            // The reliable copy (NETPLAY-06E-2c) survives a lost datagram;
            // the TCP copy below stays and the host reaps the seat once.
            reliable_lobby_send(&link.lane.0, &Message::Leave);
        }
        self.conn.send(&Message::Leave)?;
        self.conn.shutdown_write();
        if let Some(link) = &self.lane {
            // The TCP pump owns the read side: drain what it queued until
            // the host's close ends it.
            while link.inbox.pop(Some(LEAVE_DRAIN_TIMEOUT)).is_ok() {}
            return Ok(());
        }
        // Read until the host closes (its reader thread exits on `Leave`)
        // or the drain bound expires — either way no inbound data is
        // left pending at drop, so the close stays a clean FIN.
        while self.conn.recv().is_ok() {}
        Ok(())
    }

    /// The next host→client message — a `Roster` broadcast today, more
    /// lobby traffic as later F24-B legs land. `Err` means the
    /// connection to the host is gone.
    /// Keepalives are consumed here: they restart the
    /// [`LIVENESS_TIMEOUT`] clock, which a silent host lets expire
    /// (`Io` with `WouldBlock`/`TimedOut`). One that reports the
    /// session this client entered as no longer running is returned as
    /// the `Cancel` it stands for (#1612): the host's `Cancel` repeat is
    /// time-bounded, the keepalive is not. A keepalive naming an older
    /// generation than the client entered is a stale reorder and ignored.
    pub fn recv(&mut self) -> Result<Message, NetError> {
        loop {
            match self.next_message()? {
                Message::Keepalive {
                    generation,
                    running: false,
                } if self.started > 0
                    && generation >= self.started
                    && generation > self.cancelled =>
                {
                    self.cancelled = generation;
                    if let Some(link) = &self.lane {
                        link.lane.0.retire_through(generation);
                    }
                    return Ok(Message::Cancel { generation });
                }
                Message::Keepalive { .. } => {}
                msg => {
                    match &msg {
                        Message::Start { generation, .. } => {
                            self.started = self.started.max(*generation);
                            if let Some(link) = &self.lane {
                                link.lane.0.note_started(*generation);
                            }
                        }
                        Message::Cancel { generation } => {
                            self.cancelled = self.cancelled.max(*generation);
                            if let Some(link) = &self.lane {
                                link.lane.0.retire_through(*generation);
                            }
                        }
                        _ => {}
                    }
                    return Ok(msg);
                }
            }
        }
    }

    /// The next message off the control stream — or, on a lane client,
    /// off the merged inbox (reliable messages first, then lane `Snap`s).
    fn next_message(&mut self) -> Result<Message, NetError> {
        if let Some(msg) = self.backlog.pop_front() {
            return Ok(msg);
        }
        match &self.lane {
            Some(link) => {
                let ms = link.wait_ms.load(Ordering::Relaxed);
                link.inbox.pop((ms > 0).then(|| Duration::from_millis(ms)))
            }
            None => self.conn.recv(),
        }
    }

    /// Bound `recv`/`set_ready` waits — tests install a short backstop so
    /// a broken peer cannot hang the suite.
    pub fn set_timeout(&self, timeout: Option<Duration>) -> Result<(), NetError> {
        if let Some(link) = &self.lane {
            // The socket's read timeout belongs to the TCP pump thread
            // (liveness); a `recv` bound applies to the merged inbox.
            let ms = timeout.map_or(0, |t| {
                u64::try_from(t.as_millis()).unwrap_or(u64::MAX).max(1)
            });
            link.wait_ms.store(ms, Ordering::Relaxed);
            self.conn.set_write_timeout(timeout)
        } else {
            self.conn.set_timeout(timeout)
        }
    }

    /// A send-side handle for threads other than the one blocked in
    /// [`Client::recv`] — the client's counterpart of [`Host::ctl`],
    /// so a consumer that reads lobby traffic on one thread can answer
    /// it from another (a stdin driver, a Bevy system). The handle
    /// serializes sends so two callers cannot interleave a frame, and
    /// installs [`WRITE_TIMEOUT`] — socket options are shared, so the
    /// bound then also covers this `Client`'s own sends: a peer that
    /// stops reading bounds a send rather than stalling it forever.
    ///
    /// Sends through the handle are queued to a writer thread
    /// (NETPLAY-06E-4): they never wait on the socket, and a peer so far
    /// behind that its bounded queue fills gets `Err` from every later
    /// send. They are ordered among themselves but not against the
    /// `Client`'s own blocking `send`/`leave`, so a caller that needs an
    /// order sends both through the handle.
    pub fn ctl(&self) -> Result<ClientCtl, NetError> {
        let writer = self.conn.writer()?;
        writer.set_write_timeout(Some(WRITE_TIMEOUT))?;
        // Queued and sent from the writer thread: a send from the game
        // thread never waits on the socket (NETPLAY-06E-4).
        let writer = PeerWriter::spawn(writer)?;
        Ok(ClientCtl {
            writer: Arc::new(Mutex::new(writer)),
            lane: self.lane.as_ref().map(|l| l.lane.0.clone()),
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
    writer: Arc<Mutex<PeerWriter>>,
    lane: Option<Arc<ClientLane>>,
}

/// Queue a lobby-scope message on the lane's reliable channel
/// (NETPLAY-06E-2c); `false` when the channel refused it (full window,
/// dead channel) and the caller must use the control stream.
fn reliable_lobby_send(lane: &ClientLane, msg: &Message) -> bool {
    msg.encode()
        .is_ok_and(|body| lane.send_reliable(crate::rt::LOBBY_SCOPE, &body).is_ok())
}

impl ClientCtl {
    /// [`Client::lane_path_failure`], from any thread: the pump owns the
    /// `Client`, the app polls this.
    pub fn lane_path_failure(&self) -> Option<NetError> {
        self.lane.as_ref().and_then(|l| l.path_failure())
    }

    fn send(&self, msg: &Message) -> Result<(), NetError> {
        // A poisoned mutex only means a send panicked mid-write — the
        // socket is still usable, so recover the guard rather than
        // propagate a poisoning that says nothing about the wire.
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        writer.send(msg)
    }

    /// The pending-I/O queue of this client's control stream
    /// (NETPLAY-10A): frames queued for the writer thread and not yet
    /// written, with the age of the oldest. Not the lane's queue
    /// ([`lane_queue`](Self::lane_queue)) and not retained history.
    pub fn io_queue(&self) -> PeerQueueStats {
        self.writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stats()
    }

    /// [`Client::set_ready`], from any thread.
    pub fn set_ready(&self, ready: bool) -> Result<(), NetError> {
        let msg = Message::SetReady { ready };
        // NETPLAY-06E-2c: with a lane the flag rides the reliable channel
        // alone (in order with the other reliable lobby intents, so a stale
        // copy never lands after a newer one); a refusal falls back to TCP.
        if let Some(lane) = &self.lane
            && reliable_lobby_send(lane, &msg)
        {
            return Ok(());
        }
        self.send(&msg)
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

    /// Send one redundant window of fixed-tick commands (v29,
    /// NETPLAY-02A) — the client→host data-plane verb of the
    /// fixed-tick timeline. Bounds are enforced by the codec.
    ///
    /// On a lane client (NETPLAY-06A) the window goes through the lane's
    /// bounded nonblocking queue to the sender thread: this call takes no
    /// `Writer` lock and touches no socket. A window over the datagram
    /// budget, or sent after the lane closed, uses the control stream.
    pub fn send_input_window(
        &self,
        generation: u64,
        epoch: u8,
        first_seq: u64,
        commands: Vec<TickCommand>,
    ) -> Result<(), NetError> {
        let msg = Message::InputWindow {
            generation,
            epoch,
            first_seq,
            commands,
        };
        if let Some(lane) = &self.lane
            && let Some(bytes) = lane_payload(&msg)?
            && lane.send_data(generation, bytes)
        {
            return Ok(());
        }
        self.send(&msg)
    }

    /// The lane's counters; `None` on a TCP-only client.
    pub fn lane_counts(&self) -> Option<LaneCounts> {
        self.lane.as_ref().map(|l| l.counts())
    }

    /// Send `body` to the host on the reliable channel (NETPLAY-06E-1):
    /// numbered and repeated until acknowledged, delivered exactly once
    /// and in order. Takes no `Writer` lock and touches no socket.
    /// `Err(NoLane)` on a TCP-only client.
    pub fn reliable_send(
        &self,
        generation: u64,
        body: &[u8],
    ) -> Result<(), crate::ReliableSendError> {
        self.lane
            .as_ref()
            .ok_or(crate::ReliableSendError::NoLane)?
            .send_reliable(generation, body)
    }

    /// Acknowledge the state at (`epoch`, `tick`) of `generation` to the
    /// host over the lane (NETPLAY-06C) after applying it; the host only
    /// ever deltas against an acknowledged baseline. `false` on a
    /// TCP-only client or a closed lane. Unreliable: repeat it with each
    /// newer tick.
    pub fn lane_ack(&self, generation: u64, epoch: u8, tick: u64) -> bool {
        self.lane
            .as_ref()
            .is_some_and(|l| l.send_ack(generation, epoch, tick))
    }

    /// Client-side baselined-state counters (applied, refused for a
    /// missing base); `None` on a TCP-only client.
    pub fn lane_states(&self) -> Option<crate::StateStats> {
        self.lane.as_ref().map(|l| l.state_stats())
    }

    /// The lane's outbound queue bounds, age and drop counters; `None` on
    /// a TCP-only client.
    pub fn lane_queue(&self) -> Option<QueueStats> {
        self.lane.as_ref().map(|l| l.out_stats())
    }

    /// Ask the authority to reset this client's own seat (F25-B) —
    /// the predicted-session answer to the local `R` bundle, which a
    /// `Remote` session must never fire itself. Fire-and-forget like
    /// `Input`: the answer, if granted, is the next `Snap` carrying the
    /// seat's bumped `epoch` — no reply message exists. `generation`
    /// must be the session the sender is driving in; the host drops a
    /// request minted against anything else.
    pub fn request_reset(&self, generation: u64) -> Result<(), NetError> {
        let msg = Message::ResetRequest { generation };
        // NETPLAY-06E-3: with a lane the ask rides the reliable channel
        // (acked, repeated, delivered once); a refusal — no lane, full
        // window, dead channel — falls back to the control stream, which
        // the host's mailbox absorbs the same way.
        if let Some(lane) = &self.lane
            && lane.send_reliable(generation, &msg.encode()?).is_ok()
        {
            return Ok(());
        }
        self.send(&msg)
    }

    /// A clean quit from another thread: the host records
    /// [`LeaveCause::Quit`] and closes the socket, which ends the
    /// owner's blocked [`Client::recv`] — unlike [`Client::leave`] this
    /// cannot drain, so the owner finishes the close by reading the
    /// socket to its end (or dropping the client).
    pub fn leave(&self) -> Result<(), NetError> {
        if let Some(lane) = &self.lane {
            // Alongside the TCP copy, not instead of it (NETPLAY-06E-2c).
            reliable_lobby_send(lane, &Message::Leave);
        }
        // Queue it, then wait (bounded) for the writer thread to put it
        // on the wire: a process that exits right after this call would
        // otherwise take the thread, and the Quit, with it. The writer
        // stays open so a caller can repeat the Leave.
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        writer.send(&Message::Leave)?;
        if writer.flush(TEARDOWN_FLUSH) {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::TimedOut, "Leave was not flushed").into())
        }
    }
}

/// The lane client's TCP reader: the control stream's messages (and its
/// terminal error) join the lane's `Snap`s in the one inbox `Client::recv`
/// drains. A read timeout is the liveness clock firing, not the end of
/// the stream, so it is reported (once) and reading continues.
fn spawn_tcp_pump(conn: Conn, inbox: Arc<Inbox>) {
    thread::spawn(move || {
        let mut conn = conn;
        loop {
            match conn.recv() {
                Ok(msg) => {
                    if !inbox.push_tcp(Ok(msg)) {
                        return;
                    }
                }
                Err(NetError::Io(e))
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    if !inbox.push_tcp_timeout(e) {
                        return;
                    }
                }
                Err(e) => {
                    inbox.push_tcp(Err(e));
                    inbox.close();
                    return;
                }
            }
        }
    });
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
    /// `HostCtl::send_to` — a data-plane send to one player.
    SendTo(u16, Message),
    /// `HostCtl::broadcast` of a `Snap` already queued on the datagram
    /// lane of the listed seats: TCP to everyone else.
    BroadcastExcept(Message, Vec<u16>),
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
    /// The TCP peer's IP — what the seat's UDP lane must come from.
    peer: IpAddr,
    writer: PeerWriter,
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
    tx: SyncSender<LoopMsg>,
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
    tx: SyncSender<LoopMsg>,
    rx: Receiver<LoopMsg>,
    events: EventTx,
    (inputs, lane): (RemoteInputs, Arc<HostLane>),
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

    let mut last_sent = Instant::now();
    // `Start`/`Cancel` are each a single frame — a client that misses
    // one sits in the wrong phase — so the loop re-sends the newest of
    // each on the keepalive tick until `now` passes the deadline
    // (F31-B.4 / #1489). A fresh start or cancel supersedes the other.
    // A `Start` also clears the moment every rostered peer has shown it
    // entered the generation, so an acknowledged start stops resending.
    let mut start_resend: Option<(u64, Instant)> = None;
    let mut cancel_resend: Option<(u64, Instant)> = None;
    loop {
        // The consumer stopped draining events: end the lobby (explicit
        // failure) rather than queue without bound or wait on it.
        if events.overflowed() {
            break;
        }
        // Wake for the next keepalive even when no message arrives.
        let wait = KEEPALIVE_INTERVAL.saturating_sub(last_sent.elapsed());
        // NETPLAY-06E-2c: a `Leave`/`SetReady` the reliable channel
        // delivered is handled exactly like the TCP copy (a second copy
        // of either is a no-op: the seat is gone / the flag is set), in
        // delivery order ahead of the next channel message. The lane
        // thread cannot wake this loop, so the wait is cut to a short
        // poll while the keepalive clock keeps its own deadline.
        let msg = if let Some((id, msg)) = inputs.take_intent() {
            match msg {
                Message::Leave => LoopMsg::PeerGone {
                    id,
                    cause: LeaveCause::Quit,
                },
                msg => LoopMsg::PeerMessage { id, msg },
            }
        } else {
            match rx.recv_timeout(wait.min(INTENT_POLL)) {
                Err(RecvTimeoutError::Timeout) if last_sent.elapsed() < KEEPALIVE_INTERVAL => {
                    continue;
                }
                other => match other {
                    Ok(msg) => msg,
                    Err(RecvTimeoutError::Timeout) => {
                        last_sent = Instant::now();
                        // The roster rides every keepalive tick too: a removal's
                        // one-shot broadcast can be lost on a lossy link, and a
                        // survivor would keep the departed peer's seat until the
                        // next membership change. Resending the (idempotent,
                        // newest-wins) snapshot bounds that to one tick. A failed
                        // keepalive write needs no extra pass: `broadcast_roster`
                        // below reports the corrected roster either way.
                        for id in inputs.silent_lane_seats(config.liveness_timeout) {
                            remove_player(&mut players, id, LeaveCause::Lost, &events, &inputs);
                        }
                        // NETPLAY-10A: a peer that reads nothing is cut loose
                        // by the age of its oldest unwritten frame, however
                        // short its queue is. Only this peer is affected.
                        for (id, stats) in inputs.stalled_io_seats(config.peer_stall_limit) {
                            eprintln!(
                                "host: seat {id} not reading, disconnecting: io_queue len={} \
                                 oldest_age_ms={} max_wait_ms={} coalesced={} refused={}",
                                stats.len,
                                stats.oldest_age.as_millis(),
                                stats.max_wait.as_millis(),
                                stats.coalesced,
                                stats.refused,
                            );
                            remove_player(&mut players, id, LeaveCause::Lost, &events, &inputs);
                        }
                        let running = matches!(phase, Phase::InSession { .. });
                        broadcast(
                            &mut players,
                            &Message::Keepalive {
                                generation,
                                running,
                            },
                            &events,
                            &inputs,
                        );
                        // The lobby's advertised next session is as one-shot as
                        // a removal: a client that missed the host's edit would
                        // keep (and be started on) the stale ad. Newest-wins and
                        // idempotent on the client, so the lobby repeats it each
                        // tick; a running session's peers are not re-told (the
                        // ad that *started* it is theirs already).
                        if matches!(phase, Phase::Lobby)
                            && let Some(ad) = &session
                        {
                            broadcast(
                                &mut players,
                                &Message::Session(ad.clone()),
                                &events,
                                &inputs,
                            );
                        }
                        // F31-B.4 / #1489: the one-shot lifecycle frames repeat
                        // too, bounded rather than forever — a receiver ignores a
                        // repeat (its generation only climbs). A `Start` stops
                        // once every peer has shown it entered the session.
                        let now = Instant::now();
                        if let Phase::InSession {
                            generation: g,
                            session: started,
                            ..
                        } = &phase
                        {
                            match start_resend {
                                Some((rg, until)) if rg == *g && now < until => {
                                    if players.keys().all(|id| inputs.entered(*id, *g)) {
                                        start_resend = None;
                                    } else {
                                        let msg = Message::Start {
                                            generation: *g,
                                            session: started.clone(),
                                            host_pick: config.host_pick.clone(),
                                        };
                                        broadcast(&mut players, &msg, &events, &inputs);
                                    }
                                }
                                Some(_) => start_resend = None,
                                None => {}
                            }
                        } else if let Some((rg, until)) = cancel_resend {
                            if now < until {
                                broadcast(
                                    &mut players,
                                    &Message::Cancel { generation: rg },
                                    &events,
                                    &inputs,
                                );
                            } else {
                                cancel_resend = None;
                            }
                        }
                        broadcast_roster(&mut players, &events, &inputs, None);
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                },
            }
        };
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
                // Handshake writes above stayed blocking; from here every
                // send to the seat is queued so one peer that stops
                // reading cannot hold up the others.
                let Ok(writer) = PeerWriter::spawn(writer) else {
                    continue;
                };
                inputs.register_io(id, writer.probe());
                players.insert(
                    id,
                    Slot {
                        driver: hello.driver.clone(),
                        build: hello.build.clone(),
                        ready: false,
                        pick: None,
                        peer: conn.peer_addr().ip(),
                        writer,
                    },
                );
                let _ = events.send(HostEvent::Joined {
                    id,
                    driver: hello.driver,
                    build: hello.build,
                });
                broadcast_roster(&mut players, &events, &inputs, Some(&lane));
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
                        broadcast_roster(&mut players, &events, &inputs, Some(&lane));
                    } else if players.contains_key(&id) {
                        queue_lifecycle(&lane, [id], &msg);
                        // The newcomer's own `Start` is as one-shot as the
                        // original broadcast was, and it may join long
                        // after that start's window closed — re-arm the
                        // bounded repeat on admission so a dropped copy
                        // still lands (F31-B.4 / #1489). It stops once
                        // every peer (the newcomer included) has shown
                        // liveness in the generation, or the window ends.
                        start_resend =
                            Some((*generation, Instant::now() + LIFECYCLE_RESEND_WINDOW));
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
                    broadcast_roster(&mut players, &events, &inputs, Some(&lane));
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
                                broadcast_roster(&mut players, &events, &inputs, Some(&lane));
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
                                broadcast_roster(&mut players, &events, &inputs, Some(&lane));
                            }
                        }
                    }
                }
            }
            LoopMsg::PeerMessage {
                id,
                msg: Message::LaneBind { port },
            } => {
                // The client opened a datagram lane: mint the seat's
                // token and hand it over the TCP connection it belongs
                // to. A dead grant write is a dead peer, like any send.
                let failed = players.get_mut(&id).is_some_and(|slot| {
                    let token = lane.register(id, slot.peer, port);
                    slot.writer
                        .send(&Message::LaneGrant {
                            port: lane.port(),
                            token,
                        })
                        .is_err()
                });
                if failed && remove_player(&mut players, id, LeaveCause::Lost, &events, &inputs) {
                    broadcast_roster(&mut players, &events, &inputs, Some(&lane));
                }
            }
            LoopMsg::PeerMessage { .. } => {}
            LoopMsg::SetSession(ad) => {
                session = Some(ad.clone());
                // Removals under the session send change the roster too —
                // survivors get the corrected snapshot after the ad.
                if !broadcast(&mut players, &Message::Session(ad), &events, &inputs) {
                    broadcast_roster(&mut players, &events, &inputs, Some(&lane));
                }
            }
            LoopMsg::Broadcast(msg) => {
                // The data-plane send (`Snap`) follows the same removal
                // discipline as every other broadcast: a peer that fails
                // the write is reaped `Lost` and survivors get the
                // corrected roster.
                if !broadcast(&mut players, &msg, &events, &inputs) {
                    broadcast_roster(&mut players, &events, &inputs, Some(&lane));
                }
            }
            LoopMsg::BroadcastExcept(msg, skip) => {
                if !broadcast_except(&mut players, &msg, &skip, &events, &inputs) {
                    broadcast_roster(&mut players, &events, &inputs, Some(&lane));
                }
            }
            LoopMsg::SendTo(id, msg) => {
                let failed = players
                    .get_mut(&id)
                    .is_some_and(|slot| slot.writer.send(&msg).is_err());
                if failed {
                    remove_player(&mut players, id, LeaveCause::Lost, &events, &inputs);
                    broadcast_roster(&mut players, &events, &inputs, Some(&lane));
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
                        lane.set_generation(generation);
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
                            broadcast_roster(&mut players, &events, &inputs, Some(&lane));
                        }
                        queue_lifecycle(&lane, players.keys().copied(), &msg);
                        // Re-send `Start` until every peer is in the
                        // session or the window elapses (F31-B.4 /
                        // #1489). A new session supersedes any pending
                        // `Cancel` re-send: the repeat is a duplicate the
                        // receiver ignores, never the phase it wants.
                        start_resend = Some((generation, Instant::now() + LIFECYCLE_RESEND_WINDOW));
                        cancel_resend = None;
                        let _ = events.send(HostEvent::Started { generation });
                    }
                }
            }
            LoopMsg::Cancel => {
                if let Phase::InSession { generation, .. } = phase {
                    phase = Phase::Lobby;
                    lane.set_generation(0);
                    let cancel = Message::Cancel { generation };
                    broadcast(&mut players, &cancel, &events, &inputs);
                    queue_lifecycle(&lane, players.keys().copied(), &cancel);
                    // Back in the lobby a fresh start wants fresh
                    // consent — readiness resets (designed); picks stay.
                    for slot in players.values_mut() {
                        slot.ready = false;
                    }
                    let _ = events.send(HostEvent::Cancelled { generation });
                    // Departures under the `Cancel` send and the
                    // readiness reset both changed the roster.
                    broadcast_roster(&mut players, &events, &inputs, Some(&lane));
                    // Keep re-sending `Cancel` for a bounded window
                    // (F31-B.4 / #1489): the single frame may be lost, and
                    // a client still in the ended session would never
                    // return to the lobby. A fresh start supersedes it.
                    cancel_resend = Some((generation, Instant::now() + LIFECYCLE_RESEND_WINDOW));
                    start_resend = None;
                }
            }
            LoopMsg::PeerGone { id, cause } => {
                if remove_player(&mut players, id, cause, &events, &inputs) {
                    broadcast_roster(&mut players, &events, &inputs, Some(&lane));
                }
            }
        }
    }

    // Teardown: close every peer socket (wakes the reader threads, whose
    // sends into the dead channel just fail), drop the input mailbox and
    // stop the accept thread, which sees the flag within one
    // `ACCEPT_POLL`.
    // Frames still queued (a final `Cancel`) get one shared bounded
    // chance to leave before the sockets close under them.
    for slot in players.values_mut() {
        slot.writer.close_send();
    }
    let flush_by = Instant::now() + TEARDOWN_FLUSH;
    while players.values().any(|slot| !slot.writer.is_drained()) && Instant::now() < flush_by {
        thread::sleep(Duration::from_millis(2));
    }
    for slot in players.values_mut() {
        slot.writer.disconnect();
    }
    inputs.clear();
    stop.store(true, Ordering::Relaxed);
    let _ = accept.join();
}

/// Mint the lowest-unused-slot id: monotonic until wraparound, never 0,
/// never an occupied slot, never in the host-owned range from
/// [`FIRST_RESERVED_PLAYER_ID`] up. Terminates because the roster is
/// bounded.
fn alloc_id(players: &BTreeMap<u16, Slot>, next_id: &mut u16) -> u16 {
    loop {
        let id = *next_id;
        *next_id = if id + 1 >= FIRST_RESERVED_PLAYER_ID {
            1
        } else {
            id + 1
        };
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
    events: &EventTx,
    inputs: &RemoteInputs,
) -> bool {
    let Some(mut slot) = players.remove(&id) else {
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
    events: &EventTx,
    inputs: &RemoteInputs,
) -> bool {
    broadcast_except(players, msg, &[], events, inputs)
}

/// The coalescing key of a message a later copy fully supersedes, so a
/// seat that is not reading holds one copy of each instead of a backlog
/// (NETPLAY-10A). `Start`, `Cancel` and `Leave` are one-shot and never
/// coalesce; neither does `Snap`, whose impact rows are events that a
/// later snapshot does not repeat.
fn replace_key(msg: &Message) -> Option<u8> {
    match msg {
        Message::Keepalive { .. } => Some(1),
        Message::Session(_) => Some(2),
        Message::Roster { .. } => Some(3),
        _ => None,
    }
}

/// [`broadcast`] skipping the seats in `skip` — those already got `msg`
/// on the datagram lane.
fn broadcast_except(
    players: &mut BTreeMap<u16, Slot>,
    msg: &Message,
    skip: &[u16],
    events: &EventTx,
    inputs: &RemoteInputs,
) -> bool {
    let mut failed = Vec::new();
    for (id, slot) in players.iter_mut().filter(|(id, _)| !skip.contains(id)) {
        let sent = match replace_key(msg) {
            Some(key) => slot.writer.send_replaceable(key, msg),
            None => slot.writer.send(msg),
        };
        if sent.is_err() {
            failed.push(*id);
        }
    }
    let removed = !failed.is_empty();
    for id in failed {
        remove_player(players, id, LeaveCause::Lost, events, inputs);
    }
    !removed
}

/// Queue a session `Start` or `Cancel` on each seat's reliable channel at
/// the lobby scope (NETPLAY-06E-2b), in addition to the TCP copy. Lobby
/// scope because the lane generation changes under these messages (`Start`
/// raises it, `Cancel` clears it): generation 0 is always admitted, so a
/// repeat or an ack never races the change, and the channel's order puts a
/// `Cancel` after its `Start`. A body over one datagram is split across
/// several by the channel (an oversize `Start`, #1930) and applies once
/// when whole. A TCP-only seat, a full window or a dead channel is skipped
/// and counted in its `ReliableStats`.
fn queue_lifecycle(lane: &HostLane, seats: impl IntoIterator<Item = u16>, msg: &Message) {
    let Ok(body) = msg.encode() else { return };
    for id in seats {
        let _ = lane.send_reliable(id, crate::rt::LOBBY_SCOPE, &body);
    }
}

/// Send the complete roster to every player, rebuilt and resent after
/// each removal — a survivor must never see a snapshot still listing a
/// departed peer. Each pass removes at least one player, so the resend
/// loop terminates.
///
/// With a `lane` (a roster *change*, NETPLAY-06E-2a) the final snapshot is
/// also queued on every rostered seat's reliable channel at the lobby
/// scope, so a lane client learns of a join or a removal even when the
/// datagram is lost; a TCP-only seat has no lane and is skipped. The
/// keepalive tick passes `None`: its TCP resend stays, and it must not
/// refill a stalled peer's bounded reliable window every second.
fn broadcast_roster(
    players: &mut BTreeMap<u16, Slot>,
    events: &EventTx,
    inputs: &RemoteInputs,
    lane: Option<&HostLane>,
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
            if let Some(lane) = lane
                && let Ok(body) = msg.encode()
            {
                for &id in players.keys() {
                    // `NoLane`: a TCP-only peer. A full or dead channel
                    // is counted in its `ReliableStats`; the TCP copy
                    // and the keepalive resend still cover the peer.
                    let _ = lane.send_reliable(id, crate::rt::LOBBY_SCOPE, &body);
                }
            }
            return;
        }
    }
}

/// One thread per player: forward `SetReady`/`SetVehicle` to the loop,
/// absorb `Input` into the shared mailbox (data-plane traffic — the loop
/// is not woken per sample), report `Leave` as a clean quit, and drop
/// the peer on a protocol violation or a dead socket.
fn spawn_reader(conn: Conn, id: u16, tx: SyncSender<LoopMsg>, inputs: RemoteInputs) {
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
                Ok(Message::InputWindow {
                    generation,
                    epoch,
                    first_seq,
                    commands,
                }) => {
                    // Data-plane like `Input`: absorbed into the seat's
                    // bounded command buffer, never forwarded.
                    inputs.store_window(id, generation, epoch, first_seq, &commands);
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
                Ok(
                    msg @ (Message::SetReady { .. }
                    | Message::SetVehicle(_)
                    | Message::LaneBind { .. }),
                ) => LoopMsg::PeerMessage { id, msg },
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
    fn minted_ids_wrap_before_the_host_owned_range() {
        let players = BTreeMap::new();
        let mut next = FIRST_RESERVED_PLAYER_ID - 2;
        let minted: Vec<u16> = (0..4).map(|_| alloc_id(&players, &mut next)).collect();
        assert_eq!(
            minted,
            vec![
                FIRST_RESERVED_PLAYER_ID - 2,
                FIRST_RESERVED_PLAYER_ID - 1,
                1,
                2
            ]
        );
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
            liveness_timeout: HOST_LIVENESS_TIMEOUT,
            peer_stall_limit: PEER_STALL_LIMIT,
            uplink_bytes_per_sec: HOST_UPLINK_BYTES_PER_SEC,
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
                    peer: conn.peer_addr().ip(),
                    writer: PeerWriter::spawn(conn.writer().unwrap()).unwrap(),
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
        let (events, rx) = EventTx::bounded();
        let inputs = RemoteInputs::default();

        broadcast_roster(&mut players, &events, &inputs, None);

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

    /// NETPLAY-06E-4: a seat whose peer stopped reading does not hold
    /// up the others. Its parked writer fills its bounded queue, the
    /// next broadcast reaps it `Lost`, and the healthy seat still gets
    /// the message — all without the host loop waiting on a socket.
    #[test]
    fn a_stalled_seat_is_reaped_without_delaying_the_others() {
        fn seat(id: u16) -> (Slot, TcpStream) {
            let listener = listen_loopback().unwrap();
            let far = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let conn = Conn::accept(&listener).unwrap();
            let slot = Slot {
                driver: format!("p{id}"),
                build: "b".to_string(),
                ready: false,
                pick: None,
                peer: conn.peer_addr().ip(),
                writer: PeerWriter::spawn(conn.writer().unwrap()).unwrap(),
            };
            (slot, far)
        }
        let (stalled, _stalled_far) = seat(1);
        let (healthy, healthy_far) = seat(2);
        let mut players = BTreeMap::from([(1, stalled), (2, healthy)]);
        let big = vec![0u8; 200 * 1024];
        while players
            .get_mut(&1)
            .unwrap()
            .writer
            .send_frame(big.clone())
            .is_ok()
        {}
        let (events, rx) = EventTx::bounded();
        let inputs = RemoteInputs::default();

        let start = Instant::now();
        let all_delivered = broadcast(&mut players, &Message::Accept, &events, &inputs);
        assert!(start.elapsed() < WAIT, "broadcast waited on a socket");

        assert!(!all_delivered);
        assert_eq!(players.keys().copied().collect::<Vec<_>>(), [2]);
        match rx.recv_timeout(WAIT) {
            Ok(HostEvent::Left {
                id: 1,
                cause: LeaveCause::Lost,
                ..
            }) => {}
            other => panic!("expected the stalled seat Lost, got {other:?}"),
        }
        let mut far = Conn::from_stream(healthy_far).unwrap();
        far.set_timeout(Some(WAIT)).unwrap();
        assert_eq!(far.recv().unwrap(), Message::Accept);
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
            liveness_timeout: HOST_LIVENESS_TIMEOUT,
            peer_stall_limit: PEER_STALL_LIMIT,
            uplink_bytes_per_sec: HOST_UPLINK_BYTES_PER_SEC,
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
            liveness_timeout: HOST_LIVENESS_TIMEOUT,
            peer_stall_limit: PEER_STALL_LIMIT,
            uplink_bytes_per_sec: HOST_UPLINK_BYTES_PER_SEC,
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
        let (tx, rx) = mpsc::sync_channel(LOOP_QUEUE_CAP);
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
        let (tx, rx) = mpsc::sync_channel(LOOP_QUEUE_CAP);
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

    /// F31-B.4 / #1489: the admission `Start` is the last one-shot copy —
    /// a newcomer can join long after the original start's window closed.
    /// Admission re-arms the bounded repeat so a newcomer that missed the
    /// single copy still begins.
    #[test]
    fn an_admitted_late_joiner_gets_a_repeated_start() {
        let host = sessioned_host();
        let mut alice = join_sessioned(&host, "alice");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 1);
        ready_up(&host, &mut alice, "vpbug");
        host.start(LateJoin::Open).unwrap();
        host.recv_timeout(WAIT).unwrap(); // Started
        assert_eq!(recv_start(&mut alice), (1, ad("go")));

        // Outlast the start's own window: from here only a fresh re-arm
        // on admission can produce another `Start`.
        thread::sleep(LIFECYCLE_RESEND_WINDOW + KEEPALIVE_INTERVAL);

        let mut bob = join(host.addr(), "bob");
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Joined { id: 2, .. }) => {}
            other => panic!("expected Joined, got {other:?}"),
        }
        match bob.recv().unwrap() {
            Message::Session(_) => {}
            other => panic!("expected Session, got {other:?}"),
        }
        recv_roster(&mut bob, 2);
        assert_eq!(recv_start(&mut bob), (1, ad("go")));
        // The re-armed repeat — the copy a newcomer that missed the only
        // admission `Start` would still see.
        bob.set_timeout(Some(KEEPALIVE_INTERVAL * 3)).unwrap();
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
        // The `Start` may be re-sent over the window, and the session ad
        // repeats in the lobby — skip to the `Cancel`.
        loop {
            match alice.recv().unwrap() {
                Message::Cancel { generation: 1 } => break,
                Message::Start { .. } | Message::Roster { .. } | Message::Session(_) => continue,
                other => panic!("expected Cancel, got {other:?}"),
            }
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

    /// F31-B.4 / #1489: `Start` is a single frame, so a client that
    /// misses it never begins. The host re-sends it on the keepalive
    /// tick until every peer shows it entered the session (an `Input` /
    /// `ResetRequest` for the generation), so an acknowledged start
    /// stops resending.
    #[test]
    fn a_lost_start_is_resent_until_the_peer_enters() {
        let host = sessioned_host();
        let mut alice = join_sessioned(&host, "alice");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 1);
        ready_up(&host, &mut alice, "vpbug");
        host.start(LateJoin::Closed).unwrap();
        host.recv_timeout(WAIT).unwrap(); // Started

        // The frame a peer missing the first copy would still see.
        assert_eq!(recv_start(&mut alice), (1, ad("go")));
        alice.set_timeout(Some(KEEPALIVE_INTERVAL * 3)).unwrap();
        assert_eq!(recv_start(&mut alice), (1, ad("go")));

        // The peer shows it is in the session, so the re-send stops.
        alice.ctl().unwrap().send_input(drive_input(1)).unwrap();
        let landed = Instant::now() + WAIT;
        while !host.remote_inputs().entered(1, 1) {
            assert!(Instant::now() < landed, "the input never landed");
            thread::sleep(Duration::from_millis(2));
        }
        // A repeat sent by a tick that fired just before the input
        // landed can still be in flight; drain one tick of it, then the
        // stream goes quiet of `Start`.
        alice.set_timeout(Some(KEEPALIVE_INTERVAL)).unwrap();
        let grace = Instant::now() + KEEPALIVE_INTERVAL;
        while Instant::now() < grace {
            let _ = alice.recv();
        }
        let quiet = Instant::now() + KEEPALIVE_INTERVAL * 2;
        while Instant::now() < quiet {
            if let Ok(Message::Start { .. }) = alice.recv() {
                panic!("the Start was re-sent after the peer entered the session");
            }
        }
    }

    /// F31-B.4 / #1489: `Cancel` is a single frame too, so the host
    /// re-sends it for a bounded window — a client that missed it would
    /// otherwise stay in the ended session forever.
    #[test]
    fn a_lost_cancel_is_resent_for_a_bounded_window() {
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
        // The first `Cancel` may be preceded by a `Start` re-send still
        // in flight and is followed by the lobby's ad/roster traffic.
        alice.set_timeout(Some(KEEPALIVE_INTERVAL * 3)).unwrap();
        let mut cancels = 0;
        let deadline = Instant::now() + KEEPALIVE_INTERVAL * 3;
        while cancels < 2 && Instant::now() < deadline {
            match alice.recv().unwrap() {
                Message::Cancel { generation: 1 } => cancels += 1,
                Message::Start { .. } | Message::Roster { .. } | Message::Session(_) => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(cancels, 2, "the Cancel was not re-sent");
    }

    /// The predicate the `Start` re-send terminates on: a seat counts as
    /// entered a generation when its newest `Input` or `ResetRequest`
    /// names it — a sample from the previous session does not.
    #[test]
    fn entered_tracks_the_running_generation() {
        let inputs = RemoteInputs::default();
        inputs.store(1, drive_input(1));
        assert!(inputs.entered(1, 1));
        assert!(!inputs.entered(1, 2), "an old generation is not liveness");
        inputs.request_reset(2, 5);
        assert!(inputs.entered(2, 5));
        assert!(!inputs.entered(2, 4));
        // A departed seat's liveness goes with it.
        inputs.remove(1);
        assert!(!inputs.entered(1, 1));
    }

    /// A `Leave` reaps the seat in a *running* session too — the
    /// removal path runs on `Phase::InSession` exactly as on the lobby,
    /// so the app client's retried `Leave` (NET-LEAVE-LOSSY) frees a
    /// ghost seat mid-match and the survivor's roster drops it.
    #[test]
    fn a_mid_session_leave_frees_the_seat_and_rebroadcasts() {
        let host = sessioned_host();
        let mut alice = join_sessioned(&host, "alice");
        let mut bob = join_sessioned(&host, "bob");
        host.recv_timeout(WAIT).unwrap(); // Joined alice
        host.recv_timeout(WAIT).unwrap(); // Joined bob
        recv_roster(&mut alice, 2);
        recv_roster(&mut bob, 2);
        ready_up(&host, &mut alice, "vpbug");
        ready_up(&host, &mut bob, "vpsemi");
        host.start(LateJoin::Closed).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Started { generation: 1 }) => {}
            other => panic!("expected Started{{1}}, got {other:?}"),
        }
        recv_start(&mut alice);
        recv_start(&mut bob);

        // Bob quits the running session.
        bob.leave().unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Left {
                id: 2,
                cause: LeaveCause::Quit,
                ..
            }) => {}
            other => panic!("expected a Quit Left, got {other:?}"),
        }
        // The survivor's roster no longer lists the departed seat.
        let roster = recv_roster(&mut alice, 1);
        assert_eq!(roster[0].driver, "alice");
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

    /// F26-AC04: ambient traffic flows host → client only too — a client
    /// that submits a traffic frame is dropped, never absorbed.
    #[test]
    fn a_client_cannot_assert_traffic() {
        let host = sessioned_host();
        let mut mallory = join_sessioned(&host, "mallory");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut mallory, 1);
        mallory
            .send(&Message::Traffic {
                generation: 1,
                tick: 1,
                roster: 0,
                rows: vec![crate::proto::SnapCar {
                    id: 0,
                    class: 0,
                    state: 0,
                    pos: [0.0; 3],
                    rot: [0.0, 0.0, 0.0, 1.0],
                    vel: [0.0; 3],
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

    /// F26-AC04: the world clock flows host → client only — a client
    /// that submits one is dropped, never absorbed.
    #[test]
    fn a_client_cannot_assert_the_world_clock() {
        let host = sessioned_host();
        let mut mallory = join_sessioned(&host, "mallory");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut mallory, 1);
        mallory
            .send(&Message::World {
                generation: 1,
                ticks: 1,
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

    /// F26-A.4: proximity drawbridge leaves flow host → client only — a
    /// client that submits a frame is dropped, never absorbed.
    #[test]
    fn a_client_cannot_assert_drawbridge_leaves() {
        let host = sessioned_host();
        let mut mallory = join_sessioned(&host, "mallory");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut mallory, 1);
        mallory
            .send(&Message::Leaves {
                generation: 1,
                tick: 1,
                rows: Vec::new(),
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

    /// F26-A.4b: the traffic-signal phase flows host → client only — a
    /// client that asserts one is dropped, never absorbed.
    #[test]
    fn a_client_cannot_assert_the_signal_phase() {
        let host = sessioned_host();
        let mut mallory = join_sessioned(&host, "mallory");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut mallory, 1);
        mallory
            .send(&Message::Signals {
                generation: 1,
                tick: 1,
                rows: Vec::new(),
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

    /// F27-AC05: the Cops & Robbers match flows host → client only — a
    /// client that submits a frame (a pickup, a steal, a score) is
    /// dropped, never absorbed.
    #[test]
    fn a_client_cannot_assert_the_cops_and_robbers_match() {
        let host = sessioned_host();
        let mut mallory = join_sessioned(&host, "mallory");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut mallory, 1);
        mallory
            .send(&Message::Cnr {
                generation: 1,
                frame: crate::proto::SnapCnr {
                    variant: 0,
                    end: 0,
                    end_value: 0,
                    round: 9,
                    revision: u64::MAX,
                    elapsed: 0,
                    gold: 1,
                    holder: 1,
                    at: [0.0; 3],
                    free_at: 0,
                    sites: [[0.0; 3]; 3],
                    outcome: None,
                    seats: vec![crate::proto::SnapCnrSeat {
                        player: 1,
                        side: 0,
                        connected: true,
                        score: u32::MAX,
                    }],
                },
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

    /// F26-AC04, the headline case: a client that submits a snapshot
    /// claiming its own seat finished first with every gate cleared,
    /// and a race row saying the race is over, is dropped — the claim
    /// is never absorbed into the mailbox or reported to the consumer.
    /// Progress and results are the authority's to derive from its own
    /// sim of the seat; a client's only voice is its `Input`.
    #[test]
    fn a_client_cannot_award_itself_a_finish() {
        let host = sessioned_host();
        let mut mallory = join_sessioned(&host, "mallory");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut mallory, 1);
        mallory
            .send(&Message::Snap {
                generation: 1,
                tick: 1,
                entries: vec![SnapEntry {
                    player: 1,
                    pos: [0.0; 3],
                    rot: [0.0, 0.0, 0.0, 1.0],
                    vel: [0.0; 3],
                    angvel: [0.0; 3],
                    epoch: 0,
                    steer: 0,
                    spin: 0,
                    compression: 0,
                    flags: 0,
                    damage: 0,
                    breaks: 0,
                    prog_state: 2,
                    prog_ticks: 1,
                    prog_lap: u32::MAX,
                    prog_next: 0,
                    prog_cleared: u64::MAX,
                    prog_crossings: u32::MAX,
                    prog_route_clears: u32::MAX,
                    rpm: 0,
                    surf_skid: SNAP_NO_SURFACE,
                    skid_slip: 0,
                    skid_speed: 0,
                    surf_roll: SNAP_NO_SURFACE,
                    input_ack: 0,
                    input_depth: 0,
                }],
                trailers: Vec::new(),
                impacts: Vec::new(),
                race: Some(crate::proto::SnapRace {
                    phase: u8::MAX,
                    countdown: 0,
                    clock: 1,
                }),
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
        // Nothing the dropped peer said reached the consumer, and the
        // socket is closed on it.
        assert!(host.try_recv().is_err());
        assert!(host.remote_inputs().latest(1).is_none());
        assert!(mallory.recv().is_err());
    }

    #[test]
    fn an_idle_lobby_repeats_its_advertised_session() {
        let host = sessioned_host();
        let mut alice = join_sessioned(&host, "alice");
        // The join already told her once (consumed above); a lost edit
        // is repaired by the next keepalive tick, so a second copy
        // arrives unprompted.
        alice.set_timeout(Some(KEEPALIVE_INTERVAL * 4)).unwrap();
        let again = loop {
            match alice.recv().unwrap() {
                Message::Session(got) => break got,
                _ => continue,
            }
        };
        assert_eq!(again, ad("go"));
    }

    #[test]
    fn an_idle_lobby_keeps_speaking_and_a_client_swallows_it() {
        let host = sessioned_host();
        let mut alice = join_sessioned(&host, "alice");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 1);
        // A read bound shorter than the idle stretch we wait out: only
        // the host's keepalives (swallowed by `recv`) keep each read
        // inside it, so the first message `recv` returns is the one
        // sent after the lobby sat idle for several bounds.
        alice.set_timeout(Some(KEEPALIVE_INTERVAL * 2)).unwrap();
        let ctl = host.ctl();
        let sender = thread::spawn(move || {
            thread::sleep(KEEPALIVE_INTERVAL * 4);
            ctl.broadcast(&Message::World {
                generation: 1,
                ticks: 9,
            })
            .unwrap();
        });
        // The host resends the (idempotent) roster on each keepalive tick
        // too (NET-ROSTER-LOSSY), and so does the lobby's advertised
        // session, so those ride in between; the first message that is
        // not one is the broadcast.
        let first = loop {
            match alice.recv().unwrap() {
                Message::Roster { .. } | Message::Session(_) => continue,
                other => break other,
            }
        };
        assert_eq!(
            first,
            Message::World {
                generation: 1,
                ticks: 9
            }
        );
        sender.join().unwrap();
    }

    /// F26-AC04 as a table: the client→host set is exactly `SetReady`,
    /// `SetVehicle`, `Leave`, `Input` and `ResetRequest`. Every other
    /// verb, sent by a rostered client, drops it `Malformed` — a client
    /// can neither answer for the host (handshake or lifecycle verbs)
    /// nor re-handshake mid-lobby. A new wire verb that is not on the
    /// reader's allowlist is refused by default; this pins the ones
    /// that exist.
    #[test]
    fn every_host_side_verb_sent_by_a_client_drops_it() {
        let hello_again = hello("b".to_string(), "mallory".to_string(), FP);
        let spoofs = [
            ("Hello", Message::Hello(hello_again)),
            ("Accept", Message::Accept),
            (
                "Reject",
                Message::Reject {
                    code: RejectCode::LobbyFull,
                    message: "go away".to_string(),
                },
            ),
            ("Welcome", Message::Welcome { player_id: 0 }),
            ("Session", Message::Session(ad("spoofed"))),
            ("Roster", Message::Roster { players: vec![] }),
            (
                "VehicleRefused",
                Message::VehicleRefused {
                    reason: "no".to_string(),
                },
            ),
            (
                "Start",
                Message::Start {
                    generation: 1,
                    session: ad("spoofed"),
                    host_pick: None,
                },
            ),
            ("Cancel", Message::Cancel { generation: 1 }),
            (
                "Keepalive",
                Message::Keepalive {
                    generation: 1,
                    running: false,
                },
            ),
        ];
        for (name, spoof) in spoofs {
            let host = sessioned_host();
            let mut mallory = join_sessioned(&host, "mallory");
            host.recv_timeout(WAIT).unwrap();
            recv_roster(&mut mallory, 1);
            mallory.send(&spoof).unwrap();
            match host.recv_timeout(WAIT) {
                Ok(HostEvent::Left {
                    id: 1,
                    cause: LeaveCause::Malformed,
                    ..
                }) => {}
                other => panic!("{name}: expected a Malformed Left, got {other:?}"),
            }
            assert!(host.try_recv().is_err(), "{name}: nothing else surfaced");
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

    fn tick_cmd(n: u8) -> TickCommand {
        TickCommand {
            throttle: n,
            brake: 0,
            steer: 0,
            handbrake: 0,
            auto_reverse: false,
            forced_gear: None,
        }
    }

    #[test]
    fn windows_are_dropped_until_a_session_runs_and_for_other_generations() {
        let inputs = RemoteInputs::default();
        let t = Instant::now();
        inputs.store_window_at(1, 5, 0, 1, &[tick_cmd(1)], t);
        assert!(inputs.consume_command_at(1, t).is_none(), "no session yet");
        assert!(
            inputs.entered(1, 5),
            "a window still shows the peer entered"
        );
        inputs.set_running_generation(Some(5));
        inputs.store_window_at(1, 4, 0, 1, &[tick_cmd(9)], t);
        assert_eq!(inputs.seat_stats(1).unwrap().wrong_generation, 1);
        inputs.store_window_at(1, 5, 0, 1, &[tick_cmd(1)], t);
        let c = inputs.consume_command_at(1, t).unwrap();
        assert_eq!((c.command.throttle, c.finalized), (1, Some(1)));
        assert_eq!(inputs.command_ack(1), 1);
    }

    #[test]
    fn command_depth_counts_unconsumed_commands() {
        let inputs = RemoteInputs::default();
        let t = Instant::now();
        inputs.set_running_generation(Some(1));
        assert_eq!(inputs.command_depth(1), 0);
        inputs.store_window_at(1, 1, 0, 1, &[tick_cmd(1), tick_cmd(2), tick_cmd(3)], t);
        assert_eq!(inputs.command_depth(1), 3);
        inputs.consume_command_at(1, t);
        assert_eq!(inputs.command_depth(1), 2);
    }

    #[test]
    fn generation_change_resets_every_seat_and_ack() {
        let inputs = RemoteInputs::default();
        let t = Instant::now();
        inputs.set_running_generation(Some(1));
        inputs.store_window_at(1, 1, 0, 1, &[tick_cmd(1), tick_cmd(2)], t);
        inputs.consume_command_at(1, t);
        assert_eq!(inputs.command_ack(1), 1);
        inputs.set_running_generation(Some(2));
        assert_eq!(inputs.command_ack(1), 0);
        assert!(inputs.seat_stats(1).is_none());
        // The old session's leftover window cannot revive it.
        inputs.store_window_at(1, 1, 0, 2, &[tick_cmd(2)], t);
        assert_eq!(inputs.seat_stats(1).unwrap().received, 0);
    }

    #[test]
    fn seats_are_independent() {
        let inputs = RemoteInputs::default();
        let t = Instant::now();
        inputs.set_running_generation(Some(1));
        inputs.store_window_at(1, 1, 0, 1, &[tick_cmd(1)], t);
        inputs.store_window_at(2, 1, 0, 1, &[tick_cmd(7), tick_cmd(8)], t);
        assert_eq!(inputs.consume_command_at(2, t).unwrap().command.throttle, 7);
        assert_eq!(
            inputs.command_ack(1),
            0,
            "seat 2 consuming never acks seat 1"
        );
        // A flooding seat spends only its own bucket.
        for k in 0..64 {
            inputs.store_window_at(2, 1, 0, 100 + 16 * k, &[tick_cmd(0); 16], t);
        }
        assert!(inputs.seat_stats(2).unwrap().rate_limited > 0);
        assert_eq!(inputs.seat_stats(1).unwrap().rate_limited, 0);
        inputs.remove(2);
        assert!(inputs.seat_stats(2).is_none());
        assert_eq!(inputs.consume_command_at(1, t).unwrap().finalized, Some(1));
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

    /// A flood far above the bound lands only the burst plus the
    /// refill, the rest are counted and dropped — never queued — and a
    /// quiet interval restores the budget. Deterministic: explicit
    /// arrival instants, no sleeping.
    #[test]
    fn an_input_flood_is_bounded_and_counted() {
        let inputs = RemoteInputs::default();
        let t0 = Instant::now();
        // 1000 samples inside one millisecond: the burst lands, the rest drop.
        for seq in 1..=1000u64 {
            inputs.store_at(1, drive_input(seq), t0 + Duration::from_micros(seq));
        }
        let landed = u64::from(INPUT_BURST);
        assert_eq!(inputs.rate_limited(), 1000 - landed);
        assert_eq!(inputs.latest(1).unwrap().input.seq, landed);
        // A second seat has its own budget.
        inputs.store_at(2, drive_input(1), t0);
        assert_eq!(inputs.rate_limited(), 1000 - landed);
        // At the sustained rate nothing is dropped.
        let step = Duration::from_secs(1) / INPUT_RATE_HZ;
        let base = t0 + Duration::from_secs(10);
        for k in 0..240u64 {
            inputs.store_at(1, drive_input(2000 + k), base + step * k as u32);
        }
        assert_eq!(inputs.rate_limited(), 1000 - landed);
        assert_eq!(inputs.latest(1).unwrap().input.seq, 2239);
        // The map stays bounded by the roster ceiling.
        for id in 10..40u16 {
            inputs.store_at(id, drive_input(1), base);
        }
        assert!(inputs.input_len() <= MAX_PLAYERS as usize);
        // A departing seat's budget goes with it.
        inputs.remove(1);
        for seq in 1..=u64::from(INPUT_BURST) {
            inputs.store_at(1, drive_input(seq), base);
        }
        assert_eq!(inputs.latest(1).unwrap().input.seq, u64::from(INPUT_BURST));
    }

    fn drive_input(seq: u64) -> DriveInput {
        DriveInput {
            generation: 1,
            seq,
            throttle: 200,
            brake: 40,
            steer: -90,
            handbrake: 0,
            auto_reverse: true,
            forced_gear: None,
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
        // all three inputs — queue order is the happens-before, so the
        // ready flag goes through the same handle.
        ctl.set_ready(true).unwrap();
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
                input_ack: 0,
                input_depth: 0,
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

        let ctl = alice.ctl().unwrap();
        ctl.request_reset(3).unwrap();
        // Same handle as the ask, so the `Leave` cannot overtake it.
        ctl.leave().unwrap();
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
