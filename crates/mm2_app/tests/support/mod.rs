//! Shared plumbing for the `mm2_app` process-level net suites
//! (`net_host`, `net_join`): a spawned child process with its stdout
//! drained onto a channel and its stdin held open for operator
//! commands, plus the synthetic `testcity` install the event legs
//! mount.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use mm2_assets::{InstallMount, Vfs, mount_install};
use mm2_game::{EventRef, EventTableKind};

/// How long a leg waits for one record before failing rather than
/// hanging on a dead child.
pub const WAIT: Duration = Duration::from_secs(15);

/// How many retail two-process legs run at once. Each child mounts the
/// whole retail install (4 archives, ~13k files), so with every leg on
/// its own test thread the children starve each other and miss the
/// per-line [`WAIT`]; measured, 4 concurrent test threads passed
/// everything and 8 did not. Two legs is two to four children.
const RETAIL_LEGS: usize = 2;

/// A held place among the concurrently running retail legs; released
/// on drop.
pub struct RetailSlot;

static RETAIL_RUNNING: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

impl Drop for RetailSlot {
    fn drop(&mut self) {
        let (running, freed) = &RETAIL_RUNNING;
        *running.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        freed.notify_one();
    }
}

/// The operator's retail install (`MM2_RETAIL`), if set, with a
/// [`RetailSlot`] the leg holds for its whole body: blocks until fewer
/// than [`RETAIL_LEGS`] retail legs are running. A leg takes exactly
/// one slot, at its start, so the wait cannot deadlock. `None` (leg
/// skipped) takes no slot.
pub fn retail_slot() -> Option<(PathBuf, RetailSlot)> {
    let retail = std::env::var_os("MM2_RETAIL").map(PathBuf::from)?;
    let (running, freed) = &RETAIL_RUNNING;
    let mut running = running.lock().unwrap_or_else(|e| e.into_inner());
    while *running >= RETAIL_LEGS {
        running = freed.wait(running).unwrap_or_else(|e| e.into_inner());
    }
    *running += 1;
    Some((retail, RetailSlot))
}

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
    stderr: Arc<Mutex<String>>,
}

