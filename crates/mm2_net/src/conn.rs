//! Blocking TCP connections and the session handshake on top of them.
//!
//! The transport is `std::net` TCP: reliable, ordered, and right for the
//! control plane (handshake, lobby, session commands). Sockets are
//! blocking; callers that must not block the main loop run them on a
//! thread — the game's Bevy side will bridge through channels (F24-B).
//! `listen_loopback` binds `127.0.0.1` only: no public listener exists
//! unless a caller explicitly asks for one.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use crate::NetError;
use crate::frame::{read_frame, write_frame};
use crate::proto::{Hello, Message, PROTOCOL_VERSION, RejectCode, admit};

/// One framed TCP connection.
#[derive(Debug)]
pub struct Conn {
    stream: TcpStream,
    peer: SocketAddr,
}

impl Conn {
    /// Connect to a listening peer.
    pub fn connect(addr: SocketAddr) -> Result<Self, NetError> {
        let stream = TcpStream::connect(addr)?;
        // Small latency-sensitive frames (Input/Snap) must not sit in
        // Nagle's buffer behind an ACK timer.
        stream.set_nodelay(true)?;
        let peer = stream.peer_addr()?;
        Ok(Self { stream, peer })
    }

    /// Accept the next pending connection on `listener`.
    pub fn accept(listener: &TcpListener) -> Result<Self, NetError> {
        let (stream, peer) = listener.accept()?;
        Ok(Self { stream, peer })
    }

    /// Where this connection's peer lives.
    pub fn peer_addr(&self) -> SocketAddr {
        self.peer
    }

    /// The local end of this connection — the address the peer sees us
    /// at, which a UDP lane socket binds its IP from.
    pub(crate) fn local_addr(&self) -> io::Result<SocketAddr> {
        self.stream.local_addr()
    }

    /// A second handle on the same socket for a dedicated reader thread;
    /// socket options (timeouts) are shared with the original.
    pub(crate) fn try_clone(&self) -> io::Result<Conn> {
        Ok(Conn {
            stream: self.stream.try_clone()?,
            peer: self.peer,
        })
    }

    /// Close both directions: any thread blocked in `recv` on a clone of
    /// this socket wakes with an error.
    pub(crate) fn shutdown_both(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }

    /// Wrap an already-accepted stream — the accept thread's hand-off to
    /// the host loop.
    pub(crate) fn from_stream(stream: TcpStream) -> io::Result<Self> {
        // See `connect` — the session data plane cannot sit in Nagle.
        stream.set_nodelay(true)?;
        let peer = stream.peer_addr()?;
        Ok(Self { stream, peer })
    }

    /// A second handle on the same socket for the sending side. The lobby
    /// host keeps a `Writer` per player so the event loop can broadcast
    /// while a reader thread owns the `Conn` for `recv`.
    pub fn writer(&self) -> io::Result<Writer> {
        Ok(Writer {
            stream: self.stream.try_clone()?,
        })
    }

    /// Half-close the send side: we are done writing, but keep reading
    /// until the peer closes. A socket dropped with unread inbound data
    /// resets (RST), so a clean shutdown must drain rather than just
    /// drop.
    pub(crate) fn shutdown_write(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Write);
    }

    /// Bound how long `recv`/`send` may block. The handshake helpers
    /// install [`HANDSHAKE_TIMEOUT`] so a stalled peer cannot hang a
    /// join forever, and clear it (`None`) once the session is
    /// established — the control channel is then allowed to idle
    /// between requests. Callers may set their own bound for later
    /// phases (e.g. a lobby wait).
    pub fn set_timeout(&self, timeout: Option<Duration>) -> Result<(), NetError> {
        self.stream.set_read_timeout(timeout)?;
        self.stream.set_write_timeout(timeout)?;
        Ok(())
    }

    /// Bound only how long `recv` may block, leaving the send bound as
    /// it was — for a drain that must not shorten its own final send.
    pub(crate) fn set_read_timeout(&self, timeout: Option<Duration>) -> Result<(), NetError> {
        self.stream.set_read_timeout(timeout)?;
        Ok(())
    }

    /// Bound only how long a send may block.
    pub(crate) fn set_write_timeout(&self, timeout: Option<Duration>) -> Result<(), NetError> {
        self.stream.set_write_timeout(timeout)?;
        Ok(())
    }

    /// Send one protocol message.
    pub fn send(&mut self, msg: &Message) -> Result<(), NetError> {
        write_frame(&mut self.stream, &msg.encode()?)
    }

    /// Receive one protocol message.
    pub fn recv(&mut self) -> Result<Message, NetError> {
        Ok(Message::decode(&read_frame(&mut self.stream)?)?)
    }
}

