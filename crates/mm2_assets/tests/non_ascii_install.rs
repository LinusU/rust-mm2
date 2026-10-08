//! F30 edge case: a non-ASCII installation path.
//!
//! Real installs live under `C:\Users\Zoë\Spiele\Midtown Madness 2` or a
//! macOS folder with accents and spaces. Mounting, resolving, reading and
//! fingerprinting must treat that directory like any other: the archive and
//! the loose files both resolve, provenance names the real path, and a mod
//! under an equally awkward directory still overrides. Synthetic data only.

use std::fs;
use std::path::{Path, PathBuf};

use mm2_assets::{InstallMount, SourceKind, Vfs, fingerprint, mount_install, mount_mods};

const HEADER_SIZE: usize = 0x800;
const ENTRY_SIZE: usize = 16;

/// A minimal DAVE archive (uncompressed entries), laid out like the one in
/// `mm2_formats::dave`'s own tests.
fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut names = Vec::new();
    let mut name_offsets = Vec::new();
    for (name, _) in files {
        name_offsets.push(names.len() as u32);
        names.extend_from_slice(name.as_bytes());
        names.push(0);
    }
    let mft_size = (files.len() * ENTRY_SIZE).div_ceil(HEADER_SIZE) * HEADER_SIZE;
    let mut offset = HEADER_SIZE + mft_size + names.len();

    let mut data = Vec::new();
    data.extend_from_slice(b"DAVE");
    data.extend_from_slice(&(files.len() as u32).to_le_bytes());
    data.extend_from_slice(&(mft_size as u32).to_le_bytes());
    data.extend_from_slice(&(names.len() as u32).to_le_bytes());
    data.resize(HEADER_SIZE, 0);
    for ((_, contents), name_offset) in files.iter().zip(&name_offsets) {
        data.extend_from_slice(&name_offset.to_le_bytes());
        data.extend_from_slice(&(offset as u32).to_le_bytes());
        data.extend_from_slice(&(contents.len() as u32).to_le_bytes());
        data.extend_from_slice(&(contents.len() as u32).to_le_bytes());
        offset += contents.len();
    }
    data.resize(HEADER_SIZE + mft_size, 0);
    data.extend_from_slice(&names);
    for (_, contents) in files {
        data.extend_from_slice(contents);
    }
    data
}

fn write(dir: &Path, rel: &str, contents: &[u8]) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, contents).unwrap();
}

/// An install under a path with accents, CJK, an em dash, spaces and
/// parentheses, holding an archive and loose files.
fn awkward_install(root: &Path) -> PathBuf {
    let install = root.join("Spiele — Zoë's Midtown Madness 2 (日本語)");
    fs::create_dir_all(&install).unwrap();
    fs::write(
        install.join("mm2core.ar"),
        archive(&[
            ("tune/pack.csv", b"from the archive"),
            ("geometry/vpbug.pkg", b"archived body"),
        ]),
    )
    .unwrap();
    write(&install, "tune/pack.csv", b"loose beats the archive");
    write(&install, "config/readme.txt", b"loose only");
    install
}

#[test]
fn an_install_under_a_non_ascii_path_mounts_resolves_and_reads() {
    let root = tempfile::tempdir().unwrap();
    let install = awkward_install(root.path());

    let mut vfs = Vfs::new();
    let report = mount_install(&mut vfs, &install, &InstallMount::default()).unwrap();
    assert_eq!(report.archives, vec![install.join("mm2core.ar")]);
    assert!(report.skipped.is_empty(), "skipped: {:?}", report.skipped);
    assert!(report.loose_files);

    // Archive member: bytes and provenance name the real directory.
    let (bytes, r) = vfs.read_path("geometry/vpbug.pkg").unwrap();
    assert_eq!(bytes, b"archived body");
    assert_eq!(r.source.path, install.join("mm2core.ar"));
    assert_eq!(r.source.kind, SourceKind::Archive);

    // Loose file shadows the archive's copy, as it does in an ASCII path.
    let (bytes, r) = vfs.read_path("tune/pack.csv").unwrap();
    assert_eq!(bytes, b"loose beats the archive");
    assert_eq!(r.source.path, install.join("tune/pack.csv"));
    assert_eq!(
        vfs.read_logical("config/readme.txt").unwrap(),
        b"loose only"
    );

    // Case-insensitive logical lookup is unaffected by the directory name.
    assert!(vfs.resolve("GEOMETRY/VPBUG.PKG").is_some());
}

#[test]
fn a_non_ascii_install_yields_the_same_catalog_as_an_ascii_one() {
    let root = tempfile::tempdir().unwrap();
    let awkward = awkward_install(root.path());
    let plain = root.path().join("plain");
    fs::create_dir_all(&plain).unwrap();
    for entry in fs::read_dir(&awkward).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            fs::copy(entry.path(), plain.join(entry.file_name())).unwrap();
        }
    }
    write(&plain, "tune/pack.csv", b"loose beats the archive");
    write(&plain, "config/readme.txt", b"loose only");

    let list = |dir: &Path| {
        let mut vfs = Vfs::new();
        mount_install(&mut vfs, dir, &InstallMount::default()).unwrap();
        vfs.list()
    };
    assert_eq!(list(&awkward), list(&plain));

    // The fingerprint folds the source path in, so it is repeatable for one
    // install and differs between two locations — it never errors or panics
    // on the non-ASCII path.
    let fp = |dir: &Path| {
        let mut vfs = Vfs::new();
        mount_install(&mut vfs, dir, &InstallMount::default()).unwrap();
        fingerprint::catalog(&vfs)
    };
    assert_eq!(fp(&awkward), fp(&awkward));
    assert!(fp(&awkward).starts_with("fnv1a64:"));
}

#[test]
fn a_mod_under_a_non_ascii_directory_still_overrides_the_install() {
    let root = tempfile::tempdir().unwrap();
    let install = awkward_install(root.path());
    let mods = root.path().join("Müll & Mods ñ");
    let m = mods.join("zoë-skin");
    write(&m, "mod.toml", b"[mod]\nid = \"zoe-skin\"\n");
    write(&m, "geometry/vpbug.pkg", b"modded body");

    let mut vfs = Vfs::new();
    mount_install(&mut vfs, &install, &InstallMount::default()).unwrap();
    let manifests = mount_mods(&mut vfs, &mods).unwrap();
    assert_eq!(manifests.len(), 1);

    let (bytes, r) = vfs.read_path("geometry/vpbug.pkg").unwrap();
    assert_eq!(bytes, b"modded body");
    assert_eq!(r.source.path, m.join("geometry/vpbug.pkg"));
}

#[test]
fn a_loose_file_with_a_non_ascii_name_is_listed_and_readable() {
    let root = tempfile::tempdir().unwrap();
    let install = awkward_install(root.path());
    write(&install, "texture/café_ñ.tga", b"accented");

    let mut vfs = Vfs::new();
    mount_install(&mut vfs, &install, &InstallMount::default()).unwrap();
    assert!(vfs.list().iter().any(|p| p == "texture/café_ñ.tga"));
    assert_eq!(vfs.read_logical("texture/café_ñ.tga").unwrap(), b"accented");
}
