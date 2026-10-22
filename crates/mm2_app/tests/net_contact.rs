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
//! the wire). What is asserted is what the production path promises
//! today: the contact happened, nothing went non-finite, and the seats
//! both clients hold agree once the field is at rest. The per-process
//! timings (first-motion tick, ticks from the rammer's contact to the
//! victim's motion, the own-car correction peak after contact) are
//! *reported* to `target/captures/netcode-contact/` for #1766's
//! deadlines, not asserted: the app does not yet carry the remote-input
//! stream or the joint replay on the wire (RemoteCommands are not sent by
//! the host; the client is still on the TCP data plane until NETPLAY-06E-4
//! moves it to the datagram lane), so a deadline here would pin the
//! legacy snapshot-pull behaviour rather than the redesign.
//!
//! NETPLAY-05F-2 adds the cases that need the *other* car to act, through
//! `--net-contact <script>` (`evade`, `brake`, `swipe`, `push`; keyed on
//! the ticks since play began and the nearest car): a dodged predicted
//! contact on each link, a rear-end with sudden braking, a side swipe and
//! a sustained push. A scripted dodge, brake or swipe may legitimately
//! miss, so whether the contact happened is reported (`pursuer_contact_tick`
//! null = missed) and only the parked seats' agreement is bounded; the
//! push guarantees the hit and is bounded on every seat.
//!
//! Not covered, and filed as a follow-up rather than claimed: the
//! authored-scene cases (pin against a wall, three-car pileup), which need
//! a scene with a wall and a third moving car, and the ambiguous
//! high-latency report.
//!
//! Evidence level: real OS processes over loopback, seeded lossy proxies,
//! headless, synthetic dev world. Nothing here says anything about a real
//! Internet path or how a collision feels.

use std::path::PathBuf;
use std::time::Duration;

use mm2_app::netprobe::{self, Sample};
use mm2_net::{Impair, ImpairProxy, LinkDir};
use serde_json::{Value, json};

use crate::net_drive::{
    HOST_FRAME_CEILING, field, host_args, join_args, listening_addr, seat_poses, until_live,
};
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
}

const HOST: usize = 0;
const ALICE: usize = 1;
const BOB: usize = 2;

const fn case(name: &'static str, roles: [Role; 3], actor: usize, contact_certain: bool) -> Case {
    Case {
        name,
        roles,
        actor,
        contact_certain,
    }
}

/// One client's link: the same recipe family with a different delay and
/// seed, so the two seats see the session at unequal pings.
fn link(delay_ms: u64) -> Impair {
    Impair {
        delay: Duration::from_millis(delay_ms),
        jitter: Duration::from_millis(delay_ms / 4),
        loss: 0.02,
        duplicate: 0.02,
        reorder: 0.02,
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
    let addr = listening_addr(&host);
    // Asymmetric paths: alice 25 ms each way, bob 90 ms, own seeds.
    let proxy_alice = ImpairProxy::loopback_seeded(addr, 0xC0A1).unwrap();
    let proxy_bob = ImpairProxy::loopback_seeded(addr, 0xB0B2).unwrap();

    let settle_tick = SETTLE_SECS * mm2_game::RACE_TICK_HZ as u64;
    let client = |who: &str, me: usize, via: &ImpairProxy| {
        let mut flags = common(
            join_args(install.path(), via.addr(), who, HOST_FRAME_CEILING),
            who,
            me,
        );
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
    for (proxy, ms) in [(&proxy_alice, 25), (&proxy_bob, 90)] {
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
        let (up, down) = (p.stats(LinkDir::Up), p.stats(LinkDir::Down));
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

fn contact_case(case: Case) {
    let dir = report_dir();
    let run = run(case, &dir);
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
        assert!(
            moved.iter().any(|(_, m)| m.is_some()),
            "{}: no parked seat was ever displaced: {moved:?}",
            case.name
        );
    }
    let from = contact.unwrap_or(SETTLE_SKIP as u64);

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
    // With the host in the contact the legacy path converges (the
    // shove tests' bound). With two clients in it the rammer's local sim
    // and the victim's predicted car resolve the hit separately and the
    // snapshot pull only partly reconciles them: measured at 1.2 m and
    // 3.9 m in two runs. That is exactly what NETPLAY-05 exists to fix, so
    // it is reported, and only a car in another part of the map fails.
    let bound = if rammer == HOST { 1.0 } else { 10.0 };
    assert!(
        worst_bounded.1 < bound,
        "{}: seat {} diverged by {:.2} m:\n {alice}\n {bob}",
        case.name,
        worst_bounded.0,
        worst_bounded.1
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
            })
        })
        .collect();
    let report = json!({
        "case": case.name,
        "transport": "TCP data plane via per-client seeded ImpairProxy (alice 25 ms, bob 90 ms each way, 2 % loss/dup/reorder), three mm2 processes",
        "pursuer_seat": seats[rammer],
        "pursuer_contact_tick": contact,
        "contact_certain": case.contact_certain,
        "max_client_pose_divergence_m": {"seat": worst.0, "metres": worst.1},
        "seats": per_seat,
        "not_asserted": "timings are reported for NETPLAY-05 deadlines; the app does not yet send RemoteCommands or use the datagram lane",
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
