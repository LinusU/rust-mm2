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
//! complete encoded `Message`; a [`LaneTag::Probe`] carries none.

use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::lobby::{PeerMail, RemoteInputs};
use crate::proto::Message;
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
/// Socket read timeout and queue wait: how often a lane thread notices
/// its stop flag.
const POLL: Duration = Duration::from_millis(50);
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
}

impl LaneTag {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Probe),
            2 => Some(Self::Data),
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
}

#[derive(Debug)]
struct Bounded<T> {
    items: VecDeque<(Instant, T)>,
    cap: usize,
    pushed: u64,
    dropped: u64,
    max_wait: Duration,
}

impl<T> Bounded<T> {
    fn new(cap: usize) -> Self {
        Self {
            items: VecDeque::new(),
            cap,
            pushed: 0,
            dropped: 0,
            max_wait: Duration::ZERO,
        }
    }

    fn push(&mut self, item: T) {
        self.pushed += 1;
        if self.items.len() >= self.cap {
            self.items.pop_front();
            self.dropped += 1;
        }
        self.items.push_back((Instant::now(), item));
    }

    fn pop(&mut self) -> Option<T> {
        let (at, item) = self.items.pop_front()?;
        self.max_wait = self.max_wait.max(at.elapsed());
        Some(item)
    }

    fn stats(&self) -> QueueStats {
        QueueStats {
            len: self.items.len(),
            capacity: self.cap,
            pushed: self.pushed,
            dropped: self.dropped,
            oldest_age: self
                .items
                .front()
                .map_or(Duration::ZERO, |(at, _)| at.elapsed()),
            max_wait: self.max_wait,
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
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(item);
        if let Some(wake) = &self.wake {
            wake.notify();
        }
    }

    /// Dequeue the oldest entry, if any.
    pub fn pop(&self) -> Option<T> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).pop()
    }

    /// Occupancy, drop and queue-age counters.
    pub fn stats(&self) -> QueueStats {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).stats()
    }
}

/// One outbound datagram waiting for the sender thread.
#[derive(Debug)]
pub(crate) struct OutItem {
    tag: LaneTag,
    generation: u64,
    payload: Vec<u8>,
}

/// A lane direction's outbound side: its queue and datagram sequence.
#[derive(Debug)]
pub(crate) struct SeatOut {
    queue: LaneQueue<OutItem>,
    seq: AtomicU64,
}

