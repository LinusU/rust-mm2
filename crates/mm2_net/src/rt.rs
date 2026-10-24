//! The UDP datagram lane (NETPLAY-06A): a sequenced, unreliable channel
//! beside the TCP control connection, carrying the traffic where the
//! newest packet is the only one that matters — the client's
//! `InputWindow` stream and the host's replaceable `Snap`.
//!
//! **Negotiation rides TCP.** A joining client binds a UDP socket and
//! sends `Message::LaneBind { port }`; the host answers on the same
//! connection with `Message::LaneGrant { port, token }` — its own UDP
//! port and a per-seat secret token. No datagram can create a lane, so an
//! off-path sender never learns a token. The client then probes until
//! the host echoes ([`LANE_ECHO_TIMEOUT`]); silence is a
//! [`NetError::LaneTimeout`] naming the UDP port to open.
//!
//! **Every datagram is authenticated** against the seat table before it
//! can touch gameplay state: magic and version, seat, token, source
//! address (the TCP peer's IP and the announced port), session
//! generation, and a 64-wide replay window over the datagram sequence.
//! Anything else is dropped and counted in [`LaneCounts`]; none of it can
//! reach [`RemoteInputs`](crate::RemoteInputs). The token is a SipHash of
//! OS-seeded keys, not a cryptographic secret and not encryption:
//! datagrams are authenticated by possession of the token, not signed,
//! and an on-path attacker who can read them can forge them.
//!
//! **Sockets live on dedicated threads.** The game thread only pushes
//! into, or pops from, a bounded nonblocking [`LaneQueue`] that drops the
//! oldest entry on overflow and counts it; it never locks a `Writer` or
//! waits on a socket for realtime traffic.
//!
//! Wire header (little-endian, 32 bytes) then the payload, the whole
//! datagram at most [`MAX_DATAGRAM`] bytes:
//! `magic[4] "MM2R" | version u8 | tag u8 | generation u64 | seat u16 |
//! token u64 | seq u64`. The payload of a [`LaneTag::Data`] datagram is a
//! complete encoded `Message`; a [`LaneTag::Probe`] carries none; a
//! [`LaneTag::Fragment`] carries one piece of a snapshot larger than a
//! datagram and a [`LaneTag::Ack`] a baseline acknowledgement (NETPLAY-06C,
//! [`crate::frag`]). [`LaneTag::Reliable`] / [`LaneTag::ReliableAck`] carry
//! the bounded ack/repeat/dedup channel of [`crate::reliable`]
//! (NETPLAY-06E-1): datagrams that must arrive, in order, exactly once.

use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::congest::{
    Adapt, CONGESTED_ACK_AGE, Claim, DEFAULT_PEER_CEILING, Feedback, PeerController, Quantiles,
    Uplink, effective_rate, wire_bytes,
};
use crate::frag::{
    Assembler, AssemblerStats, BaselineReceiver, BaselineSender, MAX_GROUPS, Plan, StateError,
    decode_ack, decode_state_frame, encode_ack, encode_state_frame,
};
use crate::lobby::{PeerMail, RemoteInputs};
use crate::pace::{
    BULK_MAX_AGE, BULK_QUEUE_CAP, Budget, MOVEMENT_MAX_AGE, MOVEMENT_QUEUE_CAP, PEER_BURST_BYTES,
    PEER_BYTES_PER_SEC, Priority,
};
use crate::proto::Message;
use crate::reliable::{
    ReliableChannel, ReliableStats, SendError, decode_ack as decode_rel_ack, decode_data,
};
use crate::{NetError, ProtoError};

/// First four bytes of every lane datagram.
pub const MAGIC: [u8; 4] = *b"MM2R";
/// The lane header layout version, independent of `PROTOCOL_VERSION`.
pub const LANE_VERSION: u8 = 1;
/// Header size in bytes.
pub const HEADER_LEN: usize = 32;
/// Largest datagram either side sends or accepts, header included: under
/// a 1280-byte IPv6 minimum MTU with room for IP/UDP headers (designed).
pub const MAX_DATAGRAM: usize = 1200;
/// Largest payload a datagram carries.
pub const MAX_PAYLOAD: usize = MAX_DATAGRAM - HEADER_LEN;
/// Entries each bounded lane queue holds before it drops the oldest.
pub const QUEUE_CAP: usize = 32;
/// How many datagram sequences behind the newest one a seat's replay
/// window still accepts (a late reordered datagram inside it is fine).
pub const SEQ_WINDOW: u64 = 64;
/// How long a lane join waits for the host's first echo before the
/// connection result is [`NetError::LaneTimeout`].
pub const LANE_ECHO_TIMEOUT: Duration = Duration::from_secs(3);

/// A probe and its echo are padded to the largest datagram either side
/// ever sends, so the establishing round trip proves the path carries
/// [`MAX_DATAGRAM`] bytes both ways. A path whose effective MTU is lower
/// (a tunnel, a filtering middlebox) would otherwise establish on tiny
/// probes and then silently lose every full-size `Snap`; padded, it is a
/// [`NetError::LaneTimeout`] at join, naming the cause.
fn probe_padding() -> Vec<u8> {
    vec![0; MAX_PAYLOAD]
}

const PROBE_INTERVAL: Duration = Duration::from_millis(100);
/// How often an established client sends a full-size probe whatever else
/// it is sending: the host echoes it at full size, so an echo proves the
/// path still carries [`MAX_DATAGRAM`] bytes both ways (a path MTU that
/// drops after the join would otherwise lose every fragment silently).
pub const PATH_PROBE_INTERVAL: Duration = Duration::from_secs(1);
/// A client that has heard no echo for this long reports
/// [`NetError::LanePathFailed`]: about five full-size probes in a row
/// lost, which no usable path does by accident. Shorter than the host's
/// `HOST_LIVENESS_TIMEOUT`, so the client names the cause before the host
/// reaps the seat.
pub const PATH_FAILURE_AFTER: Duration = Duration::from_secs(5);
/// Socket read timeout and queue wait: how often a lane thread notices
/// its stop flag.
const POLL: Duration = Duration::from_millis(50);
/// How soon the host's sender looks again while a seat has datagrams
/// waiting for credit.
const BACKLOG_POLL: Duration = Duration::from_millis(2);

/// The millisecond clock reliable-message retransmission runs on: wall
/// time, or — for the lockstep lane tests — a manual one the test
/// advances, so a retransmission happens when the test says and a seed
/// replays the same exchange. Switch to manual before the first reliable
/// send; the two scales are not comparable.
#[derive(Debug)]
pub(crate) struct ReliableClock {
    origin: Instant,
    manual: AtomicBool,
    manual_ms: AtomicU64,
}

impl ReliableClock {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            manual: AtomicBool::new(false),
            manual_ms: AtomicU64::new(0),
        }
    }

    fn now_ms(&self) -> u64 {
        if self.manual.load(Ordering::Relaxed) {
            self.manual_ms.load(Ordering::Relaxed)
        } else {
            u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX)
        }
    }

    pub(crate) fn set_manual(&self) {
        self.manual.store(true, Ordering::Relaxed);
    }

    pub(crate) fn advance(&self, by: Duration) {
        let ms = u64::try_from(by.as_millis()).unwrap_or(u64::MAX);
        self.manual_ms.fetch_add(ms, Ordering::Relaxed);
    }
}

/// Largest reliable message piece: a datagram payload less the data
/// header. A longer message is split into pieces (see the `reliable`
/// module) up to [`MAX_RELIABLE_MESSAGE`].
pub const MAX_RELIABLE_BODY: usize = MAX_PAYLOAD - crate::reliable::DATA_HEADER;
/// Largest reliable message: [`MAX_RELIABLE_PIECES`](crate::MAX_RELIABLE_PIECES)
/// pieces.
pub const MAX_RELIABLE_MESSAGE: usize = MAX_RELIABLE_BODY * crate::reliable::MAX_RELIABLE_PIECES;
/// TCP messages the client's merged inbox holds before the TCP reader
/// thread waits (backpressure on the reliable side only).
const TCP_INBOX_CAP: usize = 1024;

/// What a datagram is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneTag {
    /// Lane establishment: the client's probe and the host's echo.
    Probe = 1,
    /// An encoded `Message`.
    Data = 2,
    /// One fragment of a snapshot too large for a datagram (host to
    /// client); see [`crate::frag`].
    Fragment = 3,
    /// The client's acknowledgement of a state it applied (client to
    /// host); see [`crate::frag`].
    Ack = 4,
    /// One reliable message, numbered and repeated until acknowledged
    /// (either direction); see [`crate::reliable`].
    Reliable = 5,
    /// Cumulative and selective acknowledgement of [`LaneTag::Reliable`]
    /// messages (either direction).
    ReliableAck = 6,
    /// One fragment of an ambient-traffic frame ([`WorldKind::Cars`],
    /// host to client). Its own tag, so its tick domain and its
    /// assembler never mix with a `Snap`'s.
    WorldCars = 7,
    /// One fragment of a traffic-signal frame ([`WorldKind::Signals`],
    /// host to client).
    WorldSignals = 8,
}

/// The world-state messages that ride the lane as [`Priority::Bulk`]
/// fragment groups (NETPLAY-10C-3). *State*, not events: a lost frame
/// self-corrects on the next, so neither is repeated by the transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldKind {
    /// `Message::Traffic`, the ambient cars.
    Cars,
    /// `Message::Signals`, the traffic-light phases.
    Signals,
}

impl WorldKind {
    pub(crate) const ALL: [Self; 2] = [Self::Cars, Self::Signals];

    /// The kind of `msg`, `None` for everything else.
    pub(crate) fn of(msg: &Message) -> Option<(Self, u64, u64)> {
        match msg {
            Message::Traffic {
                generation, tick, ..
            } => Some((Self::Cars, *generation, *tick)),
            Message::Signals {
                generation, tick, ..
            } => Some((Self::Signals, *generation, *tick)),
            _ => None,
        }
    }

    pub(crate) fn tag(self) -> LaneTag {
        match self {
            Self::Cars => LaneTag::WorldCars,
            Self::Signals => LaneTag::WorldSignals,
        }
    }

    fn from_tag(tag: LaneTag) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.tag() == tag)
    }

    fn index(self) -> usize {
        self as usize
    }

    /// The generation `msg` carries when it is this kind.
    fn generation_of(self, msg: &Message) -> Option<u64> {
        Self::of(msg).and_then(|(k, g, _)| (k == self).then_some(g))
    }
}

impl LaneTag {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Probe),
            2 => Some(Self::Data),
            3 => Some(Self::Fragment),
            4 => Some(Self::Ack),
            5 => Some(Self::Reliable),
            6 => Some(Self::ReliableAck),
            7 => Some(Self::WorldCars),
            8 => Some(Self::WorldSignals),
            _ => None,
        }
    }
}

/// A decoded datagram header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// What the datagram is.
    pub tag: LaneTag,
    /// The session generation the sender claims (0 for probes).
    pub generation: u64,
    /// The roster seat the lane belongs to.
    pub seat: u16,
    /// The seat's host-minted token.
    pub token: u64,
    /// Per-direction datagram sequence, starting at 1.
    pub seq: u64,
}

/// Serialise one datagram; `None` when `payload` exceeds [`MAX_PAYLOAD`].
pub fn encode_datagram(h: &Header, payload: &[u8]) -> Option<Vec<u8>> {
    if payload.len() > MAX_PAYLOAD {
        return None;
    }
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&MAGIC);
    out.push(LANE_VERSION);
    out.push(h.tag as u8);
    out.extend_from_slice(&h.generation.to_le_bytes());
    out.extend_from_slice(&h.seat.to_le_bytes());
    out.extend_from_slice(&h.token.to_le_bytes());
    out.extend_from_slice(&h.seq.to_le_bytes());
    out.extend_from_slice(payload);
    Some(out)
}

