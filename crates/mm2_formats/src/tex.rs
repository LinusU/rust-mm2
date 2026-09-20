//! Parser for the Angel Studios `TEX` texture format.
//!
//! Layout (all integers little-endian):
//!
//! ```text
//! u16 width, u16 height, u16 pixel_type, u16 mip_count, u16 unknown, u32 bits
//! [palette]  BGRA entries (256 for 8-bit types, 16 for 4-bit types)
//! [pixels]   level 0 followed by mip levels, each half the previous size
//! ```
//!
//! Provenance: structure per `angel-file-formats/General/TEX.md`; pixel type
//! numbers and flag meanings labelled there are carried over and flagged as
//! documented/observed in `docs/research/`.

use crate::{FormatError, Reader};

/// How pixel data is stored inside a TEX file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 256-colour palette image, 8-bit indices, palette alpha ignored.
    P8,
    /// 16-bit A1R5G5B5.
    A1R5G5B5,
    /// 8-bit greyscale.
    I8,
    /// 8-bit intensity + 8-bit alpha (Midnight Club 2 only).
    A8I8,
    /// 256-colour palette image, 8-bit indices, palette has alpha.
    Pa8,
    /// 16-colour palette image, 4-bit indices, palette alpha ignored.
    P4,
    /// 16-colour palette image, 4-bit indices, palette has alpha.
    Pa4,
    /// 24-bit RGB.
    Rgb888,
    /// 32-bit RGBA.
    Rgba8888,
    /// Anything else: preserved but not decodable.
    Unknown(u16),
}

impl PixelFormat {
    /// Map the raw `type` field to a known format.
    pub fn from_raw(raw: u16) -> Self {
        match raw {
            1 => Self::P8,
            6 => Self::A1R5G5B5,
            8 => Self::I8,
            10 => Self::A8I8,
            14 => Self::Pa8,
            15 => Self::P4,
            16 => Self::Pa4,
            17 => Self::Rgb888,
            18 => Self::Rgba8888,
            other => Self::Unknown(other),
        }
    }

    /// Raw type value as stored in the header.
    pub fn raw(self) -> u16 {
        match self {
            Self::P8 => 1,
            Self::A1R5G5B5 => 6,
            Self::I8 => 8,
            Self::A8I8 => 10,
            Self::Pa8 => 14,
            Self::P4 => 15,
            Self::Pa4 => 16,
            Self::Rgb888 => 17,
            Self::Rgba8888 => 18,
            Self::Unknown(v) => v,
        }
    }

    /// Bytes (or byte-equivalents for 4-bit) per pixel.
    fn bits_per_pixel(self) -> usize {
        match self {
            Self::P4 | Self::Pa4 => 4,
            Self::P8 | Self::Pa8 | Self::I8 => 8,
            Self::A1R5G5B5 | Self::A8I8 => 16,
            Self::Rgb888 => 24,
            Self::Rgba8888 => 32,
            Self::Unknown(_) => 0,
        }
    }

    /// Palette size in entries, if the format uses one.
    fn palette_entries(self) -> Option<usize> {
        match self {
            Self::P8 | Self::Pa8 => Some(256),
            Self::P4 | Self::Pa4 => Some(16),
            _ => None,
        }
    }
}

/// Parsed TEX header.
#[derive(Debug, Clone)]
pub struct TexHeader {
    /// Image width in pixels.
    pub width: u16,
    /// Image height in pixels.
    pub height: u16,
    /// Pixel storage format.
    pub format: PixelFormat,
    /// Number of mip levels stored (including the base level? see notes).
    pub mips: u16,
    /// Undocumented header field, preserved verbatim.
    pub unknown: u16,
    /// Texture flag bits (`ClampU` = 0x01, `ClampV` = 0x10000, ...).
    pub bits: u32,
}

/// A single mip level's raw pixel data.
#[derive(Debug, Clone)]
pub struct MipLevel {
    /// Width of this level.
    pub width: u32,
    /// Height of this level.
    pub height: u32,
    /// Encoded pixel data as stored in the file.
    pub data: Vec<u8>,
}

/// A parsed TEX texture.
#[derive(Debug, Clone)]
pub struct TexFile {
    /// File header.
    pub header: TexHeader,
    /// BGRA palette, present for palette-based formats.
    pub palette: Option<Vec<[u8; 4]>>,
    /// Mip levels in order, level 0 first.
    pub levels: Vec<MipLevel>,
}

