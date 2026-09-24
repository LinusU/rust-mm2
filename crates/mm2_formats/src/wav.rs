//! Parser for RIFF/WAVE audio (`aud/audNN/**.wav`, F07-A).
//!
//! Retail waves are uncompressed 16-bit PCM (mono 11 025 or 22 050 Hz on
//! the measured install), but the decoder validates the full RIFF/WAVE
//! envelope rather than assuming that shape: the RIFF size word, the
//! chunk table (with even-byte padding), the `fmt ` record and the
//! `data` payload bounds. Non-PCM format tags and authored envelope
//! anomalies are reported as issues, not silently accepted.
//!
//! The DirectMusic containers in `aud/dmusic` are also RIFF files
//! (`DMSG` segments, `DMST` styles, `DLS ` sound banks, `DMBD` bands);
//! [`riff_form_type`] exposes their form word so callers can classify
//! them without claiming to decode them.

use crate::{FormatError, Reader};

/// RIFF chunk fourcc for the PCM format record.
pub const FMT_ID: [u8; 4] = *b"fmt ";
/// RIFF chunk fourcc for the sample payload.
pub const DATA_ID: [u8; 4] = *b"data";
/// WAVE format tag for uncompressed PCM.
pub const FORMAT_PCM: u16 = 1;
/// Upper bound on parsed chunks — a larger table means the file is not a
/// sane RIFF container.
const MAX_CHUNKS: usize = 4096;

/// The RIFF container form type (the four bytes after the RIFF size
/// word: `WAVE`, `DMSG`, `DLS `, …), or `None` when `data` is not a RIFF
/// container at all.
pub fn riff_form_type(data: &[u8]) -> Option<[u8; 4]> {
    if data.len() < 12 || data[0..4] != *b"RIFF" {
        return None;
    }
    Some(data[8..12].try_into().unwrap())
}

/// The `fmt ` chunk: how the payload bytes decode.
#[derive(Debug, Clone)]
pub struct WaveFormat {
    /// WAVE format tag — `1` is uncompressed PCM; anything else is a
    /// compression scheme this decoder does not interpret.
    pub tag: u16,
    /// Interleaved channel count.
    pub channels: u16,
    /// Frames per second.
    pub sample_rate: u32,
    /// Authored average bytes per second (should equal
    /// `sample_rate * block_align` for PCM).
    pub byte_rate: u32,
    /// Authored bytes per frame (should equal
    /// `channels * bits_per_sample / 8` for PCM).
    pub block_align: u16,
    /// Bits per sample for PCM tags.
    pub bits_per_sample: u16,
    /// Bytes past the fixed 16-byte fmt record (cbSize + extra format
    /// data on compressed tags) — preserved verbatim.
    pub extra: Vec<u8>,
}

/// One chunk in the RIFF table, in file order.
#[derive(Debug, Clone)]
pub struct WavChunk {
    /// Chunk fourcc.
    pub id: [u8; 4],
    /// Absolute offset of the payload.
    pub offset: usize,
    /// Declared payload length.
    pub len: usize,
}

/// A parsed RIFF/WAVE file.
#[derive(Debug)]
pub struct Wav<'a> {
    /// The RIFF size word verbatim — `file_len - 8` on well-formed data.
    pub riff_size: u32,
    /// The `fmt ` record.
    pub fmt: WaveFormat,
    /// The `data` chunk payload (empty when absent or zero-length).
    pub pcm: &'a [u8],
    /// Every chunk encountered, in order — `LIST`, `cue `, `JUNK` and
    /// unknown ids are preserved for auditing.
    pub chunks: Vec<WavChunk>,
    /// Bytes after the last chunk that could not form a chunk header.
    pub trailing: usize,
    file_len: usize,
}

