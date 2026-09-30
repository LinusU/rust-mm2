//! The lobby channel: a host driver and the client join path (F24-B.1).
//!
//! The host is three kinds of thread bridged by one channel:
//!
//! - an **accept thread** blocking on the listener, forwarding every new
//!   connection;
//! - a short-lived **handshake thread** per accepted connection, running
//!   the compatibility gate under [`HANDSHAKE_TIMEOUT`] so a stalled or
//!   flooding peer can never block accepts — in-flight handshakes are
//!   capped by [`MAX_PENDING`];
//! - one **reader thread** per admitted player, forwarding the peer's
//!   lobby messages and reporting the socket's death.
//!
//! The **host loop** is the only code that mutates the roster: it mints
//! player ids, sends `Accept`/`Reject` + `Welcome`, applies `SetReady`,
//! reaps gone peers and broadcasts the whole `Roster` after every change.
//! Consumers drain [`HostEvent`]s — the Bevy bridge (F24-B follow-up) is
//! just a system that forwards them into app state.
//!
//! Nothing here knows about vehicles, cities or modes: session
//! advertisement and start/cancel are later F24-B legs, and the game-rule
//! types stay in `mm2_game` — the wire carries opaque fields only.

use std::collections::BTreeMap;
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvError, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::NetError;
use crate::conn::{Conn, HANDSHAKE_TIMEOUT, Writer, listen_loopback, recv_hello_within};
use crate::proto::{Hello, MAX_PLAYERS, Message, RejectCode, RosterEntry, SessionAdvertisement};

/// Cap on connections mid-handshake. A connect flood drops at the accept
/// boundary rather than spawning unbounded threads; each in-flight
/// handshake is itself bounded by [`HANDSHAKE_TIMEOUT`]. Sized like the
/// roster — more pending joins than seats can never all fit anyway.
const MAX_PENDING: usize = MAX_PLAYERS as usize;

/// Write bound on established lobby connections. Broadcast sends share
/// the socket with the reader thread, so they get their own deadline:
/// a peer that stops reading stalls a broadcast for this long, then is
/// dropped as `Lost` instead of blocking the loop forever.
const WRITE_TIMEOUT: Duration = HANDSHAKE_TIMEOUT;

/// Bound on `Client::leave`'s post-`Leave` drain. The host closes
/// promptly once it reads the quit (its reader thread exits and drops
/// the socket); this only bounds a host that never does.
const LEAVE_DRAIN_TIMEOUT: Duration = Duration::from_millis(250);

/// Parameters a host listens under.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// The host's `mm2_content::fingerprint::gameplay` value — the
    /// compatibility gate every joining `Hello` is checked against.
    pub gameplay_fingerprint: u64,
    /// Remote-client ceiling. The default [`MAX_PLAYERS`] counts
    /// connections only — a host that is itself a player (MP-1's eight
    /// *total*) should pass `MAX_PLAYERS - 1`; a dedicated headless host
    /// may keep the full eight. Values above [`MAX_PLAYERS`] are
    /// rejected at listen time — the wire roster cannot carry them.
    pub max_clients: u16,
}

impl HostConfig {
    /// A lobby at the documented player ceiling.
    pub fn new(gameplay_fingerprint: u64) -> Self {
        Self {
            gameplay_fingerprint,
            max_clients: MAX_PLAYERS as u16,
        }
    }
}

/// Why a roster entry went away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaveCause {
    /// The peer sent `Leave` — a deliberate quit.
    Quit,
    /// The socket died (EOF, reset, timed out): a drop, not a quit.
    Lost,
    /// The peer spoke out of turn post-handshake — a protocol violation,
    /// answered by dropping it.
    Malformed,
}

/// One observable lobby transition, drained by the consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// A peer completed the handshake and took a roster slot.
    Joined {
        /// The assigned slot.
        id: u16,
        /// Display name from its `Hello`.
        driver: String,
        /// Build identifier from its `Hello`.
        build: String,
    },
    /// A roster entry went away.
    Left {
        /// The freed slot.
        id: u16,
        /// The departed driver.
        driver: String,
        /// Whether it quit, dropped or violated the protocol.
        cause: LeaveCause,
    },
    /// A peer toggled its readiness flag.
    ReadyChanged {
        /// The slot that changed.
        id: u16,
        /// The new state.
        ready: bool,
    },
    /// A connection was refused — a failed handshake (version/content/
    /// malformed/stalled, with the wire `Reject` sent where applicable)
    /// or a join against a full roster. `reason` is display-ready.
    JoinFailed {
        /// The refused peer's address.
        peer: SocketAddr,
        /// Why it was refused.
        reason: String,
    },
}

