//! `mm2-inspect resolve` / `conflicts` explain override provenance on a
//! synthetic install and two synthetic mods (no original content).

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn write(dir: &Path, rel: &str, contents: &[u8]) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, contents).unwrap();
}

fn mod_dir(mods: &Path, id: &str, files: &[&str]) {
    let m = mods.join(id);
    write(&m, "mod.toml", format!("[mod]\nid = \"{id}\"\n").as_bytes());
    for rel in files {
        write(&m, rel, id.as_bytes());
    }
}

fn inspect(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mm2-inspect"))
        .args(args)
        .output()
        .unwrap()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    install: String,
    mods: String,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let (install, mods) = (tmp.path().join("install"), tmp.path().join("mods"));
    write(&install, "texture/shared.png", b"install");
    write(&install, "tune/only_install.txt", b"install");
    mod_dir(&mods, "alpha", &["texture/shared.png", "tune/a.txt"]);
    mod_dir(&mods, "beta", &["texture/shared.png"]);
    Fixture {
        install: install.to_str().unwrap().into(),
        mods: mods.to_str().unwrap().into(),
        _tmp: tmp,
    }
}

#[test]
fn resolve_names_the_winner_and_every_shadowed_source() {
    let f = fixture();
    let out = inspect(&[
        "--mods",
        &f.mods,
        "resolve",
        &f.install,
        "texture/shared.png",
    ]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    let winner = text.lines().find(|l| l.starts_with("winner")).unwrap();
    assert!(winner.contains("mod `beta`"), "{text}");
    let shadowed: Vec<_> = text.lines().filter(|l| l.starts_with("shadowed")).collect();
    assert_eq!(shadowed.len(), 2, "{text}");
    assert!(shadowed[0].contains("mod `alpha`"), "{text}");
    assert!(shadowed[1].contains("directory"), "{text}");
    assert!(text.contains("reason  : higher priority tier"), "{text}");

    // Without the mods the install is the only provider.
    let out = inspect(&["resolve", &f.install, "texture/shared.png"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("only source providing the path"), "{text}");
}

#[test]
fn conflicts_lists_overrides_and_strict_fails_on_mod_conflicts() {
    let f = fixture();
    let out = inspect(&["--mods", &f.mods, "conflicts", &f.install]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("logical : texture/shared.png"), "{text}");
    assert!(
        !text.contains("logical : tune/a.txt"),
        "a path with one provider is not a conflict: {text}"
    );
    assert!(
        text.contains("CONFLICT beta over alpha: 1 path(s)"),
        "{text}"
    );
    assert!(
        text.contains("override beta over install: 1 path(s)"),
        "{text}"
    );
    assert!(
        text.contains("1 conflicting path(s), 1 between mods"),
        "{text}"
    );

    let out = inspect(&["--mods", &f.mods, "conflicts", "--strict", &f.install]);
    assert!(!out.status.success(), "strict must fail on a mod conflict");

    // The prefix filter narrows the listing, not the totals.
    let out = inspect(&[
        "--mods",
        &f.mods,
        "conflicts",
        "--prefix",
        "tune/",
        &f.install,
    ]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(!text.contains("logical :"), "{text}");
    assert!(text.contains("(0 shown)"), "{text}");
}

#[test]
fn conflicts_without_mods_passes_strict() {
    let f = fixture();
    let out = inspect(&["conflicts", "--strict", &f.install]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("0 conflicting path(s), 0 between mods"),
        "{text}"
    );
}

#[test]
fn clean_mods_that_only_override_the_install_pass_strict() {
    let tmp = tempfile::tempdir().unwrap();
    let (install, mods) = (tmp.path().join("install"), tmp.path().join("mods"));
    write(&install, "texture/shared.png", b"install");
    // Two mods touching disjoint paths; one replaces original content.
    mod_dir(&mods, "alpha", &["texture/shared.png"]);
    mod_dir(&mods, "beta", &["tune/b.txt"]);
    let (install, mods) = (install.to_str().unwrap(), mods.to_str().unwrap());

    let out = inspect(&["--mods", mods, "conflicts", "--strict", install]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("override alpha over install: 1 path(s)"),
        "{text}"
    );
    assert!(
        text.contains("1 conflicting path(s), 0 between mods"),
        "{text}"
    );
}
