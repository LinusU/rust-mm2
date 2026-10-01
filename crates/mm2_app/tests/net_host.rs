//! F24-B multi-process leg: `mm2-host` runs as a *separate process* and
//! serves a real advertised lobby to `mm2_net::Client`s over loopback —
//! AC01's separate-host-process leg and AC05's headless binary (it never
//! opens a window, GPU or audio device). Client processes, LAN and the
//! impairment matrix remain F24-C scope.

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use mm2_app::net;
use mm2_game::{SessionAuthority, SessionMode, WorldMode};
use mm2_net::{Client, Message, NetError, RejectCode, hello};

const WAIT: Duration = Duration::from_secs(15);

/// A running `mm2-host` child with its stdout drained onto a channel —
/// the process's `key=value` record contract is the observable surface.
struct HostProc {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl HostProc {
    fn spawn(args: &[String]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_mm2-host"))
            .args(args)
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to spawn mm2-host");
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        Self { child, lines: rx }
    }

    /// The next record line, failing rather than hanging if the child
    /// dies or stalls.
    fn line(&self) -> String {
        self.lines
            .recv_timeout(WAIT)
            .expect("no line from mm2-host")
    }
}

impl Drop for HostProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn join(addr: SocketAddr, driver: &str, fp: u64) -> Client {
    let client = Client::join(addr, &hello("test".to_string(), driver.to_string(), fp)).unwrap();
    client.set_timeout(Some(WAIT)).unwrap();
    client
}

#[test]
fn a_dedicated_host_process_serves_an_advertised_lobby() {
    // An empty install dir is a valid (if content-free) mount — the
    // gameplay fingerprint still gates deterministically.
    let dir = tempfile::tempdir().unwrap();
    let host = HostProc::spawn(&[
        "--mm2-path".to_string(),
        dir.path().to_str().unwrap().to_string(),
        "--dev-world".to_string(),
        "--bind".to_string(),
        "127.0.0.1:0".to_string(),
        "--seed".to_string(),
        "7".to_string(),
    ]);

    // The first record binds the contract: the address peers dial and
    // the gameplay fingerprint the handshake requires.
    let first = host.line();
    let addr: SocketAddr = first
        .strip_prefix("listening=")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("unexpected first record: {first:?}"))
        .parse()
        .unwrap();
    let fp_hex = first
        .split_whitespace()
        .find_map(|tok| tok.strip_prefix("fingerprint=fnv1a64:"))
        .unwrap_or_else(|| panic!("no fingerprint in {first:?}"));
    let fp = u64::from_str_radix(fp_hex, 16).unwrap();
    assert!(first.contains("seed=7"), "seed not in {first:?}");

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
