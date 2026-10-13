//! F30 edge case "slow storage": loading an archive (and a loose install
//! file) from media that delivers its bytes over time.
//!
//! A FIFO stands in for slow media: a writer thread trickles the exact
//! bytes of a real (synthetic) `DAVE` archive in small chunks with a pause
//! between them. The loader must wait for the stream, return it whole, and
//! report a stream that stops early — or a loose path that is not a regular
//! file, which a naive read could block on forever — as an error. The
//! failure it guards is a silent short read, an empty buffer, a swallowed
//! error or a hang, none of which is a pass.
//!
//! The media and the content are synthetic: this is integration evidence at
//! the loader, not a real slow device. `Vfs::mount_archive` is exactly the
//! function the app's `mount_install` calls for a real `.ar` (the same
//! `std::fs::read` + `DaveArchive::parse` sequence), and `DirSource::read`
//! is the loose-file reader the app uses.
#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use mm2_assets::{AssetsError, InstallMount, SourceKind, Vfs, mount_install, priority};

const HEADER_SIZE: usize = 0x800;
const ENTRY_SIZE: usize = 16;

/// A minimal DAVE archive (uncompressed entries), laid out like the one in
/// `mm2_formats::dave`'s and `non_ascii_install.rs`'s own tests.
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

/// Create a FIFO with the system `mkfifo`.
fn mkfifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo {} failed", path.display());
}

/// Trickle `bytes` into `fifo` in `chunk`-sized writes, pausing `pause`
/// between them, then close. Write errors end the thread quietly: a reader
/// that stops early (a truncated stream) closes its end, and that is the
/// reader's answer, not a writer failure.
fn trickle(fifo: PathBuf, bytes: Vec<u8>, chunk: usize, pause: Duration) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let Ok(mut file) = fs::OpenOptions::new().write(true).open(&fifo) else {
            return;
        };
        for part in bytes.chunks(chunk) {
            if file.write_all(part).is_err() {
                return;
            }
            thread::sleep(pause);
        }
    })
}

#[test]
fn a_slow_archive_stream_mounts_completely_and_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("slow.ar");
    mkfifo(&fifo);
    let bytes = archive(&[
        ("tune/pack.csv", b"slow archive tuning"),
        ("geometry/vpbug.pkg", &[0xAB; 40_000]),
    ]);
    // ~11 chunks at 2 ms each: the mount cannot finish before the writer
    // has sent the whole archive.
    let writer = trickle(fifo.clone(), bytes, 4096, Duration::from_millis(2));

    let started = Instant::now();
    let mut vfs = Vfs::new();
    vfs.mount_archive(&fifo, priority::ARCHIVE)
        .expect("the whole stream mounts");
    let waited = started.elapsed();
    writer.join().unwrap();

    assert!(
        waited >= Duration::from_millis(10),
        "mounted after only {waited:?} of a slow stream"
    );
    assert_eq!(vfs.source_count(), 1);
    assert_eq!(
        vfs.read_logical("tune/pack.csv").unwrap(),
        b"slow archive tuning"
    );
    let (body, resolved) = vfs.read_path("geometry/vpbug.pkg").unwrap();
    assert_eq!(body, vec![0xAB; 40_000]);
    assert_eq!(resolved.source.kind, SourceKind::Archive);
    assert_eq!(resolved.source.path, fifo);
}

#[test]
fn a_slow_archive_stream_that_stops_early_is_a_reported_error() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("cut.ar");
    mkfifo(&fifo);
    let full = archive(&[("geometry/vpbug.pkg", &[0xCD; 8_192])]);
    let half = full[..full.len() / 2].to_vec();
    let writer = trickle(fifo.clone(), half, 512, Duration::from_millis(1));

    let mut vfs = Vfs::new();
    let err = vfs
        .mount_archive(&fifo, priority::ARCHIVE)
        .expect_err("an archive that stops early is an error");
    writer.join().unwrap();

    assert!(matches!(err, AssetsError::Archive { .. }), "{err:?}");
    // Reported, never turned into a partial mount.
    assert_eq!(vfs.source_count(), 0);
    assert!(vfs.resolve("geometry/vpbug.pkg").is_none());
    assert!(matches!(
        vfs.read_logical("geometry/vpbug.pkg"),
        Err(AssetsError::NotFound(_))
    ));
}

#[test]
fn mount_install_reports_an_archive_it_cannot_read() {
    let dir = tempfile::tempdir().unwrap();
    let install = dir.path().join("install");
    fs::create_dir_all(&install).unwrap();
    let full = archive(&[("geometry/vpbug.pkg", &[0x11; 4_096])]);
    // A regular `.ar` that storage handed back incomplete (say, a copy from
    // slow media that has not finished): a real file, not a real archive.
    fs::write(install.join("mm2core.ar"), &full[..full.len() / 2]).unwrap();

    let mut vfs = Vfs::new();
    let report = mount_install(&mut vfs, &install, &InstallMount::default()).unwrap();

    assert!(report.archives.is_empty());
    assert_eq!(report.skipped.len(), 1, "{:?}", report.skipped);
    let (path, reason) = &report.skipped[0];
    assert_eq!(path, &install.join("mm2core.ar"));
    assert!(!reason.is_empty(), "the skip must name its reason");
    assert!(vfs.resolve("geometry/vpbug.pkg").is_none());
}

#[test]
fn a_loose_path_that_is_not_a_regular_file_is_refused_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    fs::create_dir_all(data.join("tune")).unwrap();
    fs::write(data.join("tune/pack.csv"), b"real").unwrap();

    let mut vfs = Vfs::new();
    vfs.mount_dir(&data, priority::LOOSE).unwrap();
    assert_eq!(vfs.read_logical("tune/pack.csv").unwrap(), b"real");

    // Storage hands back a special file (here a FIFO with no writer) in
    // place of the loose file. A read of it would block forever; the
    // mount's read-time recheck must refuse it by name instead.
    fs::remove_file(data.join("tune/pack.csv")).unwrap();
    mkfifo(&data.join("tune/pack.csv"));
    let err = vfs
        .read_path("tune/pack.csv")
        .expect_err("a non-regular file is not content");
    assert!(matches!(err, AssetsError::NotFound(_)), "{err:?}");
}
