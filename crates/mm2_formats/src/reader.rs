//! Minimal bounds-checked binary reader used by the handwritten parsers.

use crate::FormatError;

/// A cursor over a byte slice that returns [`FormatError`] instead of panicking.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Create a reader over `data`.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Create a reader positioned at `pos`.
    pub fn at(data: &'a [u8], pos: usize) -> Result<Self, FormatError> {
        if pos > data.len() {
            return Err(FormatError::InvalidValue {
                offset: pos,
                field: "offset",
                value: pos as u64,
                reason: "offset past end of data",
            });
        }
        Ok(Self { data, pos })
    }

    /// Current absolute offset into the input.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Total length of the underlying input.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether the reader is empty at the current position.
    pub fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// Bytes remaining from the current position.
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// Seek to an absolute offset.
    pub fn seek(&mut self, pos: usize) -> Result<(), FormatError> {
        if pos > self.data.len() {
            return Err(FormatError::InvalidValue {
                offset: pos,
                field: "seek",
                value: pos as u64,
                reason: "seek past end of data",
            });
        }
        self.pos = pos;
        Ok(())
    }

    /// Skip `n` bytes forward.
    pub fn skip(&mut self, n: usize) -> Result<(), FormatError> {
        let target = self.pos.checked_add(n).ok_or(FormatError::InvalidValue {
            offset: self.pos,
            field: "skip",
            value: n as u64,
            reason: "skip overflows offset",
        })?;
        self.seek(target)
    }

    /// The remaining input without consuming it.
    pub fn rest(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }

    /// Read `n` raw bytes.
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], FormatError> {
        if self.remaining() < n {
            return Err(FormatError::UnexpectedEof {
                offset: self.pos,
                needed: n,
                remaining: self.remaining(),
            });
        }
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    /// Read a `u8`.
    pub fn u8(&mut self) -> Result<u8, FormatError> {
        Ok(self.bytes(1)?[0])
    }

    /// Read a little-endian `u16`.
    pub fn u16(&mut self) -> Result<u16, FormatError> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    /// Read a little-endian `u32`.
    pub fn u32(&mut self) -> Result<u32, FormatError> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Read a little-endian `i32`.
    pub fn i32(&mut self) -> Result<i32, FormatError> {
        Ok(self.u32()? as i32)
    }

    /// Read a little-endian `f32`.
    pub fn f32(&mut self) -> Result<f32, FormatError> {
        Ok(f32::from_bits(self.u32()?))
    }

    /// Read a `[f32; 3]` vector.
    pub fn vec3(&mut self) -> Result<[f32; 3], FormatError> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }

    /// Read a `[f32; 2]` vector.
    pub fn vec2(&mut self) -> Result<[f32; 2], FormatError> {
        Ok([self.f32()?, self.f32()?])
    }

    /// Read a length-prefixed, NUL-terminated ASCII string as used in PKG and
    /// PSDL files: a `u8` length that *includes* the terminator, then the bytes.
    pub fn lp_string(&mut self) -> Result<String, FormatError> {
        let offset = self.pos;
        let len = self.u8()? as usize;
        if len == 0 {
            return Ok(String::new());
        }
        let raw = self.bytes(len)?;
        if raw[len - 1] != 0 {
            return Err(FormatError::InvalidString {
                offset,
                reason: "length-prefixed string is not NUL terminated",
            });
        }
        String::from_utf8(raw[..len - 1].to_vec()).map_err(|_| FormatError::InvalidString {
            offset,
            reason: "string is not valid UTF-8/ASCII",
        })
    }

    /// Read a NUL-terminated ASCII string of at most `max` bytes.
    pub fn cstring(&mut self, max: usize) -> Result<String, FormatError> {
        let offset = self.pos;
        let window = self.bytes(max.min(self.remaining()))?;
        let end = window
            .iter()
            .position(|&b| b == 0)
            .ok_or(FormatError::InvalidString {
                offset,
                reason: "unterminated string",
            })?;
        let s = String::from_utf8(window[..end].to_vec()).map_err(|_| {
            FormatError::InvalidString {
                offset,
                reason: "string is not valid UTF-8/ASCII",
            }
        })?;
        self.pos = offset + end + 1;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_primitives() {
        let data = [1u8, 0x02, 0x03, 0x34, 0x12, 0, 0, 0x80, 0x3f];
        let mut r = Reader::new(&data);
        assert_eq!(r.u8().unwrap(), 1);
        assert_eq!(r.u16().unwrap(), 0x0302);
        assert_eq!(r.u16().unwrap(), 0x1234);
        assert_eq!(r.f32().unwrap(), 1.0);
    }

    #[test]
    fn eof_is_an_error() {
        let data = [1u8, 2];
        let mut r = Reader::new(&data);
        r.u8().unwrap();
        assert!(matches!(
            r.u32(),
            Err(FormatError::UnexpectedEof { .. })
        ));
    }

    #[test]
    fn lp_string_requires_nul() {
        // length 4, "abc\0"
        let ok = [4u8, b'a', b'b', b'c', 0];
        assert_eq!(Reader::new(&ok).lp_string().unwrap(), "abc");

        let bad = [4u8, b'a', b'b', b'c', b'd'];
        assert!(matches!(
            Reader::new(&bad).lp_string(),
            Err(FormatError::InvalidString { .. })
        ));
    }
}
