//! NETPLAY-05F: causal host + two-client contact over the real process
//! boundary.
//!
//! Three `mm2` processes (the host and two joined clients) run one
//! dev-world session. Each client is driven only by its own scripted
//! input (`--ram` or `--parked`) and by what arrives through its own
//! seeded [`ImpairProxy`]: the two links have different delays and
//! different seeds, so the two timelines are skewed by the *network*
//! alone, never by the test. Every process writes its own car's
//! per-update trace (`--net-trace`), and the contact is measured from
//! those traces — the rammer's abrupt speed loss, the parked seats'
//! first motion — against the tick each process saw it on.
//!
//! The cases exchange roles (alice rams, bob rams) and put the host in
//! the contact (the host rams, so the clients only ever see it through
//! the wire). The clients ride the UDP lane through a seeded
//! [`UdpImpairProxy`]; the host forwards the finalized per-seat commands
//! as `RemoteCommands`. Asserted: the contact happened, nothing went
//! non-finite, the seats both clients hold agree within 1 m once the field
//! is at rest (a guaranteed contact), a displaced seat first moves within
//! `RESPONSE_TICKS` of the pursuer's contact and no own car is corrected by
//! more than `CORRECTION_PEAK_M` in a tick (ordinary links). The
//! high-latency timings and the scripted cases that may miss are reported
//! to `target/captures/netcode-contact/`. Not yet asserted: a separation
//! deadline, and deadlines counted from the first authoritative state
//! covering the contact rather than the pursuer's own contact tick.
//!
//! NETPLAY-05F-2 adds the cases that need the *other* car to act, through
//! `--net-contact <script>` (`evade`, `brake`, `swipe`, `push`; keyed on
//! the ticks since play began and the nearest car): a dodged predicted
//! contact on each link, a rear-end with sudden braking, a side swipe and
//! a sustained push. A scripted dodge, brake or swipe may legitimately
//! miss, so whether the contact happened is reported (`pursuer_contact_tick`
//! null = missed) and only the parked seats' agreement is bounded; the
//! push guarantees the hit and is bounded on every seat. The pusher lets go
//! and parks at 40 s (`input::PUSH_UNTIL`): a car held against another never
//! rests, and its predicted copy saw-tooths against the authority by up to
//! 3.3 m at 400 ms (#1969), so only a settled field has a convergence to
//! measure.
//!
//! NETPLAY-05F-3 adds the scene cases. `--net-contact wall` sends a car
//! east down the dev world's z = 0 line to its 1 m thick, 4 m high
//! perimeter wall at x = 200 and keeps the throttle down, so it ends
//! pinned. The grid seats stand on that line (x = 0, 4, 8), so the wall
//! pileup runs bob and alice as two wall runners nose-to-tail and the host
//! ramming the rear one: bob is pinned between the wall and alice, alice
//! between bob and the host, and all three cars are moving at the first
//! contact. The same pileup and the sustained push are repeated on the
//! `HIGH_LATENCY` links (250 ms and 400 ms each way) and reported next to
//! the ordinary ones. Not covered: a pin against an authored city wall (no
//! retail data here) and a pin against the dev world's scattered barriers,
//! which are 1 m tall and can be climbed.
//!
//! Evidence level: real OS processes over loopback, seeded lossy proxies,
//! headless, synthetic dev world. Nothing here says anything about a real
//! Internet path or how a collision feels.

use std::path::PathBuf;
use std::time::Duration;

use mm2_app::netprobe::{self, Sample};
use mm2_net::{Impair, LinkDir, UdpImpair, UdpImpairProxy};
use serde_json::{Value, json};

use crate::net_drive::{HOST_FRAME_CEILING, field, host_args, join_args, seat_poses, until_live};
use crate::support::Proc;

