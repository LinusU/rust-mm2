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

#[test]
fn a_duplicate_mod_id_fails_the_inspector_naming_both_directories() {
    let tmp = tempfile::tempdir().unwrap();
    let (install, mods) = (tmp.path().join("install"), tmp.path().join("mods"));
    write(&install, "texture/shared.png", b"install");
    for dir in ["alpha", "alpha_copy"] {
        write(&mods.join(dir), "mod.toml", b"[mod]\nid = \"alpha\"\n");
        write(&mods.join(dir), "texture/shared.png", dir.as_bytes());
    }
    let out = inspect(&[
        "--mods",
        mods.to_str().unwrap(),
        "conflicts",
        install.to_str().unwrap(),
    ]);
    assert!(!out.status.success(), "{out:?}");
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(
        err.contains("alpha_copy") && err.contains("must be unique"),
        "{err}"
    );
}

#[test]
fn mods_classifies_each_mod_by_the_files_it_wins() {
    let tmp = tempfile::tempdir().unwrap();
    let (install, mods) = (tmp.path().join("install"), tmp.path().join("mods"));
    write(&install, "texture/shared.png", b"install");
    write(&install, "tune/car.txt", b"install");
    mod_dir(&mods, "a-paint", &["texture/shared.png", "aud/horn.wav"]);
    mod_dir(&mods, "b-tuning", &["tune/car.txt"]);
    let (install, mods) = (install.to_str().unwrap(), mods.to_str().unwrap());

    let out = inspect(&["--mods", mods, "mods", install]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("cosmetic-only a-paint: wins 0 gameplay + 2 cosmetic path(s), 0 shadowed"),
        "{text}"
    );
    assert!(
        text.contains("GAMEPLAY      b-tuning: wins 1 gameplay + 0 cosmetic path(s), 0 shadowed, first gameplay path tune/car.txt"),
        "{text}"
    );
    assert!(
        text.contains("2 mod(s), 1 change gameplay content"),
        "{text}"
    );

    // `--expect-cosmetic` turns the gameplay mod into a failure.
    let strict = inspect(&["--mods", mods, "mods", install, "--expect-cosmetic"]);
    assert!(!strict.status.success());
    assert!(String::from_utf8_lossy(&strict.stderr).contains("1 mod(s) change gameplay"));

    // No mods: nothing to classify, nothing to fail.
    let none = inspect(&["mods", install, "--expect-cosmetic"]);
    assert!(none.status.success(), "{none:?}");
    assert!(
        String::from_utf8(none.stdout)
            .unwrap()
            .contains("0 mod(s), 0 change gameplay")
    );
}

/// A mod directory whose manifest carries an `effect` claim.
fn claiming_mod(mods: &Path, id: &str, effect: &str, files: &[&str]) {
    mod_dir(mods, id, files);
    let manifest = format!("[mod]\nid = \"{id}\"\neffect = \"{effect}\"\n");
    write(&mods.join(id), "mod.toml", manifest.as_bytes());
}

#[test]
fn mods_reports_a_declared_effect_the_files_contradict() {
    let tmp = tempfile::tempdir().unwrap();
    let (install, mods) = (tmp.path().join("install"), tmp.path().join("mods"));
    write(&install, "tune/x.txt", b"install");
    claiming_mod(&mods, "a-skin", "cosmetic", &["texture/x.png"]);
    claiming_mod(&mods, "b-tuning", "gameplay", &["tune/x.txt"]);
    let (install, mods) = (install.to_str().unwrap(), mods.to_str().unwrap());

    let ok = inspect(&["--mods", mods, "mods", install]);
    assert!(ok.status.success(), "{ok:?}");
    let text = String::from_utf8(ok.stdout).unwrap();
    assert!(text.contains("declared effect: cosmetic"), "{text}");
    assert!(text.contains("declared effect: gameplay"), "{text}");
    assert!(!text.contains("MISMATCH"), "{text}");

    // A tuning mod claiming to be cosmetic is named, and the command fails
    // — but the verdict stays GAMEPLAY, whatever the claim says.
    claiming_mod(Path::new(mods), "c-liar", "cosmetic", &["tune/y.txt"]);
    let bad = inspect(&["--mods", mods, "mods", install]);
    assert_eq!(bad.status.code(), Some(2), "{bad:?}");
    let text = String::from_utf8(bad.stdout).unwrap();
    assert!(text.contains("MISMATCH c-liar"), "{text}");
    assert!(
        text.lines()
            .any(|l| l.starts_with("GAMEPLAY") && l.contains("c-liar")),
        "{text}"
    );
    assert!(String::from_utf8_lossy(&bad.stderr).contains("1 mod(s) declare an effect"));
}

