//! World catch-up off the control stream (NETPLAY-10C-3), through the
//! seeded lossy datagram relay. `Props` rides the lane's reliable channel
//! (its late-join snapshot is committed as delivered), `Traffic` and
//! `Signals` ride bulk fragments that the next frame supersedes.

use std::thread;
use std::time::Duration;

use crate::lane_impair_tests::STEP;
use crate::lane_reliable_tests::{Lobby, quiesce, rig, round};
use crate::{
    LinkDir, MAX_SNAP_PROPS, Message, Priority, SNAP_NO_FRAGMENT, SiteTable, SnapCar, SnapImpact,
    SnapProp, SnapSignal,
};

fn props(generation: u64, tick: u64) -> Message {
    Message::Props {
        generation,
        tick,
        table: SiteTable {
            count: 1000,
            digest: 0xfeed,
        },
        // A full frame: several datagrams on the reliable channel.
        rows: (0..u32::from(MAX_SNAP_PROPS))
            .map(|site| SnapProp {
                site: site + tick as u32 * 1000,
                fragment: SNAP_NO_FRAGMENT,
                phase: 1,
                pos: [site as f32, 0.0, 0.0],
                rot: [0.0, 0.0, 0.0, 1.0],
            })
            .collect(),
    }
}

fn traffic(generation: u64, tick: u64) -> Message {
    Message::Traffic {
        generation,
        tick,
        roster: 7,
        // Over a datagram: it is a multi-fragment group.
        rows: (0..60)
            .map(|id| SnapCar {
                id,
                class: 0,
                state: 1,
                pos: [id as f32, 0.0, tick as f32],
                rot: [0.0, 0.0, 0.0, 1.0],
                vel: [0.0; 3],
            })
            .collect(),
    }
}

fn signals(generation: u64, tick: u64) -> Message {
    Message::Signals {
        generation,
        tick,
        rows: (0..300)
            .map(|junction| SnapSignal { junction, green: 1 })
            .collect(),
    }
}

fn impact(id: u64) -> Message {
    Message::Impacts {
        generation: 0,
        impacts: vec![SnapImpact {
            seat: 0,
            id,
            tick: id,
            epoch: 0,
            ordinal: 0,
            point: [0.0; 3],
            normal: [0.0; 3],
            severity: 1.0,
            audio_id: 0,
        }],
    }
}

/// Run rounds until `done`, failing after as many as `pump` allows.
fn until(lobby: &mut Lobby, what: &str, mut done: impl FnMut(&mut Lobby) -> bool) {
    for _ in 0..600 {
        if done(lobby) {
            return;
        }
        round(lobby);
    }
    assert!(done(lobby), "never converged: {what}");
}

/// Everything the client's inbox holds right now.
fn drain(lobby: &mut Lobby) -> Vec<Message> {
    lobby
        .client
        .set_timeout(Some(Duration::from_millis(5)))
        .unwrap();
    let mut out = Vec::new();
    while let Ok(m) = lobby.client.recv() {
        out.push(m);
    }
    out
}

fn ticks(msgs: &[Message], pick: fn(&Message) -> Option<u64>) -> Vec<u64> {
    msgs.iter().filter_map(pick).collect()
}

fn props_tick(m: &Message) -> Option<u64> {
    matches!(m, Message::Props { .. }).then(|| match m {
        Message::Props { tick, .. } => *tick,
        _ => unreachable!(),
    })
}

fn traffic_tick(m: &Message) -> Option<u64> {
    match m {
        Message::Traffic { tick, .. } => Some(*tick),
        _ => None,
    }
}

fn signals_tick(m: &Message) -> Option<u64> {
    match m {
        Message::Signals { tick, .. } => Some(*tick),
        _ => None,
    }
}

#[test]
fn the_props_late_join_snapshot_completes_exactly_once_under_loss_and_reorder() {
    const FRAMES: u64 = 6;
    let mut lobby = rig(0xA11, 0.2);
    let (id, g) = (lobby.id, lobby.generation);
    // The first datagrams of the transfer are lost on purpose.
    lobby.proxy.drop_next(LinkDir::Down, 3);
    let ctl = lobby.host.ctl();
    for tick in 1..=FRAMES {
        // Never the control stream for a lane seat: a refusal is an error
        // the producer retries, so the owed mark stays set.
        let mut tries = 0;
        while ctl.send_props(id, g, &props(g, tick)).is_err() {
            tries += 1;
            assert!(tries < 600, "the reliable window never opened");
            round(&lobby);
        }
    }
    let mut got = Vec::new();
    until(&mut lobby, "every props frame", |lobby| {
        got.extend(drain(lobby));
        ticks(&got, props_tick).len() as u64 >= FRAMES
    });
    quiesce(&lobby, 40);
    got.extend(drain(&mut lobby));
    assert_eq!(
        ticks(&got, props_tick),
        (1..=FRAMES).collect::<Vec<_>>(),
        "each frame once, in order"
    );
    let s = lobby.proxy.stats(LinkDir::Down).link;
    assert!(
        s.dropped > 0 && s.duplicated > 0 && s.reordered > 0,
        "{s:?}"
    );
    let host = lobby.host.reliable_stats(id).unwrap().sender;
    assert!(host.retransmits > 0, "{host:?}");
    assert_eq!(host.in_flight, 0, "{host:?}");
    // Nothing went over the control stream: the lane carried it all.
    assert!(
        lobby.client.reliable_stats().unwrap().receiver.delivered >= lobby.base + FRAMES,
        "props rode the reliable channel"
    );
}