impl SeatOut {
    fn new(wake: Arc<Wake>) -> Self {
        Self {
            queue: LaneQueue::with_wake(QUEUE_CAP, wake),
            seq: AtomicU64::new(0),
        }
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub(crate) fn push_data(&self, generation: u64, payload: Vec<u8>) {
        self.queue.push(OutItem {
            tag: LaneTag::Data,
            generation,
            payload,
        });
    }

    pub(crate) fn stats(&self) -> QueueStats {
        self.queue.stats()
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
    window: SeqWindow,
    out: Arc<SeatOut>,
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
}

/// What the host did with a datagram, for the receive loop's counters.
enum Verdict {
    Accepted,
    Dropped(Refuse),
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
    if h.tag == LaneTag::Data && (generation == 0 || h.generation != generation) {
        return Verdict::Dropped(Refuse::WrongGeneration);
    }
    if !seat.window.accept(h.seq) {
        return Verdict::Dropped(Refuse::Replayed);
    }
    seat.established = true;
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
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl HostLane {
    /// Bind `ip:0` and start the receive and send threads.
    pub(crate) fn bind(ip: IpAddr, inputs: RemoteInputs) -> io::Result<Arc<Self>> {
        let socket = Arc::new(UdpSocket::bind(SocketAddr::new(ip, 0))?);
        socket.set_read_timeout(Some(POLL))?;
        let lane = Arc::new(Self {
            port: socket.local_addr()?.port(),
            inputs,
            counters: Counters::default(),
            stop: AtomicBool::new(false),
            wake: Arc::new(Wake::default()),
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

    pub(crate) fn counters(&self) -> &Counters {
        &self.counters
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
                window: SeqWindow::default(),
                out: Arc::new(SeatOut::new(self.wake.clone())),
            },
        );
        token
    }

    /// Set the generation datagrams must name (0 = no session).
    pub(crate) fn set_generation(&self, generation: u64) {
        self.inputs
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .lane
            .generation = generation;
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

    pub(crate) fn is_established(&self, id: u16) -> bool {
        self.established_out(id).is_some()
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
        while !self.stop.load(Ordering::Relaxed) {
            self.wake.wait(POLL);
            let targets: Vec<(u16, SocketAddr, u64, Arc<SeatOut>)> = {
                let mail = self.inputs.inner.lock().unwrap_or_else(|e| e.into_inner());
                mail.lane
                    .seats
                    .iter()
                    .filter(|(_, s)| s.established)
                    .map(|(id, s)| (*id, SocketAddr::new(s.ip, s.port), s.token, s.out.clone()))
                    .collect()
            };
            for (seat, addr, token, out) in targets {
                while let Some(item) = out.queue.pop() {
                    let header = Header {
                        tag: item.tag,
                        generation: item.generation,
                        seat,
                        token,
                        seq: out.next_seq(),
                    };
                    flush(socket, addr, &header, &item.payload, &self.counters);
                }
            }
        }
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
    lane: Bounded<Message>,
    closed: bool,
}

impl Inbox {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(InboxState {
                tcp: VecDeque::new(),
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

    fn push_lane(&self, msg: Message) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .lane
            .push(msg);
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
            .stats()
    }

    /// The next message, reliable side first. `timeout` bounds the wait
    /// (`None`: until something arrives or the inbox closes).
    pub(crate) fn pop(&self, timeout: Option<Duration>) -> Result<Message, NetError> {
        let deadline = timeout.map(|t| Instant::now() + t);
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(item) = st.tcp.pop_front() {
                self.cv.notify_all();
                return item;
            }
            if let Some(msg) = st.lane.pop() {
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
            out: SeatOut::new(wake.clone()),
            wake,
            counters: Counters::default(),
            stop: AtomicBool::new(false),
            started: AtomicU64::new(0),
            inbox,
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
        if h.tag == LaneTag::Data
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
                Err(e) if is_timeout(&e) => continue,
                Err(_) => {
                    thread::sleep(POLL);
                    continue;
                }
            };
            Counters::bump(&self.counters.received);
            let (header, payload) = match self.check(&buf[..n], from, &mut window) {
                Ok(ok) => ok,
                Err(why) => {
                    self.counters.drop_for(why);
                    continue;
                }
            };
            match header.tag {
                // A late duplicate echo of the establishing probe.
                LaneTag::Probe => Counters::bump(&self.counters.accepted),
                LaneTag::Data => match Message::decode(payload) {
                    Ok(msg @ Message::Snap { generation, .. })
                        if generation == header.generation =>
                    {
                        Counters::bump(&self.counters.accepted);
                        self.inbox.push_lane(msg);
                    }
                    Ok(Message::Snap { .. }) => self.counters.drop_for(Refuse::WrongGeneration),
                    Ok(_) => self.counters.drop_for(Refuse::Unexpected),
                    Err(_) => self.counters.drop_for(Refuse::Malformed),
                },
            }
        }
    }

    fn send_loop(&self, socket: &UdpSocket) {
        while !self.stop.load(Ordering::Relaxed) {
            self.wake.wait(POLL);
            while let Some(item) = self.out.queue.pop() {
                let header = self.header(item.tag, item.generation);
                flush(socket, self.host, &header, &item.payload, &self.counters);
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

    pub(crate) fn note_started(&self, generation: u64) {
        self.started.fetch_max(generation, Ordering::Relaxed);
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
