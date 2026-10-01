//! The wire protocol: message types, encoding and the compatibility
//! gate the handshake applies.
//!
//! All integers are little-endian. Strings are `u16le`-length-prefixed
//! UTF-8 bounded by [`MAX_STRING`]. Decoding is strict: unknown type
//! bytes, truncated fields, over-long strings and trailing bytes are
//! all errors — a peer speaking something we cannot name is a protocol
//! violation, not a parse to fudge.

/// Wire version. Bumped for any incompatible message change; peers must
/// match exactly. v2: `RosterEntry` gained the driver's `pick` field and
/// the `SetVehicle`/`VehicleRefused` negotiation pair landed.
pub const PROTOCOL_VERSION: u16 = 2;

/// Byte cap on any length-prefixed string field.
pub const MAX_STRING: usize = 256;

/// Roster ceiling: MP-1 (documented — `help:Types of Multiplayer
/// Connections`, `help:Multiplayer Screen`) puts TCP/IP play at up to 8
/// players total. The wire keeps the bound so a hostile roster payload
/// cannot claim an unbounded crowd.
pub const MAX_PLAYERS: u8 = 8;

const TAG_HELLO: u8 = 0x01;
const TAG_ACCEPT: u8 = 0x02;
const TAG_REJECT: u8 = 0x03;
const TAG_WELCOME: u8 = 0x04;
const TAG_ROSTER: u8 = 0x05;
const TAG_SET_READY: u8 = 0x06;
const TAG_LEAVE: u8 = 0x07;
const TAG_SESSION: u8 = 0x08;
const TAG_SET_VEHICLE: u8 = 0x09;
const TAG_VEHICLE_REFUSED: u8 = 0x0a;

/// Byte cap on a [`SessionAdvertisement`]'s opaque `params` field — the
/// `mm2_app` bridge's serialized session config is a few hundred bytes,
/// so 4 KiB is far above need while still trivially bounded.
pub const MAX_SESSION_PARAMS: usize = 4096;

/// What the lobby is configured to run — host → clients, opaque to the
/// wire. `mm2_net` knows nothing about cities, modes or settings: the
/// `mm2_app` bridge owns the `params` encoding and both peers decode it
/// with identical code at an identical [`PROTOCOL_VERSION`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAdvertisement {
    /// One-line display summary for lobby UIs/CLIs
    /// (`"sf, cruise, amateur"`).
    pub summary: String,
    /// The engine's own encoding of the session parameters — bounded by
    /// [`MAX_SESSION_PARAMS`]; the lobby bounds and carries it but never
    /// parses it.
    pub params: Vec<u8>,
}

/// The first message a client sends: identity plus the compatibility
/// evidence the host checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// Protocol version the client speaks.
    pub protocol: u16,
    /// `mm2_content::fingerprint::gameplay` over the client's resolved
    /// content — tuning, bounds, geometry, city and event data. A
    /// cosmetic-only mod leaves this untouched.
    pub gameplay_fingerprint: u64,
    /// Engine build identifier (diagnostic, not a gate).
    pub build: String,
    /// Driver name for the lobby roster.
    pub driver: String,
}

/// Why a host refused a `Hello`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectCode {
    /// `Hello.protocol` != [`PROTOCOL_VERSION`].
    VersionMismatch = 1,
    /// Gameplay fingerprints differ — different content, so different
    /// rules. Cosmetic-only differences never reach this.
    ContentMismatch = 2,
    /// The first frame was not a well-formed `Hello`.
    Malformed = 3,
    /// The roster was already at capacity when the peer handshook.
    LobbyFull = 4,
}

/// A driver's vehicle pick as the lobby carries it: an opaque content
/// id plus a paint index. The wire bounds both (`vehicle` ≤
/// [`MAX_STRING`], `paint` a `u8`) but knows nothing about which ids or
/// paints are legal — that is the consumer's validator
/// (`HostConfig::pick_validator`), which runs on the authoritative side.
/// What `vehicle` *means* is likewise the consumer's: `mm2_app` carries
/// catalog ids (`vpbug`) and uses the empty string for the synthetic dev
/// car.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VehiclePick {
    /// The picked vehicle's content id (consumer-defined).
    pub vehicle: String,
    /// Zero-based paint index.
    pub paint: u8,
}