/// Semantic findings on a parsed [`Wav`] — authored anomalies and
/// unsupported (but structurally valid) encodings.
#[derive(Debug, Clone, PartialEq)]
pub enum WavIssue {
    /// The RIFF size word disagrees with the real file length
    /// (`declared != file_len - 8`).
    RiffSizeMismatch {
        /// Authored size word.
        declared: u32,
        /// Bytes actually present past the RIFF header.
        actual: usize,
    },
    /// The format tag is not uncompressed PCM — parsed envelope only;
    /// payload decoding is unsupported.
    NonPcmFormat {
        /// The authored format tag.
        tag: u16,
    },
    /// A second `fmt ` or `data` chunk exists; only the first binds.
    DuplicateChunk {
        /// The duplicated fourcc.
        id: String,
    },
    /// `data` precedes `fmt ` — players read fmt first.
    DataBeforeFmt,
    /// `data` is absent — no payload to play.
    MissingData,
    /// `block_align` disagrees with `channels * bits_per_sample / 8`.
    BlockAlignMismatch {
        /// Authored value.
        declared: u16,
        /// Recomputed value.
        expected: u16,
    },
    /// `byte_rate` disagrees with `sample_rate * block_align`.
    ByteRateMismatch {
        /// Authored value.
        declared: u32,
        /// Recomputed value.
        expected: u32,
    },
    /// A zero sample rate or channel count — duration is meaningless.
    DegenerateFormat {
        /// Which field is zero (`sample_rate`/`channels`/`bits_per_sample`).
        field: &'static str,
    },
    /// The PCM payload is not a whole number of frames.
    PartialFrame {
        /// Payload bytes.
        data_len: usize,
        /// Authored bytes per frame.
        block_align: u16,
    },
    /// Bytes exist after the last chunk of the RIFF table.
    TrailingBytes {
        /// Unaccounted byte count.
        len: usize,
    },
}

impl<'a> Wav<'a> {
    /// Parse a RIFF/WAVE file. Errors are structural (truncated headers,
    /// chunks claiming bytes past EOF, no `fmt ` record); content-level
    /// anomalies surface through [`Wav::validate`].
    pub fn parse(data: &'a [u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(data);
        let magic = r.bytes(4)?;
        if magic != b"RIFF" {
            return Err(FormatError::BadMagic {
                offset: 0,
                expected: "RIFF",
                found: magic.to_vec(),
            });
        }
        let riff_size = r.u32()?;
        let form = r.bytes(4)?;
        if form != b"WAVE" {
            return Err(FormatError::BadMagic {
                offset: 8,
                expected: "WAVE",
                found: form.to_vec(),
            });
        }

        let mut chunks = Vec::new();
        let mut fmt: Option<WaveFormat> = None;
        let mut pcm: Option<&[u8]> = None;
        loop {
            if chunks.len() >= MAX_CHUNKS {
                return Err(FormatError::InvalidValue {
                    offset: r.pos(),
                    field: "chunk_count",
                    value: chunks.len() as u64,
                    reason: "chunk table exceeds sanity bound",
                });
            }
            if r.remaining() < 8 {
                break;
            }
            let hdr_off = r.pos();
            let id: [u8; 4] = r.bytes(4)?.try_into().unwrap();
            let len = r.u32()? as usize;
            let offset = r.pos();
            if len > r.remaining() {
                return Err(FormatError::InvalidValue {
                    offset: hdr_off,
                    field: "chunk_size",
                    value: len as u64,
                    reason: "chunk payload extends past end of file",
                });
            }
            let payload = r.bytes(len)?;
            chunks.push(WavChunk { id, offset, len });
            match id {
                FMT_ID if fmt.is_none() => {
                    fmt = Some(parse_fmt(payload, offset)?);
                }
                DATA_ID if pcm.is_none() => {
                    pcm = Some(payload);
                }
                _ => {}
            }
            // Chunks are padded to an even byte count.
            if len % 2 == 1 {
                r.skip(1)?;
            }
        }

        let fmt = fmt.ok_or(FormatError::InvalidValue {
            offset: 12,
            field: "fmt",
            value: 0,
            reason: "WAVE file has no fmt chunk",
        })?;
        Ok(Wav {
            riff_size,
            fmt,
            pcm: pcm.unwrap_or(&[]),
            chunks,
            trailing: r.remaining(),
            file_len: data.len(),
        })
    }

    /// Payload duration in seconds using the authored byte rate (0 when
    /// the rate is degenerate or the payload absent).
    pub fn duration_secs(&self) -> f64 {
        if self.fmt.byte_rate == 0 {
            return 0.0;
        }
        self.pcm.len() as f64 / f64::from(self.fmt.byte_rate)
    }

