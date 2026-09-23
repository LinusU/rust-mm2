//! Integration tests for the F00-C evidence smokes.
//!
//! `headless_smoke` is the same runner the `mm2 --headless` binary path
//! uses, so these exercise the production smoke — dev world without MM2
//! data, explicit failure for a requested-but-missing city, and report
//! records whose kinds stay distinguishable.

use std::path::Path;

use bevy::prelude::Vec3;
use mm2_app::session::SelectedCar;
use mm2_app::smoke::{self, SmokeRecord, SmokeStatus};
use mm2_assets::Vfs;
use mm2_game::{DevOverrides, SessionConfig, SpawnPose, WorldMode};
use mm2_vehicle::VehicleConfig;

fn write(dir: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

fn push_lp(out: &mut Vec<u8>, s: &str) {
    out.push(s.len() as u8 + 1);
    out.extend_from_slice(s.as_bytes());
    out.push(0);
}

fn push_f32s(out: &mut Vec<u8>, v: &[f32]) {
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
}

/// A one-room city whose drivable road sits at y=0 while its authored
/// bounding box reaches −60 — the shape that exposed the spawn-relative
/// "fell through the world" verdict on retail `sf/circuit0` (route
/// bottom ~28 m under the start grid) and London's −22 subway.
fn descending_psdl() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"PSD0");
    d.extend_from_slice(&2u32.to_le_bytes()); // target_size
    let verts: &[[f32; 3]] = &[
        [-5., 0., 0.],
        [-3., 0., 0.],
        [3., 0., 0.],
        [5., 0., 0.], // road section 0: sw_l, rl, rr, sw_r
        [-5., 0., 20.],
        [-3., 0., 20.],
        [3., 0., 20.],
        [5., 0., 20.], // road section 1
    ];
    d.extend_from_slice(&(verts.len() as u32).to_le_bytes());
    for v in verts {
        push_f32s(&mut d, v);
    }
    let heights = [0.15f32];
    d.extend_from_slice(&(heights.len() as u32).to_le_bytes());
    push_f32s(&mut d, &heights);
    d.extend_from_slice(&2u32.to_le_bytes()); // texture table stores count + 1
    push_lp(&mut d, "test_road");
    d.extend_from_slice(&2u32.to_le_bytes()); // nRooms
    d.extend_from_slice(&0u32.to_le_bytes()); // junctions
    let attr_words: Vec<u16> = vec![
        0x0a << 3,
        1,    // texture ref → textures[0]
        0x00, // counted road: 2 sections, 8 vertex indices
        2,
        0,
        1,
        2,
        3,
        4,
        5,
        6,
        7,
    ];
    let mut room = Vec::new();
    room.extend_from_slice(&4u32.to_le_bytes()); // nPerimeter
    room.extend_from_slice(&(attr_words.len() as u32).to_le_bytes());
    for v in [0u16, 1, 2, 3] {
        room.extend_from_slice(&v.to_le_bytes());
        room.extend_from_slice(&0u16.to_le_bytes()); // neighbour room
    }
    for w in &attr_words {
        room.extend_from_slice(&w.to_le_bytes());
    }
    d.extend_from_slice(&room);
    d.extend_from_slice(&[0u8; 2]); // room flags (nRooms entries)
    d.extend_from_slice(&[0u8; 2]); // prop rules
    push_f32s(&mut d, &[-5., -60., 0.]); // bounds min — deep authored floor
    push_f32s(&mut d, &[5., 6., 20.]); // bounds max
    push_f32s(&mut d, &[0., -27., 10.]); // bounds centre
    push_f32s(&mut d, &[70.]); // radius
    d.extend_from_slice(&0u32.to_le_bytes()); // nPaths
    d
}

fn descending_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/test.psdl", descending_psdl());
    write(
        d,
        "texture/test_road.png",
        include_bytes!("../../../assets/texture/dev_road.png"),
    );
    tmp
}

/// Regression for the "fell through the world" verdict: the end-of-run
/// pose must compare to the loaded world's authored floor, not to a
/// spawn-relative line — a session that legitimately descends below its
/// spawn is not a world fall. Staged by dev-spawning the car 40 m over
/// a city whose authored bounds reach −60: it drops onto the road,
/// grounds, and drives off the edge into recovery loops — every pose
/// it can end in sits ~40 m below the spawn yet far above the −85
/// below-world line. The spawn-relative check failed this run; the
/// authored floor does not.
#[test]
fn descending_below_spawn_is_not_a_world_fall() {
    let install = descending_install();
    let mut vfs = Vfs::new();
    vfs.mount_dir(install.path(), 0).unwrap();
    let config = SessionConfig {
        world: WorldMode::City {
            psdl: "city/test.psdl".into(),
        },
        dev: DevOverrides {
            spawn: Some(SpawnPose {
                position: Vec3::new(0.0, 40.0, 10.0),
                yaw: 0.0,
            }),
            ..DevOverrides::default()
        },
        ..SessionConfig::default()
    };
    let rec = smoke::headless_smoke(
        &config,
        vfs,
        SelectedCar {
            def: None,
            paint: 0,
        },
        &VehicleConfig::default(),
        600,
        smoke::Driver::Hold,
        None,
    );
    assert_eq!(
        rec.status,
        SmokeStatus::Pass,
        "a descent inside the authored bounds must pass: {}",
        rec.line()
    );
    assert!(
        !rec.line().contains("fell through"),
        "expected no below-world verdict: {}",
        rec.line()
    );
}