impl TexFile {
    /// Parse a TEX file from bytes.
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(data);
        let width = r.u16()?;
        let height = r.u16()?;
        let format = PixelFormat::from_raw(r.u16()?);
        let mips = r.u16()?;
        let unknown = r.u16()?;
        let bits = r.u32()?;

        if width == 0 || height == 0 {
            return Err(FormatError::InvalidValue {
                offset: 0,
                field: "dimensions",
                value: ((width as u64) << 16) | height as u64,
                reason: "texture has a zero dimension",
            });
        }

        let header = TexHeader {
            width,
            height,
            format,
            mips,
            unknown,
            bits,
        };

        let palette = format
            .palette_entries()
            .map(|n| {
                let mut pal = Vec::with_capacity(n);
                for _ in 0..n {
                    // Palette order is blue, green, red, alpha.
                    let b = r.u8()?;
                    let g = r.u8()?;
                    let red = r.u8()?;
                    let a = r.u8()?;
                    pal.push([red, g, b, a]);
                }
                Ok::<_, FormatError>(pal)
            })
            .transpose()?;

        // MM2 textures store `mips` additional levels; treat the value as the
        // total level count including base (observed values are >= 1).
        let level_count = (mips.max(1)) as u32;
        let mut levels = Vec::with_capacity(level_count as usize);
        let (mut w, mut h) = (width as u32, height as u32);
        for level in 0..level_count {
            let pixel_count = (w.max(1) * h.max(1)) as usize;
            let byte_count = match format.bits_per_pixel() {
                4 => pixel_count.div_ceil(2),
                bpp => pixel_count * bpp / 8,
            };
            let level_offset = r.pos();
            if format.bits_per_pixel() == 0 {
                // Unknown format: cannot stride mip levels, keep the rest raw.
                let data = r.bytes(r.remaining())?.to_vec();
                levels.push(MipLevel {
                    width: w,
                    height: h,
                    data,
                });
                break;
            }
            let data = r.bytes(byte_count).map_err(|e| match e {
                FormatError::UnexpectedEof { .. } => FormatError::Parse {
                    offset: level_offset,
                    reason: format!("mip level {level} truncated"),
                },
                e => e,
            })?;
            levels.push(MipLevel {
                width: w,
                height: h,
                data: data.to_vec(),
            });
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }

