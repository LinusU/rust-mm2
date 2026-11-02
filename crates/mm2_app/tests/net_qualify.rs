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
//! * **full run** (#2150): the whole `mm2_net::congest_qual_tests` plan
//!   with the same gates and the same six tagged legs, but measured as
//!   time series: each client's usable corrections from its `--net-trace`
//!   (the instants the applied snapshot tick advanced), the host movement
//!   queue from its `event=lane_pace` records (`MM2_LANE_PACE_MS=100`),
//!   stamped as they arrive. A recovery leg is judged on "usable
//!   correction within 1 s of its end" and "queue age back within one
//!   period for three records running within 2 s of its end".
//!
//! What this does not reach, stated rather than hidden: "usable" is
//! judged against the newest tick the client has seen, not the host's
//! clock, so a trace arrival is any advance of the applied tick; the
//! `lane_pace` record carries the queue's age, not its length, so
//! "steady" is the age half of the in-process definition (age <= one
//! period, three records running) without the "<= 2 datagrams" half; the
//! trace clock is aligned to the harness's by the first-snapshot record
//! (a few milliseconds of skew); headless runs have no render. Evidence
//! level: synthetic integration, no retail data, no Rocket League parity
//! claim.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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
    /// The client's `--net-trace` file, when the run reads one.
    trace: Option<PathBuf>,
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
    start_session_with(clustered_loss, None)
}

/// The declared start-up of a run, before its gates count.
const WARMUP: Duration = Duration::from_secs(3);

fn is_host_pace(line: &str) -> bool {
    line.starts_with("event=lane_pace role=host ")
}

fn is_first_snap(line: &str) -> bool {
    line.contains("event=snap_applied first=true")
}

/// [`start_session`], and with `traces` the host prints a `lane_pace`
/// record every 100 ms and every client writes a `--net-trace` file in
/// that directory; both are stamped as they arrive.
fn start_session_with(
    clustered_loss: Option<usize>,
    traces: Option<&Path>,
) -> (Proc, Vec<Seat>, Instant) {
    let install = tempfile::tempdir().unwrap();
    let mut args = host_args(install.path(), HOST_FRAME_CEILING);
    args.push("--paced".into());
    let mut host = if traces.is_some() {
        Proc::spawn_stamped(MM2_EXE, &args, &[("MM2_LANE_PACE_MS", "100")], is_host_pace)
    } else {
        Proc::spawn(MM2_EXE, &args)
    };
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
        let trace = traces.map(|dir| dir.join(format!("seat{}.jsonl", n + 1)));
        if let Some(path) = &trace {
            args.extend(["--net-trace".to_string(), path.to_str().unwrap().into()]);
        }
        let proc = Proc::spawn_stamped(MM2_EXE, &args, &[], is_first_snap);
        // Join order fixes the wire seat: client n is seat n + 1.
        host.until("ready=true");
        seats.push(Seat { proc, proxy, trace });
    }
    host.cmd("start");
    host.until("event=started generation=1");
    for s in &seats {
        s.proc.until("event=snap_applied first=true");
    }
    // The first seconds are a declared warm-up.
    thread::sleep(WARMUP);
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
fn finish(mut host: Proc, seats: &[Seat]) -> (String, Vec<String>) {
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
    let (host_rec, recs) = finish(host, &seats);

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
    let (_, recs) = finish(host, &seats);

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
    let (_, recs) = finish(host, &seats);
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

// ----- the full run, measured as time series (#2150) ---------------------

/// Snapshot period (30 Hz) and the gates, as in
/// `mm2_net::congest_qual_tests`.
const PERIOD: Duration = Duration::from_millis(33);
const GAP_P95_MAX: Duration = Duration::from_millis(66);
const GAP_MAX: Duration = Duration::from_millis(250);
const QUEUE_MAX: Duration = Duration::from_millis(100);
const FRESH_WITHIN: Duration = Duration::from_secs(1);
const QUEUE_WITHIN: Duration = Duration::from_secs(2);
const ORDINARY_END: Duration = Duration::from_secs(23);
const LEG_SLOT: Duration = Duration::from_secs(4);
const HEALTHY: std::ops::Range<usize> = 0..5;
const LOSSY: usize = 5;
const NONREADER: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Pause,
    Blackout,
    Burst,
}

/// One tagged recovery leg on the harness clock (since `go`).
#[derive(Debug, Clone)]
struct Leg {
    kind: Kind,
    seat: usize,
    detail: String,
    start: Duration,
    end: Duration,
}

/// One host `lane_pace` record for a seat, on the harness clock.
#[derive(Debug, Clone, Copy)]
struct Pace {
    at: Duration,
    wait_p95: Duration,
    wait_max: Duration,
    age: Duration,
}

fn ms_field(rec: &str, key: &str) -> Duration {
    let v: f64 = field(rec, key)
        .parse()
        .unwrap_or_else(|_| panic!("{key} is not a number in {rec}"));
    Duration::from_secs_f64(v / 1000.0)
}

