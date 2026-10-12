//! F30-AC04/AC05/AC06: the packaging step and what it lets ship.
//! `scripts/package.sh` assembles the distributable folder and tarball;
//! `scripts/check-package.sh` judges a folder against an allowlist. Both
//! run here on synthetic trees (fake binaries — nothing is built), and the
//! three relocated layouts a package uses are launched for real with the
//! `mm2` this test run built.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BINS: [&str; 4] = ["mm2", "mm2-host", "mm2-join", "mm2-inspect"];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("exited")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn write_exec(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A directory of four fake binaries.
fn fake_bins(at: &Path) -> PathBuf {
    let dir = at.join("bins");
    std::fs::create_dir_all(&dir).unwrap();
    for bin in BINS {
        write_exec(&dir.join(bin), format!("\0fake {bin}\0").as_bytes());
    }
    dir
}

/// The same, with the `.exe` names a Windows build produces.
fn fake_exe_bins(at: &Path) -> PathBuf {
    let dir = at.join("bins");
    std::fs::create_dir_all(&dir).unwrap();
    for bin in BINS {
        std::fs::write(
            dir.join(format!("{bin}.exe")),
            format!("\0fake {bin}\0").as_bytes(),
        )
        .unwrap();
    }
    dir
}

/// A directory holding a stub `uname` that reports this machine as
/// Windows (MSYS), so `scripts/package.sh` takes its Windows branch:
/// `*.exe` binaries and a `.zip`. Prepended to `PATH`, it shadows the real
/// `uname` for the script under test.
fn windows_uname_dir(at: &Path) -> PathBuf {
    let dir = at.join("stub-bin");
    std::fs::create_dir_all(&dir).unwrap();
    write_exec(
        &dir.join("uname"),
        b"#!/bin/sh\ncase \"$1\" in -m) echo x86_64 ;; *) echo MINGW64_NT-10.0 ;; esac\n",
    );
    dir
}

fn check(dir: &Path, extra: &[&str]) -> Output {
    check_layout(dir, "flat", extra)
}

fn check_layout(dir: &Path, layout: &str, extra: &[&str]) -> Output {
    Command::new("sh")
        .arg(repo().join("scripts/check-package.sh"))
        .args(["--layout", layout])
        .args(extra)
        .arg(dir)
        .output()
        .expect("run sh")
}

/// Where a layout keeps its binaries, assets and README/docs.
fn layout_dirs(layout: &str) -> (&'static str, &'static str, &'static str) {
    match layout {
        "app" => (
            "Contents/MacOS/",
            "Contents/Resources/assets",
            "Contents/Resources",
        ),
        "prefix" => ("bin/", "share/rust-mm2/assets", "share/doc/rust-mm2"),
        _ => ("", "assets", ""),
    }
}

