//! A deterministic impairment relay at the **datagram boundary**
//! (NETPLAY-06B).
//!
//! [`crate::ImpairProxy`] impairs whole frames of a TCP stream — the
//! legacy model of a datagram transport. The UDP lane (06A) is the real
//! one, so this relay sits between a lane client and the host's lane
//! socket and impairs the actual datagrams:
//!
//! - `loss`, `duplicate`, `reorder`, `delay`/`jitter` — the same seeded
//!   recipe as the TCP proxy ([`Impair`]), applied to each datagram.
//! - **burst loss** — [`UdpImpairProxy::drop_next`] drops the next `n`
//!   datagrams of a direction outright, the recipe behind the
//!   consecutive-loss cells of `docs/netplay-timeline.md` (3-6 dropped
//!   input packets, 90 dropped snapshots).
//! - **effective MTU** — [`UdpImpair::mtu`] drops every datagram larger
//!   than the limit, the way an undersized path does: silently.
//! - **black hole** — [`UdpImpair::blackhole`] drops everything.
//! - **bandwidth cap** — [`UdpImpair::rate`] serialises a direction
//!   through a drop-tail link (NETPLAY-10E): datagrams queue behind the
//!   bytes ahead of them, wait their turn, and are dropped when the
//!   modelled buffer is full.
//!
//! Each direction ([`LinkDir::Up`]: client to host, [`LinkDir::Down`]:
//! host to client) has its own recipe, its own seeded draw stream and its
//! own [`UdpStats`]; [`UdpImpairProxy::tagged`] counts datagrams per lane
//! tag (probe / data) so a test can separate the data plane from
//! establishment traffic.
//!
//! The relay can run on a **virtual clock**
//! ([`UdpImpairProxy::loopback_manual`]): delay, jitter and the reorder
//! hold are measured against time that only moves in
//! [`UdpImpairProxy::advance`], so what is delivered after each advance is
//! a function of the seed and the test's schedule rather than of machine
//! load. As with the TCP proxy the test pairs each advance with its own
//! barrier on the datagrams it expects (see [`UdpImpairProxy::stats`]).
//!
//! # Wiring
//!
//! The host authenticates a datagram's source address, so the relay owns
//! two sockets: a *front* the client dials and a *back* the host sees as
//! the client. [`UdpImpairProxy::route`] yields the matching
//! [`LaneRoute`] for [`Client::join_lane_routed`](crate::Client::join_lane_routed):
//! the TCP control connection still goes straight to the host, only the
//! datagram path is relayed. The relay serves one client; a second client
//! needs its own proxy.
//!
//! This is test scaffolding: it binds loopback and decodes nothing.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::impair::{Impair, Lane, LinkDir, LinkStats, MAX_QUEUED};
use crate::lobby::LaneRoute;
use crate::rt::MAX_DATAGRAM;

/// One direction's datagram-level recipe. `UdpImpair::default()` is a
/// transparent relay.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UdpImpair {
    /// Delay, jitter, loss, duplication and reorder per datagram.
    pub link: Impair,
    /// Effective MTU: a datagram longer than this many bytes (the whole
    /// lane datagram, header included) is dropped. `None`: unlimited.
    pub mtu: Option<usize>,
    /// Drop every datagram.
    pub blackhole: bool,
    /// A bandwidth cap: the direction behaves as a drop-tail link.
    pub rate: Option<LinkRate>,
}

/// A drop-tail bottleneck link (NETPLAY-10E). Each datagram is charged its
/// length plus 28 bytes of IP and UDP header, the same accounting the
/// host's congestion controller uses for wire bytes, and leaves the link
/// once the bytes queued ahead of it have been sent at `bytes_per_sec`.
/// A datagram that would push the bytes held on the link (the one being
/// sent included) past `buffer_bytes` is dropped; an idle link always
/// accepts one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinkRate {
    /// Link speed, bytes per second.
    pub bytes_per_sec: u32,
    /// Bytes the link holds at most, the datagram in service included.
    pub buffer_bytes: u32,
}

impl LinkRate {
    /// A link of `kbit` kilobits per second whose buffer holds `buffer`.
    pub fn kbit(kbit: u32, buffer: Duration) -> Self {
        let bytes_per_sec = kbit.saturating_mul(125).max(1);
        Self {
            bytes_per_sec,
            buffer_bytes: (u64::from(bytes_per_sec) * buffer.as_millis() as u64 / 1000) as u32,
        }
    }
}