/// A running lobby host: owns the listener and event loop, hands the
/// consumer a channel of [`HostEvent`]s. Dropping the `Host` shuts the
/// lobby down (peer sockets closed, threads reaped).
pub struct Host {
    addr: SocketAddr,
    events: Receiver<HostEvent>,
    control: Sender<LoopMsg>,
    handle: Option<JoinHandle<()>>,
}

impl Host {
    /// Listen on an explicit address. [`Self::listen_loopback`] is the
    /// safe default; a LAN/interface bind is the caller's deliberate
    /// choice, never the default.
    pub fn listen(addr: SocketAddr, config: &HostConfig) -> Result<Self, NetError> {
        Self::spawn(TcpListener::bind(addr)?, config)
    }

    /// Listen on `127.0.0.1` with an ephemeral port — tests and local
    /// play. No public interface is ever bound by default.
    pub fn listen_loopback(config: &HostConfig) -> Result<Self, NetError> {
        Self::spawn(listen_loopback()?, config)
    }

    fn spawn(listener: TcpListener, config: &HostConfig) -> Result<Self, NetError> {
        if config.max_clients > MAX_PLAYERS as u16 {
            return Err(NetError::Config(
                "max_clients exceeds the wire's MAX_PLAYERS bound",
            ));
        }
        let addr = listener.local_addr()?;
        let (tx, rx) = mpsc::channel();
        let (events_tx, events) = mpsc::channel();
        let config = config.clone();
        let control = tx.clone();
        let handle = thread::spawn(move || {
            run(listener, addr, config, tx, rx, events_tx);
        });
        Ok(Self {
            addr,
            events,
            control,
            handle: Some(handle),
        })
    }