    /// PCM frame count (`None` for non-PCM or degenerate formats).
    pub fn frames(&self) -> Option<usize> {
        if self.fmt.tag != FORMAT_PCM || self.fmt.block_align == 0 {
            return None;
        }
        Some(self.pcm.len() / usize::from(self.fmt.block_align))
    }

    /// Whole PCM frames as interleaved samples (`None` unless the file
    /// is 16-bit PCM).
    pub fn samples_i16(&self) -> Option<Vec<i16>> {
        if self.fmt.tag != FORMAT_PCM || self.fmt.bits_per_sample != 16 {
            return None;
        }
        Some(
            self.pcm
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b))
                .collect(),
        )
    }

    /// Sanity issues worth reporting: envelope inconsistencies and
    /// encodings this decoder does not interpret. Structural problems
    /// (truncation, runaway chunk sizes, missing `fmt `) fail at parse
    /// instead.
    pub fn validate(&self) -> Vec<WavIssue> {
        let mut issues = Vec::new();
        let actual = self.file_len.saturating_sub(8);
        if usize::try_from(self.riff_size).ok() != Some(actual) {
            issues.push(WavIssue::RiffSizeMismatch {
                declared: self.riff_size,
                actual,
            });
        }
        if self.fmt.tag != FORMAT_PCM {
            issues.push(WavIssue::NonPcmFormat { tag: self.fmt.tag });
        }
        for (i, c) in self.chunks.iter().enumerate() {
            if (c.id == FMT_ID || c.id == DATA_ID) && self.chunks[..i].iter().any(|p| p.id == c.id)
            {
                issues.push(WavIssue::DuplicateChunk {
                    id: String::from_utf8_lossy(&c.id).into_owned(),
                });
            }
        }
        let fmt_at = self.chunks.iter().position(|c| c.id == FMT_ID);
        match self.chunks.iter().position(|c| c.id == DATA_ID) {
            Some(d) if fmt_at.is_some_and(|f| d < f) => issues.push(WavIssue::DataBeforeFmt),
            None => issues.push(WavIssue::MissingData),
            _ => {}
        }
        if self.fmt.sample_rate == 0 {
            issues.push(WavIssue::DegenerateFormat {
                field: "sample_rate",
            });
        }
        if self.fmt.channels == 0 {
            issues.push(WavIssue::DegenerateFormat { field: "channels" });
        }
        if self.fmt.tag == FORMAT_PCM && self.fmt.bits_per_sample == 0 {
            issues.push(WavIssue::DegenerateFormat {
                field: "bits_per_sample",
            });
        }
        if self.fmt.channels > 0 && self.fmt.bits_per_sample > 0 {
            let expected = self.fmt.channels * self.fmt.bits_per_sample.div_ceil(8);
            if self.fmt.block_align != expected {
                issues.push(WavIssue::BlockAlignMismatch {
                    declared: self.fmt.block_align,
                    expected,
                });
            }
        }
        if self.fmt.sample_rate > 0 && self.fmt.block_align > 0 {
            let expected = self.fmt.sample_rate * u32::from(self.fmt.block_align);
            if self.fmt.byte_rate != expected {
                issues.push(WavIssue::ByteRateMismatch {
                    declared: self.fmt.byte_rate,
                    expected,
                });
            }
        }
        if self.fmt.block_align > 0
            && !self.pcm.is_empty()
            && !self
                .pcm
                .len()
                .is_multiple_of(usize::from(self.fmt.block_align))
        {
            issues.push(WavIssue::PartialFrame {
                data_len: self.pcm.len(),
                block_align: self.fmt.block_align,
            });
        }
        if self.trailing > 0 {
            issues.push(WavIssue::TrailingBytes { len: self.trailing });
        }
        issues
    }
}

