//! The UDP lane at the real datagram boundary (NETPLAY-06B): a lane client
//! and a host talk through [`UdpImpairProxy`], which drops, bursts,
//! reorders, duplicates, delays, MTU-limits and black-holes the actual
//! datagrams on a virtual clock.
//!
//! Every test runs in **lockstep**: after each send the test waits until
//! the proxy has read the datagram, advances virtual time, and waits until
//! the far side has counted every datagram the proxy released. Nothing
//! sleeps on a guess and nothing depends on machine speed, so a seed
//! replays the same loss pattern and the same assertions hold. (The
//! `legacy_tcp_frame_tests` in `impair.rs` cover the old frame-over-TCP
//! proxy; these do not use it.) The one real-time dependency is the
//! host's per-seat command flood bound, which refills on the wall clock:
//! [`TICK_PACE`] keeps a test's command rate under it, and every cell
//! asserts `rate_limited == 0` so a breach is loud.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::thread;
use std::time::{Duration, Instant};

use crate::lane_tests::{WAIT, cmd, eventually, hello_for, host, start_session};
use crate::{
    Client, Host, Impair, LinkDir, MAX_DATAGRAM, Message, NetError, RemoteInputs, SeatStats,
    SnapEntry, Source, TickCommand, UdpImpair, UdpImpairProxy,
};

/// Virtual time each lockstep step advances (one 60 Hz tick).
const STEP: Duration = Duration::from_millis(16);
/// Real time between ticks, keeping the command rate under the host's
/// wall-clock flood bound (`COMMAND_RATE_HZ`).
const TICK_PACE: Duration = Duration::from_millis(3);
/// Commands the host holds back before consuming a slot, so a window in
/// flight can land before its slot is due.
const LEAD: u64 = 4;

/// A host, one routed lane client, and the relay between them.
struct Rig {
    host: Host,
    client: Client,
    proxy: UdpImpairProxy,
    inputs: RemoteInputs,
    id: u16,
    generation: u64,
}

impl Rig {
    fn new(seed: u64) -> Self {
        let host = host();
        let target = SocketAddr::new(host.addr().ip(), host.lane_port());
        let proxy = UdpImpairProxy::loopback_manual(target, seed).unwrap();
        let mut client =
            Client::join_lane_routed(host.addr(), &hello_for("alice"), WAIT, Some(proxy.route()))
                .unwrap();
        client.set_timeout(Some(WAIT)).unwrap();
        let generation = start_session(&host, &mut [&mut client]);
        let inputs = host.remote_inputs();
        inputs.set_running_generation(Some(generation));
        let id = client.player_id();
        eventually("the lane", || host.lane_established(id));
        let rig = Self {
            host,
            client,
            proxy,
            inputs,
            id,
            generation,
        };
        rig.settle(STEP);
        rig
    }

    fn set_both(&self, recipe: UdpImpair) {
        self.proxy.set(LinkDir::Up, recipe);
        self.proxy.set(LinkDir::Down, recipe);
    }

    /// Queue one window `first..=last` (command `seq` carries `throttle =
    /// seq as u8`) and wait until the client's sender thread put it on the
    /// wire.
    fn send_window(&self, first: u64, last: u64) {
        let before = self.client.lane_counts().unwrap().sent;
        let commands = (first..=last).map(|s| cmd(s as u8)).collect();
        self.client
            .ctl()
            .unwrap()
            .send_input_window(self.generation, 0, first, commands)
            .unwrap();
        eventually("the window to leave the client", || {
            self.client.lane_counts().unwrap().sent == before + 1
        });
    }

    /// Queue one `Snap` for the lane seat carrying `ack` and wait until
    /// the host's sender thread put it on the wire.
    fn send_snap(&self, tick: u64, ack: u64) {
        let before = self.host.lane_counts().sent;
        let msg = Message::Snap {
            generation: self.generation,
            tick,
            entries: vec![SnapEntry {
                player: self.id,
                input_ack: ack,
                ..SnapEntry::default()
            }],
            trailers: Vec::new(),
            impacts: Vec::new(),
            race: None,
        };
        self.host.ctl().send_to(self.id, &msg).unwrap();
        eventually("the snap to leave the host", || {
            self.host.lane_counts().sent == before + 1
        });
    }

    /// The lockstep barrier: the proxy has read everything either side
    /// sent, virtual time moves by `by`, and both sides have received
    /// everything the proxy released.
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