const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");
/// Each lifecycle step is waited on for an observed line (a loaded runner
/// is slow, not wrong).
const STEP: Duration = Duration::from_secs(60);
/// Backstop for the clients' run: they stop on the authority's applied
/// tick, so this only fires on a dead link or a hung host.
const DEADLINE: Duration = Duration::from_secs(150);
/// Simulated seconds at which the clients stop: the field is at rest on
/// the authority well before (see `net_drive::SHOVE_SETTLE_SECS`).
const SETTLE_SECS: u64 = 58;
/// A parked car moving faster than this (m/s) has been touched.
const MOVED_MS: f64 = 0.5;
/// First-response deadline: ticks from the pursuer's contact to a displaced
/// seat's first motion on its own process.
const RESPONSE_TICKS: u64 = 150;
/// Largest own-car position correction (m, one tick) tolerated after a
/// guaranteed contact on the ordinary links.
const CORRECTION_PEAK_M: f64 = 4.0;
/// Ticks skipped at the start of a trace: the spawn landing jostles.
const SETTLE_SKIP: usize = 120;

/// What one seat does for the case.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    /// Holds the handbrake (`--parked`).
    Parked,
    /// Pursues the nearest car (`--ram`).
    Ram,
    /// A scripted contact role (`--net-contact <script>`).
    Script(&'static str),
}

impl Role {
    fn flags(self) -> Vec<String> {
        match self {
            Role::Parked => vec!["--parked".into()],
            Role::Ram => vec!["--ram".into()],
            Role::Script(s) => vec!["--net-contact".into(), s.into()],
        }
    }
}

/// One contact case: a role per seat (host, alice, bob).
#[derive(Clone, Copy, Debug)]
struct Case {
    name: &'static str,
    roles: [Role; 3],
    /// The seat whose own trace is read for the contact (the pursuer).
    actor: usize,
    /// Whether the scripted behaviour guarantees a hit. A dodge may
    /// legitimately avoid the contact; that outcome is reported, not
    /// failed.
    contact_certain: bool,
    /// One-way delay (ms) of alice's and bob's links.
    delays: [u64; 2],
}

const HOST: usize = 0;
const ALICE: usize = 1;
const BOB: usize = 2;

/// The links every case but the high-latency ones runs on.
const UNEQUAL: [u64; 2] = [25, 90];
/// The ambiguous high-latency links: a transcontinental path (250 ms each
/// way) and a poor mobile one (400 ms each way), 0.5 s and 0.8 s round trips.
const HIGH_LATENCY: [u64; 2] = [250, 400];

const fn case(name: &'static str, roles: [Role; 3], actor: usize, contact_certain: bool) -> Case {
    Case {
        name,
        roles,
        actor,
        contact_certain,
        delays: UNEQUAL,
    }
}

const fn slow(mut c: Case) -> Case {
    c.delays = HIGH_LATENCY;
    c
}

/// One client's link: the same recipe family with a different delay and
/// seed, so the two seats see the session at unequal pings.
fn link(delay_ms: u64) -> UdpImpair {
    UdpImpair {
        link: Impair {
            delay: Duration::from_millis(delay_ms),
            jitter: Duration::from_millis(delay_ms / 4),
            loss: 0.02,
            duplicate: 0.02,
            reorder: 0.02,
        },
        ..UdpImpair::default()
    }
}

struct Run {
    records: [String; 2],
    traces: [Vec<Sample>; 3],
}

fn speed(s: &Sample) -> f64 {
    (s.vel[0] * s.vel[0] + s.vel[2] * s.vel[2]).sqrt()
}

/// First tick (after the spawn jostle) a car that had been parked moves.
fn first_motion(samples: &[Sample]) -> Option<u64> {
    samples
        .iter()
        .skip(SETTLE_SKIP)
        .find(|s| speed(s) > MOVED_MS)
        .map(|s| s.tick)
}