impl Proc {
    /// Spawn `exe` with its stdout on the record channel. The child's
    /// log filter is pinned to the app's own default rather than the
    /// ambient `RUST_LOG`: several legs assert on a `WARN` line (an
    /// ignored `--traction` pin must be *named*, F06-AC06), and a
    /// caller's `RUST_LOG=error` shell would otherwise silence it —
    /// the same `env_remove` the `Command`-spawning suites already do.
    pub fn spawn(exe: &str, args: &[String]) -> Self {
        let mut child = Command::new(exe)
            .args(args)
            .env_remove("RUST_LOG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn {exe}: {e}"));
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr_pipe = child.stderr.take().unwrap();
        // Drained to a buffer so a failing leg can print why the child
        // died; an undrained pipe would also stall a chatty child.
        let stderr = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&stderr);
        thread::spawn(move || {
            for line in BufReader::new(stderr_pipe).lines() {
                let Ok(line) = line else { return };
                let mut sink = sink.lock().unwrap();
                sink.push_str(&line);
                sink.push('\n');
            }
        });
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
            stderr,
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

    /// Lines until one contains `needle`, waiting up to `bound` in all
    /// rather than [`WAIT`] per line — for a leg whose child is quiet
    /// for a long stretch (a headless match driving to its objective
    /// prints nothing between its events). Fails past `bound`.
    pub fn until_within(&self, needle: &str, bound: Duration) -> String {
        let deadline = Instant::now() + bound;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self
                .lines
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no {needle:?} from child within {bound:?}"));
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

    /// Reap the child and fail with its exit status and everything it
    /// wrote to stderr (plus its final `record`) unless it exited cleanly.
    pub fn wait_success(mut self, who: &str, record: &str) {
        let status = self.child.wait().expect("failed to wait for child");
        // The drain thread ends at EOF, which the exit has just caused.
        thread::sleep(Duration::from_millis(50));
        let stderr = self.stderr.lock().unwrap().clone();
        assert!(
            status.success(),
            "{who} did not exit cleanly: {status}\nrecord: {record}\nstderr:\n{stderr}"
        );
    }

    /// SIGKILL the child — the abrupt-loss legs' way to make a process
    /// vanish mid-stream instead of through its polite `quit` path (a
    /// dead peer's sockets close at the kernel, not by the lobby's own
    /// disconnect). `Drop` still reaps the corpse.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }

    /// SIGSTOP the child: it keeps its sockets open but stops
    /// speaking — a dead-but-open peer, which no socket close can model.
    /// The signal goes to the PID this `Proc` spawned and no other;
    /// `Drop`'s SIGKILL ends a stopped child, so a failing leg cannot
    /// leave it behind.
    pub fn stop(&self) {
        let status = Command::new("kill")
            .args(["-STOP", &self.child.id().to_string()])
            .status()
            .expect("failed to run kill");
        assert!(status.success(), "SIGSTOP of the child failed");
    }

    /// Reap the child, bounded: the loss legs' claim is that the child
    /// exits *on its own* once the peer dies, so a child that outlives
    /// `bound` is killed and the leg fails rather than hanging the
    /// suite.
    pub fn wait_timeout(mut self, bound: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + bound;
        loop {
            if let Some(status) = self.child.try_wait().expect("failed to poll child") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "child did not exit within {bound:?}"
            );
            thread::sleep(Duration::from_millis(25));
        }
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

// ─── An authored-pick fixture ──────────────────────────────────────
//
// The dev-car pick (`vehicle == ""`) carries no authored records, so
// legs that exercise an authored binding — F25-B's cardata audio on a
// remote seat, F15-A's `_opp` tune variants — write a minimal
// `vpt`-shaped car: the required tune and model plus the optional
// record under test. `tuned_car` is the shared base opponents.rs's
// roster fixture builds on.

/// Minimal `vehCarSim` tune — every required field, `mass` the only
/// variable so a `_opp` variant or a heavy car stays distinguishable.
pub fn vehcarsim(mass: f32) -> String {
    let wheel = |name: &str| {
        format!(
            "  {name} {{\n    SuspensionExtent 0.2\n    SuspensionLimit 0.05\n    SuspensionFactor 1.0\n    SuspensionDampCoef 0.1\n    SteeringLimit 0.5\n    BrakeCoef 0.14\n    TireDispLimitLong 0.075\n    TireDampCoefLong 0.75\n    TireDragCoefLong 0.01\n    TireDispLimitLat 0.075\n    TireDampCoefLat 0.75\n    TireDragCoefLat 0.02\n    OptimumSlipPercent 0.05\n    StaticFric 3.0\n    SlidingFric 2.95\n  }}\n"
        )
    };
    format!(
        "type: a\nvehCarSim {{\n  Mass {mass}\n  InertiaBox 2.0 1.3 3.0\n  DrivetrainType 0\n  Aero {{\n    Drag 0.5\n    Down 0.0\n  }}\n  Engine {{\n    MaxHorsePower 200.0\n    IdleRPM 750.0\n    OptRPM 5800.0\n    MaxRPM 8500.0\n  }}\n  Trans {{\n    AutoNumGears 4\n    Reverse 20.0\n    Low 20.0\n    High 75.0\n  }}\n{}{}}}\n",
        wheel("WheelFront"),
        wheel("WheelBack"),
    )
}

/// One quad geometry chunk: 4 verts, 2 tris, centred on `c`.
fn quad_geo(c: [f32; 3], hx: f32, hy: f32, hz: f32) -> Vec<u8> {
    let mut geo = Vec::new();
    geo.extend_from_slice(&1u32.to_le_bytes()); // nSections
    geo.extend_from_slice(&4u32.to_le_bytes()); // total vertices
    geo.extend_from_slice(&6u32.to_le_bytes()); // total indices
    geo.extend_from_slice(&1u32.to_le_bytes()); // sections duplicate
    geo.extend_from_slice(&0x112u32.to_le_bytes()); // fvf: XYZ|NORMAL|1 tex
    geo.extend_from_slice(&1u16.to_le_bytes()); // nStrips
    geo.extend_from_slice(&0u16.to_le_bytes()); // section flags
    geo.extend_from_slice(&(-1i32).to_le_bytes()); // shader offset → fallback
    geo.extend_from_slice(&3i32.to_le_bytes()); // prim type: triangles
    geo.extend_from_slice(&4u32.to_le_bytes()); // strip vertices
    for p in [
        [c[0] - hx, c[1] - hy, c[2] - hz],
        [c[0] + hx, c[1] - hy, c[2] + hz],
        [c[0] + hx, c[1] + hy, c[2] - hz],
        [c[0] - hx, c[1] + hy, c[2] + hz],
    ] {
        for v in p {
            geo.extend_from_slice(&v.to_le_bytes());
        }
        for n in [0.0f32, 1.0, 0.0] {
            geo.extend_from_slice(&n.to_le_bytes());
        }
        for uv in [0.0f32, 0.0] {
            geo.extend_from_slice(&uv.to_le_bytes());
        }
    }
    geo.extend_from_slice(&6u32.to_le_bytes()); // strip indices
    for i in [0u16, 1, 2, 0, 3, 1] {
        geo.extend_from_slice(&i.to_le_bytes());
    }
    geo
}

/// A PKG3 with `body_h` plus `whl0..3` — wheels authored in place
/// (no `.mtx`, so the importer falls back to geometry centres).
fn car_pkg() -> Vec<u8> {
    let mut d = b"PKG3".to_vec();
    let chunks: &[(&str, Vec<u8>)] = &[
        ("body_h", quad_geo([0.0, 0.5, 0.0], 0.9, 0.5, 1.6)),
        ("whl0_h", quad_geo([0.8, 0.3, -1.3], 0.15, 0.3, 0.15)),
        ("whl1_h", quad_geo([-0.8, 0.3, -1.3], 0.15, 0.3, 0.15)),
        ("whl2_h", quad_geo([0.8, 0.3, 1.3], 0.15, 0.3, 0.15)),
        ("whl3_h", quad_geo([-0.8, 0.3, 1.3], 0.15, 0.3, 0.15)),
    ];
    for (name, geo) in chunks {
        d.extend_from_slice(b"FILE");
        d.push(name.len() as u8 + 1);
        d.extend_from_slice(name.as_bytes());
        d.push(0);
        d.extend_from_slice(&(geo.len() as u32).to_le_bytes());
        d.extend_from_slice(geo);
    }
    d
}

/// An ASCII bound box — without it `convert` falls back to a centred
/// chassis cuboid whose hull rests high enough that the wheel rays
/// never reach the ground.
fn car_bnd() -> String {
    let mut s = "version: 1.01\nverts: 8\nmaterials: 1\nedges: 0\npolys: 6\n\n".to_string();
    for v in [
        [-0.9f32, 0.05, -1.6],
        [0.9, 0.05, -1.6],
        [0.9, 0.9, -1.6],
        [-0.9, 0.9, -1.6],
        [-0.9, 0.05, 1.6],
        [0.9, 0.05, 1.6],
        [0.9, 0.9, 1.6],
        [-0.9, 0.9, 1.6],
    ] {
        s.push_str(&format!("v {} {} {}\n", v[0], v[1], v[2]));
    }
    s.push_str("mtl default {\n  elasticity: 0.1\n  friction: 0.5\n}\n");
    for quad in [
        [0, 4, 5, 1],
        [0, 1, 2, 3],
        [4, 7, 6, 5],
        [0, 3, 7, 4],
        [1, 5, 6, 2],
        [3, 2, 6, 7],
    ] {
        s.push_str(&format!(
            "quad {} {} {} {} 0\n",
            quad[0], quad[1], quad[2], quad[3]
        ));
    }
    s
}

/// A `aud/cardata/player/<id>.csv` the real grammar accepts: the horn
/// record plus one canonical fade-window engine sample.
fn car_cardata() -> &'static str {
    "Horn wave name,Horn volume,flags,Num Engine Samples,clutch wave name,clutch volume\nTESTHORN,0.9,0,1,REV,0.5\nEngine wave name,Min Volume,Max Volume,fade in start RPM,fade in end RPM,fade out start RPM,fade out end RPM,Min Pitch,Max Pitch,Pitch shift start RPM,Pitch shift end RPM\nEIDLE,0.55,0.835,1,800,2500,7000,0.85,2,1,7000\n"
}

