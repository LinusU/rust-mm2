//! F30-AC05: the binary refuses to write its own data into the original
//! installation. Process level — `mm2_app::write_guard` has the unit
//! coverage of the containment rules.

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

#[test]
fn a_profile_dir_inside_the_install_is_refused_and_nothing_is_written() {
    let root = tempfile::tempdir().unwrap();
    let install = root.path().join("MM2");
    std::fs::create_dir(&install).unwrap();
    let saves = install.join("saves");
    let (out, log) = run(
        &install,
        &[
            "--profile-dir".as_ref(),
            saves.as_os_str(),
            "--new-profile".as_ref(),
            "Ada".as_ref(),
        ],
    );
    assert_eq!(out.status.code(), Some(2), "not refused:\n{log}");
    assert!(
        log.contains("original installation"),
        "no reason given:\n{log}"
    );
    assert!(!saves.exists(), "the refused store was created anyway");
}

#[test]
fn a_perf_log_and_a_screenshot_inside_the_install_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let install = root.path().join("MM2");
    std::fs::create_dir(&install).unwrap();
    for (flag, name) in [("--perf-log", "perf.csv"), ("--screenshot", "shot.png")] {
        let target = install.join(name);
        let (out, log) = run(&install, &[flag.as_ref(), target.as_os_str()]);
        assert_eq!(out.status.code(), Some(2), "{flag} not refused:\n{log}");
        assert!(!target.exists(), "{flag} wrote into the install");
    }
}

#[test]
fn a_profile_dir_beside_the_install_is_accepted() {
    let root = tempfile::tempdir().unwrap();
    let install = root.path().join("MM2");
    std::fs::create_dir(&install).unwrap();
    let saves = root.path().join("MM2-saves");
    let (out, log) = run(
        &install,
        &[
            "--profile-dir".as_ref(),
            saves.as_os_str(),
            "--new-profile".as_ref(),
            "Ada".as_ref(),
        ],
    );
    assert!(
        !log.contains("write destination refused"),
        "a user location was refused:\n{log}"
    );
    assert!(
        log.contains("driver profile bound"),
        "no profile bound:\n{log}"
    );
    assert!(saves.is_dir(), "the profile store was not created");
    let _ = out;
}

/// Run with every OS user-data base pointed at `home`, so the *default*
/// profile root resolves there on any platform.
fn run_with_home(install: &Path, home: &Path, extra: &[&std::ffi::OsStr]) -> (Output, String) {
    let out = Command::new(MM2_EXE)
        .arg("--mm2-path")
        .arg(install)
        .args(["--headless", "--dev-world", "--frames", "5"])
        .args(extra)
        .env_remove("RUST_LOG")
        .env("HOME", home)
        .env("XDG_DATA_HOME", home)
        .env("APPDATA", home)
        .env("USERPROFILE", home)
        .output()
        .expect("spawn mm2");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out, log)
}

#[test]
fn a_default_profile_root_inside_the_install_is_refused_and_nothing_is_written() {
    let root = tempfile::tempdir().unwrap();
    let install = root.path().join("MM2");
    std::fs::create_dir(&install).unwrap();
    let home = install.join("home");
    let (out, log) = run_with_home(&install, &home, &["--new-profile".as_ref(), "Ada".as_ref()]);
    assert_eq!(out.status.code(), Some(2), "not refused:\n{log}");
    assert!(
        log.contains("original installation"),
        "no reason given:\n{log}"
    );
    assert!(
        !home.exists(),
        "the default store was created in the install"
    );
}

#[test]
fn a_default_profile_root_outside_the_install_still_binds() {
    let root = tempfile::tempdir().unwrap();
    let install = root.path().join("MM2");
    std::fs::create_dir(&install).unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let (_, log) = run_with_home(&install, &home, &["--new-profile".as_ref(), "Ada".as_ref()]);
    assert!(
        !log.contains("write destination refused")
            && !log.contains("default profile store refused"),
        "a user location was refused:\n{log}"
    );
    assert!(
        log.contains("driver profile bound"),
        "no profile bound:\n{log}"
    );
}
