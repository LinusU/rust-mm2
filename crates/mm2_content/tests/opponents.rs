//! `CatalogEvent → OpponentRoster` producer tests (F15-A.1): synthetic
//! `race/<city>/` installs through the real VFS and catalog, converted
//! by the production `opponent_roster`.

use std::path::Path;

use mm2_assets::Vfs;
use mm2_content::{EventCatalog, OpponentReport, RosterBuild, RosterBuildError, opponent_roster};
use mm2_game::{Difficulty, EventRef, EventTableKind, OpponentIssue};

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";
const OPP_HEADER: &str =
    "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\n";

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

/// Amateur `Opponents` / professional `Opponents` columns on a
/// checkpoint table row.
fn table_row(amateur_opp: i64, pro_opp: i64) -> String {
    format!("none,0,0,0,{amateur_opp},0,0.1,0.0,1,50,1,0,0,0,{pro_opp},0,0.2,0.0,1,40,1\n")
}

fn aimap_with_opponents(rows: &str) -> String {
    let n = rows.lines().filter(|l| !l.trim().is_empty()).count();
    format!("[Opponent]\n{n}\n{rows}")
}

fn opp_row(geo: &str, route: &str, params: &str) -> String {
    format!("{geo} {route} {params}\n")
}

fn opp_file(points: &[[f32; 3]]) -> String {
    let mut s = OPP_HEADER.to_string();
    for p in points {
        s.push_str(&format!("{},{},{},0,0,0,15.5,0,0\n", p[0], p[1], p[2]));
    }
    s
}

