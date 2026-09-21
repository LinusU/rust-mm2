//! F10-A.1 ambient-traffic producer/audit tests: synthetic installs
//! through the real VFS — `city/<city>.aimap` rosters, `aivehicledata`
//! decode, per-class asset checks, unrostered files and event-level
//! override discovery.

use std::path::Path;

use mm2_assets::Vfs;
use mm2_content::{AssetCheck, TrafficAudit, ambient_roster};

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

const TUNE: &str = "type: a\n\
aiVehicleData {\n\
  Mass 500.0\n\
  Size 2.0 1.5 5.0\n\
  MaxAng 0.0 0.0 0.0\n\
  Elasticity 0.9\n\
  Friction 0.15\n\
  MaxDamage 70000.0\n\
  PtxThresh 70000.0\n\
  Spring 17000.0\n\
  Damping 1200.0\n\
  Limit 0.07\n\
  RubberSpring 12000.0\n\
  RubberDamp 600.0\n\
  CG 0.0 0.5 0.0\n\
}\n";

/// Minimal parseable records: a `PKG3` header and a one-quad BND.
const PKG: &[u8] = b"PKG3";
const BND: &str = "version: 1.01\n\
verts: 4\n\
materials: 0\n\
edges: 0\n\
polys: 1\n\
\n\
v\t0.0\t0.0\t0.0\n\
v\t1.0\t0.0\t0.0\n\
v\t1.0\t1.0\t0.0\n\
v\t0.0\t1.0\t0.0\n\
\n\
quad 0 1 2 3 0\n";

fn aimap(rows: &str) -> String {
    let n = rows.lines().filter(|l| !l.trim().is_empty()).count();
    format!("[Ambient Types/Density]\n{n}\n{rows}")
}

/// A complete `va_*` asset set for `id`.
fn ambient_assets(dir: &Path, id: &str) {
    write_str(dir, &format!("tune/vehicle/{id}.aivehicledata"), TUNE);
    write(dir, &format!("geometry/{id}.pkg"), PKG);
    write_str(dir, &format!("bound/{id}_bound.bnd"), BND);
    write_str(dir, &format!("geometry/{id}_whl0.mtx"), "");
}

#[test]
fn roster_builds_from_city_aimap_and_resolves_tuning() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write_str(
        d,
        "city/sf.aimap",
        &aimap("va_bus_f 0.25 0\nva_taxi_f 0.75 0\nva_bus_f 1.0 0\n"),
    );
    ambient_assets(d, "va_bus_f");
    ambient_assets(d, "va_taxi_f");

    let vfs = vfs_of(d);
    let (roster, diagnostics) = ambient_roster(&vfs, "sf").unwrap().unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert!(roster.issues.is_empty(), "{:?}", roster.issues);
    assert_eq!(roster.entries.len(), 3);
    // Repeat ids keep their own weight bands (london's va_compact_s).
    assert_eq!(roster.entries[0].id, "va_bus_f");
    assert_eq!(roster.entries[2].id, "va_bus_f");
    let t = roster.entries[1].tuning.as_ref().unwrap();
    assert_eq!(t.mass, 500.0);
    assert_eq!(t.cg, Some([0.0, 0.5, 0.0]));
}

#[test]
fn missing_and_malformed_tuning_stay_in_the_table() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write_str(d, "city/sf.aimap", &aimap("va_ghost 0.5 0\nva_bad 1.0 0\n"));
    // va_ghost: nothing resolves. va_bad: garbage at the path.
    write_str(d, "tune/vehicle/va_bad.aivehicledata", "not a tune file");

    let vfs = vfs_of(d);
    let (roster, diagnostics) = ambient_roster(&vfs, "sf").unwrap().unwrap();
    assert_eq!(roster.entries.len(), 2);
    assert!(!roster.spawnable(0));
    assert!(!roster.spawnable(1));
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    assert!(diagnostics.iter().any(|d| d.contains("va_ghost")));
    assert!(diagnostics.iter().any(|d| d.contains("va_bad")));
}

#[test]
fn audit_reports_assets_unrostered_and_missing_expected() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write_str(d, "city/sf.aimap", &aimap("va_ok 1.0 0\n"));
    ambient_assets(d, "va_ok");
    // Discovered but unrostered ambient file.
    ambient_assets(d, "va_extra");
    // Rostered id with a broken bound and no geometry.
    write_str(d, "city/sf.aimap", &aimap("va_ok 0.5 0\nva_broke 1.0 0\n"));
    write_str(d, "tune/vehicle/va_broke.aivehicledata", TUNE);
    write_str(d, "bound/va_broke_bound.bnd", "version: 9\njunk\n");

    let vfs = vfs_of(d);
    let audit = TrafficAudit::scan(&vfs, "sf");
    assert!(audit.aimap_present);
    assert_eq!(audit.assets.len(), 2);
    let broke = audit.assets.iter().find(|a| a.id == "va_broke").unwrap();
    assert_eq!(broke.tuning, AssetCheck::Parsed);
    assert_eq!(broke.geometry, AssetCheck::Missing);
    assert!(matches!(broke.bound, AssetCheck::Failed(_)));
    assert_eq!(audit.unrostered, vec!["va_extra".to_string()]);
    assert!(
        !audit.missing_expected.is_empty(),
        "synthetic install lacks the stock pool"
    );
    let failures = audit.failures();
    assert!(failures.iter().any(|f| f.contains("va_broke")));
    assert!(
        failures
            .iter()
            .any(|f| f.contains("expected stock ambient"))
    );
}

#[test]
fn audit_finds_event_overrides_and_bad_event_aimaps() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write_str(d, "city/sf.aimap", &aimap("va_ok 1.0 0\n"));
    ambient_assets(d, "va_ok");
    write_str(
        d,
        "race/sf/race0.aimap",
        "[Exceptions]\n2\n12 0.0 0.0\n15 0.0 0.0\n[Density]\n0.35\n",
    );
    write_str(d, "race/sf/blitz0.aimap_p", "# no ambient overrides\n");
    write_str(d, "race/sf/broke.aimap", "[Density\nunbalanced");

    let vfs = vfs_of(d);
    let audit = TrafficAudit::scan(&vfs, "sf");
    let race0 = audit
        .event_overrides
        .iter()
        .find(|o| o.logical == "race/sf/race0.aimap")
        .expect("race0 carries exceptions + density");
    assert_eq!(race0.exceptions, 2);
    assert_eq!(race0.density, Some(0.35));
    assert!(
        !audit
            .event_overrides
            .iter()
            .any(|o| o.logical == "race/sf/blitz0.aimap_p"),
        "no ambient sections → not an override"
    );
    assert!(
        audit
            .event_overrides
            .iter()
            .any(|o| o.logical == "race/sf/broke.aimap" && o.failed.is_some()),
        "unparseable event aimap is reported, not hidden"
    );
}

#[test]
fn audit_missing_city_aimap_reports_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let vfs = vfs_of(tmp.path());
    let audit = TrafficAudit::scan(&vfs, "sf");
    assert!(!audit.aimap_present);
    assert!(audit.roster.is_none());
    assert!(
        audit
            .failures()
            .iter()
            .any(|f| f.contains("does not resolve"))
    );
}