/// The rammer's contact: the first tick after it has been at speed whose
/// speed fell by `2 m/s` or more against three ticks earlier.
fn rammer_contact(samples: &[Sample]) -> Option<u64> {
    let mut fast = false;
    for w in samples.windows(4) {
        fast |= speed(&w[0]) > 5.0;
        if fast && speed(&w[0]) - speed(&w[3]) >= 2.0 {
            return Some(w[3].tick);
        }
    }
    None
}

/// Largest own-car position step beyond what its velocity explains, over
/// the ticks from `from` on — the correction the player would see.
fn correction_peak(samples: &[Sample], from: u64) -> f64 {
    samples
        .windows(2)
        .filter(|w| w[1].tick == w[0].tick + 1 && w[0].tick >= from && w[0].epoch == w[1].epoch)
        .map(|w| {
            let dt = 1.0 / mm2_game::RACE_TICK_HZ as f64;
            let predicted = [
                w[0].pos[0] + w[0].vel[0] * dt,
                w[0].pos[2] + w[0].vel[2] * dt,
            ];
            ((w[1].pos[0] - predicted[0]).powi(2) + (w[1].pos[2] - predicted[1]).powi(2)).sqrt()
        })
        .fold(0.0, f64::max)
}

fn run(case: Case, dir: &std::path::Path) -> Run {
    let install = tempfile::tempdir().unwrap();
    let trace = |who: &str| -> PathBuf { dir.join(format!("{}-{who}.jsonl", case.name)) };
    let common = |mut flags: Vec<String>, who: &str, me: usize| {
        flags.extend(case.roles[me].flags());
        flags.extend(
            [
                "--paced",
                "--netplay-timeline",
                "--net-trace",
                trace(who).to_str().unwrap(),
            ]
            .map(String::from),
        );
        flags
    };

    let mut host = Proc::spawn(
        MM2_EXE,
        &common(host_args(install.path(), HOST_FRAME_CEILING), "host", HOST),
    );
    let listening = host.until("listening=");
    let addr: std::net::SocketAddr = field(&listening, "listening").parse().unwrap();
    let lane_port: u16 = field(&listening, "lane_port").parse().unwrap();
    let lane_addr = std::net::SocketAddr::new(addr.ip(), lane_port);
    // Asymmetric paths (alice faster than bob), own seeds. The control
    // stream stays direct; the data plane is the UDP lane through the proxy.
    let proxy_alice = UdpImpairProxy::loopback_seeded(lane_addr, 0xC0A1).unwrap();
    let proxy_bob = UdpImpairProxy::loopback_seeded(lane_addr, 0xB0B2).unwrap();

    let settle_tick = SETTLE_SECS * mm2_game::RACE_TICK_HZ as u64;
    let client = |who: &str, me: usize, via: &UdpImpairProxy| {
        let mut flags = common(
            join_args(install.path(), addr, who, HOST_FRAME_CEILING),
            who,
            me,
        );
        let route = via.route();
        flags.extend([
            "--lane-via".to_string(),
            format!("{}/{}", route.dial, route.advertise_port),
        ]);
        flags.extend(["--until-snap-tick".to_string(), settle_tick.to_string()]);
        if who == "alice" {
            // Stay seated until bob leaves so bob's record holds all
            // three seats (see `net_drive::run_shove_trio`).
            flags.push("--hold-until-peer-left".into());
        }
        flags.extend(["--deadline".to_string(), DEADLINE.as_secs().to_string()]);
        flags
    };
    let alice = Proc::spawn(MM2_EXE, &client("alice", ALICE, &proxy_alice));
    host.until_within("ready=true", STEP);
    let bob = Proc::spawn(MM2_EXE, &client("bob", BOB, &proxy_bob));
    host.until_within("ready=true", STEP);
    host.cmd("start");
    host.until_within("event=started generation=1", STEP);
    // `Start` rides Down: let the clean lane deliver it, then arm.
    until_live(&alice);
    until_live(&bob);
    for (proxy, ms) in [(&proxy_alice, case.delays[0]), (&proxy_bob, case.delays[1])] {
        proxy.set(LinkDir::Up, link(ms));
        proxy.set(LinkDir::Down, link(ms));
    }

    let bound = DEADLINE + Duration::from_secs(60);
    let bob_rec = bob.until_within("smoke=headless-physics", bound);
    let alice_rec = alice.until_within("smoke=headless-physics", bound);
    alice.wait_success("alice", &alice_rec);
    bob.wait_success("bob", &bob_rec);
    for (who, rec) in [("alice", &alice_rec), ("bob", &bob_rec)] {
        assert_eq!(field(rec, "stop"), "snap-tick", "{who}: {rec}");
        for (k, v) in [("status", "pass"), ("phase", "playing"), ("finite", "true")] {
            assert_eq!(field(rec, k), v, "{who}: {rec}");
        }
    }
    for (name, p) in [("alice", &proxy_alice), ("bob", &proxy_bob)] {
        let (up, down) = (p.stats(LinkDir::Up).link, p.stats(LinkDir::Down).link);
        assert!(
            up.frames_in > 0 && down.frames_in > 0 && up.delayed > 0 && down.delayed > 0,
            "{name}'s link carried or held nothing: {up:?} {down:?}"
        );
    }
    // The relays hold socket clones: the host sees the clients leave
    // only once they are down.
    drop((proxy_alice, proxy_bob));
    for _ in 0..2 {
        host.until_within("event=left", STEP);
    }
    host.cmd("quit");
    let host_rec = host.until_within("smoke=headless-physics", STEP);
    assert_eq!(field(&host_rec, "status"), "pass", "{host_rec}");
    host.wait_success("host", &host_rec);

    let read = |who: &str| {
        let text = std::fs::read_to_string(trace(who)).unwrap_or_else(|e| panic!("{who}: {e}"));
        netprobe::parse_trace(&text)
            .unwrap_or_else(|e| panic!("{who}: {e}"))
            .samples
    };
    Run {
        records: [alice_rec, bob_rec],
        traces: [read("host"), read("alice"), read("bob")],
    }
}

