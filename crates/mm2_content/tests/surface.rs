//! F06-A surface-table producer tests: `load_surface_tables` resolves
//! the `city/materials.{mtl,csv}` pair through the real VFS and
//! `SurfaceTables` classifies PSDL texture names — synthetic mounts,
//! no original data.

use std::path::Path;

use mm2_assets::Vfs;
use mm2_content::surface::{CSV_PATH, MTL_PATH, SurfaceLoadError, load_surface_tables};
use mm2_content::{PsdlSurfaces, SurfaceSlot};
use mm2_game::SurfaceMaterial;
use mm2_vehicle::TireSurface;

fn write(dir: &Path, rel: &str, contents: &[u8]) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

/// A complete table pair: `_default`, `cobblestone`, `grass` (indices
/// 0, 1, 2); the csv names two materials, one `none` row, one dead ref
/// (`ash` — the retail `transbay_ramp_f` pattern) and one animated
/// sequence base name.
const MTL: &str = "\
mtl _default {
    elasticity: 0.0
    friction: 1.0
    effect: none
    sound: 0
    drag: 0.0
    width: 0.0
    height: 0.0
    depth: 0.0
    ptxindex: 0 0
    ptxthreshold: 0.0 0.0
}
mtl cobblestone {
    elasticity: 0.1
    friction: 0.9
    effect: none
    sound: 0
    drag: 0.0
    width: 0.0
    height: 0.0
    depth: 0.0
    ptxindex: 0 0
    ptxthreshold: 0.0 0.0
}
mtl grass {
    elasticity: 0.4
    friction: 0.7
    effect: none
    sound: 2
    drag: 0.0
    width: 0.0
    height: 0.0
    depth: 0.0
    ptxindex: 0 0
    ptxthreshold: 0.0 0.0
}
";

const CSV: &str = "\
texture,physics
test_road,cobblestone
test_grass,grass
test_none,none
dead_ref,ash
anim_seq,grass
";

fn mount_with(dir: &Path, files: &[(&str, &str)]) -> Vfs {
    for (rel, contents) in files {
        write(dir, rel, contents.as_bytes());
    }
    vfs_of(dir)
}

#[test]
fn a_complete_pair_loads_and_classifies_every_slot_kind() {
    let dir = tempfile::tempdir().unwrap();
    let vfs = mount_with(dir.path(), &[(MTL_PATH, MTL), (CSV_PATH, CSV)]);
    let tables = load_surface_tables(&vfs)
        .expect("pair parses")
        .expect("pair is present");

    assert_eq!(tables.set.defs.len(), 3);
    // Named rows carry the authored def index; `none` and blank slots
    // are authored data, not defects; absent names and dead refs fall
    // into the recorded `Unmapped` class.
    assert_eq!(tables.slot_for("test_road"), SurfaceSlot::Material(1));
    assert_eq!(tables.slot_for("test_grass"), SurfaceSlot::Material(2));
    assert_eq!(tables.slot_for("test_none"), SurfaceSlot::Default);
    assert_eq!(tables.slot_for(""), SurfaceSlot::Blank);
    assert_eq!(tables.slot_for("mystery"), SurfaceSlot::Unmapped);
    // `dead_ref` names a material the mtl never defines — unmapped,
    // not silently the named surface it cannot point at.
    assert_eq!(tables.slot_for("dead_ref"), SurfaceSlot::Unmapped);
    // Animated-frame fallback: the csv keys the sequence's base stem
    // while the PSDL table references single frames (exactly four
    // digits — `anim_seq-001` stays unmapped).
    assert_eq!(tables.slot_for("anim_seq-0001"), SurfaceSlot::Material(2));
    assert_eq!(tables.slot_for("anim_seq-001"), SurfaceSlot::Unmapped);
}

#[test]
fn resolve_psdl_reports_each_slot_and_the_unmapped_set() {
    let dir = tempfile::tempdir().unwrap();
    let vfs = mount_with(dir.path(), &[(MTL_PATH, MTL), (CSV_PATH, CSV)]);
    let tables = load_surface_tables(&vfs).unwrap().unwrap();

    let names = [
        "test_road".to_string(),
        "test_none".to_string(),
        String::new(),
        "mystery".to_string(),
    ];
    let PsdlSurfaces { slots, unmapped } = tables.resolve_psdl(&names);
    assert_eq!(
        slots,
        vec![
            SurfaceSlot::Material(1),
            SurfaceSlot::Default,
            SurfaceSlot::Blank,
            SurfaceSlot::Unmapped,
        ]
    );
    assert_eq!(
        unmapped.iter().map(String::as_str).collect::<Vec<_>>(),
        ["mystery"]
    );
    // The one dead csv row is the pair's only consistency issue.
    assert_eq!(tables.issues(), 1);
}

