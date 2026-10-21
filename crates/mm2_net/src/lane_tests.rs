//! Loopback tests for the UDP datagram lane (NETPLAY-06A): negotiation,
//! authentication, bounded handoff, and the host/join/late-join/
//! disconnect lifecycle through the lane. Loopback proves the code, not
//! WAN reachability (see `docs/multiplayer-reachability.md`).

use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::thread;
use std::time::{Duration, Instant};

use crate::lobby::Transport;
use crate::proto::{SnapEntry, TickCommand};
use crate::rt::{Header, LaneTag, encode_datagram};
use crate::{
    Client, Conn, Host, HostConfig, HostEvent, LateJoin, Message, NetError, PROTOCOL_VERSION,
    QUEUE_CAP, SessionAdvertisement, hello,
};

const FP: u64 = 0xaaaa;
const WAIT: Duration = Duration::from_secs(5);

fn host() -> Host {
    Host::listen_loopback(&HostConfig::new(FP)).unwrap()
}

fn hello_for(driver: &str) -> crate::Hello {
    hello("lane-test".to_string(), driver.to_string(), FP)
}

fn lane_client(host: &Host, driver: &str) -> Client {
    let c = Client::join_lane(host.addr(), &hello_for(driver)).unwrap();
    c.set_timeout(Some(WAIT)).unwrap();
    c
}

fn cmd(n: u8) -> TickCommand {
    TickCommand {
        throttle: n,
        brake: 0,
        steer: 0,
        handbrake: 0,
        auto_reverse: false,
        forced_gear: None,
    }
}

fn snap(generation: u64, tick: u64) -> Message {
    Message::Snap {
        generation,
        tick,
        entries: Vec::new(),
        trailers: Vec::new(),
        impacts: Vec::new(),
        race: None,
    }
}

