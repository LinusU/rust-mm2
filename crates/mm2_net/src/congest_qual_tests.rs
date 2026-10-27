//! NETPLAY-10E: the congestion qualification run.
//!
//! A real [`Host`] and seven real lane clients (eight seats with the host's
//! own) over loopback sockets. Every client reaches the host through its own
//! [`UdpImpairProxy`] shaped as a drop-tail bottleneck: 1 Mbit/s down,
//! 256 kbit/s up, 150 ms of buffer, 20 ms one way. The host's aggregate
//! uplink cap is the production 5 Mbit/s and drops to 2 Mbit/s and back
//! during the run. The producer is the app's: 30 Hz baselined snapshots
//! (owner group as movement, a world group as bulk), thinned by
//! `peer_adapt`.
//!
//! Seats, fixed before the run:
//!
//! | seat | role |
//! |---|---|
//! | 0-4 | healthy: bottleneck links only |
//! | 5 | clustered loss: 6 % loss, 3 % reordering, both ways |
//! | 6 | does not read for 10 s of the ordinary window |
//!
//! The run has three parts: a warm-up (the controllers' start-up, the first
//! three seconds, declared in advance), an **ordinary window** (20 s that
//! includes the capacity dip) and **recovery legs**, tagged by kind: a
//! 100-250 ms process pause (the client neither reads nor sends), a
//! 250-500 ms blackout (both directions) and a 3-6 datagram burst loss.
//! A gap in a healthy seat's usable corrections is excluded from the
//! ordinary gates only if it overlaps a leg *on that seat*; nothing else
//! is excluded, so a spontaneous stall of the host, the relay or the
//! machine counts against the gate.
//!
//! * Usable correction: a snapshot carrying the seat's own row that is at
//!   most three host ticks (100 ms) old on arrival.
//! * Ordinary gates (healthy seats): correction gap p95 <= 2 snapshot
//!   periods and max <= 250 ms; movement-queue age p95 <= one send period
//!   and max <= 100 ms, read from the host's pacing stats at the end of the
//!   ordinary window.
//! * Recovery bounds, per leg: a usable correction within 1 s of the leg's
//!   end, the movement queue back to steady (age <= one period, <= 2
//!   datagrams, three polls running) within 2 s.
//!
//! The pure analysis has CI tests. The run itself takes about 50 s of wall
//! time, depends on the machine's scheduling, and is `#[ignore]`d:
//!
//! ```sh
//! cargo test -p mm2_net --release congest_qual -- --ignored --nocapture
//! ```
//!
//! The report goes to stderr; `scripts/netcode-qualification.sh` keeps a copy
//! under `target/captures/netcode-qualification/`.
//! Evidence level: synthetic integration over loopback. No WAN, no real
//! NAT, no retail data, no rendered or audio result.

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::lane_tests::{cmd, hello_for, host, start_session};
use crate::{
    Client, HOST_UPLINK_BYTES_PER_SEC, Impair, LaneRoute, LinkDir, LinkRate, MAX_SNAP_IMPACTS,
    Message, PaceStats, Priority, SnapEntry, SnapImpact, UdpImpair, UdpImpairProxy,
};

/// One snapshot period (30 Hz).
const PERIOD: Duration = Duration::from_millis(33);
const CLIENTS: usize = 7;
const HEALTHY: std::ops::Range<usize> = 0..5;
const LOSSY: usize = 5;
const NONREADER: usize = 6;
/// A snapshot older than this many host ticks on arrival is not a usable
/// correction.
const FRESH_TICKS: u64 = 3;
const WARMUP: Duration = Duration::from_secs(3);
const ORDINARY_END: Duration = Duration::from_secs(23);
/// Spacing of the recovery legs.
const LEG_SLOT: Duration = Duration::from_secs(4);

const GAP_P95_MAX: Duration = Duration::from_millis(66);
const GAP_MAX: Duration = Duration::from_millis(250);
const QUEUE_P95_MAX: Duration = PERIOD;
const QUEUE_MAX: Duration = Duration::from_millis(100);
const FRESH_WITHIN: Duration = Duration::from_secs(1);
const QUEUE_WITHIN: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Pause,
    Blackout,
    Burst,
}

