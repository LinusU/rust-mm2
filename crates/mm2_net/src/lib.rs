//! Engine-to-engine networking foundation (F24-A).
//!
//! The model is **host-authoritative**: one process owns the simulation
//! truth (the `AuthorityRole::Authority` side of `mm2_game`'s contract);
//! clients send requests and receive state — they never set positions,
//! score, damage or unlock state themselves. This crate carries the
//! transport and wire protocol that model runs on:
//!
//! - [`frame`] — length-prefixed messages over a byte stream, bounded by
//!   [`MAX_FRAME`](frame::MAX_FRAME) before any allocation.
//! - [`proto`] — the typed message set (`Hello`/`Accept`/`Reject`),
//!   [`PROTOCOL_VERSION`] and the handshake's compatibility gate:
//!   protocol version and the *gameplay* content fingerprint
//!   (`mm2_content::fingerprint::gameplay`) must match; cosmetic-only
//!   differences (textures, audio, lighting) do not move it.
//! - [`conn`] — blocking TCP streams behind `std::net`, with the
//!   client/server handshake helpers. Loopback listeners are the
//!   default for tests; LAN binding is the caller's explicit choice.
//!
//! Scope deliberately ends at the session handshake. Lobby state,
//! vehicle-state replication and the gameplay dataplane are F24-B/F25
//! work on top of this layer. See `docs/research/net.md` for the
//! transport decision record.

mod conn;
mod frame;
mod proto;

pub use conn::{Conn, accept_hello, hello, listen_loopback, send_hello};
pub use frame::{MAX_FRAME, read_frame, write_frame};
pub use proto::{Hello, Message, PROTOCOL_VERSION, ProtoError, RejectCode};

/// Every failure mode of this layer: transport I/O, frame bounds, wire
/// decoding and a peer that refused us — or that we refused.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    /// Underlying stream error (connect refused, reset, timeout, EOF).
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    /// A frame declared more bytes than [`MAX_FRAME`] allows.
    #[error("frame declares {len} bytes, bound is {max}")]
    Oversize {
        /// Declared payload length.
        len: u32,
        /// The configured bound.
        max: u32,
    },
    /// Wire decode failure inside an otherwise well-framed message.
    #[error("protocol: {0}")]
    Proto(#[from] ProtoError),
    /// The peer rejected the handshake.
    #[error("rejected: {message}")]
    Rejected {
        /// Machine-readable reason.
        code: RejectCode,
        /// Human-readable detail.
        message: String,
    },
    /// A message arrived that does not fit the handshake contract —
    /// the peer's first frame was not `Hello`.
    #[error("unexpected message: {0}")]
    Unexpected(&'static str),
}
