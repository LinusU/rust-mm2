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
