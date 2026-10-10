//! A deterministic network-impairment harness (F25-B, spec req 6).
//!
//! The transport is ordered reliable TCP, so real loss, duplication and
//! reorder cannot occur on the wire — but the protocol is meant to
//! survive them (stale `Snap` ticks drop, the input mailbox is
//! latest-wins by sender `seq`, a `ResetRequest` is idempotent), and a
//! future unreliable dataplane would face them for real. [`ImpairProxy`]
//! manufactures those conditions in-process: it is a framed TCP relay
//! that sits between a peer and a host — the peer dials the proxy's
//! loopback address, the proxy dials the real target — and each
//! direction's frames pass through a per-connection *lane* that applies
//! an [`Impair`] recipe:
//!
//! - `delay`/`jitter` — every frame earns a release instant
//!   (`now + delay + uniform(0, jitter)`); the lane emits a frame when
//!   its release comes due, so jitter past a neighbour's release is a
//!   real reorder.
//! - `loss` — a frame drops outright. Whole-frame loss keeps the stream
//!   well-framed while removing exactly what the receiver would have
//!   seen — the application-level effect of a dropped datagram.
//! - `duplicate` — a frame emits twice, the copies adjacent.
//! - `reorder` — a frame defers to its successor: the pair swaps order.
//!   The successor is consumed by the swap (it draws no knobs of its
//!   own), so `reorder: 1.0` is a deterministic pairwise exchange. A
//!   deferred frame whose successor never comes emits at a bounded
//!   deadline ([`HOLD_CAP`]) rather than hanging the stream.
//!
//! Decisions come from a per-lane SplitMix64 stream seeded at
//! construction, so a fixed seed replays an identical impairment
//! pattern over identical traffic — timing never is, and tests assert
//! counts/order, not wall-clock sequences. `Proxy::set` retunes a live
//! direction, so a scenario can handshake cleanly and only then impair
//! the data plane. [`LinkStats`] counts what the recipe actually did —
//! a leg asserts the impairment *happened*, not just that the session
//! coped.
//!
//! The proxy relays opaque frame payloads without decoding them — it
//! impairs whatever the wire carries, present and future messages
//! alike. It is a test/harness tool, not production plumbing: callers
//! bind it loopback like every other listener in this crate.

use std::collections::BinaryHeap;
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::frame::{read_frame, write_frame};

/// How long a reorder-deferred frame waits for the successor it swaps
/// with before emitting anyway — a stall bound on a silent sender, not
/// part of the recipe.
const HOLD_CAP: Duration = Duration::from_millis(100);

/// Pending-frame bound per lane (held frame included). A long `delay`
/// recipe behind a fast sender accumulates releases faster than they
/// emit; the cap drops the excess rather than queueing unboundedly —
/// `LinkStats::overflowed` counts them so a capped run shows it.
const MAX_QUEUED: usize = 4096;

/// The writer loop's maximum sleep quantum — short enough that a
/// `set()` retune or a shutdown lands promptly even with nothing due.
const PUMP_QUANTUM: Duration = Duration::from_millis(5);

/// Which way a frame is travelling through the proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkDir {
    /// Accepted peer → target. In front of a host this is the
    /// client→host direction: `Hello`, lobby verbs, `Input`,
    /// `ResetRequest`.
    Up = 0,
    /// Target → accepted peer. In front of a host: `Roster`,
    /// `Start`/`Cancel`, `Snap`.
    Down = 1,
}

/// One direction's impairment recipe. `Impair::default()` is a
/// transparent relay — zero delay, no drops. Probabilities are clamped
/// to `[0, 1]` when applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Impair {
    /// Latency added to every frame's release.
    pub delay: Duration,
    /// Uniform extra release delay sampled in `[0, jitter)`.
    pub jitter: Duration,
    /// Probability a frame is dropped outright.
    pub loss: f64,
    /// Probability a frame emits a second adjacent copy.
    pub duplicate: f64,
    /// Probability a frame defers to its successor — the pair swaps.
    pub reorder: f64,
}

impl Default for Impair {
    fn default() -> Self {
        Self {
            delay: Duration::ZERO,
            jitter: Duration::ZERO,
            loss: 0.0,
            duplicate: 0.0,
            reorder: 0.0,
        }
    }
}

