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
//! The ceiling is the smaller of [`MAX_TEXTURE_DIM`] and what the render
//! device reports (F30 edge case "low GPU capability"): an adapter that
//! promises less than 8192 — an old integrated part, a software
//! rasterizer — refuses the same oversize mod texture by name instead of
//! failing texture creation inside wgpu.
//!
//! This is an implementation choice, not an original rule: the retail
//! `.tex` files are far smaller, so the ceiling only ever bites on mods.

use bevy::prelude::*;
use bevy::render::renderer::RenderDevice;
use std::sync::atomic::{AtomicU32, Ordering};

/// Largest width or height, in pixels, a VFS texture may declare on any
/// device. 8192 is wgpu's default `max_texture_dimension_2d`, which every
/// desktop adapter this project targets meets; a bigger texture would
/// need a limit the device does not promise. A device that reports less
/// lowers the ceiling through [`adopt_device_limit`].
pub const MAX_TEXTURE_DIM: u32 = 8192;

/// The render device's `max_texture_dimension_2d`, or 0 while no device
/// has been adopted (headless runs and tools never have one). Texture
/// decoding is a free function called from several loaders, none of which
/// holds the device, and one process has one device — hence a process-wide
/// value rather than a parameter threaded through each.
static DEVICE_LIMIT: AtomicU32 = AtomicU32::new(0);

/// The limit adopted from the render device, if one has been.
pub fn device_limit() -> Option<u32> {
    match DEVICE_LIMIT.load(Ordering::Relaxed) {
        0 => None,
        limit => Some(limit),
    }
}

/// The ceiling in force: [`MAX_TEXTURE_DIM`], lowered to the adopted
/// device limit.
pub fn ceiling() -> u32 {
    ceiling_for(device_limit())
}

/// [`ceiling`] for a given device limit (`None`: no device).
pub fn ceiling_for(device_limit: Option<u32>) -> u32 {
    device_limit.map_or(MAX_TEXTURE_DIM, |l| l.min(MAX_TEXTURE_DIM))
}

/// Record the render device's `max_texture_dimension_2d`. A zero limit is
/// not a device report (wgpu never promises it) and is ignored rather than
/// refusing every texture.
pub fn adopt_device_limit(limit: u32) {
    if limit > 0 {
        DEVICE_LIMIT.store(limit, Ordering::Relaxed);
    }
}

/// Forget the adopted limit. Test support: the value is process-wide.
#[doc(hidden)]
pub fn forget_device_limit() {
    DEVICE_LIMIT.store(0, Ordering::Relaxed);
}

/// `PreStartup` and `First`: adopt the render device's texture limit once
/// the renderer has produced a device. Native startup resolves the device
/// before the first schedule runs, so the `PreStartup` run lands before
/// any loader; `First` keeps trying if a backend delivers it later.
pub fn adopt_render_device(device: Option<Res<RenderDevice>>) {
    if device_limit().is_some() {
        return;
    }
    let Some(device) = device else { return };
    let limit = device.limits().max_texture_dimension_2d;
    adopt_device_limit(limit);
    if ceiling() < MAX_TEXTURE_DIM {
        warn!(
            device_limit = limit,
            ceiling = ceiling(),
            "the render device supports smaller textures than the default ceiling; \
             larger textures are refused by name"
        );
    } else {
        info!(device_limit = limit, ceiling = ceiling(), "texture ceiling");
    }
}

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

/// Whether a `width` × `height` texture is inside the [`ceiling`].
pub fn within_budget(width: u32, height: u32) -> bool {
    let max = ceiling();
    width <= max && height <= max
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
    fn a_device_limit_can_lower_the_ceiling_but_never_raise_it() {
        assert_eq!(ceiling_for(None), MAX_TEXTURE_DIM);
        assert_eq!(ceiling_for(Some(2048)), 2048);
        assert_eq!(ceiling_for(Some(MAX_TEXTURE_DIM)), MAX_TEXTURE_DIM);
        // A big-GPU adapter (16384) does not licence what the default refuses.
        assert_eq!(ceiling_for(Some(16_384)), MAX_TEXTURE_DIM);
    }

    #[test]
    fn the_ceiling_is_inclusive_on_each_axis() {
        assert!(within_budget(MAX_TEXTURE_DIM, MAX_TEXTURE_DIM));
        assert!(within_budget(1, MAX_TEXTURE_DIM));
        assert!(!within_budget(MAX_TEXTURE_DIM + 1, 1));
        assert!(!within_budget(1, MAX_TEXTURE_DIM + 1));
    }
}