/// Parse a datagram into its header and payload; `None` when it is not a
/// well-formed lane datagram (short, oversize, wrong magic, version or
/// tag, or sequence 0).
pub fn decode_datagram(buf: &[u8]) -> Option<(Header, &[u8])> {
    if buf.len() < HEADER_LEN || buf.len() > MAX_DATAGRAM || buf[..4] != MAGIC {
        return None;
    }
    if buf[4] != LANE_VERSION {
        return None;
    }
    let tag = LaneTag::from_u8(buf[5])?;
    let u64_at = |at: usize| u64::from_le_bytes(buf[at..at + 8].try_into().unwrap());
    let header = Header {
        tag,
        generation: u64_at(6),
        seat: u16::from_le_bytes([buf[14], buf[15]]),
        token: u64_at(16),
        seq: u64_at(24),
    };
    if header.seq == 0 {
        return None;
    }
    Some((header, &buf[HEADER_LEN..]))
}

/// A sliding replay window over datagram sequences: a sequence is
/// accepted once, and only while it is within [`SEQ_WINDOW`] of the
/// newest accepted one.
#[derive(Debug, Clone, Copy, Default)]
pub struct SeqWindow {
    top: u64,
    seen: u64,
}

impl SeqWindow {
    /// Record `seq`; `false` when it is a duplicate or too old.
    pub fn accept(&mut self, seq: u64) -> bool {
        if seq == 0 {
            return false;
        }
        if seq > self.top {
            let shift = seq - self.top;
            self.seen = if shift >= 64 { 0 } else { self.seen << shift };
            self.seen |= 1;
            self.top = seq;
            return true;
        }
        let behind = self.top - seq;
        if behind >= SEQ_WINDOW {
            return false;
        }
        let bit = 1u64 << behind;
        if self.seen & bit != 0 {
            return false;
        }
        self.seen |= bit;
        true
    }
}

/// Lifetime counters for one lane endpoint. Every authenticated-or-not
/// datagram lands in exactly one of `accepted` or a drop counter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaneCounts {
    /// Datagrams read off the socket.
    pub received: u64,
    /// Datagrams that passed every check and were delivered.
    pub accepted: u64,
    /// Datagrams written to the socket.
    pub sent: u64,
    /// Not a well-formed lane datagram (or an undecodable payload).
    pub malformed: u64,
    /// A seat id with no lane.
    pub unknown_seat: u64,
    /// Right seat, wrong token.
    pub bad_token: u64,
    /// Right token, wrong source address.
    pub wrong_addr: u64,
    /// A generation other than the running one.
    pub wrong_generation: u64,
    /// A duplicate or too-old sequence.
    pub replayed: u64,
    /// A well-formed message this direction does not carry.
    pub unexpected: u64,
    /// Socket send failures.
    pub send_errors: u64,
    /// Messages too large for a datagram, sent over TCP instead.
    pub oversize_tcp: u64,
}

#[derive(Debug, Default)]
pub(crate) struct Counters {
    received: AtomicU64,
    accepted: AtomicU64,
    sent: AtomicU64,
    malformed: AtomicU64,
    unknown_seat: AtomicU64,
    bad_token: AtomicU64,
    wrong_addr: AtomicU64,
    wrong_generation: AtomicU64,
    replayed: AtomicU64,
    unexpected: AtomicU64,
    send_errors: AtomicU64,
    oversize_tcp: AtomicU64,
}

impl Counters {
    fn bump(c: &AtomicU64) {
        c.fetch_add(1, Ordering::Relaxed);
    }

    fn drop_for(&self, why: Refuse) {
        Self::bump(match why {
            Refuse::Malformed => &self.malformed,
            Refuse::UnknownSeat => &self.unknown_seat,
            Refuse::BadToken => &self.bad_token,
            Refuse::WrongAddr => &self.wrong_addr,
            Refuse::WrongGeneration => &self.wrong_generation,
            Refuse::Replayed => &self.replayed,
            Refuse::Unexpected => &self.unexpected,
        });
    }

    pub(crate) fn note_oversize_tcp(&self) {
        Self::bump(&self.oversize_tcp);
    }

    pub(crate) fn snapshot(&self) -> LaneCounts {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        LaneCounts {
            received: get(&self.received),
            accepted: get(&self.accepted),
            sent: get(&self.sent),
            malformed: get(&self.malformed),
            unknown_seat: get(&self.unknown_seat),
            bad_token: get(&self.bad_token),
            wrong_addr: get(&self.wrong_addr),
            wrong_generation: get(&self.wrong_generation),
            replayed: get(&self.replayed),
            unexpected: get(&self.unexpected),
            send_errors: get(&self.send_errors),
            oversize_tcp: get(&self.oversize_tcp),
        }
    }
}

/// Why a datagram was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refuse {
    Malformed,
    UnknownSeat,
    BadToken,
    WrongAddr,
    WrongGeneration,
    Replayed,
    Unexpected,
}

/// The newest acknowledgement a seat's client sent: it applied the state
/// at (`epoch`, `tick`) of `generation` (NETPLAY-06C).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneAck {
    /// The session generation.
    pub generation: u64,
    /// The reset epoch the state belongs to.
    pub epoch: u8,
    /// The logical snapshot group the state belongs to (each group keeps
    /// its own baselines).
    pub group: u8,
    /// The server tick of the acknowledged state.
    pub tick: u64,
}

/// Why [`HostCtl::reliable_send`](crate::HostCtl) /
/// [`ClientCtl::reliable_send`](crate::ClientCtl) refused a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReliableSendError {
    /// No lane for that seat (or no lane at all on a TCP-only client).
    #[error("no datagram lane for the reliable channel")]
    NoLane,
    /// The channel refused it: full, too large or dead.
    #[error("{0}")]
    Channel(#[from] SendError),
}

/// A bounded queue's occupancy and history.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueStats {
    /// Entries queued now.
    pub len: usize,
    /// The bound.
    pub capacity: usize,
    /// Entries ever pushed.
    pub pushed: u64,
    /// Entries discarded because the queue was full (oldest first).
    pub dropped: u64,
    /// How long the oldest queued entry has waited.
    pub oldest_age: Duration,
    /// The longest any popped entry waited in the queue.
    pub max_wait: Duration,
    /// Entries discarded unsent because they outlived the queue's
    /// maximum age (NETPLAY-06D).
    pub expired: u64,
}

/// A seat's outbound pacing counters (NETPLAY-06D).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PaceStats {
    /// The movement queue.
    pub movement: QueueStats,
    /// The bulk queue.
    pub bulk: QueueStats,
    /// Movement datagrams sent.
    pub movement_sent: u64,
    /// Bulk datagrams sent.
    pub bulk_sent: u64,
    /// Of those, the world-state datagrams (ambient cars, signals) that
    /// spent the separate world allowance.
    pub world_sent: u64,
    /// Payload bytes sent, both priorities.
    pub bytes_sent: u64,
    /// Times the bulk queue's head waited for budget credit.
    pub bulk_deferred: u64,
    /// The most bytes the seat's budget was ever overdrawn by.
    pub max_debt: u64,
    /// Wire bytes sent (payload, lane header, UDP and IP), both
    /// priorities: what the host's uplink paid for this seat.
    pub wire_sent: u64,
    /// The congestion controller's current rate, wire bytes per second.
    pub rate: u32,
    /// The most the rate may be.
    pub ceiling: u32,
    /// The seat's share of the host uplink, wire bytes per second.
    pub share: u32,
    /// Times the controller cut the rate.
    pub decreases: u64,
    /// Polls in which the movement head had to wait for rate or uplink.
    pub movement_deferred: u64,
    /// Of those, the polls the aggregate uplink refused.
    pub uplink_denied: u64,
    /// How the traffic to the seat adapts at this rate.
    pub adapt: Adapt,
    /// Movement-queue wait of sent datagrams: median, 95th, 99th
    /// percentile and maximum over the last few thousand.
    pub wait_p50: Duration,
    /// See `wait_p50`.
    pub wait_p95: Duration,
    /// See `wait_p50`.
    pub wait_p99: Duration,
    /// See `wait_p50`.
    pub wait_max: Duration,
}

#[derive(Debug)]
struct Bounded<T> {
    items: VecDeque<(Instant, T)>,
    cap: usize,
    pushed: u64,
    dropped: u64,
    expired: u64,
    max_wait: Duration,
    /// How long the entry popped last had waited.
    last_wait: Duration,
}

impl<T> Bounded<T> {
    fn new(cap: usize) -> Self {
        Self {
            items: VecDeque::new(),
            cap,
            pushed: 0,
            dropped: 0,
            expired: 0,
            max_wait: Duration::ZERO,
            last_wait: Duration::ZERO,
        }
    }

    /// Pop the oldest entry younger than `max_age` that `gate` admits;
    /// older entries are discarded and counted. A refused head stays.
    fn pop_gated(
        &mut self,
        now: Instant,
        max_age: Duration,
        mut gate: impl FnMut(&T) -> bool,
    ) -> Option<T> {
        while let Some((at, _)) = self.items.front() {
            if now.saturating_duration_since(*at) > max_age {
                self.items.pop_front();
                self.expired += 1;
            } else {
                break;
            }
        }
        if !gate(&self.items.front()?.1) {
            return None;
        }
        self.pop(now)
    }

    fn push(&mut self, now: Instant, item: T) {
        self.pushed += 1;
        if self.items.len() >= self.cap {
            self.items.pop_front();
            self.dropped += 1;
        }
        self.items.push_back((now, item));
    }

    /// Pop the head and report how long it waited.
    fn pop_timed(&mut self, now: Instant) -> Option<(T, Duration)> {
        let (at, item) = self.items.pop_front()?;
        let wait = now.saturating_duration_since(at);
        self.max_wait = self.max_wait.max(wait);
        self.last_wait = wait;
        Some((item, wait))
    }

    fn pop(&mut self, now: Instant) -> Option<T> {
        self.pop_timed(now).map(|(item, _)| item)
    }

    fn stats(&self, now: Instant) -> QueueStats {
        QueueStats {
            len: self.items.len(),
            capacity: self.cap,
            pushed: self.pushed,
            dropped: self.dropped,
            oldest_age: self
                .items
                .front()
                .map_or(Duration::ZERO, |(at, _)| now.saturating_duration_since(*at)),
            max_wait: self.max_wait,
            expired: self.expired,
        }
    }
}

/// Wakes a lane's sender thread; shared by every queue it drains.
#[derive(Debug, Default)]
pub(crate) struct Wake {
    flag: Mutex<bool>,
    cv: Condvar,
}

impl Wake {
    pub(crate) fn notify(&self) {
        *self.flag.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.cv.notify_one();
    }

    fn wait(&self, timeout: Duration) {
        let mut flag = self.flag.lock().unwrap_or_else(|e| e.into_inner());
        if !*flag {
            flag = self
                .cv
                .wait_timeout(flag, timeout)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *flag = false;
    }
}

/// A bounded, nonblocking, drop-oldest queue: the gameplay handoff in
/// both directions. `push` never waits and never fails; a full queue
/// discards its oldest entry and counts it.
#[derive(Debug)]
pub struct LaneQueue<T> {
    inner: Mutex<Bounded<T>>,
    wake: Option<Arc<Wake>>,
}

impl<T> LaneQueue<T> {
    /// A standalone queue of capacity `cap` (at least 1).
    pub fn new(cap: usize) -> Self {
        Self {
            inner: Mutex::new(Bounded::new(cap.max(1))),
            wake: None,
        }
    }

