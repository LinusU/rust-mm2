//! Garage selectability production tests (F16-B.3): `garage_table`
//! folds the vehicle catalog plus every city's reward table into
//! `mm2_game`'s `GarageTable`, and `scan_garage` composes both over a
//! mounted VFS. Rows that cannot become a gate surface as
//! diagnostics, never silence.

use std::path::Path;

use mm2_assets::Vfs;
use mm2_content::{VehicleCatalog, garage_table, scan_garage};
use mm2_game::{
    EventTableKind, PaintGate, RewardRequirement, RewardRule, RewardTable, Unlock, VehicleGate,
};

const REWARDS: &str = "RaceType,RaceNum,CarName,VariantNum (zero if it unlocks a car),\n";

fn write(dir: &Path, rel: &str, contents: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

fn info(colors: &str) -> String {
    format!("Description=test\nColors={colors}\nUnlockScore=0\nUnlockFlags=0\n")
}

/// A milestone-shaped rule carrying `unlock` — `garage_table` reads
/// only the unlock, so the requirement is a placeholder.
fn grant(unlock: Unlock) -> RewardRule {
    RewardRule {
        family: EventTableKind::Blitz,
        requirement: RewardRequirement::All,
        unlock,
        message: "unlocked".to_string(),
        line: 2,
    }
}

fn rewards(rules: Vec<RewardRule>) -> RewardTable {
    RewardTable {
        milestones: rules,
        ..RewardTable::default()
    }
}

/// A synthetic install: two canonical `.info` cars (vpa with three
/// paints, vpb with one), a `.inf` fallback entry (the vpmoonrover
/// shape), and a pkg-only leftover with no metadata at all.
fn install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "tune/vpa.info", &info("One|Two|Three"));
    write(d, "tune/vpb.info", &info("Solo"));
    write(d, "tune/vpd.inf", &info("Rover"));
    write(d, "geometry/vpe.pkg", "not parsed — presence is enough");
    tmp
}

/// VEH-3/VEH-4: a `vehicle:` grant gates the car; a `paint:` grant
/// gates exactly that paint index (the authored VariantNum measured as
/// the zero-based Colors index — vpvwcup variant 5 = "Team Angel").
/// Fallback/metadata-less entries stay unlisted but keep their rows.
#[test]
fn authored_grants_gate_the_right_rows() {
    let tmp = install();
    let catalog = VehicleCatalog::scan(&vfs_of(tmp.path()));
    let table = garage_table(
        &catalog,
        &[&rewards(vec![
            grant(Unlock::Vehicle("vpb".to_string())),
            grant(Unlock::Paint {
                car: "vpa".to_string(),
                variant: 2,
            }),
        ])],
    );

    let vpa = table.row("vpa").unwrap();
    assert_eq!(vpa.gate, VehicleGate::Open);
    assert_eq!(
        vpa.paint_gates,
        vec![PaintGate::Open, PaintGate::Open, PaintGate::Reward]
    );
    assert!(vpa.listed);

    let vpb = table.row("vpb").unwrap();
    assert_eq!(vpb.gate, VehicleGate::Reward);
    assert_eq!(vpb.paint_gates, vec![PaintGate::Open]);

    // `.inf` fallback metadata and no metadata at all: cataloged,
    // evaluated, but not on the select roster.
    assert!(!table.row("vpd").unwrap().listed);
    assert!(!table.row("vpe").unwrap().listed);
    assert!(table.diagnostics.is_empty());
}

/// Grants aimed outside the catalog — an unknown vehicle, a paint
/// index the metadata never declared — are diagnostics, not gates and
/// not panics.
#[test]
fn grants_off_the_catalog_diagnose() {
    let tmp = install();
    let catalog = VehicleCatalog::scan(&vfs_of(tmp.path()));
    let table = garage_table(
        &catalog,
        &[&rewards(vec![
            grant(Unlock::Vehicle("vpghost".to_string())),
            grant(Unlock::Paint {
                car: "vpa".to_string(),
                variant: 7,
            }),
            grant(Unlock::Paint {
                car: "vpa".to_string(),
                variant: -1,
            }),
            grant(Unlock::Paint {
                car: "vpghost".to_string(),
                variant: 0,
            }),
        ])],
    );

    assert_eq!(table.row("vpa").unwrap().gate, VehicleGate::Open);
    assert!(
        table
            .row("vpa")
            .unwrap()
            .paint_gates
            .iter()
            .all(|g| *g == PaintGate::Open)
    );
    // vpghost is diagnosed twice — once per grant kind — plus the two
    // out-of-range variants on vpa.
    assert_eq!(table.diagnostics.len(), 4);
    assert!(
        table
            .diagnostics
            .iter()
            .filter(|d| d.contains("vpghost"))
            .count()
            == 2
    );
    assert!(table.diagnostics.iter().any(|d| d.contains("variant 7")));
    assert!(table.diagnostics.iter().any(|d| d.contains("variant -1")));
}

/// `scan_garage` unions every discovered race city's reward table —
/// a car won in one city must open in the other's garage — and folds
/// reward-table diagnostics into the garage's.
#[test]
fn scan_garage_unions_every_citys_rewards() {
    let tmp = install();
    let d = tmp.path();
    write(
        d,
        "race/citya/citya_rewards.csv",
        &format!("{REWARDS}blitz,half,vpb,0,half the blitz races,\n"),
    );
    write(
        d,
        "race/cityb/cityb_rewards.csv",
        &format!("{REWARDS}circuit,all,vpa,1,every circuit,\n"),
    );
    let table = scan_garage(&vfs_of(d));

    assert_eq!(table.row("vpb").unwrap().gate, VehicleGate::Reward);
    assert_eq!(
        table.row("vpa").unwrap().paint_gates,
        vec![PaintGate::Open, PaintGate::Reward, PaintGate::Open]
    );
    assert!(table.diagnostics.is_empty());
}
