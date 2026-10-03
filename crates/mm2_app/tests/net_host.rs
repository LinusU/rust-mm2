//! F24-B multi-process leg: `mm2-host` runs as a *separate process* and
//! serves a real advertised lobby to `mm2_net::Client`s over loopback —
//! AC01's separate-host-process leg and AC05's headless binary (it never
//! opens a window, GPU or audio device). Client processes, LAN and the
//! impairment matrix remain F24-C scope.

use std::net::SocketAddr;
use std::process::Command;
use std::time::Duration;

use crate::support;

use mm2_app::net;
use mm2_game::{EventRef, EventTableKind, SessionAuthority, SessionMode, WorldMode};
use mm2_net::{Client, Message, NetError, RejectCode, hello};
use support::{Proc, WAIT, event_install, listening};

const HOST_EXE: &str = env!("CARGO_BIN_EXE_mm2-host");

type HostProc = Proc;

fn join(addr: SocketAddr, driver: &str, fp: u64) -> Client {
    let client = Client::join(addr, &hello("test".to_string(), driver.to_string(), fp)).unwrap();
    client.set_timeout(Some(WAIT)).unwrap();
    client
}

/// Spawn an `mm2-host` on a content-free dev world and parse the
/// `listening=` record into (address, fingerprint) — the two facts a
/// join needs. The install dir is returned so it outlives the child.
fn dev_host(seed: u64) -> (HostProc, SocketAddr, u64, tempfile::TempDir) {
    // An empty install dir is a valid (if content-free) mount — the
    // gameplay fingerprint still gates deterministically.
    let dir = tempfile::tempdir().unwrap();
    let host = HostProc::spawn(
        HOST_EXE,
        &[
            "--mm2-path".to_string(),
            dir.path().to_str().unwrap().to_string(),
            "--dev-world".to_string(),
            "--bind".to_string(),
            "127.0.0.1:0".to_string(),
            "--seed".to_string(),
            seed.to_string(),
        ],
    );

    let (addr, fp, _) = listening(&host);
    (host, addr, fp, dir)
}

#[test]
fn a_dedicated_host_process_serves_an_advertised_lobby() {
    let (host, addr, fp, _install) = dev_host(7);

    // The advertised session arrives before the roster and decodes back
    // into the configuration the host was launched with.
    let mut alice = join(addr, "alice", fp);
    match alice.recv().unwrap() {
        Message::Session(ad) => {
            assert_eq!(ad.summary, "dev world, cruise, amateur");
            let config = net::accept(&ad).unwrap();
            assert_eq!(config.world, WorldMode::DevWorld);
            assert_eq!(config.mode, SessionMode::Cruise);
            assert_eq!(config.seed, 7);
            assert_eq!(config.authority, SessionAuthority::Remote);
        }
        other => panic!("expected Session, got {other:?}"),
    }
    match alice.recv().unwrap() {
        Message::Roster { players } => assert_eq!(players.len(), 1),
        other => panic!("expected Roster, got {other:?}"),
    }

    // A second client joins; the host process reports both joins and
    // the ready toggle as records.
    let mut bob = join(addr, "bob", fp);
    let first_join = host.line();
    let second_join = host.line();
    assert!(first_join.starts_with("event=joined id=1"), "{first_join}");
    assert!(
        second_join.starts_with("event=joined id=2"),
        "{second_join}"
    );
    match bob.recv().unwrap() {
        Message::Session(_) => {}
        other => panic!("expected Session, got {other:?}"),
    }
    match bob.recv().unwrap() {
        Message::Roster { players } => assert_eq!(players.len(), 2),
        other => panic!("expected Roster, got {other:?}"),
    }
    alice.set_ready(true).unwrap();
    assert_eq!(host.line(), "event=ready id=1 ready=true");

    // Vehicle picks run through the host's catalog validator: on an
    // empty install only the dev car (empty wire id) is legal.
    alice.set_vehicle("", 0).unwrap();
    assert_eq!(host.line(), "event=vehicle id=1 vehicle=\"\" paint=0");
    // The pick rides the roster rebroadcast to every peer — bob sees
    // alice's dev car on the snapshot.
    let pick_seen = (0..8).any(|_| match bob.recv().unwrap() {
        Message::Roster { players } => players.iter().any(|p| {
            p.pick
                == Some(mm2_net::VehiclePick {
                    vehicle: String::new(),
                    paint: 0,
                })
        }),
        _ => false,
    });
    assert!(pick_seen, "bob never saw alice's pick on the roster");

    // An out-of-catalog id is refused — to bob alone, roster unchanged.
    bob.set_vehicle("vpbug", 0).unwrap();
    match bob.recv().unwrap() {
        Message::VehicleRefused { reason } => {
            assert!(reason.contains("unknown vehicle"), "got {reason}")
        }
        other => panic!("expected VehicleRefused, got {other:?}"),
    }
    let refused = host.line();
    assert!(refused.starts_with("event=pick_refused id=2"), "{refused}");

    // A mismatched gameplay fingerprint is refused at the door — AC02's
    // gate, here against a separate host process.
    let err = Client::join(
        addr,
        &hello("test".to_string(), "mallory".to_string(), fp + 1),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        NetError::Rejected {
            code: RejectCode::ContentMismatch,
            ..
        }
    ));
    let refused = host.line();
    assert!(refused.starts_with("event=join_failed"), "{refused}");

    // A clean quit reaches the host as `quit`, not a socket drop.
    alice.leave().unwrap();
    let left = host.line();
    assert_eq!(left, "event=left id=1 driver=\"alice\" cause=quit");
}

