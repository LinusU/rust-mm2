//! The reliable-message channel through the real datagram boundary
//! (NETPLAY-06E-1): a lane client and host talk through the seeded
//! [`UdpImpairProxy`], with the reliable clock driven by hand so a
//! retransmission happens when the test advances it, not when a wall
//! clock happens to.
//!
//! Every rig applies its impairment **from the first datagram** — the
//! lane's own probes cross it too — and the first reliable message is
//! then lost on purpose: the bug class this primitive exists for is the
//! message sent once whose only datagram vanished.

use std::thread;
use std::time::Duration;

use crate::lane_impair_tests::{Rig, STEP, TICK_PACE};
use crate::{Impair, LinkDir, RELIABLE_WINDOW, ReliableSendError, SendError, UdpImpair};

/// Reliable data datagrams carry this lane tag.
const TAG_RELIABLE: u8 = 5;
const TAG_RELIABLE_ACK: u8 = 6;
/// Rounds (one [`STEP`] of both clocks each) a test waits before it calls
/// a convergence failed: ~9.6 s of virtual time, past the 3.9 s a channel
/// takes to give up.
const MAX_ROUNDS: u32 = 600;

fn lossy(loss: f64) -> UdpImpair {
    UdpImpair {
        link: Impair {
            delay: Duration::from_millis(5),
            jitter: Duration::from_millis(25),
            loss,
            duplicate: 0.25,
            reorder: 0.25,
        },
        ..UdpImpair::default()
    }
}

/// A rig whose relay drops and duplicates from the very first datagram
/// (the lane's probes cross it), then — once the lane is up — also delays
/// and reorders: the relay's virtual clock is frozen until a test advances
/// it, so a delayed probe could never establish the lane. Both reliable
/// clocks are manual.
fn rig(seed: u64, loss: f64) -> Rig {
    let from_start = UdpImpair {
        link: Impair {
            loss,
            duplicate: 0.25,
            ..Impair::default()
        },
        ..UdpImpair::default()
    };
    let rig = Rig::new_with(seed, Some(from_start));
    rig.set_both(lossy(loss));
    rig.host.use_manual_reliable_clock();
    rig.client.use_manual_reliable_clock();
    rig
}

/// One virtual round: both reliable clocks and the relay move by [`STEP`].
fn round(rig: &Rig) {
    rig.host.advance_reliable_clock(STEP);
    rig.client.advance_reliable_clock(STEP);
    rig.settle(STEP);
    thread::sleep(TICK_PACE);
}

/// Run rounds until `done`, failing after [`MAX_ROUNDS`].
fn pump(rig: &Rig, what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..MAX_ROUNDS {
        if done() {
            return;
        }
        round(rig);
    }
    assert!(done(), "never converged: {what}");
}

/// Keep running `n` more rounds: whatever was going to be delivered twice
/// has had every chance to be.
fn quiesce(rig: &Rig, n: u32) {
    for _ in 0..n {
        round(rig);
    }
}

#[test]
fn the_first_client_message_lost_from_connection_start_is_repeated_and_applied_once() {
    let rig = rig(11, 0.15);
    // The very first reliable datagram never reaches the host.
    rig.proxy.drop_next(LinkDir::Up, 1);
    rig.client.reliable_send(rig.generation, b"start?").unwrap();
    let mut got = Vec::new();
    pump(&rig, "the first message", || {
        got.extend(std::iter::from_fn(|| rig.host.reliable_recv()));
        !got.is_empty()
    });
    quiesce(&rig, 40);
    got.extend(std::iter::from_fn(|| rig.host.reliable_recv()));
    assert_eq!(got, vec![(rig.id, b"start?".to_vec())]);
    let client = rig.client.reliable_stats().unwrap();
    assert!(client.sender.retransmits >= 1, "{client:?}");
    assert_eq!((client.sender.acked, client.sender.in_flight), (1, 0));
    assert!(!client.sender.dead);
    let host = rig.host.reliable_stats(rig.id).unwrap();
    assert_eq!(host.receiver.delivered, 1, "{host:?}");
    assert!(rig.proxy.stats(LinkDir::Up).link.dropped >= 1);
}