/// What a lane actually did — the measured half of an impairment leg.
/// `frames_out` counts emitted copies, so a duplicating run reports
/// `frames_out = frames_in − dropped + duplicated` modulo what is still
/// queued. `bytes_*` are the payload byte counts behind those frame
/// counts (the 4-byte length prefix excluded) — the bandwidth leg of
/// F25-B req 6's budget evidence.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkStats {
    /// Frames read off the source socket.
    pub frames_in: u64,
    /// Frames written onward — each emitted copy counts.
    pub frames_out: u64,
    /// Payload bytes read off the source socket.
    pub bytes_in: u64,
    /// Payload bytes written onward — each emitted copy counts.
    pub bytes_out: u64,
    /// Frames scheduled onto the release queue (or reorder-held) with a
    /// positive hold — delay/jitter the recipe actually made the lane
    /// pay. Each duplicated copy counts.
    pub delayed: u64,
    /// Frames the loss draw dropped.
    pub dropped: u64,
    /// Extra copies the duplicate draw emitted.
    pub duplicated: u64,
    /// Pairs the reorder draw swapped.
    pub reordered: u64,
    /// Frames dropped because the pending queue was at [`MAX_QUEUED`].
    pub overflowed: u64,
}

impl LinkStats {
    fn absorb(&mut self, other: &LinkStats) {
        self.frames_in += other.frames_in;
        self.frames_out += other.frames_out;
        self.bytes_in += other.bytes_in;
        self.bytes_out += other.bytes_out;
        self.delayed += other.delayed;
        self.dropped += other.dropped;
        self.duplicated += other.duplicated;
        self.reordered += other.reordered;
        self.overflowed += other.overflowed;
    }
}

/// SplitMix64 — a tiny deterministic stream so a recipe's *decisions*
/// replay from a seed. Not cryptographic; this is test scaffolding.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform draw in `[0, 1)`.
    fn f64(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A uniform duration in `[0, bound)`.
    fn jitter(&mut self, bound: Duration) -> Duration {
        bound.mul_f64(self.f64())
    }

    /// Whether a `p`-probability event fires — `p` clamped to `[0, 1]`.
    fn chance(&mut self, p: f64) -> bool {
        self.f64() < p.clamp(0.0, 1.0)
    }
}

/// A frame waiting for its release instant; `order` is arrival order —
/// the tiebreak that keeps a swap's held frame behind the successor it
/// deferred to when both carry the same release.
struct Queued {
    release: Instant,
    order: u64,
    payload: Vec<u8>,
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.release == other.release && self.order == other.order
    }
}
impl Eq for Queued {}
impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Queued {
    /// Inverted: `BinaryHeap` is max-first, the lane wants earliest
    /// release (then earliest arrival) first.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .release
            .cmp(&self.release)
            .then(other.order.cmp(&self.order))
    }
}

/// One direction's pending-release queue, deferral slot, draw state and
/// counters for a single relayed connection.
struct Lane {
    rng: Rng,
    heap: BinaryHeap<Queued>,
    /// A reorder-deferred frame and the deadline it emits at even if no
    /// successor ever arrives.
    held: Option<(Vec<u8>, Instant)>,
    order: u64,
    cap: usize,
    stats: LinkStats,
}

impl Lane {
    fn new(seed: u64, cap: usize) -> Self {
        Self {
            rng: Rng(seed),
            heap: BinaryHeap::new(),
            held: None,
            order: 0,
            cap,
            stats: LinkStats::default(),
        }
    }

    fn push(&mut self, release: Instant, payload: Vec<u8>) {
        if self.heap.len() + usize::from(self.held.is_some()) >= self.cap {
            self.stats.overflowed += 1;
            return;
        }
        self.order += 1;
        self.heap.push(Queued {
            release,
            order: self.order,
            payload,
        });
    }

    /// Classify one inbound frame at `now` under `params` — queue it,
    /// hold it for a swap, duplicate it or drop it.
    fn offer(&mut self, params: &Impair, now: Instant, payload: Vec<u8>) {
        self.stats.frames_in += 1;
        self.stats.bytes_in += payload.len() as u64;
        // A deferred frame's successor arrived: the swap completes —
        // the successor emits at its own release and the held frame
        // goes right behind it (same release, later `order`). The
        // successor is consumed by the swap; it draws no knobs itself.
        if let Some((held, _deadline)) = self.held.take() {
            let release = now + params.delay + self.rng.jitter(params.jitter);
            self.push(release, payload);
            self.push(release, held);
            self.stats.reordered += 1;
            if release > now {
                self.stats.delayed += 2;
            }
            return;
        }
        if self.rng.chance(params.loss) {
            self.stats.dropped += 1;
            return;
        }
        let release = now + params.delay + self.rng.jitter(params.jitter);
        if release > now {
            self.stats.delayed += 1;
        }
        if self.rng.chance(params.duplicate) {
            if release > now {
                self.stats.delayed += 1;
            }
            self.push(release, payload.clone());
            self.stats.duplicated += 1;
        }
        if self.rng.chance(params.reorder) {
            self.held = Some((payload, release + HOLD_CAP));
        } else {
            self.push(release, payload);
        }
    }

