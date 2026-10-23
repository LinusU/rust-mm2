//! Loopback tests for send priorities, budgets and bounded queues on the
//! datagram lane (NETPLAY-06D). Time-based, so every wait spins on a
//! bounded deadline and the byte bounds carry explicit slack.

use std::thread;
use std::time::{Duration, Instant};

use crate::lane_tests::{FP, WAIT, eventually, hello_for, host, snap, start_session};
use crate::proto::SnapEntry;
use crate::{
    BULK_MAX_AGE, BULK_QUEUE_CAP, Client, HostConfig, MOVEMENT_MAX_AGE, MOVEMENT_QUEUE_CAP,
    Message, PEER_BURST_BYTES, PEER_BYTES_PER_SEC, Priority,
};

fn movement(generation: u64, tick: u64) -> Message {
    Message::Snap {
        generation,
        tick,
        entries: vec![SnapEntry::default()],
        trailers: Vec::new(),
        impacts: Vec::new(),
        race: None,
    }
}

/// A world-catch-up snapshot of the same tick: 4 fragments, no entries.
fn bulk(generation: u64, tick: u64) -> Message {
    let mut m = snap(generation, tick);
    if let Message::Snap { impacts, .. } = &mut m {
        *impacts = vec![
            crate::SnapImpact {
                seat: 1,
                id: 1,
                tick: 1,
                epoch: 0,
                ordinal: 0,
                point: [0.0; 3],
                normal: [0.0; 3],
                severity: 1.0,
                audio_id: 0,
            };
            crate::MAX_SNAP_IMPACTS as usize
        ];
    }
    m
}