/// IP and UDP header bytes a link is charged per datagram.
const WIRE_OVERHEAD: u64 = 28;

/// What a direction did, extending [`LinkStats`] with the datagram-only
/// drop causes. `link.dropped` counts **every** drop; the fields below
/// say why.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UdpStats {
    /// The shared frame/datagram counters (`frames_*` are datagrams).
    pub link: LinkStats,
    /// Datagrams dropped for exceeding [`UdpImpair::mtu`].
    pub mtu_dropped: u64,
    /// Datagrams dropped by a [`UdpImpairProxy::drop_next`] burst.
    pub burst_dropped: u64,
    /// Datagrams dropped by [`UdpImpair::blackhole`].
    pub blackholed: u64,
    /// Datagrams dropped because the [`UdpImpair::rate`] buffer was full.
    pub rate_dropped: u64,
    /// The largest datagram the proxy read in this direction, whether it
    /// forwarded or dropped it: the measured wire size, header included.
    pub largest: usize,
}

/// Datagrams are tallied by the lane-tag byte of the header (offset 5);
/// anything shorter lands on tag 0.
const TAG_OFFSET: usize = 5;

#[derive(Clone)]
struct TagCounts([(u64, u64); 256]);

impl Default for TagCounts {
    fn default() -> Self {
        Self([(0, 0); 256])
    }
}

struct Clock {
    origin: Instant,
    nanos: AtomicU64,
    /// Bumped by every advance; the relay thread echoes the epoch it read
    /// before it read the time into `seen`.
    epoch: AtomicU64,
    seen: AtomicU64,
}

struct Shared {
    params: Mutex<[UdpImpair; 2]>,
    /// Datagrams still to be burst-dropped, per direction.
    burst: [AtomicU64; 2],
    stats: Mutex<[UdpStats; 2]>,
    tags: Mutex<[TagCounts; 2]>,
    stop: AtomicBool,
    clock: Option<Clock>,
}

impl Shared {
    fn now(&self) -> Instant {
        self.clock.as_ref().map_or_else(Instant::now, |c| {
            c.origin + Duration::from_nanos(c.nanos.load(Ordering::Acquire))
        })
    }

    /// Take one datagram off the burst budget; `true` if it was owed.
    fn take_burst(&self, dir: LinkDir) -> bool {
        // A compare-exchange loop rather than `fetch_update`, which newer
        // toolchains deprecate in favour of `try_update`.
        let cell = &self.burst[dir as usize];
        let mut owed = cell.load(Ordering::Acquire);
        while owed > 0 {
            match cell.compare_exchange_weak(owed, owed - 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return true,
                Err(now) => owed = now,
            }
        }
        false
    }
}