/// The tuned pick `id` at `mass`: the required tune + model + bound —
/// the files `load_vehicle` resolves before any optional record.
pub fn tuned_car(d: &std::path::Path, id: &str, mass: f32) {
    write(d, &format!("tune/vehicle/{id}.vehcarsim"), vehcarsim(mass));
    write(d, &format!("geometry/{id}.pkg"), car_pkg());
    write(d, &format!("bound/{id}_bound.bnd"), car_bnd());
}

/// The authored pick `id`: the required tune + model + bound plus its
/// cardata record — the files `load_vehicle` resolves for a
/// `VehicleAudio`-backed spawn (F25-B protocol v15).
pub fn audio_car(d: &std::path::Path, id: &str) {
    tuned_car(d, id, 1200.0);
    write(d, &format!("aud/cardata/player/{id}.csv"), car_cardata());
}

// ─── A surface-audio fixture ───────────────────────────────────────
//
// The legs that exercise `surface_voices`' live resolve (F25-B
// protocol v16's wire source) need the authored dry table plus the
// waves its rows name — the same fixture `tests/audio.rs::surface_dir`
// authors, shared here so the net legs mount the identical data.

/// A minimal 16-bit mono PCM RIFF/WAVE at `rate` with `frames` frames.
pub fn pcm_wav(rate: u32, frames: usize) -> Vec<u8> {
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1u16.to_le_bytes()); // PCM
    fmt.extend_from_slice(&1u16.to_le_bytes()); // mono
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * 2).to_le_bytes());
    fmt.extend_from_slice(&2u16.to_le_bytes()); // block align
    fmt.extend_from_slice(&16u16.to_le_bytes());
    let pcm = vec![0x20u8; frames * 2];
    let mut body = Vec::from(&b"WAVE"[..]);
    body.extend_from_slice(b"fmt ");
    body.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    body.extend_from_slice(&fmt);
    body.extend_from_slice(b"data");
    body.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    body.extend_from_slice(&pcm);
    let mut out = Vec::from(&b"RIFF"[..]);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// The authored dry table rows — the retail `default_surfacedry.csv`
