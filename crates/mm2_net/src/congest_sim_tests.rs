//! Bandwidth-limited delivery on a virtual clock (NETPLAY-10B).
//!
//! The production send side — `SeatOut`'s two bounded queues, its rate
//! budget, the per-peer [`PeerController`], the shared [`Uplink`] and its
//! max-min division — is driven at 1 ms steps against modelled links:
//! each peer's downlink is a drop-tail FIFO of fixed capacity, the host
//! has an aggregate cap, and a client acknowledges each snapshot over a
//! 256 kbit/s uplink. Time is injected (`next_to_send_at`), so the runs
//! are deterministic and take no wall time.
//!
//! What this is not: real sockets, real clocks or a real client. It is
//! the *synthetic integration* level of `specs/QUALITY-GATES.md`; the
//! loopback qualification run is NETPLAY-10E.
//!
//! Run `cargo test -p mm2_net congest_sim -- --nocapture` for the tables.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::congest::{Quantiles, Uplink, wire_bytes};
use crate::pace::Priority;
use crate::rt::{LaneTag, MAX_PAYLOAD, SeatOut, Wake, divide_uplink};

const STEP: Duration = Duration::from_millis(1);
/// One snapshot period at the default 30 Hz.
const PERIOD: Duration = Duration::from_micros(33_334);
/// Snapshot payload at full relevance: a car's state for eight cars.
const SNAP_BYTES: usize = 900;
/// A world catch-up fragment.
const BULK_BYTES: usize = MAX_PAYLOAD;
/// Fragments of catch-up offered per snapshot.
const BULK_FRAGMENTS: usize = 4;
/// Drop-tail buffer of a peer's downlink, as queueing delay.
const LINK_BUFFER: Duration = Duration::from_millis(150);
/// One-way propagation.
const PROPAGATION: Duration = Duration::from_millis(20);
/// A client's acknowledgement is small and its uplink is 256 kbit/s.
const CLIENT_UPLINK: f64 = 256_000.0 / 8.0;
const ACK_WIRE: usize = 64;

const MBIT: u32 = 125_000;

struct Peer {
    out: SeatOut,
    /// Downlink capacity, wire bytes per second.
    link: f64,
    busy_until: Duration,
    acks: VecDeque<(Duration, u64)>,
    last_ack: Option<Duration>,
    retx: VecDeque<Duration>,
    retransmits: u64,
    link_drops: u64,
    delivered: u64,
    /// Queue plus link age of delivered movement, microseconds.
    state_age: Quantiles,
    /// Virtual time of the newest delivered movement and its age.
    fresh: Vec<(Duration, Duration)>,
    snap_index: u64,
    wire: u64,
}

struct Sim {
    base: Instant,
    now: Duration,
    next_snap: Duration,
    next_reliable: Duration,
    tick: u64,
    uplink: Arc<Mutex<Uplink>>,
    peers: Vec<Peer>,
    host_wire: u64,
    rotate: usize,
    /// Movement datagrams per snapshot and their payload size, when a
    /// test models a multi-fragment movement group.
    movement_shape: Option<(usize, usize)>,
}

impl Sim {
    fn new(peers: usize, host_cap: u32, link: f64) -> Self {
        let base = Instant::now();
        let uplink = Arc::new(Mutex::new(Uplink::new(host_cap, base)));
        let wake = Arc::new(Wake::default());
        let peers = (0..peers)
            .map(|_| Peer {
                out: SeatOut::new(wake.clone(), Some(uplink.clone()), base),
                link,
                busy_until: Duration::ZERO,
                acks: VecDeque::new(),
                last_ack: None,
                retx: VecDeque::new(),
                retransmits: 0,
                link_drops: 0,
                delivered: 0,
                state_age: Quantiles::new(1 << 16),
                fresh: Vec::new(),
                snap_index: 0,
                wire: 0,
            })
            .collect();
        Self {
            base,
            now: Duration::ZERO,
            next_snap: Duration::ZERO,
            next_reliable: Duration::ZERO,
            tick: 0,
            uplink,
            peers,
            host_wire: 0,
            rotate: 0,
            movement_shape: None,
        }
    }

