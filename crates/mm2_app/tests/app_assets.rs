//! F30-AC04: the binary finds its own synthetic assets without help from
//! the working directory. Process level — `mm2_app::app_assets` has the
//! unit coverage of each lookup rule.

use std::process::Command;

const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");

fn run_from(cwd: &std::path::Path) -> String {
    let out = Command::new(MM2_EXE)
        .args(["--headless", "--dev-world", "--frames", "5"])
        .current_dir(cwd)
        .env_remove("RUST_LOG")
        .output()
        .expect("spawn mm2");
    // Five frames cannot satisfy the smoke verdict (the car never lands),
    // so the exit status is not the check; the mount line is.
    // The tracing subscriber writes to stdout; take both streams so a
    // later change of sink does not blind the check.
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        log.contains("smoke=headless-physics"),
        "no smoke record:\n{log}"
    );
    log
}

#[test]
fn the_app_mounts_its_assets_from_an_unrelated_working_directory() {
    let elsewhere = tempfile::tempdir().unwrap();
    let log = run_from(elsewhere.path());
    assert!(
        log.contains("mounted app assets"),
        "assets not mounted from a foreign cwd:\n{log}"
    );
    assert!(
        !log.contains("app assets directory not found"),
        "lookup fell through:\n{log}"
    );
}

#[test]
fn a_working_directory_with_its_own_assets_folder_does_not_shadow_the_binarys() {
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("assets/texture")).unwrap();
    std::fs::write(cwd.path().join("assets/texture/dev_road.png"), b"not a png").unwrap();
    let log = run_from(cwd.path());
    let line = log
        .lines()
        .find(|l| l.contains("mounted app assets"))
        .unwrap_or_else(|| panic!("no mount line:\n{log}"));
    assert!(
        !line.contains(&*cwd.path().to_string_lossy()),
        "mounted the working directory's assets over the binary's: {line}"
    );
}
