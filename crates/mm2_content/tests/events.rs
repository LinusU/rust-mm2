//! Event catalog integration tests (F11-A): a synthetic `race/<city>/`
//! install mounted through the real VFS, scanned by the production
//! `EventCatalog`.

use std::path::Path;

use mm2_assets::Vfs;
use mm2_content::{EventCatalog, EventResolveError, EventStatus, RecordContent};
use mm2_formats::racefiles::RaceFileKind;
use mm2_formats::rewards::RewardNum;
use mm2_game::{EventRef, EventTableKind};

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";
const BLITZ_WAYPOINTS: &str = "x,y,z,a,radius,frame rate,state changes,texture changes,msg\n";
const OPP: &str = "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\n";
const CRASHDATA: &str =
    "Filename,Event,Checkpoints,TimeLimit,AmbDensity,extra,extra,extra,extra,etra,\n";
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

/// A small `race/london/` install covering every dependency shape.
fn synthetic_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();

    // Two checkpoint rows: race0 complete, race1 missing its waypoints.
    write(
        d,
        "race/london/mmracedata.csv",
        &format!(
            "{MM_HEADER}\nnone,0,0,0,7,0,0.1,0.0,3,50,1,0,0,1,6,0,0.2,0.0,4,40,1\nnone,0,0,0,5,0,0.3,0.0,2,60,1,0,0,0,5,0,0.4,0.0,2,55,1\n"
        ),
    );
    write(d, "race/london/race0.aimap", "# sections unparsed\n");
    write(
        d,
        "race/london/race0waypoints.csv",
        &format!("{WAYPOINTS}1,2,3,4,15,0,0,0,\n5,6,7,8,10,0,0,0,\n"),
    );
    write(
        d,
        "race/london/race0-a-0.opp",
        &format!("{OPP}1,2,3,175,0,0,0,0,0\n"),
    );
    write(
        d,
        "race/london/race0-p-0.opp",
        &format!("{OPP}1,2,3,175,0,0,0,0,0\n"),
    );
    write(d, "race/london/race1.aimap", "# sections unparsed\n");

    // One blitz row, one circuit row — both complete.
    write(
        d,
        "race/london/mmblitzdata.csv",
        &format!("{MM_HEADER}\nnone,0,0,0,0,0,0.1,0.0,0,50,1,0,0,0,0,0,0.2,0.0,0,40,1\n"),
    );
    write(d, "race/london/blitz0.aimap", "#\n");
    write(
        d,
        "race/london/blitz0waypoints.csv",
        &format!("{BLITZ_WAYPOINTS}1,2,3,4,15.0,0,0,0,\n"),
    );
    write(
        d,
        "race/london/mmcircuitdata.csv",
        &format!("{MM_HEADER}\nnone,0,0,0,7,0,0.0,0.0,3,50,1,0,0,0,7,0,0.0,0.0,4,40,1\n"),
    );
    write(d, "race/london/circuit0.aimap", "#\n");
    write(
        d,
        "race/london/circuit0waypoints.csv",
        &format!("{WAYPOINTS}1,2,3,4,15,0,0,0,\n"),
    );

    // Two crash rows: crash0 links to an existing waypoint file,
    // crash1 links to a ghost.
    write(
        d,
        "race/london/mmcrashdata.csv",
        &format!(
            "{MM_HEADER}\nlesson1,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1\nlesson2,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1\n"
        ),
    );
    write(d, "race/london/crash0.aimap", "#\n");
    write(
        d,
        "race/london/crash0data.csv",
        &format!("{CRASHDATA}longjump,0,1,26,0,0,0,0,0,0,0\n"),
    );
    write(
        d,
        "race/london/crash0data_p.csv",
        &format!("{CRASHDATA}longjump,0,1,25,0,0,0,0,0,0,0\n"),
    );
    write(
        d,
        "race/london/longjump.csv",
        &format!("{WAYPOINTS}1,2,3,4,15,0,0,0,\n"),
    );
    write(d, "race/london/crash1.aimap", "#\n");
    write(
        d,
        "race/london/crash1data.csv",
        &format!("{CRASHDATA}ghost,0,1,26,0,0,0,0,0,0,0\n"),
    );
    write(
        d,
        "race/london/crash1data_p.csv",
        &format!("{CRASHDATA}ghost,0,1,25,0,0,0,0,0,0,0\n"),
    );

    // Rewards: one indexed crash reward, two milestones.
    write(
        d,
        "race/london/london_rewards.csv",
        &format!(
            "{REWARDS}race,half,vpx,0,msg one,\nblitz,all,vpz,2,msg two,\ncrash,0,vpy,1,msg three,\n"
        ),
    );

    // Discovered-but-unclaimed records.
    write(d, "race/london/roam.aimap", "#\n");
    write(d, "race/london/race5.aimap", "#\n");
    write(d, "race/london/cir1_strtpnts", "1,2,3,90,0,0,0,0,0,\n");

    tmp
}