    fn at(&self, d: Duration) -> Instant {
        self.base + d
    }

    fn payload(tick: u64, enq: Duration, len: usize) -> Vec<u8> {
        let mut p = vec![0u8; len.max(16)];
        p[..8].copy_from_slice(&tick.to_le_bytes());
        p[8..16].copy_from_slice(&(enq.as_micros() as u64).to_le_bytes());
        p
    }

    /// Publish one snapshot round: each peer gets what its adaptation
    /// level allows, never an old snapshot queued behind a new one.
    fn publish(&mut self) {
        self.tick += 1;
        let (now, tick) = (self.at(self.now), self.tick);
        for p in &mut self.peers {
            p.snap_index += 1;
            let adapt = p.out.adapt();
            if !p
                .snap_index
                .is_multiple_of(u64::from(adapt.snapshot_divisor))
            {
                continue;
            }
            let len = SNAP_BYTES * adapt.relevance_percent as usize / 100;
            p.out.note_tick(tick, now);
            if let Some((frags, bytes)) = self.movement_shape {
                for _ in 0..frags {
                    p.out.push_tagged_at(
                        now,
                        LaneTag::Data,
                        Priority::Movement,
                        1,
                        Self::payload(tick, self.now, bytes),
                    );
                }
            } else {
                p.out.push_tagged_at(
                    now,
                    LaneTag::Data,
                    Priority::Movement,
                    1,
                    Self::payload(tick, self.now, len),
                );
            }
            if adapt.bulk {
                for _ in 0..BULK_FRAGMENTS {
                    p.out.push_tagged_at(
                        now,
                        LaneTag::Fragment,
                        Priority::Bulk,
                        1,
                        Self::payload(tick, self.now, BULK_BYTES),
                    );
                }
            }
        }
    }

    fn step(&mut self) {
        self.now += STEP;
        let now = self.at(self.now);
        if self.now >= self.next_snap {
            self.next_snap += PERIOD;
            self.publish();
        }
        if self.now >= self.next_reliable {
            self.next_reliable += Duration::from_millis(100);
            for p in &mut self.peers {
                p.out.push_tagged_at(
                    now,
                    LaneTag::Reliable,
                    Priority::Movement,
                    1,
                    Self::payload(0, self.now, 120),
                );
            }
        }
        for p in &mut self.peers {
            while p.acks.front().is_some_and(|(at, _)| *at <= self.now) {
                let (at, tick) = p.acks.pop_front().unwrap();
                p.out.on_ack(tick, self.base + at);
                p.last_ack = Some(at);
            }
            while p.retx.front().is_some_and(|at| *at <= self.now) {
                p.retx.pop_front();
                p.retransmits += 1;
                p.out.push_tagged_at(
                    now,
                    LaneTag::Reliable,
                    Priority::Movement,
                    1,
                    Self::payload(0, self.now, 120),
                );
            }
            let ack_age = p.last_ack.map(|a| self.now.saturating_sub(a));
            p.out.update_control(now, p.retransmits, ack_age);
        }
        {
            let outs: Vec<(u16, &SeatOut)> = self
                .peers
                .iter()
                .enumerate()
                .map(|(i, p)| (i as u16, &p.out))
                .collect();
            divide_uplink(&self.uplink, &outs, now);
        }
        let n = self.peers.len();
        self.rotate = (self.rotate + 1) % n.max(1);
        for k in 0..n {
            let i = (k + self.rotate) % n;
            while let Some(item) = self.peers[i].out.next_to_send_at(now) {
                self.transmit(i, item);
            }
        }
    }

