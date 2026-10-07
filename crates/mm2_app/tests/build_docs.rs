//! F30-AC03/AC06: the build documentation and release scripts describe the
//! repository as it is. A doc that names a toolchain, a package list or a
//! binary set the build no longer has is worse than none, so each fact
//! `docs/building.md` states about the build is read back from the file
//! that defines it. The scanner script is exercised on real files.

use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// The `name = "…"` of every `[[bin]]` table in a manifest.
fn bin_names(manifest: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_bin = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_bin = line == "[[bin]]";
        } else if in_bin && let Some(rest) = line.strip_prefix("name") {
            let name = rest.trim_start_matches([' ', '=']).trim_matches('"');
            names.push(name.to_string());
        }
    }
    names
}

fn workspace_bins() -> Vec<String> {
    let mut bins = bin_names(&read("crates/mm2_app/Cargo.toml"));
    bins.extend(bin_names(&read("tools/mm2_inspect/Cargo.toml")));
    bins.sort();
    bins
}

#[test]
fn the_doc_names_the_manifests_minimum_rust_version() {
    let manifest = read("Cargo.toml");
    let msrv = manifest
        .lines()
        .find_map(|l| l.trim().strip_prefix("rust-version"))
        .map(|v| {
            v.trim_start_matches([' ', '='])
                .trim_matches('"')
                .to_string()
        })
        .expect("workspace rust-version");
    let doc = read("docs/building.md");
    assert!(
        doc.contains(&format!("Rust **{msrv} or newer**")),
        "docs/building.md must state rust-version {msrv}"
    );
}

#[test]
fn the_doc_lists_every_package_ci_installs_on_linux() {
    let ci = read(".github/workflows/ci.yml");
    let doc = read("docs/building.md");
    let start = ci.find("apt-get install").expect("CI installs packages");
    // The install command continues over lines ending in a backslash.
    let mut command = String::new();
    for line in ci[start..].lines() {
        command.push_str(line.trim_end_matches('\\'));
        command.push(' ');
        if !line.trim_end().ends_with('\\') {
            break;
        }
    }
    let packages: Vec<&str> = command
        .split_whitespace()
        .filter(|w| w.starts_with("lib") || matches!(*w, "g++" | "pkg-config"))
        .collect();
    assert!(packages.len() >= 7, "parsed too few packages: {packages:?}");
    for package in packages {
        assert!(
            doc.contains(package),
            "CI installs {package}, docs/building.md does not list it"
        );
    }
}

#[test]
fn the_doc_and_the_release_script_cover_every_binary() {
    let doc = read("docs/building.md");
    let script = read("scripts/release-build.sh");
    let bins = workspace_bins();
    assert_eq!(bins, ["mm2", "mm2-host", "mm2-inspect", "mm2-join"]);
    for bin in &bins {
        assert!(doc.contains(&format!("`{bin}`")), "doc omits `{bin}`");
    }
    // The script scans exactly the workspace's binaries: each
    // `target/release/<name>${exe}` it names is a real one, and none is
    // missing.
    let mut scanned: Vec<String> = script
        .split("target/release/")
        .skip(1)
        .filter_map(|after| after.split_once("${exe}"))
        .map(|(name, _)| name)
        .filter(|name| !name.contains(char::is_whitespace))
        .map(str::to_string)
        .collect();
    scanned.sort();
    scanned.dedup();
    assert_eq!(scanned, bins, "release-build.sh and the manifests disagree");
}

#[test]
fn the_doc_lists_the_asset_search_order_the_code_uses() {
    let doc = read("docs/building.md");
    let code = read("crates/mm2_app/src/app_assets.rs");
    for rule in [
        "<exe_dir>/assets",
        "<exe_dir>/../Resources/assets",
        "<exe_dir>/../share/rust-mm2/assets",
    ] {
        assert!(
            code.contains(rule),
            "app_assets.rs no longer documents {rule}"
        );
        assert!(
            doc.contains(&rule.replace("exe_dir", "exe dir")),
            "docs/building.md omits {rule}"
        );
    }
}

#[cfg(unix)]
mod scanner {
    use super::*;
    use std::process::{Command, Output};

    fn scan(args: &[&str]) -> Output {
        Command::new("sh")
            .arg(repo().join("scripts/scan-private-paths.sh"))
            .args(args)
            .output()
            .expect("run sh")
    }

    fn code(out: &Output) -> i32 {
        out.status.code().expect("exited")
    }

    #[test]
    fn a_file_carrying_a_forbidden_path_fails_and_is_not_echoed() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("fake.bin");
        // Binary-ish: NULs around the embedded path, as in a real artifact.
        std::fs::write(&bin, b"\0\0/Users/secretname/.cargo/x.rs\0\xff\xfe").unwrap();
        let out = scan(&["--forbid", "/Users/secretname", bin.to_str().unwrap()]);
        assert_eq!(code(&out), 1);
        let report = String::from_utf8_lossy(&out.stderr);
        assert!(report.contains("FOUND 1x"), "{report}");
        assert!(
            !String::from_utf8_lossy(&out.stdout).contains("secretname")
                && !report.contains("secretname"),
            "the report leaked the path it found"
        );
    }

    #[test]
    fn a_clean_file_passes() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("clean.bin");
        std::fs::write(&bin, b"\0/rust-mm2/crates/x.rs\0/cargo/registry/y.rs\0").unwrap();
        let out = scan(&["--forbid", "/Users/secretname", bin.to_str().unwrap()]);
        assert_eq!(code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn every_file_is_checked_not_just_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let clean = dir.path().join("a");
        let dirty = dir.path().join("b");
        std::fs::write(&clean, b"nothing here").unwrap();
        std::fs::write(&dirty, b"/Users/secretname").unwrap();
        let out = scan(&[
            "--forbid",
            "/Users/secretname",
            clean.to_str().unwrap(),
            dirty.to_str().unwrap(),
        ]);
        assert_eq!(code(&out), 1);
        assert!(String::from_utf8_lossy(&out.stderr).contains(dirty.to_str().unwrap()));
    }

    #[test]
    fn a_path_with_spaces_is_scanned_as_one_file() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("with space.bin");
        std::fs::write(&bin, b"/Users/secretname").unwrap();
        let out = scan(&["--forbid", "/Users/secretname", bin.to_str().unwrap()]);
        assert_eq!(code(&out), 1, "{}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn usage_errors_and_missing_files_are_not_a_pass() {
        assert_eq!(code(&scan(&[])), 2, "no files");
        assert_eq!(
            code(&scan(&["--forbid", "/Users/secretname"])),
            2,
            "no files"
        );
        assert_eq!(code(&scan(&["--bogus", "x"])), 2, "unknown option");
        assert_eq!(code(&scan(&["--forbid"])), 2, "missing value");
        let out = scan(&["--forbid", "/Users/secretname", "/no/such/file"]);
        assert_eq!(code(&out), 2, "an unreadable file must not read as clean");
        // A forbidden list with nothing usable in it must not pass either:
        // "/" would match every binary, so it is ignored — and then nothing
        // was checked.
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("x");
        std::fs::write(&bin, b"/anything").unwrap();
        assert_eq!(code(&scan(&["--forbid", "/", bin.to_str().unwrap()])), 2);
    }

    #[test]
    fn by_default_it_forbids_this_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("x");
        std::fs::write(&bin, repo().to_str().unwrap()).unwrap();
        let out = scan(&[bin.to_str().unwrap()]);
        assert_eq!(code(&out), 1, "{}", String::from_utf8_lossy(&out.stderr));
    }
}