/// Parse the fixed 16-byte fmt record plus any extended tail.
fn parse_fmt(payload: &[u8], offset: usize) -> Result<WaveFormat, FormatError> {
    if payload.len() < 16 {
        return Err(FormatError::InvalidValue {
            offset,
            field: "fmt",
            value: payload.len() as u64,
            reason: "fmt chunk shorter than the fixed 16-byte record",
        });
    }
    let mut r = Reader::new(payload);
    let tag = r.u16()?;
    let channels = r.u16()?;
    let sample_rate = r.u32()?;
    let byte_rate = r.u32()?;
    let block_align = r.u16()?;
    let bits_per_sample = r.u16()?;
    let extra = r.rest().to_vec();
    Ok(WaveFormat {
        tag,
        channels,
        sample_rate,
        byte_rate,
        block_align,
        bits_per_sample,
        extra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal RIFF/WAVE: chunks in order, padded to even.
    fn wave(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut body = Vec::from(&b"WAVE"[..]);
        for (id, payload) in chunks {
            body.extend_from_slice(&id[..]);
            body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            body.extend_from_slice(payload);
            if payload.len() % 2 == 1 {
                body.push(0);
            }
        }
        let mut out = Vec::from(&b"RIFF"[..]);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    fn pcm_fmt(rate: u32, channels: u16, bits: u16) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&channels.to_le_bytes());
        f.extend_from_slice(&rate.to_le_bytes());
        let align = channels * bits.div_ceil(8);
        f.extend_from_slice(&(rate * u32::from(align)).to_le_bytes());
        f.extend_from_slice(&align.to_le_bytes());
        f.extend_from_slice(&bits.to_le_bytes());
        f
    }

    #[test]
    fn parses_retail_shape_16bit_mono_22k() {
        // retail `aud/aud22/enginesedan1.wav` shape: fmt + data, PCM
        // 22050 Hz mono 16-bit.
        let pcm = [0x11u8; 2 * 2205]; // 0.1 s of frames
        let file = wave(&[(b"fmt ", &pcm_fmt(22050, 1, 16)), (b"data", &pcm)]);
        let w = Wav::parse(&file).unwrap();
        assert_eq!(w.fmt.tag, FORMAT_PCM);
        assert_eq!(w.fmt.channels, 1);
        assert_eq!(w.fmt.sample_rate, 22050);
        assert_eq!(w.fmt.byte_rate, 44100);
        assert_eq!(w.fmt.block_align, 2);
        assert_eq!(w.pcm.len(), pcm.len());
        assert_eq!(w.frames(), Some(2205));
        assert!((w.duration_secs() - 0.1).abs() < 1e-6);
        assert!(w.validate().is_empty());
        assert_eq!(w.samples_i16().unwrap()[0], 0x1111);
    }

    #[test]
    fn riff_form_type_reads_form_word() {
        let mut sgt = wave(&[]);
        // Rewrite the form word to a DirectMusic segment.
        sgt[8..12].copy_from_slice(b"DMSG");
        assert_eq!(riff_form_type(&sgt), Some(*b"DMSG"));
        assert_eq!(riff_form_type(b"not riff"), None);
        assert_eq!(riff_form_type(&[b'R'; 8]), None);
    }

    #[test]
    fn rejects_non_riff_and_non_wave() {
        assert!(matches!(
            Wav::parse(b"NOPE........"),
            Err(FormatError::BadMagic { .. })
        ));
        let mut form = wave(&[]);
        form[8..12].copy_from_slice(b"DMSG");
        assert!(matches!(
            Wav::parse(&form),
            Err(FormatError::BadMagic { offset: 8, .. })
        ));
    }

    #[test]
    fn truncated_chunk_is_an_error_not_a_wrap() {
        let mut file = wave(&[(b"fmt ", &pcm_fmt(22050, 1, 16)), (b"data", b"ab")]);
        file.truncate(file.len() - 1);
        assert!(Wav::parse(&file).is_err());
    }

    #[test]
    fn odd_sized_chunks_pad_to_even() {
        // `LIST` with a 3-byte payload: one pad byte follows it.
        let file = wave(&[
            (b"fmt ", &pcm_fmt(11025, 1, 16)),
            (b"LIST", b"abc"),
            (b"data", b"\x01\x00"),
        ]);
        let w = Wav::parse(&file).unwrap();
        assert_eq!(w.chunks.len(), 3);
        assert_eq!(w.chunks[1].id, *b"LIST");
        assert_eq!(w.chunks[1].len, 3);
        assert_eq!(w.pcm, &[0x01, 0x00]);
        assert!(w.validate().is_empty());
    }

    #[test]
    fn missing_fmt_is_structural() {
        let file = wave(&[(b"data", b"ab")]);
        assert!(Wav::parse(&file).is_err());
    }

    #[test]
    fn missing_or_duplicate_chunks_are_issues() {
        let fmt = pcm_fmt(22050, 1, 16);
        let dup = wave(&[
            (b"fmt ", &fmt),
            (b"data", b"ab"),
            (b"data", b"cd"),
            (b"fmt ", &fmt),
        ]);
        let w = Wav::parse(&dup).unwrap();
        let issues = w.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, WavIssue::DuplicateChunk { id } if id == "data"))
        );
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, WavIssue::DuplicateChunk { id } if id == "fmt "))
        );

        // A fmt-only file parses but reports the missing payload.
        let nodata = wave(&[(b"fmt ", &fmt)]);
        let w = Wav::parse(&nodata).unwrap();
        assert!(w.validate().contains(&WavIssue::MissingData));
        // data-before-fmt is a finding too.
        let swapped = wave(&[(b"data", b"ab"), (b"fmt ", &fmt)]);
        let w = Wav::parse(&swapped).unwrap();
        assert!(w.validate().contains(&WavIssue::DataBeforeFmt));
    }

    #[test]
    fn fmt_and_rate_inconsistencies_are_issues() {
        let mut fmt = pcm_fmt(22050, 1, 16);
        // Break block_align: claim 4 bytes/frame for mono 16-bit. The
        // authored byte_rate stays consistent with it, so only the
        // align and partial-frame findings fire.
        fmt[12..14].copy_from_slice(&4u16.to_le_bytes());
        fmt[8..12].copy_from_slice(&(22050u32 * 4).to_le_bytes());
        let file = wave(&[(b"fmt ", &fmt), (b"data", b"abcdx")]);
        let w = Wav::parse(&file).unwrap();
        let issues = w.validate();
        assert!(issues.contains(&WavIssue::BlockAlignMismatch {
            declared: 4,
            expected: 2
        }));
        assert!(issues.contains(&WavIssue::PartialFrame {
            data_len: 5,
            block_align: 4,
        }));
        assert!(
            !issues
                .iter()
                .any(|i| matches!(i, WavIssue::ByteRateMismatch { .. }))
        );
    }

    #[test]
    fn non_pcm_tag_is_reported_not_rejected() {
        let mut fmt = pcm_fmt(22050, 1, 4);
        fmt[0..2].copy_from_slice(&0x11u16.to_le_bytes()); // IMA ADPCM
        fmt.extend_from_slice(&2u16.to_le_bytes()); // cbSize
        fmt.extend_from_slice(&[0xf9, 0x01]); // samples per block
        let file = wave(&[(b"fmt ", &fmt), (b"data", &[0u8; 256])]);
        let w = Wav::parse(&file).unwrap();
        assert_eq!(w.fmt.tag, 0x11);
        assert_eq!(w.fmt.extra.len(), 4);
        assert!(w.validate().contains(&WavIssue::NonPcmFormat { tag: 0x11 }));
        assert!(w.samples_i16().is_none());
        assert!(w.frames().is_none());
    }

    #[test]
    fn riff_size_mismatch_is_a_finding() {
        let mut file = wave(&[(b"fmt ", &pcm_fmt(11025, 1, 16)), (b"data", b"ab")]);
        // Claim a larger RIFF payload than the file holds.
        file[4..8].copy_from_slice(&0xFFFFu32.to_le_bytes());
        let w = Wav::parse(&file).unwrap();
        assert!(w.validate().contains(&WavIssue::RiffSizeMismatch {
            declared: 0xFFFF,
            actual: file.len() - 8,
        }));
    }

    #[test]
    fn trailing_bytes_are_a_finding() {
        let mut file = wave(&[(b"fmt ", &pcm_fmt(11025, 1, 16)), (b"data", b"ab")]);
        // Keep the authored RIFF size honest, then append junk.
        let size = file.len() as u32 - 8;
        file[4..8].copy_from_slice(&size.to_le_bytes());
        file.extend_from_slice(b"junk!");
        let w = Wav::parse(&file).unwrap();
        assert_eq!(w.trailing, 5);
        let issues = w.validate();
        assert!(issues.contains(&WavIssue::TrailingBytes { len: 5 }));
        assert!(issues.contains(&WavIssue::RiffSizeMismatch {
            declared: size,
            actual: file.len() - 8,
        }));
    }
}
