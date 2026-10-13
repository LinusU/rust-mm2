//! F30 edge case "low GPU capability": how big the retail textures
//! actually are, measured from their container headers and held against
//! the limits a weak adapter may report for `max_texture_dimension_2d`.
//!
//! Opt-in (`MM2_RETAIL=<dir>`); skipped without the operator's install,
//! and says so. Header reads only — no decode and no GPU upload. What
//! the engine does with a texture past the ceiling is pinned elsewhere:
//! `huge_texture` refuses an oversize mod file by name before decoding,
//! and `low_gpu` shows an adapter reporting 2048 lowering the ceiling.
//! This leg measures the originals those guards never bite.
//!
//! Two sets are measured, because they are not the same thing:
//!
//! - `texture/*` is what the VFS texture pipeline serves
//!   (`mm2_app::city::TEXTURE_EXTS`) — the originals a material binds
//!   and the renderer uploads. Every one of them must fit a 2048-limit
//!   adapter.
//! - `jpg/*` and `tune/*.tga` are menu/UI art the pipeline never loads.
//!   They are measured for completeness of "the largest retail image";
//!   `jpg/credits.jpg` is the one image above 2048 and it is a scrolling
//!   credits strip, not a surface.

use std::collections::BTreeMap;

use mm2_app::texture_budget::{MAX_TEXTURE_DIM, declared_dimensions};
use mm2_assets::{InstallMount, Vfs, mount_install};
use mm2_formats::tex::TexFile;

/// Every container whose header this leg can read a size from. The VFS
/// texture pipeline serves `png`/`ktx2`/`tga`/`tex` (`city::TEXTURE_EXTS`);
/// `jpg` is menu/UI art the pipeline does not load.
const IMAGE_EXTS: [&str; 5] = ["png", "ktx2", "tga", "tex", "jpg"];

/// The prefix the VFS texture pipeline resolves under.
const TEXTURE_PREFIX: &str = "texture/";

/// The low-end `max_texture_dimension_2d` this leg holds the pipeline
/// textures to. Every desktop adapter this project targets meets it; a
/// smaller one would refuse a retail surface by name (`low_gpu`).
const LOW_END_LIMIT: u32 = 2048;

/// Header-declared dimensions of one image container. `None` when the
/// header is not understood here — counted, never silently dropped.
fn dimensions(bytes: &[u8], ext: &str) -> Option<(u32, u32)> {
    match ext {
        "tex" => TexFile::parse(bytes)
            .ok()
            .map(|t| (u32::from(t.header.width), u32::from(t.header.height))),
        "jpg" => jpeg_dimensions(bytes),
        _ => declared_dimensions(bytes, ext),
    }
}

/// JPEG frame dimensions: walk the segment markers to the first
/// start-of-frame (SOF0..SOF15, excluding the length-only DHT/JPG/DAC
/// markers) and read its height×width.
fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..2)? != [0xFF, 0xD8] {
        return None; // not a JPEG (JFIF/EXIF both start with SOI)
    }
    let mut i = 2;
    while i + 3 < bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        if marker == 0xFF {
            i += 1; // fill byte before the marker
            continue;
        }
        // Standalone markers carry no length segment.
        if marker == 0x01 || (0xD0..=0xD8).contains(&marker) {
            i += 2;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]));
        if len < 2 {
            return None;
        }
        let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            let h = u16::from_be_bytes([*bytes.get(i + 5)?, *bytes.get(i + 6)?]);
            let w = u16::from_be_bytes([*bytes.get(i + 7)?, *bytes.get(i + 8)?]);
            return Some((u32::from(w), u32::from(h)));
        }
        i += 2 + len;
    }
    None
}

/// The larger axis of a texture; the ceiling applies per axis.
fn axis(w: u32, h: u32) -> u32 {
    w.max(h)
}

/// A coarse size bucket for the histogram, keyed by the larger axis.
fn bucket(a: u32) -> &'static str {
    match a {
        0..=64 => "<=64",
        65..=128 => "<=128",
        129..=256 => "<=256",
        257..=512 => "<=512",
        513..=1024 => "<=1024",
        1025..=2048 => "<=2048",
        2049..=4096 => "<=4096",
        4097..=8192 => "<=8192",
        _ => ">8192",
    }
}