        Ok(Self {
            header,
            palette,
            levels,
        })
    }

    /// Decode one mip level to 8-bit RGBA, top row first. Returns `None`
    /// for formats that cannot be decoded (unknown types).
    ///
    /// TEX stores its rows bottom-up, so they are reversed here: decoding
    /// in file order hands back an upside-down image, which is how retail
    /// San Francisco ended up with arched windows arching downwards and a
    /// shopfront whose `SHIRTS` sign read upside down.
    pub fn decode_rgba(&self, level: usize) -> Option<Vec<u8>> {
        self.decode_rgba_impl(level, false)
    }

    /// Like [`decode_rgba`](Self::decode_rgba) but the palette alpha byte
    /// is honored on every palette format, not only `Pa8`/`Pa4`.
    ///
    /// The P-type palette alpha is nominally ignored, but decal textures
    /// (`texture/decal_*`, `r4i_rails_f`) carry authored per-entry
    /// translucency — e.g. `decal_rxwalk03_l`'s unpainted surround sits
    /// at alpha ≈ 36 while its crosswalk bars carry ≈ 240 — which only
    /// makes sense if the decal renderer reads it (inferred; see
    /// `docs/research/pathset.md`). Ordinary surfaces must keep calling
    /// [`decode_rgba`](Self::decode_rgba).
    pub fn decode_rgba_honoring_alpha(&self, level: usize) -> Option<Vec<u8>> {
        self.decode_rgba_impl(level, true)
    }

    fn decode_rgba_impl(&self, level: usize, honor_palette_alpha: bool) -> Option<Vec<u8>> {
        let mip = self.levels.get(level)?;
        let count = (mip.width * mip.height) as usize;
        let mut out: Vec<u8> = Vec::with_capacity(count * 4);
        let fmt = self.header.format;
        match fmt {
            PixelFormat::P8 | PixelFormat::Pa8 | PixelFormat::P4 | PixelFormat::Pa4 => {
                let palette = self.palette.as_ref()?;
                let indices: Vec<u8> = if fmt.bits_per_pixel() == 4 {
                    // Two pixels per byte; high nibble first (inferred).
                    mip.data
                        .iter()
                        .flat_map(|b| [b >> 4, b & 0x0f])
                        .take(count)
                        .collect()
                } else {
                    mip.data.clone()
                };
                let has_alpha =
                    honor_palette_alpha || matches!(fmt, PixelFormat::Pa8 | PixelFormat::Pa4);
                for idx in indices {
                    let entry = palette.get(idx as usize).copied().unwrap_or([0; 4]);
                    out.extend_from_slice(&[
                        entry[0],
                        entry[1],
                        entry[2],
                        if has_alpha { entry[3] } else { 0xff },
                    ]);
                }
            }
            PixelFormat::A1R5G5B5 => {
                for px in mip.data.chunks_exact(2) {
                    let v = u16::from_le_bytes([px[0], px[1]]);
                    let a = if v & 0x8000 != 0 { 0xff } else { 0 };
                    let r = expand5((v >> 10) & 0x1f);
                    let g = expand5((v >> 5) & 0x1f);
                    let b = expand5(v & 0x1f);
                    out.extend_from_slice(&[r, g, b, a]);
                }
            }
            PixelFormat::I8 => {
                for &v in &mip.data {
                    out.extend_from_slice(&[v, v, v, 0xff]);
                }
            }
            PixelFormat::A8I8 => {
                for px in mip.data.chunks_exact(2) {
                    out.extend_from_slice(&[px[1], px[1], px[1], px[0]]);
                }
            }
            PixelFormat::Rgb888 => {
                for px in mip.data.chunks_exact(3) {
                    out.extend_from_slice(&[px[0], px[1], px[2], 0xff]);
                }
            }
            PixelFormat::Rgba8888 => {
                out.extend_from_slice(&mip.data);
            }
            PixelFormat::Unknown(_) => return None,
        }
        // Bottom-up → top-down. A level whose payload did not decode to a
        // full rectangle is handed back untouched rather than sliced on a
        // stride it does not have.
        let stride = mip.width as usize * 4;
        if stride > 0 && out.len() == stride * mip.height as usize {
            let mut flipped = Vec::with_capacity(out.len());
            for row in out.chunks_exact(stride).rev() {
                flipped.extend_from_slice(row);
            }
            out = flipped;
        }
        Some(out)
    }
}

/// Expand a 5-bit channel to 8 bits.
fn expand5(v: u16) -> u8 {
    ((v << 3) | (v >> 2)) as u8
}

