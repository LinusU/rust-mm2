//! Lobby-scope reliable messages through the real datagram boundary
//! (NETPLAY-06E-2a): no session runs, so the host's lane generation is 0
//! and the reliable datagrams name generation 0. The relay drops from the
//! first datagram, and the first significant message is lost on purpose.

use std::thread;
use std::time::Duration;

use crate::lane_impair_tests::{STEP, TICK_PACE};
use crate::lane_tests::start_session;
use crate::lane_tests::{WAIT, eventually, hello_for, host};
use crate::{Client, Host, Impair, LinkDir, Message, RosterEntry, UdpImpair, UdpImpairProxy};

const MAX_ROUNDS: u32 = 600;

struct LobbyRig {
    host: Host,
    client: Client,
    proxy: UdpImpairProxy,
    id: u16,
}

impl LobbyRig {
    fn new(seed: u64, loss: f64) -> Self {
        let host = host();
        let target = std::net::SocketAddr::new(host.addr().ip(), host.lane_port());
        let proxy = UdpImpairProxy::loopback_manual(target, seed).unwrap();
        let recipe = UdpImpair {
            link: Impair {
                loss,
                duplicate: 0.25,
                ..Impair::default()
            },
            ..UdpImpair::default()
        };
        proxy.set(LinkDir::Up, recipe);
        proxy.set(LinkDir::Down, recipe);
        let client =
            Client::join_lane_routed(host.addr(), &hello_for("alice"), WAIT, Some(proxy.route()))
                .unwrap();
        client.set_timeout(Some(WAIT)).unwrap();
        let id = client.player_id();
        eventually("the lane", || host.lane_established(id));
        host.use_manual_reliable_clock();
        client.use_manual_reliable_clock();
        Self {
            host,
            client,
            proxy,
            id,
        }
    }

    fn settle(&self, by: Duration) {
        eventually("the proxy to read what was sent", || {
            self.proxy.stats(LinkDir::Up).link.frames_in == self.client.lane_counts().unwrap().sent
                && self.proxy.stats(LinkDir::Down).link.frames_in == self.host.lane_counts().sent
        });
        assert!(self.proxy.advance(by), "the relay never caught up");
        eventually("both sides to receive what the proxy released", || {
            self.host.lane_counts().received == self.proxy.stats(LinkDir::Up).link.frames_out
                && self.client.lane_counts().unwrap().received
                    == self.proxy.stats(LinkDir::Down).link.frames_out
        });
    }

    fn round(&self) {
        self.host.advance_reliable_clock(STEP);
        self.client.advance_reliable_clock(STEP);
        self.settle(STEP);
        thread::sleep(TICK_PACE);
    }

    fn pump(&self, what: &str, mut done: impl FnMut() -> bool) {
        for _ in 0..MAX_ROUNDS {
            if done() {
                return;
            }
            self.round();
        }
        assert!(done(), "never converged: {what}");
    }

    fn quiesce(&self, n: u32) {
        for _ in 0..n {
            self.round();
        }
    }

    fn client_delivered(&self) -> u64 {
        self.client.reliable_stats().unwrap().receiver.delivered
    }
}

#[test]
fn a_lobby_scope_message_each_way_survives_a_lost_first_datagram() {
    let rig = LobbyRig::new(21, 0.15);
    rig.proxy.drop_next(LinkDir::Up, 1);
    rig.client.reliable_send(0, b"hello host").unwrap();
    let mut got = Vec::new();
    rig.pump("the client's message", || {
        got.extend(std::iter::from_fn(|| rig.host.reliable_recv()));
        !got.is_empty()
    });
    rig.quiesce(40);
    got.extend(std::iter::from_fn(|| rig.host.reliable_recv()));
    assert_eq!(got, vec![(rig.id, b"hello host".to_vec())]);
    assert!(rig.client.reliable_stats().unwrap().sender.retransmits >= 1);

    rig.proxy.drop_next(LinkDir::Down, 1);
    rig.host.reliable_send(rig.id, 0, b"hello client").unwrap();
    let mut back = Vec::new();
    rig.pump("the host's message", || {
        back.extend(std::iter::from_fn(|| rig.client.reliable_recv()));
        !back.is_empty()
    });
    rig.quiesce(40);
    back.extend(std::iter::from_fn(|| rig.client.reliable_recv()));
    assert_eq!(back, vec![b"hello client".to_vec()]);
    assert_eq!(rig.host.reliable_stats(rig.id).unwrap().sender.acked, 1);
}