    fn with_wake(cap: usize, wake: Arc<Wake>) -> Self {
        Self {
            inner: Mutex::new(Bounded::new(cap.max(1))),
            wake: Some(wake),
        }
    }

    /// Enqueue `item`, discarding the oldest entry when full.
    pub fn push(&self, item: T) {
        self.push_at(Instant::now(), item);
    }

    /// [`push`](Self::push) at an injected `now`.
    fn push_at(&self, now: Instant, item: T) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(now, item);
        if let Some(wake) = &self.wake {
            wake.notify();
        }
    }

    /// Dequeue the oldest entry, if any.
    pub fn pop(&self) -> Option<T> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop(Instant::now())
    }

    fn pop_gated(
        &self,
        now: Instant,
        max_age: Duration,
        gate: impl FnMut(&T) -> bool,
    ) -> Option<(T, Duration)> {
        let mut q = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let item = q.pop_gated(now, max_age, gate)?;
        Some((item, q.last_wait))
    }

    /// Occupancy, drop and queue-age counters.
    pub fn stats(&self) -> QueueStats {
        self.stats_at(Instant::now())
    }

    fn stats_at(&self, now: Instant) -> QueueStats {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stats(now)
    }
}

/// One outbound datagram waiting for the sender thread.
#[derive(Debug)]
pub(crate) struct OutItem {
    pub(crate) tag: LaneTag,
    generation: u64,
    pub(crate) payload: Vec<u8>,
}

/// A lane direction's outbound side: two bounded queues (movement drains
/// before bulk, NETPLAY-06D), the budget that gates what is sent, the
/// congestion controller that sets that budget's rate (NETPLAY-10B) and
/// the datagram sequence.
#[derive(Debug)]
pub(crate) struct SeatOut {
    queue: LaneQueue<OutItem>,
    bulk: LaneQueue<OutItem>,
    seq: AtomicU64,
    /// The host's aggregate uplink; `None` on a client, whose single
    /// peer is the host and whose traffic is small.
    uplink: Option<Arc<Mutex<Uplink>>>,
    pub(crate) pace: Mutex<PaceState>,
}

/// Samples kept for the queue-wait percentiles.
const WAIT_SAMPLES: usize = 4096;

#[derive(Debug)]
pub(crate) struct PaceState {
    budget: Budget,
    /// The world-state allowance (NETPLAY-10C-3): ambient cars and signals
    /// spend this instead of `budget`, so movement using the whole peer
    /// budget cannot starve them, and they cannot spend the movement
    /// credit. A quarter of the peer's rate.
    world: Budget,
    world_sent: u64,
    ctl: PeerController,
    /// The uplink share last granted (wire bytes per second).
    share: u32,
    movement_sent: u64,
    bulk_sent: u64,
    bulk_deferred: u64,
    movement_deferred: u64,
    uplink_denied: u64,
    max_debt: u64,
    wire_sent: u64,
    /// When something was last sent, for the silent-peer test.
    last_sent: Option<Instant>,
    pub(crate) movement_waits: Quantiles,
    /// Snapshot ticks queued and not yet acknowledged, oldest first.
    pub(crate) ticks: VecDeque<(u64, Instant)>,
    /// The newest round-trip sample the controller has not seen yet.
    rtt: Option<Duration>,
    /// Snapshots a newer acknowledgement passed over.
    skipped: u64,
}

/// Credit bulk leaves in the bucket for movement: world catch-up never
/// spends the last of a seat's credit, so the next snapshot is not made to
/// wait behind it.
const BULK_RESERVE_BYTES: usize = 2048;

/// The world-state allowance is this fraction (1/n) of the peer's rate.
const WORLD_SHARE_DIV: u32 = 4;

fn is_world_tag(tag: LaneTag) -> bool {
    WorldKind::from_tag(tag).is_some()
}

/// Unacknowledged snapshot ticks remembered per seat.
const TICK_LOG_CAP: usize = 64;

impl SeatOut {
    pub(crate) fn new(wake: Arc<Wake>, uplink: Option<Arc<Mutex<Uplink>>>, now: Instant) -> Self {
        Self {
            queue: LaneQueue::with_wake(MOVEMENT_QUEUE_CAP, wake.clone()),
            bulk: LaneQueue::with_wake(BULK_QUEUE_CAP, wake),
            seq: AtomicU64::new(0),
            uplink,
            pace: Mutex::new(PaceState {
                budget: Budget::new(PEER_BYTES_PER_SEC, PEER_BURST_BYTES, now),
                world: Budget::new(
                    PEER_BYTES_PER_SEC / WORLD_SHARE_DIV,
                    PEER_BURST_BYTES / WORLD_SHARE_DIV,
                    now,
                ),
                world_sent: 0,
                ctl: PeerController::new(DEFAULT_PEER_CEILING, now),
                share: DEFAULT_PEER_CEILING,
                movement_sent: 0,
                bulk_sent: 0,
                bulk_deferred: 0,
                movement_deferred: 0,
                uplink_denied: 0,
                max_debt: 0,
                wire_sent: 0,
                last_sent: None,
                movement_waits: Quantiles::new(WAIT_SAMPLES),
                ticks: VecDeque::new(),
                rtt: None,
                skipped: 0,
            }),
        }
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Queue a reliable-channel payload (data or ack) as a movement
    /// datagram: control traffic is never starved by the bulk budget.
    fn push_reliable(&self, tag: LaneTag, generation: u64, payload: Vec<u8>) {
        self.push_tagged(tag, Priority::Movement, generation, payload);
    }

    pub(crate) fn push_data(&self, generation: u64, payload: Vec<u8>) {
        self.push_tagged(LaneTag::Data, Priority::Movement, generation, payload);
    }

    fn push_tagged(&self, tag: LaneTag, priority: Priority, generation: u64, payload: Vec<u8>) {
        self.push_tagged_at(Instant::now(), tag, priority, generation, payload);
    }

    pub(crate) fn push_tagged_at(
        &self,
        now: Instant,
        tag: LaneTag,
        priority: Priority,
        generation: u64,
        payload: Vec<u8>,
    ) {
        let item = OutItem {
            tag,
            generation,
            payload,
        };
        match priority {
            Priority::Movement => self.queue.push_at(now, item),
            Priority::Bulk => self.bulk.push_at(now, item),
        }
    }

    /// Queue fragment payloads (from [`frag::split`]) in order, all at
    /// `priority`.
    pub(crate) fn push_fragments(&self, generation: u64, fragments: Vec<Vec<u8>>) {
        for f in fragments {
            self.push_tagged(LaneTag::Fragment, Priority::Movement, generation, f);
        }
    }

    /// Queue fragment payloads, each at the priority of its group
    /// (`priorities[group]`; a group past the list is bulk).
    pub(crate) fn push_fragments_prio(
        &self,
        generation: u64,
        fragments: Vec<Vec<u8>>,
        priorities: &[Priority],
    ) {
        for f in fragments {
            // Byte 8 of a fragment payload is its group index.
            let group = f.get(8).copied().map_or(0, usize::from);
            let p = priorities.get(group).copied().unwrap_or(Priority::Bulk);
            self.push_tagged(LaneTag::Fragment, p, generation, f);
        }
    }

    /// Queue one world-state frame's fragments as bulk datagrams under
    /// `kind`'s tag.
    pub(crate) fn push_world(&self, kind: WorldKind, generation: u64, fragments: Vec<Vec<u8>>) {
        for f in fragments {
            self.push_tagged(kind.tag(), Priority::Bulk, generation, f);
        }
    }

    /// The next datagram to send, or `None` when nothing is due.
    fn next_to_send(&self) -> Option<OutItem> {
        self.next_to_send_at(Instant::now())
    }

    /// [`next_to_send`](Self::next_to_send) at an injected `now`: stale
    /// entries are discarded, movement goes first, and every datagram
    /// must fit both the peer's rate budget and the host's aggregate
    /// uplink (wire bytes). A datagram that does not fit waits; it is
    /// never replaced by an older one and is discarded when it outlives
    /// its queue age.
    pub(crate) fn next_to_send_at(&self, now: Instant) -> Option<OutItem> {
        let mut pace = self.pace.lock().unwrap_or_else(|e| e.into_inner());
        let PaceState { budget, ctl, .. } = &mut *pace;
        let allow_bulk = ctl.adapt().bulk;
        let mut denied = false;
        let uplink = self.uplink.as_deref();
        let got = self.queue.pop_gated(now, MOVEMENT_MAX_AGE, |i| {
            let wire = wire_bytes(i.payload.len());
            // Movement needs positive credit (and may end one datagram in
            // debt); the uplink is asked second so a refused peer does
            // not consume shared credit.
            if budget.credit(now) <= 0 {
                return false;
            }
            if let Some(up) = uplink
                && !up
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .try_spend(wire, now)
            {
                denied = true;
                return false;
            }
            budget.spend(wire, now);
            true
        });
        if let Some((item, wait)) = got {
            pace.movement_sent += 1;
            pace.wire_sent += wire_bytes(item.payload.len()) as u64;
            pace.last_sent = Some(now);
            pace.movement_waits.add(wait.as_micros() as u64);
            let debt = (-pace.budget.credit(now)).max(0) as u64;
            pace.max_debt = pace.max_debt.max(debt);
            return Some(item);
        }
        if self.queue.stats_at(now).len > 0 {
            pace.movement_deferred += 1;
            if denied {
                pace.uplink_denied += 1;
            }
            // Movement waits; bulk never overtakes it.
            return None;
        }
        if !allow_bulk {
            // Low-priority traffic is shed, not queued, at a low rate.
            return None;
        }
        let PaceState { budget, world, .. } = &mut *pace;
        let mut blocked = false;
        let got = self.bulk.pop_gated(now, BULK_MAX_AGE, |i| {
            let wire = wire_bytes(i.payload.len());
            if is_world_tag(i.tag) {
                // Its own allowance: the uplink is asked only once that
                // covers the datagram.
                if world.credit(now) < wire as i64 {
                    blocked = true;
                    return false;
                }
                if let Some(up) = uplink
                    && !up
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .try_spend(wire, now)
                {
                    blocked = true;
                    return false;
                }
                world.spend(wire, now);
                return true;
            }
            if budget.credit(now) < (wire + BULK_RESERVE_BYTES) as i64 {
                blocked = true;
                return false;
            }
            if let Some(up) = uplink
                && !up
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .try_spend(wire, now)
            {
                blocked = true;
                return false;
            }
            budget.spend(wire, now);
            true
        });
        if blocked {
            pace.bulk_deferred += 1;
        }
        let (item, _) = got?;
        pace.bulk_sent += 1;
        if is_world_tag(item.tag) {
            pace.world_sent += 1;
        }
        pace.wire_sent += wire_bytes(item.payload.len()) as u64;
        pace.last_sent = Some(now);
        Some(item)
    }

    /// Whether anything is queued at either priority.
    fn has_pending(&self) -> bool {
        self.queue.stats().len + self.bulk.stats().len > 0
    }

    /// Note that the snapshot of `tick` was queued for this seat now: the
    /// round-trip clock for its acknowledgement.
    pub(crate) fn note_tick(&self, tick: u64, now: Instant) {
        let mut pace = self.pace.lock().unwrap_or_else(|e| e.into_inner());
        if pace.ticks.back().is_some_and(|(t, _)| *t >= tick) {
            // The same tick again (another group), or a restarted clock.
            if pace.ticks.back().is_some_and(|(t, _)| *t == tick) {
                return;
            }
            pace.ticks.clear();
        }
        if pace.ticks.len() >= TICK_LOG_CAP {
            pace.ticks.pop_front();
        }
        pace.ticks.push_back((tick, now));
    }

    /// The peer acknowledged the state of `tick`: take a round-trip
    /// sample from it and count every older snapshot it passed over.
    pub(crate) fn on_ack(&self, tick: u64, now: Instant) {
        let mut pace = self.pace.lock().unwrap_or_else(|e| e.into_inner());
        let mut sample = None;
        while let Some(&(t, at)) = pace.ticks.front() {
            if t > tick {
                break;
            }
            pace.ticks.pop_front();
            if t == tick {
                sample = Some(now.saturating_duration_since(at));
            } else {
                pace.skipped += 1;
            }
        }
        if sample.is_some() {
            pace.rtt = sample;
        }
    }

    /// The controller's input from the send loop: the reliable channel's
    /// retransmission count and the age of the peer's newest
    /// acknowledgement (`None`: nothing to acknowledge yet).
    pub(crate) fn update_control(&self, now: Instant, retransmits: u64, ack_age: Option<Duration>) {
        let mv = self.queue.stats_at(now);
        let mut pace = self.pace.lock().unwrap_or_else(|e| e.into_inner());
        // An ack that is merely quiet because nothing was sent is no
        // evidence: only count it while datagrams flow.
        let flowing = pace
            .last_sent
            .is_some_and(|t| now.saturating_duration_since(t) <= CONGESTED_ACK_AGE);
        let fb = Feedback {
            retransmits,
            // Bulk is meant to expire when the budget is spent on
            // movement; only movement discarded unsent is congestion.
            expired: mv.expired,
            wire_sent: pace.wire_sent,
            queue_age: mv.oldest_age,
            ack_age: ack_age.filter(|_| flowing),
            rtt: pace.rtt.take(),
            skipped: pace.skipped,
        };
        pace.ctl.on_feedback(fb, now);
        let rate = effective_rate(pace.ctl.rate(), pace.share);
        pace.budget.retune(rate, PEER_BURST_BYTES, now);
        pace.world.retune(
            rate / WORLD_SHARE_DIV,
            PEER_BURST_BYTES / WORLD_SHARE_DIV,
            now,
        );
    }

    /// The rate the peer wants from the uplink.
    pub(crate) fn claim_rate(&self) -> u32 {
        self.pace
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ctl
            .rate()
    }

    /// Grant the peer `share` wire bytes per second of the uplink.
    pub(crate) fn set_share(&self, share: u32, now: Instant) {
        let mut pace = self.pace.lock().unwrap_or_else(|e| e.into_inner());
        pace.share = share;
        let rate = effective_rate(pace.ctl.rate(), share);
        pace.budget.retune(rate, PEER_BURST_BYTES, now);
        pace.world.retune(
            rate / WORLD_SHARE_DIV,
            PEER_BURST_BYTES / WORLD_SHARE_DIV,
            now,
        );
    }

    /// How the traffic to this peer should adapt now.
    pub(crate) fn adapt(&self) -> Adapt {
        self.pace
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ctl
            .adapt()
    }

    /// The movement queue's counters (the one `Data` and `Ack` use).
    pub(crate) fn stats(&self) -> QueueStats {
        self.queue.stats()
    }

    pub(crate) fn pace_stats(&self) -> PaceStats {
        self.pace_stats_at(Instant::now())
    }

    pub(crate) fn pace_stats_at(&self, now: Instant) -> PaceStats {
        let pace = self.pace.lock().unwrap_or_else(|e| e.into_inner());
        let waits = &pace.movement_waits;
        let us = |p| Duration::from_micros(waits.percentile(p));
        PaceStats {
            movement: self.queue.stats_at(now),
            bulk: self.bulk.stats_at(now),
            movement_sent: pace.movement_sent,
            bulk_sent: pace.bulk_sent,
            world_sent: pace.world_sent,
            bytes_sent: pace.budget.spent(),
            bulk_deferred: pace.bulk_deferred,
            max_debt: pace.max_debt,
            wire_sent: pace.wire_sent,
            rate: pace.ctl.rate(),
            ceiling: pace.ctl.ceiling(),
            share: pace.share,
            decreases: pace.ctl.decreases(),
            movement_deferred: pace.movement_deferred,
            uplink_denied: pace.uplink_denied,
            adapt: pace.ctl.adapt(),
            wait_p50: us(50),
            wait_p95: us(95),
            wait_p99: us(99),
            wait_max: Duration::from_micros(waits.max()),
        }
    }
}

/// A secret per-seat token: SipHash over OS-seeded keys (see the module
/// note on what that does and does not protect). Never zero.
fn mint_token(seat: u16) -> u64 {
    use std::hash::{BuildHasher, Hasher, RandomState};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = RandomState::new().build_hasher();
    h.write_u16(seat);
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    h.finish().max(1)
}

/// The host's record of one seat's lane.
#[derive(Debug)]
pub(crate) struct LaneSeat {
    ip: IpAddr,
    port: u16,
    token: u64,
    established: bool,
    /// When the last authenticated datagram arrived (the grant, until
    /// then): the host's liveness clock for the seat.
    heard: Instant,
    window: SeqWindow,
    out: Arc<SeatOut>,
    /// The newest acknowledgement per logical snapshot group.
    acks: [Option<LaneAck>; MAX_GROUPS],
    /// When an `acks` entry last advanced.
    ack_at: Option<Instant>,
    /// The seat's baselines, one sender per logical group. Born with the
    /// lane seat, so a client that rejoins under the same id starts from
    /// full states instead of deltas against a baseline it never held.
    baselines: Arc<Mutex<SeatBaselines>>,
    rel: ReliableChannel,
}

/// The client's baselines and counters.
#[derive(Debug, Default)]
struct ClientStates {
    receivers: [BaselineReceiver; MAX_GROUPS],
    stats: StateStats,
}

/// What the host holds to delta one seat's snapshot groups against.
#[derive(Debug, Default)]
pub(crate) struct SeatBaselines {
    senders: [BaselineSender; MAX_GROUPS],
}

/// Counters for baselined snapshot states (NETPLAY-10D-2), on either end
/// of a lane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StateStats {
    /// Host: groups sent as a full state.
    pub sent_full: u64,
    /// Host: groups sent as a delta against an acknowledged baseline.
    pub sent_delta: u64,
    /// Host: bytes of state payload sent.
    pub sent_bytes: u64,
    /// Host: bytes the same groups would have taken as full states.
    pub raw_bytes: u64,
    /// Client: states applied and acknowledged.
    pub applied: u64,
    /// Client: deltas refused because their base is not held.
    pub missing_base: u64,
    /// Client: states refused as stale (a duplicate or reordered group).
    pub stale: u64,
    /// Client: states that did not parse.
    pub malformed: u64,
}

