//! Shared plumbing for the `mm2_app` process-level net suites
//! (`net_host`, `net_join`): a spawned child process with its stdout
//! drained onto a channel and its stdin held open for operator
//! commands, plus the synthetic `testcity` install the event legs
//! mount.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use mm2_assets::{InstallMount, Vfs, mount_install};
use mm2_game::{EventRef, EventTableKind};

/// How long a leg waits for one record before failing rather than
/// hanging on a dead child.
pub const WAIT: Duration = Duration::from_secs(15);

pub const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
pub const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";
/// A row with one authored lap/time block per difficulty — as a
/// circuit row its `NumLaps` of 1 builds an `Ordered` definition.
pub const ROW: &str = "none,0,0,0,0,0,0.1,0.0,1,50,1,0,0,0,0,0,0.2,0.0,1,40,1";
/// A circuit row whose `NumLaps` is zero — resolves fine, fails the
/// race-definition build (`Ordered` needs at least one lap).
pub const LAPLESS_ROW: &str = "none,0,0,0,0,0,0.1,0.0,0,50,1,0,0,0,0,0,0.2,0.0,0,40,1";

/// A running `mm2-host`/`mm2-join` child with its stdout drained onto
/// a channel — the process's `key=value` record contract is the
/// observable surface — and its stdin held open for operator commands.
pub struct Proc {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl Proc {
    pub fn spawn(exe: &str, args: &[String]) -> Self {
        let mut child = Command::new(exe)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn {exe}: {e}"));
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            lines: rx,
        }
    }

    /// The next record line, failing rather than hanging if the child
    /// dies or stalls.
    pub fn line(&self) -> String {
        self.lines
            .recv_timeout(WAIT)
            .unwrap_or_else(|_| panic!("no line from child"))
    }

    /// Lines until one contains `needle` — returns it. Earlier records
    /// are consumed and discarded; a dead child fails rather than
    /// hanging.
    pub fn until(&self, needle: &str) -> String {
        loop {
            let line = self.line();
            if line.contains(needle) {
                return line;
            }
        }
    }

    /// One operator command on the child's stdin (`start`, `cancel`,
    /// `ready`, `vehicle …`, `quit`).
    pub fn cmd(&mut self, command: &str) {
        writeln!(self.stdin, "{command}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// Reap the child and return its exit status — for `quit` legs.
    pub fn wait(mut self) -> std::process::ExitStatus {
        self.child.wait().expect("failed to wait for child")
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Read the `listening=` record an `mm2-host` prints and return
/// (address, fingerprint, line) — the two facts a join needs plus the
/// record itself for callers asserting on `session=`.
pub fn listening(host: &Proc) -> (SocketAddr, u64, String) {
    // The first record binds the contract: the address peers dial and
    // the gameplay fingerprint the handshake requires.
    let first = host.line();
    let addr: SocketAddr = first
        .strip_prefix("listening=")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("unexpected first record: {first:?}"))
        .parse()
        .unwrap();
    let fp_hex = first
        .split_whitespace()
        .find_map(|tok| tok.strip_prefix("fingerprint=fnv1a64:"))
        .unwrap_or_else(|| panic!("no fingerprint in {first:?}"));
    let fp = u64::from_str_radix(fp_hex, 16).unwrap();
    (addr, fp, first)
}

/// Mount `dir` as an install on a fresh VFS — the view a joining peer
/// checks an advertised session against.
pub fn mount(dir: &std::path::Path) -> Vfs {
    let mut vfs = Vfs::new();
    mount_install(&mut vfs, dir, &InstallMount::default()).unwrap();
    vfs
}

/// An `EventRef` into the fixture city's tables.
pub fn event(table: EventTableKind, index: usize) -> EventRef {
    EventRef {
        city: "testcity".to_string(),
        table,
        index,
    }
}

pub fn write(dir: &std::path::Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

pub fn waypoint_row(x: f32, z: f32) -> String {
    format!("{x},0,{z},0,15,0,0,0,\n")
}

fn waypoints(points: &[(f32, f32)]) -> String {
    let mut out = WAYPOINTS.to_string();
    for &(x, z) in points {
        out.push_str(&waypoint_row(x, z));
    }
    out
}

/// A minimal `testcity` install: the `city/testcity.psdl` `--city`
/// requires, one checkpoint row (`race:0`) with the records
/// `EventCatalog::resolve` demands (`race0.aimap`,
/// `race0waypoints.csv`), and a two-row circuit table — `circuit:0`
/// resolves Ready but cannot build (`NumLaps` 0), `circuit:1` builds
/// an `Ordered` definition. The lobby layer never loads geometry, so
/// stub bytes suffice.
pub fn event_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/testcity.psdl", b"synthetic fixture stub\n");
    write(
        d,
        "race/testcity/mmracedata.csv",
        format!("{MM_HEADER}\n{ROW}\n"),
    );
    write(d, "race/testcity/race0.aimap", "#\n");
    write(
        d,
        "race/testcity/race0waypoints.csv",
        waypoints(&[
            (60.0, 140.0),
            (110.0, 140.0),
            (140.0, 140.0),
            (165.0, 140.0),
            (180.0, 140.0),
        ]),
    );
    write(
        d,
        "race/testcity/mmcircuitdata.csv",
        format!("{MM_HEADER}\n{LAPLESS_ROW}\n{ROW}\n"),
    );
    for stem in ["circuit0", "circuit1"] {
        write(d, &format!("race/testcity/{stem}.aimap"), "#\n");
        write(
            d,
            &format!("race/testcity/{stem}waypoints.csv"),
            waypoints(&[
                (60.0, 140.0),
                (110.0, 140.0),
                (140.0, 140.0),
                (165.0, 140.0),
            ]),
        );
    }
    tmp
}