/// The host's records for wire `seat` (client index + 1) since `go`.
fn pace_series(host: &Proc, go: Instant, seat: usize) -> Vec<Pace> {
    let tag = format!(" seat={} ", seat + 1);
    host.stamped()
        .into_iter()
        .filter(|(_, l)| l.contains(&tag))
        .filter_map(|(at, l)| {
            Some(Pace {
                at: at.checked_duration_since(go)?,
                wait_p95: ms_field(&l, "wait_p95_ms"),
                wait_max: ms_field(&l, "wait_max_ms"),
                age: ms_field(&l, "age_ms"),
            })
        })
        .collect()
}

/// Instants (since `go`) at which the client's applied snapshot tick
/// advanced, read from its trace and placed on the harness clock by the
/// client's first-snapshot record.
fn arrivals(seat: &Seat, go: Instant) -> Vec<Duration> {
    let text = std::fs::read_to_string(seat.trace.as_ref().unwrap()).unwrap();
    let trace = mm2_app::netprobe::parse_trace(&text).unwrap();
    let first = seat
        .proc
        .stamped()
        .into_iter()
        .next()
        .expect("no first-snapshot record from the client")
        .0;
    let mut origin = None;
    let mut last_tick = None;
    let mut out = Vec::new();
    for s in &trace.samples {
        let Some(tick) = s.snap_tick else { continue };
        if last_tick.is_some_and(|t| tick <= t) {
            continue;
        }
        last_tick = Some(tick);
        let w0 = *origin.get_or_insert(s.wall_ms);
        let at = first + Duration::from_secs_f64((s.wall_ms - w0).max(0.0) / 1000.0);
        out.push(at.saturating_duration_since(go));
    }
    out
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = ((sorted.len() as f64 - 1.0) * p).ceil() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

/// Gaps between consecutive arrivals that start after `from` and overlap
/// none of the seat's own legs.
fn ordinary_gaps(arr: &[Duration], legs: &[&Leg], from: Duration) -> Vec<Duration> {
    arr.windows(2)
        .filter(|w| w[0] >= from)
        .filter(|w| !legs.iter().any(|l| w[0] < l.end && w[1] > l.start))
        .map(|w| w[1] - w[0])
        .collect()
}

/// Time from the leg's end until the queue age has been within one period
/// for three records running.
fn queue_back(series: &[Pace], end: Duration) -> Option<Duration> {
    let mut steady = 0;
    for p in series.iter().filter(|p| p.at > end) {
        steady = if p.age <= PERIOD { steady + 1 } else { 0 };
        if steady >= 3 {
            return Some(p.at - end);
        }
    }
    None
}

#[test]
#[ignore = "real-time qualification: run scripts/netcode-qualification-mm2.sh on a quiet machine"]
fn the_congest_qual_plan_holds_across_real_processes() {
    use std::fmt::Write as _;
    let traces =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("qualify-{}", std::process::id()));
    std::fs::create_dir_all(&traces).unwrap();
    let (mut host, seats, t0) = start_session_with(Some(LOSSY), Some(&traces));
    let go = t0 - WARMUP;
    let at = |d: Duration| sleep_until(go, d);
    let mut errors: Vec<String> = Vec::new();

    // ----- the ordinary window: the nonreader stops for 10 s, the host's
    // uplink dips 5 -> 2 -> 5 Mbit/s inside it.
    at(Duration::from_secs(6));
    seats[NONREADER].proc.stop();
    at(Duration::from_secs(10));
    host.cmd("uplink 2000");
    at(Duration::from_secs(16));
    seats[NONREADER].proc.cont();
    host.cmd("uplink 5000");
    at(ORDINARY_END);

    // ----- the recovery legs, one per slot.
    let plan: [(Kind, usize, u64); 6] = [
        (Kind::Pause, 0, 250),
        (Kind::Pause, 1, 100),
        (Kind::Blackout, 2, 500),
        (Kind::Blackout, 3, 250),
        (Kind::Burst, 4, 6),
        (Kind::Burst, 1, 3),
    ];
    let mut legs: Vec<Leg> = Vec::new();
    for (n, (kind, seat, amount)) in plan.into_iter().enumerate() {
        at(ORDINARY_END + LEG_SLOT * n as u32);
        let s = &seats[seat];
        let start = go.elapsed();
        let detail = match kind {
            Kind::Pause => {
                s.proc.stop();
                thread::sleep(Duration::from_millis(amount));
                s.proc.cont();
                format!("{amount} ms SIGSTOP")
            }
            Kind::Blackout => {
                let (up, down) = (s.proxy.impair(LinkDir::Up), s.proxy.impair(LinkDir::Down));
                let black = UdpImpair {
                    blackhole: true,
                    ..UdpImpair::default()
                };
                s.proxy.set(LinkDir::Up, black);
                s.proxy.set(LinkDir::Down, black);
                thread::sleep(Duration::from_millis(amount));
                s.proxy.set(LinkDir::Up, up);
                s.proxy.set(LinkDir::Down, down);
                format!("{amount} ms blackout, both ways")
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
                let dropped = s.proxy.stats(LinkDir::Down).burst_dropped - before;
                if dropped < amount {
                    errors.push(format!(
                        "burst on seat {seat}: only {dropped} of {amount} datagrams dropped"
                    ));
                }
                format!("{amount} datagrams lost in a row, down")
            }
        };
        legs.push(Leg {
            kind,
            seat,
            detail,
            start,
            end: go.elapsed(),
        });
    }
    at(ORDINARY_END + LEG_SLOT * plan.len() as u32);
    let run_end = go.elapsed();
    let pace: Vec<Vec<Pace>> = (0..SEATS).map(|n| pace_series(&host, go, n)).collect();

    finish(host, &seats);
    let arr: Vec<Vec<Duration>> = seats.iter().map(|s| arrivals(s, go)).collect();

    // ----- analysis
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mut report = String::new();
    let _ = writeln!(
        report,
        "#2150 congestion qualification, real mm2 processes, loopback only (run {:.1} s)",
        run_end.as_secs_f64()
    );
    let mut failures = errors;
    let mut pooled = Vec::new();
    for seat in 0..SEATS {
        let own: Vec<&Leg> = legs.iter().filter(|l| l.seat == seat).collect();
        let mut gaps = ordinary_gaps(&arr[seat], &own, WARMUP);
        // The ordinary window ends where the legs begin.
        gaps.sort();
        let last = pace[seat].iter().rev().find(|p| p.at <= ORDINARY_END);
        let role = match seat {
            s if HEALTHY.contains(&s) => "healthy",
            LOSSY => "lossy",
            _ => "nonreader",
        };
        let _ = writeln!(
            report,
            "seat {} {role:9} arrivals {} gaps n={} p95 {:.0} max {:.0} ms | queue wait p95 {} max {} ms (last record of the ordinary window)",
            seat + 1,
            arr[seat].len(),
            gaps.len(),
            ms(percentile(&gaps, 0.95)),
            ms(percentile(&gaps, 1.0)),
            last.map_or("NONE".into(), |p| format!("{:.1}", ms(p.wait_p95))),
            last.map_or("NONE".into(), |p| format!("{:.1}", ms(p.wait_max))),
        );
        if !HEALTHY.contains(&seat) {
            continue;
        }
        pooled.extend(gaps);
        match last {
            None => failures.push(format!(
                "seat {}: no lane_pace record in the ordinary window",
                seat + 1
            )),
            Some(p) => {
                if p.wait_p95 > PERIOD {
                    failures.push(format!(
                        "seat {}: queue wait p95 {:?} > {PERIOD:?}",
                        seat + 1,
                        p.wait_p95
                    ));
                }
                if p.wait_max > QUEUE_MAX {
                    failures.push(format!(
                        "seat {}: queue wait max {:?} > {QUEUE_MAX:?}",
                        seat + 1,
                        p.wait_max
                    ));
                }
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
    if pooled.is_empty() {
        failures.push("no usable-correction gaps were measured".into());
    }
    if p95 > GAP_P95_MAX {
        failures.push(format!(
            "healthy correction gap p95 {p95:?} > {GAP_P95_MAX:?}"
        ));
    }
    if max > GAP_MAX {
        failures.push(format!("healthy correction gap max {max:?} > {GAP_MAX:?}"));
    }

    let _ = writeln!(
        report,
        "\nrecovery legs (tagged, not part of the ordinary gates)"
    );
    for leg in &legs {
        let fresh = arr[leg.seat]
            .iter()
            .find(|a| **a >= leg.end)
            .map(|a| *a - leg.end);
        let queue = queue_back(&pace[leg.seat], leg.end);
        // The measurement's own check: a SIGSTOP shows in the client's
        // trace as a gap about as long as the pause.
        let widest = arr[leg.seat]
            .windows(2)
            .filter(|w| w[0] < leg.end && w[1] > leg.start)
            .map(|w| w[1] - w[0])
            .max()
            .unwrap_or_default();
        let _ = writeln!(
            report,
            "{:?} seat {}: {}; widest gap over it {:.0} ms; usable correction {} after the leg, queue steady {}",
            leg.kind,
            leg.seat + 1,
            leg.detail,
            ms(widest),
            fresh.map_or("NONE".into(), |d| format!("{:.0} ms", ms(d))),
            queue.map_or("NOT within 2 s".into(), |d| format!("{:.0} ms", ms(d))),
        );
        if leg.kind == Kind::Pause {
            let want = (leg.end - leg.start).mul_f64(0.6);
            if widest < want {
                failures.push(format!(
                    "seat {}: the pause left no gap in the trace (widest {widest:?})",
                    leg.seat + 1
                ));
            }
        }
        if fresh.is_none_or(|d| d > FRESH_WITHIN) {
            failures.push(format!(
                "{:?} seat {}: no usable correction within {FRESH_WITHIN:?}",
                leg.kind,
                leg.seat + 1
            ));
        }
        if queue.is_none_or(|d| d > QUEUE_WITHIN) {
            failures.push(format!(
                "{:?} seat {}: queue not steady within {QUEUE_WITHIN:?}",
                leg.kind,
                leg.seat + 1
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