fn report_dir() -> PathBuf {
    let dir = std::env::var_os("MM2_NET_REPORT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/captures/netcode-contact")
        });
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A tick whose wall-clock step is longer than this (ms) was starved of the
/// CPU: the 60 Hz step is 16.7 ms.
const STALL_MS: f64 = 100.0;
/// Starved ticks, in any one process, above which a run measured the
/// machine and not the netcode. Measured with eight busy loops on eight
/// cores (the three processes then share what is left): runs that met every
/// bound had 0-17 such ticks in each trace, runs that broke the 4 m own-car
/// correction bound or the 1 m convergence bound had 67-90. A stalled
/// process is a latency spike the netcode cannot be asked to hide, and the
/// own car's recovery snap is its lead times its speed, so the lead the
/// stalls add scales the correction with the load, not with the code.
const STARVED_TICKS: usize = 40;
/// Attempts at a case: a starved attempt that breaks a bound is repeated.
const ATTEMPTS: usize = 4;

/// Ticks of `samples` that took longer than [`STALL_MS`] of wall time.
fn stalled_ticks(samples: &[Sample]) -> usize {
    samples
        .windows(2)
        .filter(|w| w[1].wall_ms - w[0].wall_ms > STALL_MS)
        .count()
}

/// Whether any process of the run was starved, with the worst count.
fn starvation(run: &Run) -> (bool, usize) {
    let worst = run
        .traces
        .iter()
        .map(|t| stalled_ticks(t))
        .max()
        .unwrap_or(0);
    (worst > STARVED_TICKS, worst)
}