    /// Every `Snap` the client has received since the last drain, as
    /// `(tick, ack)` in arrival order.
    fn drain(&mut self) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        while self.client.lane_inbox_stats().unwrap().len > 0 {
            if let Message::Snap { tick, entries, .. } = self.client.recv().unwrap() {
                out.push((tick, entries[0].input_ack));
            }
        }
        out
    }

    fn seat(&self) -> SeatStats {
        self.inputs.seat_stats(self.id).unwrap()
    }

    /// One fixed tick of the host: consume this seat's slot once it is
    /// `LEAD` commands behind the client.
    fn consume(&self, tick: u64) -> Option<(Source, Option<u64>, TickCommand)> {
        if tick <= LEAD {
            return None;
        }
        let c = self.inputs.consume_command(self.id)?;
        Some((c.source, c.finalized, c.command))
    }
}

#[derive(Debug, PartialEq)]
struct RunSummary {
    up: (u64, u64, u64, u64, u64),
    down: (u64, u64, u64, u64, u64),
    seat: (u64, u64, u64, u64, u64, u64),
    snaps: Vec<(u64, u64)>,
    ack: u64,
}

fn drops(rig: &Rig, dir: LinkDir) -> (u64, u64, u64, u64, u64) {
    let s = rig.proxy.stats(dir);
    (
        s.link.dropped,
        s.link.duplicated,
        s.link.reordered,
        rig.proxy.tagged(dir, 2).0,
        rig.proxy.tagged(dir, 2).1,
    )
}

/// 120 ticks of a client sending 4-command windows and a host answering
/// with ack-carrying snaps through 25% loss, duplication and reorder plus
/// 10-50 ms of delay in both directions, then a clean tail.
fn random_impairment_run(seed: u64) -> RunSummary {
    const N: u64 = 120;
    const CLEAN: u64 = 10;
    let mut rig = Rig::new(seed);
    rig.set_both(UdpImpair {
        link: Impair {
            delay: Duration::from_millis(10),
            jitter: Duration::from_millis(40),
            loss: 0.25,
            duplicate: 0.25,
            reorder: 0.25,
        },
        ..UdpImpair::default()
    });
    let mut snaps = Vec::new();
    for t in 1..=N + CLEAN {
        if t == N + 1 {
            // Impairment ends; whatever the relay still holds is released.
            rig.set_both(UdpImpair::default());
            rig.settle(Duration::from_secs(1));
            snaps.extend(rig.drain());
        }
        rig.send_window(t.saturating_sub(3).max(1), t);
        rig.consume(t);
        rig.send_snap(t, rig.inputs.command_ack(rig.id));
        rig.settle(STEP);
        snaps.extend(rig.drain());
        thread::sleep(TICK_PACE);
    }
    // The host keeps ticking until it has spent every command; the ack
    // converges on the last one the client sent.
    let mut last_tick = N + CLEAN;
    while rig.inputs.command_ack(rig.id) < N + CLEAN {
        last_tick += 1;
        assert!(last_tick <= N + CLEAN + 32, "the ack never converged");
        rig.consume(last_tick);
        rig.send_snap(last_tick, rig.inputs.command_ack(rig.id));
        rig.settle(STEP);
        snaps.extend(rig.drain());
    }
    rig.send_snap(last_tick + 1, N + CLEAN);
    rig.settle(STEP);
    snaps.extend(rig.drain());

    for dir in [LinkDir::Up, LinkDir::Down] {
        let s = rig.proxy.stats(dir).link;
        assert!(
            s.dropped > 0 && s.duplicated > 0 && s.reordered > 0,
            "{dir:?} {s:?}"
        );
    }
    let seat = rig.seat();
    assert_eq!(seat.rate_limited, 0, "{seat:?}");
    assert_eq!(seat.overflow, 0, "{seat:?}");
    assert!(seat.max_occupancy <= 16, "{seat:?}");
    assert!(
        rig.host.lane_counts().replayed > 0,
        "duplicates were refused"
    );
    assert!(
        rig.client.lane_counts().unwrap().replayed > 0,
        "duplicates were refused"
    );

    // Newer snapshot and ack progress continued past all the damage: the
    // newest tick got through, carrying the host's final ack.
    let newest = snaps.iter().map(|s| s.0).max().unwrap();
    assert_eq!(newest, last_tick + 1, "the last snapshot arrived");
    let ack = rig.inputs.command_ack(rig.id);
    assert_eq!(ack, N + CLEAN);
    assert_eq!(snaps.iter().map(|s| s.1).max().unwrap(), ack);
    // Most commands arrived (redundancy covers the rest), and a lost slot
    // was finalized rather than waited on.
    assert!(seat.received * 10 >= (N + CLEAN) * 9, "{seat:?}");
    RunSummary {
        up: drops(&rig, LinkDir::Up),
        down: drops(&rig, LinkDir::Down),
        seat: (
            seat.received,
            seat.duplicates,
            seat.old,
            seat.gap_finalized,
            seat.fallback,
            seat.finalized,
        ),
        snaps,
        ack,
    }
}

