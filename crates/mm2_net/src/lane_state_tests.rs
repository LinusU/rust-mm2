//! ACKed delta baselines at the real datagram boundary (NETPLAY-10D-2): a
//! host sends a seat's snapshot groups through [`HostCtl::send_snap_states_prio`],
//! the client reconstructs and acknowledges them, and the seeded
//! [`UdpImpairProxy`] loses baselines, acknowledgements and whole ticks.
//!
//! Lockstep like `lane_impair_tests`: [`exchange`] waits for the proxy's
//! per-tag counters, not for a sleep, so a blackholed leg is exact.

use crate::lane_impair_tests::{Rig, STEP};
use crate::lane_tests::eventually;
use crate::{Impair, LaneTag, LinkDir, Message, REFRESH_TICKS, SnapEntry, StateStats, UdpImpair};

const FRAGMENT: u8 = LaneTag::Fragment as u8;
const ACK: u8 = LaneTag::Ack as u8;

/// A group of the tick: one entry per player, every field a function of
/// `(tick, player)` so a wrongly reconstructed byte shows up.
fn group(generation: u64, tick: u64, players: &[u16]) -> Message {
    Message::Snap {
        generation,
        tick,
        entries: players
            .iter()
            .map(|&p| SnapEntry {
                player: p,
                pos: [tick as f32 * 0.25 + f32::from(p), 1.5, f32::from(p) * 3.0],
                vel: [15.0, 0.0, f32::from(p)],
                rpm: (tick % 4000) as u16,
                input_ack: tick.saturating_sub(3),
                ..SnapEntry::default()
            })
            .collect(),
        trailers: Vec::new(),
        impacts: Vec::new(),
        race: None,
    }
}

/// Send `groups` of `tick`, expecting `delivered` of them to cross the
/// relay (and so be acknowledged); returns the `Snap`s the client got.
fn exchange(
    rig: &mut Rig,
    tick: u64,
    epoch: u8,
    groups: &[(u8, Message)],
    delivered: u64,
) -> Vec<Message> {
    let frags = rig.proxy.tagged(LinkDir::Down, FRAGMENT).0;
    let acks = rig.proxy.tagged(LinkDir::Up, ACK).0;
    let applied = rig.client.lane_states().unwrap().applied;
    assert!(
        rig.host
            .ctl()
            .send_snap_states_prio(rig.id, rig.generation, tick, epoch, groups, &[])
            .unwrap()
    );
    eventually("the groups to leave the host", || {
        rig.proxy.tagged(LinkDir::Down, FRAGMENT).0 >= frags + groups.len() as u64
    });
    rig.settle(STEP);
    eventually("the client to apply what crossed", || {
        rig.client.lane_states().unwrap().applied >= applied + delivered
    });
    eventually("the acknowledgements to leave the client", || {
        rig.proxy.tagged(LinkDir::Up, ACK).0 >= acks + delivered
    });
    rig.settle(STEP);
    (0..delivered).map(|_| next_snap(rig)).collect()
}

/// The next `Snap` the client has, past the control messages that
/// preceded the run.
fn next_snap(rig: &mut Rig) -> Message {
    loop {
        let m = rig.client.recv().unwrap();
        if matches!(m, Message::Snap { .. }) {
            return m;
        }
    }
}

fn host_states(rig: &Rig) -> StateStats {
    rig.host.ctl().lane_states()
}

fn blackhole() -> UdpImpair {
    UdpImpair {
        blackhole: true,
        ..UdpImpair::default()
    }
}

#[test]
fn a_delta_follows_an_acknowledged_full_state_and_reconstructs_exactly() {
    let mut rig = Rig::new_with(7, None);
    let g = rig.generation;
    let (a, b) = (group(g, 10, &[0, 1, 2]), group(g, 11, &[0, 1, 2]));

    let got = exchange(&mut rig, 10, 0, &[(0, a.clone())], 1);
    assert_eq!(got, [a]);
    assert_eq!(
        (host_states(&rig).sent_full, host_states(&rig).sent_delta),
        (1, 0)
    );
    eventually("the group ack", || {
        rig.host
            .lane_group_ack(rig.id, 0)
            .is_some_and(|k| k.tick == 10)
    });

    let got = exchange(&mut rig, 11, 0, &[(0, b.clone())], 1);
    assert_eq!(got, [b], "the delta reproduces the state byte for byte");
    let st = host_states(&rig);
    assert_eq!((st.sent_full, st.sent_delta), (1, 1));
    assert!(st.sent_bytes < st.raw_bytes, "{st:?}");
    assert_eq!(rig.client.lane_states().unwrap().missing_base, 0);
}