/// A folder `check-package.sh` accepts, in `layout`.
fn good_layout_package(at: &Path, layout: &str) -> PathBuf {
    let (bin_prefix, asset_dir, doc_prefix) = layout_dirs(layout);
    let dir = at.join("pkg");
    let docs = if doc_prefix.is_empty() {
        dir.join("docs")
    } else {
        dir.join(doc_prefix).join("docs")
    };
    std::fs::create_dir_all(dir.join(asset_dir).join("texture")).unwrap();
    std::fs::create_dir_all(&docs).unwrap();
    for bin in BINS {
        let path = dir.join(format!("{bin_prefix}{bin}"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_exec(&path, b"\0fake\0");
    }
    let readme = if doc_prefix.is_empty() {
        dir.join("README.md")
    } else {
        dir.join(doc_prefix).join("README.md")
    };
    std::fs::write(readme, "readme").unwrap();
    std::fs::write(docs.join("building.md"), "doc").unwrap();
    std::fs::write(dir.join(asset_dir).join("texture/dev_road.png"), b"png").unwrap();
    assert_eq!(
        code(&check_layout(&dir, layout, &[])),
        0,
        "fixture must be clean"
    );
    dir
}

/// The flat fixture, kept for the tests that predate the layouts.
fn good_package(at: &Path) -> PathBuf {
    good_layout_package(at, "flat")
}

/// The fixture plus one extra file, which must fail naming `needle`.
fn assert_rejected_with(rel: &str, bytes: &[u8], needle: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = good_package(tmp.path());
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    let out = check(&dir, &[]);
    assert_eq!(code(&out), 1, "{rel}: {}", stderr(&out));
    assert!(stderr(&out).contains(needle), "{rel}: {}", stderr(&out));
    assert!(
        stderr(&out).contains(rel),
        "{rel} not named: {}",
        stderr(&out)
    );
}

/// The same, in `layout`: `rel` is the file's real path in that layout.
fn assert_layout_rejected_with(layout: &str, rel: &str, bytes: &[u8], needle: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = good_layout_package(tmp.path(), layout);
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    let out = check_layout(&dir, layout, &[]);
    assert_eq!(code(&out), 1, "{rel}: {}", stderr(&out));
    assert!(stderr(&out).contains(needle), "{rel}: {}", stderr(&out));
    assert!(
        stderr(&out).contains(rel),
        "{rel} not named: {}",
        stderr(&out)
    );
}

#[test]
fn a_clean_package_passes_and_counts_its_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = good_package(tmp.path());
    let out = check(&dir, &[]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("package clean: 7 files"));
}

#[test]
fn original_content_saves_captures_and_credentials_are_each_named() {
    assert_rejected_with("mm2core.ar", b"DAVE", "original game archive");
    assert_rejected_with("assets/mm2tex.ar", b"DAVE", "original game archive");
    assert_rejected_with("driver-1/profile.json", b"{}", "profile or settings");
    assert_rejected_with("settings.json", b"{}", "profile or settings");
    assert_rejected_with("perf.csv", b"frame,ms", "perf capture");
    assert_rejected_with("run.report.json", b"{}", "perf capture");
    assert_rejected_with("game.log", b"x", "log file");
    assert_rejected_with(".env", b"TOKEN=1", "credential");
    assert_rejected_with("deploy.pem", b"KEY", "credential");
}

#[test]
fn anything_off_the_allowlist_fails_even_if_nobody_named_it() {
    assert_rejected_with("screenshot.png", b"png", "not on the allowlist");
    assert_rejected_with("notes.txt", b"x", "not on the allowlist");
    assert_rejected_with("docs/nested/deep.md", b"x", "not on the allowlist");
    assert_rejected_with("docs/page.html", b"x", "not on the allowlist");
    assert_rejected_with(
        "assets/texture/car.dds",
        b"x",
        "asset type not on the allowlist",
    );
    assert_rejected_with(
        "assets/scripts/run.sh",
        b"x",
        "asset type not on the allowlist",
    );
}

#[test]
fn an_oversize_asset_is_a_problem_but_a_normal_one_is_not() {
    assert_rejected_with(
        "assets/texture/big.png",
        &vec![0u8; 8 * 1024 * 1024],
        "asset over 8 MiB",
    );
    let tmp = tempfile::tempdir().unwrap();
    let dir = good_package(tmp.path());
    std::fs::write(
        dir.join("assets/texture/ok.png"),
        vec![0u8; 8 * 1024 * 1024 - 1],
    )
    .unwrap();
    assert_eq!(code(&check(&dir, &[])), 0);
}

#[test]
fn a_symlink_fails_because_it_can_point_out_of_the_package() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = good_package(tmp.path());
    std::os::unix::fs::symlink("/etc/hosts", dir.join("assets/hosts.txt")).unwrap();
    let out = check(&dir, &[]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("symlink or special file: assets/hosts.txt"));
}

