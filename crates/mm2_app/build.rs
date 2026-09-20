//! Embed the engine commit into `mm2` so smoke reports are versioned by
//! the exact code that produced them (same mechanism as `mm2-inspect`;
//! F00 requirement: keep coverage reports versioned by engine commit).

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    // Re-run when HEAD moves or the branch ref advances. `git rev-parse
    // --git-path` resolves correctly inside worktrees, where `.git` is a
    // file rather than a directory.
    if let Some(head) = git(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head}");
    }
    if let Some(symbolic) = git(&["symbolic-ref", "-q", "HEAD"])
        && let Some(r) = git(&["rev-parse", "--git-path", &symbolic])
    {
        println!("cargo:rerun-if-changed={r}");
    }
    if let Some(common) = git(&["rev-parse", "--git-common-dir"]) {
        println!("cargo:rerun-if-changed={common}/packed-refs");
    }

    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
    println!(
        "cargo:rustc-env=MM2_BUILD_COMMIT={}{}",
        commit,
        if dirty { "-dirty" } else { "" }
    );
}