#[test]
fn a_lost_baseline_is_never_deltaed_against() {
    let mut rig = Rig::new_with(8, None);
    let g = rig.generation;
    // The first full state never arrives, so it is never acknowledged.
    rig.proxy.set(LinkDir::Down, blackhole());
    let frags = rig.proxy.tagged(LinkDir::Down, FRAGMENT).0;
    rig.host
        .ctl()
        .send_snap_states_prio(rig.id, g, 20, 0, &[(0, group(g, 20, &[0, 1]))], &[])
        .unwrap();
    eventually("the lost state to leave the host", || {
        rig.proxy.tagged(LinkDir::Down, FRAGMENT).0 > frags
    });
    rig.settle(STEP);
    rig.proxy.set(LinkDir::Down, UdpImpair::default());

    // The next tick must be a full state: nothing is acknowledged.
    let next = group(g, 21, &[0, 1]);
    let got = exchange(&mut rig, 21, 0, &[(0, next.clone())], 1);
    assert_eq!(got, [next]);
    let st = host_states(&rig);
    assert_eq!((st.sent_full, st.sent_delta), (2, 0));
    let client = rig.client.lane_states().unwrap();
    assert_eq!((client.applied, client.missing_base), (1, 0));

    // Acknowledged now, so the third tick deltas.
    let third = group(g, 22, &[0, 1]);
    assert_eq!(exchange(&mut rig, 22, 0, &[(0, third.clone())], 1), [third]);
    assert_eq!(host_states(&rig).sent_delta, 1);
}

#[test]
fn a_lost_ack_costs_a_full_state_and_the_next_ack_recovers() {
    let mut rig = Rig::new_with(9, None);
    let g = rig.generation;
    let first = group(g, 30, &[0, 1]);
    exchange(&mut rig, 30, 0, &[(0, first)], 1);
    eventually("the first ack", || {
        rig.host
            .lane_group_ack(rig.id, 0)
            .is_some_and(|k| k.tick == 30)
    });

    // Tick 31 is applied by the client, but its ack is lost.
    rig.proxy.set(LinkDir::Up, blackhole());
    let acks = rig.proxy.tagged(LinkDir::Up, ACK).0;
    let second = group(g, 31, &[0, 1]);
    let frags = rig.proxy.tagged(LinkDir::Down, FRAGMENT).0;
    rig.host
        .ctl()
        .send_snap_states_prio(rig.id, g, 31, 0, &[(0, second.clone())], &[])
        .unwrap();
    eventually("the second state to cross", || {
        rig.proxy.tagged(LinkDir::Down, FRAGMENT).0 > frags
    });
    rig.settle(STEP);
    assert_eq!(next_snap(&mut rig), second);
    eventually("the lost ack to leave the client", || {
        rig.proxy.tagged(LinkDir::Up, ACK).0 > acks
    });
    rig.settle(STEP);
    assert_eq!(rig.host.lane_group_ack(rig.id, 0).unwrap().tick, 30);
    rig.proxy.set(LinkDir::Up, UdpImpair::default());

    // Tick 32 deltas against the still-acknowledged tick 30, not the lost 31
    // — the client holds both, so it applies.
    let third = group(g, 32, &[0, 1]);
    assert_eq!(exchange(&mut rig, 32, 0, &[(0, third.clone())], 1), [third]);
    let st = host_states(&rig);
    assert_eq!((st.sent_full, st.sent_delta), (1, 2));
    assert_eq!(rig.client.lane_states().unwrap().missing_base, 0);
    // The ack of 32 supersedes the lost one of 31.
    assert_eq!(rig.host.lane_group_ack(rig.id, 0).unwrap().tick, 32);
}

#[test]
fn an_epoch_bump_forces_a_full_state_and_retires_old_acks() {
    let mut rig = Rig::new_with(10, None);
    let g = rig.generation;
    exchange(&mut rig, 40, 3, &[(0, group(g, 40, &[0, 1]))], 1);
    exchange(&mut rig, 41, 3, &[(0, group(g, 41, &[0, 1]))], 1);
    assert_eq!(host_states(&rig).sent_delta, 1);

    // A reset: the new epoch is a full state although tick 41 is acked.
    let after = group(g, 42, &[0, 1]);
    assert_eq!(exchange(&mut rig, 42, 4, &[(0, after.clone())], 1), [after]);
    let st = host_states(&rig);
    assert_eq!((st.sent_full, st.sent_delta), (2, 1));
    assert_eq!(rig.host.lane_group_ack(rig.id, 0).unwrap().epoch, 4);

    // And the epoch's own ack lets the next tick delta again.
    exchange(&mut rig, 43, 4, &[(0, group(g, 43, &[0, 1]))], 1);
    assert_eq!(host_states(&rig).sent_delta, 2);
}