#[test]
fn retail_textures_fit_the_texture_ceiling() {
    let Some((retail, _slot)) = crate::support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set; retail texture sizes NOT measured");
        return;
    };
    let mut vfs = Vfs::new();
    mount_install(&mut vfs, &retail, &InstallMount::default()).expect("mount the retail install");

    let mut by_ext: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_dir: BTreeMap<String, usize> = BTreeMap::new();
    let mut histogram: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut pipeline_histogram: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut largest: Vec<(u32, String, u32, u32)> = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();
    let mut pipeline_over: Vec<(String, u32, u32)> = Vec::new();
    let mut other_over: Vec<(String, u32, u32)> = Vec::new();
    let mut pipeline = 0usize;
    let mut pipeline_largest: Option<(u32, String, u32, u32)> = None;

    for logical in vfs.list() {
        let Some((_, ext)) = logical.rsplit_once('.') else {
            continue;
        };
        let ext = ext.to_ascii_lowercase();
        if !IMAGE_EXTS.contains(&ext.as_str()) {
            continue;
        }
        let is_pipeline = logical.starts_with(TEXTURE_PREFIX);
        let dir = logical.split('/').next().unwrap_or("").to_string();
        *by_ext.entry(ext.clone()).or_default() += 1;
        *by_dir.entry(dir).or_default() += 1;
        let bytes = match vfs.read_logical(&logical) {
            Ok(b) => b,
            Err(e) => {
                unreadable.push(format!("{logical}: {e}"));
                continue;
            }
        };
        let Some((w, h)) = dimensions(&bytes, &ext) else {
            unreadable.push(format!("{logical}: header not understood"));
            continue;
        };
        if is_pipeline {
            pipeline += 1;
            *pipeline_histogram.entry(bucket(axis(w, h))).or_default() += 1;
            if pipeline_largest
                .as_ref()
                .is_none_or(|(a, ..)| axis(w, h) > *a)
            {
                pipeline_largest = Some((axis(w, h), logical.clone(), w, h));
            }
            if axis(w, h) > LOW_END_LIMIT {
                pipeline_over.push((logical.clone(), w, h));
            }
        } else if axis(w, h) > LOW_END_LIMIT {
            other_over.push((logical.clone(), w, h));
        }
        *histogram.entry(bucket(axis(w, h))).or_default() += 1;
        largest.push((axis(w, h), logical.clone(), w, h));
    }
    largest.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    eprintln!("== retail image sizes ({}) ==", retail.display());
    eprintln!("by directory prefix: {by_dir:?}");
    eprintln!("by extension:        {by_ext:?}");
    eprintln!(
        "pipeline textures: {pipeline}, over {LOW_END_LIMIT}: {}",
        pipeline_over.len()
    );
    eprintln!("unreadable headers: {}", unreadable.len());
    eprintln!("size histogram (by larger axis):");
    for (k, n) in &histogram {
        eprintln!("  {k:>8}: {n}");
    }
    eprintln!("pipeline histogram (by larger axis):");
    for (k, n) in &pipeline_histogram {
        eprintln!("  {k:>8}: {n}");
    }
    eprintln!("largest 5:");
    for (_, name, w, h) in largest.iter().take(5) {
        eprintln!("  {name} {w}x{h}");
    }
    if let Some((_, name, w, h)) = &pipeline_largest {
        eprintln!("largest pipeline texture: {name} {w}x{h}");
    }
    eprintln!(
        "non-pipeline images over {LOW_END_LIMIT}: {}",
        other_over.len()
    );
    for (name, w, h) in &other_over {
        eprintln!("  {name} {w}x{h}");
    }
    for u in &unreadable {
        eprintln!("unreadable: {u}");
    }

    // Thousands of stock surfaces, as expected from the archives.
    assert!(
        pipeline > 1000,
        "expected thousands of retail textures, measured {pipeline}"
    );
    assert!(
        unreadable.is_empty(),
        "{} header(s) unreadable",
        unreadable.len()
    );
    // The low-GPU claim: every surface the texture pipeline serves fits an
    // adapter that reports 2048.
    assert!(
        pipeline_over.is_empty(),
        "{} pipeline texture(s) exceed a {LOW_END_LIMIT} limit (first: {:?})",
        pipeline_over.len(),
        pipeline_over.first()
    );
    assert!(
        pipeline_largest.as_ref().is_some_and(|(a, ..)| *a <= 1024),
        "the largest pipeline texture is unexpectedly big: {pipeline_largest:?}"
    );
    // Anything above 2048 is menu/UI art outside the pipeline, never a
    // surface an adapter would be asked to upload.
    assert!(
        other_over.iter().all(|(name, ..)| name.starts_with("jpg/")),
        "a non-menu image exceeds {LOW_END_LIMIT}: {other_over:?}"
    );
    assert!(
        largest.first().is_some_and(|(a, ..)| *a <= MAX_TEXTURE_DIM),
        "a retail image exceeds the {MAX_TEXTURE_DIM} ceiling: {:?}",
        largest.first()
    );
}