#[test]
fn a_missing_binary_asset_or_readme_fails() {
    for (gone, needle) in [
        ("mm2-host", "missing binary: mm2-host"),
        ("mm2-inspect", "missing binary: mm2-inspect"),
        ("README.md", "missing README.md"),
        (
            "assets/texture/dev_road.png",
            "missing assets/texture/dev_road.png",
        ),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let dir = good_package(tmp.path());
        std::fs::remove_file(dir.join(gone)).unwrap();
        let out = check(&dir, &[]);
        assert_eq!(code(&out), 1, "{gone}: {}", stderr(&out));
        assert!(stderr(&out).contains(needle), "{gone}: {}", stderr(&out));
    }
}

#[test]
fn a_binary_that_cannot_run_or_mixes_flavours_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = good_package(tmp.path());
    std::fs::set_permissions(dir.join("mm2"), std::fs::Permissions::from_mode(0o644)).unwrap();
    let out = check(&dir, &[]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("binary is not executable: mm2"));

    let tmp = tempfile::tempdir().unwrap();
    let dir = good_package(tmp.path());
    std::fs::write(dir.join("mm2.exe"), b"MZ").unwrap();
    let out = check(&dir, &[]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("both flavours"), "{}", stderr(&out));

    // An all-`.exe` set is a valid (Windows) package; no exec bit needed.
    let tmp = tempfile::tempdir().unwrap();
    let dir = good_package(tmp.path());
    for bin in BINS {
        std::fs::rename(dir.join(bin), dir.join(format!("{bin}.exe"))).unwrap();
    }
    let out = check(&dir, &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

#[test]
fn every_problem_is_listed_not_just_the_first() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = good_package(tmp.path());
    std::fs::write(dir.join("mm2core.ar"), b"x").unwrap();
    std::fs::write(dir.join("game.log"), b"x").unwrap();
    std::fs::remove_file(dir.join("mm2-join")).unwrap();
    let out = check(&dir, &[]);
    let report = stderr(&out);
    assert_eq!(code(&out), 1);
    for needle in ["mm2core.ar", "game.log", "mm2-join", "3 problem(s)"] {
        assert!(report.contains(needle), "{needle} missing:\n{report}");
    }
}

#[test]
fn a_private_path_in_any_shipped_file_fails_without_being_echoed() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = good_package(tmp.path());
    std::fs::write(dir.join("docs/building.md"), "see /Users/secretname/x").unwrap();
    let out = check(&dir, &["--forbid", "/Users/secretname"]);
    let report = stderr(&out);
    assert_eq!(code(&out), 1, "{report}");
    assert!(report.contains("private path"), "{report}");
    assert!(
        !report.contains("secretname"),
        "the report leaked: {report}"
    );
    // Without the file's offence the same call is clean.
    std::fs::write(dir.join("docs/building.md"), "fine").unwrap();
    assert_eq!(code(&check(&dir, &["--forbid", "/Users/secretname"])), 0);
}

#[test]
fn usage_errors_are_not_a_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let script = repo().join("scripts/check-package.sh");
    let run = |args: &[&str]| code(&Command::new("sh").arg(&script).args(args).output().unwrap());
    assert_eq!(run(&[]), 2, "no directory");
    assert_eq!(run(&["/no/such/dir"]), 2, "missing directory");
    assert_eq!(run(&["--bogus", tmp.path().to_str().unwrap()]), 2);
    assert_eq!(run(&["--forbid"]), 2, "missing value");
    // An empty folder is a failure of content, not a usage error.
    assert_eq!(run(&[tmp.path().to_str().unwrap()]), 1, "empty package");
}