#[test]
fn traffic_and_signals_recover_from_loss_on_the_next_frame() {
    const FRAMES: u64 = 24;
    let mut lobby = rig(0xB22, 0.3);
    let (id, g) = (lobby.id, lobby.generation);
    let ctl = lobby.host.ctl();
    let mut got = Vec::new();
    for tick in 1..=FRAMES {
        ctl.send_world_state(id, &traffic(g, tick)).unwrap();
        ctl.broadcast_world_state(&signals(g, 1000 + tick)).unwrap();
        round(&lobby);
        got.extend(drain(&mut lobby));
        // The pacer's byte budget refills on the wall clock: offer at
        // the cadence the game does (here a frame every 120 ms, inside
        // the world allowance) rather than all at once.
        thread::sleep(Duration::from_millis(120));
    }
    // The link heals; one more frame of each lands whole.
    lobby.set_both(crate::UdpImpair::default());
    lobby.settle(Duration::from_secs(1));
    // Let the world allowance refill before the frames that must land.
    thread::sleep(Duration::from_millis(400));
    ctl.send_world_state(id, &traffic(g, FRAMES + 1)).unwrap();
    ctl.broadcast_world_state(&signals(g, 1000 + FRAMES + 1))
        .unwrap();
    until(&mut lobby, "the healed frames", |lobby| {
        got.extend(drain(lobby));
        ticks(&got, traffic_tick).contains(&(FRAMES + 1))
            && ticks(&got, signals_tick).contains(&(1000 + FRAMES + 1))
    });
    let cars = ticks(&got, traffic_tick);
    let lights = ticks(&got, signals_tick);
    for (name, t) in [("traffic", &cars), ("signals", &lights)] {
        let mut unique = t.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), t.len(), "{name}: no frame twice {t:?}");
        assert!(
            (t.len() as u64) < FRAMES + 1,
            "{name}: loss dropped some {t:?}"
        );
        assert!(t.len() >= 5, "{name}: later frames recovered {t:?}");
    }
    assert!(lobby.proxy.stats(LinkDir::Down).link.dropped > 0);
}

#[test]
fn reliable_events_complete_once_while_a_large_world_transfer_is_in_flight() {
    let mut lobby = rig(0xC33, 0.2);
    let (id, g) = (lobby.id, lobby.generation);
    let ctl = lobby.host.ctl();
    for tick in 1..=4 {
        ctl.send_props(id, g, &props(g, tick)).unwrap();
    }
    for n in 1..=3 {
        ctl.send_world_state(id, &traffic(g, n)).unwrap();
    }
    let mut ev = impact(77);
    if let Message::Impacts { generation, .. } = &mut ev {
        *generation = g;
    }
    let fan = ctl.reliable_broadcast(g, &ev).unwrap();
    assert_eq!((fan.reliable, fan.fell_back), (1, 0));
    let mut got = Vec::new();
    until(&mut lobby, "the impact and the props", |lobby| {
        got.extend(drain(lobby));
        got.iter().any(|m| matches!(m, Message::Impacts { .. }))
            && ticks(&got, props_tick).len() == 4
    });
    quiesce(&lobby, 40);
    got.extend(drain(&mut lobby));
    let impacts = got
        .iter()
        .filter(|m| matches!(m, Message::Impacts { .. }))
        .count();
    assert_eq!(impacts, 1, "the event is applied once");
    assert_eq!(ticks(&got, props_tick), vec![1, 2, 3, 4]);
}

/// Bulk world state never holds a healthy seat's movement: with the link
/// clean, every movement group is delivered and none is deferred, expired
/// or dropped while props, cars and signals stream alongside.
#[test]
fn movement_is_unaffected_by_a_world_catch_up_in_flight() {
    const TICKS: u64 = 60;
    let mut lobby = rig(0xD44, 0.0);
    let (id, g) = (lobby.id, lobby.generation);
    lobby.set_both(crate::UdpImpair::default());
    let before = lobby.host.lane_pace(id).unwrap();
    let ctl = lobby.host.ctl();
    let mut movement = Vec::new();
    for tick in 1..=TICKS {
        // A late-join catch-up of four full props frames, then ambient
        // cars and lights at a fraction of the tick rate.
        if tick <= 4 {
            ctl.send_props(id, g, &props(g, tick)).unwrap();
        }
        if tick % 6 == 0 {
            ctl.send_world_state(id, &traffic(g, tick)).unwrap();
            ctl.send_world_state(id, &signals(g, tick)).unwrap();
        }
        let snap = Message::Snap {
            generation: g,
            tick,
            entries: vec![crate::SnapEntry::default()],
            trailers: Vec::new(),
            impacts: Vec::new(),
            race: None,
        };
        ctl.send_snap_groups_prio(id, g, tick, &[snap], &[Priority::Movement])
            .unwrap();
        round(&lobby);
        movement.extend(drain(&mut lobby).into_iter().filter_map(|m| match m {
            Message::Snap { tick, .. } => Some(tick),
            _ => None,
        }));
        lobby.settle(STEP);
        thread::sleep(Duration::from_millis(14));
    }
    assert_eq!(
        movement,
        (1..=TICKS).collect::<Vec<_>>(),
        "every movement group arrived, in order"
    );
    let after = lobby.host.lane_pace(id).unwrap();
    assert!(after.bulk_sent > before.bulk_sent, "{after:?}");
    assert!(after.world_sent > before.world_sent, "{after:?}");
    assert_eq!(
        after.movement.dropped + after.movement.expired,
        before.movement.dropped + before.movement.expired,
        "{after:?}"
    );
}