/// One roster entry — the lobby's view of a connected driver. Player ids
/// are host-minted `u16` slots; `0` is reserved for the host player at the
/// app layer, so wire ids start at 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterEntry {
    /// Host-assigned slot.
    pub player_id: u16,
    /// Display name from the peer's `Hello`.
    pub driver: String,
    /// Build identifier from the peer's `Hello` (diagnostic).
    pub build: String,
    /// Whether the peer has marked itself ready to start.
    pub ready: bool,
    /// The driver's current vehicle pick; `None` until the peer sets one.
    pub pick: Option<VehiclePick>,
}

/// One wire message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Client → host hello; always the first frame.
    Hello(Hello),
    /// Host → client acceptance of the handshake.
    Accept,
    /// Host → client refusal, with a reason the UI/CLI can show.
    Reject {
        /// Machine-readable reason.
        code: RejectCode,
        /// Human-readable detail.
        message: String,
    },
    /// Host → client slot assignment, sent right after `Accept`.
    Welcome {
        /// The roster slot the client now owns.
        player_id: u16,
    },
    /// Host → clients: the session this lobby is configured to run.
    /// Broadcast to everyone when the host sets it and sent to each
    /// newcomer between `Welcome` and the roster — like `Roster` it is a
    /// complete snapshot, not a delta, so receivers replace wholesale.
    Session(SessionAdvertisement),
    /// Host → every client: the complete roster after any change. The
    /// roster is authoritative state, not a delta — receivers replace
    /// theirs wholesale.
    Roster {
        /// Every connected player, in slot order.
        players: Vec<RosterEntry>,
    },
    /// Client → host: toggle this player's readiness flag.
    SetReady {
        /// The new readiness state.
        ready: bool,
    },
    /// Client → host: set this player's vehicle pick. The host applies
    /// it through the consumer's pick validator and rebroadcasts the
    /// roster, or answers this peer alone with `VehicleRefused` — a bad
    /// pick is a refused request, not a protocol violation.
    SetVehicle(VehiclePick),
    /// Host → the refused client only: its `SetVehicle` pick failed the
    /// host's validator, with a display-ready reason. The roster is
    /// unchanged — receivers must not treat this as an error that ends
    /// the connection.
    VehicleRefused {
        /// Why the pick was refused.
        reason: String,
    },
    /// Client → host: a clean quit. Distinguishes a deliberate leave from
    /// a dropped connection on the wire.
    Leave,
}

/// A wire-decode failure on a well-framed payload.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtoError {
    /// Unknown message tag.
    #[error("unknown message tag {0:#04x}")]
    BadTag(u8),
    /// Unknown [`RejectCode`].
    #[error("unknown reject code {0}")]
    BadRejectCode(u8),
    /// Fewer bytes than the field needs.
    #[error("truncated field")]
    Truncated,
    /// A string field declared more than [`MAX_STRING`] bytes.
    #[error("string field over {MAX_STRING} bytes")]
    OversizeString,
    /// A string field was not valid UTF-8.
    #[error("string field is not UTF-8")]
    InvalidUtf8,
    /// Bytes left over after the message's last field.
    #[error("{0} trailing bytes")]
    Trailing(usize),
    /// A bool field carried a byte other than 0 or 1.
    #[error("invalid bool byte {0}")]
    InvalidBool(u8),
    /// A roster declared more than [`MAX_PLAYERS`] entries.
    #[error("roster declares {0} players, bound is {MAX_PLAYERS}")]
    OversizeRoster(u8),
    /// A session advertisement's `params` field declared more than
    /// [`MAX_SESSION_PARAMS`] bytes.
    #[error("session params declare {0} bytes, bound is {MAX_SESSION_PARAMS}")]
    OversizeSessionParams(usize),
}