/// The session lifecycle leg against the separate host process: stdin
/// drives `start`/`cancel`, the gate refuses early, an open (MP-5
/// cruise) session admits a mid-session joiner with the running
/// `Start`, and `cancel` re-opens the lobby for a second generation.
#[test]
fn a_dedicated_host_process_runs_start_and_cancel() {
    let (mut host, addr, fp, _install) = dev_host(9);

    let mut alice = join(addr, "alice", fp);
    match alice.recv().unwrap() {
        Message::Session(_) => {}
        other => panic!("expected Session, got {other:?}"),
    }
    match alice.recv().unwrap() {
        Message::Roster { players } => assert_eq!(players.len(), 1),
        other => panic!("expected Roster, got {other:?}"),
    }
    assert_eq!(
        host.line(),
        "event=joined id=1 driver=\"alice\" build=\"test\""
    );

    // The gate names the blocker — a start before the ready flag is
    // refused, not silently accepted.
    host.cmd("start");
    assert_eq!(
        host.line(),
        "event=start_refused reason=\"alice is not ready\""
    );

    alice.set_vehicle("", 0).unwrap();
    assert_eq!(host.line(), "event=vehicle id=1 vehicle=\"\" paint=0");
    alice.set_ready(true).unwrap();
    assert_eq!(host.line(), "event=ready id=1 ready=true");

    // This time the start goes through: the record reports generation 1
    // and `Start` carries the running session self-contained.
    host.cmd("start");
    assert_eq!(host.line(), "event=started generation=1");
    loop {
        match alice.recv().unwrap() {
            Message::Start {
                generation,
                session,
                host_pick,
            } => {
                assert!(
                    host_pick.is_none(),
                    "a dedicated mm2-host has no player seat"
                );
                assert_eq!(generation, 1);
                let config = net::accept(&session).unwrap();
                assert_eq!(config.world, WorldMode::DevWorld);
                assert_eq!(config.seed, 9);
                break;
            }
            Message::Roster { .. } => continue,
            other => panic!("expected Start, got {other:?}"),
        }
    }

    // mm2-host advertises cruise only — MP-5's open rule: a mid-session
    // joiner is admitted and told the running session's Start after its
    // first roster.
    let mut bob = join(addr, "bob", fp);
    assert_eq!(
        host.line(),
        "event=joined id=2 driver=\"bob\" build=\"test\""
    );
    match bob.recv().unwrap() {
        Message::Session(_) => {}
        other => panic!("expected Session, got {other:?}"),
    }
    loop {
        match bob.recv().unwrap() {
            Message::Start { generation, .. } => {
                assert_eq!(generation, 1);
                break;
            }
            Message::Roster { .. } => continue,
            other => panic!("expected Start, got {other:?}"),
        }
    }

    // Cancel returns everyone to the lobby; readiness reset is visible
    // on the rebroadcast roster. alice may still have bob's join roster
    // queued ahead of the Cancel — skip roster traffic to it.
    host.cmd("cancel");
    assert_eq!(host.line(), "event=cancelled generation=1");
    for client in [&mut alice, &mut bob] {
        loop {
            match client.recv().unwrap() {
                Message::Cancel { generation: 1 } => break,
                Message::Roster { .. } => continue,
                other => panic!("expected Cancel, got {other:?}"),
            }
        }
        match client.recv().unwrap() {
            Message::Roster { players } => {
                assert!(players.iter().all(|p| !p.ready));
            }
            other => panic!("expected Roster, got {other:?}"),
        }
    }

    // A second session is a fresh generation. bob picks and readies;
    // alice's pick survived the cancel so she needs only ready.
    bob.set_vehicle("", 0).unwrap();
    assert_eq!(host.line(), "event=vehicle id=2 vehicle=\"\" paint=0");
    bob.set_ready(true).unwrap();
    assert_eq!(host.line(), "event=ready id=2 ready=true");
    alice.set_ready(true).unwrap();
    assert_eq!(host.line(), "event=ready id=1 ready=true");
    host.cmd("start");
    assert_eq!(host.line(), "event=started generation=2");

    // `quit` is a clean shutdown — the process exits on its own.
    host.cmd("quit");
    assert!(host.wait().success(), "mm2-host did not exit cleanly");
}