/// A table whose `_default` friction is not `1.0`, so normalization
/// has a visible divisor: `slick` (0.4/0.8 = 0.5), `tacky` (1.2/0.8 =
/// 1.5), `fieldless` (no `friction` line → neutral), `negative`
/// (invalid → neutral).
const MTL_SCALED: &str = "\
mtl _default {
    elasticity: 0.0
    friction: 0.8
    effect: none
    sound: 0
    drag: 0.0
    width: 0.0
    height: 0.0
    depth: 0.0
    ptxindex: 0 0
    ptxthreshold: 0.0 0.0
}
mtl slick {
    elasticity: 0.1
    friction: 0.4
    effect: none
    sound: 0
    drag: 0.0
    width: 0.0
    height: 0.0
    depth: 0.0
    ptxindex: 0 0
    ptxthreshold: 0.0 0.0
}
mtl tacky {
    elasticity: 0.1
    friction: 1.2
    effect: none
    sound: 0
    drag: 0.0
    width: 0.0
    height: 0.0
    depth: 0.0
    ptxindex: 0 0
    ptxthreshold: 0.0 0.0
}
mtl fieldless {
    elasticity: 0.1
    effect: none
    sound: 0
    drag: 0.0
    width: 0.0
    height: 0.0
    depth: 0.0
    ptxindex: 0 0
    ptxthreshold: 0.0 0.0
}
mtl negative {
    elasticity: 0.1
    friction: -0.5
    effect: none
    sound: 0
    drag: 0.0
    width: 0.0
    height: 0.0
    depth: 0.0
    ptxindex: 0 0
    ptxthreshold: 0.0 0.0
}
";

#[test]
fn tire_surface_normalizes_friction_against_the_default_material() {
    let dir = tempfile::tempdir().unwrap();
    let vfs = mount_with(dir.path(), &[(MTL_PATH, MTL), (CSV_PATH, CSV)]);
    let tables = load_surface_tables(&vfs).unwrap().unwrap();

    // `_default` friction 1.0 is the reference: authored values land
    // as relative grip scales.
    assert_eq!(tables.tire_surface(0).grip, 1.0);
    assert!((tables.tire_surface(1).grip - 0.9).abs() < 1e-6);
    assert!((tables.tire_surface(2).grip - 0.7).abs() < 1e-6);
    // An out-of-range index resolves to the neutral reference, never
    // panics or guesses.
    assert_eq!(tables.tire_surface(99).grip, 1.0);
    // `Unspecified` carries no `TireSurface` — an unmarked collider is
    // the neutral surface.
    assert_eq!(tables.tire_surface_for(SurfaceMaterial::Unspecified), None);
    assert_eq!(
        tables.tire_surface_for(SurfaceMaterial::Authored(2)),
        Some(TireSurface { grip: 0.7 })
    );

    // A non-1.0 `_default` divides visibly: slick halves, tacky grows.
    let dir = tempfile::tempdir().unwrap();
    let vfs = mount_with(
        dir.path(),
        &[(MTL_PATH, MTL_SCALED), (CSV_PATH, "texture,physics\n")],
    );
    let tables = load_surface_tables(&vfs).unwrap().unwrap();
    assert_eq!(tables.tire_surface(0).grip, 1.0, "_default itself");
    assert!((tables.tire_surface(1).grip - 0.5).abs() < 1e-6, "slick");
    assert!((tables.tire_surface(2).grip - 1.5).abs() < 1e-6, "tacky");
    // A missing or invalid `friction` — both flagged by `issues()` —
    // resolves neutral rather than guessed.
    assert_eq!(tables.tire_surface(3).grip, 1.0, "fieldless");
    assert_eq!(tables.tire_surface(4).grip, 1.0, "negative");
    assert!(tables.issues() > 0);

    // Without a `_default` block the authored values apply raw.
    let dir = tempfile::tempdir().unwrap();
    let vfs = mount_with(
        dir.path(),
        &[
            (
                MTL_PATH,
                MTL_SCALED
                    .replacen("mtl _default", "mtl fallback", 1)
                    .as_str(),
            ),
            (CSV_PATH, "texture,physics\n"),
        ],
    );
    let tables = load_surface_tables(&vfs).unwrap().unwrap();
    assert!(
        (tables.tire_surface(1).grip - 0.4).abs() < 1e-6,
        "slick applies raw without a _default"
    );
}

#[test]
fn an_absent_pair_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let vfs = vfs_of(dir.path());
    assert!(load_surface_tables(&vfs).unwrap().is_none());
}

#[test]
fn a_half_pair_is_a_missing_error_not_an_absent_feature() {
    let dir = tempfile::tempdir().unwrap();
    let vfs = mount_with(dir.path(), &[(CSV_PATH, CSV)]);
    assert!(matches!(
        load_surface_tables(&vfs),
        Err(SurfaceLoadError::Missing(MTL_PATH))
    ));

    let dir = tempfile::tempdir().unwrap();
    let vfs = mount_with(dir.path(), &[(MTL_PATH, MTL)]);
    assert!(matches!(
        load_surface_tables(&vfs),
        Err(SurfaceLoadError::Missing(CSV_PATH))
    ));
}

#[test]
fn an_unparseable_half_fails_the_pair() {
    let dir = tempfile::tempdir().unwrap();
    let vfs = mount_with(
        dir.path(),
        &[(MTL_PATH, "not an mtl file {"), (CSV_PATH, CSV)],
    );
    assert!(matches!(
        load_surface_tables(&vfs),
        Err(SurfaceLoadError::Parse(_))
    ));
}
