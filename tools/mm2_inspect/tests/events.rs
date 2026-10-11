//! `mm2-inspect events` listing and `--strict` contract (F11-AC01) on
//! synthetic installs (no original content).

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn write(dir: &Path, rel: &str, contents: &[u8]) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, contents).unwrap();
}

fn inspect(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mm2-inspect"))
        .args(args)
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\r\n\
    0,0,0,0,10,0,0,0,\r\n\
    50,0,0,0,10,0,0,0,\r\n\
    100,0,0,0,10,0,0,0,\r\n\
    150,0,0,0,10,0,0,0,\r\n";
const STARTS: &str = "0,0,0,90,0,0,0,0,0,\r\n5,0,0,90,0,0,0,0,0,\r\n";
const OPP: &str = "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\r\n\
    0,0,0,175,0,0,0,0,0\r\n\
    50,0,0,0,0,0,0,0,0\r\n\
    100,0,0,0,0,0,0,0,0\r\n";
const TABLE_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, \
    Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, \
    Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const AIMAP: &str = "[Opponent]\r\n1\r\nvpbug circuit0-a-0.opp 1.00 0 50.0 0.7 0 0 0 0 0 1.0\r\n";
const AIMAP_P: &str = "[Opponent]\r\n2\r\n\
    vpbug circuit0-p-0.opp 1.00 0 50.0 0.7 0 0 0 0 0 1.0\r\n\
    vpford circuit0-p-1.opp 1.00 0 50.0 0.7 0 0 0 0 0 1.0\r\n";

/// A london install with a 2-row circuit table: row 0 has the full
/// authored record set, row 1 has none of it (so it is incomplete).
fn synthetic_install() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let mut table = format!("{TABLE_HEADER}\r\n");
    for _ in 0..2 {
        table.push_str(
            "none, 0, 0, 0, 1, 0, 0.5, 0.5, 3, 50.0, 1, \
             0, 0, 0, 2, 0, 0.5, 0.5, 3, 60.0, 2\r\n",
        );
    }
    write(d, "race/london/mmcircuitdata.csv", table.as_bytes());
    write(d, "race/london/circuit0waypoints.csv", WAYPOINTS.as_bytes());
    write(d, "race/london/circuit0_strtpnts", STARTS.as_bytes());
    write(d, "race/london/circuit0.aimap", AIMAP.as_bytes());
    write(d, "race/london/circuit0.aimap_p", AIMAP_P.as_bytes());
    write(d, "race/london/circuit0-a-0.opp", OPP.as_bytes());
    write(d, "race/london/circuit0-p-0.opp", OPP.as_bytes());
    write(d, "race/london/circuit0-p-1.opp", OPP.as_bytes());
    write(d, "race/london/circuit0.pathset", b"PTH1\0\0\0\0\0\0\0\0");
    write(d, "tune/vpbug.info", b"vpbug\n");
    dir
}

#[test]
fn empty_install_fails_strict_and_passes_without() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap();

    let strict = inspect(&["events", path, "--strict"]);
    assert!(!strict.status.success(), "{strict:?}");
    let text = stdout(&strict);
    assert!(text.contains("(no authored events cataloged)"), "{text}");
    assert!(text.contains("table not resolvable"), "{text}");

    let lax = inspect(&["events", path]);
    assert!(lax.status.success(), "{lax:?}");
    assert!(stdout(&lax).contains("(no authored events cataloged)"));
}

#[test]
fn lists_ready_and_incomplete_rows_with_tags_and_failed_refs() {
    let dir = synthetic_install();
    let path = dir.path().to_str().unwrap();
    let out = inspect(&["events", path, "--city", "london"]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);

    let row = |i: &str| {
        text.lines()
            .find(|l| l.trim_start().starts_with(i))
            .unwrap_or_else(|| panic!("no row {i} in:\n{text}"))
            .to_string()
    };
    let (r0, r1) = (row("[ 0]"), row("[ 1]"));
    assert!(r0.contains("circuit0"), "{text}");
    assert!(r0.contains("ready"), "{r0}");
    assert!(r0.contains("oppa:") && r0.contains("oppp:"), "{r0}");
    assert!(r0.contains("wp:") && r0.contains("strtpnts:"), "{r0}");
    assert!(r1.contains("incomplete ("), "{r1}");
    assert!(!text.contains("failed ref:"), "{text}");
}

#[test]
fn a_dead_reference_is_listed_not_dropped() {
    let dir = synthetic_install();
    // A record that exists but does not parse is a failed reference.
    write(
        dir.path(),
        "race/london/circuit0-p-1.opp",
        b"not an opp file",
    );
    let path = dir.path().to_str().unwrap();
    let out = inspect(&["events", path, "--city", "london"]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    assert!(
        text.contains("failed ref: race/london/circuit0-p-1.opp"),
        "{text}"
    );
    assert!(text.contains("!Opp"), "{text}");

    let strict = inspect(&["events", path, "--city", "london", "--strict"]);
    assert!(!strict.status.success(), "{strict:?}");
}

#[test]
fn strict_fails_on_the_incomplete_event() {
    let dir = synthetic_install();
    let path = dir.path().to_str().unwrap();
    let out = inspect(&["events", path, "--city", "london", "--strict"]);
    assert!(!out.status.success(), "{out:?}");
    assert!(stdout(&out).contains("incomplete ("), "{}", stdout(&out));
}
