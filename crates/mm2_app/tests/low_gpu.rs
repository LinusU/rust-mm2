//! F30 edge case "low GPU capability": the texture ceiling follows the
//! render device's `max_texture_dimension_2d` when that is smaller than
//! the default, through the production `load_image` path.
//!
//! The ceiling is process-wide, so this suite is its own test binary and
//! its legs run in one `#[test]` in a fixed order. The device limit is
//! *adopted* here by calling `adopt_device_limit` with the number a weak
//! adapter would report; no adapter is queried and no GPU upload is
//! exercised — `adopt_render_device`, the system that reads the number off
//! a real `RenderDevice`, is not run (a device cannot be built without an
//! adapter). Synthetic data only.

use std::path::Path;

use mm2_app::city::load_image;
use mm2_app::texture_budget::{
    MAX_TEXTURE_DIM, adopt_device_limit, ceiling, device_limit, forget_device_limit,
};
use mm2_assets::Vfs;

/// An uncompressed 32bpp TGA, top-left origin, opaque green fill.
fn tga32(w: u16, h: u16) -> Vec<u8> {
    let mut t = vec![0u8; 18];
    t[2] = 2;
    t[12..14].copy_from_slice(&w.to_le_bytes());
    t[14..16].copy_from_slice(&h.to_le_bytes());
    t[16] = 32;
    t[17] = 0x28;
    for _ in 0..(w as usize * h as usize) {
        t.extend_from_slice(&[0, 255, 0, 255]);
    }
    t
}

fn write(root: &Path, rel: &str, bytes: Vec<u8>) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn a_weak_adapters_limit_lowers_the_ceiling_for_mod_textures() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "texture/fits_2048.tga", tga32(2048, 1));
    write(dir.path(), "texture/past_2048.tga", tga32(2049, 1));
    write(dir.path(), "texture/past_2048_tall.tga", tga32(1, 4096));
    write(dir.path(), "texture/small.tga", tga32(64, 64));
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();

    // No device adopted: the default ceiling decides, so 4096 decodes.
    forget_device_limit();
    assert_eq!(device_limit(), None);
    assert_eq!(ceiling(), MAX_TEXTURE_DIM);
    assert!(load_image(&vfs, "past_2048_tall").is_some());

    // A 2048-limit adapter: the same file is now refused by name, the
    // ceiling itself still decodes, and a small texture is unaffected.
    adopt_device_limit(2048);
    assert_eq!(device_limit(), Some(2048));
    assert_eq!(ceiling(), 2048);
    assert!(load_image(&vfs, "fits_2048").is_some());
    assert!(load_image(&vfs, "past_2048").is_none());
    assert!(load_image(&vfs, "past_2048_tall").is_none());
    assert!(load_image(&vfs, "small").is_some());

    // A big adapter never raises the ceiling past the default.
    adopt_device_limit(16_384);
    assert_eq!(ceiling(), MAX_TEXTURE_DIM);
    write(
        dir.path(),
        "texture/over_default.tga",
        tga32(MAX_TEXTURE_DIM as u16 + 1, 1),
    );
    assert!(load_image(&vfs, "over_default").is_none());

    // A zero report is not a device limit and does not refuse everything.
    forget_device_limit();
    adopt_device_limit(0);
    assert_eq!(device_limit(), None);
    assert!(load_image(&vfs, "small").is_some());
}
