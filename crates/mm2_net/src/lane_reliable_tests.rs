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
fn rig(seed: u64, loss: f64) -> Lobby {
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
    // Starting the session broadcast rosters, which the host sends to the
    // client on this very channel (NETPLAY-06E-2a). Settle and discard
    // them so a test counts only its own messages; `base` is how many
    // the host→client direction carried.
    let lobby = Lobby { rig, base: 0 };
    let id = lobby.id;
    pump(&lobby, "the join and start rosters", || {
        let s = lobby.host.reliable_stats(id).unwrap().sender;
        s.queued > 0 && s.in_flight == 0
    });
    while lobby.client.reliable_recv().is_some() {}
    let base = lobby.host.reliable_stats(id).unwrap().sender.queued;
    assert_eq!(
        lobby.client.reliable_stats().unwrap().receiver.delivered,
        base
    );
    Lobby { base, ..lobby }
}

/// A [`Rig`] plus the count of roster messages already carried
/// host→client before the test began.
struct Lobby {
    rig: Rig,
    base: u64,
}

impl std::ops::DerefMut for Lobby {
    fn deref_mut(&mut self) -> &mut Rig {
        &mut self.rig
    }
}

impl std::ops::Deref for Lobby {
    type Target = Rig;
    fn deref(&self) -> &Rig {
        &self.rig
    }
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
    assert_eq!(
        (host.sender.acked, host.sender.in_flight),
        (rig.base + 1, 0)
    );
    assert_eq!(
        rig.client.reliable_stats().unwrap().receiver.delivered,
        rig.base + 1
    );
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
    for (name, tx, rx, base) in [
        ("client→host", client.sender, host.receiver, 0),
        ("host→client", host.sender, client.receiver, rig.base),
    ] {
        let n = u64::from(N) + base;
        assert_eq!(tx.queued, n, "{name} {tx:?}");
        assert_eq!((tx.acked, tx.in_flight), (n, 0), "{name} {tx:?}");
        assert!(!tx.dead && tx.given_up == 0, "{name} {tx:?}");
        assert!(
            tx.refused_full > 0,
            "{name}: the window was exercised {tx:?}"
        );
        assert_eq!(rx.delivered, n, "{name} {rx:?}");
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
    // Two bytes: a lone byte can be a bare `Leave` tag, which the lane
    // takes for itself (NETPLAY-06E-2c).
    for i in 0..RELIABLE_WINDOW {
        rig.client
            .reliable_send(rig.generation, &[0xEE, i as u8])
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
    let expect: Vec<Vec<u8>> = (0..RELIABLE_WINDOW).map(|i| vec![0xEE, i as u8]).collect();
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

// NETPLAY-06E-3: the app messages that ride the channel.

fn impact(id: u64) -> crate::SnapImpact {
    crate::SnapImpact {
        seat: 1,
        id,
        tick: 40 + id,
        point: [1.0, 2.0, 3.0],
        normal: [0.0, 1.0, 0.0],
        severity: 7.5,
        audio_id: -3,
    }
}

fn cnr_with_outcome(generation: u64) -> Message {
    Message::Cnr {
        generation,
        frame: crate::SnapCnr {
            variant: 1,
            end: 2,
            end_value: 500,
            round: 1,
            revision: 9,
            elapsed: 4_000,
            gold: 0,
            holder: crate::SNAP_CNR_NO_PLAYER,
            at: [0.0; 3],
            free_at: 0,
            sites: [[0.0; 3]; 3],
            outcome: Some(crate::SnapCnrOutcome {
                reason: 1,
                winner: 0,
                who: 1,
                at_tick: 3_999,
            }),
            seats: Vec::new(),
        },
    }
}

use crate::Message;

/// The `Cnr`/`Impacts` events the client's `recv` yields until it times
/// out (the lobby's own setup traffic is filtered out).
fn client_events(rig: &mut Rig) -> Vec<Message> {
    rig.client
        .set_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let mut out = Vec::new();
    while let Ok(m) = rig.client.recv() {
        if matches!(m, Message::Cnr { .. } | Message::Impacts { .. }) {
            out.push(m);
        }
    }
    out
}

#[test]
fn a_reset_ask_whose_first_datagram_is_lost_reaches_the_host_once() {
    let rig = rig(21, 0.15);
    rig.proxy.drop_next(LinkDir::Up, 1);
    rig.client
        .ctl()
        .unwrap()
        .request_reset(rig.generation)
        .unwrap();
    let inputs = rig.host.remote_inputs();
    let mut asks = Vec::new();
    pump(&rig, "the reset ask", || {
        asks.extend(inputs.drain_resets());
        !asks.is_empty()
    });
    quiesce(&rig, 40);
    asks.extend(inputs.drain_resets());
    assert_eq!(asks, vec![(rig.id, rig.generation)]);
    let client = rig.client.reliable_stats().unwrap();
    assert!(client.sender.retransmits >= 1, "{client:?}");
    assert_eq!(
        rig.host.reliable_stats(rig.id).unwrap().receiver.delivered,
        1
    );
    // The ask was consumed by the mailbox, not left for `reliable_recv`.
    assert_eq!(rig.host.reliable_recv(), None);
}

#[test]
fn a_reset_ask_behind_another_message_class_is_still_absorbed() {
    let rig = rig(22, 0.0);
    let ctl = rig.client.ctl().unwrap();
    rig.client.reliable_send(rig.generation, b"other").unwrap();
    ctl.request_reset(rig.generation).unwrap();
    let inputs = rig.host.remote_inputs();
    pump(&rig, "both messages delivered", || {
        rig.host.reliable_stats(rig.id).unwrap().receiver.delivered == 2
    });
    // The ask waits behind the unconsumed message…
    assert!(inputs.drain_resets().is_empty());
    // …and follows the moment the lane's consumer takes it.
    assert_eq!(rig.host.reliable_recv(), Some((rig.id, b"other".to_vec())));
    assert_eq!(inputs.drain_resets(), vec![(rig.id, rig.generation)]);
}

#[test]
fn a_cnr_outcome_whose_first_datagram_is_lost_reaches_the_client_once() {
    let mut rig = rig(23, 0.15);
    rig.proxy.drop_next(LinkDir::Down, 1);
    let msg = cnr_with_outcome(rig.generation);
    let fanout = rig
        .host
        .ctl()
        .reliable_broadcast(rig.generation, &msg)
        .unwrap();
    assert_eq!((fanout.reliable, fanout.fell_back), (1, 0));
    pump(&rig, "the outcome frame", || {
        rig.client.reliable_stats().unwrap().receiver.delivered == rig.base + 1
    });
    quiesce(&rig, 40);
    assert_eq!(client_events(&mut rig), vec![msg]);
    let host = rig.host.reliable_stats(rig.id).unwrap();
    assert!(host.sender.retransmits >= 1, "{host:?}");
    assert_eq!(
        (host.sender.acked, host.sender.in_flight),
        (rig.base + 1, 0)
    );
}

#[test]
fn impact_events_survive_a_lost_first_datagram_in_order_and_apply_once() {
    let mut rig = rig(24, 0.15);
    rig.proxy.drop_next(LinkDir::Down, 1);
    let sent: Vec<Message> = (1..=3)
        .map(|i| Message::Impacts {
            generation: rig.generation,
            impacts: vec![impact(i)],
        })
        .collect();
    for m in &sent {
        rig.host
            .ctl()
            .reliable_broadcast(rig.generation, m)
            .unwrap();
    }
    pump(&rig, "the impact events", || {
        rig.client.reliable_stats().unwrap().receiver.delivered == rig.base + 3
    });
    quiesce(&rig, 40);
    assert_eq!(client_events(&mut rig), sent);
    assert_eq!(
        rig.client.reliable_stats().unwrap().receiver.delivered,
        rig.base + 3
    );
}

#[test]
fn a_refused_reliable_send_falls_back_to_the_control_stream() {
    let mut rig = rig(25, 0.0);
    // Fill the window with messages nobody acks yet (the relay is
    // frozen until a round runs), then one more.
    let mut sent = Vec::new();
    let mut fell_back = 0;
    for i in 1..=u64::from(RELIABLE_WINDOW as u32) + 2 {
        let m = Message::Impacts {
            generation: rig.generation,
            impacts: vec![impact(i)],
        };
        let f = rig
            .host
            .ctl()
            .reliable_broadcast(rig.generation, &m)
            .unwrap();
        fell_back += u32::from(f.fell_back);
        sent.push(m);
    }
    assert!(fell_back >= 1, "the window never filled");
    pump(&rig, "delivery", || {
        rig.client.reliable_stats().unwrap().receiver.delivered - rig.base + u64::from(fell_back)
            >= sent.len() as u64
    });
    quiesce(&rig, 40);
    let mut got = client_events(&mut rig);
    assert_eq!(got.len(), sent.len(), "every message exactly once");
    got.sort_by_key(|m| match m {
        Message::Impacts { impacts, .. } => impacts[0].id,
        _ => 0,
    });
    assert_eq!(got, sent);
}