impl RejectCode {
    fn to_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(v: u8) -> Result<Self, ProtoError> {
        match v {
            1 => Ok(Self::VersionMismatch),
            2 => Ok(Self::ContentMismatch),
            3 => Ok(Self::Malformed),
            4 => Ok(Self::LobbyFull),
            other => Err(ProtoError::BadRejectCode(other)),
        }
    }
}

/// Bounds-checked little-endian decode cursor.
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ProtoError> {
        let end = self.pos.checked_add(n).ok_or(ProtoError::Truncated)?;
        let out = self.buf.get(self.pos..end).ok_or(ProtoError::Truncated)?;
        self.pos = end;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, ProtoError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ProtoError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, ProtoError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn bool(&mut self) -> Result<bool, ProtoError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(ProtoError::InvalidBool(other)),
        }
    }

    fn string(&mut self) -> Result<String, ProtoError> {
        let len = self.u16()? as usize;
        if len > MAX_STRING {
            return Err(ProtoError::OversizeString);
        }
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| ProtoError::InvalidUtf8)
    }

    fn finish(self) -> Result<(), ProtoError> {
        let left = self.buf.len() - self.pos;
        if left > 0 {
            Err(ProtoError::Trailing(left))
        } else {
            Ok(())
        }
    }
}

fn put_string(out: &mut Vec<u8>, s: &str) -> Result<(), ProtoError> {
    if s.len() > MAX_STRING {
        return Err(ProtoError::OversizeString);
    }
    out.extend_from_slice(&(s.len() as u16).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

impl Message {
    /// Serialize into one frame payload.
    pub fn encode(&self) -> Result<Vec<u8>, ProtoError> {
        let mut out = Vec::new();
        match self {
            Self::Hello(h) => {
                out.push(TAG_HELLO);
                out.extend_from_slice(&h.protocol.to_le_bytes());
                out.extend_from_slice(&h.gameplay_fingerprint.to_le_bytes());
                put_string(&mut out, &h.build)?;
                put_string(&mut out, &h.driver)?;
            }
            Self::Accept => out.push(TAG_ACCEPT),
            Self::Reject { code, message } => {
                out.push(TAG_REJECT);
                out.push(code.to_u8());
                put_string(&mut out, message)?;
            }
            Self::Welcome { player_id } => {
                out.push(TAG_WELCOME);
                out.extend_from_slice(&player_id.to_le_bytes());
            }
            Self::Session(ad) => {
                out.push(TAG_SESSION);
                put_string(&mut out, &ad.summary)?;
                if ad.params.len() > MAX_SESSION_PARAMS {
                    return Err(ProtoError::OversizeSessionParams(ad.params.len()));
                }
                out.extend_from_slice(&(ad.params.len() as u16).to_le_bytes());
                out.extend_from_slice(&ad.params);
            }
            Self::Roster { players } => {
                out.push(TAG_ROSTER);
                if players.len() > MAX_PLAYERS as usize {
                    return Err(ProtoError::OversizeRoster(players.len() as u8));
                }
                out.push(players.len() as u8);
                for p in players {
                    out.extend_from_slice(&p.player_id.to_le_bytes());
                    out.push(p.ready as u8);
                    put_string(&mut out, &p.driver)?;
                    put_string(&mut out, &p.build)?;
                    match &p.pick {
                        Some(pick) => {
                            out.push(1);
                            put_string(&mut out, &pick.vehicle)?;
                            out.push(pick.paint);
                        }
                        None => out.push(0),
                    }
                }
            }
            Self::SetReady { ready } => {
                out.push(TAG_SET_READY);
                out.push(*ready as u8);
            }
            Self::SetVehicle(pick) => {
                out.push(TAG_SET_VEHICLE);
                put_string(&mut out, &pick.vehicle)?;
                out.push(pick.paint);
            }
            Self::VehicleRefused { reason } => {
                out.push(TAG_VEHICLE_REFUSED);
                put_string(&mut out, reason)?;
            }
            Self::Leave => out.push(TAG_LEAVE),
        }
        Ok(out)
    }

    /// Parse one frame payload. Strict: every byte must be consumed.
    pub fn decode(payload: &[u8]) -> Result<Self, ProtoError> {
        let mut cur = Cursor::new(payload);
        let msg = match cur.u8()? {
            TAG_HELLO => Self::Hello(Hello {
                protocol: cur.u16()?,
                gameplay_fingerprint: cur.u64()?,
                build: cur.string()?,
                driver: cur.string()?,
            }),
            TAG_ACCEPT => Self::Accept,
            TAG_REJECT => Self::Reject {
                code: RejectCode::from_u8(cur.u8()?)?,
                message: cur.string()?,
            },
            TAG_WELCOME => Self::Welcome {
                player_id: cur.u16()?,
            },
            TAG_SESSION => {
                let summary = cur.string()?;
                let len = cur.u16()? as usize;
                if len > MAX_SESSION_PARAMS {
                    return Err(ProtoError::OversizeSessionParams(len));
                }
                Self::Session(SessionAdvertisement {
                    summary,
                    params: cur.take(len)?.to_vec(),
                })
            }
            TAG_ROSTER => {
                let count = cur.u8()?;
                if count > MAX_PLAYERS {
                    return Err(ProtoError::OversizeRoster(count));
                }
                let mut players = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    players.push(RosterEntry {
                        player_id: cur.u16()?,
                        ready: cur.bool()?,
                        driver: cur.string()?,
                        build: cur.string()?,
                        pick: if cur.bool()? {
                            Some(VehiclePick {
                                vehicle: cur.string()?,
                                paint: cur.u8()?,
                            })
                        } else {
                            None
                        },
                    });
                }
                Self::Roster { players }
            }
            TAG_SET_READY => Self::SetReady { ready: cur.bool()? },
            TAG_SET_VEHICLE => Self::SetVehicle(VehiclePick {
                vehicle: cur.string()?,
                paint: cur.u8()?,
            }),
            TAG_VEHICLE_REFUSED => Self::VehicleRefused {
                reason: cur.string()?,
            },
            TAG_LEAVE => Self::Leave,
            tag => return Err(ProtoError::BadTag(tag)),
        };
        cur.finish()?;
        Ok(msg)
    }
}

/// The host-side compatibility gate: whether a `Hello` is allowed in.
/// `Ok(())` accepts; `Err((code, message))` is the refusal to send back.
/// Pure so the decision is testable without a socket.
pub fn admit(hello: &Hello, gameplay_fingerprint: u64) -> Result<(), (RejectCode, String)> {
    if hello.protocol != PROTOCOL_VERSION {
        return Err((
            RejectCode::VersionMismatch,
            format!(
                "protocol {PROTOCOL_VERSION} required, client offered {}",
                hello.protocol
            ),
        ));
    }
    if hello.gameplay_fingerprint != gameplay_fingerprint {
        return Err((
            RejectCode::ContentMismatch,
            "gameplay content differs (tuning/bounds/geometry/events)".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> Hello {
        Hello {
            protocol: PROTOCOL_VERSION,
            gameplay_fingerprint: 0xdead_beef,
            build: "test".to_string(),
            driver: "driver one".to_string(),
        }
    }

    #[test]
    fn roundtrip_all_variants() {
        for msg in [
            Message::Hello(hello()),
            Message::Accept,
            Message::Reject {
                code: RejectCode::ContentMismatch,
                message: "content differs".to_string(),
            },
            Message::Welcome { player_id: 3 },
            Message::Session(SessionAdvertisement {
                summary: "sf, cruise, amateur".to_string(),
                params: vec![1, 2, 3, 4],
            }),
            Message::Session(SessionAdvertisement {
                summary: String::new(),
                params: Vec::new(),
            }),
            Message::Roster {
                players: vec![
                    RosterEntry {
                        player_id: 1,
                        driver: "driver one".to_string(),
                        build: "test".to_string(),
                        ready: false,
                        pick: Some(VehiclePick {
                            vehicle: "vpbug".to_string(),
                            paint: 2,
                        }),
                    },
                    RosterEntry {
                        player_id: 2,
                        driver: "driver two".to_string(),
                        build: "test".to_string(),
                        ready: true,
                        pick: None,
                    },
                ],
            },
            Message::SetReady { ready: true },
            Message::SetVehicle(VehiclePick {
                vehicle: "vpbug".to_string(),
                paint: 0,
            }),
            Message::SetVehicle(VehiclePick {
                vehicle: String::new(),
                paint: 0,
            }),
            Message::VehicleRefused {
                reason: "unknown vehicle id".to_string(),
            },
            Message::Leave,
        ] {
            let bytes = msg.encode().unwrap();
            assert_eq!(Message::decode(&bytes).unwrap(), msg);
        }
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(matches!(
            Message::decode(&[0xff]),
            Err(ProtoError::BadTag(0xff))
        ));
        // Truncated hello: tag + partial protocol field.
        assert!(matches!(
            Message::decode(&[TAG_HELLO, 0x01]),
            Err(ProtoError::Truncated)
        ));
        // String length beyond the cap.
        let mut bad = vec![TAG_HELLO];
        bad.extend_from_slice(&1u16.to_le_bytes());
        bad.extend_from_slice(&0u64.to_le_bytes());
        bad.extend_from_slice(&1000u16.to_le_bytes());
        assert!(matches!(
            Message::decode(&bad),
            Err(ProtoError::OversizeString)
        ));
        // Trailing bytes are a violation.
        let mut extra = Message::Accept.encode().unwrap();
        extra.push(0);
        assert!(matches!(
            Message::decode(&extra),
            Err(ProtoError::Trailing(1))
        ));
        // Unknown reject code.
        assert!(matches!(
            Message::decode(&[TAG_REJECT, 99]),
            Err(ProtoError::BadRejectCode(99))
        ));
        // A roster declaring more than the player ceiling.
        assert!(matches!(
            Message::decode(&[TAG_ROSTER, MAX_PLAYERS + 1]),
            Err(ProtoError::OversizeRoster(9))
        ));
        // Session params declaring more than the bound.
        let mut bad_session = vec![TAG_SESSION];
        bad_session.extend_from_slice(&1u16.to_le_bytes()); // summary len
        bad_session.push(b's');
        bad_session.extend_from_slice(&(MAX_SESSION_PARAMS as u16 + 1).to_le_bytes());
        assert!(matches!(
            Message::decode(&bad_session),
            Err(ProtoError::OversizeSessionParams(4097))
        ));
        // A bool field byte other than 0/1.
        assert!(matches!(
            Message::decode(&[TAG_SET_READY, 2]),
            Err(ProtoError::InvalidBool(2))
        ));
    }

    #[test]
    fn an_oversize_roster_does_not_encode() {
        let players = vec![
            RosterEntry {
                player_id: 0,
                driver: "d".to_string(),
                build: "b".to_string(),
                ready: false,
                pick: None,
            };
            MAX_PLAYERS as usize + 1
        ];
        assert!(matches!(
            Message::Roster { players }.encode(),
            Err(ProtoError::OversizeRoster(9))
        ));
    }

    #[test]
    fn an_oversize_session_does_not_encode() {
        let msg = Message::Session(SessionAdvertisement {
            summary: "s".to_string(),
            params: vec![0; MAX_SESSION_PARAMS + 1],
        });
        assert!(matches!(
            msg.encode(),
            Err(ProtoError::OversizeSessionParams(4097))
        ));
    }

    #[test]
    fn admit_gates_version_and_content() {
        assert_eq!(admit(&hello(), 0xdead_beef), Ok(()));
        let mut wrong_version = hello();
        wrong_version.protocol = PROTOCOL_VERSION + 1;
        assert!(matches!(
            admit(&wrong_version, 0xdead_beef),
            Err((RejectCode::VersionMismatch, _))
        ));
        let mut wrong_content = hello();
        wrong_content.gameplay_fingerprint += 1;
        assert!(matches!(
            admit(&wrong_content, 0xdead_beef),
            Err((RejectCode::ContentMismatch, _))
        ));
    }
}
