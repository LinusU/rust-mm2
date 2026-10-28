//! NETPLAY-10 residual (#2158): the congestion envelope with real `mm2`
//! processes. `mm2_net::congest_qual_tests` drives a real `Host` and seven
//! lane-client *threads* with a miniature producer; here one headless
//! `--paced` `mm2 --host` and seven headless `--paced` `mm2 --join`
//! processes play a dev-world cruise, each client's lane behind its own
//! [`UdpImpairProxy`] (1 Mbit/s down, 256 kbit/s up, 150 ms drop-tail
//! buffer, 20 ms one way). The app's real producer (`publish_snapshots`),
//! vehicle simulation and prediction are in the loop.
//!
//! Two runs, both `#[ignore]`d because their gates are real-time (run
//! `scripts/netcode-qualification-mm2.sh` on an otherwise idle machine):
//!
//! * **ordinary**: the host's uplink dips 5 -> 2 -> 5 Mbit/s (the stdin
//!   `uplink <kbit>` word), seat 6 has clustered loss and reordering,
//!   seat 7 is `SIGSTOP`ped for 10 s (a peer that neither reads nor
//!   sends, with its sockets open). Gates, on the healthy seats 1-5:
//!   usable-correction gap p95 <= 66 ms / max <= 250 ms (the client's
//!   `corr=` cell) and host movement-queue wait p95 <= 33 ms / max
//!   <= 100 ms (the host's `qwait=` cell).
//! * **legs**: one recovery leg per client at a different time: `SIGSTOP`
//!   pauses of 250 and 100 ms, blackouts of 500 and 250 ms, bursts of 6
//!   and 3 lost datagrams. Each client's largest correction gap must stay
//!   within its leg plus 1 s.
//! * **host pause**: a 250 ms `SIGSTOP` of the host; every client's
//!   largest gap must stay within 1.25 s.
//!
//! What this does not reach, stated rather than hidden: the app records a
//! run-long gap distribution, not a time series, so "movement queue steady
//! within 2 s of a leg" is not measured; "usable" is judged against the
//! newest tick the client has seen, not the host's clock; headless runs
//! have no render. Evidence level: synthetic integration, no retail data,
//! no Rocket League parity claim.

use std::net::SocketAddr;
use std::thread;
use std::time::{Duration, Instant};

use crate::net_drive::{HOST_FRAME_CEILING, field, host_args, join_args};
use crate::support::Proc;
use mm2_net::{Impair, LinkDir, LinkRate, UdpImpair, UdpImpairProxy};

const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");
const SEATS: usize = 7;
const BUFFER: Duration = Duration::from_millis(150);
const ONE_WAY: Duration = Duration::from_millis(20);

struct Seat {
    proc: Proc,
    proxy: UdpImpairProxy,
}

fn link(kbit: u32, extra: Impair) -> UdpImpair {
    UdpImpair {
        link: Impair {
            delay: ONE_WAY,
            ..extra
        },
        rate: Some(LinkRate::kbit(kbit, BUFFER)),
        ..UdpImpair::default()
    }
}

