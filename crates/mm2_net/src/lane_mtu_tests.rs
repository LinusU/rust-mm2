//! MTU and fragment failure at the real datagram boundary (NETPLAY-10D-1):
//! a path that carries small datagrams but drops the fragment-sized ones,
//! a path whose MTU collapses after the lane is up, and the measured wire
//! size of an eight-seat correction.
//!
//! Lockstep like `lane_impair_tests`: every send is followed by a
//! [`Rig::settle`], so the proxy's counters, not a sleep, say what crossed.
//! The one wall-clock dependency is the client's path-health probe, which
//! the failure test tightens through `set_path_bounds` and then polls on a
//! bounded deadline.

use std::time::{Duration, Instant};

use crate::frag::{FRAG_HEADER, split};
use crate::lane_impair_tests::{Rig, STEP};
use crate::lane_tests::{WAIT, big_snap, eventually};
use crate::{LinkDir, MAX_DATAGRAM, MAX_PAYLOAD, Message, NetError, SnapEntry, UdpImpair};

fn small_snap(generation: u64, tick: u64, player: u16) -> Message {
    Message::Snap {
        generation,
        tick,
        entries: vec![SnapEntry {
            player,
            ..SnapEntry::default()
        }],
        trailers: Vec::new(),
        impacts: Vec::new(),
        race: None,
    }
}

/// The tick of the next `Snap` the client receives (other messages, such
/// as rosters, are skipped).
fn next_snap_tick(client: &mut crate::Client) -> u64 {
    loop {
        if let Message::Snap { tick, .. } = client.recv().unwrap() {
            return tick;
        }
    }
}

fn mtu(limit: Option<usize>) -> UdpImpair {
    UdpImpair {
        mtu: limit,
        ..UdpImpair::default()
    }
}

/// Fragments `msg` takes on the lane, and the largest datagram among them
/// (lane header included).
fn fragments_of(msg: &Message, tick: u64) -> (usize, usize) {
    let bytes = msg.encode().unwrap();
    let frags = if bytes.len() > MAX_PAYLOAD {
        split(tick, &[&bytes]).unwrap()
    } else {
        vec![bytes.clone()]
    };
    println!(
        "tick {tick}: {} B message, {} datagram(s)",
        bytes.len(),
        frags.len()
    );
    let largest = frags
        .iter()
        .map(|f| f.len() + MAX_DATAGRAM - MAX_PAYLOAD)
        .max();
    (frags.len(), largest.unwrap())
}

#[test]
fn selective_oversize_drops_cost_only_the_snapshots_that_need_fragments() {
    let mut rig = Rig::new_with(21, None);
    let limit = 600;
    rig.proxy.set(LinkDir::Down, mtu(Some(limit)));
    let big = big_snap(rig.generation, 5);
    let (frags, _) = fragments_of(&big, 5);
    assert!(frags >= 2, "the fixture needs fragments");

    // A snapshot that fits under the limit is untouched.
    let before = rig.host.lane_counts().sent;
    rig.host
        .ctl()
        .send_to(rig.id, &small_snap(rig.generation, 4, rig.id))
        .unwrap();
    eventually("the small snap to leave", || {
        rig.host.lane_counts().sent > before
    });
    rig.settle(STEP);
    eventually("the small snap", || {
        rig.client.lane_inbox_stats().unwrap().len >= 1
    });
    assert_eq!(next_snap_tick(&mut rig.client), 4);
    assert_eq!(rig.proxy.stats(LinkDir::Down).mtu_dropped, 0);

    // The fragmented one loses every full-size fragment: nothing is
    // delivered, and nothing is delivered half-assembled.
    let before = rig.host.lane_counts().sent;
    rig.host.ctl().send_to(rig.id, &big).unwrap();
    eventually("the fragments to leave", || {
        rig.host.lane_counts().sent >= before + frags as u64
    });
    rig.settle(STEP);
    let dropped = rig.proxy.stats(LinkDir::Down).mtu_dropped;
    assert!(dropped >= (frags as u64).saturating_sub(1), "{dropped}");
    assert_eq!(rig.client.lane_inbox_stats().unwrap().len, 0);
    let st = rig.client.lane_assembly().unwrap();
    assert_eq!((st.groups_completed, st.ticks_completed), (0, 0));

    // Whatever small tail fragment did cross is bounded: it expires, it
    // does not pin the assembler.
    eventually("the orphan assembly to expire", || {
        rig.client.lane_assembly().unwrap().expired >= 1
            || (dropped as usize == frags && rig.client.lane_assembly().unwrap().expired == 0)
    });

    // The path heals: the same snapshot now arrives whole.
    rig.proxy.set(LinkDir::Down, mtu(None));
    let before = rig.host.lane_counts().sent;
    rig.host.ctl().send_to(rig.id, &big).unwrap();
    eventually("the fragments to leave again", || {
        rig.host.lane_counts().sent >= before + frags as u64
    });
    rig.settle(STEP);
    eventually("the healed snap", || {
        rig.client.lane_inbox_stats().unwrap().len >= 1
    });
    assert_eq!(next_snap_tick(&mut rig.client), 5);
}