/// Strip an animated-texture frame suffix (`<stem>-NNNN`, exactly four
/// digits) from a texture stem. MM2 stores animated surfaces (water:
/// `s_thames`, `s_pond`, `s_ocean`) as numbered frames `s_thames-0001` …
/// `s_thames-0030` with no plain `s_thames` file, and the PSDL texture
/// table references single frames (`s_thames-0009`). Consumers looking
/// up the surface *stem* — `materials.csv` keys the base name — call
/// this when the full name misses. Returns `None` when the tail is not
/// a four-digit frame number.
pub fn frame_base_stem(name: &str) -> Option<&str> {
    let (base, frame) = name.rsplit_once('-')?;
    (frame.len() == 4 && frame.bytes().all(|b| b.is_ascii_digit()) && !base.is_empty())
        .then_some(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(w: u16, h: u16, ty: u16, mips: u16) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&w.to_le_bytes());
        d.extend_from_slice(&h.to_le_bytes());
        d.extend_from_slice(&ty.to_le_bytes());
        d.extend_from_slice(&mips.to_le_bytes());
        d.extend_from_slice(&0u16.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d
    }

    #[test]
    fn parses_rgba8888() {
        let mut d = header(2, 2, 18, 1);
        d.extend_from_slice(&[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 1, 2, 3, 4]);
        let tex = TexFile::parse(&d).unwrap();
        assert_eq!(tex.header.width, 2);
        assert_eq!(tex.levels.len(), 1);
        // Rows come back top-down, so the file's *last* row leads.
        let rgba = tex.decode_rgba(0).unwrap();
        assert_eq!(&rgba[..8], &[0, 0, 255, 255, 1, 2, 3, 4]);
        assert_eq!(&rgba[8..], &[255, 0, 0, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn parses_p8_with_palette() {
        let mut d = header(2, 2, 1, 1);
        // palette: entry 0 = black, entry 1 = red (BGRA order in file)
        for i in 0..256usize {
            if i == 1 {
                d.extend_from_slice(&[0, 0, 255, 0]);
            } else {
                d.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
        // File rows are [0, 1] then [1, 0]; top-down output leads with the
        // file's last row, so pixel 0 is palette index 1.
        d.extend_from_slice(&[0, 1, 1, 0]);
        let tex = TexFile::parse(&d).unwrap();
        let rgba = tex.decode_rgba(0).unwrap();
        assert_eq!(&rgba[..4], &[255, 0, 0, 0xff]);
        assert_eq!(&rgba[12..16], &[255, 0, 0, 0xff]);
    }

    #[test]
    fn honoring_alpha_reads_p8_palette_alpha() {
        // Same P8 file as above but entry 1 carries alpha 0x40: the
        // decal decode surfaces it where `decode_rgba` reports opaque.
        let mut d = header(2, 2, 1, 1);
        for i in 0..256usize {
            if i == 1 {
                d.extend_from_slice(&[0, 0, 255, 0x40]);
            } else {
                d.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
        d.extend_from_slice(&[0, 1, 1, 0]);
        let tex = TexFile::parse(&d).unwrap();
        let rgba = tex.decode_rgba_honoring_alpha(0).unwrap();
        assert_eq!(&rgba[..4], &[255, 0, 0, 0x40]);
        assert_eq!(&rgba[12..16], &[255, 0, 0, 0x40]);
        // Untouched decode still reports opaque.
        assert_eq!(tex.decode_rgba(0).unwrap()[3], 0xff);
    }

    #[test]
    fn parses_a1r5g5b5() {
        let mut d = header(1, 1, 6, 1);
        d.extend_from_slice(&(0x8000u16 | (31 << 10)).to_le_bytes());
        let tex = TexFile::parse(&d).unwrap();
        assert_eq!(tex.decode_rgba(0).unwrap(), vec![255, 0, 0, 255]);
    }

    #[test]
    fn reads_mip_levels() {
        let mut d = header(4, 4, 18, 2);
        d.extend_from_slice(&[0u8; 4 * 4 * 4]);
        d.extend_from_slice(&[7u8; 2 * 2 * 4]);
        let tex = TexFile::parse(&d).unwrap();
        assert_eq!(tex.levels.len(), 2);
        assert_eq!((tex.levels[1].width, tex.levels[1].height), (2, 2));
        assert!(tex.decode_rgba(1).unwrap().iter().all(|&b| b == 7));
    }

    #[test]
    fn rejects_zero_size() {
        let d = header(0, 4, 18, 1);
        assert!(matches!(
            TexFile::parse(&d),
            Err(FormatError::InvalidValue { .. })
        ));
    }

    #[test]
    fn rejects_truncated_pixels() {
        let mut d = header(4, 4, 18, 1);
        d.extend_from_slice(&[0u8; 10]);
        assert!(TexFile::parse(&d).is_err());
    }

    #[test]
    fn frame_base_stem_strips_only_four_digits() {
        assert_eq!(frame_base_stem("s_thames-0009"), Some("s_thames"));
        assert_eq!(frame_base_stem("s_ocean-0001"), Some("s_ocean"));
        assert_eq!(frame_base_stem("r1_l"), None);
        assert_eq!(frame_base_stem("base4_garage-basewindow"), None);
        assert_eq!(frame_base_stem("x-123"), None);
        assert_eq!(frame_base_stem("x-12345"), None);
        assert_eq!(frame_base_stem("-0001"), None);
    }

    #[test]
    fn preserves_unknown_type() {
        let mut d = header(1, 1, 99, 1);
        d.extend_from_slice(&[1, 2, 3, 4]);
        let tex = TexFile::parse(&d).unwrap();
        assert_eq!(tex.header.format, PixelFormat::Unknown(99));
        assert!(tex.decode_rgba(0).is_none());
    }
}