#[test]
fn each_layout_is_checked_where_its_files_belong() {
    for layout in ["flat", "app", "prefix"] {
        let tmp = tempfile::tempdir().unwrap();
        let dir = good_layout_package(tmp.path(), layout);
        assert_eq!(
            code(&check_layout(&dir, layout, &[])),
            0,
            "{layout} fixture must be clean"
        );
        // The same folder judged as another layout is not that layout:
        // its binaries, assets and README are somewhere else.
        for other in ["flat", "app", "prefix"] {
            if other == layout {
                continue;
            }
            let out = check_layout(&dir, other, &[]);
            assert_eq!(code(&out), 1, "{layout} as {other}: {}", stderr(&out));
            assert!(
                stderr(&out).contains("missing"),
                "{layout} as {other} should miss files: {}",
                stderr(&out)
            );
        }
    }
}

#[test]
fn an_unknown_layout_is_a_usage_error() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = good_package(tmp.path());
    assert_eq!(code(&check_layout(&dir, "bogus", &[])), 2);
    assert_eq!(
        code(&check_layout(&dir, "flat", &["--layout"])),
        2,
        "no value"
    );
}

#[test]
fn a_stray_or_wrong_type_file_is_named_by_its_real_path_in_a_layout() {
    // A file the bundle was never meant to carry fails whatever its depth,
    // and the message says where it really is.
    assert_layout_rejected_with(
        "app",
        "Contents/Info.plist",
        b"plist",
        "not on the allowlist",
    );
    assert_layout_rejected_with(
        "app",
        "Contents/Resources/assets/texture/car.dds",
        b"x",
        "asset type not on the allowlist",
    );
    assert_layout_rejected_with(
        "prefix",
        "share/doc/rust-mm2/notes.txt",
        b"x",
        "not on the allowlist",
    );
    assert_layout_rejected_with(
        "prefix",
        "share/rust-mm2/assets/car.dds",
        b"x",
        "asset type not on the allowlist",
    );
}

#[test]
fn a_layout_missing_one_of_its_own_places_fails() {
    for (layout, gone, needle) in [
        ("app", "Contents/MacOS/mm2", "missing binary: mm2"),
        (
            "app",
            "Contents/Resources/assets/texture/dev_road.png",
            "missing Contents/Resources/assets/texture/dev_road.png",
        ),
        ("app", "Contents/Resources/README.md", "missing README.md"),
        ("prefix", "bin/mm2-join", "missing binary: mm2-join"),
        (
            "prefix",
            "share/rust-mm2/assets/texture/dev_road.png",
            "missing share/rust-mm2/assets/texture/dev_road.png",
        ),
        (
            "prefix",
            "share/doc/rust-mm2/README.md",
            "missing README.md",
        ),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let dir = good_layout_package(tmp.path(), layout);
        std::fs::remove_file(dir.join(gone)).unwrap();
        let out = check_layout(&dir, layout, &[]);
        assert_eq!(code(&out), 1, "{layout} {gone}: {}", stderr(&out));
        assert!(
            stderr(&out).contains(needle),
            "{layout} {gone}: {}",
            stderr(&out)
        );
    }
}

fn package(args: &[&str]) -> Output {
    Command::new("sh")
        .arg(repo().join("scripts/package.sh"))
        .args(args)
        .output()
        .expect("run sh")
}

/// `scripts/package.sh` under the stub `uname` in `uname_dir`, so the
/// script sees a Windows host.
fn package_as_windows(args: &[&str], uname_dir: &Path) -> Output {
    let path = format!(
        "{}:{}",
        uname_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    Command::new("sh")
        .arg(repo().join("scripts/package.sh"))
        .args(args)
        .env("PATH", path)
        .output()
        .expect("run sh")
}

fn tar_listing(tarball: &Path) -> Vec<String> {
    let out = Command::new("tar")
        .arg("-tzf")
        .arg(tarball)
        .output()
        .unwrap();
    assert!(out.status.success());
    let mut names: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.ends_with('/'))
        .map(|l| l.split_once('/').map_or(l, |(_, rest)| rest).to_string())
        .collect();
    names.sort();
    names
}