#[test]
fn snapshot_and_ack_progress_survive_loss_reorder_and_duplication_both_ways() {
    random_impairment_run(0x5eed);
}

#[test]
fn a_seed_replays_the_same_datagram_fate() {
    assert_eq!(random_impairment_run(7), random_impairment_run(7));
}

/// What [`burst_run`] measured: the sequences a model says survive, the
/// host's counters, each finalized slot, and the rig for further asserts.
struct BurstRun {
    survive: BTreeSet<u64>,
    stats: SeatStats,
    finalized: Vec<(u64, Source, TickCommand)>,
    rig: Rig,
}

/// Windows of `w` commands sent every `cadence` ticks for `ticks` ticks;
/// `k` consecutive windows starting at the `first_lost`-th are dropped on
/// the way to the host. Returns the sequences a model says survive, the
/// host's counters and what the host consumed per finalized sequence.
fn burst_run(cadence: u64, w: u64, ticks: u64, first_lost: u64, k: u64) -> BurstRun {
    let rig = Rig::new(1);
    let mut survive = BTreeSet::new();
    let mut finalized = Vec::new();
    let mut window = 0;
    let mut sent_windows = 0;
    for t in 1..=ticks {
        if t % cadence == 0 {
            window += 1;
            if window == first_lost {
                rig.proxy.drop_next(LinkDir::Up, k);
            }
            let (first, last) = ((t + 1).saturating_sub(w).max(1), t);
            rig.send_window(first, last);
            sent_windows += 1;
            if window < first_lost || window >= first_lost + k {
                survive.extend(first..=last);
            }
        }
        rig.settle(STEP);
        if let Some((source, Some(seq), command)) = rig.consume(t) {
            finalized.push((seq, source, command));
        }
        thread::sleep(TICK_PACE);
    }
    // Drain the lead: keep ticking the host until every command is spent
    // (starved ticks during the burst finalized nothing and owe a slot).
    let mut t = ticks;
    while rig.inputs.command_ack(rig.id) < ticks {
        t += 1;
        assert!(t <= ticks + 3 * LEAD + k + 8, "the ack never converged");
        if let Some((source, Some(seq), command)) = rig.consume(t) {
            finalized.push((seq, source, command));
        }
    }
    assert_eq!(
        rig.inputs.windows_received(rig.id),
        sent_windows - k,
        "exactly the burst never reached the host"
    );
    assert_eq!(rig.proxy.stats(LinkDir::Up).burst_dropped, k);
    let stats = rig.seat();
    BurstRun {
        survive,
        stats,
        finalized,
        rig,
    }
}

/// The burst-loss recipes of `docs/netplay-timeline.md`: a command is
/// recovered iff one of the **distinct datagrams** carrying it survives.
/// Copies packed into one datagram count once, so redundancy is measured
/// in transmissions over time, not in window length.
#[test]
fn redundancy_spans_distinct_transmissions_so_bursts_shorter_than_it_cost_nothing() {
    // (cadence ticks, window len, burst length in datagrams, commands lost)
    let cells: [(u64, u64, u64, u64); 8] = [
        // 4 transmissions per command; a 3-datagram burst misses none
        (1, 4, 3, 0),
        // a burst as long as the redundancy loses exactly one command
        (1, 4, 4, 1),
        // longer bursts lose burst - redundancy + 1 commands
        (1, 4, 6, 3),
        // the production cadence (a window every 2 ticks, 4 transmissions
        // per command, 2 new commands each): a 3-burst misses none ...
        (2, 8, 3, 0),
        // ... a 4-burst loses the 2 commands only those 4 windows carried
        (2, 8, 4, 2),
        (2, 8, 6, 6),
        // one transmission per command: a single loss costs a whole window
        (8, 8, 1, 8),
        // control: no loss
        (1, 4, 0, 0),
    ];
    for (cadence, w, k, lost_expected) in cells {
        let ticks = 64;
        let BurstRun {
            survive,
            stats,
            finalized,
            rig,
        } = burst_run(cadence, w, ticks, 6, k);
        let label = format!("cadence {cadence} window {w} burst {k}");
        let all: BTreeSet<u64> = (1..=ticks).collect();
        let lost: Vec<u64> = all.difference(&survive).copied().collect();
        assert_eq!(lost.len() as u64, lost_expected, "{label}: model {lost:?}");
        assert_eq!(stats.received, survive.len() as u64, "{label}: {stats:?}");
        assert_eq!(stats.gap_finalized, lost_expected, "{label}: {stats:?}");
        assert_eq!(stats.rate_limited, 0, "{label}: {stats:?}");
        assert_eq!(stats.overflow, 0, "{label}: {stats:?}");
        // The ack walked over every sequence, lost or not.
        assert_eq!(rig.inputs.command_ack(rig.id), ticks, "{label}: {stats:?}");
        // A recovered command was applied as the driver sent it; a lost
        // one expired into a substitute and was never applied twice.
        let seqs: Vec<u64> = finalized.iter().map(|f| f.0).collect();
        assert_eq!(seqs, (1..=ticks).collect::<Vec<_>>(), "{label}");
        for (seq, source, command) in finalized {
            if lost.contains(&seq) {
                assert_ne!(source, Source::Command, "{label}: seq {seq}");
            } else {
                assert_eq!(
                    (source, command),
                    (Source::Command, cmd(seq as u8)),
                    "{label}"
                );
            }
        }
    }
}

