//! A size ceiling for textures decoded from the VFS (F30 edge case "huge
//! mod texture").
//!
//! A mod may replace any `texture/<stem>` with a PNG/TGA/KTX2 file whose
//! header declares any size. Decoding allocates width × height × 4 bytes
//! before anything can look at the result, and wgpu rejects a texture past
//! the adapter's `max_texture_dimension_2d` with a validation error that
//! takes the whole renderer down. So the declared size is read from the
//! container header and checked *before* the decoder runs; a file over the
//! ceiling is a decode miss, reported by name, like any other unreadable
//! texture — never decoded, never replaced by a guess.
//!
//! This is an implementation choice, not an original rule: the retail
//! `.tex` files are far smaller, so the ceiling only ever bites on mods.

/// Largest width or height, in pixels, a VFS texture may declare. 8192 is
/// wgpu's default `max_texture_dimension_2d`, which every desktop adapter
/// this project targets meets; a bigger texture would need a limit the
/// device does not promise.
pub const MAX_TEXTURE_DIM: u32 = 8192;

/// Width and height a texture container declares, read from its header
/// alone. `None` when the extension is not one whose header is understood
/// here, or the header is too short to hold the fields — the decoder then
/// reports its own error, and the post-decode check in
/// [`within_budget`] still applies.
pub fn declared_dimensions(bytes: &[u8], ext: &str) -> Option<(u32, u32)> {
    let u32_at = |at: usize, be: bool| -> Option<u32> {
        let b: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
        Some(if be {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    };
    let u16_at = |at: usize| -> Option<u32> {
        let b: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
        Some(u16::from_le_bytes(b) as u32)
    };
    match ext.to_ascii_lowercase().as_str() {
        // 8-byte signature, then the IHDR chunk: length, "IHDR", width, height.
        "png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.get(12..16) == Some(b"IHDR") => {
            Some((u32_at(16, true)?, u32_at(20, true)?))
        }
        // The 18-byte TGA header keeps width and height at offsets 12 and 14.
        "tga" => Some((u16_at(12)?, u16_at(14)?)),
        // 12-byte identifier, vkFormat, typeSize, then pixelWidth/pixelHeight.
        "ktx2" if bytes.starts_with(b"\xabKTX 20\xbb\r\n\x1a\n") => {
            Some((u32_at(20, false)?, u32_at(24, false)?))
        }
        _ => None,
    }
}

/// Whether a `width` × `height` texture is inside [`MAX_TEXTURE_DIM`].
pub fn within_budget(width: u32, height: u32) -> bool {
    width <= MAX_TEXTURE_DIM && height <= MAX_TEXTURE_DIM
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_header(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        v.extend_from_slice(&13u32.to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v
    }

    fn tga_header(w: u16, h: u16) -> Vec<u8> {
        let mut v = vec![0u8; 12];
        v.extend_from_slice(&w.to_le_bytes());
        v.extend_from_slice(&h.to_le_bytes());
        v.extend_from_slice(&[32, 0x28]);
        v
    }

    fn ktx2_header(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"\xabKTX 20\xbb\r\n\x1a\n".to_vec();
        v.extend_from_slice(&[0; 8]);
        v.extend_from_slice(&w.to_le_bytes());
        v.extend_from_slice(&h.to_le_bytes());
        v
    }

    #[test]
    fn each_container_header_yields_its_declared_size() {
        assert_eq!(
            declared_dimensions(&png_header(70_000, 3), "png"),
            Some((70_000, 3))
        );
        assert_eq!(
            declared_dimensions(&tga_header(65_535, 12), "tga"),
            Some((65_535, 12))
        );
        assert_eq!(
            declared_dimensions(&ktx2_header(16_384, 16_384), "ktx2"),
            Some((16_384, 16_384))
        );
        // Extensions match case-insensitively, as the VFS does.
        assert_eq!(declared_dimensions(&png_header(4, 5), "PNG"), Some((4, 5)));
    }

    #[test]
    fn a_header_that_is_short_or_not_the_claimed_container_declares_nothing() {
        assert_eq!(declared_dimensions(&png_header(4, 4)[..20], "png"), None);
        assert_eq!(declared_dimensions(&tga_header(4, 4)[..13], "tga"), None);
        assert_eq!(declared_dimensions(&ktx2_header(4, 4)[..24], "ktx2"), None);
        // A PNG name on KTX2 bytes (and the reverse) is the decoder's to reject.
        assert_eq!(declared_dimensions(&ktx2_header(9, 9), "png"), None);
        assert_eq!(declared_dimensions(&png_header(9, 9), "ktx2"), None);
        assert_eq!(declared_dimensions(&[], "tga"), None);
        assert_eq!(declared_dimensions(&png_header(9, 9), "jpg"), None);
    }

    #[test]
    fn the_ceiling_is_inclusive_on_each_axis() {
        assert!(within_budget(MAX_TEXTURE_DIM, MAX_TEXTURE_DIM));
        assert!(within_budget(1, MAX_TEXTURE_DIM));
        assert!(!within_budget(MAX_TEXTURE_DIM + 1, 1));
        assert!(!within_budget(1, MAX_TEXTURE_DIM + 1));
    }
}
