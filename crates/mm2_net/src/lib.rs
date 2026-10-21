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
//! - [`lobby`] — the session lobby driver: [`Host`] accepts and gates
//!   peers on its own threads, mints roster slots, tracks readiness and
//!   vehicle picks, runs the consumer's start/cancel under the
//!   per-session [`LateJoin`](lobby::LateJoin) policy, and reports
//!   join/leave/ready/pick/session transitions as events; [`Client`] is
//!   the joining side. The lobby's socket pair also carries the F25-A
//!   session data plane: `Input` frames land in the host's
//!   [`RemoteInputs`](lobby::RemoteInputs) mailbox (latest-wins per
//!   roster slot, on the sender's `seq`), `ResetRequest`s absorb
//!   alongside them (F25-B), and `Snap` snapshots broadcast to every
//!   peer.
//! - [`impair`] — the deterministic impairment harness (F25-B, spec
//!   req 6): an [`ImpairProxy`](impair::ImpairProxy) is a framed TCP
//!   relay a test inserts between a peer and a host, applying a seeded
//!   per-direction recipe of delay, jitter, loss, duplication and
//!   reorder while [`LinkStats`](impair::LinkStats) counts what the
//!   recipe actually did.
//! - [`impair_udp`] — the same recipe idea at the datagram boundary
//!   (NETPLAY-06B): a [`UdpImpairProxy`](impair_udp::UdpImpairProxy)
//!   relays the lane's UDP datagrams on a virtual clock with loss, burst
//!   loss, reorder, duplication, delay, an effective-MTU limit and a
//!   black hole, per direction, with per-direction and per-tag counters.
//!   [`impair`] is the legacy TCP-frame proxy.
//! - [`rt`] — the UDP datagram lane (NETPLAY-06A): `InputWindow` and
//!   `Snap` ride sequenced unreliable datagrams beside the TCP control
//!   connection, authenticated per seat (token, source address,
//!   generation, replay window) and handed to the game through bounded
//!   nonblocking drop-oldest queues.
//!
//! Scope deliberately ends at transport: the session advertisement is
//! an opaque blob (the `mm2_app` bridge owns the `SessionConfig`
//! mapping) and what `Input`/`Snap` payloads *mean* — vehicle state,
//! interpolation, reconciliation — is the app's F25/F26 domain, not a
//! wire-layer concern. See `docs/research/net.md` for the transport
//! decision record.

mod cmdbuf;
mod conn;
mod frag;
mod frame;
mod impair;
mod impair_udp;
mod lobby;
mod netcmd;
mod pace;
mod proto;
mod rt;

#[cfg(test)]
mod lane_impair_tests;
#[cfg(test)]
mod lane_pace_tests;
#[cfg(test)]
mod lane_tests;

pub use cmdbuf::{
    BUFFER_CAP, COMMAND_BURST, COMMAND_RATE_HZ, Consumed, HOLD_LAST_TICKS, NEUTRAL, SeatStats,
    Source,
};
pub use conn::{
    Conn, HANDSHAKE_TIMEOUT, Writer, accept_hello, accept_hello_within, hello, listen_loopback,
    send_hello, send_hello_within,
};
pub use frag::{
    Assembler, AssemblerStats, BASELINE_CAP, BaselineReceiver, BaselineSender, Completed,
    FRAG_DATA, FRAG_HEADER, FRAGMENT_TTL, FragRefuse, MAX_ASSEMBLIES, MAX_FRAGMENTS, MAX_GROUPS,
    Plan, REFRESH_TICKS, SplitError, StateError, delta_apply, delta_encode,
    split as split_fragments,
};
pub use frame::{MAX_FRAME, read_frame, write_frame};
pub use impair::{Impair, ImpairProxy, LinkDir, LinkStats};
pub use impair_udp::{UdpImpair, UdpImpairProxy, UdpStats};
pub use lobby::{
    Client, ClientCtl, Host, HostConfig, HostCtl, HostEvent, INPUT_BURST, INPUT_RATE_HZ,
    KEEPALIVE_INTERVAL, LIFECYCLE_RESEND_WINDOW, LaneRoute, LateJoin, LeaveCause, PickValidator,
    RemoteInputs, StampedInput, Transport,
};
pub use netcmd::{REMOTE_BUFFER_CAP, RemoteCommands, RemoteSeatStats};
pub use pace::{
    BULK_MAX_AGE, BULK_QUEUE_CAP, Budget, Cadence, MAX_DEBT_BYTES, MAX_SNAPSHOT_HZ,
    MOVEMENT_MAX_AGE, MOVEMENT_QUEUE_CAP, PEER_BURST_BYTES, PEER_BYTES_PER_SEC, Priority,
    SNAPSHOT_HZ,
};
pub use proto::{
    DriveInput, FIRST_RESERVED_PLAYER_ID, Hello, MAX_INPUT_WINDOW, MAX_PLAYERS, MAX_REMOTE_RUN,
    MAX_REMOTE_SEATS, MAX_SESSION_PARAMS, MAX_SNAP_CARS, MAX_SNAP_CNR_SEATS, MAX_SNAP_IMPACTS,
    MAX_SNAP_LEAVES, MAX_SNAP_PROPS, MAX_SNAP_SIGNALS, MAX_STRING, Message, PROTOCOL_VERSION,
    ProtoError, RejectCode, RemoteCommand, RemoteSeat, RosterEntry, SIGNAL_NO_GREEN,
    SNAP_CNR_NO_PLAYER, SNAP_FLAG_BRAKE, SNAP_FLAG_GROUNDED, SNAP_FLAG_REVERSE, SNAP_NO_FRAGMENT,
    SNAP_NO_SURFACE, SessionAdvertisement, SiteTable, SnapCar, SnapCnr, SnapCnrOutcome,
    SnapCnrSeat, SnapEntry, SnapImpact, SnapLeaf, SnapProp, SnapRace, SnapSignal, SnapTrailer,
    TickCommand, VehiclePick,
};
pub use rt::{
    HEADER_LEN, Header, LANE_ECHO_TIMEOUT, LANE_VERSION, LaneAck, LaneCounts, LaneQueue, LaneTag,
    MAGIC, MAX_DATAGRAM, MAX_PAYLOAD, PaceStats, QUEUE_CAP, QueueStats, SEQ_WINDOW, SeqWindow,
    decode_datagram, encode_datagram,
};

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
    /// The UDP datagram lane never came up: the host did not echo our
    /// probes. The TCP connection is fine; UDP between the two machines
    /// is not.
    #[error(
        "no UDP lane echo from {host} within {waited:?}: the host's UDP port {} is blocked or \
         unreachable — allow inbound UDP on that port (host firewall / router port forward), \
         or the path's MTU is below the {} bytes a lane datagram needs (tunnel / VPN); see \
         docs/multiplayer-reachability.md",
        .host.port(),
        crate::rt::MAX_DATAGRAM
    )]
    LaneTimeout {
        /// The host's UDP endpoint that stayed silent.
        host: std::net::SocketAddr,
        /// How long we probed.
        waited: std::time::Duration,
    },
    /// A caller configuration the wire cannot represent — e.g.
    /// `HostConfig::max_clients` above [`MAX_PLAYERS`].
    #[error("invalid host configuration: {0}")]
    Config(&'static str),
}