/// Spawn the host and the clients, start the session, and return once
/// every client is live. `clustered_loss` seat index gets 6 % loss and 3 %
/// reordering both ways on top of the rate cap.
fn start_session(clustered_loss: Option<usize>) -> (Proc, Vec<Seat>, Instant) {
    let install = tempfile::tempdir().unwrap();
    let mut args = host_args(install.path(), HOST_FRAME_CEILING);
    args.push("--paced".into());
    let mut host = Proc::spawn(MM2_EXE, &args);
    let listening = host.until("listening=");
    let addr: SocketAddr = field(&listening, "listening").parse().unwrap();
    let lane_port: u16 = field(&listening, "lane_port").parse().unwrap();
    let mut seats = Vec::new();
    for n in 0..SEATS {
        let proxy = UdpImpairProxy::loopback_seeded(
            SocketAddr::new(addr.ip(), lane_port),
            0x10_0000 + n as u64,
        )
        .unwrap();
        let bad = Impair {
            loss: 0.06,
            reorder: 0.03,
            ..Impair::default()
        };
        let extra = if clustered_loss == Some(n) {
            bad
        } else {
            Impair::default()
        };
        proxy.set(LinkDir::Down, link(1000, extra));
        proxy.set(LinkDir::Up, link(256, extra));
        let route = proxy.route();
        let mut args = join_args(install.path(), addr, &format!("seat{}", n + 1), 600_000);
        args.extend([
            "--paced".to_string(),
            "--lane-via".to_string(),
            format!("{}/{}", route.dial, route.advertise_port),
        ]);
        let proc = Proc::spawn(MM2_EXE, &args);
        // Join order fixes the wire seat: client n is seat n + 1.
        host.until("ready=true");
        seats.push(Seat { proc, proxy });
    }
    host.cmd("start");
    host.until("event=started generation=1");
    for s in &seats {
        s.proc.until("event=snap_applied first=true");
    }
    // The first seconds are a declared warm-up.
    thread::sleep(Duration::from_secs(3));
    let t0 = Instant::now();
    (host, seats, t0)
}

fn sleep_until(t0: Instant, at: Duration) {
    let now = t0.elapsed();
    if at > now {
        thread::sleep(at - now);
    }
}

/// `quit` the host (the clients then lose it and print their records)
/// and read every process's final record.
fn finish(mut host: Proc, seats: Vec<Seat>) -> (String, Vec<String>) {
    host.cmd("quit");
    let bound = Duration::from_secs(90);
    let host_rec = host.until_within("smoke=headless-physics", bound);
    let recs: Vec<String> = seats
        .iter()
        .map(|s| s.proc.until_within("smoke=headless-physics", bound))
        .collect();
    eprintln!("host: {host_rec}");
    for (n, r) in recs.iter().enumerate() {
        eprintln!("seat {}: {r}", n + 1);
    }
    (host_rec, recs)
}

/// `(count, p95 ms, max ms)` of a client's `corr=` cell.
fn corr(rec: &str) -> (u64, u64, u64) {
    let v: Vec<u64> = field(rec, "corr")
        .split('/')
        .map(|x| x.parse().unwrap())
        .collect();
    (v[0], v[1], v[2])
}

/// `(p95 us, max us)` the host's `qwait=` cell reports for `seat`.
fn qwait(host_rec: &str, seat: u16) -> (u64, u64) {
    let cell = field(host_rec, "qwait")
        .split(',')
        .find_map(|c| c.strip_prefix(&format!("{seat}:")))
        .unwrap_or_else(|| panic!("no qwait for seat {seat}: {host_rec}"));
    let (p95, max) = cell.split_once('/').unwrap();
    (p95.parse().unwrap(), max.parse().unwrap())
}