/// Poll `cond` until it holds or [`WAIT`] passes.
fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// Start a session on `host` for the already-joined `clients`.
fn start_session(host: &Host, clients: &mut [&mut Client]) -> u64 {
    host.set_session(SessionAdvertisement {
        summary: "lane".to_string(),
        params: Vec::new(),
    })
    .unwrap();
    for c in clients.iter_mut() {
        c.set_vehicle("", 0).unwrap();
        c.set_ready(true).unwrap();
    }
    let deadline = Instant::now() + WAIT;
    loop {
        host.start(LateJoin::Open).unwrap();
        loop {
            match host.recv_timeout(Duration::from_millis(100)) {
                Ok(HostEvent::Started { generation }) => return generation,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        assert!(Instant::now() < deadline, "session never started");
    }
}

/// A hand-driven lane peer: a raw TCP control connection and a raw UDP
/// socket, so a test controls every datagram byte.
struct RawPeer {
    conn: Conn,
    udp: UdpSocket,
    host_udp: SocketAddr,
    seat: u16,
    token: u64,
    seq: u64,
}

impl RawPeer {
    fn join(host: &Host, driver: &str) -> Self {
        let mut conn = Conn::connect(host.addr()).unwrap();
        crate::send_hello(&mut conn, &hello_for(driver)).unwrap();
        conn.set_timeout(Some(WAIT)).unwrap();
        let seat = loop {
            if let Message::Welcome { player_id } = conn.recv().unwrap() {
                break player_id;
            }
        };
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        conn.send(&Message::LaneBind {
            port: udp.local_addr().unwrap().port(),
        })
        .unwrap();
        let (port, token) = loop {
            if let Message::LaneGrant { port, token } = conn.recv().unwrap() {
                break (port, token);
            }
        };
        assert_ne!(token, 0);
        assert_eq!(port, host.lane_port());
        Self {
            conn,
            udp,
            host_udp: SocketAddr::new(host.addr().ip(), port),
            seat,
            token,
            seq: 0,
        }
    }

    fn header(&mut self, tag: LaneTag, generation: u64) -> Header {
        self.seq += 1;
        Header {
            tag,
            generation,
            seat: self.seat,
            token: self.token,
            seq: self.seq,
        }
    }

    fn send_raw(&self, bytes: &[u8]) {
        self.udp.send_to(bytes, self.host_udp).unwrap();
    }

    fn send_header(&self, h: &Header, payload: &[u8]) {
        self.send_raw(&encode_datagram(h, payload).unwrap());
    }

    fn window_bytes(generation: u64, first_seq: u64) -> Vec<u8> {
        Message::InputWindow {
            generation,
            epoch: 0,
            first_seq,
            commands: vec![cmd(9)],
        }
        .encode()
        .unwrap()
    }

    /// Probe until the host echoes, so the seat is established.
    fn establish(&mut self, host: &Host) {
        let h = self.header(LaneTag::Probe, 0);
        self.send_header(&h, &[]);
        eventually("the lane to establish", || host.lane_established(self.seat));
    }
}

#[test]
fn join_lane_negotiates_a_token_and_establishes() {
    let host = host();
    let client = lane_client(&host, "alice");
    assert_eq!(client.transport(), Transport::Lane);
    assert_eq!(
        Client::join(host.addr(), &hello_for("bob"))
            .unwrap()
            .transport(),
        Transport::Tcp
    );
    eventually("the host to see the lane", || {
        host.lane_established(client.player_id())
    });
    let theirs = client.lane_counts().unwrap();
    assert!(theirs.accepted >= 1 && theirs.sent >= 1, "{theirs:?}");
    assert!(host.lane_counts().accepted >= 1);
}

#[test]
fn a_silent_udp_path_is_a_lane_timeout_naming_the_port() {
    // A host that completes the TCP handshake and grants a lane whose UDP
    // port never answers — what a blocked firewall looks like.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
    let silent_port = silent.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let mut conn = Conn::accept(&listener).unwrap();
        crate::accept_hello(&mut conn, FP).unwrap();
        conn.send(&Message::Welcome { player_id: 1 }).unwrap();
        assert!(matches!(conn.recv().unwrap(), Message::LaneBind { .. }));
        conn.send(&Message::LaneGrant {
            port: silent_port,
            token: 7,
        })
        .unwrap();
        // Hold the connection open until the client gives up.
        let _ = conn.recv();
    });
    let started = Instant::now();
    let err = Client::join_lane_within(addr, &hello_for("alice"), Duration::from_millis(400))
        .unwrap_err();
    assert!(started.elapsed() < WAIT, "the wait is bounded");
    let NetError::LaneTimeout { host, .. } = &err else {
        panic!("expected LaneTimeout, got {err:?}");
    };
    assert_eq!(host.port(), silent_port);
    let text = err.to_string();
    assert!(text.contains(&silent_port.to_string()), "{text}");
    assert!(text.contains("UDP") && text.contains("firewall"), "{text}");
    server.join().unwrap();
}

#[test]
fn input_windows_and_snaps_ride_the_lane_and_not_tcp_twice() {
    let host = host();
    let mut lane = lane_client(&host, "alice");
    let mut tcp = Client::join(host.addr(), &hello_for("bob")).unwrap();
    tcp.set_timeout(Some(WAIT)).unwrap();
    let generation = start_session(&host, &mut [&mut lane, &mut tcp]);
    let inputs = host.remote_inputs();
    inputs.set_running_generation(Some(generation));
    let id = lane.player_id();
    eventually("the lane", || host.lane_established(id));

    // Client -> host: the window arrives through the lane.
    let ctl = lane.ctl().unwrap();
    let sent_before = lane.lane_counts().unwrap().sent;
    ctl.send_input_window(generation, 0, 1, vec![cmd(5), cmd(6)])
        .unwrap();
    eventually("the window to land", || inputs.windows_received(id) == 1);
    assert_eq!(lane.lane_counts().unwrap().sent, sent_before + 1);
    assert_eq!(inputs.seat_stats(id).unwrap().received, 2);

    // Host -> clients: the lane seat gets it as a datagram, the TCP seat
    // over TCP, and nobody gets two.
    let sent = host.lane_counts().sent;
    host.ctl().broadcast(&snap(generation, 77)).unwrap();
    for client in [&mut lane, &mut tcp] {
        loop {
            match client.recv().unwrap() {
                Message::Snap { tick: 77, .. } => break,
                Message::Snap { tick, .. } => panic!("unexpected snap {tick}"),
                _ => {}
            }
        }
    }
    assert_eq!(host.lane_counts().sent, sent + 1, "exactly one datagram");
    lane.set_timeout(Some(Duration::from_millis(300))).unwrap();
    tcp.set_timeout(Some(Duration::from_millis(300))).unwrap();
    for client in [&mut lane, &mut tcp] {
        while let Ok(msg) = client.recv() {
            assert!(!matches!(msg, Message::Snap { .. }), "duplicate {msg:?}");
        }
    }

    // send_to takes the lane for the lane seat too.
    let sent = host.lane_counts().sent;
    host.ctl().send_to(id, &snap(generation, 78)).unwrap();
    lane.set_timeout(Some(WAIT)).unwrap();
    loop {
        if matches!(lane.recv().unwrap(), Message::Snap { tick: 78, .. }) {
            break;
        }
    }
    assert_eq!(host.lane_counts().sent, sent + 1);
}