fn tarball_in(out: &Path) -> PathBuf {
    let found: Vec<PathBuf> = std::fs::read_dir(out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".tar.gz"))
        .collect();
    assert_eq!(found.len(), 1, "{found:?}");
    found.into_iter().next().unwrap()
}

/// Extract `tarball` into `into` and return its one top-level entry.
fn unpack(tarball: &Path, into: &Path) -> PathBuf {
    std::fs::create_dir_all(into).unwrap();
    let status = Command::new("tar")
        .arg("-C")
        .arg(into)
        .arg("-xzf")
        .arg(tarball)
        .status()
        .unwrap();
    assert!(status.success());
    let entries: Vec<PathBuf> = std::fs::read_dir(into)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
    entries.into_iter().next().unwrap()
}

/// The files a zip holds, as flat names under its single top-level entry.
fn zip_listing(zip: &Path) -> Vec<String> {
    let out = Command::new("unzip")
        .arg("-Z1")
        .arg(zip)
        .output()
        .expect("run unzip");
    assert!(
        out.status.success(),
        "unzip -Z1 failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut names: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.ends_with('/'))
        .map(|l| l.split_once('/').map_or(l, |(_, rest)| rest).to_string())
        .collect();
    names.sort();
    names
}

/// Extract `zip` into `into` and return its one top-level entry.
fn unpack_zip(zip: &Path, into: &Path) -> PathBuf {
    std::fs::create_dir_all(into).unwrap();
    let status = Command::new("unzip")
        .arg("-q")
        .arg(zip)
        .arg("-d")
        .arg(into)
        .status()
        .unwrap();
    assert!(status.success());
    let entries: Vec<PathBuf> = std::fs::read_dir(into)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
    entries.into_iter().next().unwrap()
}

/// The one directory a `--layout`-specific run staged in `out`.
fn only_dir(out: &Path) -> PathBuf {
    let dirs: Vec<PathBuf> = std::fs::read_dir(out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs.into_iter().next().unwrap()
}

fn package_args<'a>(layout: &'a str, bins: &'a Path, out: &'a Path) -> [&'a str; 6] {
    [
        "--layout",
        layout,
        "--bin-dir",
        bins.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]
}

/// The default invocation, without a `--layout` flag (which means `all`).
fn package_args_default<'a>(bins: &'a Path, out: &'a Path) -> [&'a str; 4] {
    [
        "--bin-dir",
        bins.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]
}

#[test]
fn the_package_script_ships_the_program_assets_and_docs_and_nothing_else() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = fake_bins(tmp.path());
    let out_dir = tmp.path().join("out");
    let out = package(&package_args("flat", &bins, &out_dir));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("packaged:"));
    let mut expected: Vec<String> = BINS.iter().map(|b| b.to_string()).collect();
    expected.extend(
        [
            "LICENSE-APACHE",
            "LICENSE-MIT",
            "README.md",
            "assets/texture/dev_road.png",
            "docs/building.md",
            "docs/modding.md",
        ]
        .map(String::from),
    );
    expected.sort();
    assert_eq!(tar_listing(&tarball_in(&out_dir)), expected);
    // The shipped assets are the repository's own, byte for byte.
    let staged = std::fs::read_dir(&out_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    assert_eq!(
        std::fs::read(staged.join("assets/texture/dev_road.png")).unwrap(),
        std::fs::read(repo().join("assets/texture/dev_road.png")).unwrap()
    );
}

#[test]
fn the_tarball_unpacks_to_a_package_the_check_accepts() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = fake_bins(tmp.path());
    let out_dir = tmp.path().join("out");
    let out = package(&package_args("flat", &bins, &out_dir));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let unpack = tmp.path().join("unpack");
    std::fs::create_dir(&unpack).unwrap();
    let status = Command::new("tar")
        .arg("-C")
        .arg(&unpack)
        .arg("-xzf")
        .arg(tarball_in(&out_dir))
        .status()
        .unwrap();
    assert!(status.success());
    let root = std::fs::read_dir(&unpack)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(code(&check(&root, &[])), 0);
}