    /// Append every frame whose release has come due to `out`, in
    /// release order. A held frame whose deadline passed joins the due
    /// set — a stalled stream must not hold it forever.
    fn collect_due(&mut self, now: Instant, out: &mut Vec<Vec<u8>>) {
        if let Some((_, deadline)) = &self.held
            && *deadline <= now
        {
            let (payload, release) = self.held.take().unwrap();
            self.push(release, payload);
        }
        while let Some(q) = self.heap.peek()
            && q.release <= now
        {
            out.push(self.heap.pop().unwrap().payload);
        }
    }

    /// When the next pending frame becomes due — the writer loop's
    /// sleep bound.
    fn next_due(&self) -> Option<Instant> {
        self.heap
            .peek()
            .map(|q| q.release)
            .into_iter()
            .chain(self.held.as_ref().map(|(_, d)| *d))
            .min()
    }

    /// End of stream: everything still pending emits immediately, in
    /// release order — a close must not strand the tail of a clean
    /// stream (a `Leave` behind a delay still owes the peer its FIN).
    fn finish(&mut self, out: &mut Vec<Vec<u8>>) {
        if let Some((payload, release)) = self.held.take() {
            self.push(release, payload);
        }
        while let Some(q) = self.heap.pop() {
            out.push(q.payload);
        }
    }
}

/// The proxy's shared mutable state: the live recipe per direction, one
/// stats slot per spawned lane (slot parity is the lane's
/// [`LinkDir`], so several connections' lanes sum correctly on read),
/// and the stop flag every thread polls.
struct Shared {
    params: Mutex<[Impair; 2]>,
    stats: Mutex<Vec<LinkStats>>,
    stop: AtomicBool,
}

impl Shared {
    fn recipe(&self, dir: LinkDir) -> Impair {
        self.params.lock().unwrap_or_else(|e| e.into_inner())[dir as usize]
    }

    /// A fresh zeroed slot; its index is the lane's identity.
    fn alloc_stats(&self) -> usize {
        let mut stats = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        stats.push(LinkStats::default());
        stats.len() - 1
    }

    fn publish(&self, slot: usize, stats: LinkStats) {
        self.stats.lock().unwrap_or_else(|e| e.into_inner())[slot] = stats;
    }

    /// Every `dir` lane's counters summed.
    fn stats(&self, dir: LinkDir) -> LinkStats {
        self.stats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .enumerate()
            .filter(|(idx, _)| idx % 2 == dir as usize)
            .fold(LinkStats::default(), |mut acc, (_, s)| {
                acc.absorb(s);
                acc
            })
    }
}

/// A framed TCP relay with a per-direction, per-connection impairment
/// lane — see the module docs. Bind with [`Self::loopback`] /
/// [`Self::loopback_seeded`], point the client at [`Self::addr`], retune
/// mid-run with [`Self::set`], and read what happened from
/// [`Self::stats`]. Dropping the proxy stops the accept loop and every
/// lane and joins their threads.
pub struct ImpairProxy {
    addr: SocketAddr,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
    /// Every lane thread — writers first so a `send`-blocked reader can
    /// never outlive the receiver that would free it.
    lanes: Arc<Mutex<Vec<JoinHandle<()>>>>,
    /// A clone of every relayed socket — shutdown forces the blocked
    /// readers awake on drop.
    sockets: Arc<Mutex<Vec<TcpStream>>>,
}

impl ImpairProxy {
    /// Relay connections accepted on an ephemeral loopback port to
    /// `target`, with a deterministic seed for every lane's draws. The
    /// recipe starts transparent — `set` installs impairment.
    pub fn loopback_seeded(target: SocketAddr, seed: u64) -> io::Result<Self> {
        let listener = TcpListener::bind(SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0)))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            params: Mutex::new([Impair::default(), Impair::default()]),
            stats: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
        });
        let lanes: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));
        let sockets: Arc<Mutex<Vec<TcpStream>>> = Arc::new(Mutex::new(Vec::new()));
        let accept = {
            let shared = shared.clone();
            let lanes = lanes.clone();
            let sockets = sockets.clone();
            thread::spawn(move || {
                let mut conn_idx = 0u64;
                while !shared.stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((peer, _)) => {
                            if relay(peer, target, seed, conn_idx, &shared, &lanes, &sockets)
                                .is_ok()
                            {
                                conn_idx += 1;
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(1));
                        }
                        Err(_) => break,
                    }
                }
            })
        };
        Ok(Self {
            addr,
            shared,
            accept: Some(accept),
            lanes,
            sockets,
        })
    }

    /// [`Self::loopback_seeded`] with the fixed seed `0` — transparent
    /// until `set`, deterministic once impaired.
    pub fn loopback(target: SocketAddr) -> io::Result<Self> {
        Self::loopback_seeded(target, 0)
    }

    /// The address a peer dials to reach the target through this proxy.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Replace one direction's recipe, live: the next classified frame
    /// reads it, so a scenario can handshake cleanly and impair only the
    /// session phase.
    pub fn set(&self, dir: LinkDir, impair: Impair) {
        self.shared.params.lock().unwrap_or_else(|e| e.into_inner())[dir as usize] = impair;
    }

    /// The recipe `dir` currently runs.
    pub fn impair(&self, dir: LinkDir) -> Impair {
        self.shared.recipe(dir)
    }

    /// What `dir`'s lanes have done so far — summed across every
    /// connection they have relayed.
    pub fn stats(&self, dir: LinkDir) -> LinkStats {
        self.shared.stats(dir)
    }
}