#[test]
#[ignore = "real-time qualification: run scripts/netcode-qualification-mm2.sh on a quiet machine"]
fn the_ordinary_envelope_holds_across_real_processes() {
    // Seat 6 (client index 5) has clustered loss, seat 7 (index 6) stops.
    let (mut host, seats, t0) = start_session(Some(5));
    sleep_until(t0, Duration::from_secs(4));
    seats[6].proc.stop();
    sleep_until(t0, Duration::from_secs(7));
    host.cmd("uplink 2000");
    sleep_until(t0, Duration::from_secs(13));
    host.cmd("uplink 5000");
    sleep_until(t0, Duration::from_secs(14));
    seats[6].proc.cont();
    sleep_until(t0, Duration::from_secs(22));
    let (host_rec, recs) = finish(host, seats);

    let mut failures = Vec::new();
    for (n, rec) in recs.iter().enumerate().take(5) {
        let (count, p95, max) = corr(rec);
        let (q95, qmax) = qwait(&host_rec, n as u16 + 1);
        eprintln!(
            "healthy seat {}: gaps n={count} p95 {p95} ms max {max} ms | queue wait p95 {:.1} max {:.1} ms",
            n + 1,
            q95 as f64 / 1000.0,
            qmax as f64 / 1000.0
        );
        if p95 > 66 {
            failures.push(format!("seat {} correction gap p95 {p95} ms > 66", n + 1));
        }
        if max > 250 {
            failures.push(format!("seat {} correction gap max {max} ms > 250", n + 1));
        }
        if q95 > 33_000 {
            failures.push(format!("seat {} queue wait p95 {q95} us > 33000", n + 1));
        }
        if qmax > 100_000 {
            failures.push(format!("seat {} queue wait max {qmax} us > 100000", n + 1));
        }
    }
    eprintln!("host lane: {}", field(&host_rec, "lane"));
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
#[ignore = "real-time qualification: run scripts/netcode-qualification-mm2.sh on a quiet machine"]
fn every_recovery_leg_recovers_within_a_second_across_real_processes() {
    let (host, seats, t0) = start_session(None);
    let at = |k: u64| Duration::from_secs(2 + 3 * k);
    sleep_until(t0, at(0));
    seats[0].proc.stop();
    thread::sleep(Duration::from_millis(250));
    seats[0].proc.cont();
    sleep_until(t0, at(1));
    seats[1].proc.stop();
    thread::sleep(Duration::from_millis(100));
    seats[1].proc.cont();
    for (k, (idx, ms)) in [(2usize, 500u64), (3, 250)].into_iter().enumerate() {
        sleep_until(t0, at(2 + k as u64));
        let black = UdpImpair {
            blackhole: true,
            ..UdpImpair::default()
        };
        let (up, down) = (
            seats[idx].proxy.impair(LinkDir::Up),
            seats[idx].proxy.impair(LinkDir::Down),
        );
        seats[idx].proxy.set(LinkDir::Up, black);
        seats[idx].proxy.set(LinkDir::Down, black);
        thread::sleep(Duration::from_millis(ms));
        seats[idx].proxy.set(LinkDir::Up, up);
        seats[idx].proxy.set(LinkDir::Down, down);
    }
    sleep_until(t0, at(4));
    seats[4].proxy.drop_next(LinkDir::Down, 6);
    sleep_until(t0, at(5));
    seats[5].proxy.drop_next(LinkDir::Down, 3);
    sleep_until(t0, at(6));
    let (_, recs) = finish(host, seats);

    // Each client's own leg, ms (seat 7 is the untouched control). A
    // burst of lost datagrams has no length of its own: only the bound.
    let legs = [250u64, 100, 500, 250, 0, 0, 0];
    let mut failures = Vec::new();
    for (n, rec) in recs.iter().enumerate() {
        let (count, p95, max) = corr(rec);
        eprintln!(
            "seat {}: gaps n={count} p95 {p95} ms max {max} ms (leg {} ms)",
            n + 1,
            legs[n]
        );
        if max > legs[n] + 1000 {
            failures.push(format!("seat {} max gap {max} ms > leg + 1000", n + 1));
        }
        // The leg took effect: it shows as a gap of about its length.
        if max < legs[n] * 6 / 10 {
            failures.push(format!(
                "seat {} saw no gap for its {} ms leg",
                n + 1,
                legs[n]
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
#[ignore = "real-time qualification: run scripts/netcode-qualification-mm2.sh on a quiet machine"]
fn a_paused_host_recovers_every_client_within_a_second() {
    let (host, seats, t0) = start_session(None);
    sleep_until(t0, Duration::from_secs(4));
    // SIGSTOP of the host process: its producer, loop and sockets all stop.
    host.stop();
    thread::sleep(Duration::from_millis(250));
    host.cont();
    sleep_until(t0, Duration::from_secs(10));
    let (_, recs) = finish(host, seats);
    let mut failures = Vec::new();
    for (n, rec) in recs.iter().enumerate() {
        let (count, p95, max) = corr(rec);
        eprintln!("seat {}: gaps n={count} p95 {p95} ms max {max} ms", n + 1);
        if max > 250 + 1000 {
            failures.push(format!("seat {} max gap {max} ms > 1250", n + 1));
        }
        if max < 150 {
            failures.push(format!("seat {} saw no gap for the host pause", n + 1));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
