//! Length-prefixed framing over a byte stream.
//!
//! Wire format: `u32le` payload length followed by that many payload
//! bytes. The length is checked against [`MAX_FRAME`] *before* the
//! buffer is allocated, so a hostile or corrupt peer cannot make us
//! reserve memory it never sends (F24-AC03).

use std::io::{Read, Write};

use crate::NetError;

/// Largest payload one frame may carry. The control plane (handshake,
/// lobby) never approaches this; it exists so a corrupt length word
/// fails fast instead of allocating gigabytes.
pub const MAX_FRAME: u32 = 256 * 1024;

/// Write one frame: `u32le` length then `payload`.
pub fn write_frame(w: &mut impl Write, payload: &[u8]) -> Result<(), NetError> {
    let len = u32::try_from(payload.len()).map_err(|_| NetError::Oversize {
        len: u32::MAX,
        max: MAX_FRAME,
    })?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()?;
    Ok(())
}

/// Read one frame, bounded by [`MAX_FRAME`]. A clean peer EOF while
/// waiting for the length word is `UnexpectedEof`, like any truncation.
pub fn read_frame(r: &mut impl Read) -> Result<Vec<u8>, NetError> {
    let mut len_bytes = [0u8; 4];
    r.read_exact(&mut len_bytes)?;
    let len = u32::from_le_bytes(len_bytes);
    if len > MAX_FRAME {
        return Err(NetError::Oversize {
            len,
            max: MAX_FRAME,
        });
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload)?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"hello").unwrap();
        write_frame(&mut buf, &[]).unwrap();
        let mut cur = Cursor::new(buf);
        assert_eq!(read_frame(&mut cur).unwrap(), b"hello");
        assert_eq!(read_frame(&mut cur).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn oversize_length_is_rejected_before_allocation() {
        let mut cur = Cursor::new((MAX_FRAME + 1).to_le_bytes());
        let err = read_frame(&mut cur).unwrap_err();
        assert!(matches!(
            err,
            NetError::Oversize { len, .. } if len == MAX_FRAME + 1
        ));
    }

    #[test]
    fn truncated_payload_is_eof() {
        let mut buf = (10u32).to_le_bytes().to_vec();
        buf.extend_from_slice(b"abc");
        let mut cur = Cursor::new(buf);
        let err = read_frame(&mut cur).unwrap_err();
        assert!(matches!(
            err,
            NetError::Io(e) if e.kind() == std::io::ErrorKind::UnexpectedEof
        ));
    }

    #[test]
    fn empty_stream_is_eof() {
        let mut cur = Cursor::new(Vec::new());
        let err = read_frame(&mut cur).unwrap_err();
        assert!(matches!(
            err,
            NetError::Io(e) if e.kind() == std::io::ErrorKind::UnexpectedEof
        ));
    }
}