#[test]
fn a_ninety_snapshot_burst_on_the_return_path_does_not_stall_ack_progress() {
    let mut rig = Rig::new(2);
    let mut got = Vec::new();
    for t in 1..=150u64 {
        if t == 11 {
            rig.proxy.drop_next(LinkDir::Down, 90);
        }
        rig.send_window(t.saturating_sub(3).max(1), t);
        rig.settle(STEP);
        rig.consume(t);
        rig.send_snap(t, rig.inputs.command_ack(rig.id));
        rig.settle(STEP);
        got.extend(rig.drain());
        thread::sleep(TICK_PACE);
    }
    let ticks: Vec<u64> = got.iter().map(|s| s.0).collect();
    let expected: Vec<u64> = (1..=10).chain(101..=150).collect();
    assert_eq!(ticks, expected, "exactly the burst of 90 vanished");
    assert!(
        got.windows(2).all(|p| p[0].1 <= p[1].1),
        "the ack never went backwards"
    );
    let ack = rig.inputs.command_ack(rig.id);
    assert_eq!(
        got.last().unwrap().1,
        ack,
        "the ack caught up after the burst"
    );
    assert_eq!(ack, 150 - LEAD);
    let down = rig.proxy.stats(LinkDir::Down);
    assert_eq!(down.burst_dropped, 90);
    assert_eq!(rig.proxy.tagged(LinkDir::Down, 2), (150, 60));
    assert_eq!(rig.seat().rate_limited, 0);
    assert_eq!(rig.seat().gap_finalized, 0, "the up path was clean");
}

#[test]
fn duplicated_datagrams_are_refused_by_the_replay_window_in_both_directions() {
    let mut rig = Rig::new(3);
    rig.set_both(UdpImpair {
        link: Impair {
            duplicate: 1.0,
            ..Impair::default()
        },
        ..UdpImpair::default()
    });
    let base = (
        rig.host.lane_counts().replayed,
        rig.client.lane_counts().unwrap().replayed,
    );
    let mut got = Vec::new();
    for t in 1..=10 {
        rig.send_window(t, t);
        rig.send_snap(t, 0);
        rig.settle(STEP);
        got.extend(rig.drain());
    }
    assert_eq!(
        got.iter().map(|s| s.0).collect::<Vec<_>>(),
        (1..=10).collect::<Vec<_>>()
    );
    assert_eq!(rig.inputs.windows_received(rig.id), 10, "each window once");
    assert_eq!(rig.seat().received, 10);
    assert_eq!(rig.host.lane_counts().replayed - base.0, 10);
    assert_eq!(rig.client.lane_counts().unwrap().replayed - base.1, 10);
    assert_eq!(rig.proxy.tagged(LinkDir::Up, 2), (10, 20));
    assert_eq!(rig.proxy.tagged(LinkDir::Down, 2), (10, 20));
}

#[test]
fn reordered_datagrams_arrive_swapped_and_newer_state_still_wins() {
    let mut rig = Rig::new(4);
    rig.set_both(UdpImpair {
        link: Impair {
            reorder: 1.0,
            ..Impair::default()
        },
        ..UdpImpair::default()
    });
    let mut got = Vec::new();
    for t in 1..=10u64 {
        rig.send_window(t.saturating_sub(3).max(1), t);
        rig.send_snap(t, 0);
        rig.settle(STEP);
        got.extend(rig.drain());
    }
    assert_eq!(
        got.iter().map(|s| s.0).collect::<Vec<_>>(),
        [2, 1, 4, 3, 6, 5, 8, 7, 10, 9],
        "every pair swapped on the wire"
    );
    assert_eq!(rig.proxy.stats(LinkDir::Down).link.reordered, 5);
    // The host got the windows swapped too and lost nothing: gap-filling
    // sequences inside the buffer are accepted by seq.
    assert_eq!(rig.inputs.windows_received(rig.id), 10);
    assert_eq!(rig.seat().received, 10);
    assert_eq!(rig.client.lane_counts().unwrap().replayed, 0);
}

