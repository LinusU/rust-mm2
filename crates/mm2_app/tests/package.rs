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

fn check(dir: &Path, extra: &[&str]) -> Output {
    Command::new("sh")
        .arg(repo().join("scripts/check-package.sh"))
        .args(extra)
        .arg(dir)
        .output()
        .expect("run sh")
}

/// A folder `check-package.sh` accepts.
fn good_package(at: &Path) -> PathBuf {
    let dir = at.join("pkg");
    std::fs::create_dir_all(dir.join("assets/texture")).unwrap();
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    for bin in BINS {
        write_exec(&dir.join(bin), b"\0fake\0");
    }
    std::fs::write(dir.join("README.md"), "readme").unwrap();
    std::fs::write(dir.join("docs/building.md"), "doc").unwrap();
    std::fs::write(dir.join("assets/texture/dev_road.png"), b"png").unwrap();
    assert_eq!(code(&check(&dir, &[])), 0, "fixture must be clean");
    dir
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

fn package(args: &[&str]) -> Output {
    Command::new("sh")
        .arg(repo().join("scripts/package.sh"))
        .args(args)
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

fn package_args<'a>(bins: &'a Path, out: &'a Path) -> [&'a str; 4] {
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
    let out = package(&package_args(&bins, &out_dir));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("packaged:"));
    let mut expected: Vec<String> = BINS.iter().map(|b| b.to_string()).collect();
    expected.extend(
        [
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
    let out = package(&package_args(&bins, &out_dir));
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
    let mut args = package_args(&bins, &out_dir).to_vec();
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
    let args = package_args(&bins, &out_dir);
    assert_eq!(code(&package(&args)), 0);
    let tarball = tarball_in(&out_dir);
    let before = std::fs::read(&tarball).unwrap();
    let again = package(&args);
    assert_eq!(code(&again), 2, "{}", stderr(&again));
    assert!(stderr(&again).contains("already exists"));
    assert_eq!(std::fs::read(&tarball).unwrap(), before, "overwritten");

    std::fs::remove_file(bins.join("mm2-join")).unwrap();
    let other = tmp.path().join("other");
    let out = package(&package_args(&bins, &other));
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("mm2-join"), "{}", stderr(&out));
    assert!(!other.exists(), "a partial package was left behind");
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