#[test]
fn the_first_host_message_lost_from_connection_start_is_repeated_and_applied_once() {
    let rig = rig(12, 0.15);
    rig.proxy.drop_next(LinkDir::Down, 1);
    rig.host
        .reliable_send(rig.id, rig.generation, b"cancel")
        .unwrap();
    let mut got = Vec::new();
    pump(&rig, "the first message", || {
        got.extend(std::iter::from_fn(|| rig.client.reliable_recv()));
        !got.is_empty()
    });
    quiesce(&rig, 40);
    got.extend(std::iter::from_fn(|| rig.client.reliable_recv()));
    assert_eq!(got, vec![b"cancel".to_vec()]);
    let host = rig.host.reliable_stats(rig.id).unwrap();
    assert!(host.sender.retransmits >= 1, "{host:?}");
    assert_eq!((host.sender.acked, host.sender.in_flight), (1, 0));
    assert_eq!(rig.client.reliable_stats().unwrap().receiver.delivered, 1);
}

#[test]
fn a_stream_both_ways_survives_loss_reorder_duplication_and_a_burst() {
    const N: u32 = 60;
    let rig = rig(0x5eed, 0.15);
    rig.proxy.drop_next(LinkDir::Up, 1);
    rig.proxy.drop_next(LinkDir::Down, 1);
    let (mut up, mut down) = (0..N, 0..N);
    let (mut next_up, mut next_down) = (up.next(), down.next());
    let (mut at_host, mut at_client) = (Vec::new(), Vec::new());
    let mut burst_done = false;
    pump(&rig, "the streams", || {
        // Offer messages until the window pushes back, then wait for acks.
        while let Some(i) = next_up {
            match rig.client.reliable_send(rig.generation, &i.to_le_bytes()) {
                Ok(()) => next_up = up.next(),
                Err(ReliableSendError::Channel(SendError::Full)) => break,
                Err(e) => panic!("{e}"),
            }
        }
        while let Some(i) = next_down {
            match rig
                .host
                .reliable_send(rig.id, rig.generation, &i.to_le_bytes())
            {
                Ok(()) => next_down = down.next(),
                Err(ReliableSendError::Channel(SendError::Full)) => break,
                Err(e) => panic!("{e}"),
            }
        }
        if !burst_done && at_host.len() >= 10 {
            // A burst loss mid-stream, both ways.
            rig.proxy.drop_next(LinkDir::Up, 5);
            rig.proxy.drop_next(LinkDir::Down, 5);
            burst_done = true;
        }
        at_host.extend(std::iter::from_fn(|| rig.host.reliable_recv()).map(|(_, m)| m));
        at_client.extend(std::iter::from_fn(|| rig.client.reliable_recv()));
        at_host.len() == N as usize && at_client.len() == N as usize
    });
    quiesce(&rig, 40);
    at_host.extend(std::iter::from_fn(|| rig.host.reliable_recv()).map(|(_, m)| m));
    at_client.extend(std::iter::from_fn(|| rig.client.reliable_recv()));
    let expect: Vec<Vec<u8>> = (0..N).map(|i| i.to_le_bytes().to_vec()).collect();
    assert_eq!(at_host, expect, "client → host: exactly once, in order");
    assert_eq!(at_client, expect, "host → client: exactly once, in order");

    for dir in [LinkDir::Up, LinkDir::Down] {
        let s = rig.proxy.stats(dir).link;
        assert!(
            s.dropped > 0 && s.duplicated > 0 && s.reordered > 0,
            "{dir:?} {s:?}"
        );
    }
    let client = rig.client.reliable_stats().unwrap();
    let host = rig.host.reliable_stats(rig.id).unwrap();
    for (name, tx, rx) in [
        ("client→host", client.sender, host.receiver),
        ("host→client", host.sender, client.receiver),
    ] {
        assert_eq!(tx.queued, u64::from(N), "{name} {tx:?}");
        assert_eq!((tx.acked, tx.in_flight), (u64::from(N), 0), "{name} {tx:?}");
        assert!(!tx.dead && tx.given_up == 0, "{name} {tx:?}");
        assert!(
            tx.refused_full > 0,
            "{name}: the window was exercised {tx:?}"
        );
        assert_eq!(rx.delivered, u64::from(N), "{name} {rx:?}");
    }
}

