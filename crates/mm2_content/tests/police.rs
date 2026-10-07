//! `CatalogEvent → PoliceRoster` producer tests (F20-A.1): synthetic
//! `race/<city>/` installs through the real VFS and catalog, built by
//! the production `police_roster` / `cruise_police_roster`.

use std::path::Path;

use bevy::prelude::Vec3;
use mm2_assets::Vfs;
use mm2_content::{
    EventCatalog, PoliceBuild, PoliceReport, RosterBuildError, cruise_police_roster, police_roster,
};
use mm2_game::{Difficulty, EventRef, EventTableKind, PoliceIssue};

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";

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

fn event(catalog: &EventCatalog, index: usize) -> &mm2_content::CatalogEvent {
    catalog
        .get(&EventRef {
            city: catalog.city.clone(),
            table: EventTableKind::Checkpoint,
            index,
        })
        .expect("test event resolves")
}

/// Amateur / professional `Cops` columns on a checkpoint table row.
fn table_row(amateur_cops: i64, pro_cops: i64) -> String {
    format!("none,0,0,0,0,{amateur_cops},0.1,0.0,1,50,1,0,0,0,0,{pro_cops},0.2,0.0,1,40,1\n")
}

/// A `[Police]` section from `(x, y, z, tail)` rows.
fn police_section(rows: &[(&str, f32, f32, f32, &str)]) -> String {
    let mut s = format!("[Police]\n{}\n", rows.len());
    for (geo, x, y, z, tail) in rows {
        s.push_str(&format!("{geo} {x} {y} {z} {tail}\n"));
    }
    s
}

/// A one-event `race/london/` install plus the caller's files.
fn install(
    amateur_cops: i64,
    pro_cops: i64,
    files: &[(&str, String)],
) -> (tempfile::TempDir, EventCatalog, Vfs) {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/london/mmracedata.csv",
        &format!("{MM_HEADER}\n{}", table_row(amateur_cops, pro_cops)),
    );
    write(d, "race/london/race0.aimap", "#\n");
    write(
        d,
        "race/london/race0waypoints.csv",
        &format!(
            "{WAYPOINTS}0,0,0,90,15,0,0,0,\n10,0,-30,90,8,0,0,0,\n40,0,-60,90,9,0,0,0,\n70,0,-90,90,10,0,0,0,\n"
        ),
    );
    for (rel, contents) in files {
        write(d, &format!("race/london/{rel}"), contents);
    }
    let vfs = vfs_of(d);
    let catalog = EventCatalog::scan(&vfs, "london");
    (tmp, catalog, vfs)
}

#[test]
fn amateur_roster_wires_position_heading_and_raw_tail() {
    let section = police_section(&[
        ("vpcop", 10.0, 1.0, -20.0, "-110 0 15 0.5 50"),
        ("vpcop", 30.0, 2.0, 40.0, "535 0 15 0.5 50"),
    ]);
    let (_t, catalog, vfs) = install(2, 0, &[("race0.aimap", section)]);
    let roster =
        police_roster(&vfs, event(&catalog, 0), Difficulty::Amateur).expect("roster builds");

    assert!(roster.issues.is_empty(), "{:?}", roster.issues);
    assert_eq!(roster.entries.len(), 2);
    let first = &roster.entries[0];
    assert_eq!(first.vehicle, "vpcop");
    assert_eq!(first.position, Vec3::new(10.0, 1.0, -20.0));
    assert_eq!(first.heading_deg, Some(-110.0));
    assert_eq!(first.params, [-110.0, 0.0, 15.0, 0.5, 50.0], "tail raw");
    // The authored 535 is an unnormalised 175° yaw; the raw value stays.
    let second = &roster.entries[1];
    assert_eq!(second.heading_deg, Some(175.0));
    assert_eq!(second.params[0], 535.0);
    assert_eq!(roster.placeable().count(), 2);
}

#[test]
fn professional_prefers_aimap_p_and_amateur_does_not_read_it() {
    let (_t, catalog, vfs) = install(
        1,
        3,
        &[
            (
                "race0.aimap",
                police_section(&[("vpcop", 0.0, 0.0, 0.0, "0 0")]),
            ),
            (
                "race0.aimap_p",
                police_section(&[
                    ("vpcop", 1.0, 0.0, 0.0, "0 0"),
                    ("vpcop", 2.0, 0.0, 0.0, "0 0"),
                    ("vpcop", 3.0, 0.0, 0.0, "0 0"),
                ]),
            ),
        ],
    );
    let am = police_roster(&vfs, event(&catalog, 0), Difficulty::Amateur).unwrap();
    let pro = police_roster(&vfs, event(&catalog, 0), Difficulty::Professional).unwrap();
    assert_eq!((am.entries.len(), pro.entries.len()), (1, 3));
    assert!(am.issues.is_empty() && pro.issues.is_empty());
    assert_eq!(pro.entries[2].position.x, 3.0);
    assert_eq!(
        am.entries[0].params.len(),
        2,
        "a two-value tail (evade0 shape) is kept as authored"
    );
}