#[test]
fn a_bulk_flood_beyond_the_budget_does_not_stop_movement_and_stays_bounded() {
    let host = host();
    let mut lane = Client::join_lane(host.addr(), &hello_for("alice")).unwrap();
    lane.set_timeout(Some(WAIT)).unwrap();
    let generation = start_session(&host, &mut [&mut lane]);
    let id = lane.player_id();
    eventually("the lane", || host.lane_established(id));

    // 60 Hz of ~4.3 kB bulk (about 258 kB/s offered against a 128 kB/s
    // budget) for two seconds, each tick with its movement group.
    const TICKS: u64 = 120;
    let ctl = host.ctl();
    let started = Instant::now();
    let producer = thread::spawn(move || {
        for tick in 1..=TICKS {
            let groups = [movement(generation, tick), bulk(generation, tick)];
            ctl.send_snap_groups_prio(
                id,
                generation,
                tick,
                &groups,
                &[Priority::Movement, Priority::Bulk],
            )
            .unwrap();
            thread::sleep(Duration::from_millis(16));
        }
        Instant::now()
    });
    let mut movement_ticks = Vec::new();
    let mut bulk_ticks = 0u64;
    let deadline = started + Duration::from_secs(10);
    lane.set_timeout(Some(Duration::from_millis(400))).unwrap();
    while Instant::now() < deadline {
        match lane.recv() {
            Ok(Message::Snap { tick, entries, .. }) if !entries.is_empty() => {
                movement_ticks.push(tick);
            }
            Ok(Message::Snap { .. }) => bulk_ticks += 1,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    // Production ended here; the queue may still drain for one age bound.
    let elapsed = producer.join().unwrap() - started + BULK_MAX_AGE;

    // Movement kept flowing: nearly every tick (loopback drops nothing;
    // the slack is the client's own bounded inbox under scheduling).
    assert!(
        movement_ticks.len() as u64 >= TICKS * 9 / 10,
        "movement delivered {}/{TICKS}",
        movement_ticks.len()
    );
    assert!(movement_ticks.windows(2).all(|w| w[0] < w[1]), "in order");
    // Bulk was shed, not queued: less than offered arrived, the budget
    // was exhausted and obsolete entries were discarded unsent.
    let p = host.lane_pace(id).unwrap();
    assert!(bulk_ticks < TICKS, "bulk {bulk_ticks} of {TICKS}");
    assert!(p.bulk_deferred > 0, "{p:?}");
    assert!(p.bulk.expired + p.bulk.dropped > 0, "{p:?}");
    assert!(p.bulk.len <= BULK_QUEUE_CAP && p.movement.len <= MOVEMENT_QUEUE_CAP);
    assert_eq!(p.movement.dropped + p.movement.expired, 0, "{p:?}");
    // Nothing waited longer than its bound (plus one poll of slack).
    let slack = Duration::from_millis(60);
    assert!(p.bulk.max_wait <= BULK_MAX_AGE + slack, "{p:?}");
    // Bandwidth: bulk only spends credit, so the bytes sent stay within
    // rate * elapsed + burst + the movement overdraw (~140 B x 120).
    let allowed = u64::from(PEER_BYTES_PER_SEC) * elapsed.as_millis() as u64 / 1000
        + u64::from(PEER_BURST_BYTES)
        + 120 * 200;
    assert!(
        p.bytes_sent <= allowed,
        "{} > {allowed}: {p:?}",
        p.bytes_sent
    );
    eprintln!(
        "pace: elapsed {elapsed:?} movement {}/{TICKS} bulk {bulk_ticks}/{TICKS} {p:?}",
        movement_ticks.len()
    );
}

#[test]
fn a_stalled_link_never_accumulates_obsolete_state() {
    let host = crate::Host::listen_loopback(&HostConfig::new(FP)).unwrap();
    let mut lane = Client::join_lane(host.addr(), &hello_for("alice")).unwrap();
    lane.set_timeout(Some(WAIT)).unwrap();
    let generation = start_session(&host, &mut [&mut lane]);
    let id = lane.player_id();
    eventually("the lane", || host.lane_established(id));
    // Far more bulk than the queue holds, in one burst: the queue stays
    // at its bound, the overflow is counted as dropped, and whatever the
    // budget cannot send within the age bound expires instead of waiting.
    for tick in 1..=40 {
        host.ctl()
            .send_snap_groups_prio(
                id,
                generation,
                tick,
                &[bulk(generation, tick)],
                &[Priority::Bulk],
            )
            .unwrap();
    }
    let p = host.lane_pace(id).unwrap();
    assert!(p.bulk.len <= BULK_QUEUE_CAP, "{p:?}");
    assert!(p.bulk.dropped > 0, "{p:?}");
    eventually("the backlog to drain or expire", || {
        let p = host.lane_pace(id).unwrap();
        p.bulk.len == 0
    });
    let p = host.lane_pace(id).unwrap();
    assert!(
        p.bulk.max_wait <= BULK_MAX_AGE + Duration::from_millis(60),
        "{p:?}"
    );
    assert!(p.bulk.expired > 0 || p.bulk_deferred > 0, "{p:?}");
}

/// NETPLAY-10B on real sockets: the host's aggregate uplink holds across
/// seats, counts every byte it admits, and old movement is discarded
/// rather than queued when the offered load exceeds it.
#[test]
fn the_aggregate_uplink_cap_holds_across_two_real_lanes() {
    const CAP: u32 = 40_000;
    let mut config = HostConfig::new(FP);
    config.uplink_bytes_per_sec = CAP;
    let host = crate::Host::listen_loopback(&config).unwrap();
    let mut a = Client::join_lane(host.addr(), &hello_for("alice")).unwrap();
    let mut b = Client::join_lane(host.addr(), &hello_for("bob")).unwrap();
    a.set_timeout(Some(WAIT)).unwrap();
    b.set_timeout(Some(WAIT)).unwrap();
    let generation = start_session(&host, &mut [&mut a, &mut b]);
    let ids = [a.player_id(), b.player_id()];
    for id in ids {
        eventually("the lane", || host.lane_established(id));
    }
    let (_, spent_before) = host.uplink();
    let wire_before: u64 = ids
        .iter()
        .map(|id| host.lane_pace(*id).unwrap().wire_sent)
        .sum();

    // ~54 kB/s offered to each of two seats against a 40 kB/s cap.
    const TICKS: u64 = 180;
    let ctl = host.ctl();
    let started = Instant::now();
    for tick in 1..=TICKS {
        let mut m = movement(generation, tick);
        if let Message::Snap { entries, .. } = &mut m {
            *entries = vec![SnapEntry::default(); 8];
        }
        for id in ids {
            ctl.send_snap_groups(id, generation, tick, std::slice::from_ref(&m))
                .unwrap();
        }
        thread::sleep(Duration::from_millis(16));
    }
    let elapsed = started.elapsed();
    thread::sleep(MOVEMENT_MAX_AGE * 2);

    let (cap, spent) = host.uplink();
    assert_eq!(cap, CAP);
    let admitted = spent - spent_before;
    let seats: u64 = ids
        .iter()
        .map(|id| host.lane_pace(*id).unwrap().wire_sent)
        .sum::<u64>()
        - wire_before;
    assert_eq!(admitted, seats, "every admitted byte belongs to a seat");
    let allowed = u64::from(CAP) * (elapsed + MOVEMENT_MAX_AGE * 2).as_millis() as u64 / 1000
        + u64::from(crate::UPLINK_BURST_BYTES);
    assert!(
        admitted <= allowed,
        "{admitted} B > {allowed} B in {elapsed:?}"
    );
    for id in ids {
        let p = host.lane_pace(id).unwrap();
        assert!(p.movement.len <= MOVEMENT_QUEUE_CAP, "{p:?}");
        assert!(
            p.wait_max <= MOVEMENT_MAX_AGE + Duration::from_millis(60),
            "{p:?}"
        );
        assert!(p.movement_deferred > 0, "the cap did bind: {p:?}");
    }
    eprintln!("uplink: admitted {admitted} B of {allowed} B allowed in {elapsed:?}");
}
