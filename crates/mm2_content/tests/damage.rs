//! F05-A.1 damage-content tests: `DamageAudit` census over a synthetic
//! install (record decode, breakaway inventory, uncatalogued files,
//! malformed records as failures, authored absences as findings) and
//! `load_vehicle`'s damage/stuck/gyro attachment with provenance and
//! warning paths.

use std::path::Path;

use mm2_assets::Vfs;
use mm2_content::{AssetCheck, DamageAudit, load_vehicle};

fn write(dir: &Path, rel: &str, contents: &[u8]) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

fn write_str(dir: &Path, rel: &str, contents: &str) {
    write(dir, rel, contents.as_bytes());
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

const CARDAMAGE: &str = "type: a\n\
vehCarDamage {\n\
  MaxDamage 321300.0\n\
  MedDamage 150000.0\n\
  ImpactThreshold 1500.0\n\
  RegenerateRate 0.0\n\
  SmokeOffset 0.0 0.5 -1.0\n\
  TextelDamageRadius 0.5\n\
  Position 0.0 0.0 0.0\n\
  PositionVar 0.1 0.1 0.1\n\
  Velocity 0.0 1.0 0.0\n\
  VelocityVar 0.5 0.5 0.5\n\
  Life 1.0\n\
  LifeVar 0.5\n\
  Mass 1.0\n\
  MassVar 0.0\n\
  Radius 0.5\n\
  RadiusVar 0.25\n\
  Drag 0.0\n\
  DragVar 0.0\n\
  Damp 0.0\n\
  DampVar 0.0\n\
  DRadius 0.5\n\
  DRadiusVar 0.0\n\
  DAlpha -0.5\n\
  DAlphaVar 0.0\n\
  DRotation 0.0\n\
  DRotationVar 0.0\n\
  InitialBlast 0\n\
  SpewRate 10.0\n\
  SpewTimeLimit 0.0\n\
  Gravity -9.8\n\
  TexFrameStart 0\n\
  TexFrameEnd 0\n\
  BirthFlags 0\n\
  Height 0.0\n\
  Intensity 1.0\n\
  Color -167772161\n\
  SmokeOffset2 0.0 0.5 1.0\n\
  DoublePivot 0\n\
}\n";

const STUCK: &str = "type: a\n\
vehStuck {\n\
  Turn 1.570796\n\
  Translation 0.1\n\
  TimeThresh 1.0\n\
  Rotation 0.0\n\
  PosThresh 0.5\n\
  MoveThresh 0.05\n\
}\n";

const GYRO: &str = "type: a\n\
vehGyro {\n\
  Spin180 3.0\n\
  Reverse180 3.0\n\
  Drift 0.1\n\
}\n";

fn vehcarsim() -> String {
    let wheel = |name: &str| {
        format!(
            "  {name} {{\n    SuspensionExtent 0.2\n    SuspensionLimit 0.05\n    SuspensionFactor 1.0\n    SuspensionDampCoef 0.1\n    SteeringLimit 0.5\n    BrakeCoef 0.14\n    TireDispLimitLong 0.075\n    TireDampCoefLong 0.75\n    TireDragCoefLong 0.01\n    TireDispLimitLat 0.075\n    TireDampCoefLat 0.75\n    TireDragCoefLat 0.02\n    OptimumSlipPercent 0.05\n    StaticFric 3.0\n    SlidingFric 2.95\n  }}\n"
        )
    };
    format!(
        "type: a\nvehCarSim {{\n  Mass 1000.0\n  InertiaBox 2.0 1.3 3.0\n  DrivetrainType 0\n  Aero {{\n    Drag 0.5\n    Down 0.0\n  }}\n  Engine {{\n    MaxHorsePower 200.0\n    IdleRPM 750.0\n    OptRPM 5800.0\n    MaxRPM 8500.0\n  }}\n  Trans {{\n    AutoNumGears 4\n    Reverse 20.0\n    Low 20.0\n    High 75.0\n  }}\n{}{}}}\n",
        wheel("WheelFront"),
        wheel("WheelBack"),
    )
}

fn quad_geo(c: [f32; 3], hx: f32, hy: f32, hz: f32) -> Vec<u8> {
    let mut geo = Vec::new();
    geo.extend_from_slice(&1u32.to_le_bytes());
    geo.extend_from_slice(&4u32.to_le_bytes());
    geo.extend_from_slice(&6u32.to_le_bytes());
    geo.extend_from_slice(&1u32.to_le_bytes());
    geo.extend_from_slice(&0x112u32.to_le_bytes());
    geo.extend_from_slice(&0u16.to_le_bytes());
    geo.extend_from_slice(&(-1i32).to_le_bytes());
    geo.extend_from_slice(&3i32.to_le_bytes());
    geo.extend_from_slice(&4u32.to_le_bytes());
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
    geo.extend_from_slice(&6u32.to_le_bytes());
    for i in [0u16, 1, 2, 0, 3, 1] {
        geo.extend_from_slice(&i.to_le_bytes());
    }
    geo
}

/// PKG3 with body, four wheels and one `break01` chunk.
fn car_pkg() -> Vec<u8> {
    let mut d = b"PKG3".to_vec();
    let chunks: &[(&str, Vec<u8>)] = &[
        ("body_h", quad_geo([0.0, 0.5, 0.0], 0.9, 0.5, 1.6)),
        ("whl0_h", quad_geo([0.8, 0.3, -1.3], 0.15, 0.3, 0.15)),
        ("whl1_h", quad_geo([-0.8, 0.3, -1.3], 0.15, 0.3, 0.15)),
        ("whl2_h", quad_geo([0.8, 0.3, 1.3], 0.15, 0.3, 0.15)),
        ("whl3_h", quad_geo([-0.8, 0.3, 1.3], 0.15, 0.3, 0.15)),
        ("break01_h", quad_geo([0.0, 0.4, -1.6], 0.4, 0.3, 0.05)),
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

/// 48-byte transform record (bounds + pivot + origin).
fn mtx() -> Vec<u8> {
    let mut d = Vec::new();
    for f in [
        -0.15f32, -0.3, -0.15, 0.15, 0.3, 0.15, 0.0, 0.0, 0.0, 0.8, 0.3, -1.3,
    ] {
        d.extend_from_slice(&f.to_le_bytes());
    }
    d
}

/// A synthetic install: `vpt` fully equipped (damage/stuck/gyro plus a
/// `break01` pkg chunk with its `.mtx` and banger record, and a
/// `break02` banger record with no geometry), `vpbroke` with a
/// malformed `vehcardamage`, `vpnaked` with no damage records at all,
/// and `vporphan` carrying damage records outside the catalog.
fn install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    for id in ["vpt", "vpbroke", "vpnaked"] {
        write_str(
            d,
            &format!("tune/{id}.info"),
            "Description=Test Car\nColors=Red\n",
        );
        write_str(d, &format!("tune/vehicle/{id}.vehcarsim"), &vehcarsim());
        write(d, &format!("geometry/{id}.pkg"), &car_pkg());
        write_str(d, &format!("bound/{id}_bound.bnd"), &car_bnd());
        write(d, &format!("geometry/{id}_whl0.mtx"), &mtx());
    }
    for ext in ["vehcardamage", "vehstuck", "vehgyro"] {
        let body = match ext {
            "vehcardamage" => CARDAMAGE,
            "vehstuck" => STUCK,
            _ => GYRO,
        };
        write_str(d, &format!("tune/vehicle/vpt.{ext}"), body);
        write_str(d, &format!("tune/vehicle/vpbroke.{ext}"), body);
    }
    // vpbroke's damage record resolves but does not decode.
    write_str(
        d,
        "tune/vehicle/vpbroke.vehcardamage",
        "type: a\nvehCarDamage {\n  MaxDamage soon\n}\n",
    );
    // The orphan: damage records with no catalog membership at all.
    write_str(d, "tune/vehicle/vporphan.vehcardamage", CARDAMAGE);

    // Breakaway inventory on vpt: break01 binds through the pkg chunk
    // and an mtx; break02 is a banger record with no geometry.
    write(d, "geometry/vpt_break01.mtx", &mtx());
    write_str(
        d,
        "tune/banger/vpt_break01.dgbangerdata",
        "type: a\ndgBangerData {\n  Mass 50.0\n}\n",
    );
    write_str(
        d,
        "tune/banger/vpt_break02.dgbangerdata",
        "type: a\ndgBangerData {\n  Mass 50.0\n}\n",
    );
    tmp
}

#[test]
fn the_audit_counts_every_discovered_record() {
    let tmp = install();
    let vfs = vfs_of(tmp.path());
    let audit = DamageAudit::scan(&vfs);

    let vpt = audit.assets.iter().find(|a| a.id == "vpt").unwrap();
    assert_eq!(vpt.damage, AssetCheck::Parsed);
    assert_eq!(vpt.stuck, AssetCheck::Parsed);
    assert_eq!(vpt.gyro, AssetCheck::Parsed);
    assert!(vpt.issues.is_empty(), "{:?}", vpt.issues);

    // Breakaway inventory: the pkg chunk, the mtx part and both banger
    // records; break02 is a dead authored fragment.
    assert_eq!(
        vpt.break_chunks.as_deref(),
        Some(["break01".to_string()].as_slice())
    );
    assert_eq!(vpt.break_mtx, ["break01"]);
    assert_eq!(vpt.break_records, ["break01", "break02"]);
    assert_eq!(vpt.dead_breaks, ["break02"]);

    let vpbroke = audit.assets.iter().find(|a| a.id == "vpbroke").unwrap();
    assert!(matches!(vpbroke.damage, AssetCheck::Failed(_)));
    assert_eq!(vpbroke.stuck, AssetCheck::Parsed);

    let vpnaked = audit.assets.iter().find(|a| a.id == "vpnaked").unwrap();
    assert_eq!(vpnaked.damage, AssetCheck::Missing);
    assert_eq!(vpnaked.stuck, AssetCheck::Missing);

    // The orphan record stays in the denominator, not filtered out.
    assert_eq!(audit.uncatalogued.len(), 1);
    assert_eq!(audit.uncatalogued[0].id, "vporphan");
    assert_eq!(audit.uncatalogued[0].status, AssetCheck::Parsed);

    // Exactly one failure: the malformed vehcardamage. Missing records
    // and dead fragments are findings, not failures.
    let failures = audit.failures();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("vpbroke"));
    assert!(
        audit
            .diagnostics
            .iter()
            .any(|d| d.contains("vpt") && d.contains("break02")),
        "{:?}",
        audit.diagnostics
    );
}

#[test]
fn load_vehicle_attaches_the_records_with_provenance() {
    let tmp = install();
    let vfs = vfs_of(tmp.path());
    let def = load_vehicle(&vfs, "vpt", 0).unwrap();
    let d = def.damage.as_ref().expect("vehcardamage attaches");
    assert_eq!(d.max_damage, 321300.0);
    assert_eq!(d.med_damage, 150000.0);
    assert!(def.stuck.is_some());
    assert!(def.gyro.is_some());
    for ext in ["vehcardamage", "vehstuck", "vehgyro"] {
        assert!(
            def.sources.iter().any(|s| s.contains(ext)),
            "missing {ext} in {:?}",
            def.sources
        );
    }
}

#[test]
fn a_malformed_record_warns_instead_of_sinking_the_load() {
    let tmp = install();
    let vfs = vfs_of(tmp.path());
    let def = load_vehicle(&vfs, "vpbroke", 0).unwrap();
    assert!(def.damage.is_none());
    assert!(def.stuck.is_some());
    assert!(
        def.report
            .warnings
            .iter()
            .any(|w| w.contains("vehcardamage")),
        "{:?}",
        def.report.warnings
    );
    // The resolved-but-broken file still records provenance.
    assert!(
        def.sources.iter().any(|s| s.contains("vehcardamage")),
        "{:?}",
        def.sources
    );

    // No fabricated record on an authored absence either.
    let def = load_vehicle(&vfs, "vpnaked", 0).unwrap();
    assert!(def.damage.is_none());
    assert!(def.stuck.is_none());
    assert!(def.gyro.is_none());
}
