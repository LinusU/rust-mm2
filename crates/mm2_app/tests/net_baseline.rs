//! NETPLAY-01: the continuous-driving baseline harness.
//!
//! A real host plus two independently driven clients (three `mm2`
//! processes over loopback UDP, no impairment proxy) each drive a
//! scripted input route through the production vehicle/network schedule
//! and write a per-update trajectory (`--net-trace`). [`netprobe`]
//! reduces each trace to continuous-motion metrics; the report lands as
//! JSON under `MM2_NET_REPORT_DIR` (default `target/captures/netcode-baseline`).
//!
//! Two modes, both explicit:
//! * **baseline/report** (default): the run is measured and reported, the
//!   only assertions are that the harness itself worked (samples exist,
//!   the session ran, the trace is consecutive). Current production code
//!   is known not to meet the future quality targets, so quality is not
//!   asserted here.
//! * **strict** (`MM2_NET_STRICT=1`): every clean scenario must also pass
//!   [`Limits::STRICT`]. NETPLAY-09 turns this into the regression gate.
//!
//! The fault legs run unconditionally: a deliberate defect injected into
//! the real own-seat correction path (`--net-fault`) must be flagged by
//! the checker, which is what makes the checker evidence rather than a
//! formula.
//!
//! Scripts supply inputs only. Waits are bounded by the clients' own
//! frame budgets; every child is a `Proc`, killed by its own PID on drop.
//!
//! Evidence level: real OS processes over real loopback sockets, headless
//! (no render, no GPU), dev-world cruise. Not a real-network or
//! human-feel result.

use std::path::{Path, PathBuf};

use crate::net_drive::{
    HOST_FRAME_CEILING, host_args, join_args, listening_addr, start_when_ready,
};
use crate::support::{Proc, drive_bound};
use mm2_app::netprobe::{self, Limits, Metrics};
use serde_json::{Value, json};

const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");
const FRAMES: u32 = 2400;
const SEED: u64 = 11;

struct Scenario<'a> {
    name: &'static str,
    /// What differs from the clean run, for the report.
    about: &'static str,
    host: &'a [&'a str],
    alice: &'a [&'a str],
    bob: &'a [&'a str],
}

fn report_dir() -> PathBuf {
    let dir = std::env::var_os("MM2_NET_REPORT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/netcode-baseline")
        });
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn strict() -> bool {
    std::env::var_os("MM2_NET_STRICT").is_some()
}

struct Outcome {
    metrics: Vec<(&'static str, Metrics)>,
    report: Value,
}

fn run(s: &Scenario<'_>) -> Outcome {
    let dir = report_dir();
    let install = tempfile::tempdir().unwrap();
    let trace = |seat: &str| dir.join(format!("{}-{seat}.jsonl", s.name));
    let with = |mut args: Vec<String>, seat: &str, route: &str, extra: &[&str]| {
        args.extend(
            ["--net-route", route, "--net-trace"]
                .map(String::from)
                .into_iter()
                .chain([trace(seat).to_str().unwrap().to_string()])
                .chain(extra.iter().map(|a| a.to_string())),
        );
        args
    };
    let mut host = Proc::spawn(
        MM2_EXE,
        &with(
            host_args(install.path(), HOST_FRAME_CEILING),
            "host",
            "accel",
            s.host,
        ),
    );
    let addr = listening_addr(&host);
    let alice = Proc::spawn(
        MM2_EXE,
        &with(
            join_args(install.path(), addr, "alice", FRAMES),
            "alice",
            "slalom",
            s.alice,
        ),
    );
    let bob = Proc::spawn(
        MM2_EXE,
        &with(
            join_args(install.path(), addr, "bob", FRAMES),
            "bob",
            "accel",
            s.bob,
        ),
    );
    start_when_ready(&mut host, 2);
    let bound = drive_bound(FRAMES);
    let alice_rec = alice.until_within("smoke=headless-physics", bound);
    let bob_rec = bob.until_within("smoke=headless-physics", bound);
    alice.wait_success("alice", &alice_rec);
    bob.wait_success("bob", &bob_rec);
    host.cmd("quit");
    let host_rec = host.until_within("smoke=headless-physics", std::time::Duration::from_secs(60));
    assert!(
        host.wait().success(),
        "host did not exit cleanly: {host_rec}"
    );

    let mut metrics = Vec::new();
    let mut seats = serde_json::Map::new();
    for seat in ["host", "alice", "bob"] {
        let text =
            std::fs::read_to_string(trace(seat)).unwrap_or_else(|e| panic!("{seat} trace: {e}"));
        let t = netprobe::parse_trace(&text).unwrap_or_else(|e| panic!("{seat}: {e}"));
        let m = netprobe::analyze(&t);
        assert!(
            m.pairs > 600,
            "{}/{seat}: only {} consecutive pairs in the trace",
            s.name,
            m.pairs
        );
        seats.insert(seat.into(), m.to_json());
        metrics.push((seat, m));
    }
    let report = json!({
        "scenario": s.name, "about": s.about,
        "transport": "loopback UDP 127.0.0.1, three mm2 processes, no impairment proxy",
        "seed": SEED, "client_frame_cap": FRAMES, "world": "dev-world cruise, headless",
        "args": { "host": s.host, "alice": s.alice, "bob": s.bob },
        "routes": { "host": "accel", "alice": "slalom", "bob": "accel" },
        "seats": seats,
        "unobserved": "owner-session RTT, machine setup and logs; snapshot age, ack, buffer occupancy, replay and history-miss fields do not exist in this protocol version",
    });
    std::fs::write(
        dir.join(format!("{}.json", s.name)),
        serde_json::to_string_pretty(&report).unwrap(),
    )
    .unwrap();
    for (seat, m) in &metrics {
        eprintln!(
            "{:>10} {:>5}: pairs={} max_resid={:.2}m >0.25m/min={:.1} cum={:.1}m/min yaw_flips={} logged={:?}",
            s.name,
            seat,
            m.pairs,
            m.residual_all.max,
            m.over_per_min(1),
            m.cumulative_per_min,
            m.yaw_flips,
            m.logged
        );
    }
    Outcome { metrics, report }
}

fn seat<'a>(o: &'a Outcome, who: &str) -> &'a Metrics {
    &o.metrics.iter().find(|(s, _)| *s == who).unwrap().1
}