/// Every seat's lane plus the generation datagrams must name. Lives
/// inside the `RemoteInputs` mailbox lock so an authenticated datagram
/// and a seat's removal cannot interleave: a departed seat's datagram is
/// an unknown seat, never a late write into a recycled id.
#[derive(Debug, Default)]
pub(crate) struct LaneTable {
    /// The running session generation, 0 when none runs.
    generation: u64,
    seats: BTreeMap<u16, LaneSeat>,
}

impl LaneTable {
    pub(crate) fn remove(&mut self, id: u16) {
        self.seats.remove(&id);
    }

    pub(crate) fn clear(&mut self) {
        self.generation = 0;
        self.seats.clear();
    }

    /// The seats whose lane has authenticated nothing for longer than
    /// `bound` as of `now`.
    pub(crate) fn silent(&self, now: Instant, bound: Duration) -> Vec<u16> {
        self.seats
            .iter()
            .filter(|(_, s)| now.saturating_duration_since(s.heard) > bound)
            .map(|(id, _)| *id)
            .collect()
    }
}

/// The generation value of the lobby scope (NETPLAY-06E-2a): no session
/// runs, so a reliable datagram names generation 0. Authentication is the
/// same seat token, address and replay window as for every other tag;
/// only the generation test is relaxed, and only for the two reliable
/// tags. `Data`, `Fragment` and `Ack` still need the running generation,
/// and a reliable datagram naming a *nonzero* generation must still match
/// the running one.
pub(crate) const LOBBY_SCOPE: u64 = 0;

fn is_reliable_tag(tag: LaneTag) -> bool {
    matches!(tag, LaneTag::Reliable | LaneTag::ReliableAck)
}

/// Whether the host accepts a datagram header under `running` (0: none).
fn generation_admits(h: &Header, running: u64) -> bool {
    match h.tag {
        LaneTag::Probe => true,
        t if is_reliable_tag(t) && h.generation == LOBBY_SCOPE => true,
        _ => running != 0 && h.generation == running,
    }
}

/// What the host did with a datagram, for the receive loop's counters.
enum Verdict {
    Accepted,
    Dropped(Refuse),
}

/// Host-owned classes the reliable channel delivered, taken off the front
/// of the seat's queue in order: reset asks go into the mailbox's
/// newest-wins slot (NETPLAY-06E-3), a lobby `Leave`/`SetReady`
/// (NETPLAY-06E-2c) into the mailbox's intent queue for the lobby loop.
/// Only the front of the delivered queue is looked at: any other message
/// class stays queued for [`HostLane::recv_reliable`], and what is behind
/// it follows once that is consumed (it calls this again).
fn absorb_reliable_resets(mail: &mut PeerMail, id: u16) {
    let Some(seat) = mail.lane.seats.get_mut(&id) else {
        return;
    };
    let mut taken = Vec::new();
    while let Some(body) = seat.rel.receiver.pop_if(|b| {
        matches!(
            Message::decode(b),
            Ok(Message::ResetRequest { .. } | Message::Leave | Message::SetReady { .. })
        )
    }) {
        if let Ok(msg) = Message::decode(&body) {
            taken.push(msg);
        }
    }
    for msg in taken {
        match msg {
            Message::ResetRequest { generation } => mail.absorb_reset(id, generation),
            other => mail.absorb_lobby_intent(id, other),
        }
    }
}