/// Listen on `127.0.0.1:0` — an ephemeral loopback port. Tests and
/// local play bind here; exposing a LAN interface is a deliberate,
/// separately configured choice (see `docs/research/net.md`).
pub fn listen_loopback() -> io::Result<TcpListener> {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
}

/// Designed upper bound on one handshake exchange: `Hello` out, the
/// verdict back — two small frames, so even a slow link finishes far
/// inside it. The handshake helpers install this deadline on entry, so
/// a peer that completes the TCP handshake and then goes silent stalls
/// a join for this long rather than forever.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Client side of the handshake: send our `Hello` and wait for the
/// host's verdict, bounded by [`HANDSHAKE_TIMEOUT`]. `Ok(())` means
/// accepted; `Err(NetError::Rejected)` carries the host's reason.
/// `hello.protocol` is the client's claim — callers pass
/// [`PROTOCOL_VERSION`]; tests pass something else to exercise the
/// gate.
pub fn send_hello(conn: &mut Conn, hello: &Hello) -> Result<(), NetError> {
    send_hello_within(conn, hello, HANDSHAKE_TIMEOUT)
}

/// [`send_hello`] with an explicit deadline — for callers that need a
/// bound other than [`HANDSHAKE_TIMEOUT`], and for tests, which use a
/// short one rather than waiting out the default.
///
/// The deadline is installed before any I/O; on `Ok` it is cleared so
/// the established control channel may idle, and on `Err` it is left
/// installed — a failed handshake's connection is expected to be
/// dropped.
pub fn send_hello_within(
    conn: &mut Conn,
    hello: &Hello,
    deadline: Duration,
) -> Result<(), NetError> {
    conn.set_timeout(Some(deadline))?;
    let verdict = conn
        .send(&Message::Hello(hello.clone()))
        .and_then(|()| conn.recv());
    let verdict = match verdict {
        Ok(Message::Accept) => Ok(()),
        Ok(Message::Reject { code, message }) => Err(NetError::Rejected { code, message }),
        Ok(_) => Err(NetError::Unexpected("expected Accept or Reject")),
        Err(e) => Err(e),
    };
    if verdict.is_ok() {
        conn.set_timeout(None)?;
    }
    verdict
}

/// Host side of the handshake: read the peer's first frame, apply the
/// compatibility gate and answer, bounded by [`HANDSHAKE_TIMEOUT`].
/// Returns the peer's `Hello` on acceptance — the lobby keeps the
/// driver name and build for the roster. A failure still answers the
/// peer with `Reject` when the reason is a protocol/content mismatch,
/// so a refused client sees a reason rather than a dropped socket.
pub fn accept_hello(conn: &mut Conn, gameplay_fingerprint: u64) -> Result<Hello, NetError> {
    accept_hello_within(conn, gameplay_fingerprint, HANDSHAKE_TIMEOUT)
}

/// [`accept_hello`] with an explicit deadline — same contract as
/// [`send_hello_within`]: installed before any I/O, cleared on `Ok`,
/// left installed on `Err`.
pub fn accept_hello_within(
    conn: &mut Conn,
    gameplay_fingerprint: u64,
    deadline: Duration,
) -> Result<Hello, NetError> {
    let hello = recv_hello_within(conn, gameplay_fingerprint, deadline)?;
    conn.send(&Message::Accept)?;
    conn.set_timeout(None)?;
    Ok(hello)
}

