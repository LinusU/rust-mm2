//! Lobby-scope reliable messages through the real datagram boundary
//! (NETPLAY-06E-2a): no session runs, so the host's lane generation is 0
//! and the reliable datagrams name generation 0. The relay drops from the
//! first datagram, and the first significant message is lost on purpose.

use std::thread;
use std::time::Duration;

use crate::lane_impair_tests::{STEP, TICK_PACE};
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
