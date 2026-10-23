//! NETPLAY-10C-1: partial same-tick `Snap` groups end to end. A host
//! sends every tick as two groups (the owner's seat, then the world's
//! seats) over a real UDP lane that runs through the seeded
//! [`UdpImpairProxy`] (loss, duplication, reordering). The client pushes
//! every `Snap` it receives into [`RemoteSnaps`] in arrival order, and the
//! outcome must be exactly what the per-seat freshness rule predicts: no
//! group is held for its sibling, a duplicate or an older tick for a seat
//! is a counted stale drop, and the staged state stays bounded.
//!
//! Evidence level: synthetic integration (in-process host and client,
//! loopback UDP through the seeded proxy, no retail data).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::thread;
use std::time::{Duration, Instant};

use mm2_app::netdrive::RemoteSnaps;
use mm2_net::{
    Client, Host, HostConfig, HostEvent, Impair, LateJoin, LinkDir, Message, SessionAdvertisement,
    SnapEntry, UdpImpair, UdpImpairProxy, hello,
};

const FINGERPRINT: u64 = 0x10C1;
const WAIT: Duration = Duration::from_secs(10);
const TICKS: u64 = 60;
/// The owner's seat is the lane client's own (wire id 1); the world group
/// carries two other seats.
const WORLD: [u16; 2] = [5, 6];

fn group(generation: u64, tick: u64, seats: &[u16]) -> Message {
    Message::Snap {
        generation,
        tick,
        entries: seats
            .iter()
            .map(|&player| SnapEntry {
                player,
                ..SnapEntry::default()
            })
            .collect(),
        trailers: Vec::new(),
        impacts: Vec::new(),
        race: None,
    }
}

/// Spin on `cond` until it holds or [`WAIT`] passes.
fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

fn start_session(host: &Host, client: &mut Client) -> u64 {
    host.set_session(SessionAdvertisement {
        summary: "partial".to_string(),
        params: Vec::new(),
    })
    .unwrap();
    client.set_vehicle("", 0).unwrap();
    client.send(&Message::SetReady { ready: true }).unwrap();
    let deadline = Instant::now() + WAIT;
    loop {
        host.start(LateJoin::Open).unwrap();
        while let Ok(event) = host.recv_timeout(Duration::from_millis(100)) {
            if let HostEvent::Started { generation } = event {
                return generation;
            }
        }
        assert!(Instant::now() < deadline, "the session never started");
    }
}

/// The freshness rule, restated independently of `RemoteSnaps`: a group
/// is stale when every seat it names has already seen an equal or newer
/// tick; otherwise it moves each named seat's mark forward.
fn model_stale(received: &[(u64, Vec<u16>)]) -> (u64, HashMap<u16, u64>) {
    let mut marks: HashMap<u16, u64> = HashMap::new();
    let mut stale = 0;
    for (tick, seats) in received {
        let fresh: Vec<u16> = seats
            .iter()
            .copied()
            .filter(|s| marks.get(s).is_none_or(|m| tick > m))
            .collect();
        if fresh.is_empty() {
            stale += 1;
        }
        for s in fresh {
            marks.insert(s, *tick);
        }
    }
    (stale, marks)
}

#[test]
fn partial_groups_over_a_lossy_reordering_lane_land_per_seat() {
    let host = Host::listen_loopback(&HostConfig::new(FINGERPRINT)).unwrap();
    let target = SocketAddr::new(host.addr().ip(), host.lane_port());
    let proxy = UdpImpairProxy::loopback_seeded(target, 0x10C1_0001).unwrap();
    let mut client = Client::join_lane_routed(
        host.addr(),
        &hello("partial".to_string(), "alice".to_string(), FINGERPRINT),
        WAIT,
        Some(proxy.route()),
    )
    .unwrap();
    client
        .set_timeout(Some(Duration::from_millis(600)))
        .unwrap();
    let generation = start_session(&host, &mut client);
    let id = client.player_id();
    eventually("the lane", || host.lane_established(id));

    // Loss starts once the lane and the session stand.
    let lossy = UdpImpair {
        link: Impair {
            loss: 0.15,
            duplicate: 0.20,
            reorder: 0.30,
            ..Impair::default()
        },
        ..UdpImpair::default()
    };
    proxy.set(LinkDir::Down, lossy);

    for tick in 1..=TICKS {
        let groups = [
            group(generation, tick, &[id]),
            group(generation, tick, &WORLD),
        ];
        assert!(
            host.ctl()
                .send_snap_groups(id, generation, tick, &groups)
                .unwrap(),
            "the lane seat takes the groups"
        );
        thread::sleep(Duration::from_millis(4));
    }

    // Everything the lane delivers, in arrival order, until it goes quiet.
    let mut snaps = RemoteSnaps::default();
    let mut received: Vec<(u64, Vec<u16>)> = Vec::new();
    while let Ok(msg) = client.recv() {
        if let Message::Snap {
            generation: g,
            tick,
            entries,
            trailers,
            impacts,
            race,
        } = msg
        {
            assert_eq!(g, generation);
            received.push((tick, entries.iter().map(|e| e.player).collect()));
            snaps.push(g, tick, entries, trailers, impacts, race);
        }
    }

    let (stale, marks) = model_stale(&received);
    assert!(
        received.len() as u64 > TICKS / 2,
        "the lane delivered {} of {} groups",
        received.len(),
        2 * TICKS
    );
    assert_eq!(snaps.stale_drops(), stale, "arrival order: {received:?}");
    for (seat, tick) in &marks {
        assert_eq!(snaps.seat_mark(*seat), Some((generation, *tick)));
    }
    // The owner's group is never held for the world's: its newest tick is
    // whatever arrived, not what the world groups allowed.
    let owner_newest = received
        .iter()
        .filter(|(_, seats)| seats == &[id])
        .map(|(tick, _)| *tick)
        .max()
        .expect("an owner group arrived");
    assert_eq!(snaps.seat_mark(id), Some((generation, owner_newest)));
    assert!(
        owner_newest >= TICKS / 2,
        "owner newest tick {owner_newest}"
    );
    // Bounded: one staged group per distinct tick at most, never a backlog.
    assert!(
        snaps.staged_groups() <= 3,
        "{} staged",
        snaps.staged_groups()
    );
    let stats = proxy.stats(LinkDir::Down);
    assert!(
        stats.link.dropped > 0,
        "the recipe really dropped datagrams"
    );
}