/// 10-column schema. Row 0 is the `_default` road (NOSOUND rolling +
/// two slippage bands), row 1 `grass` (a rolling loop + one wide
/// band).
pub const DRY_SURFACE_TABLE: &[u8] = b"Tunnel sound index\n0\n\
surface wave,max speed,min surface volume,max surface volume,min surface pitch,max surface pitch,min skid volume,max skid volume,num skid samples\n\
NOSOUND,125,0,0,0,0,0.5,0.88,2\n\
skid wave,min slippage,max slippage\n\
ROADSKID1,0.5,0.75\n\
ROADSKID2,0.75,1\n\
surface wave,max speed,min surface volume,max surface volume,min surface pitch,max surface pitch,min skid volume,max skid volume,num skid samples\n\
ROLLWAVE,25,0.35,0.75,0.85,1.25,0.5,0.72,1\n\
skid wave,min slippage,max slippage\n\
GRASSSKID,0.25,1\n";

/// The surface-audio slice of an install: the dry table plus a wave
/// for every stem it names at a distinguishing sample rate — the files
/// `SurfaceAudio::load` and `WaveBank::index` resolve so a live contact
/// both resolves and voices.
pub fn surface_audio(d: &std::path::Path) {
    write(
        d,
        "aud/cardata/player/default_surfacedry.csv",
        DRY_SURFACE_TABLE,
    );
    for (stem, rate) in [
        ("roadskid1", 22050),
        ("roadskid2", 32000),
        ("grassskid", 11025),
        ("rollwave", 48000),
    ] {
        write(
            d,
            &format!("aud/aud22/surfaces/{stem}.22k.wav"),
            pcm_wav(rate, 220),
        );
    }
}