#[test]
fn catalog_indexes_rows_and_dependencies() {
    let tmp = synthetic_install();
    let vfs = vfs_of(tmp.path());
    let cat = EventCatalog::scan(&vfs, "london");

    assert_eq!(cat.events.len(), 6);
    assert!(cat.diagnostics.is_empty(), "{:?}", cat.diagnostics);
    assert_eq!(cat.tables.len(), 4);
    assert!(cat.tables.iter().all(|t| t.error.is_none()));

    // Stable identity: first event is Checkpoint row 0.
    let first = &cat.events[0];
    assert_eq!(
        first.event_ref,
        EventRef {
            city: "london".into(),
            table: EventTableKind::Checkpoint,
            index: 0,
        }
    );
    assert_eq!(first.stem, "race0");
    assert!(first.status.is_ready(), "{:?}", first.status);
    assert_eq!(first.amateur.opponents, 7);
    assert_eq!(first.professional.opponents, 6);

    // race0 carries its parsed waypoints and both difficulty opps.
    let kinds: Vec<RaceFileKind> = first.records.iter().map(|r| r.kind).collect();
    assert!(kinds.contains(&RaceFileKind::Aimap));
    let wp = first
        .records
        .iter()
        .find(|r| r.kind == RaceFileKind::Waypoints)
        .unwrap();
    assert!(matches!(
        wp.content,
        RecordContent::Waypoints { rows: 2, .. }
    ));
    let mut diffs: Vec<char> = first.records.iter().filter_map(|r| r.difficulty).collect();
    diffs.sort();
    assert_eq!(diffs, vec!['a', 'p']);

    // race1 is kept in the catalog but flagged, not dropped.
    let race1 = cat
        .events
        .iter()
        .find(|e| e.stem == "race1")
        .expect("race1 cataloged");
    match &race1.status {
        EventStatus::Incomplete { missing } => {
            assert!(missing.iter().any(|m| m.contains("waypoints")));
        }
        other => panic!("race1 should be incomplete, got {other:?}"),
    }
}

#[test]
fn crash_course_links_and_rewards() {
    let tmp = synthetic_install();
    let vfs = vfs_of(tmp.path());
    let cat = EventCatalog::scan(&vfs, "london");

    let crash0 = cat.events.iter().find(|e| e.stem == "crash0").unwrap();
    assert_eq!(crash0.description, "lesson1");
    assert!(crash0.status.is_ready(), "{:?}", crash0.status);
    // Both difficulty tables plus the referenced waypoint file resolve.
    assert!(
        crash0
            .records
            .iter()
            .any(|r| r.logical == "race/london/longjump.csv"
                && matches!(r.content, RecordContent::Waypoints { rows: 1, .. }))
    );
    assert_eq!(crash0.rewards.len(), 1);
    assert_eq!(crash0.rewards[0].car, "vpy");

    // crash1's ghost Filename link is a failed reference, and the event
    // is incomplete because of it.
    let crash1 = cat.events.iter().find(|e| e.stem == "crash1").unwrap();
    assert!(crash1.failed.iter().any(|f| f.reference.contains("ghost")));
    assert!(!crash1.status.is_ready());

    // Milestone rewards stay on the catalog; the referenced waypoint
    // stem and the rewards table itself are not extras.
    assert_eq!(cat.milestone_rewards.len(), 2);
    assert!(
        cat.milestone_rewards
            .iter()
            .all(|r| matches!(r.race_num, RewardNum::Half | RewardNum::All))
    );
    let extra_labels: Vec<&str> = cat.extras.iter().map(|e| e.label.as_str()).collect();
    assert!(extra_labels.contains(&"roam"));
    assert!(extra_labels.contains(&"race5"));
    assert!(extra_labels.contains(&"cir1"));
    assert!(!extra_labels.contains(&"longjump"));
    assert!(!extra_labels.contains(&"london_rewards"));
}

#[test]
fn resolve_validates_dependencies() {
    let tmp = synthetic_install();
    let vfs = vfs_of(tmp.path());
    let cat = EventCatalog::scan(&vfs, "london");

    let ready = EventRef {
        city: "london".into(),
        table: EventTableKind::Checkpoint,
        index: 0,
    };
    assert_eq!(cat.resolve(&ready).unwrap().stem, "race0");

    let oob = EventRef {
        city: "london".into(),
        table: EventTableKind::Checkpoint,
        index: 9,
    };
    assert!(matches!(
        cat.resolve(&oob),
        Err(EventResolveError::UnknownEvent)
    ));

    let broken = EventRef {
        city: "london".into(),
        table: EventTableKind::Checkpoint,
        index: 1,
    };
    assert!(matches!(
        cat.resolve(&broken),
        Err(EventResolveError::Incomplete { .. })
    ));

    let wrong_city = EventRef {
        city: "sf".into(),
        table: EventTableKind::Checkpoint,
        index: 0,
    };
    assert!(matches!(
        cat.resolve(&wrong_city),
        Err(EventResolveError::WrongCity { .. })
    ));
}

#[test]
fn empty_install_reports_not_panics() {
    let tmp = tempfile::tempdir().unwrap();
    let vfs = vfs_of(tmp.path());
    let cat = EventCatalog::scan(&vfs, "london");
    assert!(cat.is_empty());
    assert_eq!(cat.tables.len(), 4);
    assert!(cat.tables.iter().all(|t| t.error.is_some()));
}

#[test]
fn malformed_table_is_a_table_error_not_a_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "race/london/mmracedata.csv", "not,a,table\n1,2,3\n");
    write(
        d,
        "race/london/mmblitzdata.csv",
        &format!("{MM_HEADER}\nnone,0,0,0,0,0,0.1,0.0,0,50,1,0,0,0,0,0,0.2,0.0,0,40,1\n"),
    );
    let vfs = vfs_of(d);
    let cat = EventCatalog::scan(&vfs, "london");
    let race_table = cat
        .tables
        .iter()
        .find(|t| t.table == EventTableKind::Checkpoint)
        .unwrap();
    assert!(race_table.error.is_some());
    assert_eq!(cat.events.len(), 1); // the blitz row still catalogs
}