/// Run `case` and check its bounds. The bounds are never loosened: an
/// attempt that breaks one on a machine that starved its processes (see
/// [`STARVED_TICKS`]) is run again, and one that breaks one on a machine
/// that did not fails the test, as does the last attempt.
fn contact_case(case: Case) {
    let dir = report_dir();
    for attempt in 1..=ATTEMPTS {
        let run = run(case, &dir);
        let (starved, stalls) = starvation(&run);
        let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            check_case(case, &dir, &run)
        }));
        match checked {
            Ok(()) => return,
            Err(failure) if starved && attempt < ATTEMPTS => {
                eprintln!(
                    "{}: attempt {attempt} broke a bound while a process was starved \
                     ({stalls} ticks over {STALL_MS} ms): running it again",
                    case.name
                );
                drop(failure);
            }
            Err(failure) => std::panic::resume_unwind(failure),
        }
    }
}

/// Where a diverged seat's own process last moved it (#2103): the final
/// pose in each client's record, and on the seat's own trace the last tick
/// its car was more than 1 m from where it ended, with that tick's
/// authority tick and wall time, next to the trace's last tick and wall
/// time. A seat that kept moving until the very end of its run (its
/// neighbour's copy gone, say) shows a fork tick close to the last one.
fn fork_report(run: &Run, seat: u16, records: &[&String; 2]) -> String {
    let mut out = String::new();
    for (who, rec) in ["alice", "bob"].iter().zip(records) {
        let held = seat_poses(rec)
            .into_iter()
            .find(|(i, ..)| *i == seat)
            .map(|(_, x, z)| format!("({x:.1}, {z:.1})"));
        // The newest authority tick this process applied for the seat.
        let mark = rec
            .split_whitespace()
            .find_map(|t| t.strip_prefix("marks="))
            .and_then(|m| {
                m.split('/')
                    .find_map(|c| c.strip_prefix(&format!("{seat}:")))
            })
            .unwrap_or("none");
        out += &format!(
            "  {who}'s record holds seat {seat} at {}, last authority tick applied {mark}\n",
            held.unwrap_or_else(|| "nowhere".into())
        );
    }
    let Some(trace) = run.traces.get(usize::from(seat)) else {
        return out;
    };
    if let Some(last) = trace.last() {
        let fork = trace
            .iter()
            .rev()
            .find(|s| {
                ((s.pos[0] - last.pos[0]).powi(2) + (s.pos[2] - last.pos[2]).powi(2)).sqrt() > 1.0
            })
            .map(|s| (s.tick, s.snap_tick, s.wall_ms));
        out += &format!(
            "  seat {seat}'s own trace ends at tick {} ({:.0} ms), x = {:.1}; last more than 1 m from it at {fork:?} (tick, authority tick, wall ms)\n",
            last.tick, last.wall_ms, last.pos[0]
        );
    }
    out
}

