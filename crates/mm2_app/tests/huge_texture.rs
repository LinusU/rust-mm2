//! F30 edge case "huge mod texture": a texture past the supported size is
//! refused by name before it is decoded, through the production
//! `load_image` path that mods' PNG/TGA replacements take.
//!
//! The oversize files are real, valid, small TGAs (8193 × 1 — a few tens
//! of KB), so a decoder would accept them: it is the size guard, not a
//! decode error, that turns them into a miss. Synthetic data only; the
//! limit itself is wgpu's default `max_texture_dimension_2d`, not a
//! measurement of any adapter, and no GPU upload is exercised here.

use mm2_app::city::load_image;
use mm2_app::texture_budget::MAX_TEXTURE_DIM;
use mm2_assets::Vfs;

use crate::support::write;

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

fn mounted(dirs: &[&std::path::Path]) -> Vfs {
    let mut vfs = Vfs::new();
    for (priority, d) in dirs.iter().enumerate() {
        vfs.mount_dir(d, priority as i32).unwrap();
    }
    vfs
}

#[test]
fn a_texture_at_the_ceiling_decodes_and_one_past_it_is_a_miss() {
    let dir = tempfile::tempdir().unwrap();
    let max = MAX_TEXTURE_DIM as u16;
    write(dir.path(), "texture/at_wide.tga", tga32(max, 1));
    write(dir.path(), "texture/at_tall.tga", tga32(1, max));
    write(dir.path(), "texture/over_wide.tga", tga32(max + 1, 1));
    write(dir.path(), "texture/over_tall.tga", tga32(1, max + 1));
    let vfs = mounted(&[dir.path()]);

    let (img, _) = load_image(&vfs, "at_wide").expect("the ceiling itself is allowed");
    assert_eq!(img.texture_descriptor.size.width, MAX_TEXTURE_DIM);
    let (img, _) = load_image(&vfs, "at_tall").expect("the ceiling itself is allowed");
    assert_eq!(img.texture_descriptor.size.height, MAX_TEXTURE_DIM);
    assert!(load_image(&vfs, "over_wide").is_none());
    assert!(load_image(&vfs, "over_tall").is_none());
}

#[test]
fn an_oversize_mod_texture_is_a_miss_not_a_silent_fall_back_to_the_original() {
    let base = tempfile::tempdir().unwrap();
    let modd = tempfile::tempdir().unwrap();
    write(base.path(), "texture/skin.tga", tga32(4, 4));
    write(
        modd.path(),
        "texture/skin.tga",
        tga32(MAX_TEXTURE_DIM as u16 + 1, 1),
    );

    // The base alone decodes; the mounted mod wins resolution and is then
    // refused, so the caller sees the miss it would see for a corrupt mod
    // file rather than a texture the mod author did not supply.
    assert!(load_image(&mounted(&[base.path()]), "skin").is_some());
    assert!(load_image(&mounted(&[base.path(), modd.path()]), "skin").is_none());
}

#[test]
fn a_header_that_lies_about_a_huge_size_is_a_miss() {
    let dir = tempfile::tempdir().unwrap();
    // 65535 × 65535 declared, no pixel data. The size guard answers before
    // the decoder runs; this pins the miss, not the absence of an allocation.
    let mut t = tga32(1, 1);
    t[12..16].copy_from_slice(&[0xff; 4]);
    write(dir.path(), "texture/liar.tga", t);
    assert!(load_image(&mounted(&[dir.path()]), "liar").is_none());
}