fn baseline(s: &Scenario<'_>) {
    let o = run(s);
    if strict() {
        for (who, m) in &o.metrics {
            let bad = netprobe::check(m, &Limits::STRICT);
            assert!(bad.is_empty(), "{}/{who}: {bad:?}", s.name);
        }
    }
    let _ = o.report;
}

#[test]
fn baseline_clean_loopback() {
    baseline(&Scenario {
        name: "clean",
        about: "all processes paced to wall-clock 60 Hz, no stalls",
        host: &["--paced"],
        alice: &["--paced"],
        bob: &["--paced"],
    });
}

#[test]
fn baseline_unequal_frame_pacing() {
    baseline(&Scenario {
        name: "unequal-pacing",
        about: "host unpaced (steps as fast as the CPU allows) while both clients hold 60 Hz",
        host: &[],
        alice: &["--paced"],
        bob: &["--paced"],
    });
}

#[test]
fn baseline_client_stalls() {
    baseline(&Scenario {
        name: "client-stall",
        about: "alice stalls her whole process 100 ms every 120 updates",
        host: &["--paced"],
        alice: &["--paced", "--net-stall", "120:100"],
        bob: &["--paced"],
    });
}

#[test]
fn baseline_car_contact() {
    baseline(&Scenario {
        name: "contact",
        about: "alice rams the nearest car (--ram) instead of the slalom route",
        host: &["--paced"],
        alice: &["--paced", "--ram"],
        bob: &["--paced"],
    });
}

/// A deliberate defect through the real own-seat correction path must be
/// flagged, and the same scenario without it must be worse-by-construction
/// than the fault: the fault run logs its injections and the checker
/// names the breach.
fn fault_leg(name: &'static str, flag: &'static str, needle: &str) {
    let o = run(&Scenario {
        name,
        about: "alice's own-seat correction path carries a deliberate fault",
        host: &["--paced"],
        alice: &["--paced", "--net-fault", flag],
        bob: &["--paced"],
    });
    let alice = seat(&o, "alice");
    let injected = alice
        .logged
        .iter()
        .find(|(k, _)| k == "fault")
        .map_or(0, |(_, c)| *c);
    assert!(
        injected >= 10,
        "{name}: the fault never reached the correction path ({injected})"
    );
    let bad = netprobe::check(alice, &Limits::STRICT);
    assert!(
        bad.iter().any(|b| b.contains(needle)),
        "{name}: the checker missed the fault: {bad:?} {alice:?}"
    );
}

#[test]
fn the_checker_flags_injected_repeated_small_corrections() {
    fault_leg("fault-kicks", "kicks", "corrections/min");
}

#[test]
fn the_checker_flags_injected_alternating_yaw_kicks() {
    fault_leg("fault-yaw", "yaw", "yaw flips");
}
