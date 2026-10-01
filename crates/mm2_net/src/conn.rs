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
}