#[test]
fn a_reliable_datagram_naming_a_generation_that_does_not_run_is_still_refused() {
    let rig = LobbyRig::new(22, 0.0);
    let before = rig.host.lane_counts().wrong_generation;
    rig.client.reliable_send(7, b"stale").unwrap();
    eventually("the refusal", || {
        rig.host.lane_counts().wrong_generation > before
    });
    rig.quiesce(5);
    assert_eq!(rig.host.reliable_recv(), None);
}

fn recv_roster(client: &mut Client) -> Vec<RosterEntry> {
    loop {
        if let Message::Roster { players } = client.recv().unwrap() {
            return players;
        }
    }
}

#[test]
fn a_roster_change_and_a_removal_reach_the_lane_client_reliably_and_apply_once() {
    let mut rig = LobbyRig::new(23, 0.15);
    // Everything the client was told while joining.
    while rig.client.lane_inbox_stats().unwrap().len > 0 {
        rig.client.recv().unwrap();
    }
    assert_eq!(rig.client_delivered(), 0);

    // The reliable datagram of the join's roster is lost on purpose.
    rig.proxy.drop_next(LinkDir::Down, 1);
    let bob = Client::join(rig.host.addr(), &hello_for("bob")).unwrap();
    bob.set_timeout(Some(WAIT)).unwrap();
    rig.pump("the join roster", || rig.client_delivered() >= 1);
    assert!(rig.host.reliable_stats(rig.id).unwrap().sender.retransmits >= 1);
    while recv_roster(&mut rig.client).len() != 2 {}

    // Bob leaves: the removal is a roster without him.
    bob.leave().unwrap();
    rig.pump("the removal roster", || rig.client_delivered() >= 2);
    rig.quiesce(40);
    assert_eq!(rig.client_delivered(), 2, "applied exactly once each");
    let mut last = Vec::new();
    while rig.client.lane_inbox_stats().unwrap().len > 0 || last.len() != 1 {
        last = recv_roster(&mut rig.client);
    }
    assert_eq!(last.len(), 1);
    assert_eq!(last[0].player_id, rig.id);
    let host = rig.host.reliable_stats(rig.id).unwrap();
    assert_eq!((host.sender.acked, host.sender.in_flight), (2, 0));
}

fn host_events(rig: &LobbyRig) -> Vec<crate::HostEvent> {
    std::iter::from_fn(|| rig.host.try_recv().ok()).collect()
}

/// NETPLAY-06E-2c: the reliable copy alone (the TCP `Leave` is never sent)
/// reaps the seat though its first datagram is lost, and a second copy of
/// the same `Leave` is a no-op.
#[test]
fn a_leave_on_the_reliable_channel_reaps_the_seat_once_after_a_lost_datagram() {
    let rig = LobbyRig::new(24, 0.15);
    let mut events = host_events(&rig);
    let leave = Message::Leave.encode().unwrap();
    rig.proxy.drop_next(LinkDir::Up, 1);
    rig.client.reliable_send(0, &leave).unwrap();
    rig.client.reliable_send(0, &leave).unwrap();
    rig.pump("the seat to be reaped", || {
        events.extend(host_events(&rig));
        events
            .iter()
            .any(|e| matches!(e, crate::HostEvent::Left { .. }))
    });
    rig.quiesce(40);
    events.extend(host_events(&rig));
    let left: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            crate::HostEvent::Left { id, cause, .. } => Some((*id, *cause)),
            _ => None,
        })
        .collect();
    assert_eq!(left, vec![(rig.id, crate::LeaveCause::Quit)]);
    assert!(rig.client.reliable_stats().unwrap().sender.retransmits >= 1);
}