/// The receive-and-gate half of [`accept_hello_within`]: reads the peer's
/// first frame under `deadline`, applies the compatibility gate and sends
/// a `Reject` on refusal — but on success sends **nothing** and leaves the
/// deadline installed. The lobby host uses this so the roster-capacity
/// check happens *before* `Accept` goes out: the host loop answers
/// `Accept`/`Reject` itself once it knows there is a slot.
pub(crate) fn recv_hello_within(
    conn: &mut Conn,
    gameplay_fingerprint: u64,
    deadline: Duration,
) -> Result<Hello, NetError> {
    conn.set_timeout(Some(deadline))?;
    let hello = match conn.recv() {
        Ok(Message::Hello(h)) => h,
        Ok(_) => {
            send_reject(conn, RejectCode::Malformed, "first message must be Hello");
            return Err(NetError::Unexpected("first message was not Hello"));
        }
        Err(NetError::Proto(e)) => {
            send_reject(conn, RejectCode::Malformed, &e.to_string());
            return Err(e.into());
        }
        Err(e) => return Err(e),
    };
    match admit(&hello, gameplay_fingerprint) {
        Ok(()) => Ok(hello),
        Err((code, message)) => {
            send_reject(conn, code, &message);
            Err(NetError::Rejected { code, message })
        }
    }
}

fn send_reject(conn: &mut Conn, code: RejectCode, message: &str) {
    // Best effort: the peer may already be gone.
    let _ = conn.send(&Message::Reject {
        code,
        message: message.to_string(),
    });
}

/// The write half of a connection, cloned from the same socket —
/// [`Conn::writer`]. Socket options are shared with the `Conn`: a
/// timeout installed here bounds the owner's sends too. Two `Writer`s
/// on one socket can interleave frames — consumers that share one wrap
/// it in a mutex (see `lobby::ClientCtl`).
#[derive(Debug)]
pub struct Writer {
    stream: TcpStream,
}

impl Writer {
    /// Send one protocol message.
    pub fn send(&mut self, msg: &Message) -> Result<(), NetError> {
        write_frame(&mut self.stream, &msg.encode()?)
    }

    /// Bound how long `send` may block. Socket options are shared with the
    /// owning `Conn`, so the handshake deadline already bounds the first
    /// post-handshake writes; the host installs a fresh bound once the
    /// session clears `Conn`'s timeouts.
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> Result<(), NetError> {
        self.stream.set_write_timeout(timeout)?;
        Ok(())
    }