#[test]
fn a_snap_over_the_datagram_budget_stays_on_tcp() {
    let host = host();
    let mut lane = lane_client(&host, "alice");
    let generation = start_session(&host, &mut [&mut lane]);
    eventually("the lane", || host.lane_established(lane.player_id()));
    let big = Message::Snap {
        generation,
        tick: 5,
        entries: vec![SnapEntry::default(); crate::MAX_PLAYERS as usize],
        trailers: Vec::new(),
        impacts: vec![
            crate::SnapImpact {
                seat: 1,
                id: 1,
                tick: 1,
                point: [0.0; 3],
                normal: [0.0; 3],
                severity: 1.0,
                audio_id: 0,
            };
            crate::MAX_SNAP_IMPACTS as usize
        ],
        race: None,
    };
    assert!(
        big.encode().unwrap().len() > crate::MAX_PAYLOAD,
        "the fixture must exceed one datagram"
    );
    let sent = host.lane_counts().sent;
    host.ctl().broadcast(&big).unwrap();
    loop {
        if matches!(lane.recv().unwrap(), Message::Snap { tick: 5, .. }) {
            break;
        }
    }
    assert_eq!(host.lane_counts().sent, sent, "no datagram was sent");
    assert_eq!(host.lane_counts().oversize_tcp, 1);
}