/// Authenticate one datagram against the seat table and, if it is good,
/// apply it. The checks run cheapest-and-least-trusting first and the
/// replay window moves only after authentication, so a forger cannot
/// advance it.
fn ingest(
    mail: &mut PeerMail,
    h: &Header,
    payload: &[u8],
    from: SocketAddr,
    now: Instant,
) -> Verdict {
    let generation = mail.lane.generation;
    let Some(seat) = mail.lane.seats.get_mut(&h.seat) else {
        return Verdict::Dropped(Refuse::UnknownSeat);
    };
    if seat.token != h.token {
        return Verdict::Dropped(Refuse::BadToken);
    }
    if from.ip().to_canonical() != seat.ip || from.port() != seat.port {
        return Verdict::Dropped(Refuse::WrongAddr);
    }
    if !generation_admits(h, generation) {
        return Verdict::Dropped(Refuse::WrongGeneration);
    }
    if !seat.window.accept(h.seq) {
        return Verdict::Dropped(Refuse::Replayed);
    }
    seat.established = true;
    seat.heard = now;
    match h.tag {
        LaneTag::Probe => {
            // Echo every probe: the client may have lost the last echo.
            seat.out.queue.push(OutItem {
                tag: LaneTag::Probe,
                generation: 0,
                payload: probe_padding(),
            });
            Verdict::Accepted
        }
        LaneTag::Data => match Message::decode(payload) {
            Ok(Message::InputWindow {
                generation: g,
                epoch,
                first_seq,
                commands,
            }) => {
                if g != h.generation {
                    return Verdict::Dropped(Refuse::WrongGeneration);
                }
                mail.absorb_window(h.seat, g, epoch, first_seq, &commands, now);
                Verdict::Accepted
            }
            Ok(_) => Verdict::Dropped(Refuse::Unexpected),
            Err(_) => Verdict::Dropped(Refuse::Malformed),
        },
        LaneTag::Fragment | LaneTag::WorldCars | LaneTag::WorldSignals => {
            Verdict::Dropped(Refuse::Unexpected)
        }
        LaneTag::Reliable => match decode_data(payload) {
            Some((id, more, body)) => {
                let ack = seat.rel.receiver.on_data(id, more, body);
                seat.out
                    .push_reliable(LaneTag::ReliableAck, h.generation, ack);
                absorb_reliable_resets(mail, h.seat);
                Verdict::Accepted
            }
            None => {
                seat.rel.receiver.note_malformed();
                Verdict::Dropped(Refuse::Malformed)
            }
        },
        LaneTag::ReliableAck => match decode_rel_ack(payload) {
            Some((cum, mask)) => {
                seat.rel.sender.on_ack(cum, mask);
                Verdict::Accepted
            }
            None => Verdict::Dropped(Refuse::Malformed),
        },
        LaneTag::Ack => match decode_ack(payload) {
            Some((epoch, group, tick)) => {
                let slot = &mut seat.acks[usize::from(group)];
                let newer = slot.is_none_or(|a| a.generation != h.generation || a.tick < tick);
                if newer {
                    *slot = Some(LaneAck {
                        generation: h.generation,
                        epoch,
                        group,
                        tick,
                    });
                    seat.ack_at = Some(now);
                    seat.out.on_ack(tick, now);
                    // The sender judges scope and retention itself; an
                    // ack it cannot match is simply not a baseline.
                    seat.baselines
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .senders[usize::from(group)]
                    .ack(h.generation, epoch, tick);
                }
                Verdict::Accepted
            }
            None => Verdict::Dropped(Refuse::Malformed),
        },
    }
}

/// The host's datagram socket, its two threads and its counters.
#[derive(Debug)]
pub(crate) struct HostLane {
    port: u16,
    inputs: RemoteInputs,
    counters: Counters,
    stop: AtomicBool,
    wake: Arc<Wake>,
    clock: ReliableClock,
    /// The host's aggregate uplink, shared by every seat's sender.
    uplink: Arc<Mutex<Uplink>>,
    states: Mutex<StateStats>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl HostLane {
    /// Bind `ip:0` and start the receive and send threads. `uplink_cap` is
    /// the host's aggregate send budget in wire bytes per second.
    pub(crate) fn bind(ip: IpAddr, inputs: RemoteInputs, uplink_cap: u32) -> io::Result<Arc<Self>> {
        let socket = Arc::new(UdpSocket::bind(SocketAddr::new(ip, 0))?);
        socket.set_read_timeout(Some(POLL))?;
        let lane = Arc::new(Self {
            port: socket.local_addr()?.port(),
            inputs,
            counters: Counters::default(),
            stop: AtomicBool::new(false),
            wake: Arc::new(Wake::default()),
            clock: ReliableClock::new(),
            uplink: Arc::new(Mutex::new(Uplink::new(uplink_cap, Instant::now()))),
            states: Mutex::new(StateStats::default()),
            threads: Mutex::new(Vec::new()),
        });
        let recv = {
            let (lane, socket) = (lane.clone(), socket.clone());
            thread::spawn(move || lane.recv_loop(&socket))
        };
        let send = {
            let lane = lane.clone();
            thread::spawn(move || lane.send_loop(&socket))
        };
        lane.threads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend([recv, send]);
        Ok(lane)
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    pub(crate) fn counts(&self) -> LaneCounts {
        self.counters.snapshot()
    }

    /// Change the aggregate uplink budget (wire bytes per second).
    pub(crate) fn set_uplink_cap(&self, cap: u32) {
        self.uplink
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_cap(cap, Instant::now());
        self.wake.notify();
    }

    /// The aggregate uplink's cap and the wire bytes it has admitted.
    pub(crate) fn uplink_stats(&self) -> (u32, u64) {
        let up = self.uplink.lock().unwrap_or_else(|e| e.into_inner());
        (up.cap(), up.spent())
    }

    /// Open (or replace) `id`'s lane for a peer at `ip`, expecting
    /// datagrams from `port`; returns the token to grant.
    pub(crate) fn register(&self, id: u16, ip: IpAddr, port: u16) -> u64 {
        let token = mint_token(id);
        let mut mail = self.inputs.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.lane.seats.insert(
            id,
            LaneSeat {
                ip: ip.to_canonical(),
                port,
                token,
                established: false,
                heard: Instant::now(),
                window: SeqWindow::default(),
                out: Arc::new(SeatOut::new(
                    self.wake.clone(),
                    Some(self.uplink.clone()),
                    Instant::now(),
                )),
                acks: [None; MAX_GROUPS],
                ack_at: None,
                baselines: Arc::default(),
                rel: ReliableChannel::new(MAX_RELIABLE_BODY),
            },
        );
        token
    }

    /// Set the generation datagrams must name (0 = no session).
    ///
    /// Every seat's unacked reliable messages minted for any other nonzero
    /// generation are retired (`ReliableSender::retire`): that generation
    /// has ended, the receiver would refuse them unacked and the channel
    /// would die. Lobby-scope messages stay.
    pub(crate) fn set_generation(&self, generation: u64) {
        let now = self.clock.now_ms();
        let mut mail = self.inputs.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.lane.generation = generation;
        for seat in mail.lane.seats.values_mut() {
            seat.rel.sender.retire(now, |g| g != generation);
        }
        drop(mail);
        self.wake.notify();
    }

    /// The outbound side of every seat whose lane is established.
    pub(crate) fn established_outs(&self) -> Vec<(u16, Arc<SeatOut>)> {
        let mail = self.inputs.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.lane
            .seats
            .iter()
            .filter(|(_, s)| s.established)
            .map(|(id, s)| (*id, s.out.clone()))
            .collect()
    }

    pub(crate) fn established_out(&self, id: u16) -> Option<Arc<SeatOut>> {
        let mail = self.inputs.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.lane
            .seats
            .get(&id)
            .filter(|s| s.established)
            .map(|s| s.out.clone())
    }

    /// The newest acknowledgement `id`'s client sent.
    pub(crate) fn latest_ack(&self, id: u16) -> Option<LaneAck> {
        let mail = self.inputs.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.lane
            .seats
            .get(&id)
            .and_then(|s| s.acks.iter().flatten().max_by_key(|a| a.tick).copied())
    }

    /// The newest acknowledgement `id`'s client sent for `group`.
    pub(crate) fn group_ack(&self, id: u16, group: u8) -> Option<LaneAck> {
        let mail = self.inputs.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.lane
            .seats
            .get(&id)
            .and_then(|s| s.acks.get(usize::from(group)).copied().flatten())
    }

    /// Baselined state counters of the host side.
    pub(crate) fn state_stats(&self) -> StateStats {
        *self.states.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Encode each `(group, snapshot bytes)` of `id`'s tick against the
    /// baselines its client acknowledged, as the data of fragment groups.
    /// `None` when the seat has no established lane. Nothing is queued.
    pub(crate) fn encode_states(
        &self,
        id: u16,
        generation: u64,
        epoch: u8,
        tick: u64,
        groups: &[(u8, Vec<u8>)],
    ) -> Option<Vec<Vec<u8>>> {
        let baselines = {
            let mail = self.inputs.inner.lock().unwrap_or_else(|e| e.into_inner());
            mail.lane
                .seats
                .get(&id)
                .filter(|s| s.established)
                .map(|s| s.baselines.clone())?
        };
        let mut baselines = baselines.lock().unwrap_or_else(|e| e.into_inner());
        let mut stats = self.states.lock().unwrap_or_else(|e| e.into_inner());
        Some(
            groups
                .iter()
                .map(|(group, bytes)| {
                    let (payload, plan) = baselines.senders[usize::from(*group)]
                        .encode(generation, epoch, tick, bytes);
                    match plan {
                        Plan::Full => stats.sent_full += 1,
                        Plan::Delta(_) => stats.sent_delta += 1,
                    }
                    stats.sent_bytes += payload.len() as u64;
                    stats.raw_bytes += bytes.len() as u64;
                    encode_state_frame(*group, epoch, &payload)
                })
                .collect(),
        )
    }

    pub(crate) fn is_established(&self, id: u16) -> bool {
        self.established_out(id).is_some()
    }

    /// Send `body` to `id` on the reliable channel (NETPLAY-06E-1):
    /// numbered, repeated until acknowledged, delivered exactly once and
    /// in order. Nonblocking; allowed before the lane is established (the
    /// first transmission waits in the seat's queue, repeats start once
    /// it is).
    pub(crate) fn send_reliable(
        &self,
        id: u16,
        generation: u64,
        body: &[u8],
    ) -> Result<(), ReliableSendError> {
        let mut mail = self.inputs.inner.lock().unwrap_or_else(|e| e.into_inner());
        let seat = mail
            .lane
            .seats
            .get_mut(&id)
            .ok_or(ReliableSendError::NoLane)?;
        let payloads = seat
            .rel
            .sender
            .send(self.clock.now_ms(), generation, body)
            .map_err(ReliableSendError::Channel)?;
        for payload in payloads {
            seat.out
                .push_reliable(LaneTag::Reliable, generation, payload);
        }
        Ok(())
    }

    /// The next reliably delivered message from any seat, with its seat.
    pub(crate) fn recv_reliable(&self) -> Option<(u16, Vec<u8>)> {
        let mut mail = self.inputs.inner.lock().unwrap_or_else(|e| e.into_inner());
        let found = mail
            .lane
            .seats
            .iter_mut()
            .find_map(|(id, s)| s.rel.receiver.pop().map(|m| (*id, m)));
        if let Some((id, _)) = found {
            // The popped message may have been blocking reset asks.
            absorb_reliable_resets(&mut mail, id);
        }
        found
    }

    pub(crate) fn reliable_stats(&self, id: u16) -> Option<ReliableStats> {
        let mail = self.inputs.inner.lock().unwrap_or_else(|e| e.into_inner());
        mail.lane.seats.get(&id).map(|s| s.rel.stats())
    }

    pub(crate) fn clock(&self) -> &ReliableClock {
        &self.clock
    }

    pub(crate) fn poke(&self) {
        self.wake.notify();
    }

    /// Stop both threads and wait for them.
    pub(crate) fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.wake.notify();
        let threads = std::mem::take(&mut *self.threads.lock().unwrap_or_else(|e| e.into_inner()));
        for t in threads {
            let _ = t.join();
        }
    }

    fn recv_loop(&self, socket: &UdpSocket) {
        // One byte over the datagram bound, so an oversize datagram reads
        // as oversize instead of being silently truncated into range.
        let mut buf = [0u8; MAX_DATAGRAM + 1];
        while !self.stop.load(Ordering::Relaxed) {
            let (n, from) = match socket.recv_from(&mut buf) {
                Ok(r) => r,
                Err(e) if is_timeout(&e) => continue,
                Err(_) => {
                    thread::sleep(POLL);
                    continue;
                }
            };
            Counters::bump(&self.counters.received);
            let Some((header, payload)) = decode_datagram(&buf[..n]) else {
                self.counters.drop_for(Refuse::Malformed);
                continue;
            };
            let verdict = {
                let mut mail = self.inputs.inner.lock().unwrap_or_else(|e| e.into_inner());
                ingest(&mut mail, &header, payload, from, Instant::now())
            };
            match verdict {
                Verdict::Accepted => Counters::bump(&self.counters.accepted),
                Verdict::Dropped(why) => self.counters.drop_for(why),
            }
        }
    }

    fn send_loop(&self, socket: &UdpSocket) {
        let mut backlog = false;
        while !self.stop.load(Ordering::Relaxed) {
            // Datagrams held back for rate or uplink credit are retried
            // on a short tick: credit accrues with time, not with a push.
            self.wake.wait(if backlog { BACKLOG_POLL } else { POLL });
            backlog = false;
            let wall = Instant::now();
            let targets: Vec<(u16, SocketAddr, u64, Arc<SeatOut>)> = {
                let now = self.clock.now_ms();
                let mut mail = self.inputs.inner.lock().unwrap_or_else(|e| e.into_inner());
                mail.lane
                    .seats
                    .iter_mut()
                    .filter(|(_, s)| s.established)
                    .map(|(id, s)| {
                        for (generation, payload) in s.rel.sender.poll(now) {
                            s.out.push_reliable(LaneTag::Reliable, generation, payload);
                        }
                        let retransmits = s.rel.sender.stats().retransmits;
                        let ack_age = s.ack_at.map(|t| wall.saturating_duration_since(t));
                        s.out.update_control(wall, retransmits, ack_age);
                        (*id, SocketAddr::new(s.ip, s.port), s.token, s.out.clone())
                    })
                    .collect()
            };
            {
                let outs: Vec<(u16, &SeatOut)> =
                    targets.iter().map(|(id, _, _, o)| (*id, &**o)).collect();
                divide_uplink(&self.uplink, &outs, wall);
            }
            for (seat, addr, token, out) in targets {
                while let Some(item) = out.next_to_send() {
                    let header = Header {
                        tag: item.tag,
                        generation: item.generation,
                        seat,
                        token,
                        seq: out.next_seq(),
                    };
                    flush(socket, addr, &header, &item.payload, &self.counters);
                }
                backlog |= out.has_pending();
            }
        }
    }
}

/// Hand every seat its max-min fair share of the aggregate uplink
/// (NETPLAY-10B): a seat asking for less than an equal share gets what it
/// asks and the remainder is split among the rest.
pub(crate) fn divide_uplink(uplink: &Mutex<Uplink>, outs: &[(u16, &SeatOut)], now: Instant) {
    let claims: Vec<Claim> = outs
        .iter()
        .map(|(seat, out)| Claim {
            seat: *seat,
            rate: out.claim_rate(),
        })
        .collect();
    let shares = uplink
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .shares(&claims);
    for ((_, out), (_, share)) in outs.iter().zip(shares) {
        out.set_share(share, now);
    }
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}

fn flush(socket: &UdpSocket, to: SocketAddr, header: &Header, payload: &[u8], c: &Counters) {
    let Some(datagram) = encode_datagram(header, payload) else {
        // Unreachable for the callers (they size-check), but a payload
        // that does not fit is counted rather than panicked on.
        Counters::bump(&c.send_errors);
        return;
    };
    match socket.send_to(&datagram, to) {
        Ok(_) => Counters::bump(&c.sent),
        Err(_) => Counters::bump(&c.send_errors),
    }
}

/// Reliably delivered messages the client inbox holds before the lane
/// stops taking more out of the channel's receiver.
const REL_INBOX_CAP: usize = 64;

/// Whether a reliably delivered body is a host event the client's inbox
/// owns (everything else stays for `recv_reliable`). A lobby `Roster`
/// (NETPLAY-06E-2a) and the session `Start`/`Cancel` (2b) are too:
/// `Client::recv` returns them, and so is the world-prop `Props` frame
/// (NETPLAY-10C-3), whose late-join snapshot is committed on the host as
/// delivered.
fn is_host_event(body: &[u8]) -> bool {
    matches!(
        Message::decode(body),
        Ok(Message::Cnr { .. }
            | Message::Impacts { .. }
            | Message::Props { .. }
            | Message::Roster { .. }
            | Message::Start { .. }
            | Message::Cancel { .. })
    )
}

/// The client's merged receive side: the TCP reader thread feeds the
/// reliable queue, the lane receive thread feeds a bounded drop-oldest
/// queue of `Snap`s, and `Client::recv` drains both.
#[derive(Debug)]
pub(crate) struct Inbox {
    state: Mutex<InboxState>,
    cv: Condvar,
}

#[derive(Debug)]
struct InboxState {
    tcp: VecDeque<Result<Message, NetError>>,
    /// Messages the lane's reliable channel delivered (NETPLAY-06E-3):
    /// bounded by [`REL_INBOX_CAP`], never dropped — the receiver keeps
    /// what does not fit and the sender keeps repeating.
    rel: VecDeque<Message>,
    lane: Bounded<Message>,
    closed: bool,
}

impl Inbox {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(InboxState {
                tcp: VecDeque::new(),
                rel: VecDeque::new(),
                lane: Bounded::new(QUEUE_CAP),
                closed: false,
            }),
            cv: Condvar::new(),
        })
    }

    /// Queue a reliable-side result; waits while the queue is full.
    /// `false` once the inbox is closed.
    pub(crate) fn push_tcp(&self, item: Result<Message, NetError>) -> bool {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while st.tcp.len() >= TCP_INBOX_CAP && !st.closed {
            st = self.cv.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        if st.closed {
            return false;
        }
        st.tcp.push_back(item);
        self.cv.notify_all();
        true
    }

    /// Queue a read-timeout error, unless one is already waiting last.
    pub(crate) fn push_tcp_timeout(&self, e: io::Error) -> bool {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.closed {
            return false;
        }
        if !matches!(st.tcp.back(), Some(Err(_))) {
            st.tcp.push_back(Err(NetError::Io(e)));
            self.cv.notify_all();
        }
        true
    }

    /// Room left for reliably delivered messages.
    fn rel_room(&self) -> usize {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        REL_INBOX_CAP.saturating_sub(st.rel.len())
    }

    fn push_rel(&self, msg: Message) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .rel
            .push_back(msg);
        self.cv.notify_all();
    }

    fn push_lane(&self, msg: Message) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .lane
            .push(Instant::now(), msg);
        self.cv.notify_all();
    }

    pub(crate) fn close(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        self.cv.notify_all();
    }

    pub(crate) fn lane_stats(&self) -> QueueStats {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .lane
            .stats(Instant::now())
    }

    /// The next message, reliable side first. `timeout` bounds the wait
    /// (`None`: until something arrives or the inbox closes).
    pub(crate) fn pop(&self, timeout: Option<Duration>) -> Result<Message, NetError> {
        let deadline = timeout.map(|t| Instant::now() + t);
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            // A control stream that ended (EOF, a reset) is
            // the last thing the client hears: the host exiting right after
            // it sent a once-only message (the results frame) must not
            // lose what the lane already delivered. A read timeout is not
            // an end and keeps its place in line.
            let ended = matches!(
                st.tcp.front(),
                Some(Err(NetError::Io(e))) if !is_timeout(e)
            );
            if ended {
                if let Some(msg) = st.rel.pop_front() {
                    return Ok(msg);
                }
                if let Some(msg) = st.lane.pop(Instant::now()) {
                    return Ok(msg);
                }
            }
            if let Some(item) = st.tcp.pop_front() {
                self.cv.notify_all();
                return item;
            }
            if let Some(msg) = st.rel.pop_front() {
                return Ok(msg);
            }
            if let Some(msg) = st.lane.pop(Instant::now()) {
                return Ok(msg);
            }
            if st.closed {
                return Err(NetError::Io(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "connection closed",
                )));
            }
            st = match deadline {
                None => self.cv.wait(st).unwrap_or_else(|e| e.into_inner()),
                Some(d) => {
                    let left = d.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(NetError::Io(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "timed out waiting for the host",
                        )));
                    }
                    self.cv
                        .wait_timeout(st, left)
                        .unwrap_or_else(|e| e.into_inner())
                        .0
                }
            };
        }
    }
}