    /// Forcibly close the socket — a blocked `recv` on the peer's reader
    /// thread (or our own) wakes with an error. Used for host teardown.
    pub fn disconnect(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

/// Frames one [`PeerWriter`] queues before it refuses more. 256 frames
/// is over four seconds of 60 Hz snapshots: a peer that far behind is
/// stalled, and refusing is the bounded answer (the caller reaps it).
/// A designed bound.
pub const PEER_QUEUE_CAP: usize = 256;

/// A [`Writer`] behind a bounded queue and its own thread (NETPLAY-06E-4).
///
/// [`send`](Self::send) encodes the message and hands the frame to the
/// thread: it never touches the socket, so a peer that stopped reading
/// cannot stall the caller — the game thread for a client, the host loop
/// (and so every *other* peer) for a seat. Frames go out in the order
/// they were queued.
///
/// A peer that cannot keep up is bounded rather than waited on: a full
/// queue or a failed socket write makes this and every later `send`
/// return `Err`, and a failed write also closes the socket, so the
/// caller's existing "failed send = lost peer" discipline applies
/// unchanged. Handshake-time writes stay blocking on the plain
/// [`Writer`]; wrap it only once the handshake is done.
#[derive(Debug)]
pub struct PeerWriter {
    tx: Option<mpsc::SyncSender<Vec<u8>>>,
    failed: Arc<AtomicBool>,
    /// Set by the thread when it exits, drained or failed.
    done: Arc<AtomicBool>,
    stream: TcpStream,
}

impl PeerWriter {
    /// Take over `writer`'s socket and start its writer thread. The
    /// thread ends when the `PeerWriter` is dropped (after flushing what
    /// is queued) or the socket fails.
    pub fn spawn(writer: Writer) -> io::Result<Self> {
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(PEER_QUEUE_CAP);
        let failed = Arc::new(AtomicBool::new(false));
        let stream = writer.stream.try_clone()?;
        let flag = Arc::clone(&failed);
        let done = Arc::new(AtomicBool::new(false));
        let exited = Arc::clone(&done);
        let mut out = writer.stream;
        thread::Builder::new()
            .name("mm2-peer-writer".into())
            .spawn(move || {
                while let Ok(frame) = rx.recv() {
                    if write_frame(&mut out, &frame).is_err() {
                        flag.store(true, Ordering::Release);
                        // Wake the reader on this socket: the peer is gone.
                        let _ = out.shutdown(std::net::Shutdown::Both);
                        break;
                    }
                }
                exited.store(true, Ordering::Release);
            })?;
        Ok(Self {
            tx: Some(tx),
            failed,
            done,
            stream,
        })
    }

    /// Queue one protocol message. `Err` when the peer is dead, its
    /// queue is full, or the message does not encode.
    pub fn send(&mut self, msg: &Message) -> Result<(), NetError> {
        self.send_frame(msg.encode()?)
    }

    pub(crate) fn send_frame(&mut self, frame: Vec<u8>) -> Result<(), NetError> {
        if self.failed.load(Ordering::Acquire) {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe).into());
        }
        let Some(tx) = &self.tx else {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe).into());
        };
        match tx.try_send(frame) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(_)) => {
                Err(io::Error::new(io::ErrorKind::WouldBlock, "peer send queue full").into())
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                Err(io::Error::from(io::ErrorKind::BrokenPipe).into())
            }
        }
    }

    /// Stop accepting frames; the thread drains what is queued and exits.
    /// Pair with [`is_drained`](Self::is_drained) and
    /// [`disconnect`](Self::disconnect) for an orderly close.
    pub fn close_send(&mut self) {
        self.tx = None;
    }

    /// Whether the writer thread has written everything queued (or
    /// failed) and exited. Only meaningful after [`close_send`](Self::close_send).
    pub fn is_drained(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    /// Stop accepting frames, give the thread up to `deadline` to write
    /// what is queued, then half-close the send side so the peer reads
    /// the frames and a clean FIN. Returns whether everything was
    /// written: `false` when the deadline passed or the socket failed.
    /// The bound keeps a stalled peer from hanging an exit; the read
    /// side is left open for the caller to drain.
    pub fn flush_and_close_send(&mut self, deadline: Duration) -> bool {
        self.close_send();
        let by = std::time::Instant::now() + deadline;
        while !self.is_drained() && std::time::Instant::now() < by {
            thread::sleep(Duration::from_millis(1));
        }
        let flushed = self.is_drained() && !self.failed.load(Ordering::Acquire);
        if flushed {
            let _ = self.stream.shutdown(std::net::Shutdown::Write);
        }
        flushed
    }

    /// Forcibly close the socket: a write parked in the thread and a
    /// blocked `recv` on the peer's reader both wake with an error.
    pub fn disconnect(&mut self) {
        self.tx = None;
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

/// Compose a `Hello` at the current protocol version.
pub fn hello(build: String, driver: String, gameplay_fingerprint: u64) -> Hello {
    Hello {
        protocol: PROTOCOL_VERSION,
        gameplay_fingerprint,
        build,
        driver,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;

    const TIMEOUT: Duration = Duration::from_secs(10);

    /// Run `host` on a loopback listener thread and `client` on a fresh
    /// connection; returns the client thread's result. `backstop` is a
    /// safety timeout applied to both ends so a broken test cannot hang
    /// the suite — legs exercising the handshake's own deadline
    /// management pass `None` and must not pre-set one themselves.
    fn pair_with<H, C, R>(backstop: Option<Duration>, host: H, client: C) -> R
    where
        H: FnOnce(Conn) + Send + 'static,
        C: FnOnce(Conn) -> R + Send + 'static,
        R: Send + 'static,
    {
        let listener = listen_loopback().unwrap();
        let addr = listener.local_addr().unwrap();
        let (ready, wait) = mpsc::channel();
        let host_thread = thread::spawn(move || {
            ready.send(()).unwrap();
            let conn = Conn::accept(&listener).unwrap();
            conn.set_timeout(backstop).unwrap();
            host(conn);
        });
        wait.recv().unwrap();
        let conn = Conn::connect(addr).unwrap();
        conn.set_timeout(backstop).unwrap();
        let out = client(conn);
        host_thread.join().unwrap();
        out
    }

    fn pair<H, C, R>(host: H, client: C) -> R
    where
        H: FnOnce(Conn) + Send + 'static,
        C: FnOnce(Conn) -> R + Send + 'static,
        R: Send + 'static,
    {
        pair_with(Some(TIMEOUT), host, client)
    }

    /// The no-deadline leg: sockets start unbounded, so only the
    /// handshake helpers themselves can bound the exchange.
    fn pair_untimed<H, C, R>(host: H, client: C) -> R
    where
        H: FnOnce(Conn) + Send + 'static,
        C: FnOnce(Conn) -> R + Send + 'static,
        R: Send + 'static,
    {
        pair_with(None, host, client)
    }

    #[test]
    fn matching_peers_complete_the_handshake() {
        let result: Result<(), NetError> = pair(
            |mut conn| {
                let hello = accept_hello(&mut conn, 0xaaaa).unwrap();
                assert_eq!(hello.driver, "driver one");
                assert_eq!(hello.protocol, PROTOCOL_VERSION);
                // Established session: the deadline is cleared so the
                // control channel may idle.
                assert_eq!(conn.stream.read_timeout().unwrap(), None);
                assert_eq!(conn.stream.write_timeout().unwrap(), None);
            },
            |mut conn| {
                send_hello(
                    &mut conn,
                    &hello("build".to_string(), "driver one".to_string(), 0xaaaa),
                )?;
                assert_eq!(conn.stream.read_timeout().unwrap(), None);
                assert_eq!(conn.stream.write_timeout().unwrap(), None);
                Ok(())
            },
        );
        result.unwrap();
    }

    #[test]
    fn the_handshake_helpers_install_the_default_deadline() {
        // Neither socket has a timeout before the handshake runs —
        // `pair_untimed` sets none. After a *failed* handshake the
        // helpers' default deadline is what remains installed.
        pair_untimed(
            |mut conn| {
                let err = accept_hello(&mut conn, 0xaaaa).unwrap_err();
                assert!(matches!(
                    err,
                    NetError::Rejected {
                        code: RejectCode::VersionMismatch,
                        ..
                    }
                ));
                assert_eq!(conn.stream.read_timeout().unwrap(), Some(HANDSHAKE_TIMEOUT));
                assert_eq!(
                    conn.stream.write_timeout().unwrap(),
                    Some(HANDSHAKE_TIMEOUT)
                );
            },
            |mut conn| {
                let err = send_hello(
                    &mut conn,
                    &Hello {
                        protocol: PROTOCOL_VERSION + 1,
                        gameplay_fingerprint: 0xaaaa,
                        build: "build".to_string(),
                        driver: "d".to_string(),
                    },
                )
                .unwrap_err();
                assert!(matches!(
                    err,
                    NetError::Rejected {
                        code: RejectCode::VersionMismatch,
                        ..
                    }
                ));
                assert_eq!(conn.stream.read_timeout().unwrap(), Some(HANDSHAKE_TIMEOUT));
                assert_eq!(
                    conn.stream.write_timeout().unwrap(),
                    Some(HANDSHAKE_TIMEOUT)
                );
            },
        );
    }

    /// A peer that completes TCP and then goes silent must not hang the
    /// host: the installed deadline fires and `accept_hello` errors out.
    /// Uses the `_within` variant so the leg does not wait out the 10 s
    /// default.
    #[test]
    fn a_silent_client_cannot_stall_accept_hello() {
        pair(
            |mut conn| match accept_hello_within(&mut conn, 0xaaaa, Duration::from_millis(150)) {
                Err(NetError::Io(e)) => assert!(
                    matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ),
                    "expected a read timeout, got {e}"
                ),
                other => panic!("expected a read timeout, got {other:?}"),
            },
            |conn| {
                // Hold the connection open but send nothing — well past
                // the host's handshake deadline.
                thread::sleep(Duration::from_millis(500));
                drop(conn);
            },
        );
    }

    /// Symmetric leg: a host that never answers must not hang the
    /// client's `send_hello` either.
    #[test]
    fn a_silent_host_cannot_stall_send_hello() {
        let result = pair(
            |conn| {
                thread::sleep(Duration::from_millis(500));
                drop(conn);
            },
            |mut conn| {
                send_hello_within(
                    &mut conn,
                    &hello("b".to_string(), "d".to_string(), 0xaaaa),
                    Duration::from_millis(150),
                )
            },
        );
        match result {
            Err(NetError::Io(e)) => assert!(
                matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ),
                "expected a read timeout, got {e}"
            ),
            other => panic!("expected a read timeout, got {other:?}"),
        }
    }

    #[test]
    fn version_mismatch_is_rejected_with_a_reason() {
        let result = pair(
            |mut conn| {
                let err = accept_hello(&mut conn, 0xaaaa).unwrap_err();
                assert!(matches!(
                    err,
                    NetError::Rejected {
                        code: RejectCode::VersionMismatch,
                        ..
                    }
                ));
            },
            |mut conn| {
                send_hello(
                    &mut conn,
                    &Hello {
                        protocol: PROTOCOL_VERSION + 1,
                        gameplay_fingerprint: 0xaaaa,
                        build: "build".to_string(),
                        driver: "d".to_string(),
                    },
                )
            },
        );
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            NetError::Rejected {
                code: RejectCode::VersionMismatch,
                ..
            }
        ));
    }

    #[test]
    fn content_mismatch_is_rejected() {
        let result = pair(
            |mut conn| {
                let err = accept_hello(&mut conn, 0xaaaa).unwrap_err();
                assert!(matches!(
                    err,
                    NetError::Rejected {
                        code: RejectCode::ContentMismatch,
                        ..
                    }
                ));
            },
            |mut conn| send_hello(&mut conn, &hello("b".to_string(), "d".to_string(), 0xbbbb)),
        );
        assert!(matches!(
            result.unwrap_err(),
            NetError::Rejected {
                code: RejectCode::ContentMismatch,
                ..
            }
        ));
    }

    #[test]
    fn a_non_hello_first_message_is_rejected() {
        let result = pair(
            |mut conn| {
                let err = accept_hello(&mut conn, 0xaaaa).unwrap_err();
                assert!(matches!(err, NetError::Unexpected(_)));
            },
            |mut conn| {
                conn.send(&Message::Accept).unwrap();
                conn.recv()
            },
        );
        // The host's Malformed reject reaches the client.
        assert!(matches!(
            result.unwrap(),
            Message::Reject {
                code: RejectCode::Malformed,
                ..
            }
        ));
    }

    #[test]
    fn an_oversize_frame_does_not_allocate() {
        use std::io::Write;
        pair(
            |mut conn| {
                let err = conn.recv().unwrap_err();
                assert!(matches!(err, NetError::Oversize { .. }));
            },
            |mut conn| {
                // Bypass the encoder and declare a giant frame.
                conn.stream
                    .write_all(&(crate::MAX_FRAME + 1).to_le_bytes())
                    .unwrap();
            },
        )
    }

    #[test]
    fn peers_are_loopback() {
        let listener = listen_loopback().unwrap();
        let addr = listener.local_addr().unwrap();
        assert!(addr.ip().is_loopback());
        let conn = Conn::connect(addr).unwrap();
        let (peer_conn, _) = listener.accept().unwrap();
        assert!(conn.peer_addr().ip().is_loopback());
        assert!(peer_conn.peer_addr().unwrap().ip().is_loopback());
    }

    /// A frame queued right before `flush_and_close_send` reaches the
    /// peer, followed by a clean end of stream.
    #[test]
    fn flush_and_close_send_delivers_the_queued_frames() {
        use std::io::Read;
        let (mut writer, mut far) = peer_pair();
        far.set_read_timeout(Some(TIMEOUT)).unwrap();
        for _ in 0..50 {
            writer.send(&Message::Leave).unwrap();
        }
        assert!(writer.flush_and_close_send(Duration::from_secs(5)));
        for _ in 0..50 {
            assert_eq!(
                Message::decode(&read_frame(&mut far).unwrap()).unwrap(),
                Message::Leave
            );
        }
        let mut rest = Vec::new();
        far.read_to_end(&mut rest).unwrap();
        assert!(rest.is_empty());
    }

    /// A stalled peer bounds the flush instead of hanging it.
    #[test]
    fn flush_and_close_send_gives_up_on_a_stalled_peer() {
        let (mut writer, _far) = peer_pair();
        let big = vec![0u8; 200 * 1024];
        for _ in 0..64 {
            let _ = writer.send_frame(big.clone());
        }
        let start = std::time::Instant::now();
        assert!(!writer.flush_and_close_send(Duration::from_millis(100)));
        assert!(start.elapsed() < TIMEOUT);
        writer.disconnect();
    }

    /// A connected loopback pair: the accepted side wrapped in a
    /// `PeerWriter`, plus the raw client end.
    fn peer_pair() -> (PeerWriter, TcpStream) {
        let listener = listen_loopback().unwrap();
        let addr = listener.local_addr().unwrap();
        let far = TcpStream::connect(addr).unwrap();
        let conn = Conn::accept(&listener).unwrap();
        (PeerWriter::spawn(conn.writer().unwrap()).unwrap(), far)
    }

    /// NETPLAY-06E-4: a peer that never reads parks the writer thread
    /// mid-write. Sends keep returning promptly, the bounded queue then
    /// refuses (so the caller can reap the peer), and nothing waits on
    /// the socket.
    #[test]
    fn a_send_to_a_stalled_peer_returns_promptly_and_is_bounded() {
        let (mut writer, _far) = peer_pair();
        let big = vec![0u8; 200 * 1024];
        let start = std::time::Instant::now();
        let mut refused_at = None;
        for n in 0..PEER_QUEUE_CAP * 4 {
            if writer.send_frame(big.clone()).is_err() {
                refused_at = Some(n);
                break;
            }
        }
        let n = refused_at.expect("an unread peer never filled its bounded queue");
        assert!(n >= PEER_QUEUE_CAP, "refused after only {n} frames");
        assert!(
            start.elapsed() < TIMEOUT,
            "sends to a stalled peer took {:?}",
            start.elapsed()
        );
        // Still refused, and `disconnect` frees the parked thread.
        assert!(writer.send(&Message::Accept).is_err());
        writer.disconnect();
    }

    /// Queued frames reach the peer whole and in order.
    #[test]
    fn queued_sends_arrive_in_order() {
        let (mut writer, far) = peer_pair();
        for id in 1..=20u16 {
            writer.send(&Message::Welcome { player_id: id }).unwrap();
        }
        let mut far = Conn::from_stream(far).unwrap();
        far.set_timeout(Some(TIMEOUT)).unwrap();
        for id in 1..=20u16 {
            assert_eq!(far.recv().unwrap(), Message::Welcome { player_id: id });
        }
    }

    /// A socket that fails under the writer thread makes later sends
    /// `Err` instead of queueing into the void.
    #[test]
    fn a_dead_socket_fails_later_sends() {
        let (mut writer, far) = peer_pair();
        drop(far);
        let deadline = std::time::Instant::now() + TIMEOUT;
        while writer.send(&Message::Accept).is_ok() {
            assert!(std::time::Instant::now() < deadline, "send never failed");
            thread::sleep(Duration::from_millis(5));
        }
    }
}