#[test]
fn unauthenticated_wrong_seat_and_wrong_generation_datagrams_cannot_move_a_player() {
    let host = host();
    let mut a = RawPeer::join(&host, "alice");
    let mut b = RawPeer::join(&host, "bob");
    for c in [&mut a, &mut b] {
        c.conn
            .send(&Message::SetVehicle(crate::VehiclePick {
                vehicle: String::new(),
                paint: 0,
            }))
            .unwrap();
        c.conn.send(&Message::SetReady { ready: true }).unwrap();
    }
    host.set_session(SessionAdvertisement {
        summary: "lane".to_string(),
        params: Vec::new(),
    })
    .unwrap();
    let generation = loop {
        host.start(LateJoin::Open).unwrap();
        match host.recv_timeout(Duration::from_millis(200)) {
            Ok(HostEvent::Started { generation }) => break generation,
            _ => continue,
        }
    };
    let inputs = host.remote_inputs();
    inputs.set_running_generation(Some(generation));
    a.establish(&host);
    b.establish(&host);
    let counts = || host.lane_counts();
    let (a_seat, b_seat) = (a.seat, b.seat);
    let silent = |inputs: &crate::RemoteInputs| {
        inputs.windows_received(a_seat) == 0
            && inputs.windows_received(b_seat) == 0
            && inputs.last_activity(a_seat).is_none()
            && inputs.last_activity(b_seat).is_none()
    };
    let window = RawPeer::window_bytes(generation, 1);
    let base = counts();

    // Garbage and short datagrams.
    a.send_raw(b"hello");
    a.send_raw(&[]);
    let mut bad_magic = encode_datagram(&a.header(LaneTag::Data, generation), &window).unwrap();
    bad_magic[0] = b'X';
    a.send_raw(&bad_magic);
    // A seat the host never minted.
    let mut h = a.header(LaneTag::Data, generation);
    h.seat = 999;
    a.send_header(&h, &window);
    // Right seat, wrong token.
    let mut h = a.header(LaneTag::Data, generation);
    h.token ^= 1;
    a.send_header(&h, &window);
    // Seat B with A's token (wrong-seat claim), from A's socket.
    let mut h = a.header(LaneTag::Data, generation);
    h.seat = b.seat;
    a.send_header(&h, &window);
    // Seat B with B's token but from A's socket: wrong source address.
    let mut h = b.header(LaneTag::Data, generation);
    h.seat = b.seat;
    a.send_header(&h, &window);
    // Wrong generation in the header, then a header/payload mismatch.
    let h = a.header(LaneTag::Data, generation + 1);
    a.send_header(&h, &window);
    let h = a.header(LaneTag::Data, generation);
    a.send_header(&h, &RawPeer::window_bytes(generation + 1, 1));
    // A message kind the host never takes over the lane.
    let h = a.header(LaneTag::Data, generation);
    a.send_header(&h, &Message::ResetRequest { generation }.encode().unwrap());
    // An undecodable payload under valid credentials.
    let h = a.header(LaneTag::Data, generation);
    a.send_header(&h, &[0xff, 1, 2]);

    eventually("every attack to be counted", || {
        let c = counts();
        c.received - base.received >= 11
            && c.malformed - base.malformed >= 4
            && c.unknown_seat - base.unknown_seat == 1
            && c.bad_token - base.bad_token == 2
            && c.wrong_addr - base.wrong_addr == 1
            && c.wrong_generation - base.wrong_generation == 2
            && c.unexpected - base.unexpected == 1
    });
    let c = counts();
    assert_eq!(c.accepted, base.accepted, "nothing was accepted: {c:?}");
    assert!(silent(&inputs), "no attack moved a player: {c:?}");

    // A valid window from the right peer lands — and replaying the very
    // same datagram does not.
    let h = a.header(LaneTag::Data, generation);
    let valid = encode_datagram(&h, &window).unwrap();
    a.send_raw(&valid);
    eventually("the valid window", || inputs.windows_received(a.seat) == 1);
    a.send_raw(&valid);
    eventually("the replay to be counted", || {
        counts().replayed - base.replayed == 1
    });
    assert_eq!(inputs.windows_received(a.seat), 1);
    assert_eq!(inputs.windows_received(b.seat), 0);

    // Between sessions the generation gate is shut: a window naming the
    // old generation after Cancel is wrong-generation, not delivered.
    host.cancel().unwrap();
    eventually("the cancel", || {
        matches!(
            host.recv_timeout(Duration::from_millis(50)),
            Ok(HostEvent::Cancelled { .. })
        )
    });
    let before = counts().wrong_generation;
    let h = a.header(LaneTag::Data, generation);
    a.send_header(&h, &RawPeer::window_bytes(generation, 2));
    eventually("the post-cancel window to be refused", || {
        counts().wrong_generation == before + 1
    });
    assert_eq!(inputs.windows_received(a.seat), 1);
}

#[test]
fn the_outbound_queue_holds_its_bound_and_counts_drops() {
    let host = host();
    let mut raw = RawPeer::join(&host, "alice");
    raw.conn
        .send(&Message::SetVehicle(crate::VehiclePick {
            vehicle: String::new(),
            paint: 0,
        }))
        .unwrap();
    raw.conn.send(&Message::SetReady { ready: true }).unwrap();
    host.set_session(SessionAdvertisement {
        summary: "lane".to_string(),
        params: Vec::new(),
    })
    .unwrap();
    let generation = loop {
        host.start(LateJoin::Open).unwrap();
        if let Ok(HostEvent::Started { generation }) = host.recv_timeout(Duration::from_millis(200))
        {
            break generation;
        }
    };
    raw.establish(&host);
    // Flood far past the bound in a tight loop; the sender thread drains
    // concurrently, so the invariants are the bound and the accounting.
    let ctl = host.ctl();
    for tick in 0..2000 {
        ctl.send_to(raw.seat, &snap(generation, tick)).unwrap();
        let s = host.lane_queue(raw.seat).unwrap();
        assert!(s.len <= s.capacity && s.capacity == QUEUE_CAP, "{s:?}");
    }
    let s = host.lane_queue(raw.seat).unwrap();
    assert_eq!(s.pushed, 2001, "2000 snaps plus the probe echo: {s:?}");
    eventually("the queue to drain", || {
        host.lane_queue(raw.seat).unwrap().len == 0
    });
    let s = host.lane_queue(raw.seat).unwrap();
    assert!(s.max_wait > Duration::ZERO, "queue age is reported: {s:?}");
    assert_eq!(
        host.lane_counts().sent + host.lane_counts().send_errors,
        s.pushed - s.dropped,
        "every non-dropped entry was sent: {s:?}"
    );
}

