//! F30 edge case: a non-ASCII installation path, at process level.
//!
//! `mm2_assets/tests/non_ascii_install.rs` covers the VFS (archive, loose
//! files, mods). This drives the real binary: it must mount an install and
//! bind a profile store whose directories carry accents, CJK and spaces, and
//! the write guard must still recognise that install as protected.

use std::path::Path;
use std::process::{Command, Output};

const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");

fn run(install: &Path, extra: &[&std::ffi::OsStr]) -> (Output, String) {
    let out = Command::new(MM2_EXE)
        .arg("--mm2-path")
        .arg(install)
        .args(["--headless", "--dev-world", "--frames", "5"])
        .args(extra)
        .env_remove("RUST_LOG")
        .output()
        .expect("spawn mm2");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out, log)
}

fn awkward_install(root: &Path) -> std::path::PathBuf {
    let install = root.join("Spiele — Zoë's Midtown Madness 2 (日本語)");
    std::fs::create_dir_all(install.join("tune")).unwrap();
    std::fs::write(install.join("tune/readme.txt"), b"loose").unwrap();
    install
}

#[test]
fn an_install_under_a_non_ascii_path_mounts_and_the_app_runs() {
    let root = tempfile::tempdir().unwrap();
    let install = awkward_install(root.path());
    // The 5-frame smoke verdict ("never grounded") is not about the path;
    // the mount record is.
    let (_, log) = run(&install, &[]);
    assert!(
        log.contains("mounted MM2 installation"),
        "install not mounted:\n{log}"
    );
    assert!(
        log.contains("Zoë's Midtown Madness 2 (日本語)"),
        "provenance lost the real directory name:\n{log}"
    );
}

#[test]
fn a_non_ascii_profile_dir_beside_the_install_binds() {
    let root = tempfile::tempdir().unwrap();
    let install = awkward_install(root.path());
    let saves = root.path().join("Spielstände ñ");
    let (out, log) = run(
        &install,
        &[
            "--profile-dir".as_ref(),
            saves.as_os_str(),
            "--new-profile".as_ref(),
            "Zoë".as_ref(),
        ],
    );
    assert_ne!(
        out.status.code(),
        Some(2),
        "refused a user location:\n{log}"
    );
    assert!(
        log.contains("driver profile bound"),
        "no profile bound:\n{log}"
    );
    assert!(saves.is_dir(), "the profile store was not created");
}

#[test]
fn a_non_ascii_profile_dir_inside_the_install_is_still_refused() {
    let root = tempfile::tempdir().unwrap();
    let install = awkward_install(root.path());
    let saves = install.join("Spielstände ñ");
    let (out, log) = run(
        &install,
        &[
            "--profile-dir".as_ref(),
            saves.as_os_str(),
            "--new-profile".as_ref(),
            "Zoë".as_ref(),
        ],
    );
    assert_eq!(out.status.code(), Some(2), "not refused:\n{log}");
    assert!(
        log.contains("original installation"),
        "no reason given:\n{log}"
    );
    assert!(!saves.exists(), "the refused store was created anyway");
}
