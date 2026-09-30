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
pub struct Conn {
    stream: TcpStream,
    peer: SocketAddr,
}

impl Conn {
    /// Connect to a listening peer.
    pub fn connect(addr: SocketAddr) -> Result<Self, NetError> {
        let stream = TcpStream::connect(addr)?;
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

    /// Bound how long `recv`/`send` may block. The handshake sets this so
    /// a stalled peer cannot hang a join forever; an established session
    /// clears it (`None`) since the control channel is allowed to idle.
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

/// Client side of the handshake: send our `Hello` and wait for the
/// host's verdict. `Ok(())` means accepted; `Err(NetError::Rejected)`
/// carries the host's reason. `hello.protocol` is the client's claim —
/// callers pass [`PROTOCOL_VERSION`]; tests pass something else to
/// exercise the gate.
pub fn send_hello(conn: &mut Conn, hello: &Hello) -> Result<(), NetError> {
    conn.send(&Message::Hello(hello.clone()))?;
    match conn.recv()? {
        Message::Accept => Ok(()),
        Message::Reject { code, message } => Err(NetError::Rejected { code, message }),
        _ => Err(NetError::Unexpected("expected Accept or Reject")),
    }
}

/// Host side of the handshake: read the peer's first frame, apply the
/// compatibility gate and answer. Returns the peer's `Hello` on
/// acceptance — the lobby keeps the driver name and build for the
/// roster. A failure still answers the peer with `Reject` when the
/// reason is a protocol/content mismatch, so a refused client sees a
/// reason rather than a dropped socket.
pub fn accept_hello(conn: &mut Conn, gameplay_fingerprint: u64) -> Result<Hello, NetError> {
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
        Ok(()) => {
            conn.send(&Message::Accept)?;
            Ok(hello)
        }
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
    /// connection; returns the client thread's result.
    fn pair<H, C, R>(host: H, client: C) -> R
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
            conn.set_timeout(Some(TIMEOUT)).unwrap();
            host(conn);
        });
        wait.recv().unwrap();
        let conn = Conn::connect(addr).unwrap();
        conn.set_timeout(Some(TIMEOUT)).unwrap();
        let out = client(conn);
        host_thread.join().unwrap();
        out
    }

    #[test]
    fn matching_peers_complete_the_handshake() {
        let result = pair(
            |mut conn| {
                let hello = accept_hello(&mut conn, 0xaaaa).unwrap();
                assert_eq!(hello.driver, "driver one");
                assert_eq!(hello.protocol, PROTOCOL_VERSION);
            },
            |mut conn| {
                send_hello(
                    &mut conn,
                    &hello("build".to_string(), "driver one".to_string(), 0xaaaa),
                )
            },
        );
        result.unwrap();
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