#[test]
fn a_binary_with_the_builders_path_stops_the_package_before_any_tarball() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = fake_bins(tmp.path());
    write_exec(&bins.join("mm2"), b"\0/Users/secretname/.cargo/x.rs\0");
    let out_dir = tmp.path().join("out");
    let mut args = package_args("flat", &bins, &out_dir).to_vec();
    args.extend(["--forbid", "/Users/secretname"]);
    let out = package(&args);
    assert_ne!(code(&out), 0);
    assert!(stderr(&out).contains("private path"), "{}", stderr(&out));
    assert!(
        std::fs::read_dir(&out_dir).unwrap().all(|e| !e
            .unwrap()
            .path()
            .to_string_lossy()
            .ends_with(".tar.gz")),
        "a tarball was written for a failing package"
    );
}

#[test]
fn the_package_script_refuses_to_overwrite_and_to_run_without_all_binaries() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = fake_bins(tmp.path());
    let out_dir = tmp.path().join("out");
    let args = package_args("flat", &bins, &out_dir);
    assert_eq!(code(&package(&args)), 0);
    let tarball = tarball_in(&out_dir);
    let before = std::fs::read(&tarball).unwrap();
    let again = package(&args);
    assert_eq!(code(&again), 2, "{}", stderr(&again));
    assert!(stderr(&again).contains("already exists"));
    assert_eq!(std::fs::read(&tarball).unwrap(), before, "overwritten");

    std::fs::remove_file(bins.join("mm2-join")).unwrap();
    let other = tmp.path().join("other");
    let out = package(&package_args("flat", &bins, &other));
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("mm2-join"), "{}", stderr(&out));
    assert!(!other.exists(), "a partial package was left behind");
}

#[test]
fn the_prefix_install_is_assembled_checked_and_archived() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = fake_bins(tmp.path());
    let out_dir = tmp.path().join("out");
    let out = package(&package_args("prefix", &bins, &out_dir));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("packaged:"));

    let prefix = only_dir(&out_dir);
    for rel in [
        "bin/mm2",
        "bin/mm2-host",
        "bin/mm2-join",
        "bin/mm2-inspect",
        "share/rust-mm2/assets/texture/dev_road.png",
        "share/doc/rust-mm2/README.md",
        "share/doc/rust-mm2/LICENSE-MIT",
        "share/doc/rust-mm2/LICENSE-APACHE",
        "share/doc/rust-mm2/docs/building.md",
        "share/doc/rust-mm2/docs/modding.md",
    ] {
        assert!(prefix.join(rel).is_file(), "missing {rel}");
    }
    assert_eq!(code(&check_layout(&prefix, "prefix", &[])), 0);
    // The asset is the repository's own, byte for byte.
    assert_eq!(
        std::fs::read(prefix.join("share/rust-mm2/assets/texture/dev_road.png")).unwrap(),
        std::fs::read(repo().join("assets/texture/dev_road.png")).unwrap()
    );
    // The tarball unpacks to the same tree and is checked in place.
    let root = unpack(&tarball_in(&out_dir), &tmp.path().join("unpack"));
    assert_eq!(root.file_name().unwrap(), prefix.file_name().unwrap());
    assert_eq!(code(&check_layout(&root, "prefix", &[])), 0);
}