#[test]
fn the_client_inbox_holds_its_bound_and_drops_oldest() {
    let host = host();
    let mut alice = lane_client(&host, "alice");
    let generation = start_session(&host, &mut [&mut alice]);
    eventually("the lane", || host.lane_established(alice.player_id()));
    let ctl = host.ctl();
    // Nobody consumes: the lane side of the inbox must cap at QUEUE_CAP.
    for tick in 0..(QUEUE_CAP as u64 * 3) {
        ctl.send_to(alice.player_id(), &snap(generation, tick))
            .unwrap();
        // Pace so the sender thread's loopback datagrams are not lost to
        // socket buffer pressure on a loaded machine.
        thread::sleep(Duration::from_millis(1));
    }
    eventually("the datagrams to arrive", || {
        alice.lane_counts().unwrap().accepted > QUEUE_CAP as u64 + 4
    });
    let s = alice.lane_inbox_stats().unwrap();
    assert_eq!(s.len, QUEUE_CAP, "{s:?}");
    assert_eq!(s.dropped, s.pushed - QUEUE_CAP as u64, "{s:?}");
    // The survivors are the newest, in order.
    let mut last = None;
    while let Ok(msg) = alice.recv() {
        if let Message::Snap { tick, .. } = msg {
            assert!(last.is_none_or(|l| tick > l));
            last = Some(tick);
            if alice.lane_inbox_stats().unwrap().len == 0 {
                break;
            }
        }
    }
    assert!(last.is_some());
}

#[test]
fn late_join_and_disconnect_work_through_the_lane() {
    let host = host();
    let mut alice = lane_client(&host, "alice");
    let generation = start_session(&host, &mut [&mut alice]);
    let inputs = host.remote_inputs();
    inputs.set_running_generation(Some(generation));

    // A late joiner is dropped into the running session and its lane
    // carries windows for that generation.
    let mut bob = lane_client(&host, "bob");
    loop {
        if matches!(bob.recv().unwrap(), Message::Start { .. }) {
            break;
        }
    }
    let bob_id = bob.player_id();
    eventually("bob's lane", || host.lane_established(bob_id));
    bob.ctl()
        .unwrap()
        .send_input_window(generation, 0, 1, vec![cmd(1)])
        .unwrap();
    eventually("bob's window", || inputs.windows_received(bob_id) == 1);
    assert_eq!(inputs.windows_received(alice.player_id()), 0);

    // Bob leaves: his lane is gone with his seat, and the old credentials
    // are an unknown seat — they cannot move a recycled id.
    bob.leave().unwrap();
    eventually("bob's departure", || !host.lane_established(bob_id));
    let before = host.lane_counts().unknown_seat;
    let stale = UdpSocket::bind("127.0.0.1:0").unwrap();
    let h = Header {
        tag: LaneTag::Probe,
        generation: 0,
        seat: bob_id,
        token: 1,
        seq: 1,
    };
    stale
        .send_to(
            &encode_datagram(&h, &[]).unwrap(),
            SocketAddr::new(host.addr().ip(), host.lane_port()),
        )
        .unwrap();
    eventually("the stale probe to be refused", || {
        host.lane_counts().unknown_seat == before + 1
    });
    assert!(!host.lane_established(bob_id));

    // Alice is unaffected and a dropped connection also frees the lane.
    assert!(host.lane_established(alice.player_id()));
    let alice_id = alice.player_id();
    drop(alice);
    eventually("alice's lane to close", || !host.lane_established(alice_id));
}

#[test]
fn a_v32_peer_is_refused_before_any_lane_exists() {
    let host = host();
    let mut conn = Conn::connect(host.addr()).unwrap();
    let mut old = hello_for("alice");
    old.protocol = PROTOCOL_VERSION - 1;
    assert!(matches!(
        crate::send_hello(&mut conn, &old),
        Err(NetError::Rejected { .. })
    ));
}