/// A seeded UDP relay with per-direction impairment — see the module
/// docs. Dropping it stops the relay thread.
pub struct UdpImpairProxy {
    front: SocketAddr,
    back_port: u16,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl UdpImpairProxy {
    /// Relay to the host's lane socket at `target` on the wall clock, with
    /// a seed for every draw. Transparent until [`Self::set`].
    pub fn loopback_seeded(target: SocketAddr, seed: u64) -> io::Result<Self> {
        Self::start(target, seed, false)
    }

    /// [`Self::loopback_seeded`] on a virtual clock that only moves in
    /// [`Self::advance`].
    pub fn loopback_manual(target: SocketAddr, seed: u64) -> io::Result<Self> {
        Self::start(target, seed, true)
    }

    fn start(target: SocketAddr, seed: u64, manual: bool) -> io::Result<Self> {
        let bind = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let front = UdpSocket::bind(bind)?;
        let back = UdpSocket::bind(bind)?;
        front.set_nonblocking(true)?;
        back.set_nonblocking(true)?;
        let (front_addr, back_port) = (front.local_addr()?, back.local_addr()?.port());
        let shared = Arc::new(Shared {
            params: Mutex::new([UdpImpair::default(); 2]),
            burst: [AtomicU64::new(0), AtomicU64::new(0)],
            stats: Mutex::new([UdpStats::default(); 2]),
            tags: Mutex::new([TagCounts::default(), TagCounts::default()]),
            stop: AtomicBool::new(false),
            clock: manual.then(|| Clock {
                origin: Instant::now(),
                nanos: AtomicU64::new(0),
                epoch: AtomicU64::new(0),
                seen: AtomicU64::new(0),
            }),
        });
        let thread = {
            let shared = shared.clone();
            thread::spawn(move || relay(&front, &back, target, seed, &shared))
        };
        Ok(Self {
            front: front_addr,
            back_port,
            shared,
            thread: Some(thread),
        })
    }

    /// The route a lane client takes through this relay: dial the front,
    /// have the host expect the back.
    pub fn route(&self) -> LaneRoute {
        LaneRoute {
            dial: self.front,
            advertise_port: self.back_port,
        }
    }

    /// Replace one direction's recipe, live.
    pub fn set(&self, dir: LinkDir, impair: UdpImpair) {
        self.shared.params.lock().unwrap_or_else(|e| e.into_inner())[dir as usize] = impair;
    }

    /// The recipe `dir` currently runs.
    pub fn impair(&self, dir: LinkDir) -> UdpImpair {
        self.shared.params.lock().unwrap_or_else(|e| e.into_inner())[dir as usize]
    }

    /// Drop, without a loss draw, the next `n` datagrams `dir` receives —
    /// a burst of consecutive loss. Replaces any burst still pending.
    pub fn drop_next(&self, dir: LinkDir, n: u64) {
        self.shared.burst[dir as usize].store(n, Ordering::Release);
    }

    /// Move the virtual clock forward by `by` and return once the relay
    /// has released whatever fell due; `false` if it never caught up.
    /// Datagrams offered before the call were stamped at the old time.
    /// A wall-clock relay has no clock to move: returns `true`.
    pub fn advance(&self, by: Duration) -> bool {
        let Some(clock) = &self.shared.clock else {
            return true;
        };
        clock
            .nanos
            .fetch_add(by.as_nanos() as u64, Ordering::AcqRel);
        let epoch = clock.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        let deadline = Instant::now() + Duration::from_secs(10);
        while clock.seen.load(Ordering::Acquire) < epoch {
            if Instant::now() >= deadline {
                return false;
            }
            thread::yield_now();
        }
        true
    }

    /// What `dir` has done so far.
    pub fn stats(&self, dir: LinkDir) -> UdpStats {
        self.shared.stats.lock().unwrap_or_else(|e| e.into_inner())[dir as usize]
    }

    /// `(read, written)` datagram counts for lane tag `tag` (the header's
    /// tag byte: 1 probe, 2 data) in `dir`.
    pub fn tagged(&self, dir: LinkDir, tag: u8) -> (u64, u64) {
        self.shared.tags.lock().unwrap_or_else(|e| e.into_inner())[dir as usize].0[tag as usize]
    }
}

impl Drop for UdpImpairProxy {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The relay thread: receive from both sockets, classify, emit what fell
/// due. One thread keeps both directions' order of events a function of
/// the schedule alone.
fn relay(front: &UdpSocket, back: &UdpSocket, target: SocketAddr, seed: u64, shared: &Shared) {
    let mut lanes = [
        Lane::new(seed ^ 0x5555_aaaa_5555_aaaa, MAX_QUEUED),
        Lane::new(seed.rotate_left(32) ^ 0xaaaa_5555_aaaa_5555, MAX_QUEUED),
    ];
    let mut extra = [UdpStats::default(); 2];
    // When each direction's bottleneck link finishes what it holds.
    let mut busy: [Option<Instant>; 2] = [None; 2];
    let mut tags = [TagCounts::default(), TagCounts::default()];
    // The client's socket, learned from the first datagram on the front.
    let mut peer: Option<SocketAddr> = None;
    let mut buf = [0u8; MAX_DATAGRAM + 64];
    let mut due: Vec<Vec<u8>> = Vec::new();
    while !shared.stop.load(Ordering::Relaxed) {
        // Epoch before time: an echo of `e` proves this pass saw at least
        // the time `advance` set for it.
        let epoch = shared
            .clock
            .as_ref()
            .map(|c| c.epoch.load(Ordering::Acquire));
        let now = shared.now();
        for dir in [LinkDir::Up, LinkDir::Down] {
            let i = dir as usize;
            lanes[i].collect_due(now, &mut due);
            for datagram in due.drain(..) {
                let sent = match dir {
                    LinkDir::Up => back.send_to(&datagram, target).is_ok(),
                    LinkDir::Down => peer.is_some_and(|p| front.send_to(&datagram, p).is_ok()),
                };
                if sent {
                    lanes[i].stats.frames_out += 1;
                    lanes[i].stats.bytes_out += datagram.len() as u64;
                    tags[i].0[tag_of(&datagram)].1 += 1;
                }
            }
        }
        let mut idle = true;
        loop {
            let mut progressed = false;
            if let Ok((n, from)) = front.recv_from(&mut buf) {
                progressed = true;
                // Only the first client sticks: a stranger on the front
                // must not redirect the host's replies.
                if *peer.get_or_insert(from) == from {
                    classify(
                        shared,
                        &mut lanes,
                        &mut extra,
                        &mut busy,
                        &mut tags,
                        LinkDir::Up,
                        &buf[..n],
                    );
                }
            }
            if let Ok((n, from)) = back.recv_from(&mut buf)
                && from == target
            {
                progressed = true;
                classify(
                    shared,
                    &mut lanes,
                    &mut extra,
                    &mut busy,
                    &mut tags,
                    LinkDir::Down,
                    &buf[..n],
                );
            }
            idle &= !progressed;
            if !progressed {
                break;
            }
        }
        publish(shared, &lanes, &extra, &tags);
        if let (Some(clock), Some(epoch)) = (&shared.clock, epoch) {
            clock.seen.store(epoch, Ordering::Release);
        }
        if idle {
            let nap = if epoch.is_some() {
                Duration::from_micros(100)
            } else {
                lanes
                    .iter()
                    .filter_map(Lane::next_due)
                    .min()
                    .map_or(Duration::from_millis(1), |d| {
                        d.saturating_duration_since(now)
                    })
                    .min(Duration::from_millis(1))
            };
            thread::sleep(nap.max(Duration::from_micros(50)));
        }
    }
}

fn tag_of(datagram: &[u8]) -> usize {
    datagram.get(TAG_OFFSET).copied().map_or(0, usize::from)
}

/// One received datagram: drop it for a datagram-level cause, else hand it
/// to the direction's lane recipe.
fn classify(
    shared: &Shared,
    lanes: &mut [Lane; 2],
    extra: &mut [UdpStats; 2],
    busy: &mut [Option<Instant>; 2],
    tags: &mut [TagCounts; 2],
    dir: LinkDir,
    datagram: &[u8],
) {
    let i = dir as usize;
    let recipe = shared.params.lock().unwrap_or_else(|e| e.into_inner())[i];
    tags[i].0[tag_of(datagram)].0 += 1;
    extra[i].largest = extra[i].largest.max(datagram.len());
    if recipe.blackhole {
        extra[i].blackholed += 1;
        lanes[i].drop_frame(datagram);
    } else if recipe.mtu.is_some_and(|mtu| datagram.len() > mtu) {
        extra[i].mtu_dropped += 1;
        lanes[i].drop_frame(datagram);
    } else if shared.take_burst(dir) {
        extra[i].burst_dropped += 1;
        lanes[i].drop_frame(datagram);
    } else {
        let now = shared.now();
        let mut link = recipe.link;
        if let Some(rate) = recipe.rate {
            let wire = datagram.len() as u64 + WIRE_OVERHEAD;
            let free = busy[i].map_or(now, |b| b.max(now));
            let queued =
                (free - now).as_nanos() as u64 * u64::from(rate.bytes_per_sec) / 1_000_000_000;
            if queued > 0 && queued + wire > u64::from(rate.buffer_bytes) {
                extra[i].rate_dropped += 1;
                lanes[i].drop_frame(datagram);
                return;
            }
            let done =
                free + Duration::from_nanos(wire * 1_000_000_000 / u64::from(rate.bytes_per_sec));
            busy[i] = Some(done);
            link.delay += done - now;
        }
        lanes[i].offer(&link, now, datagram.to_vec());
    }
}

fn publish(shared: &Shared, lanes: &[Lane; 2], extra: &[UdpStats; 2], tags: &[TagCounts; 2]) {
    let mut stats = shared.stats.lock().unwrap_or_else(|e| e.into_inner());
    for i in 0..2 {
        stats[i] = UdpStats {
            link: lanes[i].stats,
            ..extra[i]
        };
    }
    drop(stats);
    *shared.tags.lock().unwrap_or_else(|e| e.into_inner()) = tags.clone();
}