#[test]
fn lost_acks_still_converge() {
    let rig = rig(21, 0.0);
    rig.client.reliable_send(rig.generation, b"once").unwrap();
    // Every ack of the first three copies is lost on the way back.
    rig.proxy.drop_next(LinkDir::Down, 3);
    pump(&rig, "the sender to see an ack", || {
        rig.client.reliable_stats().unwrap().sender.acked == 1
    });
    quiesce(&rig, 40);
    let got: Vec<_> = std::iter::from_fn(|| rig.host.reliable_recv()).collect();
    assert_eq!(got, vec![(rig.id, b"once".to_vec())]);
    let client = rig.client.reliable_stats().unwrap();
    let host = rig.host.reliable_stats(rig.id).unwrap();
    assert_eq!(client.sender.in_flight, 0);
    assert!(client.sender.retransmits >= 3, "{client:?}");
    assert_eq!(host.receiver.delivered, 1);
    assert!(host.receiver.duplicates >= 3, "{host:?}");
}

#[test]
fn a_black_hole_is_bounded_and_counted() {
    let rig = rig(31, 0.0);
    rig.proxy.set(
        LinkDir::Up,
        UdpImpair {
            blackhole: true,
            ..UdpImpair::default()
        },
    );
    for i in 0..3u8 {
        rig.client.reliable_send(rig.generation, &[i]).unwrap();
    }
    pump(&rig, "the channel to give up", || {
        rig.client.reliable_stats().unwrap().sender.dead
    });
    quiesce(&rig, 40);
    let s = rig.client.reliable_stats().unwrap().sender;
    assert_eq!((s.queued, s.given_up, s.in_flight, s.acked), (3, 3, 0, 0));
    assert_eq!(
        s.retransmits,
        3 * u64::from(crate::MAX_ATTEMPTS - 1),
        "{s:?}"
    );
    assert_eq!(
        rig.client.reliable_send(rig.generation, b"late"),
        Err(ReliableSendError::Channel(SendError::Dead))
    );
    assert_eq!(rig.client.reliable_stats().unwrap().sender.refused_dead, 1);
    // Nothing more goes on the wire once it gave up, and nothing arrived.
    let sent = rig.proxy.tagged(LinkDir::Up, TAG_RELIABLE).0;
    assert_eq!(sent, 3 * u64::from(crate::MAX_ATTEMPTS), "bounded: {sent}");
    quiesce(&rig, 40);
    assert_eq!(rig.proxy.tagged(LinkDir::Up, TAG_RELIABLE).0, sent);
    assert_eq!(rig.host.reliable_recv(), None);
    assert_eq!(rig.proxy.tagged(LinkDir::Down, TAG_RELIABLE_ACK).0, 0);
}

#[test]
fn a_full_window_and_an_oversize_body_are_refused_not_dropped() {
    let rig = rig(41, 0.0);
    for i in 0..RELIABLE_WINDOW {
        rig.client
            .reliable_send(rig.generation, &[i as u8])
            .unwrap();
    }
    assert_eq!(
        rig.client.reliable_send(rig.generation, b"x"),
        Err(ReliableSendError::Channel(SendError::Full))
    );
    assert_eq!(
        rig.client
            .reliable_send(rig.generation, &vec![0; crate::MAX_RELIABLE_BODY + 1]),
        Err(ReliableSendError::Channel(SendError::TooLarge))
    );
    let mut got = Vec::new();
    pump(&rig, "the window to deliver", || {
        got.extend(std::iter::from_fn(|| rig.host.reliable_recv()).map(|(_, m)| m));
        got.len() == RELIABLE_WINDOW
    });
    let expect: Vec<Vec<u8>> = (0..RELIABLE_WINDOW).map(|i| vec![i as u8]).collect();
    assert_eq!(got, expect);
    let s = rig.client.reliable_stats().unwrap().sender;
    assert_eq!((s.refused_full, s.refused_large), (1, 1));
    pump(&rig, "every ack", || {
        rig.client.reliable_stats().unwrap().sender.in_flight == 0
    });
    assert!(
        rig.client
            .reliable_send(rig.generation, b"room again")
            .is_ok()
    );
}

#[test]
fn a_seat_without_a_lane_refuses_reliable_sends() {
    let rig = rig(51, 0.0);
    assert_eq!(
        rig.host.reliable_send(rig.id + 100, rig.generation, b"x"),
        Err(ReliableSendError::NoLane)
    );
}