/// The client's side of an established lane.
#[derive(Debug)]
pub(crate) struct ClientLane {
    host: SocketAddr,
    seat: u16,
    token: u64,
    out: SeatOut,
    wake: Arc<Wake>,
    counters: Counters,
    stop: AtomicBool,
    /// Newest `Start` generation the client consumed (0: none) — a
    /// datagram naming an older one is stale.
    started: AtomicU64,
    inbox: Arc<Inbox>,
    assembler: Mutex<Assembler>,
    /// The receiving end of the host's baselined groups, one per logical
    /// group, and the client-side counters.
    receivers: Mutex<ClientStates>,
    /// When the host's last probe echo arrived (path-health clock).
    last_echo: Mutex<Instant>,
    /// Probe interval and failure bound in milliseconds (test seam).
    probe_every_ms: AtomicU64,
    fail_after_ms: AtomicU64,
    /// Test seam: probe only after a quiet `probe_every_ms` (the pre-10D
    /// keepalive), so a test that fixes datagram counts is not perturbed.
    idle_probe_only: AtomicBool,
    /// One assembler per [`WorldKind`] (indexed by it): each kind has its
    /// own tick domain.
    world: [Mutex<Assembler>; 2],
    rel: Mutex<ReliableChannel>,
    clock: ReliableClock,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl ClientLane {
    /// Probe the host from `socket` until it echoes, then start the
    /// receive and send threads. A host that stays silent for `timeout`
    /// is [`NetError::LaneTimeout`].
    pub(crate) fn establish(
        socket: UdpSocket,
        host: SocketAddr,
        seat: u16,
        token: u64,
        inbox: Arc<Inbox>,
        timeout: Duration,
    ) -> Result<Arc<Self>, NetError> {
        socket.set_read_timeout(Some(POLL))?;
        let wake = Arc::new(Wake::default());
        let lane = Arc::new(Self {
            host,
            seat,
            token,
            out: SeatOut::new(wake.clone(), None, Instant::now()),
            wake,
            counters: Counters::default(),
            stop: AtomicBool::new(false),
            started: AtomicU64::new(0),
            inbox,
            assembler: Mutex::new(Assembler::new()),
            receivers: Mutex::new(ClientStates::default()),
            last_echo: Mutex::new(Instant::now()),
            probe_every_ms: AtomicU64::new(PATH_PROBE_INTERVAL.as_millis() as u64),
            fail_after_ms: AtomicU64::new(PATH_FAILURE_AFTER.as_millis() as u64),
            idle_probe_only: AtomicBool::new(false),
            world: [Mutex::new(Assembler::new()), Mutex::new(Assembler::new())],
            rel: Mutex::new(ReliableChannel::new(MAX_RELIABLE_BODY)),
            clock: ReliableClock::new(),
            threads: Mutex::new(Vec::new()),
        });
        let socket = Arc::new(socket);
        let mut window = SeqWindow::default();
        let start = Instant::now();
        let mut next_probe = start;
        let mut buf = [0u8; MAX_DATAGRAM + 1];
        loop {
            let now = Instant::now();
            if now.duration_since(start) >= timeout {
                return Err(NetError::LaneTimeout {
                    host,
                    waited: timeout,
                });
            }
            if now >= next_probe {
                let header = lane.header(LaneTag::Probe, 0);
                flush(&socket, host, &header, &probe_padding(), &lane.counters);
                next_probe = now + PROBE_INTERVAL;
            }
            match socket.recv_from(&mut buf) {
                Ok((n, from)) => {
                    Counters::bump(&lane.counters.received);
                    match lane.check(&buf[..n], from, &mut window) {
                        Ok((header, _)) if header.tag == LaneTag::Probe => {
                            Counters::bump(&lane.counters.accepted);
                            lane.note_echo();
                            break;
                        }
                        Ok(_) => Counters::bump(&lane.counters.unexpected),
                        Err(why) => lane.counters.drop_for(why),
                    }
                }
                Err(e) if is_timeout(&e) => {}
                Err(e) => return Err(e.into()),
            }
        }
        let recv = {
            let (lane, socket) = (lane.clone(), socket.clone());
            thread::spawn(move || lane.recv_loop(&socket, window))
        };
        let send = {
            let lane = lane.clone();
            thread::spawn(move || lane.send_loop(&socket))
        };
        lane.threads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend([recv, send]);
        Ok(lane)
    }

    fn header(&self, tag: LaneTag, generation: u64) -> Header {
        Header {
            tag,
            generation,
            seat: self.seat,
            token: self.token,
            seq: self.out.next_seq(),
        }
    }

    /// The client-side authentication: source, seat, token, staleness
    /// and replay.
    fn check<'a>(
        &self,
        buf: &'a [u8],
        from: SocketAddr,
        window: &mut SeqWindow,
    ) -> Result<(Header, &'a [u8]), Refuse> {
        let (h, payload) = decode_datagram(buf).ok_or(Refuse::Malformed)?;
        if from.ip().to_canonical() != self.host.ip().to_canonical()
            || from.port() != self.host.port()
        {
            return Err(Refuse::WrongAddr);
        }
        if h.seat != self.seat {
            return Err(Refuse::UnknownSeat);
        }
        if h.token != self.token {
            return Err(Refuse::BadToken);
        }
        if h.tag != LaneTag::Probe
            && !(is_reliable_tag(h.tag) && h.generation == LOBBY_SCOPE)
            && (h.generation == 0 || h.generation < self.started.load(Ordering::Relaxed))
        {
            return Err(Refuse::WrongGeneration);
        }
        if !window.accept(h.seq) {
            return Err(Refuse::Replayed);
        }
        Ok((h, payload))
    }