/// NETPLAY-06E-2c: ready / ready-again (the rematch intent) ride the
/// reliable channel alone on a lane client; the first datagram is lost,
/// both flags apply once and in order.
#[test]
fn ready_and_ready_again_survive_a_lost_datagram_and_apply_once_in_order() {
    let mut rig = LobbyRig::new(25, 0.15);
    let mut events = host_events(&rig);
    rig.proxy.drop_next(LinkDir::Up, 1);
    rig.client.set_ready(true).unwrap();
    rig.client.set_ready(false).unwrap();
    rig.client.set_ready(true).unwrap();
    let flags = |events: &[crate::HostEvent]| -> Vec<bool> {
        events
            .iter()
            .filter_map(|e| match e {
                crate::HostEvent::ReadyChanged { ready, .. } => Some(*ready),
                _ => None,
            })
            .collect()
    };
    rig.pump("all three flags", || {
        events.extend(host_events(&rig));
        flags(&events).len() >= 3
    });
    rig.quiesce(40);
    events.extend(host_events(&rig));
    assert_eq!(flags(&events), vec![true, false, true]);
    assert!(rig.client.reliable_stats().unwrap().sender.retransmits >= 1);
    assert_eq!(rig.client.reliable_stats().unwrap().sender.in_flight, 0);
}

/// Every `Start`/`Cancel` generation the client returns until the inbox
/// and the control stream have gone quiet, in the order returned.
fn drain_lifecycle(client: &mut Client) -> Vec<(&'static str, u64)> {
    client
        .set_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let mut seen = Vec::new();
    while let Ok(msg) = client.recv() {
        match msg {
            Message::Start { generation, .. } => seen.push(("start", generation)),
            Message::Cancel { generation } => seen.push(("cancel", generation)),
            _ => {}
        }
    }
    client.set_timeout(Some(WAIT)).unwrap();
    seen
}

#[test]
fn start_and_cancel_survive_a_lost_datagram_and_apply_once_in_order() {
    let mut rig = LobbyRig::new(26, 0.15);
    // Readiness and the pick are roster changes: let them settle so the
    // datagram dropped next is the `Start` itself.
    let started = start_session_dropping_first(&mut rig);
    let before = rig.client_delivered();
    assert!(rig.host.reliable_stats(rig.id).unwrap().sender.retransmits >= 1);

    rig.proxy.drop_next(LinkDir::Down, 1);
    rig.host.cancel().unwrap();
    // `Cancel`, then the roster it changed: both ride the channel.
    rig.pump("the cancel", || rig.client_delivered() >= before + 2);
    rig.quiesce(40);
    assert_eq!(rig.client_delivered(), before + 2, "applied exactly once");
    let host = rig.host.reliable_stats(rig.id).unwrap();
    assert_eq!(host.sender.in_flight, 0);

    // The TCP copy and its resend name the same generations, and the app
    // ignores a repeat; what matters is that `Start` is never behind its
    // `Cancel`.
    let seen = drain_lifecycle(&mut rig.client);
    assert_eq!(seen.first(), Some(&("start", started)));
    let cancel = seen.iter().position(|m| m.0 == "cancel").unwrap();
    assert!(seen[..cancel].iter().all(|m| m.0 == "start"), "{seen:?}");
}

fn start_session_dropping_first(rig: &mut LobbyRig) -> u64 {
    rig.host
        .set_session(crate::SessionAdvertisement {
            summary: "lane".to_string(),
            params: Vec::new(),
        })
        .unwrap();
    rig.client.set_vehicle("", 0).unwrap();
    rig.client.set_ready(true).unwrap();
    // The ready roster reaches the client reliably, and the pick's.
    rig.pump("the roster changes", || rig.client_delivered() >= 1);
    rig.quiesce(60);
    assert_eq!(rig.host.reliable_stats(rig.id).unwrap().sender.in_flight, 0);
    let settled = rig.client_delivered();
    rig.proxy.drop_next(LinkDir::Down, 1);
    let generation = start_session(&rig.host, &mut []);
    rig.pump("the start", || rig.client_delivered() > settled);
    rig.quiesce(40);
    assert_eq!(rig.client_delivered(), settled + 1, "applied exactly once");
    generation
}

