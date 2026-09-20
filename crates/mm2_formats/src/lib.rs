//! Pure-Rust parsers for Midtown Madness 2 file formats.
//!
//! This crate has no dependency on Bevy or any game engine. All parsers
//! operate on byte slices and produce strongly typed representations.
//! Malformed input produces structured [`FormatError`] values rather than
//! panicking.

pub mod bai;
pub mod bnd;
pub mod crashdata;
pub mod dave;
pub mod info;
pub mod inst;
pub mod mtx;
pub mod opp;
pub mod pkg;
pub mod psdl;
pub mod racedata;
pub mod racefiles;
pub mod reader;
pub mod rewards;
pub mod tex;
pub mod tune;
pub mod veh;
pub mod waypoints;

pub use reader::Reader;

use std::ops::Range;

/// Error type produced by all parsers in this crate.
#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    /// The input ended before a complete structure could be read.
    #[error(
        "unexpected end of data at offset {offset:#x}, needed {needed} bytes, have {remaining}"
    )]
    UnexpectedEof {
        /// Byte offset where the read started.
        offset: usize,
        /// Number of bytes requested.
        needed: usize,
        /// Number of bytes remaining in the input.
        remaining: usize,
    },

    /// A magic identifier did not match the expected value.
    #[error("bad magic at offset {offset:#x}: expected {expected}, found {found:?}")]
    BadMagic {
        /// Offset of the magic value.
        offset: usize,
        /// Expected magic as text.
        expected: &'static str,
        /// Found bytes.
        found: Vec<u8>,
    },

    /// A count, offset or size field points outside the input or is implausible.
    #[error("invalid {field} value {value:#x} at offset {offset:#x}: {reason}")]
    InvalidValue {
        /// Offset of the offending field.
        offset: usize,
        /// Name of the field.
        field: &'static str,
        /// Raw value of the field.
        value: u64,
        /// Why the value is invalid.
        reason: &'static str,
    },

    /// A string field was not valid for its expected encoding.
    #[error("invalid string at offset {offset:#x}: {reason}")]
    InvalidString {
        /// Offset of the string.
        offset: usize,
        /// Why the string is invalid.
        reason: &'static str,
    },

    /// An entry's compressed data could not be inflated.
    #[error("failed to decompress data: {0}")]
    Decompression(String),

    /// Trailing or internal data could not be interpreted.
    #[error("parse error at offset {offset:#x}: {reason}")]
    Parse {
        /// Offset where the problem was found.
        offset: usize,
        /// Description of the problem.
        reason: String,
    },
}

impl FormatError {
    /// Convenience constructor for [`FormatError::Parse`].
    pub fn parse(offset: usize, reason: impl Into<String>) -> Self {
        Self::Parse {
            offset,
            reason: reason.into(),
        }
    }
}

/// Helper for bounds-checking a byte range against an input length.
pub(crate) fn check_range(len: usize, range: Range<usize>) -> Result<(), FormatError> {
    if range.end > len || range.start > range.end {
        return Err(FormatError::InvalidValue {
            offset: range.start,
            field: "range",
            value: range.end as u64,
            reason: "range extends past end of data",
        });
    }
    Ok(())
}