    /// The address peers dial — `127.0.0.1:<ephemeral>` for a loopback
    /// host.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Advertise (or replace) the session this lobby will run. The
    /// advertisement is broadcast to every connected player and sent to
    /// each newcomer between `Welcome` and the roster, so a client always
    /// sees the current session alongside its first roster. A payload
    /// the wire cannot carry is refused here rather than silently
    /// dropped by the loop.
    ///
    /// What the advertisement *says* is the consumer's business — the
    /// `mm2_app` bridge maps `SessionConfig` onto the opaque `params`
    /// blob; the lobby only bounds and carries it.
    pub fn set_session(&self, session: SessionAdvertisement) -> Result<(), NetError> {
        Message::Session(session.clone()).encode()?;
        self.control
            .send(LoopMsg::SetSession(session))
            .map_err(|_| {
                NetError::Io(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "host loop is not running",
                ))
            })?;
        Ok(())
    }

    /// Wait for the next lobby event, unbounded — for consumers that
    /// live entirely on lobby traffic (the dedicated host). `Err` means
    /// the loop is gone.
    pub fn recv(&self) -> Result<HostEvent, RecvError> {
        self.events.recv()
    }

    /// Wait for the next lobby event, at most `timeout`.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<HostEvent, RecvTimeoutError> {
        self.events.recv_timeout(timeout)
    }

    /// Drain a pending event without blocking.
    pub fn try_recv(&self) -> Result<HostEvent, mpsc::TryRecvError> {
        self.events.try_recv()
    }

    /// Shut the lobby down: peer sockets close (their clients see the
    /// connection die), the accept thread wakes and exits, the loop
    /// joins. Also runs on `Drop`.
    pub fn shutdown(&mut self) {
        let _ = self.control.send(LoopMsg::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The joining side of a lobby. `join` completes the handshake and the
/// slot assignment; the roster and later lobby traffic arrive through
/// [`Client::recv`].
#[derive(Debug)]
pub struct Client {
    conn: Conn,
    player_id: u16,
}

impl Client {
    /// Connect to `addr`, run the handshake and read the host's
    /// `Welcome` carrying this client's roster slot. `Accept` was only
    /// the compatibility verdict, so the `Welcome` wait gets its own
    /// [`HANDSHAKE_TIMEOUT`] bound — a host may not stall it forever.
    pub fn join(addr: SocketAddr, hello: &Hello) -> Result<Self, NetError> {
        let mut conn = Conn::connect(addr)?;
        crate::send_hello(&mut conn, hello)?;
        conn.set_timeout(Some(HANDSHAKE_TIMEOUT))?;
        let welcome = conn.recv();
        conn.set_timeout(None)?;
        let player_id = match welcome? {
            Message::Welcome { player_id } => player_id,
            _ => return Err(NetError::Unexpected("expected Welcome")),
        };
        Ok(Self { conn, player_id })
    }

    /// The roster slot the host assigned.
    pub fn player_id(&self) -> u16 {
        self.player_id
    }

    /// Toggle this client's readiness flag; the host rebroadcasts the
    /// roster to everyone.
    pub fn set_ready(&mut self, ready: bool) -> Result<(), NetError> {
        self.conn.send(&Message::SetReady { ready })
    }

    /// Send an arbitrary lobby message. The host only accepts the
    /// client→host set (`SetReady`, `Leave`) post-handshake — anything
    /// else is a protocol violation that gets this client dropped.
    pub fn send(&mut self, msg: &Message) -> Result<(), NetError> {
        self.conn.send(msg)
    }

    /// A clean quit: the host reports `Quit`, not a dropped connection.
    /// After sending `Leave` the socket is half-closed and drained
    /// briefly — a socket dropped with unread inbound data resets (RST),
    /// which would make a deliberate quit indistinguishable from a
    /// crash. Consumes the client.
    pub fn leave(mut self) -> Result<(), NetError> {
        self.conn.send(&Message::Leave)?;
        self.conn.shutdown_write();
        self.conn.set_timeout(Some(LEAVE_DRAIN_TIMEOUT))?;
        // Read until the host closes (its reader thread exits on `Leave`)
        // or the drain bound expires — either way no inbound data is
        // left pending at drop, so the close stays a clean FIN.
        while self.conn.recv().is_ok() {}
        Ok(())
    }

    /// The next host→client message — a `Roster` broadcast today, more
    /// lobby traffic as later F24-B legs land. `Err` means the
    /// connection to the host is gone.
    pub fn recv(&mut self) -> Result<Message, NetError> {
        self.conn.recv()
    }

    /// Bound `recv`/`set_ready` waits — tests install a short backstop so
    /// a broken peer cannot hang the suite.
    pub fn set_timeout(&self, timeout: Option<Duration>) -> Result<(), NetError> {
        self.conn.set_timeout(timeout)
    }
}

/// Everything the host loop can be woken by — one channel for the accept
/// thread, the handshake threads, the per-player readers and control.
enum LoopMsg {
    /// Accept thread → loop: a fresh connection to handshake.
    Accepted(Conn),
    /// Handshake thread → loop: gated and admitted pending a seat.
    Handshaken { conn: Conn, hello: Hello },
    /// Handshake thread → loop: refused or failed before `Accept`.
    JoinFailed { peer: SocketAddr, reason: String },
    /// Reader thread → loop: a lobby message from a player.
    PeerMessage { id: u16, msg: Message },
    /// Reader thread → loop: the player's socket ended.
    PeerGone { id: u16, cause: LeaveCause },
    /// `Host::set_session` — the session this lobby advertises.
    SetSession(SessionAdvertisement),
    /// `Host::shutdown`.
    Shutdown,
}

/// A rostered player's host-side state.
struct Slot {
    driver: String,
    build: String,
    ready: bool,
    writer: Writer,
}

fn run(
    listener: TcpListener,
    addr: SocketAddr,
    config: HostConfig,
    tx: Sender<LoopMsg>,
    rx: Receiver<LoopMsg>,
    events: Sender<HostEvent>,
) {
    let stop = Arc::new(AtomicBool::new(false));
    let accept = {
        let tx = tx.clone();
        let stop = stop.clone();
        thread::spawn(move || {
            loop {
                match listener.accept() {
                    // On shutdown the loop self-connects to wake this accept;
                    // the flag tells a wake connection from a real peer.
                    Ok((stream, _)) => {
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
                        match Conn::from_stream(stream) {
                            Ok(conn) => {
                                if tx.send(LoopMsg::Accepted(conn)).is_err() {
                                    return;
                                }
                            }
                            Err(_) => return,
                        }
                    }
                    Err(_) => return,
                }
            }
        })
    };

    let mut players: BTreeMap<u16, Slot> = BTreeMap::new();
    // The session this lobby advertises; `None` until the consumer sets
    // one — clients then simply never see a `Session` message.
    let mut session: Option<SessionAdvertisement> = None;
    let mut pending = 0usize;
    // Wire ids mint from 1; 0 is reserved for the host player at the app
    // layer so the wire roster and the displayed roster share numbering.
    let mut next_id: u16 = 1;

    while let Ok(msg) = rx.recv() {
        match msg {
            LoopMsg::Shutdown => break,
            LoopMsg::Accepted(conn) => {
                if pending >= MAX_PENDING {
                    // Beyond the in-flight bound: drop at the door; the
                    // peer's connect just sees a closed socket.
                    continue;
                }
                pending += 1;
                let tx = tx.clone();
                let fingerprint = config.gameplay_fingerprint;
                thread::spawn(move || {
                    let mut conn = conn;
                    let peer = conn.peer_addr();
                    let msg = match recv_hello_within(&mut conn, fingerprint, HANDSHAKE_TIMEOUT) {
                        // The deadline stays installed; the loop answers
                        // the verdict itself once it knows a seat is free.
                        Ok(hello) => LoopMsg::Handshaken { conn, hello },
                        Err(e) => LoopMsg::JoinFailed {
                            peer,
                            reason: e.to_string(),
                        },
                    };
                    let _ = tx.send(msg);
                });
            }
            LoopMsg::JoinFailed { peer, reason } => {
                pending = pending.saturating_sub(1);
                let _ = events.send(HostEvent::JoinFailed { peer, reason });
            }
            LoopMsg::Handshaken { mut conn, hello } => {
                pending = pending.saturating_sub(1);
                if players.len() >= config.max_clients as usize {
                    let _ = conn.send(&Message::Reject {
                        code: RejectCode::LobbyFull,
                        message: "lobby is full".to_string(),
                    });
                    let _ = events.send(HostEvent::JoinFailed {
                        peer: conn.peer_addr(),
                        reason: "lobby is full".to_string(),
                    });
                    continue;
                }
                let id = alloc_id(&players, &mut next_id);
                let Ok(mut writer) = conn.writer() else {
                    continue;
                };
                if writer
                    .send(&Message::Accept)
                    .and_then(|()| writer.send(&Message::Welcome { player_id: id }))
                    .is_err()
                {
                    continue;
                }
                // A newcomer always sees the current session before its
                // first roster. A dead write here drops the conn
                // silently, like a failed `Welcome` above.
                if let Some(ad) = &session
                    && writer.send(&Message::Session(ad.clone())).is_err()
                {
                    continue;
                }
                if conn.set_timeout(None).is_err() {
                    continue;
                }
                // The session clear also lifted the write bound; keep
                // sends bounded so a peer that stops reading is dropped
                // rather than freezing the broadcast.
                let _ = writer.set_write_timeout(Some(WRITE_TIMEOUT));
                players.insert(
                    id,
                    Slot {
                        driver: hello.driver.clone(),
                        build: hello.build.clone(),
                        ready: false,
                        writer,
                    },
                );
                let _ = events.send(HostEvent::Joined {
                    id,
                    driver: hello.driver,
                    build: hello.build,
                });
                broadcast_roster(&mut players, &events);
                // The newcomer's own first roster send may have removed
                // it as `Lost` — a departed slot gets no reader.
                if players.contains_key(&id) {
                    spawn_reader(conn, id, tx.clone());
                }
            }
            LoopMsg::PeerMessage {
                id,
                msg: Message::SetReady { ready },
            } => {
                if let Some(slot) = players.get_mut(&id) {
                    slot.ready = ready;
                    let _ = events.send(HostEvent::ReadyChanged { id, ready });
                    broadcast_roster(&mut players, &events);
                }
            }
            LoopMsg::PeerMessage { .. } => {}
            LoopMsg::SetSession(ad) => {
                session = Some(ad.clone());
                // Removals under the session send change the roster too —
                // survivors get the corrected snapshot after the ad.
                if !broadcast(&mut players, &Message::Session(ad), &events) {
                    broadcast_roster(&mut players, &events);
                }
            }
            LoopMsg::PeerGone { id, cause } => {
                if let Some(slot) = players.remove(&id) {
                    // The reporting reader already exited, but removal
                    // always disconnects — no path leaves a live socket.
                    slot.writer.disconnect();
                    let _ = events.send(HostEvent::Left {
                        id,
                        driver: slot.driver,
                        cause,
                    });
                    broadcast_roster(&mut players, &events);
                }
            }
        }
    }

    // Teardown: close every peer socket (wakes the reader threads, whose
    // sends into the dead channel just fail), stop the accept thread and
    // wake its blocking accept with a self-connect.
    for slot in players.values_mut() {
        slot.writer.disconnect();
    }
    stop.store(true, Ordering::Relaxed);
    let _ = TcpStream::connect(addr);
    let _ = accept.join();
}

/// Mint the lowest-unused-slot id: monotonic until wraparound, never 0,
/// never an occupied slot. Terminates because the roster is bounded.
fn alloc_id(players: &BTreeMap<u16, Slot>, next_id: &mut u16) -> u16 {
    loop {
        let id = *next_id;
        *next_id = next_id.wrapping_add(1).max(1);
        if !players.contains_key(&id) {
            return id;
        }
    }
}

/// Send `msg` to every player once. A failed write means the peer is
/// gone: disconnect it (waking its reader thread, which is still blocked
/// in `recv` on the same socket — otherwise the thread, the fd and the
/// client's dead-but-open connection all leak) and drop it as `Lost`.
/// Returns `true` when every player received the message; on `false` the
/// roster changed and callers whose message no longer fits should resend
/// a corrected snapshot (`broadcast_roster` does this for `Roster`
/// itself).
fn broadcast(players: &mut BTreeMap<u16, Slot>, msg: &Message, events: &Sender<HostEvent>) -> bool {
    let mut failed = Vec::new();
    for (id, slot) in players.iter_mut() {
        if slot.writer.send(msg).is_err() {
            failed.push(*id);
        }
    }
    for id in &failed {
        if let Some(slot) = players.remove(id) {
            slot.writer.disconnect();
            let _ = events.send(HostEvent::Left {
                id: *id,
                driver: slot.driver,
                cause: LeaveCause::Lost,
            });
        }
    }
    failed.is_empty()
}

/// Send the complete roster to every player, rebuilt and resent after
/// each removal — a survivor must never see a snapshot still listing a
/// departed peer. Each pass removes at least one player, so the resend
/// loop terminates.
fn broadcast_roster(players: &mut BTreeMap<u16, Slot>, events: &Sender<HostEvent>) {
    loop {
        let msg = Message::Roster {
            players: players
                .iter()
                .map(|(&id, s)| RosterEntry {
                    player_id: id,
                    driver: s.driver.clone(),
                    build: s.build.clone(),
                    ready: s.ready,
                })
                .collect(),
        };
        if broadcast(players, &msg, events) {
            return;
        }
    }
}

/// One thread per player: forward `SetReady`, report `Leave` as a clean
/// quit, and drop the peer on a protocol violation or a dead socket.
fn spawn_reader(conn: Conn, id: u16, tx: Sender<LoopMsg>) {
    thread::spawn(move || {
        let mut conn = conn;
        loop {
            let msg = match conn.recv() {
                Ok(Message::Leave) => LoopMsg::PeerGone {
                    id,
                    cause: LeaveCause::Quit,
                },
                Ok(msg @ Message::SetReady { .. }) => LoopMsg::PeerMessage { id, msg },
                Ok(_) => LoopMsg::PeerGone {
                    id,
                    cause: LeaveCause::Malformed,
                },
                Err(_) => LoopMsg::PeerGone {
                    id,
                    cause: LeaveCause::Lost,
                },
            };
            let terminal = matches!(msg, LoopMsg::PeerGone { .. });
            if tx.send(msg).is_err() || terminal {
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hello;

    const FP: u64 = 0xaaaa;
    const WAIT: Duration = Duration::from_secs(5);

    fn host() -> Host {
        Host::listen_loopback(&HostConfig::new(FP)).unwrap()
    }

    fn join(addr: SocketAddr, driver: &str) -> Client {
        let client = Client::join(
            addr,
            &hello("test-build".to_string(), driver.to_string(), FP),
        )
        .unwrap();
        client.set_timeout(Some(WAIT)).unwrap();
        client
    }

    /// Read until a roster of exactly `n` players arrives — earlier
    /// rosters from prior transitions may still be queued on the socket.
    fn recv_roster(client: &mut Client, n: usize) -> Vec<RosterEntry> {
        for _ in 0..8 {
            match client.recv().unwrap() {
                Message::Roster { players } if players.len() == n => return players,
                Message::Roster { .. } => continue,
                other => panic!("expected a Roster, got {other:?}"),
            }
        }
        panic!("no {n}-player roster arrived");
    }

    #[test]
    fn a_join_takes_a_slot_and_sees_itself_in_the_roster() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        assert_eq!(alice.player_id(), 1);

        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Joined { id, driver, build }) => {
                assert_eq!(id, 1);
                assert_eq!(driver, "alice");
                assert_eq!(build, "test-build");
            }
            other => panic!("expected Joined, got {other:?}"),
        }
        assert_eq!(
            recv_roster(&mut alice, 1),
            [RosterEntry {
                player_id: 1,
                driver: "alice".to_string(),
                build: "test-build".to_string(),
                ready: false,
            }]
        );
    }

    #[test]
    fn a_second_join_broadcasts_the_grown_roster_to_everyone() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        let mut bob = join(host.addr(), "bob");
        assert_eq!(bob.player_id(), 2);

        let expected = vec![
            RosterEntry {
                player_id: 1,
                driver: "alice".to_string(),
                build: "test-build".to_string(),
                ready: false,
            },
            RosterEntry {
                player_id: 2,
                driver: "bob".to_string(),
                build: "test-build".to_string(),
                ready: false,
            },
        ];
        // The newcomer and the incumbent both see the same roster.
        assert_eq!(recv_roster(&mut bob, 2), expected);
        assert_eq!(recv_roster(&mut alice, 2), expected);

        for (id, driver) in [(1, "alice"), (2, "bob")] {
            match host.recv_timeout(WAIT) {
                Ok(HostEvent::Joined {
                    id: got,
                    driver: name,
                    ..
                }) => {
                    assert_eq!((got, name.as_str()), (id, driver));
                }
                other => panic!("expected Joined, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_ready_change_rebroadcasts_to_everyone() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        let mut bob = join(host.addr(), "bob");
        host.recv_timeout(WAIT).unwrap();
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 2);
        recv_roster(&mut bob, 2);

        alice.set_ready(true).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::ReadyChanged { id: 1, ready: true }) => {}
            other => panic!("expected ReadyChanged, got {other:?}"),
        }
        for client in [&mut alice, &mut bob] {
            let roster = recv_roster(client, 2);
            assert!(roster[0].ready);
            assert!(!roster[1].ready);
        }
    }

    #[test]
    fn a_clean_leave_and_a_drop_report_different_causes() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        let bob = join(host.addr(), "bob");
        host.recv_timeout(WAIT).unwrap();
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 2);

        bob.leave().unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Left {
                id: 2,
                driver,
                cause: LeaveCause::Quit,
            }) => assert_eq!(driver, "bob"),
            other => panic!("expected a Quit Left, got {other:?}"),
        }
        assert_eq!(recv_roster(&mut alice, 1).len(), 1);

        let carol = join(host.addr(), "carol");
        host.recv_timeout(WAIT).unwrap();
        drop(carol); // no Leave — a dead socket
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Left {
                driver,
                cause: LeaveCause::Lost,
                ..
            }) => assert_eq!(driver, "carol"),
            other => panic!("expected a Lost Left, got {other:?}"),
        }
        assert_eq!(recv_roster(&mut alice, 1).len(), 1);
    }

    #[test]
    fn a_rejoin_mints_a_fresh_slot() {
        let host = host();
        let alice = join(host.addr(), "alice");
        assert_eq!(alice.player_id(), 1);
        alice.leave().unwrap();
        host.recv_timeout(WAIT).unwrap(); // Joined
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Left {
                cause: LeaveCause::Quit,
                ..
            }) => {}
            other => panic!("expected a Quit Left, got {other:?}"),
        }

        // Slots are not recycled — a rejoining driver cannot inherit a
        // stale id a peer still remembers.
        let alice2 = join(host.addr(), "alice");
        assert_eq!(alice2.player_id(), 2);
    }

    #[test]
    fn a_full_lobby_rejects_with_lobby_full() {
        let config = HostConfig {
            gameplay_fingerprint: FP,
            max_clients: 1,
        };
        let host = Host::listen_loopback(&config).unwrap();
        let _alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap();

        let err =
            Client::join(host.addr(), &hello("b".to_string(), "bob".to_string(), FP)).unwrap_err();
        assert!(matches!(
            err,
            NetError::Rejected {
                code: RejectCode::LobbyFull,
                ..
            }
        ));
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::JoinFailed { reason, .. }) => {
                assert!(reason.contains("full"), "got {reason}");
            }
            other => panic!("expected JoinFailed, got {other:?}"),
        }
    }

    #[test]
    fn an_incompatible_peer_never_touches_the_roster() {
        let host = host();
        let err = Client::join(
            host.addr(),
            &hello("b".to_string(), "mallory".to_string(), FP + 1),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            NetError::Rejected {
                code: RejectCode::ContentMismatch,
                ..
            }
        ));
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::JoinFailed { reason, .. }) => {
                assert!(reason.contains("content"), "got {reason}");
            }
            other => panic!("expected JoinFailed, got {other:?}"),
        }
        // And nothing else: no Joined, no roster.
        assert!(host.try_recv().is_err());
    }

    #[test]
    fn out_of_turn_messages_drop_the_peer() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 1);

        // `Accept` is host→client traffic; a client sending it violates
        // the protocol and is disconnected.
        alice.send(&Message::Accept).unwrap();
        match host.recv_timeout(WAIT) {
            Ok(HostEvent::Left {
                id: 1,
                cause: LeaveCause::Malformed,
                ..
            }) => {}
            other => panic!("expected a Malformed Left, got {other:?}"),
        }
        // The peer's socket is closed: its next read fails.
        assert!(alice.recv().is_err());
    }

    #[test]
    fn a_handshake_flood_is_bounded_at_the_door() {
        let host = host();
        // Occupy every in-flight handshake slot with peers that connect
        // and never speak — each holds its slot until HANDSHAKE_TIMEOUT,
        // so once the pending cap saturates the door stays closed for the
        // rest of the window.
        let _stallers: Vec<TcpStream> = (0..MAX_PENDING)
            .map(|_| TcpStream::connect(host.addr()).unwrap())
            .collect();
        // A real join during the flood is refused at the accept
        // boundary, not queued behind ten seconds of dead handshakes.
        // Attempts made before the pending count saturates can still
        // slip through, so retry until one is refused — once saturated,
        // every attempt must fail fast.
        let mut joined = Vec::new();
        let refused = (0..100).any(|_| {
            match Client::join(
                host.addr(),
                &hello("b".to_string(), "alice".to_string(), FP),
            ) {
                Ok(client) => {
                    joined.push(client);
                    thread::sleep(Duration::from_millis(10));
                    false
                }
                // Refused at the accept boundary (Io — the conn is
                // dropped mid-handshake) or at the roster cap itself:
                // either way the join cannot hang behind the flood.
                Err(NetError::Io(_))
                | Err(NetError::Rejected {
                    code: RejectCode::LobbyFull,
                    ..
                }) => true,
                Err(other) => panic!("unexpected join failure: {other:?}"),
            }
        });
        assert!(refused, "no join was refused while the flood held");
    }

    /// The iteration-010 review's zombie-drop defect: a broadcast write
    /// failure removed the peer without disconnecting it — the `Writer`
    /// drop left the reader thread blocked in `recv` on the same
    /// socket, leaking the thread and the fd and leaving the client on
    /// a dead-but-open connection. Here the roster fails *encode* (nine
    /// entries over `MAX_PLAYERS`), so no bytes and no FIN reach the
    /// peer — the assertions below can only pass if the removal path
    /// itself disconnects the socket.
    #[test]
    fn a_failed_broadcast_disconnects_the_peer() {
        use std::io::Read;

        let listener = listen_loopback().unwrap();
        let addr = listener.local_addr().unwrap();
        let mut peer = TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();
        let conn = Conn::from_stream(server).unwrap();
        // Nine slots sharing this one socket — the oversize roster fails
        // every send before a byte is written.
        let mut players = BTreeMap::new();
        for id in 1..=9u16 {
            players.insert(
                id,
                Slot {
                    driver: format!("p{id}"),
                    build: "b".to_string(),
                    ready: false,
                    writer: conn.writer().unwrap(),
                },
            );
        }
        // The production reader shape: blocked in `recv` for as long as
        // the socket lives.
        let (gone, check) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut conn = conn;
            let _ = conn.recv();
            let _ = gone.send(());
        });
        let (events, rx) = mpsc::channel();

        broadcast_roster(&mut players, &events);

        assert!(players.is_empty());
        for _ in 0..9 {
            match rx.recv_timeout(WAIT) {
                Ok(HostEvent::Left {
                    cause: LeaveCause::Lost,
                    ..
                }) => {}
                other => panic!("expected a Lost Left, got {other:?}"),
            }
        }
        // The blocked reader woke — its thread and fd are reaped.
        check
            .recv_timeout(WAIT)
            .expect("the departed peers' reader never woke");
        reader.join().unwrap();
        // And the peer is told: its socket reads EOF, not silence. No
        // roster byte was ever sent, so only `disconnect` can do this.
        peer.set_read_timeout(Some(WAIT)).unwrap();
        let mut buf = [0u8; 8];
        assert_eq!(peer.read(&mut buf).unwrap(), 0);
    }

    /// The iteration-010 review's config-validation defect: a
    /// `max_clients` above the wire's `MAX_PLAYERS` bound admitted a
    /// roster `Roster::encode` cannot represent — every broadcast then
    /// failed `OversizeRoster` and mass-dropped the lobby as `Lost`,
    /// with no diagnostic pointing at the config. It is rejected at
    /// listen instead.
    #[test]
    fn an_over_cap_max_clients_is_rejected() {
        let over = HostConfig {
            gameplay_fingerprint: FP,
            max_clients: MAX_PLAYERS as u16 + 1,
        };
        match Host::listen_loopback(&over) {
            Err(NetError::Config(msg)) => assert!(msg.contains("max_clients"), "got {msg}"),
            Err(e) => panic!("expected NetError::Config, got {e}"),
            Ok(_) => panic!("an over-cap max_clients listened successfully"),
        }
        // The bound itself still listens.
        Host::listen_loopback(&HostConfig {
            gameplay_fingerprint: FP,
            max_clients: MAX_PLAYERS as u16,
        })
        .unwrap();
    }

    #[test]
    fn host_shutdown_disconnects_everyone() {
        let mut host = host();
        let mut alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 1);

        host.shutdown();
        // The client's socket is dead: its next read errors instead of
        // idling on a lobby that no longer exists.
        assert!(alice.recv().is_err());
    }

    fn ad(tag: &str) -> SessionAdvertisement {
        SessionAdvertisement {
            summary: format!("summary {tag}"),
            params: format!("params {tag}").into_bytes(),
        }
    }

    #[test]
    fn a_newcomer_sees_the_session_before_its_first_roster() {
        let host = host();
        host.set_session(ad("one")).unwrap();
        let mut alice = join(host.addr(), "alice");
        host.recv_timeout(WAIT).unwrap(); // Joined

        match alice.recv().unwrap() {
            Message::Session(got) => assert_eq!(got, ad("one")),
            other => panic!("expected Session, got {other:?}"),
        }
        assert_eq!(recv_roster(&mut alice, 1).len(), 1);
    }

    #[test]
    fn a_session_change_rebroadcasts_to_everyone() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        let mut bob = join(host.addr(), "bob");
        host.recv_timeout(WAIT).unwrap();
        host.recv_timeout(WAIT).unwrap();
        recv_roster(&mut alice, 2);
        recv_roster(&mut bob, 2);

        host.set_session(ad("two")).unwrap();
        for client in [&mut alice, &mut bob] {
            match client.recv().unwrap() {
                Message::Session(got) => assert_eq!(got, ad("two")),
                other => panic!("expected Session, got {other:?}"),
            }
        }

        // Replacement, not accumulation: a third advertisement lands
        // alone, and a newcomer sees only the latest.
        host.set_session(ad("three")).unwrap();
        match alice.recv().unwrap() {
            Message::Session(got) => assert_eq!(got, ad("three")),
            other => panic!("expected Session, got {other:?}"),
        }
        let mut carol = join(host.addr(), "carol");
        match carol.recv().unwrap() {
            Message::Session(got) => assert_eq!(got, ad("three")),
            other => panic!("expected Session, got {other:?}"),
        }
    }

    #[test]
    fn a_lobby_without_a_session_sends_none() {
        let host = host();
        let mut alice = join(host.addr(), "alice");
        // No `Session` was ever set — the first message is the roster.
        match alice.recv().unwrap() {
            Message::Roster { .. } => {}
            other => panic!("expected Roster, got {other:?}"),
        }
    }

    /// `set_session` checks the payload against the wire bound before
    /// handing it to the loop — an over-cap params blob is a caller
    /// error, not a broadcast that silently fails every player.
    #[test]
    fn an_oversize_session_is_refused_before_the_wire() {
        let host = host();
        let ad = SessionAdvertisement {
            summary: "s".to_string(),
            params: vec![0; crate::MAX_SESSION_PARAMS + 1],
        };
        match host.set_session(ad) {
            Err(NetError::Proto(crate::proto::ProtoError::OversizeSessionParams(n))) => {
                assert_eq!(n, crate::MAX_SESSION_PARAMS + 1);
            }
            other => panic!("expected OversizeSessionParams, got {other:?}"),
        }
        // The loop never saw the bad payload: a join right after gets
        // only its roster, no Session.
        let mut alice = join(host.addr(), "alice");
        match alice.recv().unwrap() {
            Message::Roster { .. } => {}
            other => panic!("expected Roster, got {other:?}"),
        }
    }
}