/// The global `city/materials.{mtl,csv}` pair a dev world needs to
/// resolve a surface class at all: `_default` authors `sound: 1` — the
/// dry table's grass row, so an unmarked collider (`SurfaceMaterial::
/// Unspecified`, every dev-world surface) inherits a rolling loop and
/// a wide skid band through the designed fallback, and a nonzero index
/// proves the class lookup rather than an accidental row 0.
/// `ptxindex -1 -1` keeps the wheel-effect channels dark; the csv maps
/// no textures because nothing named ever queries it.
pub fn surface_materials(d: &std::path::Path) {
    write(
        d,
        "city/materials.mtl",
        b"mtl _default {\n\
          elasticity: 0.0\n\
          friction: 1.0\n\
          effect: none\n\
          sound: 1\n\
          drag: 0.0\n\
          width: 0.0\n\
          height: 0.0\n\
          depth: 0.0\n\
          ptxindex: -1 -1\n\
          ptxthreshold: 0.0 0.0\n\
          }\n",
    );
    write(d, "city/materials.csv", b"texture,physics\n");
}

/// The named link recipes the impairment matrices arm (F25-AC03,
/// F26-AC01/AC03): one table so every replicated-state cell — `Snap`,
/// `Props`, `Traffic`, `World` — is read against the same weather.
pub fn impair_cells() -> Vec<(&'static str, mm2_net::Impair)> {
    use mm2_net::Impair;
    vec![
        // The control: a transparent pair of lanes.
        ("clean", Impair::default()),
        // Latency — a fixed hold every frame pays.
        (
            "latency",
            Impair {
                delay: Duration::from_millis(100),
                jitter: Duration::from_millis(20),
                ..Impair::default()
            },
        ),
        // Jitter — small fixed hold, wide spread: releases overtake
        // each other, a real reorder source on a lane.
        (
            "jitter",
            Impair {
                delay: Duration::from_millis(10),
                jitter: Duration::from_millis(60),
                ..Impair::default()
            },
        ),
        // Loss — every fifth frame gone, both ways.
        (
            "loss",
            Impair {
                loss: 0.20,
                ..Impair::default()
            },
        ),
        // Heavy loss — six of ten frames never arrive.
        (
            "loss-heavy",
            Impair {
                loss: 0.60,
                ..Impair::default()
            },
        ),
        // Duplication — every other frame emits a second adjacent copy.
        (
            "duplicate",
            Impair {
                duplicate: 0.50,
                ..Impair::default()
            },
        ),
        // Reorder — every other frame swaps with its successor.
        (
            "reorder",
            Impair {
                reorder: 0.50,
                ..Impair::default()
            },
        ),
        // Combined — the recipe the two-process `net_drive` leg runs.
        (
            "combined",
            Impair {
                delay: Duration::from_millis(40),
                jitter: Duration::from_millis(30),
                loss: 0.05,
                duplicate: 0.10,
                reorder: 0.10,
            },
        ),
    ]
}

// ---------------------------------------------------------------------------
// Synthetic traffic-city install (F10-A.2 fixture, shared by the `app` and
// `network` suites): PSDL + BAI + aimap + two ambient classes.
// ---------------------------------------------------------------------------