#[test]
fn a_baseline_older_than_the_refresh_bound_is_replaced_by_a_full_state() {
    let mut rig = Rig::new_with(11, None);
    let g = rig.generation;
    exchange(&mut rig, 50, 0, &[(0, group(g, 50, &[0, 1]))], 1);
    let late = 50 + REFRESH_TICKS + 1;
    let state = group(g, late, &[0, 1]);
    assert_eq!(
        exchange(&mut rig, late, 0, &[(0, state.clone())], 1),
        [state]
    );
    let st = host_states(&rig);
    assert_eq!(
        (st.sent_full, st.sent_delta),
        (2, 0),
        "refreshed, not deltaed"
    );
}

#[test]
fn groups_keep_independent_baselines() {
    let mut rig = Rig::new_with(12, None);
    let g = rig.generation;
    let both = |t: u64| {
        vec![
            (0u8, group(g, t, &[0, 1])),
            (1u8, group(g, t, &[2, 3, 4, 5, 6, 7])),
        ]
    };
    exchange(&mut rig, 60, 0, &both(60), 2);
    assert_eq!(host_states(&rig).sent_full, 2);

    // Tick 61 names only group 0, so only group 0 is acknowledged for it.
    exchange(&mut rig, 61, 0, &both(61)[..1], 1);
    eventually("the group acks", || {
        rig.host
            .lane_group_ack(rig.id, 0)
            .is_some_and(|a| a.tick == 61)
            && rig
                .host
                .lane_group_ack(rig.id, 1)
                .is_some_and(|a| a.tick == 60)
    });
    assert_eq!(host_states(&rig).sent_delta, 1);

    // Tick 62: group 0 deltas against 61, group 1 against its own 60.
    let got = exchange(&mut rig, 62, 0, &both(62), 2);
    assert_eq!(
        got,
        both(62).into_iter().map(|(_, m)| m).collect::<Vec<_>>()
    );
    let st = host_states(&rig);
    assert_eq!((st.sent_full, st.sent_delta), (2, 3));
    assert_eq!(rig.client.lane_states().unwrap().missing_base, 0);
}

#[test]
fn a_seeded_lossy_run_reconstructs_every_applied_group_exactly() {
    let mut rig = Rig::new_with(13, None);
    let g = rig.generation;
    rig.set_both(UdpImpair {
        link: Impair {
            loss: 0.2,
            duplicate: 0.1,
            reorder: 0.1,
            ..Impair::default()
        },
        ..UdpImpair::default()
    });
    let mut sent = std::collections::BTreeMap::new();
    let mut got = 0u64;
    for t in 100..400u64 {
        // A roster change every 97 ticks bumps the epoch.
        let epoch = ((t - 100) / 97) as u8;
        let groups = [
            (0u8, group(g, t, &[0, 1])),
            (1u8, group(g, t, &[2, 3, 4, 5, 6, 7])),
        ];
        for (k, m) in &groups {
            sent.insert((t, *k), m.clone());
        }
        let frags = rig.proxy.tagged(LinkDir::Down, FRAGMENT).0;
        rig.host
            .ctl()
            .send_snap_states_prio(rig.id, g, t, epoch, &groups, &[])
            .unwrap();
        eventually("the tick to leave the host", || {
            rig.proxy.tagged(LinkDir::Down, FRAGMENT).0 >= frags + 2
        });
        rig.settle(STEP);
        while rig.client.lane_inbox_stats().unwrap().len > 0 {
            let Message::Snap {
                tick, ref entries, ..
            } = next_snap(&mut rig)
            else {
                continue;
            };
            let group_id = u8::from(entries[0].player >= 2);
            let Message::Snap { entries: want, .. } = &sent[&(tick, group_id)] else {
                unreachable!()
            };
            assert_eq!(entries, want, "tick {tick} group {group_id} reconstructed");
            got += 1;
        }
    }
    let host = host_states(&rig);
    let client = rig.client.lane_states().unwrap();
    eprintln!("host {host:?}\nclient {client:?}");
    assert!(got > 300, "enough groups crossed: {got}");
    assert!(host.sent_delta > host.sent_full, "{host:?}");
    assert!(host.sent_bytes < host.raw_bytes, "{host:?}");
    assert!(host.sent_full >= 4, "an epoch bump refreshes: {host:?}");
    assert_eq!(client.malformed, 0);
}