/// A Windows run: the flat layout is a `.zip` named for Windows, it holds
/// the four `.exe` binaries and both licenses, and the zip's own contents
/// are checked after unzipping — the same path a real Windows host takes
/// (`uname -s` = `MINGW64_NT-10.0`).
#[test]
fn a_windows_package_is_a_zip_of_dot_exe_binaries() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = fake_exe_bins(tmp.path());
    let uname = windows_uname_dir(tmp.path());
    let out_dir = tmp.path().join("out");
    let args = package_args("flat", &bins, &out_dir);
    let out = package_as_windows(&args, &uname);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("packaged:"));

    // The archive is a zip named for Windows, and no tarball is written.
    let zip = out_dir.join("rust-mm2-windows-x86_64.zip");
    assert!(zip.is_file(), "no Windows zip in {out_dir:?}");
    let names: Vec<String> = std::fs::read_dir(&out_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !names.iter().any(|n| n.ends_with(".tar.gz")),
        "a tarball was written for a Windows package: {names:?}"
    );

    // All four `.exe` binaries, both licenses and the usual content.
    let mut expected: Vec<String> = BINS.iter().map(|b| format!("{b}.exe")).collect();
    expected.extend(
        [
            "LICENSE-APACHE",
            "LICENSE-MIT",
            "README.md",
            "assets/texture/dev_road.png",
            "docs/building.md",
            "docs/modding.md",
        ]
        .map(String::from),
    );
    expected.sort();
    assert_eq!(zip_listing(&zip), expected);

    // The zip unpacks to a flat `.exe` package the check accepts — that
    // is the Windows flavour the allowlist has to judge, not the Unix one.
    let root = unpack_zip(&zip, &tmp.path().join("unpack"));
    assert_eq!(root.file_name().unwrap(), "rust-mm2-windows-x86_64");
    assert_eq!(
        code(&check(&root, &[])),
        0,
        "{}",
        stderr(&check(&root, &[]))
    );
    assert!(root.join("mm2.exe").is_file());

    // Nothing is overwritten on a second run.
    let again = package_as_windows(&args, &uname);
    assert_eq!(code(&again), 2, "{}", stderr(&again));
    assert!(
        stderr(&again).contains("already exists"),
        "{}",
        stderr(&again)
    );
}

/// The default (`--layout all`) Windows run ships the flat and prefix
/// layouts, both as zips.
#[test]
fn a_windows_default_run_assembles_only_zip_archives() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = fake_exe_bins(tmp.path());
    let uname = windows_uname_dir(tmp.path());
    let out_dir = tmp.path().join("out");
    let out = package_as_windows(&package_args_default(&bins, &out_dir), &uname);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let mut names: Vec<String> = std::fs::read_dir(&out_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "rust-mm2-windows-x86_64",
            "rust-mm2-windows-x86_64-prefix",
            "rust-mm2-windows-x86_64-prefix.zip",
            "rust-mm2-windows-x86_64.zip",
        ]
    );
}

#[cfg(target_os = "macos")]
#[test]
fn the_app_bundle_is_assembled_checked_and_archived() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = fake_bins(tmp.path());
    let out_dir = tmp.path().join("out");
    let out = package(&package_args("app", &bins, &out_dir));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("packaged:"));

    let app = out_dir.join("rust-mm2.app");
    for rel in [
        "Contents/MacOS/mm2",
        "Contents/MacOS/mm2-host",
        "Contents/MacOS/mm2-join",
        "Contents/MacOS/mm2-inspect",
        "Contents/Resources/assets/texture/dev_road.png",
        "Contents/Resources/README.md",
        "Contents/Resources/LICENSE-MIT",
        "Contents/Resources/LICENSE-APACHE",
        "Contents/Resources/docs/building.md",
        "Contents/Resources/docs/modding.md",
    ] {
        assert!(app.join(rel).is_file(), "missing {rel}");
    }
    assert_eq!(code(&check_layout(&app, "app", &[])), 0);
    // The packaged asset is the repository's own, byte for byte. These
    // binaries are fakes, so the launch half of AC04 — the app's own asset
    // search finding this layout — is the real-binary test below.
    assert_eq!(
        std::fs::read(app.join("Contents/Resources/assets/texture/dev_road.png")).unwrap(),
        std::fs::read(repo().join("assets/texture/dev_road.png")).unwrap()
    );
    let tarball = out_dir.join("rust-mm2.app.tar.gz");
    assert!(tarball.is_file(), "no bundle archive");
    let root = unpack(&tarball, &tmp.path().join("unpack"));
    assert_eq!(root.file_name().unwrap(), "rust-mm2.app");
    assert_eq!(code(&check_layout(&root, "app", &[])), 0);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn the_app_layout_is_refused_off_macos() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = fake_bins(tmp.path());
    let out_dir = tmp.path().join("out");
    let out = package(&package_args("app", &bins, &out_dir));
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(stderr(&out).contains("macOS-only"), "{}", stderr(&out));
}