fn check_case(case: Case, dir: &std::path::Path, run: &Run) {
    let seats = ["host", "alice", "bob"];
    let rammer = case.actor;
    for (who, t) in seats.iter().zip(&run.traces) {
        assert!(t.len() > 600, "{who}: only {} trace samples", t.len());
    }

    // The contact, as the pursuer itself saw it. A scripted dodge or
    // swipe may legitimately miss: that is a reported outcome, not a
    // failure.
    let contact = rammer_contact(&run.traces[rammer]);
    // A parked seat displaced by it (scripted seats move on their own).
    let moved: Vec<(&str, Option<u64>)> = seats
        .iter()
        .zip(&run.traces)
        .enumerate()
        .filter(|(i, _)| *i != rammer && case.roles[*i] == Role::Parked)
        .map(|(_, (who, t))| (*who, first_motion(t)))
        .collect();
    if case.contact_certain {
        assert!(
            contact.is_some(),
            "{}: the pursuer never hit anything",
            case.name
        );
        // (A case with no parked seat has nobody to displace: the scripted
        // victims are checked by the convergence bound below.)
        assert!(
            moved.is_empty() || moved.iter().any(|(_, m)| m.is_some()),
            "{}: no parked seat was ever displaced: {moved:?}",
            case.name
        );
    }
    let from = contact.unwrap_or(SETTLE_SKIP as u64);

    // Deadlines (#1766 AC1, AC2). Measured on the lane over unequal links:
    // a displaced seat's first motion fell within 79 ticks of the rammer's
    // own contact tick (the host, seeing a client's hit through its link)
    // and the own-car correction peaked at 1.9 m. The bounds are about
    // twice that, to be a deadline and not a flake; the
    // contact tick is the pursuer's own view, so this is not yet counted
    // from the first authoritative state covering the contact.
    if case.contact_certain && case.delays == UNEQUAL {
        let contact = contact.expect("asserted above");
        for (who, first) in &moved {
            if let Some(first) = first {
                assert!(
                    *first <= contact + RESPONSE_TICKS,
                    "{}: {who} first moved at tick {first}, {} ticks after the contact at {contact}",
                    case.name,
                    first - contact
                );
            }
        }
        for (who, t) in seats.iter().zip(&run.traces) {
            let peak = correction_peak(t, from);
            assert!(
                peak < CORRECTION_PEAK_M,
                "{}: {who}'s own car was corrected by {peak:.2} m in one tick",
                case.name
            );
        }
    }

    // Convergence once at rest: every seat both clients hold agrees.
    let (alice, bob) = (&run.records[0], &run.records[1]);
    let bob_seats = seat_poses(bob);
    assert_eq!(bob_seats.len(), 3, "{bob}");
    let mut compared = 0;
    let mut worst = (0, 0.0_f64);
    // The worst seat that is *meant* to be at rest: a scripted car that
    // is still driving when the clients stop (a dodge that never parks)
    // differs by the link's lag, not by a reconciliation failure, so it
    // is reported but not bounded unless the case guarantees the contact.
    let mut worst_bounded = (0, 0.0_f64);
    for (id, x, z) in seat_poses(alice) {
        let (_, bx, bz) = bob_seats
            .iter()
            .find(|(i, ..)| *i == id)
            .unwrap_or_else(|| panic!("bob lacks seat {id}: {bob}"));
        let apart = ((x - bx).powi(2) + (z - bz).powi(2)).sqrt();
        if apart > worst.1 {
            worst = (id, apart);
        }
        let bounded =
            case.contact_certain || case.roles.get(usize::from(id)) == Some(&Role::Parked);
        if bounded && apart > worst_bounded.1 {
            worst_bounded = (id, apart);
        }
        compared += 1;
    }
    assert!(compared >= 2, "{alice}");
    // A guaranteed contact converges to the shove tests' 1 m whoever is in
    // it: the host forwards the finalized commands (`RemoteCommands`) and
    // the clients replay the contact jointly over the datagram lane. (On
    // the legacy TCP snapshot pull two clients in the contact ended 1.2 m
    // and 3.9 m apart.) A scripted case that may miss keeps the loose bound
    // that only catches a car in another part of the map.
    let bound = if case.contact_certain { 1.0 } else { 10.0 };
    assert!(
        worst_bounded.1 < bound,
        "{}: seat {} diverged by {:.2} m:\n {alice}\n {bob}\n{}",
        case.name,
        worst_bounded.0,
        worst_bounded.1,
        fork_report(run, worst_bounded.0, &[alice, bob])
    );

    let per_seat: Vec<Value> = seats
        .iter()
        .zip(&run.traces)
        .enumerate()
        .map(|(i, (who, t))| {
            json!({
                "seat": who,
                // The rammer is moving by design: only a parked seat's
                // first motion is a displacement.
                "role": format!("{:?}", case.roles[i]),
                "first_motion_tick": if case.roles[i] == Role::Parked { first_motion(t) } else { None },
                "correction_peak_m_after_contact": correction_peak(t, from),
                // Where the seat's own process last saw it: the pinned
                // order against the wall (x = 200) in the pileup.
                "final_xz": t.last().map(|s| [s.pos[0], s.pos[2]]),
            })
        })
        .collect();
    let report = json!({
        "case": case.name,
        "transport": format!("UDP lane via per-client seeded UdpImpairProxy (alice {} ms, bob {} ms each way, 2 % loss/dup/reorder), three mm2 processes", case.delays[0], case.delays[1]),
        "pursuer_seat": seats[rammer],
        "pursuer_contact_tick": contact,
        "contact_certain": case.contact_certain,
        "max_client_pose_divergence_m": {"seat": worst.0, "metres": worst.1},
        "seats": per_seat,
        "asserted": "convergence < 1 m on guaranteed contacts; first response and correction peak bounded on the ordinary links; high-latency timings are reported only",
    });
    eprintln!("{report:#}");
    std::fs::write(
        dir.join(format!("{}.json", case.name)),
        serde_json::to_string_pretty(&report).unwrap(),
    )
    .unwrap();
}