fn push_v3(d: &mut Vec<u8>, v: [f32; 3]) {
    for f in v {
        d.extend_from_slice(&f.to_le_bytes());
    }
}

fn push_f32s(d: &mut Vec<u8>, v: &[f32]) {
    for f in v {
        d.extend_from_slice(&f.to_le_bytes());
    }
}

/// The same `CAI1` fixture `tests/nav_overlay.rs` uses: two roads
/// chained through one intersection at the origin — road 0 spans
/// z −30..−4 (end joins the intersection), road 1 spans z 4..30
/// (start joins it). One vehicle lane + one sidewalk per side.
///
/// `r0_end`/`r1_start` are the authored `vehicleRule` codes on the two
/// junction-connected ends: `r0_end` rules the forward approach into
/// the junction (road 0's right lane), `r1_start` the backward
/// approach (road 1's left lane). `bai_bytes` authors `NeverStop` so
/// the plain fixture keeps its free-flow semantics.
pub fn bai_bytes() -> Vec<u8> {
    bai_with_rules(3, 3)
}

pub fn bai_with_rules(r0_end: u16, r1_start: u16) -> Vec<u8> {
    bai_with_lights((r0_end, None), (r1_start, None))
}

/// `bai_with_rules` plus authored `trafficLightOrigin` markers on the
/// two junction-connected ends — `None` writes the zero "no light"
/// marker. The axis authors a fixed `[0,1,0]` whenever a light
/// exists; its convention is unverified and the runtime ignores it.
pub fn bai_with_lights(
    r0_end: (u16, Option<[f32; 3]>),
    r1_start: (u16, Option<[f32; 3]>),
) -> Vec<u8> {
    bai_with_lane_offsets(&[3.75], r0_end, r1_start)
}