/// The event leg against the separate host process: `mm2-host --event`
/// advertises an authored event and `start` applies MP-5's race rule —
/// joins close for the session's duration and `cancel` re-opens them.
#[test]
fn an_event_host_closes_joins_at_start() {
    let install = event_install();
    let mut host = HostProc::spawn(
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
    let (addr, fp, first) = listening(&host);
    assert!(
        first.contains("session=\"testcity, race:0, amateur\""),
        "{first}"
    );

    // The advertised session names the authored event, decodable back
    // into the same SessionConfig shape a local `--event` run builds.
    let mut alice = join(addr, "alice", fp);
    match alice.recv().unwrap() {
        Message::Session(ad) => {
            let config = net::accept(&ad).unwrap();
            assert_eq!(
                config.world,
                WorldMode::City {
                    psdl: "city/testcity.psdl".to_string()
                }
            );
            assert_eq!(
                config.mode,
                SessionMode::Event(EventRef {
                    city: "testcity".to_string(),
                    table: EventTableKind::Checkpoint,
                    index: 0,
                })
            );
            assert_eq!(config.seed, 13);
        }
        other => panic!("expected Session, got {other:?}"),
    }
    match alice.recv().unwrap() {
        Message::Roster { players } => assert_eq!(players.len(), 1),
        other => panic!("expected Roster, got {other:?}"),
    }
    assert_eq!(
        host.line(),
        "event=joined id=1 driver=\"alice\" build=\"test\""
    );

    alice.set_vehicle("", 0).unwrap();
    assert_eq!(host.line(), "event=vehicle id=1 vehicle=\"\" paint=0");
    alice.set_ready(true).unwrap();
    assert_eq!(host.line(), "event=ready id=1 ready=true");

    host.cmd("start");
    assert_eq!(host.line(), "event=started generation=1");
    loop {
        match alice.recv().unwrap() {
            Message::Start { generation, .. } => {
                assert_eq!(generation, 1);
                break;
            }
            Message::Roster { .. } => continue,
            other => panic!("expected Start, got {other:?}"),
        }
    }

    // MP-5's race rule: a started event lobby refuses a late joiner —
    // `LateJoin::Closed` has a real consumer here, not just wire legs.
    let err = Client::join(addr, &hello("test".to_string(), "bob".to_string(), fp)).unwrap_err();
    assert!(matches!(
        err,
        NetError::Rejected {
            code: RejectCode::SessionStarted,
            ..
        }
    ));
    let refused = host.line();
    assert!(
        refused.starts_with("event=join_failed") && refused.contains("already started"),
        "{refused}"
    );

    // Cancel re-opens the lobby: the same join now succeeds and is
    // served the advertised event session with no `Start` behind it.
    host.cmd("cancel");
    assert_eq!(host.line(), "event=cancelled generation=1");
    loop {
        match alice.recv().unwrap() {
            Message::Cancel { generation: 1 } => break,
            Message::Roster { .. } => continue,
            other => panic!("expected Cancel, got {other:?}"),
        }
    }
    let mut bob = join(addr, "bob", fp);
    assert_eq!(
        host.line(),
        "event=joined id=2 driver=\"bob\" build=\"test\""
    );
    match bob.recv().unwrap() {
        Message::Session(ad) => {
            assert!(matches!(
                net::accept(&ad).unwrap().mode,
                SessionMode::Event(_)
            ));
        }
        other => panic!("expected Session, got {other:?}"),
    }
    match bob.recv().unwrap() {
        Message::Roster { players } => assert_eq!(players.len(), 2),
        other => panic!("expected Roster, got {other:?}"),
    }
    // The lobby is in lobby phase again: nothing follows the roster.
    bob.set_timeout(Some(Duration::from_millis(300))).unwrap();
    match bob.recv() {
        Err(NetError::Io(e)) => assert!(matches!(
            e.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        )),
        other => panic!("expected a quiet socket post-cancel, got {other:?}"),
    }

    host.cmd("quit");
    assert!(host.wait().success(), "mm2-host did not exit cleanly");
}

/// `--event` failures are startup failures (exit 2), before the lobby
/// opens: a malformed selector, a row beyond the city's table and an
/// event that resolves but cannot build all refuse at flag time — the
/// host never advertises a session it cannot run.
#[test]
fn an_unrunnable_event_is_refused_at_flag_time() {
    let install = event_install();
    let install = install.path().to_str().unwrap().to_string();
    for (args, needle) in [
        // Not the `<table>:<row>` grammar.
        (
            vec!["--city", "testcity", "--event", "bogus"],
            "invalid --event",
        ),
        // Row beyond the single-row checkpoint table — resolve fails.
        (
            vec!["--city", "testcity", "--event", "race:9"],
            "cannot run",
        ),
        // Resolves Ready but `NumLaps` zero cannot build an Ordered
        // definition — the gate is the real build, not just resolve.
        (
            vec!["--city", "testcity", "--event", "circuit:0"],
            "cannot run",
        ),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_mm2-host"))
            .arg("--mm2-path")
            .arg(&install)
            .args(&args)
            .output()
            .expect("failed to run mm2-host");
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?} must exit 2, stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(needle), "{args:?} stderr: {stderr}");
    }
}