impl Drop for ImpairProxy {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        // Force every relayed socket closed — the lane readers are
        // blocked in `read_frame` on them and only a socket error wakes
        // one; the lane writers see the flag inside a PUMP_QUANTUM.
        for socket in self
            .sockets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
        {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
        for handle in self
            .lanes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
        {
            let _ = handle.join();
        }
    }
}

/// Wire one accepted peer socket to `target` through a lane pair —
/// `Up` reads the peer and writes the target, `Down` the reverse.
/// Returns `Err` (and the accept loop drops the peer) when the target
/// is unreachable.
fn relay(
    peer: TcpStream,
    target: SocketAddr,
    seed: u64,
    conn_idx: u64,
    shared: &Arc<Shared>,
    lanes: &Arc<Mutex<Vec<JoinHandle<()>>>>,
    sockets: &Arc<Mutex<Vec<TcpStream>>>,
) -> io::Result<()> {
    // The accept loop polls the listener nonblocking, and an accepted
    // socket inherits that flag — restore blocking I/O or the lane
    // reader's `read_frame` dies on `WouldBlock` the moment the buffer
    // drains and the whole lane follows.
    peer.set_nonblocking(false)?;
    let upstream = TcpStream::connect(target)?;
    // The same latency sensitivity as the real transport — relayed
    // frames must not sit in Nagle's buffer.
    upstream.set_nodelay(true)?;
    peer.set_nodelay(true)?;
    sockets
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .extend([peer.try_clone()?, upstream.try_clone()?]);
    let mut handles = lanes.lock().unwrap_or_else(|e| e.into_inner());
    spawn_lane(
        LinkDir::Up,
        peer.try_clone()?,
        upstream.try_clone()?,
        lane_seed(seed, conn_idx, LinkDir::Up),
        shared,
        &mut handles,
    );
    spawn_lane(
        LinkDir::Down,
        upstream,
        peer,
        lane_seed(seed, conn_idx, LinkDir::Down),
        shared,
        &mut handles,
    );
    Ok(())
}