/// `bai_with_lights` parameterized on the per-side lane offsets —
/// extra driving lanes sit inside the sidewalk band (8.5·side), which
/// stays the outermost curve.
pub fn bai_with_lane_offsets(
    lanes: &[f32],
    r0_end: (u16, Option<[f32; 3]>),
    r1_start: (u16, Option<[f32; 3]>),
) -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"CAI1");
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&2u16.to_le_bytes());

    let write_road = |d: &mut Vec<u8>,
                      id: u16,
                      z0: f32,
                      z1: f32,
                      end: (u32, u32, u16, Option<[f32; 3]>),
                      start: (u32, u32, u16, Option<[f32; 3]>)| {
        d.extend_from_slice(&id.to_le_bytes());
        d.extend_from_slice(&2u16.to_le_bytes()); // nSections
        d.extend_from_slice(&0u16.to_le_bytes()); // flags
        d.extend_from_slice(&1u16.to_le_bytes()); // nRooms
        d.extend_from_slice(&1u16.to_le_bytes()); // room 1
        d.extend_from_slice(&7.5f32.to_le_bytes()); // half_width
        d.extend_from_slice(&15.0f32.to_le_bytes()); // base_speed
        let curves = lanes.len() + 1; // driving lanes + one sidewalk
        for side in [1f32, -1f32] {
            for n in [lanes.len() as u16, 0, 0, 1, 0] {
                // lanes, trams, trains, sidewalks, ambientTypes
                d.extend_from_slice(&n.to_le_bytes());
            }
            for _ in 0..curves {
                for s in [0f32, (z1 - z0).abs()] {
                    d.extend_from_slice(&s.to_le_bytes());
                }
            }
            for off in lanes.iter().copied().chain([8.5]) {
                let edge = if off == 8.5 { 9.5 } else { off + 1.25 };
                d.extend_from_slice(&edge.to_le_bytes());
            }
            d.extend_from_slice(&[0xCDu8; 40]);
            for off in lanes.iter().copied().chain([8.5]) {
                for z in [z0, z1] {
                    push_v3(d, [off * side, 0.0, z]);
                }
            }
            for z in [z0, z1] {
                push_v3(d, [7.5 * side, 0.0, z]);
            }
            for z in [z0, z1] {
                push_v3(d, [9.5 * side, 0.0, z]);
            }
        }
        for s in [0f32, (z1 - z0).abs()] {
            d.extend_from_slice(&s.to_le_bytes());
        }
        for z in [z0, z1] {
            push_v3(d, [0.0, 0.0, z]);
        }
        for _ in 0..2 {
            push_v3(d, [1.0, 0.0, 0.0]);
        }
        for _ in 0..2 {
            push_v3(d, [0.0, 1.0, 0.0]);
        }
        for _ in 0..2 {
            push_v3(d, [0.0, 0.0, 1.0]);
        }
        for _ in 0..2 {
            push_v3(d, [0.0, 0.0, 1.0]);
        }
        // The file stores `end` first, then `start`.
        for (intersection, road_index, rule, light) in [end, start] {
            d.extend_from_slice(&intersection.to_le_bytes());
            d.extend_from_slice(&0xCDCDu16.to_le_bytes());
            d.extend_from_slice(&rule.to_le_bytes());
            d.extend_from_slice(&0u16.to_le_bytes());
            d.extend_from_slice(&road_index.to_le_bytes());
            push_v3(d, light.unwrap_or([0.0; 3]));
            push_v3(d, light.map_or([0.0; 3], |_| [0.0, 1.0, 0.0]));
        }
    };
    write_road(
        &mut d,
        0,
        -30.0,
        -4.0,
        (0, 0, r0_end.0, r0_end.1),
        (0, mm2_formats::bai::END_FILL, 0, None),
    );
    write_road(
        &mut d,
        1,
        4.0,
        30.0,
        (0, mm2_formats::bai::END_FILL, 0, None),
        (0, 1, r1_start.0, r1_start.1),
    );

    d.extend_from_slice(&0u16.to_le_bytes()); // intersection id
    d.extend_from_slice(&1u16.to_le_bytes()); // room
    push_v3(&mut d, [0.0, 0.0, 0.0]);
    d.extend_from_slice(&2u16.to_le_bytes());
    for r in [0u32, 1] {
        d.extend_from_slice(&r.to_le_bytes());
    }
    d.extend_from_slice(&0u32.to_le_bytes()); // culling rooms
    d
}

/// One-room PSDL quad, same as `tests/nav_overlay.rs` but widened to
/// span the fixture's whole play space — the BAI lanes at x ±3.75 and
/// the player's quarantine spawn at z=200 all need ground under them
/// (a parked blocker or a bubble centre cannot stand on nothing).
pub fn synthetic_psdl() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"PSD0");
    d.extend_from_slice(&2u32.to_le_bytes());
    let verts: &[[f32; 3]] = &[
        [-40., 0., -40.],
        [-40., 0., 240.],
        [40., 0., 240.],
        [40., 0., -40.],
    ];
    d.extend_from_slice(&(verts.len() as u32).to_le_bytes());
    for v in verts {
        push_f32s(&mut d, v);
    }
    let heights = [0.15f32, 2.0, 6.0];
    d.extend_from_slice(&(heights.len() as u32).to_le_bytes());
    push_f32s(&mut d, &heights);
    d.extend_from_slice(&1u32.to_le_bytes());
    d.extend_from_slice(&2u32.to_le_bytes()); // nRooms
    d.extend_from_slice(&0u32.to_le_bytes()); // junctions
    let mut room = Vec::new();
    room.extend_from_slice(&4u32.to_le_bytes());
    room.extend_from_slice(&6u32.to_le_bytes());
    for v in [0u16, 1, 2, 3] {
        room.extend_from_slice(&v.to_le_bytes());
        room.extend_from_slice(&0u16.to_le_bytes());
    }
    for w in [0x06u16 << 3, 2, 0, 1, 2, 3] {
        room.extend_from_slice(&w.to_le_bytes());
    }
    d.extend_from_slice(&room);
    d.extend_from_slice(&[0u8; 2]);
    d.extend_from_slice(&[0u8; 2]);
    push_f32s(&mut d, &[-40., 0., -40.]);
    push_f32s(&mut d, &[40., 6., 240.]);
    push_f32s(&mut d, &[0., 3., 100.]);
    push_f32s(&mut d, &[160.]);
    d.extend_from_slice(&0u32.to_le_bytes());
    d
}

