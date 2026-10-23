//! NETPLAY-10A: a peer that stops reading must cost the others nothing,
//! over real loopback sockets — the host's queues stay bounded, the
//! healthy peers see every frame in order, and the stalled peer is
//! disconnected by an explicit policy.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use crate::proto::{SNAP_NO_FRAGMENT, SiteTable, SnapProp};
use crate::{Client, Host, HostConfig, HostEvent, LeaveCause, Message, PEER_QUEUE_CAP, hello};

const FP: u64 = 0x10a;
const WAIT: Duration = Duration::from_secs(20);

fn props(tick: u64) -> Message {
    Message::Props {
        generation: 1,
        tick,
        table: SiteTable::default(),
        rows: (0..96)
            .map(|site| SnapProp {
                site,
                fragment: SNAP_NO_FRAGMENT,
                phase: 0,
                pos: [0.0; 3],
                rot: [0.0, 0.0, 0.0, 1.0],
            })
            .collect(),
    }
}

fn join(host: &Host, driver: &str) -> Client {
    let client = Client::join(
        host.addr(),
        &hello("test".to_string(), driver.to_string(), FP),
    )
    .unwrap();
    client.set_timeout(Some(WAIT)).unwrap();
    client
}

/// A reader thread that records every `Props` tick it receives, in order.
fn spawn_healthy(mut client: Client) -> (Arc<AtomicU64>, mpsc::Receiver<Vec<u64>>) {
    let newest = Arc::new(AtomicU64::new(0));
    let seen = Arc::clone(&newest);
    let (done, ticks) = mpsc::channel();
    thread::spawn(move || {
        let mut got = Vec::new();
        while let Ok(msg) = client.recv() {
            if let Message::Props { tick, .. } = msg {
                got.push(tick);
                seen.store(tick, Ordering::Release);
                if tick == u64::MAX {
                    break;
                }
            }
        }
        let _ = done.send(got);
    });
    (newest, ticks)
}

#[test]
fn a_non_reading_peer_is_cut_loose_and_costs_the_healthy_peers_nothing() {
    let host = Host::listen_loopback(&HostConfig {
        // The queue-full path is the fast one; this bounds the slow one.
        peer_stall_limit: Duration::from_millis(500),
        ..HostConfig::new(FP)
    })
    .unwrap();
    let healthy_a = join(&host, "a");
    let healthy_b = join(&host, "b");
    let stalled = join(&host, "stalled"); // never calls `recv`
    let mut ids = Vec::new();
    while ids.len() < 3 {
        match host.recv_timeout(WAIT).unwrap() {
            HostEvent::Joined { id, .. } => ids.push(id),
            other => panic!("unexpected {other:?}"),
        }
    }
    let stalled_id = ids[2];
    let ctl = host.ctl();
    let (_, ticks_a) = spawn_healthy(healthy_a);
    let (_, ticks_b) = spawn_healthy(healthy_b);

    let mut worst_call = Duration::ZERO;
    let mut max_len = 0;
    let mut sent = Vec::new();
    let mut left = None;
    let deadline = Instant::now() + WAIT;
    let mut tick = 0u64;
    while left.is_none() {
        assert!(Instant::now() < deadline, "the stalled peer was never cut");
        tick += 1;
        loop {
            let t = Instant::now();
            let r = ctl.broadcast(&props(tick));
            worst_call = worst_call.max(t.elapsed());
            match r {
                Ok(()) => {
                    sent.push(tick);
                    break;
                }
                // The host loop's bounded queue pushes back with an error,
                // never a wait: back off and retry.
                Err(_) => thread::sleep(Duration::from_millis(1)),
            }
        }
        if let Some(q) = host.io_queue(stalled_id) {
            assert!(q.len <= PEER_QUEUE_CAP, "queue grew past its bound: {q:?}");
            max_len = max_len.max(q.len);
        }
        while let Ok(ev) = host.try_recv() {
            if let HostEvent::Left { id, cause, .. } = ev {
                assert_eq!(id, stalled_id, "a healthy peer was dropped");
                left = Some(cause);
            }
        }
        thread::sleep(Duration::from_micros(200));
    }
    assert_eq!(left, Some(LeaveCause::Lost));
    assert!(
        worst_call < Duration::from_secs(1),
        "a broadcast waited {worst_call:?} on a socket"
    );
    assert!(
        max_len > 0,
        "the socket never pushed back; test proved nothing"
    );
    drop(stalled);

    // The sentinel closes the healthy readers; each saw every frame, once,
    // in order.
    loop {
        if ctl.broadcast(&props(u64::MAX)).is_ok() {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    for ticks in [ticks_a, ticks_b] {
        let mut got = ticks.recv_timeout(WAIT).unwrap();
        assert_eq!(got.pop(), Some(u64::MAX));
        assert_eq!(got, sent, "a healthy peer lost or reordered frames");
    }
}

/// The host's event queue is bounded: a consumer that never drains ends
/// the lobby (explicit failure) instead of growing the queue.
#[test]
fn a_consumer_that_never_drains_events_ends_the_lobby() {
    let host = Host::listen_loopback(&HostConfig::new(FP)).unwrap();
    let mut client = join(&host, "spammer");
    let ctl = client.ctl().unwrap();
    let deadline = Instant::now() + WAIT;
    let mut flip = false;
    // Every ready toggle is one host event; `host` is never drained.
    // The lobby ends when its event queue fills, closing this socket.
    let ended = loop {
        assert!(Instant::now() < deadline, "the lobby never gave up");
        flip = !flip;
        match ctl.set_ready(flip) {
            Ok(()) => {}
            // The client's own bounded queue: let the writer catch up.
            Err(_) if client.recv().is_ok() => continue,
            Err(_) => break true,
        }
        // Keep this side's receive queue from backing the host up.
        client.set_timeout(Some(Duration::from_millis(1))).unwrap();
        if let Err(e) = client.recv()
            && !matches!(&e, crate::NetError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock
                || io.kind() == std::io::ErrorKind::TimedOut)
        {
            break true;
        }
    };
    assert!(ended);
}