/// #1930: a `Start` whose session blob is the largest the lobby accepts
/// spans several datagrams; the first piece is lost on purpose, the
/// channel repeats it, and the whole `Start` applies once on the channel.
#[test]
fn an_oversize_start_survives_a_lost_datagram_on_the_channel_and_applies_once() {
    let mut rig = LobbyRig::new(27, 0.15);
    rig.host
        .set_session(crate::SessionAdvertisement {
            summary: "big".to_string(),
            params: vec![7; crate::MAX_SESSION_PARAMS],
        })
        .unwrap();
    rig.client.set_vehicle("", 0).unwrap();
    rig.client.set_ready(true).unwrap();
    rig.pump("the roster changes", || rig.client_delivered() >= 1);
    rig.quiesce(60);
    assert_eq!(rig.host.reliable_stats(rig.id).unwrap().sender.in_flight, 0);
    let settled = rig.client_delivered();
    rig.proxy.drop_next(LinkDir::Down, 1);
    let queued = rig.host.reliable_stats(rig.id).unwrap().sender.queued;
    // `start_session` would replace the advertised blob with an empty one.
    let generation = loop {
        rig.host.start(crate::LateJoin::Open).unwrap();
        if let Ok(crate::HostEvent::Started { generation }) =
            rig.host.recv_timeout(Duration::from_millis(500))
        {
            break generation;
        }
    };
    rig.pump("the start", || rig.client_delivered() > settled);
    rig.quiesce(40);
    assert_eq!(rig.client_delivered(), settled + 1, "applied exactly once");
    let host = rig.host.reliable_stats(rig.id).unwrap();
    assert!(
        host.sender.queued >= queued + 4 && host.sender.retransmits >= 1,
        "{host:?}"
    );
    assert_eq!(host.sender.in_flight, 0);
    let seen = drain_lifecycle(&mut rig.client);
    assert_eq!(seen.first(), Some(&("start", generation)), "{seen:?}");
}

/// A client whose datagrams stop arriving while its TCP stream stays open
/// is reaped as `Lost` after the host's silence bound, and an idle client
/// whose probes do arrive is not.
#[test]
fn a_silent_lane_is_reaped_as_lost_and_an_idle_one_is_not() {
    use crate::{HostConfig, HostEvent, LeaveCause};
    let bound = Duration::from_millis(1500);
    let host = Host::listen_loopback(&HostConfig {
        liveness_timeout: bound,
        ..HostConfig::new(crate::lane_tests::FP)
    })
    .unwrap();
    let target = std::net::SocketAddr::new(host.addr().ip(), host.lane_port());
    let proxy = UdpImpairProxy::loopback_seeded(target, 7).unwrap();
    let mute = Client::join_lane_routed(host.addr(), &hello_for("mute"), WAIT, Some(proxy.route()))
        .unwrap();
    let idle = Client::join_lane(host.addr(), &hello_for("idle")).unwrap();
    let (mute_id, idle_id) = (mute.player_id(), idle.player_id());
    eventually("both lanes", || {
        host.lane_established(mute_id) && host.lane_established(idle_id)
    });
    // Black-hole the mute client's uplink; its TCP stream stays open.
    proxy.set(
        LinkDir::Up,
        UdpImpair {
            link: Impair {
                loss: 1.0,
                ..Impair::default()
            },
            ..UdpImpair::default()
        },
    );
    let deadline = std::time::Instant::now() + bound * 6;
    loop {
        if let Ok(HostEvent::Left { id, cause, .. }) = host.recv_timeout(Duration::from_millis(100))
        {
            assert_eq!(id, mute_id, "only the muted seat is reaped");
            assert_eq!(cause, LeaveCause::Lost);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the silent seat was never reaped"
        );
    }
    assert!(!host.lane_established(mute_id), "the lane seat is released");
    assert!(host.lane_established(idle_id), "the idle client stays");
    drop((mute, idle));
}