    fn recv_loop(&self, socket: &UdpSocket, mut window: SeqWindow) {
        let mut buf = [0u8; MAX_DATAGRAM + 1];
        while !self.stop.load(Ordering::Relaxed) {
            let (n, from) = match socket.recv_from(&mut buf) {
                Ok(r) => r,
                Err(e) if is_timeout(&e) => {
                    let now = Instant::now();
                    for a in std::iter::once(&self.assembler).chain(&self.world) {
                        a.lock().unwrap_or_else(|e| e.into_inner()).expire(now);
                    }
                    continue;
                }
                Err(_) => {
                    thread::sleep(POLL);
                    continue;
                }
            };
            Counters::bump(&self.counters.received);
            // Anything an earlier datagram delivered while the inbox
            // was full goes in now.
            self.drain_reliable();
            let (header, payload) = match self.check(&buf[..n], from, &mut window) {
                Ok(ok) => ok,
                Err(why) => {
                    self.counters.drop_for(why);
                    continue;
                }
            };
            match header.tag {
                // The echo of a path probe (or a late one of the
                // establishing probe): the path carries full datagrams.
                LaneTag::Probe => {
                    Counters::bump(&self.counters.accepted);
                    self.note_echo();
                }
                LaneTag::Data => match Message::decode(payload) {
                    Ok(msg) if host_lane_generation(&msg) == Some(header.generation) => {
                        Counters::bump(&self.counters.accepted);
                        self.inbox.push_lane(msg);
                    }
                    Ok(msg) if host_lane_generation(&msg).is_some() => {
                        self.counters.drop_for(Refuse::WrongGeneration);
                    }
                    Ok(_) => self.counters.drop_for(Refuse::Unexpected),
                    Err(_) => self.counters.drop_for(Refuse::Malformed),
                },
                LaneTag::Fragment => self.on_fragment(&header, payload),
                LaneTag::WorldCars | LaneTag::WorldSignals => {
                    self.on_world_fragment(&header, payload)
                }
                LaneTag::Ack => self.counters.drop_for(Refuse::Unexpected),
                LaneTag::Reliable => match decode_data(payload) {
                    Some((id, more, body)) => {
                        let ack = self
                            .rel
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .receiver
                            .on_data(id, more, body);
                        self.out
                            .push_reliable(LaneTag::ReliableAck, header.generation, ack);
                        Counters::bump(&self.counters.accepted);
                        self.drain_reliable();
                    }
                    None => {
                        self.rel
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .receiver
                            .note_malformed();
                        self.counters.drop_for(Refuse::Malformed);
                    }
                },
                LaneTag::ReliableAck => match decode_rel_ack(payload) {
                    Some((cum, mask)) => {
                        self.rel
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .sender
                            .on_ack(cum, mask);
                        Counters::bump(&self.counters.accepted);
                    }
                    None => self.counters.drop_for(Refuse::Malformed),
                },
            }
        }
    }

    /// Move reliably delivered host events (`Cnr`, `Impacts`) into the
    /// inbox while it has room; anything else stays queued in the
    /// receiver for [`ClientLane::recv_reliable`].
    fn drain_reliable(&self) {
        let mut rel = self.rel.lock().unwrap_or_else(|e| e.into_inner());
        for _ in 0..self.inbox.rel_room() {
            let Some(body) = rel.receiver.pop_if(is_host_event) else {
                break;
            };
            if let Ok(msg) = Message::decode(&body) {
                self.inbox.push_rel(msg);
            }
        }
    }

    /// One fragment: reassemble, and hand each completed group to the
    /// game as its own `Snap` the moment it is whole.
    fn on_fragment(&self, header: &Header, payload: &[u8]) {
        let done = self
            .assembler
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .accept(payload, Instant::now());
        match done {
            Ok(None) => Counters::bump(&self.counters.accepted),
            Ok(Some(group)) if decode_state_frame(&group.data).is_some() => {
                self.on_state(header, group.tick, &group.data)
            }
            Ok(Some(group)) => match Message::decode(&group.data) {
                Ok(msg @ Message::Snap { generation, .. }) if generation == header.generation => {
                    Counters::bump(&self.counters.accepted);
                    self.inbox.push_lane(msg);
                }
                Ok(Message::Snap { .. }) => self.counters.drop_for(Refuse::WrongGeneration),
                Ok(_) => self.counters.drop_for(Refuse::Unexpected),
                Err(_) => self.counters.drop_for(Refuse::Malformed),
            },
            Err(_) => self.counters.drop_for(Refuse::Malformed),
        }
    }

    /// A completed group that carries a baselined state: reconstruct it,
    /// hand the `Snap` to the game and acknowledge the tick. A delta whose
    /// base is not held is refused and *not* acknowledged, so the host
    /// keeps sending against the baseline it still believes in until its
    /// refresh sends a full state.
    fn on_state(&self, header: &Header, tick: u64, data: &[u8]) {
        let Some((group, epoch, payload)) = decode_state_frame(data) else {
            return self.counters.drop_for(Refuse::Malformed);
        };
        let applied = {
            let mut st = self.receivers.lock().unwrap_or_else(|e| e.into_inner());
            let r = st.receivers[usize::from(group)].apply(header.generation, epoch, tick, payload);
            match &r {
                Ok(_) => st.stats.applied += 1,
                Err(StateError::MissingBase(_)) => st.stats.missing_base += 1,
                Err(StateError::Stale) => st.stats.stale += 1,
                Err(_) => st.stats.malformed += 1,
            }
            r
        };
        match applied {
            Ok(bytes) => match Message::decode(&bytes) {
                Ok(msg @ Message::Snap { generation, .. }) if generation == header.generation => {
                    Counters::bump(&self.counters.accepted);
                    // Acknowledged only once applied, and queued before the
                    // game can see the snapshot.
                    self.send_ack_group(header.generation, epoch, group, tick);
                    self.inbox.push_lane(msg);
                }
                Ok(Message::Snap { .. }) => self.counters.drop_for(Refuse::WrongGeneration),
                Ok(_) => self.counters.drop_for(Refuse::Unexpected),
                Err(_) => self.counters.drop_for(Refuse::Malformed),
            },
            Err(StateError::Stale) => self.counters.drop_for(Refuse::Replayed),
            Err(_) => self.counters.drop_for(Refuse::Malformed),
        }
    }

    /// Baselined state counters of the client side.
    pub(crate) fn state_stats(&self) -> StateStats {
        self.receivers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stats
    }

    /// One fragment of a world-state frame: reassembled by its kind's own
    /// assembler, admitted only as that kind and under the datagram's
    /// generation, and queued for the game like a `Snap`. The game's
    /// freshness rules (newest tick per generation) are unchanged.
    fn on_world_fragment(&self, header: &Header, payload: &[u8]) {
        let Some(kind) = WorldKind::from_tag(header.tag) else {
            return self.counters.drop_for(Refuse::Unexpected);
        };
        let done = self.world[kind.index()]
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .accept(payload, Instant::now());
        match done {
            Ok(None) => Counters::bump(&self.counters.accepted),
            Ok(Some(group)) => match Message::decode(&group.data) {
                Ok(msg) => match kind.generation_of(&msg) {
                    Some(g) if g == header.generation => {
                        Counters::bump(&self.counters.accepted);
                        self.inbox.push_lane(msg);
                    }
                    Some(_) => self.counters.drop_for(Refuse::WrongGeneration),
                    None => self.counters.drop_for(Refuse::Unexpected),
                },
                Err(_) => self.counters.drop_for(Refuse::Malformed),
            },
            Err(_) => self.counters.drop_for(Refuse::Malformed),
        }
    }