/// Each lane gets its own draw stream — per connection, per direction,
/// all derived from the proxy's seed.
fn lane_seed(seed: u64, conn_idx: u64, dir: LinkDir) -> u64 {
    seed ^ conn_idx
        .wrapping_mul(2)
        .wrapping_add(dir as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// One lane is two threads: a reader that only frames the source into a
/// bounded channel (blocking `read_frame` — a timed partial read would
/// corrupt the stream, so reads stay unsplittable), and a writer that
/// classifies, schedules and emits. The channel bound back-pressures a
/// sender that outruns the release queue rather than buffering
/// unboundedly ahead of it.
fn spawn_lane(
    dir: LinkDir,
    mut src: TcpStream,
    mut dst: TcpStream,
    seed: u64,
    shared: &Arc<Shared>,
    lanes: &mut Vec<JoinHandle<()>>,
) {
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(MAX_QUEUED);
    let slot = shared.alloc_stats();
    let reader = thread::spawn(move || {
        // A writer that ended early (its destination closed) drops the
        // receiver, but the source keeps being read and discarded: a
        // relay whose far side vanished must not stop reading, or the
        // sender's socket buffer fills and the sender stalls on a write
        // the vanished peer will never take — and, if it serves other
        // peers from one loop, starves them (the reset leg's control
        // client lost its host that way).
        let mut sink = false;
        while let Ok(payload) = read_frame(&mut src) {
            if !sink && tx.send(payload).is_err() {
                sink = true;
            }
        }
    });
    let shared_w = shared.clone();
    let writer = thread::spawn(move || {
        let mut lane = Lane::new(seed, MAX_QUEUED);
        let mut out: Vec<Vec<u8>> = Vec::new();
        let mut open = true;
        while open {
            if shared_w.stop.load(Ordering::Relaxed) {
                break;
            }
            let now = Instant::now();
            lane.collect_due(now, &mut out);
            for payload in out.drain(..) {
                lane.stats.frames_out += 1;
                lane.stats.bytes_out += payload.len() as u64;
                if write_frame(&mut dst, &payload).is_err() {
                    open = false;
                    break;
                }
            }
            shared_w.publish(slot, lane.stats);
            if !open {
                break;
            }
            let wait = lane
                .next_due()
                .map(|due| due.saturating_duration_since(now))
                .unwrap_or(PUMP_QUANTUM)
                .min(PUMP_QUANTUM);
            match rx.recv_timeout(wait.max(Duration::from_micros(50))) {
                Ok(payload) => lane.offer(&shared_w.recipe(dir), Instant::now(), payload),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => open = false,
            }
        }
        // Stream end or proxy shutdown: flush what the queue still
        // held so a clean close is a clean close.
        lane.finish(&mut out);
        for payload in out.drain(..) {
            lane.stats.frames_out += 1;
            lane.stats.bytes_out += payload.len() as u64;
            let _ = write_frame(&mut dst, &payload);
        }
        shared_w.publish(slot, lane.stats);
    });
    // Writers first: joining on drop frees the channel receivers a
    // blocked reader's `send` waits on.
    lanes.extend([writer, reader]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::listen_loopback;
    use crate::proto::DriveInput;
    use crate::{Client, Host, HostConfig, hello};
    use std::sync::mpsc::{Receiver, SyncSender};

    const WAIT: Duration = Duration::from_secs(10);

    fn frame(n: u32) -> Vec<u8> {
        n.to_le_bytes().to_vec()
    }

    fn drained(lane: &mut Lane, now: Instant) -> Vec<u32> {
        let mut out = Vec::new();
        lane.collect_due(now, &mut out);
        out.iter()
            .map(|p| u32::from_le_bytes(p.clone().try_into().unwrap()))
            .collect()
    }

    /// One lane leg: offer `0..n` at `t0`, collect at `due`, return the
    /// emitted payload order.
    fn run_lane(seed: u64, impair: Impair, n: u32, due: Instant) -> (Vec<u32>, LinkStats) {
        let mut lane = Lane::new(seed, MAX_QUEUED);
        let t0 = due - Duration::from_secs(10);
        for i in 0..n {
            lane.offer(&impair, t0 + Duration::from_millis(u64::from(i)), frame(i));
        }
        (drained(&mut lane, due), lane.stats)
    }

    #[test]
    fn a_transparent_lane_passes_frames_in_order() {
        let due = Instant::now();
        let (out, stats) = run_lane(0, Impair::default(), 6, due);
        assert_eq!(out, [0, 1, 2, 3, 4, 5]);
        assert_eq!(stats.frames_in, 6);
        assert_eq!(stats.bytes_in, 24, "four payload bytes per frame");
        assert_eq!(stats.delayed, 0, "a transparent lane holds nothing");
        assert_eq!(stats.dropped + stats.duplicated + stats.reordered, 0);
    }

    #[test]
    fn delay_holds_frames_until_their_release() {
        let mut lane = Lane::new(0, MAX_QUEUED);
        let impair = Impair {
            delay: Duration::from_millis(50),
            ..Impair::default()
        };
        let t0 = Instant::now();
        lane.offer(&impair, t0, frame(1));
        assert!(drained(&mut lane, t0 + Duration::from_millis(40)).is_empty());
        assert_eq!(drained(&mut lane, t0 + Duration::from_millis(60)), [1]);
        assert_eq!(lane.stats.delayed, 1, "the positive hold counted");
    }

    #[test]
    fn full_loss_drops_every_frame() {
        let due = Instant::now();
        let (out, stats) = run_lane(
            0,
            Impair {
                loss: 1.0,
                ..Impair::default()
            },
            5,
            due,
        );
        assert!(out.is_empty());
        assert_eq!(stats.dropped, 5);
    }

    #[test]
    fn full_duplicate_emits_adjacent_copies() {
        let due = Instant::now();
        let (out, stats) = run_lane(
            0,
            Impair {
                duplicate: 1.0,
                ..Impair::default()
            },
            3,
            due,
        );
        assert_eq!(out, [0, 0, 1, 1, 2, 2]);
        assert_eq!(stats.duplicated, 3);
    }

    #[test]
    fn full_reorder_swaps_each_pair() {
        let due = Instant::now();
        let (out, stats) = run_lane(
            0,
            Impair {
                reorder: 1.0,
                ..Impair::default()
            },
            4,
            due,
        );
        assert_eq!(out, [1, 0, 3, 2]);
        assert_eq!(stats.reordered, 2);
    }

    #[test]
    fn a_deferred_frame_emits_at_its_hold_deadline() {
        let mut lane = Lane::new(0, MAX_QUEUED);
        let impair = Impair {
            reorder: 1.0,
            ..Impair::default()
        };
        let t0 = Instant::now();
        lane.offer(&impair, t0, frame(7));
        // No successor ever arrives — the deadline, not the swap,
        // releases it.
        assert!(drained(&mut lane, t0 + HOLD_CAP - Duration::from_millis(1)).is_empty());
        assert_eq!(
            drained(&mut lane, t0 + HOLD_CAP + Duration::from_millis(1)),
            [7]
        );
    }

    #[test]
    fn the_pending_queue_is_bounded() {
        let mut lane = Lane::new(0, 4);
        let impair = Impair {
            delay: Duration::from_secs(3600),
            ..Impair::default()
        };
        let t0 = Instant::now();
        for i in 0..10 {
            lane.offer(&impair, t0, frame(i));
        }
        assert_eq!(lane.stats.frames_in, 10);
        assert_eq!(lane.stats.overflowed, 6);
        let mut out = Vec::new();
        lane.finish(&mut out);
        assert_eq!(out.len(), 4, "only the bounded queue drains");
    }

    #[test]
    fn decisions_replay_from_the_seed() {
        let impair = Impair {
            delay: Duration::from_millis(2),
            jitter: Duration::ZERO,
            loss: 0.3,
            duplicate: 0.3,
            reorder: 0.3,
        };
        let due = Instant::now();
        let (a, sa) = run_lane(7, impair, 40, due);
        let (b, sb) = run_lane(7, impair, 40, due);
        assert_eq!(a, b, "same seed, same traffic → same emission");
        assert_eq!(sa, sb);
        assert!(
            sa.dropped + sa.duplicated + sa.reordered > 0,
            "the recipe visibly impaired: {sa:?}"
        );
        let (c, _) = run_lane(8, impair, 40, due);
        assert_ne!(a, c, "a different seed draws a different pattern");
    }

    // ── over real loopback sockets ───────────────────────────────────

    /// A one-connection relay target: reports each received frame on
    /// `in_rx`, writes whatever is pushed to `out_tx`.
    fn sink() -> (SocketAddr, Receiver<Vec<u8>>, SyncSender<Vec<u8>>) {
        let listener = listen_loopback().unwrap();
        let addr = listener.local_addr().unwrap();
        let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>();
        let (out_tx, out_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_nodelay(true).unwrap();
            let mut writer = stream.try_clone().unwrap();
            thread::spawn(move || {
                while let Ok(payload) = out_rx.recv() {
                    if write_frame(&mut writer, &payload).is_err() {
                        break;
                    }
                }
            });
            while let Ok(payload) = read_frame(&mut stream) {
                if in_tx.send(payload).is_err() {
                    break;
                }
            }
        });
        (addr, in_rx, out_tx)
    }

    fn client(proxy: &ImpairProxy) -> TcpStream {
        let s = TcpStream::connect(proxy.addr()).unwrap();
        s.set_nodelay(true).unwrap();
        s
    }

    fn received(rx: &Receiver<Vec<u8>>, n: usize) -> Vec<u32> {
        (0..n)
            .map(|_| {
                u32::from_le_bytes(
                    rx.recv_timeout(WAIT)
                        .expect("a relayed frame never arrived")
                        .try_into()
                        .unwrap(),
                )
            })
            .collect()
    }

    /// Poll `stats` until `frames_in` covers `n` sent frames — the
    /// publish cadence is the writer loop's, never synchronous with the
    /// send.
    fn stats_at_least(proxy: &ImpairProxy, dir: LinkDir, frames_in: u64) -> LinkStats {
        let deadline = Instant::now() + WAIT;
        loop {
            let stats = proxy.stats(dir);
            if stats.frames_in >= frames_in {
                return stats;
            }
            assert!(
                Instant::now() < deadline,
                "lane never saw {frames_in} frames"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_clean_proxy_relays_frames_in_order() {
        let (target, in_rx, _out) = sink();
        let proxy = ImpairProxy::loopback(target).unwrap();
        let mut c = client(&proxy);
        for i in 0..4 {
            write_frame(&mut c, &frame(i)).unwrap();
        }
        assert_eq!(received(&in_rx, 4), [0, 1, 2, 3]);
        let stats = stats_at_least(&proxy, LinkDir::Up, 4);
        assert_eq!(stats.frames_out, 4);
        assert_eq!(stats.bytes_in, 16);
        assert_eq!(stats.bytes_out, 16, "each relayed frame's payload counted");
        assert_eq!(stats.delayed, 0, "a clean lane holds nothing");
        assert_eq!(stats.dropped + stats.duplicated + stats.reordered, 0);
    }

    #[test]
    fn upstream_loss_drops_whole_frames() {
        let (target, in_rx, _out) = sink();
        let proxy = ImpairProxy::loopback(target).unwrap();
        proxy.set(
            LinkDir::Up,
            Impair {
                loss: 1.0,
                ..Impair::default()
            },
        );
        let mut c = client(&proxy);
        for i in 0..3 {
            write_frame(&mut c, &frame(i)).unwrap();
        }
        assert!(
            in_rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "a lost frame must never arrive"
        );
        let stats = stats_at_least(&proxy, LinkDir::Up, 3);
        assert_eq!(stats.dropped, 3);
    }

    #[test]
    fn upstream_reorder_swaps_pairs_on_the_wire() {
        let (target, in_rx, _out) = sink();
        let proxy = ImpairProxy::loopback(target).unwrap();
        proxy.set(
            LinkDir::Up,
            Impair {
                reorder: 1.0,
                ..Impair::default()
            },
        );
        let mut c = client(&proxy);
        for i in 0..4 {
            write_frame(&mut c, &frame(i)).unwrap();
        }
        assert_eq!(received(&in_rx, 4), [1, 0, 3, 2]);
        let stats = stats_at_least(&proxy, LinkDir::Up, 4);
        assert_eq!(stats.reordered, 2);
        assert_eq!(stats.bytes_out, 16, "a swap emits both payloads once");
        assert_eq!(
            stats.delayed, 0,
            "reorder alone pays no delay/jitter hold — `reordered` counts it"
        );
    }

    #[test]
    fn upstream_duplicate_doubles_frames_on_the_wire() {
        let (target, in_rx, _out) = sink();
        let proxy = ImpairProxy::loopback(target).unwrap();
        proxy.set(
            LinkDir::Up,
            Impair {
                duplicate: 1.0,
                ..Impair::default()
            },
        );
        let mut c = client(&proxy);
        for i in 0..3 {
            write_frame(&mut c, &frame(i)).unwrap();
        }
        assert_eq!(received(&in_rx, 6), [0, 0, 1, 1, 2, 2]);
        let stats = stats_at_least(&proxy, LinkDir::Up, 3);
        assert_eq!(stats.duplicated, 3);
        assert_eq!(
            stats.bytes_out, 24,
            "each duplicated copy re-pays the payload bytes"
        );
    }

    #[test]
    fn a_delay_recipe_holds_frames_for_the_latency() {
        let (target, in_rx, _out) = sink();
        let proxy = ImpairProxy::loopback(target).unwrap();
        proxy.set(
            LinkDir::Up,
            Impair {
                delay: Duration::from_millis(60),
                ..Impair::default()
            },
        );
        let mut c = client(&proxy);
        let sent = Instant::now();
        write_frame(&mut c, &frame(1)).unwrap();
        in_rx.recv_timeout(WAIT).unwrap();
        assert!(
            sent.elapsed() >= Duration::from_millis(45),
            "the frame arrived faster than its release: {:?}",
            sent.elapsed()
        );
        let stats = stats_at_least(&proxy, LinkDir::Up, 1);
        assert_eq!(stats.delayed, 1, "the 60 ms hold counted");
        assert_eq!(stats.bytes_in, 4);
        assert_eq!(stats.bytes_out, 4);
    }

    #[test]
    fn impairment_is_per_direction_and_retunable() {
        let (target, in_rx, out_tx) = sink();
        let proxy = ImpairProxy::loopback(target).unwrap();
        proxy.set(
            LinkDir::Up,
            Impair {
                loss: 1.0,
                ..Impair::default()
            },
        );
        let mut c = client(&proxy);
        // Up is dead, Down is clean: our frame never lands, the
        // target's frame comes straight back.
        write_frame(&mut c, &frame(9)).unwrap();
        out_tx.send(frame(42)).unwrap();
        assert_eq!(
            u32::from_le_bytes(read_frame(&mut c).unwrap().try_into().unwrap()),
            42
        );
        assert!(in_rx.recv_timeout(Duration::from_millis(300)).is_err());
        // Live retune: clear the recipe and the next frame passes.
        proxy.set(LinkDir::Up, Impair::default());
        write_frame(&mut c, &frame(10)).unwrap();
        assert_eq!(received(&in_rx, 1), [10]);
        let stats = stats_at_least(&proxy, LinkDir::Up, 2);
        assert_eq!(stats.dropped, 1);
    }

    /// A peer that vanished must not back-pressure the target: the
    /// Down lane keeps reading and discarding, so the target's writes
    /// to the dead link complete rather than filling its socket buffer.
    #[test]
    fn a_vanished_peer_does_not_stall_the_targets_writes() {
        let (target, _in_rx, out_tx) = sink();
        let proxy = ImpairProxy::loopback(target).unwrap();
        let mut c = client(&proxy);
        // One round trip so the lane pair is wired before the close.
        out_tx.send(frame(1)).unwrap();
        read_frame(&mut c).unwrap();
        drop(c);
        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            // Far past any socket buffer the dead link could absorb.
            for _ in 0..4096 {
                if out_tx.send(vec![0u8; 8192]).is_err() {
                    return;
                }
            }
            let _ = done_tx.send(());
        });
        done_rx
            .recv_timeout(Duration::from_secs(20))
            .expect("the target's writes stalled behind a vanished peer");
    }

    /// A seeded recipe replays identically over identical traffic —
    /// the AC03 matrix's repeatability contract. Both runs see the same
    /// delivered sequence and the same measured counters.
    #[test]
    fn a_seeded_recipe_replays_identically() {
        let recipe = Impair {
            delay: Duration::from_millis(2),
            jitter: Duration::ZERO,
            loss: 0.25,
            duplicate: 0.25,
            reorder: 0.25,
        };
        let run = || {
            let (target, in_rx, _out) = sink();
            let proxy = ImpairProxy::loopback_seeded(target, 11).unwrap();
            proxy.set(LinkDir::Up, recipe);
            let mut c = client(&proxy);
            for i in 0..24 {
                write_frame(&mut c, &frame(i)).unwrap();
            }
            // Delivered frames then the measured counters.
            let mut got = Vec::new();
            while let Ok(p) = in_rx.recv_timeout(Duration::from_millis(300)) {
                got.push(u32::from_le_bytes(p.try_into().unwrap()));
            }
            let stats = stats_at_least(&proxy, LinkDir::Up, 24);
            (got, stats)
        };
        let (first, first_stats) = run();
        let (second, second_stats) = run();
        assert_eq!(first, second, "same seed + traffic → same delivery");
        assert_eq!(first_stats, second_stats);
        assert!(
            first_stats.dropped + first_stats.duplicated + first_stats.reordered > 0,
            "the recipe visibly impaired: {first_stats:?}"
        );
        assert_ne!(
            first,
            (0..24).collect::<Vec<u32>>(),
            "impairment changed the stream"
        );
    }

    /// The lobby's data plane over an impaired link: inputs reordered
    /// on the wire never regress the mailbox — `seq` is the freshness
    /// order — and a reordered `ResetRequest` pair still reports the
    /// newest generation.
    #[test]
    fn a_lobbys_mailbox_stays_fresh_under_reorder() {
        let host = Host::listen_loopback(&HostConfig::new(0xbeef)).unwrap();
        let proxy = ImpairProxy::loopback_seeded(host.addr(), 3).unwrap();
        // The handshake crosses while the recipe is still transparent.
        let mut alice = Client::join(
            proxy.addr(),
            &hello("build".to_string(), "alice".to_string(), 0xbeef),
        )
        .unwrap();
        host.recv_timeout(WAIT).unwrap(); // Joined
        proxy.set(
            LinkDir::Up,
            Impair {
                reorder: 1.0,
                ..Impair::default()
            },
        );
        let ctl = alice.ctl().unwrap();
        let input = |seq| DriveInput {
            generation: 1,
            seq,
            throttle: 200,
            brake: 0,
            steer: 0,
            handbrake: 0,
        };
        // Wire order is now [2,1,4,3] — the mailbox must end on the
        // sender's newest, not the newest *arrival*.
        for seq in 1..=4 {
            ctl.send_input(input(seq)).unwrap();
        }
        let deadline = Instant::now() + WAIT;
        loop {
            if host
                .remote_inputs()
                .latest(1)
                .is_some_and(|s| s.input.seq == 4)
            {
                break;
            }
            assert!(Instant::now() < deadline, "the freshest seq never held");
            thread::sleep(Duration::from_millis(2));
        }
        // The same swap on reset asks: generation 2 arrives before
        // generation 1 — the stale ask can never mask the fresh one.
        ctl.request_reset(1).unwrap();
        ctl.request_reset(2).unwrap();
        loop {
            let drained = host.remote_inputs().drain_resets();
            if let Some(&(_, generation)) = drained.first() {
                assert_eq!(generation, 2, "the newest generation won");
                break;
            }
            assert!(Instant::now() < deadline, "no request landed");
            thread::sleep(Duration::from_millis(2));
        }
        let stats = stats_at_least(&proxy, LinkDir::Up, 6);
        assert!(stats.reordered >= 2, "the reorder recipe ran: {stats:?}");
        // Lobby order still holds — a SetReady afterwards applies fine.
        alice.set_ready(true).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(crate::HostEvent::ReadyChanged { id: 1, ready: true }) => {}
            other => panic!("expected ReadyChanged, got {other:?}"),
        }
    }

    /// A snapshot's stale-tick drop is the receiver-side half of
    /// reorder tolerance — a duplicated `Snap` arrives twice and the
    /// second copy must not apply. Exercised at the `RemoteSnaps`
    /// level by the app tests; here the wire-level fact: the proxy
    /// really does emit two frames.
    #[test]
    fn duplicated_frames_really_do_double_on_the_wire() {
        let (target, _in_rx, out_tx) = sink();
        let proxy = ImpairProxy::loopback(target).unwrap();
        proxy.set(
            LinkDir::Down,
            Impair {
                duplicate: 1.0,
                ..Impair::default()
            },
        );
        let mut c = client(&proxy);
        out_tx.send(frame(5)).unwrap();
        assert_eq!(read_frame(&mut c).unwrap(), frame(5));
        assert_eq!(read_frame(&mut c).unwrap(), frame(5));
        let stats = stats_at_least(&proxy, LinkDir::Down, 1);
        assert_eq!(stats.duplicated, 1);
    }
}