#[test]
fn a_missing_variant_falls_back_and_says_so() {
    let (_t, catalog, vfs) = install(
        1,
        1,
        &[(
            "race0.aimap",
            police_section(&[("vpcop", 5.0, 0.0, 5.0, "90 0")]),
        )],
    );
    let pro = police_roster(&vfs, event(&catalog, 0), Difficulty::Professional).unwrap();
    assert_eq!(pro.entries.len(), 1, "the only authored lineup is used");
    assert_eq!(
        pro.issues,
        [PoliceIssue::MissingVariant {
            wanted: Difficulty::Professional,
            used: Difficulty::Amateur
        }]
    );
}

#[test]
fn a_table_count_that_disagrees_is_reported_not_repaired() {
    // The table claims 3 cops; the aimap wires 1 — no padding, no trim.
    let (_t, catalog, vfs) = install(
        3,
        0,
        &[(
            "race0.aimap",
            police_section(&[("vpcop", 5.0, 0.0, 5.0, "90 0")]),
        )],
    );
    let roster = police_roster(&vfs, event(&catalog, 0), Difficulty::Amateur).unwrap();
    assert_eq!(roster.entries.len(), 1);
    assert_eq!(
        roster.issues,
        [PoliceIssue::CountMismatch { wired: 1, table: 3 }]
    );
    // And the other direction: wired rows with a zero table.
    let (_t, catalog, vfs) = install(
        0,
        0,
        &[(
            "race0.aimap",
            police_section(&[("vpcop", 5.0, 0.0, 5.0, "90 0")]),
        )],
    );
    let roster = police_roster(&vfs, event(&catalog, 0), Difficulty::Amateur).unwrap();
    assert_eq!(
        roster.issues,
        [PoliceIssue::CountMismatch { wired: 1, table: 0 }]
    );
}

#[test]
fn an_event_with_no_police_section_is_an_empty_roster_that_matches_a_zero_table() {
    let (_t, catalog, vfs) = install(0, 0, &[]);
    let roster = police_roster(&vfs, event(&catalog, 0), Difficulty::Amateur).unwrap();
    assert!(roster.entries.is_empty() && roster.issues.is_empty());
}

#[test]
fn the_chase_distance_scalar_rides_the_roster() {
    let aimap = format!(
        "[CopChaseDistance]\n150.0\n{}",
        police_section(&[("vpcop", 0.0, 0.0, 0.0, "0 0")])
    );
    let (_t, catalog, vfs) = install(1, 0, &[("race0.aimap", aimap)]);
    let roster = police_roster(&vfs, event(&catalog, 0), Difficulty::Amateur).unwrap();
    assert_eq!(roster.chase_distance, Some(150.0));
}

#[test]
fn a_row_with_a_tail_less_position_is_still_a_placeable_slot() {
    let (_t, catalog, vfs) = install(
        1,
        0,
        &[("race0.aimap", "[Police]\n1\nvpcop 1 2 3\n".into())],
    );
    let roster = police_roster(&vfs, event(&catalog, 0), Difficulty::Amateur).unwrap();
    assert_eq!(roster.entries.len(), 1);
    assert_eq!(roster.entries[0].heading_deg, None);
    assert!(roster.entries[0].placeable());
}

#[test]
fn an_event_with_incomplete_records_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/london/mmracedata.csv",
        &format!("{MM_HEADER}\n{}", table_row(0, 0)),
    );
    // An aimap but no waypoints: the catalog marks the event incomplete.
    write(d, "race/london/race0.aimap", &police_section(&[]));
    let vfs = vfs_of(d);
    let catalog = EventCatalog::scan(&vfs, "london");
    let ev = event(&catalog, 0);
    assert!(matches!(
        police_roster(&vfs, ev, Difficulty::Amateur),
        Err(RosterBuildError::NotReady(_))
    ));
}

#[test]
fn cruise_reads_the_roam_record_with_the_same_variant_split() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/london/roam.aimap",
        &police_section(&[("vpcop", 1.0, 0.0, 1.0, "10 0 15 0.5 50")]),
    );
    write(
        d,
        "race/london/roam.aimap_p",
        &police_section(&[
            ("vpcop", 1.0, 0.0, 1.0, "10 0 15 0.5 50"),
            ("vpcop", 2.0, 0.0, 2.0, "20 0 15 0.5 50"),
        ]),
    );
    let vfs = vfs_of(d);
    let am = cruise_police_roster(&vfs, "london", Difficulty::Amateur).unwrap();
    let pro = cruise_police_roster(&vfs, "london", Difficulty::Professional).unwrap();
    assert_eq!((am.entries.len(), pro.entries.len()), (1, 2));
    assert!(am.issues.is_empty() && pro.issues.is_empty());
    assert_eq!(pro.entries[1].heading_deg, Some(20.0));

    // Only the amateur record ships: Professional falls back, noted.
    std::fs::remove_file(d.join("race/london/roam.aimap_p")).unwrap();
    let vfs = vfs_of(d);
    let pro = cruise_police_roster(&vfs, "london", Difficulty::Professional).unwrap();
    assert_eq!(pro.entries.len(), 1);
    assert_eq!(
        pro.issues,
        [PoliceIssue::MissingVariant {
            wanted: Difficulty::Professional,
            used: Difficulty::Amateur
        }]
    );
    // Neither ships: an error, never an empty roster.
    std::fs::remove_file(d.join("race/london/roam.aimap")).unwrap();
    let vfs = vfs_of(d);
    assert!(matches!(
        cruise_police_roster(&vfs, "london", Difficulty::Amateur),
        Err(RosterBuildError::NoAimapRecord)
    ));
}