#[test]
fn the_default_layout_assembles_every_layout_this_platform_ships() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = fake_bins(tmp.path());
    let out_dir = tmp.path().join("out");
    let out = package(&package_args_default(&bins, &out_dir));
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let names: Vec<String> = std::fs::read_dir(&out_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let count = |matches: &dyn Fn(&str) -> bool| names.iter().filter(|n| matches(n)).count();
    let is_app = |n: &str| n == "rust-mm2.app" || n == "rust-mm2.app.tar.gz";
    let macos = usize::from(cfg!(target_os = "macos"));
    assert_eq!(count(&is_app), 2 * macos, "bundle: {names:?}");
    let prefix = |n: &str| n.ends_with("-prefix") || n.ends_with("-prefix.tar.gz");
    assert_eq!(count(&prefix), 2, "prefix: {names:?}");
    // Plus the flat folder and tarball, which carry neither marker.
    assert_eq!(names.len(), 4 + 2 * macos, "{names:?}");
}

// ---- the relocated layouts, launched for real ----

/// Run the freshly built `mm2` from `exe` (a copy placed in a package
/// layout) in an unrelated working directory and return the mount line.
fn mount_line_of(exe: &Path) -> String {
    let cwd = tempfile::tempdir().unwrap();
    let out = Command::new(exe)
        .args(["--headless", "--dev-world", "--frames", "5"])
        .current_dir(cwd.path())
        .env_remove("RUST_LOG")
        .output()
        .expect("spawn the relocated mm2");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    log.lines()
        .find(|l| l.contains("mounted app assets"))
        .unwrap_or_else(|| panic!("no mount line:\n{log}"))
        .to_string()
}

fn copy_assets(to: &Path) {
    let texture = to.join("texture");
    std::fs::create_dir_all(&texture).unwrap();
    std::fs::copy(
        repo().join("assets/texture/dev_road.png"),
        texture.join("dev_road.png"),
    )
    .unwrap();
}

#[test]
fn a_flat_portable_folder_finds_the_assets_beside_the_binary() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let exe = root.join("mm2");
    std::fs::copy(env!("CARGO_BIN_EXE_mm2"), &exe).unwrap();
    copy_assets(&root.join("assets"));
    let line = mount_line_of(&exe);
    assert!(
        line.contains(root.join("assets").to_str().unwrap()),
        "not the package's assets: {line}"
    );
}

#[test]
fn a_macos_app_bundle_finds_the_assets_in_resources() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let macos = root.join("Midtown.app/Contents/MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_mm2"), macos.join("mm2")).unwrap();
    copy_assets(&root.join("Midtown.app/Contents/Resources/assets"));
    let line = mount_line_of(&macos.join("mm2"));
    assert!(
        line.contains("Midtown.app/Contents/MacOS/../Resources/assets"),
        "not the bundle's Resources: {line}"
    );
}

#[test]
fn a_unix_prefix_install_finds_the_assets_under_share() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let bin = root.join("prefix/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_mm2"), bin.join("mm2")).unwrap();
    copy_assets(&root.join("prefix/share/rust-mm2/assets"));
    let line = mount_line_of(&bin.join("mm2"));
    assert!(
        line.contains("prefix/bin/../share/rust-mm2/assets"),
        "not the prefix's share dir: {line}"
    );
}