    fn transmit(&mut self, i: usize, item: crate::rt::OutItem) {
        let wire = wire_bytes(item.payload.len());
        self.host_wire += wire as u64;
        let p = &mut self.peers[i];
        p.wire += wire as u64;
        let serial = Duration::from_secs_f64(wire as f64 / p.link);
        let start = p.busy_until.max(self.now);
        let finish = start + serial;
        if finish - self.now > LINK_BUFFER {
            p.link_drops += 1;
            if item.tag == LaneTag::Reliable {
                p.retx.push_back(self.now + Duration::from_millis(100));
            }
            return;
        }
        p.busy_until = finish;
        let arrive = finish + PROPAGATION;
        if item.tag != LaneTag::Data {
            return;
        }
        let tick = u64::from_le_bytes(item.payload[..8].try_into().unwrap());
        let enq =
            Duration::from_micros(u64::from_le_bytes(item.payload[8..16].try_into().unwrap()));
        let age = arrive - enq;
        p.delivered += 1;
        p.state_age.add(age.as_micros() as u64);
        p.fresh.push((arrive, age));
        let ack_back = Duration::from_secs_f64(ACK_WIRE as f64 / CLIENT_UPLINK) + PROPAGATION;
        p.acks.push_back((arrive + ack_back, tick));
    }

    fn run(&mut self, span: Duration) {
        let until = self.now + span;
        while self.now < until {
            self.step();
        }
    }

    fn wire_of(&self, ids: &[usize]) -> Vec<f64> {
        ids.iter().map(|&i| self.peers[i].wire as f64).collect()
    }
}