/// One tagged recovery leg: `[start, end]` since the run began.
#[derive(Debug, Clone)]
struct Leg {
    kind: Kind,
    seat: usize,
    detail: String,
    start: Duration,
    end: Duration,
}

/// A usable-or-not own-row arrival: time since the run began, whether it
/// was fresh.
type Arrival = (Duration, bool);

/// Intervals between consecutive usable arrivals.
fn usable_gaps(arrivals: &[Arrival]) -> Vec<(Duration, Duration)> {
    let usable: Vec<Duration> = arrivals.iter().filter(|a| a.1).map(|a| a.0).collect();
    usable.windows(2).map(|w| (w[0], w[1])).collect()
}

/// The gaps after `from` that overlap none of `legs` (the legs of one
/// seat).
fn ordinary_gaps(gaps: &[(Duration, Duration)], legs: &[&Leg], from: Duration) -> Vec<Duration> {
    gaps.iter()
        .filter(|(a, _)| *a >= from)
        .filter(|(a, b)| !legs.iter().any(|l| *a < l.end && *b > l.start))
        .map(|(a, b)| *b - *a)
        .collect()
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = ((sorted.len() as f64 - 1.0) * p).ceil() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

/// Time from `end` to the first usable arrival at or after it.
fn fresh_after(arrivals: &[Arrival], end: Duration) -> Option<Duration> {
    arrivals
        .iter()
        .find(|a| a.1 && a.0 >= end)
        .map(|a| a.0 - end)
}

#[test]
fn gaps_are_between_usable_arrivals_only() {
    let ms = Duration::from_millis;
    let arrivals = [
        (ms(0), true),
        (ms(30), false),
        (ms(100), true),
        (ms(130), true),
    ];
    assert_eq!(
        usable_gaps(&arrivals),
        [(ms(0), ms(100)), (ms(100), ms(130))]
    );
}

#[test]
fn only_a_gap_overlapping_a_leg_of_the_seat_is_excluded() {
    let ms = Duration::from_millis;
    let leg = Leg {
        kind: Kind::Blackout,
        seat: 0,
        detail: String::new(),
        start: ms(1000),
        end: ms(1500),
    };
    let gaps = [
        (ms(900), ms(930)),
        (ms(960), ms(1700)),
        (ms(1700), ms(1730)),
        (ms(2000), ms(2400)),
    ];
    let kept = ordinary_gaps(&gaps, &[&leg], ms(0));
    assert_eq!(kept, [ms(30), ms(30), ms(400)]);
    // No leg: nothing is excluded, a long stall stays in the sample.
    assert_eq!(ordinary_gaps(&gaps, &[], ms(0)).len(), 4);
    // Warm-up is dropped by start time.
    assert_eq!(ordinary_gaps(&gaps, &[], ms(1000)).len(), 2);
}

#[test]
fn percentiles_and_fresh_after() {
    let ms = Duration::from_millis;
    let mut v: Vec<Duration> = (1..=100).map(ms).collect();
    v.sort();
    assert_eq!(percentile(&v, 0.95), ms(96));
    assert_eq!(percentile(&v, 1.0), ms(100));
    assert_eq!(percentile(&[], 0.5), Duration::ZERO);
    let a = [(ms(10), true), (ms(600), false), (ms(900), true)];
    assert_eq!(fresh_after(&a, ms(500)), Some(ms(400)));
    assert_eq!(fresh_after(&a, ms(901)), None);
}

/// What a client thread is told to do.
#[derive(Default)]
struct Knobs {
    /// Nanoseconds since the run began until which the client neither
    /// reads nor sends (a process pause).
    pause_until: AtomicU64,
    /// The client leaves its inbox unread.
    nonreading: AtomicBool,
    stop: AtomicBool,
}

struct Seat {
    id: u16,
    proxy: UdpImpairProxy,
    base: [UdpImpair; 2],
    knobs: Arc<Knobs>,
    arrivals: Arc<Mutex<Vec<Arrival>>>,
    thread: Option<thread::JoinHandle<()>>,
}

fn link(kbit: u32, loss: f64, reorder: f64) -> UdpImpair {
    UdpImpair {
        link: Impair {
            delay: Duration::from_millis(20),
            loss,
            reorder,
            ..Impair::default()
        },
        rate: Some(LinkRate::kbit(kbit, Duration::from_millis(150))),
        ..UdpImpair::default()
    }
}

fn entries(tick: u64, players: &[u16]) -> Vec<SnapEntry> {
    players
        .iter()
        .map(|&player| SnapEntry {
            player,
            pos: [
                tick as f32 * 0.5 + f32::from(player),
                f32::from(player),
                0.0,
            ],
            vel: [15.0, 0.0, 0.0],
            ..SnapEntry::default()
        })
        .collect()
}

fn group(generation: u64, tick: u64, entries: Vec<SnapEntry>, world: bool) -> Message {
    let impacts = if world {
        (0..MAX_SNAP_IMPACTS)
            .map(|i| SnapImpact {
                seat: 1,
                id: u64::from(i) + tick * 64,
                tick,
                epoch: 0,
                ordinal: 0,
                point: [
                    tick as f32 * 0.37 + i as f32,
                    (tick * 7 + u64::from(i)) as f32 * 0.11,
                    1.5,
                ],
                normal: [0.0, 1.0, 0.0],
                severity: 1.0 + (tick % 13) as f32,
                audio_id: 0,
            })
            .collect()
    } else {
        Vec::new()
    };
    Message::Snap {
        generation,
        tick,
        entries,
        trailers: Vec::new(),
        impacts,
        race: None,
    }
}

fn client_thread(
    mut client: Client,
    id: u16,
    generation: u64,
    knobs: Arc<Knobs>,
    arrivals: Arc<Mutex<Vec<Arrival>>>,
    host_tick: Arc<AtomicU64>,
    go: Instant,
) -> thread::JoinHandle<()> {
    client.set_timeout(Some(Duration::from_millis(5))).unwrap();
    thread::spawn(move || {
        let mut seq = 0u64;
        let mut last_tick = 0u64;
        let mut next_send = Instant::now();
        while !knobs.stop.load(Ordering::Relaxed) {
            if go.elapsed().as_nanos() < u128::from(knobs.pause_until.load(Ordering::Acquire)) {
                thread::sleep(Duration::from_millis(2));
                continue;
            }
            if Instant::now() >= next_send {
                next_send += Duration::from_micros(16_667);
                seq += 1;
                let first = seq.saturating_sub(3).max(1);
                let commands = (first..=seq).map(|_| cmd(9)).collect();
                let _ = client.send(&Message::InputWindow {
                    generation,
                    epoch: 0,
                    first_seq: first,
                    commands,
                });
            }
            if knobs.nonreading.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            match client.recv() {
                Ok(Message::Snap { tick, entries, .. }) => {
                    let at = go.elapsed();
                    if tick > last_tick && entries.iter().any(|e| e.player == id) {
                        last_tick = tick;
                        let fresh =
                            host_tick.load(Ordering::Acquire).saturating_sub(tick) <= FRESH_TICKS;
                        arrivals.lock().unwrap().push((at, fresh));
                    }
                }
                Ok(_) => {}
                Err(_) => thread::sleep(Duration::from_millis(1)),
            }
        }
    })
}

struct Phase {
    label: &'static str,
    cap_bytes: u32,
    secs: f64,
    admitted: u64,
}

fn seconds(d: Duration) -> f64 {
    d.as_secs_f64()
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn sleep_until(go: Instant, at: Duration) {
    let deadline = go + at;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        thread::sleep(left.min(Duration::from_millis(5)));
    }
}

#[test]
#[ignore = "qualification run: ~50 s of wall time, scheduling-sensitive; see the module docs"]
fn congest_qual_eight_seats_under_caps_loss_and_recovery_legs() {
    let host = host();
    let target = SocketAddr::new(host.addr().ip(), host.lane_port());
    let mut clients = Vec::new();
    let mut proxies = Vec::new();
    for i in 0..CLIENTS {
        let proxy = UdpImpairProxy::loopback_seeded(target, 0x10e0 + i as u64).unwrap();
        let LaneRoute { .. } = proxy.route();
        let c = Client::join_lane_routed(
            host.addr(),
            &hello_for(&format!("seat{i}")),
            Duration::from_secs(5),
            Some(proxy.route()),
        )
        .unwrap();
        proxies.push(proxy);
        clients.push(c);
    }
    let ids: Vec<u16> = clients.iter().map(Client::player_id).collect();
    for &id in &ids {
        crate::lane_tests::eventually("a lane", || host.lane_established(id));
    }
    let generation = {
        let mut refs: Vec<&mut Client> = clients.iter_mut().collect();
        start_session(&host, &mut refs)
    };

    // Shape every link. Seat 5 adds clustered loss on both directions.
    let go = Instant::now();
    let host_tick = Arc::new(AtomicU64::new(0));
    let mut seats: Vec<Seat> = Vec::new();
    for (i, (client, proxy)) in clients.into_iter().zip(proxies).enumerate() {
        let (loss, reorder) = if i == LOSSY { (0.06, 0.03) } else { (0.0, 0.0) };
        let base = [link(256, loss, reorder), link(1000, loss, reorder)];
        proxy.set(LinkDir::Up, base[0]);
        proxy.set(LinkDir::Down, base[1]);
        let knobs = Arc::new(Knobs::default());
        let arrivals = Arc::new(Mutex::new(Vec::new()));
        let thread = client_thread(
            client,
            ids[i],
            generation,
            knobs.clone(),
            arrivals.clone(),
            host_tick.clone(),
            go,
        );
        seats.push(Seat {
            id: ids[i],
            proxy,
            base,
            knobs,
            arrivals,
            thread: Some(thread),
        });
    }

    // The producer: the app's loop in miniature, on an absolute schedule so
    // a late tick is visible as a late tick.
    let stop = Arc::new(AtomicBool::new(false));
    let late = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));
    let producer = {
        let ctl = host.ctl();
        let (stop, late, host_tick, errors) = (
            stop.clone(),
            late.clone(),
            host_tick.clone(),
            errors.clone(),
        );
        let ids = ids.clone();
        thread::spawn(move || {
            let mut players = vec![0u16];
            players.extend(&ids);
            let mut skipped = vec![0u32; ids.len()];
            let mut next = Instant::now();
            let mut tick = 0u64;
            while !stop.load(Ordering::Relaxed) {
                tick += 1;
                host_tick.store(tick, Ordering::Release);
                for (n, &id) in ids.iter().enumerate() {
                    let adapt = ctl.peer_adapt(id).unwrap_or_default();
                    if skipped[n] + 1 < adapt.snapshot_divisor {
                        skipped[n] += 1;
                        continue;
                    }
                    skipped[n] = 0;
                    let mut groups =
                        vec![(0u8, group(generation, tick, entries(tick, &players), false))];
                    let mut prios = vec![Priority::Movement];
                    if adapt.bulk {
                        groups.push((1, group(generation, tick, Vec::new(), true)));
                        prios.push(Priority::Bulk);
                    }
                    let r = ctl.send_snap_states_prio(id, generation, tick, 1, &groups, &prios);
                    if r.is_err() {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
                next += PERIOD;
                let now = Instant::now();
                if now > next + Duration::from_millis(10) {
                    late.fetch_add(1, Ordering::Relaxed);
                    next = now;
                }
                thread::sleep(next.saturating_duration_since(now));
            }
        })
    };

    // ----- warm-up and the ordinary window ------------------------------
    let mut phases: Vec<Phase> = Vec::new();
    let cap5 = HOST_UPLINK_BYTES_PER_SEC;
    let cap2 = 2_000_000 / 8;
    sleep_until(go, WARMUP);
    let mut mark = (go.elapsed(), host.uplink().1);
    let mut close =
        |label: &'static str, cap: u32, host: &crate::Host, mark: &mut (Duration, u64)| {
            let now = (go.elapsed(), host.uplink().1);
            phases.push(Phase {
                label,
                cap_bytes: cap,
                secs: seconds(now.0 - mark.0),
                admitted: now.1 - mark.1,
            });
            *mark = now;
        };
    host.set_uplink_cap(cap5);
    sleep_until(go, Duration::from_secs(6));
    seats[NONREADER]
        .knobs
        .nonreading
        .store(true, Ordering::Relaxed);
    sleep_until(go, Duration::from_secs(10));
    close("5 Mbit/s", cap5, &host, &mut mark);
    host.set_uplink_cap(cap2);
    sleep_until(go, Duration::from_secs(16));
    close("2 Mbit/s", cap2, &host, &mut mark);
    seats[NONREADER]
        .knobs
        .nonreading
        .store(false, Ordering::Relaxed);
    host.set_uplink_cap(cap5);
    sleep_until(go, ORDINARY_END);
    close("5 Mbit/s again", cap5, &host, &mut mark);
    let ordinary_pace: Vec<PaceStats> = seats
        .iter()
        .map(|s| host.lane_pace(s.id).unwrap_or_default())
        .collect();

    // ----- recovery legs ---------------------------------------------------
    let mut legs: Vec<Leg> = Vec::new();
    let mut recovery: Vec<(Option<Duration>, Option<Duration>)> = Vec::new();
    let plan: [(Kind, usize, u64); 6] = [
        (Kind::Pause, 0, 250),
        (Kind::Pause, 1, 100),
        (Kind::Blackout, 2, 500),
        (Kind::Blackout, 3, 250),
        (Kind::Burst, 4, 6),
        (Kind::Burst, 1, 3),
    ];
    for (n, (kind, seat, amount)) in plan.into_iter().enumerate() {
        let slot = ORDINARY_END + LEG_SLOT * n as u32;
        sleep_until(go, slot);
        let s = &seats[seat];
        let start = go.elapsed();
        let detail;
        match kind {
            Kind::Pause => {
                let until = start + Duration::from_millis(amount);
                s.knobs
                    .pause_until
                    .store(until.as_nanos() as u64, Ordering::Release);
                detail = format!("{amount} ms process pause");
                sleep_until(go, until);
            }
            Kind::Blackout => {
                for dir in [LinkDir::Up, LinkDir::Down] {
                    s.proxy.set(
                        dir,
                        UdpImpair {
                            blackhole: true,
                            ..s.base[dir as usize]
                        },
                    );
                }
                detail = format!("{amount} ms blackout, both ways");
                thread::sleep(Duration::from_millis(amount));
                for dir in [LinkDir::Up, LinkDir::Down] {
                    s.proxy.set(dir, s.base[dir as usize]);
                }
            }
            Kind::Burst => {
                let before = s.proxy.stats(LinkDir::Down).burst_dropped;
                s.proxy.drop_next(LinkDir::Down, amount);
                let deadline = Instant::now() + Duration::from_secs(1);
                while s.proxy.stats(LinkDir::Down).burst_dropped < before + amount
                    && Instant::now() < deadline
                {
                    thread::sleep(Duration::from_millis(2));
                }
                detail = format!("{amount} datagrams lost in a row, down");
            }
        }
        let end = go.elapsed();
        // The queue back to steady: age within a period, at most two
        // datagrams, three polls running.
        let mut steady = 0;
        let mut queue_back = None;
        while go.elapsed() - end <= QUEUE_WITHIN {
            let m = host.lane_pace(s.id).unwrap_or_default().movement;
            steady = if m.oldest_age <= PERIOD && m.len <= 2 {
                steady + 1
            } else {
                0
            };
            if steady >= 3 {
                queue_back = Some(go.elapsed() - end);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        legs.push(Leg {
            kind,
            seat,
            detail,
            start,
            end,
        });
        recovery.push((None, queue_back));
        let _ = n;
    }
    sleep_until(go, ORDINARY_END + LEG_SLOT * plan.len() as u32);
    let run_end = go.elapsed();
    let final_pace: Vec<PaceStats> = seats
        .iter()
        .map(|s| host.lane_pace(s.id).unwrap_or_default())
        .collect();
    let uplink_total = host.uplink();
    let down: Vec<_> = seats.iter().map(|s| s.proxy.stats(LinkDir::Down)).collect();
    let up: Vec<_> = seats.iter().map(|s| s.proxy.stats(LinkDir::Up)).collect();

    stop.store(true, Ordering::Relaxed);
    for s in &seats {
        s.knobs.stop.store(true, Ordering::Relaxed);
    }
    producer.join().unwrap();
    for s in &mut seats {
        s.thread.take().unwrap().join().unwrap();
    }

    // ----- analysis ----------------------------------------------------------
    let arrivals: Vec<Vec<Arrival>> = seats
        .iter()
        .map(|s| s.arrivals.lock().unwrap().clone())
        .collect();
    for (leg, rec) in legs.iter().zip(recovery.iter_mut()) {
        rec.0 = fresh_after(&arrivals[leg.seat], leg.end);
    }
    let mut report = String::new();
    let _ = writeln!(
        report,
        "NETPLAY-10E congestion qualification, loopback only"
    );
    let _ = writeln!(
        report,
        "run {:.1} s; host producer ticks late by >10 ms: {}",
        seconds(run_end),
        late.load(Ordering::Relaxed)
    );
    let mut failures: Vec<String> = Vec::new();

    let _ = writeln!(
        report,
        "\nordinary window (from {:.0} s), usable-correction gaps / movement-queue wait",
        seconds(WARMUP)
    );
    let mut pooled = Vec::new();
    for seat in 0..CLIENTS {
        let own: Vec<&Leg> = legs.iter().filter(|l| l.seat == seat).collect();
        let g = ordinary_gaps(&usable_gaps(&arrivals[seat]), &own, WARMUP);
        let mut sorted = g.clone();
        sorted.sort();
        let role = if HEALTHY.contains(&seat) {
            "healthy"
        } else if seat == LOSSY {
            "lossy"
        } else {
            "nonreader"
        };
        let p = &ordinary_pace[seat];
        let _ = writeln!(
            report,
            "seat {seat} {role:9} gaps n={} p50 {:.0} p95 {:.0} max {:.0} ms | queue wait p50 {:.0} p95 {:.0} max {:.0} ms | rate {} B/s of {} adapt {:?} decreases {}",
            sorted.len(),
            ms(percentile(&sorted, 0.5)),
            ms(percentile(&sorted, 0.95)),
            ms(percentile(&sorted, 1.0)),
            ms(p.wait_p50),
            ms(p.wait_p95),
            ms(p.wait_max),
            p.rate,
            p.ceiling,
            p.adapt,
            p.decreases,
        );
        if HEALTHY.contains(&seat) {
            pooled.extend(sorted);
            if p.wait_p95 > QUEUE_P95_MAX {
                failures.push(format!(
                    "seat {seat}: queue wait p95 {:?} > {QUEUE_P95_MAX:?}",
                    p.wait_p95
                ));
            }
            if p.wait_max > QUEUE_MAX {
                failures.push(format!(
                    "seat {seat}: queue wait max {:?} > {QUEUE_MAX:?}",
                    p.wait_max
                ));
            }
        }
    }
    pooled.sort();
    let (p95, max) = (percentile(&pooled, 0.95), percentile(&pooled, 1.0));
    let _ = writeln!(
        report,
        "healthy pooled gaps n={} p95 {:.0} ms (gate {:.0}) max {:.0} ms (gate {:.0})",
        pooled.len(),
        ms(p95),
        ms(GAP_P95_MAX),
        ms(max),
        ms(GAP_MAX)
    );
    if p95 > GAP_P95_MAX {
        failures.push(format!(
            "healthy correction gap p95 {p95:?} > {GAP_P95_MAX:?}"
        ));
    }
    if max > GAP_MAX {
        failures.push(format!("healthy correction gap max {max:?} > {GAP_MAX:?}"));
    }
    for seat in HEALTHY {
        let own: Vec<&Leg> = legs.iter().filter(|l| l.seat == seat).collect();
        let worst = ordinary_gaps(&usable_gaps(&arrivals[seat]), &own, WARMUP)
            .into_iter()
            .max()
            .unwrap_or_default();
        if worst > GAP_MAX {
            failures.push(format!(
                "seat {seat}: ordinary gap max {worst:?} > {GAP_MAX:?}"
            ));
        }
    }

    let _ = writeln!(report, "\nrates and headroom");
    for p in &phases {
        let allowed = f64::from(p.cap_bytes) * p.secs;
        let _ = writeln!(
            report,
            "host uplink {:14} cap {:.0} kbit/s, admitted {:.0} kbit/s over {:.1} s, headroom {:.1} %",
            p.label,
            f64::from(p.cap_bytes) * 8.0 / 1000.0,
            p.admitted as f64 * 8.0 / 1000.0 / p.secs,
            p.secs,
            (1.0 - p.admitted as f64 / allowed) * 100.0
        );
    }
    let _ = writeln!(
        report,
        "host uplink whole run: cap now {} B/s, {} wire bytes admitted",
        uplink_total.0, uplink_total.1
    );
    for seat in 0..CLIENTS {
        let d = &down[seat].link;
        let u = &up[seat].link;
        let _ = writeln!(
            report,
            "seat {seat} down {:.0} of 1000 kbit/s (relay drops: {} full-buffer, {} other), up {:.0} of 256 kbit/s ({} full-buffer); host rate {} B/s, movement deferred {}, uplink denied {}, bulk sent {} / expired {} / dropped {}",
            d.bytes_out as f64 * 8.0 / 1000.0 / seconds(run_end),
            down[seat].rate_dropped,
            d.dropped - down[seat].rate_dropped,
            u.bytes_out as f64 * 8.0 / 1000.0 / seconds(run_end),
            up[seat].rate_dropped,
            final_pace[seat].rate,
            final_pace[seat].movement_deferred,
            final_pace[seat].uplink_denied,
            final_pace[seat].bulk_sent,
            final_pace[seat].bulk.expired,
            final_pace[seat].bulk.dropped,
        );
    }

    let _ = writeln!(
        report,
        "\nrecovery legs (tagged, not part of the ordinary gates)"
    );
    for (leg, (fresh, queue)) in legs.iter().zip(&recovery) {
        let _ = writeln!(
            report,
            "{:?} seat {}: {}; usable correction {} after the leg, queue steady {}",
            leg.kind,
            leg.seat,
            leg.detail,
            fresh.map_or("NONE".into(), |d| format!("{:.0} ms", ms(d))),
            queue.map_or("NOT within 2 s".into(), |d| format!("{:.0} ms", ms(d))),
        );
        if fresh.is_none_or(|d| d > FRESH_WITHIN) {
            failures.push(format!(
                "{:?} seat {}: no usable correction within {FRESH_WITHIN:?}",
                leg.kind, leg.seat
            ));
        }
        if queue.is_none_or(|d| d > QUEUE_WITHIN) {
            failures.push(format!(
                "{:?} seat {}: queue not steady within {QUEUE_WITHIN:?}",
                leg.kind, leg.seat
            ));
        }
    }
    let _ = writeln!(
        report,
        "\ngate result: {}",
        if failures.is_empty() {
            "PASS".to_string()
        } else {
            format!("FAIL ({})", failures.len())
        }
    );
    for f in &failures {
        let _ = writeln!(report, "  {f}");
    }
    eprintln!("{report}");
    assert!(failures.is_empty(), "{failures:#?}");
}