/// A `va_*` package: one body chunk only — no wheels, no `.mtx` parts.
fn va_pkg() -> Vec<u8> {
    let mut d = b"PKG3".to_vec();
    let geo = quad_geo([0.0, 0.5, 0.0], 0.9, 0.5, 1.6);
    d.extend_from_slice(b"FILE");
    d.push("body_h".len() as u8 + 1);
    d.extend_from_slice(b"body_h");
    d.push(0);
    d.extend_from_slice(&(geo.len() as u32).to_le_bytes());
    d.extend_from_slice(&geo);
    d
}

/// Bound box under the quad — underside at local ~0.
fn va_bnd() -> String {
    let mut s = "version: 1.01\nverts: 8\nmaterials: 1\nedges: 0\npolys: 6\n\n".to_string();
    for v in [
        [-0.9f32, 0.05, -1.6],
        [0.9, 0.05, -1.6],
        [0.9, 0.9, -1.6],
        [-0.9, 0.9, -1.6],
        [-0.9, 0.05, 1.6],
        [0.9, 0.05, 1.6],
        [0.9, 0.9, 1.6],
        [-0.9, 0.9, 1.6],
    ] {
        s.push_str(&format!("v {} {} {}\n", v[0], v[1], v[2]));
    }
    s.push_str("mtl default {\n  elasticity: 0.1\n  friction: 0.5\n}\n");
    for quad in [
        [0, 4, 5, 1],
        [0, 1, 2, 3],
        [4, 7, 6, 5],
        [0, 3, 7, 4],
        [1, 5, 6, 2],
        [3, 2, 6, 7],
    ] {
        s.push_str(&format!(
            "quad {} {} {} {} 0\n",
            quad[0], quad[1], quad[2], quad[3]
        ));
    }
    s
}

const VA_TUNE: &str = "type: a\n\
aiVehicleData {\n\
  Mass 500.0\n\
  Size 1.8 0.9 3.2\n\
  Elasticity 0.1\n\
  Friction 0.5\n\
  MaxDamage 70000.0\n\
  PtxThresh 70000.0\n\
  Spring 17000.0\n\
  Damping 1200.0\n\
  Limit 0.07\n\
  RubberSpring 12000.0\n\
  RubberDamp 600.0\n\
}\n";

/// The city aimap: a two-class roster plus `[Density] 0.25` — authored
/// below the session-config default 0.5, so a correct density chain
/// targets 0.25 × 32 = 8, not 16.
pub fn city_aimap() -> String {
    "[Ambient Types/Density]\n2\nva_test_a 0.5 0\nva_test_b 1.0 0\n[Density]\n0.25\n".to_string()
}

pub fn ambient_assets(dir: &Path, id: &str) {
    write(dir, &format!("tune/vehicle/{id}.aivehicledata"), VA_TUNE);
    write(dir, &format!("geometry/{id}.pkg"), va_pkg());
    write(dir, &format!("bound/{id}_bound.bnd"), va_bnd());
}

/// The synthetic city install: PSDL + BAI + aimap + two ambient classes.
pub fn city_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/test.psdl", synthetic_psdl());
    write(d, "city/test.bai", bai_bytes());
    write(d, "city/test.aimap", city_aimap());
    ambient_assets(d, "va_test_a");
    ambient_assets(d, "va_test_b");
    tmp
}
