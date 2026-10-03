//! Vehicle catalog metadata: the vehicle-select comparison figures
//! read from `tune/<id>.info`.

use std::path::Path;

use mm2_assets::Vfs;
use mm2_content::{DisplayStats, VehicleCatalog};

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

/// The retail layout: `Top Speed` carries a space, values may be
/// padded with tabs, and a missing or garbled figure stays `None`
/// rather than reading as zero.
#[test]
fn display_stats_read_verbatim_from_info() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "tune/vpbug.info",
        "Description=VW New Beetle\r\nHorsepower=150\t\r\nTop Speed=91 \t\r\n\
         Durability=760000\r\nMass=4250\r\n",
    );
    write(
        tmp.path(),
        "tune/vpodd.info",
        "Description=Odd\r\nHorsepower=lots\r\nMass=900\r\n",
    );
    let catalog = VehicleCatalog::scan(&vfs_of(tmp.path()));

    let bug = catalog.find("vpbug").unwrap();
    assert_eq!(
        bug.stats,
        DisplayStats {
            horsepower: Some(150.0),
            top_speed: Some(91.0),
            durability: Some(760000.0),
            mass: Some(4250.0),
        }
    );
    let odd = catalog.find("vpodd").unwrap();
    assert_eq!(
        odd.stats,
        DisplayStats {
            horsepower: None,
            top_speed: None,
            durability: None,
            mass: Some(900.0),
        }
    );
}
