//! `mm2-join` exercised as a separate OS process — the client side of
//! F24-B's headless lobby pair.
//!
//! These legs are the first evidence for AC01's "separate client
//! processes" leg: real `mm2-join` children dial a host, pick
//! vehicles, ready, observe `Start` and leave cleanly — distinct
//! processes on both sides, over loopback. LAN/Internet reachability
//! and the impairment matrix remain F24-C scope; gameplay replication
//! is F25/F26 — a started session is reported, not simulated. The
//! gate `mm2-join` consumes (`net::check_session`) has its own legs in
//! `net_check.rs`.

use std::net::SocketAddr;

use crate::support;

use mm2_app::net;
use mm2_game::{EventTableKind, SessionConfig, SessionMode, WorldMode};
use mm2_net::{Host, HostConfig, HostEvent};
use support::{Proc, WAIT, event, event_install, listening, mount};

const HOST_EXE: &str = env!("CARGO_BIN_EXE_mm2-host");
const JOIN_EXE: &str = env!("CARGO_BIN_EXE_mm2-join");

fn join_args(
    install: &std::path::Path,
    addr: SocketAddr,
    driver: &str,
    extra: &[&str],
) -> Vec<String> {
    let mut args = vec![
        "--mm2-path".to_string(),
        install.to_str().unwrap().to_string(),
        "--connect".to_string(),
        addr.to_string(),
        "--driver".to_string(),
        driver.to_string(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

/// AC01's missing leg: two separate `mm2-join` processes against a
/// separate `mm2-host` — each dials, picks a vehicle, readies, sees
/// the roster grow to two, observes `Start` and leaves cleanly.
/// Loopback, same install mount: LAN/Internet are F24-C.
#[test]
fn separate_client_processes_pick_ready_and_start() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(
        HOST_EXE,
        &[
            "--mm2-path".to_string(),
            install.path().to_str().unwrap().to_string(),
            "--dev-world".to_string(),
            "--bind".to_string(),
            "127.0.0.1:0".to_string(),
            "--seed".to_string(),
            7.to_string(),
        ],
    );
    let (addr, _fp, _) = listening(&host);

    let mut alice = Proc::spawn(
        JOIN_EXE,
        &join_args(install.path(), addr, "alice", &["--vehicle", "", "--ready"]),
    );
    let connected = alice.line();
    assert!(
        connected.starts_with("connected=") && connected.contains("driver=\"alice\""),
        "{connected}"
    );
    assert_eq!(alice.line(), "session=\"dev world, cruise, amateur\"");
    // pick roster, then ready roster — wire order.
    assert_eq!(alice.line(), "event=roster players=1 ready=0");
    alice.until("event=roster players=1 ready=1");
    let joined = host.line();
    assert!(
        joined.starts_with("event=joined id=1") && joined.contains("driver=\"alice\""),
        "{joined}"
    );
    assert_eq!(host.line(), "event=vehicle id=1 vehicle=\"\" paint=0");
    assert_eq!(host.line(), "event=ready id=1 ready=true");

    let mut bob = Proc::spawn(
        JOIN_EXE,
        &join_args(install.path(), addr, "bob", &["--vehicle", "", "--ready"]),
    );
    assert!(bob.line().starts_with("connected="));
    assert_eq!(bob.line(), "session=\"dev world, cruise, amateur\"");
    bob.until("event=roster players=2 ready=1");
    let joined = host.line();
    assert!(
        joined.starts_with("event=joined id=2") && joined.contains("driver=\"bob\""),
        "{joined}"
    );
    assert_eq!(host.line(), "event=vehicle id=2 vehicle=\"\" paint=0");
    assert_eq!(host.line(), "event=ready id=2 ready=true");
    alice.until("event=roster players=2 ready=2");

    host.cmd("start");
    assert_eq!(host.line(), "event=started generation=1");
    for client in [&alice, &bob] {
        assert_eq!(
            client.until("event=started"),
            "event=started generation=1 session=\"dev world, cruise, amateur\""
        );
    }

    // A clean leave per client: `quit` sends Leave, the host records
    // the quit and the client exits 0.
    alice.cmd("quit");
    assert!(alice.wait().success(), "alice did not exit cleanly");
    assert_eq!(host.line(), "event=left id=1 driver=\"alice\" cause=quit");
    bob.cmd("quit");
    assert!(bob.wait().success(), "bob did not exit cleanly");
    assert_eq!(host.line(), "event=left id=2 driver=\"bob\" cause=quit");

    host.cmd("quit");
    assert!(host.wait().success(), "mm2-host did not exit cleanly");
}

/// A client mounting different content than the host fails the
/// gameplay-fingerprint handshake: `join_failed`, exit 1, and the host
/// logs the refusal — a usable error on both ends (AC02/AC04 legs).
#[test]
fn a_client_process_reports_a_refused_join() {
    let install = event_install();
    let mut host = Proc::spawn(
        HOST_EXE,
        &[
            "--mm2-path".to_string(),
            install.path().to_str().unwrap().to_string(),
            "--city".to_string(),
            "testcity".to_string(),
            "--bind".to_string(),
            "127.0.0.1:0".to_string(),
        ],
    );
    let (addr, _fp, _) = listening(&host);

    // A different mount → a different gameplay fingerprint → the
    // handshake refuses before any lobby traffic.
    let other = tempfile::tempdir().unwrap();
    let bob = Proc::spawn(JOIN_EXE, &join_args(other.path(), addr, "bob", &[]));
    let refused = bob.line();
    assert!(
        refused.starts_with("event=join_failed") && refused.contains("content"),
        "{refused}"
    );
    assert_eq!(bob.wait().code(), Some(1));
    let logged = host.line();
    assert!(
        logged.starts_with("event=join_failed") && logged.contains("content"),
        "{logged}"
    );

    host.cmd("quit");
    assert!(host.wait().success(), "mm2-host did not exit cleanly");
}

/// The join-side content gate in a real process: an in-process `Host`
/// advertises a session the client's install cannot run (a content
/// check the fingerprint handshake cannot express — both sides mount
/// the same empty install, so fingerprints match while the
/// advertisement names absent content). The client reports
/// `session_refused`, leaves cleanly and exits 1; the host sees the
/// join then the clean `quit` cause.
#[test]
fn a_client_process_refuses_a_session_it_cannot_run() {
    let install = tempfile::tempdir().unwrap();
    let vfs = mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;

    for (config, needle) in [
        // An event ad on a dev world: the client's install has no
        // testcity tables to resolve it against.
        (
            SessionConfig {
                world: WorldMode::DevWorld,
                mode: SessionMode::Event(event(EventTableKind::Checkpoint, 0)),
                ..SessionConfig::default()
            },
            "cannot run here",
        ),
        // A city world that resolves to nothing on the client mount.
        (
            SessionConfig {
                world: WorldMode::City {
                    psdl: "city/nothere.psdl".to_string(),
                },
                mode: SessionMode::Cruise,
                ..SessionConfig::default()
            },
            "does not resolve",
        ),
    ] {
        let mut host = Host::listen_loopback(&HostConfig::new(fp)).unwrap();
        host.set_session(net::advertise(&config).unwrap()).unwrap();

        let checker = Proc::spawn(
            JOIN_EXE,
            &join_args(install.path(), host.addr(), "checker", &[]),
        );
        assert!(checker.line().starts_with("connected="));
        assert!(checker.line().starts_with("session="));
        let refused = checker.line();
        assert!(
            refused.starts_with("event=session_refused") && refused.contains(needle),
            "{refused}"
        );
        assert_eq!(checker.wait().code(), Some(1));

        // The refusal left the lobby cleanly — the host saw the join
        // and then a `quit` leave, not a dropped socket.
        let joined = host.recv_timeout(WAIT).unwrap();
        assert!(matches!(joined, HostEvent::Joined { .. }), "{joined:?}");
        let left = host.recv_timeout(WAIT).unwrap();
        assert!(
            matches!(left, HostEvent::Left { cause, .. } if cause == mm2_net::LeaveCause::Quit),
            "{left:?}"
        );
        host.shutdown();
    }
}

/// The positive leg of the gate: `mm2-host` advertises the authored
/// `testcity` checkpoint event, a `mm2-join` on the same install
/// validates it through `net::check_session`, readies and observes
/// `Start` carrying the same event summary.
#[test]
fn a_client_process_accepts_a_runnable_event() {
    let install = event_install();
    let mut host = Proc::spawn(
        HOST_EXE,
        &[
            "--mm2-path".to_string(),
            install.path().to_str().unwrap().to_string(),
            "--city".to_string(),
            "testcity".to_string(),
            "--event".to_string(),
            "race:0".to_string(),
            "--bind".to_string(),
            "127.0.0.1:0".to_string(),
            "--seed".to_string(),
            13.to_string(),
        ],
    );
    let (addr, _fp, _) = listening(&host);

    let mut alice = Proc::spawn(
        JOIN_EXE,
        &join_args(install.path(), addr, "alice", &["--vehicle", "", "--ready"]),
    );
    assert!(alice.line().starts_with("connected="));
    assert_eq!(alice.line(), "session=\"testcity, race:0, amateur\"");
    alice.until("event=roster players=1 ready=1");
    host.until("event=ready id=1 ready=true");

    host.cmd("start");
    assert_eq!(host.line(), "event=started generation=1");
    assert_eq!(
        alice.until("event=started"),
        "event=started generation=1 session=\"testcity, race:0, amateur\""
    );

    alice.cmd("quit");
    assert!(alice.wait().success(), "alice did not exit cleanly");
    host.cmd("quit");
    assert!(host.wait().success(), "mm2-host did not exit cleanly");
}

/// F26-AC05's changed rematch at process level, the refusing side: a
/// `mm2-join` process that accepted round one's ad hears the host
/// re-advertise a session its own mount cannot run (a city that
/// resolves to nothing). The same gate as the first ad applies —
/// `session_refused`, a clean `quit` leave seen by the host, exit 1 —
/// and the process does not sit in a lobby it cannot play. The host is
/// the in-process `mm2_net::Host`, standing in for a host whose install
/// resolves a city this client's does not (an `mm2` host would refuse
/// to advertise it); the client is a separate OS process.
#[test]
fn a_client_process_leaves_cleanly_when_the_changed_session_is_unrunnable() {
    let install = tempfile::tempdir().unwrap();
    let vfs = mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let mut host = Host::listen_loopback(&HostConfig::new(fp)).unwrap();
    host.set_session(net::advertise(&SessionConfig::default()).unwrap())
        .unwrap();

    let client = Proc::spawn(
        JOIN_EXE,
        &join_args(install.path(), host.addr(), "alice", &[]),
    );
    assert!(client.line().starts_with("connected="));
    let first = client.line();
    assert!(first.starts_with("session="), "{first}");
    let joined = host.recv_timeout(WAIT).unwrap();
    assert!(matches!(joined, HostEvent::Joined { .. }), "{joined:?}");

    host.set_session(
        net::advertise(&SessionConfig {
            world: WorldMode::City {
                psdl: "city/nothere.psdl".to_string(),
            },
            mode: SessionMode::Cruise,
            ..SessionConfig::default()
        })
        .unwrap(),
    )
    .unwrap();

    client.until("event=session_refused");
    assert_eq!(client.wait().code(), Some(1));
    let left = host.recv_timeout(WAIT).unwrap();
    assert!(
        matches!(left, HostEvent::Left { cause, .. } if cause == mm2_net::LeaveCause::Quit),
        "{left:?}"
    );
    host.shutdown();
}