    fn note_echo(&self) {
        *self.last_echo.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    /// How long the host's probe echoes have been missing, once that
    /// exceeds the failure bound: the real-time path cannot carry
    /// full-size datagrams (or any), so fragmented snapshots are not
    /// arriving. `None` while the path is healthy.
    pub(crate) fn path_failure(&self) -> Option<NetError> {
        let silent = self
            .last_echo
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed();
        (silent > Duration::from_millis(self.fail_after_ms.load(Ordering::Relaxed))).then_some(
            NetError::LanePathFailed {
                host: self.host,
                silent,
            },
        )
    }

    /// Test seam: probe only when idle for `probe_every` (the keepalive
    /// behaviour), keeping wall-clock probes out of exact datagram counts.
    #[cfg(test)]
    pub(crate) fn set_idle_probe_only(&self) {
        self.idle_probe_only.store(true, Ordering::Relaxed);
    }

    /// Test seam: probe cadence and failure bound.
    #[cfg(test)]
    pub(crate) fn set_path_bounds(&self, probe_every: Duration, fail_after: Duration) {
        self.probe_every_ms
            .store(probe_every.as_millis() as u64, Ordering::Relaxed);
        self.fail_after_ms
            .store(fail_after.as_millis() as u64, Ordering::Relaxed);
    }

    /// Fragment reassembly counters, including expiry.
    pub(crate) fn assembly_stats(&self) -> AssemblerStats {
        self.assembler
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stats()
    }

    /// Acknowledge the state at (`epoch`, `tick`) of `generation` to the
    /// host. Unreliable like everything on the lane: the caller repeats
    /// it with each newer tick.
    pub(crate) fn send_ack(&self, generation: u64, epoch: u8, tick: u64) -> bool {
        self.send_ack_group(generation, epoch, 0, tick)
    }

    /// [`send_ack`](Self::send_ack) for logical snapshot `group`.
    pub(crate) fn send_ack_group(&self, generation: u64, epoch: u8, group: u8, tick: u64) -> bool {
        if self.stop.load(Ordering::Relaxed) {
            return false;
        }
        self.out.push_tagged(
            LaneTag::Ack,
            Priority::Movement,
            generation,
            encode_ack(epoch, group, tick),
        );
        true
    }

    fn send_loop(&self, socket: &UdpSocket) {
        // The host reaps a seat whose lane is silent (`HOST_LIVENESS_TIMEOUT`),
        // so an idle client — a lobby, a paused session — sends a probe
        // (which the host authenticates and echoes) when nothing else left
        // for a keepalive interval.
        let mut last_probe = Instant::now();
        let mut last_sent = Instant::now();
        while !self.stop.load(Ordering::Relaxed) {
            self.wake.wait(POLL);
            let due = self
                .rel
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .sender
                .poll(self.clock.now_ms());
            for (generation, payload) in due {
                self.out
                    .push_reliable(LaneTag::Reliable, generation, payload);
            }
            while let Some(item) = self.out.queue.pop() {
                let header = self.header(item.tag, item.generation);
                flush(socket, self.host, &header, &item.payload, &self.counters);
                last_sent = Instant::now();
            }
            // Probe on a fixed cadence, busy or idle: it keeps the host's
            // liveness fed and is the client's evidence the path still
            // carries full-size datagrams.
            let every = Duration::from_millis(self.probe_every_ms.load(Ordering::Relaxed));
            let quiet =
                !self.idle_probe_only.load(Ordering::Relaxed) || last_sent.elapsed() >= every;
            if last_probe.elapsed() >= every && quiet {
                let header = self.header(LaneTag::Probe, 0);
                flush(socket, self.host, &header, &probe_padding(), &self.counters);
                last_probe = Instant::now();
                last_sent = last_probe;
            }
        }
    }

    /// Queue an encoded `InputWindow` for the sender thread. `false` when
    /// the lane is closed (the caller then falls back to TCP, which
    /// reports the dead connection) or the payload exceeds a datagram.
    pub(crate) fn send_data(&self, generation: u64, payload: Vec<u8>) -> bool {
        if self.stop.load(Ordering::Relaxed) {
            return false;
        }
        if payload.len() > MAX_PAYLOAD {
            self.counters.note_oversize_tcp();
            return false;
        }
        self.out.push_data(generation, payload);
        true
    }

    /// Send `body` to the host on the reliable channel (NETPLAY-06E-1);
    /// see [`HostLane::send_reliable`].
    pub(crate) fn send_reliable(
        &self,
        generation: u64,
        body: &[u8],
    ) -> Result<(), ReliableSendError> {
        if self.stop.load(Ordering::Relaxed) {
            return Err(ReliableSendError::NoLane);
        }
        let payloads = self
            .rel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sender
            .send(self.clock.now_ms(), generation, body)?;
        for payload in payloads {
            self.out
                .push_reliable(LaneTag::Reliable, generation, payload);
        }
        Ok(())
    }

    pub(crate) fn recv_reliable(&self) -> Option<Vec<u8>> {
        let body = self
            .rel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .receiver
            .pop();
        if body.is_some() {
            // The popped message may have been blocking host events.
            self.drain_reliable();
        }
        body
    }

    pub(crate) fn reliable_stats(&self) -> ReliableStats {
        self.rel.lock().unwrap_or_else(|e| e.into_inner()).stats()
    }

    pub(crate) fn clock(&self) -> &ReliableClock {
        &self.clock
    }

    pub(crate) fn poke(&self) {
        self.wake.notify();
    }

    pub(crate) fn note_started(&self, generation: u64) {
        self.started.fetch_max(generation, Ordering::Relaxed);
        self.retire_through(generation.saturating_sub(1));
    }

    /// Retire the unacked reliable messages minted for generations up to
    /// and including `generation`, which have ended (a newer `Start` or a
    /// `Cancel`); see `ReliableSender::retire`.
    pub(crate) fn retire_through(&self, generation: u64) {
        self.rel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sender
            .retire(self.clock.now_ms(), |g| g <= generation);
        self.wake.notify();
    }

    pub(crate) fn counts(&self) -> LaneCounts {
        self.counters.snapshot()
    }

    pub(crate) fn out_stats(&self) -> QueueStats {
        self.out.stats()
    }

    pub(crate) fn close(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.wake.notify();
        self.inbox.close();
    }
}

impl Drop for ClientLaneGuard {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Closes a client's lane when the owning `Client` goes away; the lane
/// threads see the flag within one [`POLL`].
#[derive(Debug)]
pub(crate) struct ClientLaneGuard(pub(crate) Arc<ClientLane>);

/// The generation of a host→client message that may ride the datagram
/// lane (`Snap` and, v34, `RemoteCommands`); `None` for everything else,
/// which stays on the control stream.
pub(crate) fn host_lane_generation(msg: &Message) -> Option<u64> {
    match msg {
        Message::Snap { generation, .. } | Message::RemoteCommands { generation, .. } => {
            Some(*generation)
        }
        _ => None,
    }
}

/// Encode `msg` for the lane, or `None` when it exceeds a datagram's
/// payload budget.
pub(crate) fn lane_payload(msg: &Message) -> Result<Option<Vec<u8>>, ProtoError> {
    let bytes = msg.encode()?;
    Ok((bytes.len() <= MAX_PAYLOAD).then_some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(tag: LaneTag, seq: u64) -> Header {
        Header {
            tag,
            generation: 7,
            seat: 3,
            token: 0xdead_beef_cafe,
            seq,
        }
    }

    #[test]
    fn datagram_roundtrips_and_header_is_fixed_size() {
        let h = header(LaneTag::Data, 9);
        let d = encode_datagram(&h, b"abc").unwrap();
        assert_eq!(d.len(), HEADER_LEN + 3);
        assert_eq!(decode_datagram(&d), Some((h, &b"abc"[..])));
        assert_eq!(encode_datagram(&h, &[]).unwrap().len(), HEADER_LEN);
    }

    #[test]
    fn payload_budget_is_enforced_both_ways() {
        let h = header(LaneTag::Data, 1);
        assert!(encode_datagram(&h, &vec![0; MAX_PAYLOAD]).is_some());
        assert!(encode_datagram(&h, &vec![0; MAX_PAYLOAD + 1]).is_none());
        let mut over = encode_datagram(&h, &[]).unwrap();
        over.resize(MAX_DATAGRAM + 1, 0);
        assert!(decode_datagram(&over).is_none());
    }

    #[test]
    fn malformed_datagrams_are_refused() {
        let good = encode_datagram(&header(LaneTag::Data, 1), b"x").unwrap();
        assert!(decode_datagram(&good[..HEADER_LEN - 1]).is_none());
        let mut bad = good.clone();
        bad[0] ^= 1;
        assert!(decode_datagram(&bad).is_none(), "magic");
        let mut bad = good.clone();
        bad[4] = LANE_VERSION + 1;
        assert!(decode_datagram(&bad).is_none(), "version");
        let mut bad = good.clone();
        bad[5] = 99;
        assert!(decode_datagram(&bad).is_none(), "tag");
        assert!(
            decode_datagram(&encode_datagram(&header(LaneTag::Data, 0), b"").unwrap()).is_none(),
            "sequence 0"
        );
    }

    #[test]
    fn replay_window_rejects_duplicates_and_old_but_takes_reordered() {
        let mut w = SeqWindow::default();
        assert!(!w.accept(0));
        assert!(w.accept(10));
        assert!(!w.accept(10), "duplicate");
        assert!(w.accept(8), "reordered inside the window");
        assert!(!w.accept(8), "reordered duplicate");
        assert!(w.accept(10 + SEQ_WINDOW), "jump ahead");
        assert!(!w.accept(10), "now out of the window");
        assert!(w.accept(11 + 1), "inside the new window");
        assert!(w.accept(10 + 3 * SEQ_WINDOW), "jump past the whole window");
        assert!(!w.accept(10 + SEQ_WINDOW), "old after a big jump");
    }

    #[test]
    fn queue_holds_its_bound_and_drops_oldest() {
        let q = LaneQueue::new(3);
        for i in 0..10u32 {
            q.push(i);
            assert!(q.stats().len <= 3);
        }
        let s = q.stats();
        assert_eq!((s.len, s.capacity, s.pushed, s.dropped), (3, 3, 10, 7));
        assert_eq!(
            (q.pop(), q.pop(), q.pop(), q.pop()),
            (Some(7), Some(8), Some(9), None)
        );
        assert!(q.stats().max_wait >= Duration::ZERO);
    }

    #[test]
    fn queue_age_is_reported() {
        let q = LaneQueue::new(2);
        q.push(1);
        thread::sleep(Duration::from_millis(20));
        assert!(q.stats().oldest_age >= Duration::from_millis(20));
        q.pop();
        let s = q.stats();
        assert_eq!(s.oldest_age, Duration::ZERO);
        assert!(s.max_wait >= Duration::from_millis(20));
    }

    #[test]
    fn tokens_are_nonzero_and_distinct() {
        let tokens: Vec<u64> = (0..64).map(|_| mint_token(1)).collect();
        assert!(tokens.iter().all(|t| *t != 0));
        let mut sorted = tokens.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), tokens.len());
    }

    #[test]
    fn inbox_hands_over_lane_delivered_messages_before_a_control_stream_end() {
        let inbox = Inbox::new();
        inbox.push_rel(Message::Cancel { generation: 1 });
        inbox.push_lane(Message::Cancel { generation: 2 });
        assert!(inbox.push_tcp(Err(NetError::Io(io::Error::from(
            io::ErrorKind::UnexpectedEof
        )))));
        assert_eq!(
            inbox.pop(Some(Duration::ZERO)).unwrap(),
            Message::Cancel { generation: 1 }
        );
        assert_eq!(
            inbox.pop(Some(Duration::ZERO)).unwrap(),
            Message::Cancel { generation: 2 }
        );
        assert!(matches!(
            inbox.pop(Some(Duration::ZERO)),
            Err(NetError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof
        ));
    }

    #[test]
    fn inbox_merges_reliable_first_and_bounds_the_lane_side() {
        let inbox = Inbox::new();
        for i in 0..(QUEUE_CAP as u64 + 5) {
            inbox.push_lane(Message::Cancel { generation: i });
        }
        let s = inbox.lane_stats();
        assert_eq!((s.len, s.dropped), (QUEUE_CAP, 5));
        assert!(inbox.push_tcp(Ok(Message::Leave)));
        assert_eq!(inbox.pop(Some(Duration::ZERO)).unwrap(), Message::Leave);
        assert_eq!(
            inbox.pop(Some(Duration::ZERO)).unwrap(),
            Message::Cancel { generation: 5 },
            "oldest five were dropped"
        );
    }

    #[test]
    fn inbox_times_out_and_closes() {
        let inbox = Inbox::new();
        let Err(NetError::Io(e)) = inbox.pop(Some(Duration::from_millis(10))) else {
            panic!("expected a timeout");
        };
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        inbox.close();
        assert!(!inbox.push_tcp(Ok(Message::Leave)));
        let Err(NetError::Io(e)) = inbox.pop(None) else {
            panic!("expected closed");
        };
        assert_eq!(e.kind(), io::ErrorKind::ConnectionAborted);
    }
}
