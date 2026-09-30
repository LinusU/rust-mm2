//! The wire protocol: message types, encoding and the compatibility
//! gate the handshake applies.
//!
//! All integers are little-endian. Strings are `u16le`-length-prefixed
//! UTF-8 bounded by [`MAX_STRING`]. Decoding is strict: unknown type
//! bytes, truncated fields, over-long strings and trailing bytes are
//! all errors — a peer speaking something we cannot name is a protocol
//! violation, not a parse to fudge.

/// Wire version. Bumped for any incompatible message change; peers must
/// match exactly.
pub const PROTOCOL_VERSION: u16 = 1;

/// Byte cap on any length-prefixed string field.
pub const MAX_STRING: usize = 256;

const TAG_HELLO: u8 = 0x01;
const TAG_ACCEPT: u8 = 0x02;
const TAG_REJECT: u8 = 0x03;

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