/// The dev world must start with no original data at all — an empty VFS —
/// and the car must settle and drive.
#[test]
fn dev_world_headless_smoke_passes_without_mm2_data() {
    let vfs = Vfs::new();
    let rec = smoke::headless_smoke(
        &SessionConfig::default(),
        vfs,
        SelectedCar {
            def: None,
            paint: 0,
        },
        &VehicleConfig::default(),
        600,
        smoke::Driver::Hold,
        None,
    );
    assert_eq!(
        rec.status,
        SmokeStatus::Pass,
        "expected pass, got: {}",
        rec.line()
    );
    assert!(rec.line().starts_with("smoke=headless-physics"));
    assert!(rec.line().contains("world=dev-world"));
    // The session clock ran on the fixed step: ~2 ticks per 60 Hz update
    // at the 120 Hz timestep, minus the clock priming update (F01-AC03).
    let ticks: u64 = rec
        .line()
        .split_whitespace()
        .find_map(|kv| kv.strip_prefix("ticks=").and_then(|v| v.parse().ok()))
        .expect("record reports ticks=");
    assert!(
        (1100..=1200).contains(&ticks),
        "600 updates should produce ~1200 fixed ticks, got {ticks}"
    );
}

/// A mid-run session restart is a legitimate lifecycle event — a
/// `RestartEvent` disabled outcome, a Backspace/results-row restart or
/// the `--restart` dev override all travel the same production
/// `Playing → Unloading → Menu → begin` path, which despawns the
/// session-owned car and spawns a new one. The record must follow the
/// live entity, count the restart and never report the despawned one
/// as a "non-finite pose" (a false failure observed on retail London
/// `checkpoint:0`, where the scripted driver disabled mid-run and the
/// event legitimately restarted).
#[test]
fn dev_world_headless_smoke_follows_player_across_restart() {
    let vfs = Vfs::new();
    let config = SessionConfig {
        dev: DevOverrides {
            restart: true,
            ..DevOverrides::default()
        },
        ..SessionConfig::default()
    };
    let rec = smoke::headless_smoke(
        &config,
        vfs,
        SelectedCar {
            def: None,
            paint: 0,
        },
        &VehicleConfig::default(),
        600,
        smoke::Driver::Hold,
        None,
    );
    assert_eq!(
        rec.status,
        SmokeStatus::Pass,
        "expected pass, got: {}",
        rec.line()
    );
    let line = rec.line();
    assert!(
        line.contains("rs=1"),
        "record should count the restart it went through: {line}"
    );
    assert!(
        !line.contains("NaN"),
        "a despawned entity must not report a NaN pose: {line}"
    );
    assert!(
        line.contains("phase=playing"),
        "the restarted session should be live again at the cap: {line}"
    );
    assert!(
        line.contains("final=("),
        "the live player should have a pose at the cap: {line}"
    );
}

/// A specifically requested city that the VFS cannot provide must report
/// an explicit failure — never a pass and never a silent dev world.
#[test]
fn requested_missing_city_is_an_explicit_failure() {
    let vfs = Vfs::new();
    let config = SessionConfig {
        world: WorldMode::City {
            psdl: "city/atlantis.psdl".into(),
        },
        ..SessionConfig::default()
    };
    let rec = smoke::headless_smoke(
        &config,
        vfs,
        SelectedCar {
            def: None,
            paint: 0,
        },
        &VehicleConfig::default(),
        600,
        smoke::Driver::Hold,
        None,
    );
    assert_eq!(
        rec.status,
        SmokeStatus::Fail,
        "expected fail, got: {}",
        rec.line()
    );
    assert!(
        rec.line().contains("city/atlantis.psdl"),
        "record should name the missing logical path: {}",
        rec.line()
    );
}

/// AC05: a visual record and a headless-physics record must not be
/// confusable, and `unavailable` must stay distinct from `fail`.
#[test]
fn record_lines_distinguish_kinds_and_statuses() {
    let headless = SmokeRecord {
        kind: smoke::KIND_HEADLESS_PHYSICS,
        world: "dev-world".into(),
        status: SmokeStatus::Pass,
        detail: "updates=600".into(),
    };
    let visual = SmokeRecord {
        kind: smoke::KIND_VISUAL,
        world: "dev-world".into(),
        status: SmokeStatus::Pass,
        detail: "frames=done".into(),
    };
    assert!(headless.line().contains("smoke=headless-physics"));
    assert!(visual.line().contains("smoke=visual"));
    assert!(!visual.line().contains("smoke=headless"));

    let unavailable = SmokeRecord {
        status: SmokeStatus::Unavailable,
        ..visual.clone()
    };
    assert!(unavailable.line().contains("status=unavailable"));
    assert!(!unavailable.line().contains("status=fail"));

    // Exit codes: pass 0, fail 3, unavailable 4 — distinct, nonzero on
    // anything that is not a pass.
    assert_eq!(SmokeStatus::Pass.exit_code(), 0);
    assert_eq!(SmokeStatus::Fail.exit_code(), 3);
    assert_eq!(SmokeStatus::Unavailable.exit_code(), 4);
}