/// Jain's fairness index over `xs`: 1.0 is perfectly equal.
fn jain(xs: &[f64]) -> f64 {
    let sum: f64 = xs.iter().sum();
    let sq: f64 = xs.iter().map(|x| x * x).sum();
    if sq == 0.0 {
        1.0
    } else {
        sum * sum / (xs.len() as f64 * sq)
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn report(label: &str, sim: &Sim, since_wire: &[u64], span: Duration) {
    println!("--- {label} ({:.0} s)", span.as_secs_f64());
    println!(
        "{:>4} {:>8} {:>8} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>5} {:>5} {:>7}",
        "peer",
        "kB/s",
        "rate kB/s",
        "target",
        "q50ms",
        "q95ms",
        "q99ms",
        "age50",
        "age95",
        "age99",
        "retx%",
        "drops",
        "divisor"
    );
    for (i, p) in sim.peers.iter().enumerate() {
        let s = p.out.pace_stats_at(sim.at(sim.now));
        let sent = (p.wire - since_wire[i]) as f64 / span.as_secs_f64() / 1000.0;
        let reliable = (span.as_secs_f64() * 10.0).max(1.0);
        println!(
            "{:>4} {:>8.1} {:>8.1} {:>6.1} {:>6.1} {:>6.1} {:>6.1} {:>6.1} {:>6.1} {:>6.1} {:>5.1} {:>5} {:>7}",
            i,
            sent,
            f64::from(s.rate) / 1000.0,
            f64::from(s.share) / 1000.0,
            ms(s.wait_p50),
            ms(s.wait_p95),
            ms(s.wait_p99),
            p.state_age.percentile(50) as f64 / 1000.0,
            p.state_age.percentile(95) as f64 / 1000.0,
            p.state_age.percentile(99) as f64 / 1000.0,
            p.retransmits as f64 / reliable * 100.0,
            p.link_drops,
            s.adapt.snapshot_divisor,
        );
    }
    let total: u64 = sim
        .peers
        .iter()
        .zip(since_wire)
        .map(|(p, w)| p.wire - w)
        .sum();
    println!(
        "host wire: {:.1} kB/s of cap {:.1} kB/s; fairness (Jain) over all: {:.3}",
        total as f64 / span.as_secs_f64() / 1000.0,
        f64::from(sim.uplink.lock().unwrap().cap()) / 1000.0,
        jain(&sim.wire_of(&(0..sim.peers.len()).collect::<Vec<_>>())),
    );
}

/// Run `warm` to settle, then reset the queue-wait samples by reading the
/// stats afterwards from fresh peers only if needed; here the settled
/// stretch is simply the run after `warm`.
fn measured(sim: &mut Sim, warm: Duration, span: Duration, label: &str) -> Vec<u64> {
    sim.run(warm);
    for p in &mut sim.peers {
        p.state_age = Quantiles::new(1 << 16);
        p.out.pace.lock().unwrap().movement_waits = Quantiles::new(4096);
    }
    let before: Vec<u64> = sim.peers.iter().map(|p| p.wire).collect();
    let host_before = sim.host_wire;
    sim.run(span);
    report(label, sim, &before, span);
    let sent = sim.host_wire - host_before;
    let cap = u64::from(sim.uplink.lock().unwrap().cap());
    assert!(
        sent as f64 <= cap as f64 * span.as_secs_f64() + 64.0 * 1024.0,
        "the host sent {sent} B in {span:?} against a cap of {cap} B/s"
    );
    before
}

fn assert_healthy(sim: &Sim, ids: &[usize]) {
    for &i in ids {
        let s = sim.peers[i].out.pace_stats_at(sim.at(sim.now));
        assert!(
            s.wait_p95 <= PERIOD,
            "peer {i}: movement queue wait p95 {:?} exceeds one send period",
            s.wait_p95
        );
        assert!(
            s.wait_max <= Duration::from_millis(100),
            "peer {i}: movement queue wait max {:?}",
            s.wait_max
        );
        assert!(s.movement.len <= s.movement.capacity);
        assert!(s.bulk.len <= s.bulk.capacity);
        assert!(sim.peers[i].out.pace.lock().unwrap().ticks.len() <= 64);
    }
}

const WARM: Duration = Duration::from_secs(5);
const SPAN: Duration = Duration::from_secs(20);

#[test]
fn congest_sim_seven_peers_share_a_five_megabit_host() {
    let mut sim = Sim::new(7, 5 * MBIT, f64::from(MBIT));
    measured(
        &mut sim,
        WARM,
        SPAN,
        "host + 7 peers, 5 Mbit host, 1 Mbit peers",
    );
    let all: Vec<usize> = (0..7).collect();
    assert_healthy(&sim, &all);
    let f = jain(&sim.wire_of(&all));
    assert!(f > 0.98, "fairness {f}");
    for p in &sim.peers {
        assert!(p.delivered > 0);
        assert_eq!(p.link_drops, 0, "a fair share fits the downlink");
    }
}

#[test]
fn congest_sim_capacity_dropping_five_two_five() {
    let mut sim = Sim::new(7, 5 * MBIT, f64::from(MBIT));
    let all: Vec<usize> = (0..7).collect();
    measured(&mut sim, WARM, Duration::from_secs(10), "5 Mbit");
    assert_healthy(&sim, &all);
    sim.uplink
        .lock()
        .unwrap()
        .set_cap(2 * MBIT, sim.at(sim.now));
    measured(
        &mut sim,
        Duration::from_secs(3),
        Duration::from_secs(10),
        "2 Mbit",
    );
    // Movement alone (7 x ~30 kB/s) fits 2 Mbit, so it stays fresh while
    // the catch-up traffic is what yields.
    assert_healthy(&sim, &all);
    let resumed = sim.now;
    sim.uplink
        .lock()
        .unwrap()
        .set_cap(5 * MBIT, sim.at(sim.now));
    // A fresh usable correction within 1 s on every peer.
    sim.run(Duration::from_secs(1));
    for (i, p) in sim.peers.iter().enumerate() {
        let ok = p
            .fresh
            .iter()
            .any(|(at, age)| *at > resumed && *age <= Duration::from_millis(100));
        assert!(
            ok,
            "peer {i} had no fresh state within 1 s of the capacity resuming"
        );
    }
    // Queues back to steady bounds within 2 s.
    sim.run(Duration::from_secs(1));
    for (i, p) in sim.peers.iter().enumerate() {
        let s = p.out.pace_stats_at(sim.at(sim.now));
        assert!(
            s.movement.oldest_age <= PERIOD,
            "peer {i}: movement queue still {:?} old 2 s after capacity resumed",
            s.movement.oldest_age
        );
        assert!(s.movement.len <= 4, "peer {i}: {} queued", s.movement.len);
    }
    measured(
        &mut sim,
        Duration::from_secs(2),
        Duration::from_secs(10),
        "5 Mbit again",
    );
    assert_healthy(&sim, &all);
    for p in &sim.peers {
        assert!(p.out.adapt().snapshot_divisor == 1);
    }
}

#[test]
fn congest_sim_one_constrained_peer_does_not_slow_the_healthy() {
    let mut sim = Sim::new(7, 5 * MBIT, f64::from(MBIT));
    // Peer 3's downlink is 160 kbit/s: less than one full-rate stream.
    sim.peers[3].link = 20_000.0;
    measured(&mut sim, WARM, SPAN, "peer 3 on 160 kbit/s");
    let healthy: Vec<usize> = [0, 1, 2, 4, 5, 6].to_vec();
    assert_healthy(&sim, &healthy);
    let f = jain(&sim.wire_of(&healthy));
    assert!(f > 0.98, "fairness among the healthy {f}");
    let slow = sim.peers[3].out.pace_stats_at(sim.at(sim.now));
    assert!(slow.decreases > 0, "the slow peer's rate was cut");
    assert!(
        slow.adapt.snapshot_divisor > 1,
        "the slow peer is sent fewer snapshots, not older ones"
    );
    assert!(
        slow.movement.oldest_age <= Duration::from_millis(100),
        "bounded queue age on the slow peer: {:?}",
        slow.movement.oldest_age
    );
    // The slow peer's state age stays bounded by the buffer plus path, not
    // seconds of backlog.
    assert!(
        sim.peers[3].state_age.percentile(99) < 500_000,
        "slow peer state age p99 {} us",
        sim.peers[3].state_age.percentile(99)
    );
}

#[test]
fn congest_sim_a_cut_uplink_is_divided_not_queued() {
    // 7 peers on a 1 Mbit host: full-rate movement (7 x ~30 kB/s) cannot
    // all fit; every peer is brought down to its share rather than
    // backlogged.
    let mut sim = Sim::new(7, MBIT, f64::from(MBIT));
    measured(
        &mut sim,
        Duration::from_secs(8),
        Duration::from_secs(10),
        "7 peers on 1 Mbit",
    );
    for p in &sim.peers {
        let s = p.out.pace_stats_at(sim.at(sim.now));
        assert!(s.movement.oldest_age <= Duration::from_millis(100));
        assert!(s.adapt.snapshot_divisor > 1);
        assert!(s.movement.len <= s.movement.capacity);
    }
    let f = jain(&sim.wire_of(&(0..7).collect::<Vec<_>>()));
    assert!(f > 0.95, "fairness {f}");
}

#[test]
fn congest_sim_links_dipping_to_a_fifth_and_back() {
    // Every downlink falls from 1 Mbit to 200 kbit and returns: the
    // controllers find the narrower path from the round-trip time and
    // loss, shed cadence and catch-up, and climb back when it widens.
    let mut sim = Sim::new(7, 5 * MBIT, f64::from(MBIT));
    let all: Vec<usize> = (0..7).collect();
    measured(&mut sim, WARM, Duration::from_secs(5), "1 Mbit links");
    for p in &mut sim.peers {
        p.link = f64::from(MBIT) / 5.0;
    }
    measured(
        &mut sim,
        Duration::from_secs(1),
        Duration::from_secs(8),
        "200 kbit links",
    );
    for (i, p) in sim.peers.iter().enumerate() {
        let s = p.out.pace_stats_at(sim.at(sim.now));
        assert!(s.decreases > 0, "peer {i} never saw the narrower path");
        assert!(
            s.movement.oldest_age <= Duration::from_millis(100),
            "peer {i}: queue age {:?}",
            s.movement.oldest_age
        );
        // The link, not a backlog, is what the state age measures.
        assert!(p.state_age.percentile(99) < 400_000, "peer {i} age p99");
    }
    let resumed = sim.now;
    for p in &mut sim.peers {
        p.link = f64::from(MBIT);
    }
    sim.run(Duration::from_secs(1));
    for (i, p) in sim.peers.iter().enumerate() {
        let ok = p
            .fresh
            .iter()
            .any(|(at, age)| *at > resumed && *age <= Duration::from_millis(100));
        assert!(
            ok,
            "peer {i}: no fresh usable state within 1 s of capacity resuming"
        );
    }
    sim.run(Duration::from_secs(1));
    for (i, p) in sim.peers.iter().enumerate() {
        let s = p.out.pace_stats_at(sim.at(sim.now));
        assert!(
            s.movement.oldest_age <= PERIOD,
            "peer {i} queue not back to bounds"
        );
        assert!(s.movement.len <= 4, "peer {i}: {} queued", s.movement.len);
    }
    // The rate climbs back in steps, not one jump; report how long.
    let mut waited = Duration::from_secs(2);
    while sim.peers.iter().any(|p| p.out.adapt().snapshot_divisor > 1)
        && waited < Duration::from_secs(30)
    {
        sim.run(Duration::from_millis(250));
        waited += Duration::from_millis(250);
    }
    println!(
        "full cadence on every peer {:.2} s after the links widened",
        waited.as_secs_f64()
    );
    assert!(
        waited <= Duration::from_secs(15),
        "recovery took {waited:?}"
    );
    sim.run(Duration::from_secs(10));
    assert_healthy(&sim, &all);
    for (i, p) in sim.peers.iter().enumerate() {
        assert_eq!(
            p.out.adapt().snapshot_divisor,
            1,
            "peer {i} never recovered full cadence"
        );
    }
}

/// Bulk catch-up fragments must not cost the movement group its rate
/// (NETPLAY-10C-2b): for every movement size that fits the ceiling on its
/// own, adding four bulk fragments per snapshot leaves the controller at
/// the ceiling with nothing expired, because bulk only spends the credit
/// above its reserve.
#[test]
fn congest_sim_bulk_fragments_leave_multi_fragment_movement_its_rate() {
    for (frags, bytes) in [(1, 900), (2, 1400), (3, 1250), (3, 1350), (4, 1000)] {
        let mut sim = Sim::new(1, 40 * MBIT, f64::from(10 * MBIT));
        sim.movement_shape = Some((frags, bytes));
        sim.run(Duration::from_secs(20));
        let s = sim.peers[0].out.pace_stats_at(sim.at(sim.now));
        assert_eq!(s.rate, s.ceiling, "{frags}x{bytes}: {s:?}");
        assert_eq!(s.decreases, 0, "{frags}x{bytes}: {s:?}");
        assert_eq!(s.movement.expired, 0, "{frags}x{bytes}: {s:?}");
        assert!(s.adapt.bulk, "{frags}x{bytes}: {s:?}");
        assert!(s.bulk_sent > 0, "bulk still flows at {frags}x{bytes}");
    }
}

/// A join-time burst of reliable retransmits on an adequate link (#2126):
/// the roster/Props catch-up is lost a few times as the client connects,
/// which cuts the rate below half the ceiling and switches bulk off. Once
/// the link is clean the controller must be back at bulk and near the
/// ceiling within `JOIN_BURST_RECOVERY`.
#[test]
fn congest_sim_join_time_retransmit_burst_recovers_bulk() {
    let mut sim = Sim::new(1, 40 * MBIT, f64::from(10 * MBIT));
    sim.run(Duration::from_secs(1));
    // Retransmits at 0.0, 0.3 and 0.6 s into the burst: three cuts.
    for _ in 0..3 {
        sim.peers[0].retransmits += 1;
        sim.run(Duration::from_millis(300));
    }
    let burst_end = sim.now;
    let s = sim.peers[0].out.pace_stats_at(sim.at(sim.now));
    println!("after burst: {s:?}");
    assert!(!s.adapt.bulk, "the burst should have cut bulk: {s:?}");
    let mut bulk_back = None;
    let mut near_ceiling = None;
    while sim.now < burst_end + Duration::from_secs(30) {
        sim.run(Duration::from_millis(50));
        let s = sim.peers[0].out.pace_stats_at(sim.at(sim.now));
        if bulk_back.is_none() && s.adapt.bulk {
            bulk_back = Some(sim.now - burst_end);
        }
        if near_ceiling.is_none() && u64::from(s.rate) * 10 >= u64::from(s.ceiling) * 9 {
            near_ceiling = Some(sim.now - burst_end);
        }
    }
    println!("bulk back after {bulk_back:?}, near ceiling after {near_ceiling:?}");
    let bound = JOIN_BURST_RECOVERY;
    assert!(
        bulk_back.is_some_and(|d| d <= bound),
        "bulk back {bulk_back:?}"
    );
    assert!(
        near_ceiling.is_some_and(|d| d <= bound),
        "ceiling {near_ceiling:?}"
    );
}

const JOIN_BURST_RECOVERY: Duration = Duration::from_secs(3);