#[test]
fn a_client_ramming_a_client_is_measured_on_unequal_links() {
    contact_case(case(
        "alice-rams",
        [Role::Parked, Role::Ram, Role::Parked],
        ALICE,
        true,
    ));
}

#[test]
fn the_roles_exchanged_are_measured_on_unequal_links() {
    contact_case(case(
        "bob-rams",
        [Role::Parked, Role::Parked, Role::Ram],
        BOB,
        true,
    ));
}

#[test]
fn the_host_in_the_contact_converges_on_unequal_links() {
    contact_case(case(
        "host-rams",
        [Role::Ram, Role::Parked, Role::Parked],
        HOST,
        true,
    ));
}

// Cases that need the other car to act. The pursuer is a client in each,
// so both links (25 ms and 90 ms) carry the contact; the roles are
// exchanged for the dodge to put the evader on the slow link too.

#[test]
fn a_dodged_contact_is_measured_with_the_evader_on_the_fast_link() {
    contact_case(case(
        "evade-alice-evades",
        [Role::Parked, Role::Script("evade"), Role::Ram],
        BOB,
        false,
    ));
}

#[test]
fn a_dodged_contact_is_measured_with_the_evader_on_the_slow_link() {
    contact_case(case(
        "evade-bob-evades",
        [Role::Parked, Role::Ram, Role::Script("evade")],
        ALICE,
        false,
    ));
}

#[test]
fn a_rear_end_with_sudden_braking_is_measured_on_unequal_links() {
    contact_case(case(
        "brake-bob-brakes",
        [Role::Parked, Role::Ram, Role::Script("brake")],
        ALICE,
        false,
    ));
}

#[test]
fn a_side_swipe_is_measured_on_unequal_links() {
    contact_case(case(
        "swipe-bob-swipes",
        [Role::Parked, Role::Parked, Role::Script("swipe")],
        BOB,
        false,
    ));
}

#[test]
fn a_sustained_push_is_measured_on_unequal_links() {
    contact_case(case(
        "push-alice-pushes",
        [Role::Parked, Role::Script("push"), Role::Parked],
        ALICE,
        true,
    ));
}

// Scene cases (NETPLAY-05F-3).

#[test]
fn a_three_car_pileup_against_a_wall_is_measured_on_unequal_links() {
    contact_case(case(
        "wall-pileup",
        [
            Role::Script("push"),
            Role::Script("wall"),
            Role::Script("wall"),
        ],
        HOST,
        true,
    ));
}

#[test]
fn a_three_car_pileup_against_a_wall_is_measured_on_high_latency_links() {
    contact_case(slow(case(
        "wall-pileup-high-latency",
        [
            Role::Script("push"),
            Role::Script("wall"),
            Role::Script("wall"),
        ],
        HOST,
        true,
    )));
}

#[test]
fn a_sustained_push_is_measured_on_high_latency_links() {
    contact_case(slow(case(
        "push-alice-pushes-high-latency",
        [Role::Parked, Role::Script("push"), Role::Parked],
        ALICE,
        true,
    )));
}