#[test]
fn an_eight_seat_correction_with_a_full_impact_burst_stays_within_the_datagram_budget() {
    let mut rig = Rig::new_with(22, None);
    // The path allows exactly the documented budget and not a byte more.
    rig.proxy.set(LinkDir::Down, mtu(Some(MAX_DATAGRAM)));
    let quiet = Message::Snap {
        generation: rig.generation,
        tick: 6,
        entries: (0..crate::MAX_PLAYERS as u16)
            .map(|i| SnapEntry {
                player: i,
                ..SnapEntry::default()
            })
            .collect(),
        trailers: Vec::new(),
        impacts: Vec::new(),
        race: None,
    };
    let burst = big_snap(rig.generation, 7);
    let mut total = 0;
    for (msg, tick) in [(&quiet, 6u64), (&burst, 7)] {
        let (frags, largest) = fragments_of(msg, tick);
        assert!(largest <= MAX_DATAGRAM, "tick {tick}: {largest} B");
        let before = rig.host.lane_counts().sent;
        rig.host.ctl().send_to(rig.id, msg).unwrap();
        eventually("the fragments to leave", || {
            rig.host.lane_counts().sent >= before + frags as u64
        });
        rig.settle(STEP);
        eventually("the snap", || {
            rig.client.lane_inbox_stats().unwrap().len >= 1
        });
        loop {
            if let Message::Snap { entries, .. } = rig.client.recv().unwrap() {
                assert_eq!(entries.len(), crate::MAX_PLAYERS as usize);
                break;
            }
        }
        total += frags;
    }
    let down = rig.proxy.stats(LinkDir::Down);
    assert_eq!(down.mtu_dropped, 0, "nothing exceeded the budget");
    assert!(down.largest <= MAX_DATAGRAM, "measured {} B", down.largest);
    assert!(total >= 2);
    // The fragment header and the lane header are the only overhead.
    assert_eq!(MAX_DATAGRAM - MAX_PAYLOAD + FRAG_HEADER, 44);
}

#[test]
fn a_path_whose_mtu_collapses_after_the_join_is_a_surfaced_failure_that_heals() {
    for dir in [LinkDir::Up, LinkDir::Down] {
        let rig = Rig::new_with(23, None);
        rig.client
            .set_path_bounds(Duration::from_millis(60), Duration::from_millis(400));
        let deadline = Instant::now() + WAIT;
        // Healthy: echoes keep arriving, no failure however long we look.
        let healthy_until = Instant::now() + Duration::from_millis(900);
        while Instant::now() < healthy_until {
            rig.settle(STEP);
            assert!(
                rig.client.lane_path_failure().is_none(),
                "{dir:?}: healthy path reported failed"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        // The path's MTU drops one byte under a full datagram.
        rig.proxy.set(dir, mtu(Some(MAX_DATAGRAM - 1)));
        let err = loop {
            rig.settle(STEP);
            if let Some(err) = rig.client.lane_path_failure() {
                break err;
            }
            assert!(Instant::now() < deadline, "{dir:?}: no failure surfaced");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(
            matches!(err, NetError::LanePathFailed { .. }),
            "{dir:?}: {err:?}"
        );
        let text = err.to_string();
        assert!(text.contains("MTU") && text.contains("UDP"), "{text}");
        assert!(rig.proxy.stats(dir).mtu_dropped >= 1, "{dir:?}");
        // The path recovers; the next echo clears the failure.
        rig.proxy.set(dir, mtu(None));
        loop {
            rig.settle(STEP);
            if rig.client.lane_path_failure().is_none() {
                break;
            }
            assert!(Instant::now() < deadline, "{dir:?}: failure never cleared");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