/// The shipped `examples/mods` are documentation authors copy from, so each
/// must make an honest claim about what it changes.
#[test]
fn the_shipped_example_mods_declare_an_effect_their_files_confirm() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods");
    let mut dirs: Vec<_> = fs::read_dir(&examples)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("mod.toml").is_file())
        .collect();
    assert!(!dirs.is_empty(), "no example mods under {examples:?}");
    dirs.sort();
    for dir in &dirs {
        let manifest = fs::read_to_string(dir.join("mod.toml")).unwrap();
        assert!(manifest.contains("effect = "), "{dir:?} declares no effect");
    }
    let install = tempfile::tempdir().unwrap();
    let out = inspect(&[
        "--mods",
        examples.to_str().unwrap(),
        "mods",
        install.path().to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(!text.contains("MISMATCH"), "{text}");
    assert!(
        text.contains(&format!("{} mod(s)", dirs.len())),
        "every example mod is mounted and reported: {text}"
    );
}

#[test]
fn deps_shows_which_source_served_the_files_a_failed_load_read() {
    let tmp = tempfile::tempdir().unwrap();
    let (install, mods) = (tmp.path().join("install"), tmp.path().join("mods"));
    write(&install, "tune/vehicle/vpt.vehcarsim", b"install");
    write(&install, "geometry/vpt.pkg", b"not a package");
    mod_dir(&mods, "retune", &["tune/vehicle/vpt.vehcarsim"]);
    let (install, mods) = (install.to_str().unwrap(), mods.to_str().unwrap());

    // The synthetic car is not loadable, but the report names what the
    // loader read before it gave up, and who served it.
    let out = inspect(&["--mods", mods, "deps", install, "vpt"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("mod `retune`: 1 file(s)\n  tune/vehicle/vpt.vehcarsim"),
        "{text}"
    );
    assert!(text.contains("load failed after"), "{text}");
}

/// A synthetic London circuit table with three rows; only row 0 has its
/// authored records.
fn write_event_install(install: &Path) {
    let header = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, \
        NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, \
        Peds, NumLaps, TimeLimit, Difficulty";
    let row = "none, 0, 0, 0, 0, 0, 0.5, 0.5, 3, 50.0, 1, 0, 0, 0, 0, 0, 0.5, 0.5, 3, 60.0, 2";
    let table = format!("{header}\r\n{row}\r\n{row}\r\n{row}\r\n");
    write(install, "race/london/mmcircuitdata.csv", table.as_bytes());
    write(
        install,
        "race/london/circuit0waypoints.csv",
        WAYPOINTS.as_bytes(),
    );
    write(
        install,
        "race/london/circuit0_strtpnts",
        b"0,0,0,90,0,0,0,0,0,\r\n5,0,0,90,0,0,0,0,0,\r\n",
    );
}

const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\r\n\
    0,0,0,0,10,0,0,0,\r\n50,0,0,0,10,0,0,0,\r\n100,0,0,0,10,0,0,0,\r\n150,0,0,0,10,0,0,0,\r\n";

#[test]
fn deps_traces_an_event_and_credits_only_the_mod_that_replaced_its_files() {
    let tmp = tempfile::tempdir().unwrap();
    let (install, mods) = (tmp.path().join("install"), tmp.path().join("mods"));
    write_event_install(&install);
    // A real (different) route: the mod moves the second gate.
    let moved = WAYPOINTS.replace("50,0,0,0,10", "60,0,0,0,10");
    write(
        &mods.join("reroute"),
        "mod.toml",
        b"[mod]\nid = \"reroute\"\n",
    );
    write(
        &mods.join("reroute"),
        "race/london/circuit0waypoints.csv",
        moved.as_bytes(),
    );
    // A mod for another city: its files are never part of London's scan.
    write(
        &mods.join("elsewhere"),
        "mod.toml",
        b"[mod]\nid = \"elsewhere\"\n",
    );
    write(
        &mods.join("elsewhere"),
        "race/sf/circuit0waypoints.csv",
        WAYPOINTS.as_bytes(),
    );
    let (install, mods) = (install.to_str().unwrap(), mods.to_str().unwrap());
    let trace = |row: &str, extra: &[&str]| {
        let mut args = vec!["--mods", mods, "deps", install, "--city", "london"];
        args.extend(["--event", row]);
        args.extend(extra);
        inspect(&args)
    };

    let out = trace("circuit:0", &["--expect-mod", "reroute"]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("mod `reroute`: 1 file(s)\n  race/london/circuit0waypoints.csv"),
        "{text}"
    );
    // The catalog table itself is still the original's.
    assert!(text.contains("race/london/mmcircuitdata.csv"), "{text}");
    assert!(text.contains("london Circuit:0 (circuit0)"), "{text}");

    // The other city's mod is never read, so expecting it fails.
    let bystander = trace("circuit:0", &["--expect-mod", "elsewhere"]);
    assert_eq!(bystander.status.code(), Some(2), "{bystander:?}");
    assert!(String::from_utf8_lossy(&bystander.stderr).contains("no file was read from mod"));

    // A row with no authored line is a failed lookup, not a silent empty trace.
    let unknown = trace("circuit:9", &[]);
    assert_eq!(unknown.status.code(), Some(2), "{unknown:?}");
    assert!(
        String::from_utf8(unknown.stdout)
            .unwrap()
            .contains("load failed after")
    );

    // A car id and an event are alternatives, not a pair.
    let both = inspect(&[
        "deps",
        install,
        "vpt",
        "--city",
        "london",
        "--event",
        "circuit:0",
    ]);
    assert!(!both.status.success());
    let neither = inspect(&["deps", install]);
    assert!(!neither.status.success());
}