#[test]
fn delay_holds_datagrams_until_virtual_time_reaches_them() {
    let rig = Rig::new(5);
    rig.set_both(UdpImpair {
        link: Impair {
            delay: Duration::from_millis(50),
            ..Impair::default()
        },
        ..UdpImpair::default()
    });
    rig.send_window(1, 1);
    rig.settle(Duration::from_millis(49));
    assert_eq!(rig.inputs.windows_received(rig.id), 0, "still in flight");
    assert_eq!(rig.proxy.tagged(LinkDir::Up, 2), (1, 0));
    rig.settle(Duration::from_millis(1));
    eventually("the delayed window", || {
        rig.inputs.windows_received(rig.id) == 1
    });
    assert_eq!(rig.proxy.stats(LinkDir::Up).link.delayed, 1);
}

#[test]
fn an_effective_mtu_below_a_full_datagram_is_a_lane_timeout_naming_the_mtu() {
    let host = host();
    let target = SocketAddr::new(host.addr().ip(), host.lane_port());
    for (dir, label) in [
        (LinkDir::Up, "client to host"),
        (LinkDir::Down, "host to client"),
    ] {
        let proxy = UdpImpairProxy::loopback_manual(target, 6).unwrap();
        proxy.set(
            dir,
            UdpImpair {
                mtu: Some(MAX_DATAGRAM - 1),
                ..UdpImpair::default()
            },
        );
        let started = Instant::now();
        let err = Client::join_lane_routed(
            host.addr(),
            &hello_for("alice"),
            Duration::from_millis(500),
            Some(proxy.route()),
        )
        .map(|_| ())
        .unwrap_err();
        assert!(started.elapsed() < WAIT, "{label}: bounded, not a hang");
        assert!(
            matches!(err, NetError::LaneTimeout { .. }),
            "{label}: {err:?}"
        );
        let text = err.to_string();
        assert!(
            text.contains("MTU") && text.contains("UDP"),
            "{label}: {text}"
        );
        assert!(proxy.stats(dir).mtu_dropped >= 1, "{label}");
        assert_eq!(
            proxy.stats(dir).link.frames_out,
            0,
            "{label}: nothing crossed"
        );
    }
    // The same path at exactly the datagram budget establishes.
    let proxy = UdpImpairProxy::loopback_manual(target, 6).unwrap();
    for dir in [LinkDir::Up, LinkDir::Down] {
        proxy.set(
            dir,
            UdpImpair {
                mtu: Some(MAX_DATAGRAM),
                ..UdpImpair::default()
            },
        );
    }
    let client =
        Client::join_lane_routed(host.addr(), &hello_for("bob"), WAIT, Some(proxy.route()))
            .unwrap();
    eventually("the lane", || host.lane_established(client.player_id()));
    assert_eq!(proxy.stats(LinkDir::Up).mtu_dropped, 0);
}

#[test]
fn a_black_hole_never_blocks_the_sender_and_is_counted() {
    let rig = Rig::new(8);
    rig.proxy.set(
        LinkDir::Up,
        UdpImpair {
            blackhole: true,
            ..UdpImpair::default()
        },
    );
    let started = Instant::now();
    let ctl = rig.client.ctl().unwrap();
    for t in 1..=200 {
        ctl.send_input_window(rig.generation, 0, t, vec![cmd(t as u8)])
            .unwrap();
        let q = ctl.lane_queue().unwrap();
        assert!(q.len <= q.capacity, "{q:?}");
    }
    assert!(
        started.elapsed() < WAIT,
        "sending into a black hole is bounded"
    );
    let sent_total = || rig.client.lane_counts().unwrap().sent;
    eventually("the sender thread to drain", || {
        ctl.lane_queue().unwrap().len == 0
            && rig.proxy.stats(LinkDir::Up).link.frames_in == sent_total()
    });
    let up = rig.proxy.stats(LinkDir::Up);
    assert!(up.blackholed >= 1 && up.link.frames_out == up.link.frames_in - up.blackholed);
    assert_eq!(rig.inputs.windows_received(rig.id), 0, "nothing arrived");
    // The control stream is a separate path and is unaffected.
    assert!(rig.host.lane_established(rig.id));
}