#[test]
fn the_report_counts_events_cruise_extras_and_unresolved_vehicles() {
    let (_t, catalog, vfs) = install(
        1,
        0,
        &[
            (
                "race0.aimap",
                police_section(&[("vpcop", 5.0, 0.0, 5.0, "90 0")]),
            ),
            (
                "roam.aimap",
                police_section(&[("vpghost", 1.0, 0.0, 1.0, "0 0")]),
            ),
            (
                "stray0.aimap",
                police_section(&[("vpcop", 1.0, 0.0, 1.0, "0 0")]),
            ),
        ],
    );
    let report = PoliceReport::scan(&vfs, &catalog.city);
    assert_eq!(report.entries.len(), 1);
    assert!(matches!(report.entries[0].amateur, PoliceBuild::Built(_)));
    // 1 event × 2 difficulties + the Cruise pair, all built; the pro
    // event falls back to the amateur record and so reports a variant
    // issue, plus a count mismatch against its zero table.
    assert_eq!(
        (report.built(), report.failed(), report.unsupported()),
        (4, 0, 0)
    );
    assert_eq!(report.wired(), 1 + 1 + 1 + 1);
    assert_eq!(
        report.issues(),
        3,
        "pro variant + pro count + cruise pro variant"
    );
    assert_eq!(report.extras.len(), 1);
    assert_eq!(report.extras[0].logical, "race/london/stray0.aimap");
    // No vehicle catalog on a synthetic install: every wired id is
    // unresolved, and the audit says so rather than assuming it away.
    assert_eq!(report.unresolved_vehicles, ["vpcop", "vpghost"]);
}

/// Retail: the production producer wires, for every cataloged
/// checkpoint/blitz/circuit event at both difficulties, exactly the
/// `Cops` count its table row authors (measured 2026-10-07: 0
/// mismatches over 128 builds), Blitz/Circuit field none (BLZ-2,
/// CIR-4), and Cruise's `roam` record wires 19/20 (sf amateur/pro) and
/// 20/20 (london). Skipped without the operator's install
/// (`MM2_RETAIL=<dir>`).
#[test]
fn retail_every_event_wires_its_tables_cop_count() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut vfs = Vfs::new();
    mm2_assets::mount_install(&mut vfs, &retail, &mm2_assets::InstallMount::default()).unwrap();
    let mut police_events = 0;
    for (city, cruise) in [("sf", [19, 20]), ("london", [20, 20])] {
        let report = PoliceReport::scan(&vfs, city);
        assert_eq!(report.entries.len(), 45, "{city}: events cataloged");
        assert_eq!(report.failed(), 0, "{city}");
        assert!(report.unresolved_vehicles.is_empty(), "{city}");
        for entry in &report.entries {
            let crash = entry.event_ref.table == EventTableKind::CrashCourse;
            for build in [&entry.amateur, &entry.professional] {
                match build {
                    PoliceBuild::Unsupported => assert!(crash, "{city} {}", entry.stem),
                    PoliceBuild::Built(s) => {
                        assert!(!crash);
                        assert!(s.issues.is_empty(), "{city} {}: {:?}", entry.stem, s.issues);
                        assert_eq!(Some(s.wired as i64), s.table_cops, "{city} {}", entry.stem);
                        if matches!(
                            entry.event_ref.table,
                            EventTableKind::Blitz | EventTableKind::Circuit
                        ) {
                            assert_eq!(s.wired, 0, "{city} {}", entry.stem);
                        }
                        police_events += usize::from(s.wired > 0);
                        assert!(s.vehicles.iter().all(|v| v == "vpcop"));
                    }
                    PoliceBuild::Failed(e) => panic!("{city} {}: {e}", entry.stem),
                }
            }
        }
        for (build, want) in report.cruise.iter().zip(cruise) {
            let PoliceBuild::Built(s) = build else {
                panic!("{city}: cruise roster did not build");
            };
            assert_eq!(s.wired, want, "{city} cruise");
            assert!(s.issues.is_empty(), "{city} cruise: {:?}", s.issues);
            assert_eq!(s.table_cops, None);
        }
        assert_eq!(report.extras.len(), 1, "{city}: extra stems wiring cops");
    }
    assert_eq!(police_events, 25, "event builds fielding at least one cop");
}
