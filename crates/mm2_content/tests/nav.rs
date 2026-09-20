//! F09-B nav producer tests: `load_nav_graph`/`load_nav_overrides`
//! resolve, parse and distill through the real VFS — synthetic mount
//! fixtures, no original data.

use std::path::Path;

use mm2_assets::Vfs;
use mm2_content::NavLoadError;
use mm2_content::nav::{load_nav_graph, load_nav_overrides};

fn write(dir: &Path, rel: &str, contents: &[u8]) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

/// The smallest parseable `CAI1`: no roads, no intersections, no
/// culling rooms. Magic + two u16 counts + u32 cull count.
const EMPTY_BAI: &[u8] = &[
    b'C', b'A', b'I', b'1', // magic
    0, 0, // intersections
    0, 0, // roads
    0, 0, 0, 0, // culling rooms
];

#[test]
fn nav_graph_loads_through_the_vfs() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "city/test.bai", EMPTY_BAI);
    let vfs = vfs_of(dir.path());

    let build = load_nav_graph(&vfs, "test").expect("empty graph builds");
    assert_eq!(build.graph.stats().roads, 0);
    // City names are case-insensitive in the logical path.
    let build = load_nav_graph(&vfs, "TEST").expect("empty graph builds");
    assert_eq!(build.graph.stats().roads, 0);
}

#[test]
fn missing_bai_is_a_resolve_error() {
    let vfs = vfs_of(tempfile::tempdir().unwrap().path());
    let err = load_nav_graph(&vfs, "nowhere").unwrap_err();
    assert!(matches!(err, NavLoadError::Resolve(_)), "{err:?}");
}

#[test]
fn aimap_overrides_distill_exceptions_and_speed_limit() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "city/test.aimap",
        b"[Speed Limit]\n15\n\n[Ambients Drive On The Left]\n1\n\n[Exceptions]\n3\n12 0.00 0\n30 0.00 25\n44 0.50 0\n",
    );
    let vfs = vfs_of(dir.path());

    let overrides = load_nav_overrides(&vfs, "city/test.aimap")
        .expect("aimap parses")
        .expect("file resolves");
    // Zero-density rows close; the 0.50 row does not.
    assert!(overrides.is_closed(12));
    assert!(overrides.is_closed(30));
    assert!(!overrides.is_closed(44));
    assert_eq!(overrides.default_speed_limit, Some(15.0));
    assert_eq!(overrides.speed_limit(30), Some(25.0));
    assert_eq!(overrides.speed_limit(12), Some(15.0));
    assert_eq!(overrides.drive_on_left, Some(1));
}

#[test]
fn an_absent_aimap_is_no_overrides_not_an_error() {
    let vfs = vfs_of(tempfile::tempdir().unwrap().path());
    let overrides = load_nav_overrides(&vfs, "city/test.aimap").expect("absent is fine");
    assert!(overrides.is_none());
}

#[test]
fn a_malformed_aimap_is_an_error_not_empty_overrides() {
    let dir = tempfile::tempdir().unwrap();
    // Stray data before any section header is a parse failure.
    write(dir.path(), "city/test.aimap", b"15\n[Speed Limit]\n15\n");
    let vfs = vfs_of(dir.path());
    let err = load_nav_overrides(&vfs, "city/test.aimap").unwrap_err();
    assert!(matches!(err, NavLoadError::ParseAimap(_)), "{err:?}");
}