/// A one-event `race/london/` install: checkpoint row, waypoints, and
/// the authored opponent records a caller adds on top.
fn install(
    amateur_opp: i64,
    pro_opp: i64,
    files: &[(&str, String)],
) -> (tempfile::TempDir, EventCatalog, Vfs) {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/london/mmracedata.csv",
        &format!("{MM_HEADER}\n{}", table_row(amateur_opp, pro_opp)),
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
fn amateur_roster_wires_the_aimap_variant() {
    let (_t, catalog, vfs) = install(
        2,
        0,
        &[
            (
                "race0.aimap",
                aimap_with_opponents(
                    &(opp_row("vpfoo", "race0-a-0.opp", "0.90 0 50.0 0.7 0 0 0 0 0 1.0")
                        + &opp_row("vpbar", "race0-a-1.opp", "0.80 0 50.0 0.7 0 0 0 0 0 1.0")),
                ),
            ),
            (
                "race0-a-0.opp",
                opp_file(&[[0.0, 0.0, 0.0], [10.0, 0.0, -30.0]]),
            ),
            ("race0-a-1.opp", opp_file(&[[1.0, 0.0, 1.0]])),
        ],
    );
    let roster =
        opponent_roster(&vfs, event(&catalog, 0), Difficulty::Amateur).expect("roster builds");

    assert_eq!(roster.entries.len(), 2);
    assert!(roster.issues.is_empty(), "{:?}", roster.issues);
    let first = &roster.entries[0];
    assert_eq!(first.vehicle, "vpfoo");
    assert_eq!(
        first.skill(),
        Some(0.90),
        "first param is the skill-like value"
    );
    assert_eq!(first.params.len(), 10, "numeric tail preserved raw");
    let route = first.route.as_ref().expect("route resolved");
    assert_eq!(route.points.len(), 2);
    assert_eq!(route.points[0].position, bevy::prelude::Vec3::ZERO);
    assert_eq!(
        route.points[1].target_speed, 15.5,
        "authored column verbatim"
    );
    assert!(route.length() > 0.0);
}

#[test]
fn professional_prefers_aimap_p_and_p_tagged_routes() {
    let (_t, catalog, vfs) = install(
        1,
        1,
        &[
            (
                "race0.aimap",
                aimap_with_opponents(&opp_row(
                    "vpfoo",
                    "race0-a-0.opp",
                    "0.70 0 50.0 0.7 0 0 0 0 0 1.0",
                )),
            ),
            (
                "race0.aimap_p",
                aimap_with_opponents(&opp_row(
                    "vpbar",
                    "race0-p-0.opp",
                    "1.00 0 50.0 0.7 0 0 0 0 0 1.0",
                )),
            ),
            ("race0-a-0.opp", opp_file(&[[0.0, 0.0, 0.0]])),
            ("race0-p-0.opp", opp_file(&[[5.0, 0.0, 5.0]])),
        ],
    );
    let pro = opponent_roster(&vfs, event(&catalog, 0), Difficulty::Professional)
        .expect("pro roster builds");

    assert_eq!(pro.entries.len(), 1);
    assert_eq!(pro.entries[0].vehicle, "vpbar", "the _p variant's lineup");
    assert_eq!(
        pro.entries[0].route.as_ref().unwrap().points[0].position,
        bevy::prelude::Vec3::new(5.0, 0.0, 5.0),
        "the -p- tagged route resolved"
    );
    assert!(pro.issues.is_empty(), "{:?}", pro.issues);
}

#[test]
fn missing_p_variant_falls_back_to_the_only_authored_lineup() {
    let (_t, catalog, vfs) = install(
        1,
        1,
        &[
            (
                "race0.aimap",
                aimap_with_opponents(&opp_row(
                    "vpfoo",
                    "race0-a-0.opp",
                    "0.70 0 50.0 0.7 0 0 0 0 0 1.0",
                )),
            ),
            ("race0-a-0.opp", opp_file(&[[0.0, 0.0, 0.0]])),
        ],
    );
    let pro = opponent_roster(&vfs, event(&catalog, 0), Difficulty::Professional)
        .expect("the only authored roster still builds");

    assert_eq!(pro.entries.len(), 1);
    assert_eq!(pro.entries[0].vehicle, "vpfoo");
    assert!(
        pro.issues.contains(&OpponentIssue::MissingVariant {
            wanted: Difficulty::Professional,
            used: Difficulty::Amateur,
        }),
        "the fallback is recorded, not silent: {:?}",
        pro.issues
    );
    // And the amateur build does not flag a variant issue.
    let am = opponent_roster(&vfs, event(&catalog, 0), Difficulty::Amateur).unwrap();
    assert!(am.issues.is_empty(), "{:?}", am.issues);
}

#[test]
fn a_dead_route_ref_keeps_the_authored_slot() {
    let (_t, catalog, vfs) = install(
        2,
        0,
        &[(
            "race0.aimap",
            aimap_with_opponents(
                &(opp_row("vpfoo", "race0-a-0.opp", "0.9")
                    + &opp_row("vpbar", "race0-a-9.opp", "0.8")),
            ),
        )],
    );
    // `race0-a-0.opp` also missing — both refs die; the table still
    // authors 2 opponents.
    let roster = opponent_roster(&vfs, event(&catalog, 0), Difficulty::Amateur).unwrap();

    assert_eq!(roster.entries.len(), 2, "authored slots are kept");
    assert!(roster.entries.iter().all(|e| e.route.is_none()));
    let unresolved: Vec<_> = roster
        .issues
        .iter()
        .filter_map(|i| match i {
            OpponentIssue::UnresolvedRoute { name } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(unresolved, ["race0-a-0.opp", "race0-a-9.opp"]);
}

#[test]
fn count_mismatch_is_an_issue_not_an_error() {
    // The `sf/race0` shape (RACE-11): the table authors 7 amateur
    // opponents but the aimap wires 6 — and the orphaned seventh route
    // file ships unreferenced.
    let rows = (0..6)
        .map(|i| opp_row("vpfoo", &format!("race0-a-{i}.opp"), "0.9"))
        .collect::<String>();
    let mut files: Vec<(String, String)> =
        vec![("race0.aimap".into(), aimap_with_opponents(&rows))];
    for i in 0..7 {
        files.push((
            format!("race0-a-{i}.opp"),
            opp_file(&[[i as f32, 0.0, 0.0]]),
        ));
    }
    let file_refs: Vec<(&str, String)> =
        files.iter().map(|(a, b)| (a.as_str(), b.clone())).collect();
    let (_t, catalog, vfs) = install(7, 0, &file_refs);
    let roster = opponent_roster(&vfs, event(&catalog, 0), Difficulty::Amateur).unwrap();

    assert_eq!(
        roster.entries.len(),
        6,
        "the aimap is the lineup, not the table"
    );
    assert!(roster.entries.iter().all(|e| e.route.is_some()));
    assert!(
        roster
            .issues
            .contains(&OpponentIssue::CountMismatch { wired: 6, table: 7 }),
        "{:?}",
        roster.issues
    );
    assert!(
        roster.issues.contains(&OpponentIssue::UnreferencedRoute {
            name: "race0-a-6.opp".into()
        }),
        "{:?}",
        roster.issues
    );
}

#[test]
fn unreferenced_routes_are_scoped_to_the_selected_variant() {
    let rows = opp_row("vpfoo", "race0-a-0.opp", "0.9") + &opp_row("vpbar", "race0-a-1.opp", "0.8");
    let prows = opp_row("vppro", "race0-p-0.opp", "1.0");
    let (_t, catalog, vfs) = install(
        2,
        1,
        &[
            ("race0.aimap", aimap_with_opponents(&rows)),
            ("race0.aimap_p", aimap_with_opponents(&prows)),
            ("race0-a-0.opp", opp_file(&[[0.0, 0.0, 0.0]])),
            ("race0-a-1.opp", opp_file(&[[1.0, 0.0, 0.0]])),
            ("race0-a-2.opp", opp_file(&[[2.0, 0.0, 0.0]])), // spare
            ("race0-p-0.opp", opp_file(&[[3.0, 0.0, 0.0]])),
        ],
    );
    let am = opponent_roster(&vfs, event(&catalog, 0), Difficulty::Amateur).unwrap();
    assert!(
        am.issues.contains(&OpponentIssue::UnreferencedRoute {
            name: "race0-a-2.opp".into()
        }),
        "the spare -a- file is flagged for the amateur roster: {:?}",
        am.issues
    );
    let pro = opponent_roster(&vfs, event(&catalog, 0), Difficulty::Professional).unwrap();
    assert!(
        !pro.issues.iter().any(
            |i| matches!(i, OpponentIssue::UnreferencedRoute { name } if name == "race0-a-2.opp")
        ),
        "the other difficulty's spare is not this roster's issue: {:?}",
        pro.issues
    );
}

#[test]
fn a_route_ref_tagged_for_the_wrong_difficulty_is_flagged() {
    let (_t, catalog, vfs) = install(
        1,
        0,
        &[
            (
                "race0.aimap",
                aimap_with_opponents(&opp_row("vpfoo", "race0-p-0.opp", "0.9")),
            ),
            ("race0-p-0.opp", opp_file(&[[0.0, 0.0, 0.0]])),
        ],
    );
    let roster = opponent_roster(&vfs, event(&catalog, 0), Difficulty::Amateur).unwrap();
    assert!(
        roster.issues.contains(&OpponentIssue::WrongDifficultyTag {
            name: "race0-p-0.opp".into(),
            expected: 'a',
        }),
        "{:?}",
        roster.issues
    );
    assert!(
        roster.entries[0].route.is_some(),
        "the route still resolves — the tag is a consistency note"
    );
}

#[test]
fn incomplete_events_and_crash_course_are_rejected() {
    // No waypoints → the catalog marks the event Incomplete.
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/london/mmracedata.csv",
        &format!("{MM_HEADER}\n{}", table_row(1, 1)),
    );
    write(d, "race/london/race0.aimap", "#\n");
    let vfs = vfs_of(d);
    let catalog = EventCatalog::scan(&vfs, "london");
    assert!(matches!(
        opponent_roster(&vfs, event(&catalog, 0), Difficulty::Amateur),
        Err(RosterBuildError::NotReady(_))
    ));

    // A complete Crash Course event is deferred scope, not a failure.
    let tmp2 = tempfile::tempdir().unwrap();
    let d = tmp2.path();
    write(
        d,
        "race/london/mmcrashdata.csv",
        &format!("{MM_HEADER}\n{}", table_row(0, 0)),
    );
    write(d, "race/london/crash0.aimap", "#\n");
    write(
        d,
        "race/london/crash0data.csv",
        "Filename,Event,Checkpoints,TimeLimit,AmbDensity,extra,extra,extra,extra,etra,\nlongjump,0,1,26,0,0,0,0,0,0,0\n",
    );
    write(
        d,
        "race/london/crash0data_p.csv",
        "Filename,Event,Checkpoints,TimeLimit,AmbDensity,extra,extra,extra,extra,etra,\nlongjump,0,1,25,0,0,0,0,0,0,0\n",
    );
    write(
        d,
        "race/london/longjump.csv",
        &format!("{WAYPOINTS}0,0,0,90,15,0,0,0,\n30,0,0,90,8,0,0,0,\n60,0,0,90,10,0,0,0,\n"),
    );
    let vfs = vfs_of(d);
    let catalog = EventCatalog::scan(&vfs, "london");
    let ev = catalog
        .get(&EventRef {
            city: "london".into(),
            table: EventTableKind::CrashCourse,
            index: 0,
        })
        .unwrap();
    assert!(ev.status.is_ready());
    assert!(matches!(
        opponent_roster(&vfs, ev, Difficulty::Amateur),
        Err(RosterBuildError::CrashCourseUnsupported)
    ));
}

#[test]
fn report_covers_events_extras_and_vehicle_resolution() {
    // Event race0 wires vpfoo (a catalog vehicle) and vpghost (nothing).
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/london/mmracedata.csv",
        &format!("{MM_HEADER}\n{}", table_row(2, 0)),
    );
    write(
        d,
        "race/london/race0.aimap",
        &aimap_with_opponents(
            &(opp_row("vpfoo", "race0-a-0.opp", "0.9")
                + &opp_row("vpghost", "race0-a-1.opp", "0.8")),
        ),
    );
    write(
        d,
        "race/london/race0waypoints.csv",
        &format!("{WAYPOINTS}0,0,0,90,15,0,0,0,\n10,0,-30,90,8,0,0,0,\n70,0,-90,90,10,0,0,0,\n"),
    );
    write(
        d,
        "race/london/race0-a-0.opp",
        &opp_file(&[[0.0, 0.0, 0.0]]),
    );
    write(
        d,
        "race/london/race0-a-1.opp",
        &opp_file(&[[1.0, 0.0, 0.0]]),
    );
    // A non-event stem still carrying a wired lineup — the `stunt0`
    // shape: one row, dead `.opp` ref.
    write(
        d,
        "race/london/stunt0.aimap",
        &aimap_with_opponents(&opp_row("vpfoo", "opp-c0.2", "1.00")),
    );
    write(d, "tune/vpfoo.info", "Description=Vpfoo test car\n");
    let vfs = vfs_of(d);
    let report = OpponentReport::scan(&vfs, "london");

    assert_eq!(report.entries.len(), 1);
    assert_eq!(report.built(), 2, "amateur + professional builds");
    match &report.entries[0].amateur {
        RosterBuild::Built(s) => {
            assert_eq!(s.wired, 2);
            assert_eq!(s.routes, 2);
            assert_eq!(s.vehicles, ["vpfoo", "vpghost"]);
        }
        other => panic!("expected Built, got {other:?}"),
    }
    assert_eq!(
        report.unresolved_vehicles,
        ["vpghost"],
        "the id no vehicle catalog knows is named"
    );
    assert_eq!(report.extra_rosters.len(), 1);
    assert_eq!(report.extra_rosters[0].wired, 1);
    assert_eq!(report.extra_rosters[0].dead_refs, 1);
}
