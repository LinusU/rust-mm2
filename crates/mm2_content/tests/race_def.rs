//! `CatalogEvent → RaceDefinition` producer tests (F11-B.2): synthetic
//! `race/<city>/` installs through the real VFS and catalog, converted
//! by the production `race_definition`.

use std::path::Path;

use mm2_assets::Vfs;
use mm2_content::{EventCatalog, RaceBuildError, race_definition};
use mm2_game::{CheckpointRule, Difficulty, EventRef, EventTableKind};

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";
const ROW: &str = "none,0,0,0,0,0,0.1,0.0,1,50,1,0,0,0,0,0,0.2,0.0,1,40,1";

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

fn event(
    catalog: &EventCatalog,
    table: EventTableKind,
    index: usize,
) -> &mm2_content::CatalogEvent {
    catalog
        .get(&EventRef {
            city: catalog.city.clone(),
            table,
            index,
        })
        .expect("test event resolves")
}

/// One authored waypoint row.
fn row(x: f32, z: f32, width: f32) -> String {
    format!("{x},0,{z},90,{width},0,0,0,\n")
}

#[test]
fn checkpoint_event_produces_anyorder_gates_and_a_finish() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/london/mmracedata.csv",
        &format!("{MM_HEADER}\n{ROW}\n"),
    );
    write(d, "race/london/race0.aimap", "#\n");
    write(
        d,
        "race/london/race0waypoints.csv",
        &format!(
            "{WAYPOINTS}{}{}{}{}",
            row(0.0, 0.0, 15.0),    // start line
            row(10.0, -30.0, 8.0),  // gate
            row(40.0, -60.0, 9.0),  // gate
            row(70.0, -90.0, 10.0)  // finish
        ),
    );
    let vfs = vfs_of(d);
    let catalog = EventCatalog::scan(&vfs, "london");
    let ev = event(&catalog, EventTableKind::Checkpoint, 0);

    let def = race_definition(ev, Difficulty::Amateur).unwrap();
    assert_eq!(def.rule, CheckpointRule::AnyOrder);
    assert_eq!(def.checkpoints.len(), 2, "start line and finish excluded");
    assert_eq!(def.checkpoints[0].center.x, 10.0);
    assert_eq!(def.checkpoints[0].radius, 8.0, "authored width verbatim");
    assert_eq!(def.checkpoints[0].heading_deg, 90.0);
    let finish = def.finish.expect("last row is the finish trigger");
    assert_eq!(finish.center.x, 70.0);
    assert_eq!(finish.radius, 10.0);
    assert_eq!(def.laps, 0);
    assert!(
        def.countdown_ticks > 0,
        "countdown default drives Ready→Countdown→Playing"
    );
}

#[test]
fn derived_start_slot_sits_behind_the_line_facing_the_course() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/london/mmracedata.csv",
        &format!("{MM_HEADER}\n{ROW}\n"),
    );
    write(d, "race/london/race0.aimap", "#\n");
    write(
        d,
        "race/london/race0waypoints.csv",
        &format!(
            "{WAYPOINTS}{}{}{}{}",
            row(0.0, 0.0, 15.0),
            row(0.0, -30.0, 8.0), // course runs toward -Z
            row(0.0, -60.0, 9.0),
            row(0.0, -90.0, 10.0)
        ),
    );
    let vfs = vfs_of(d);
    let catalog = EventCatalog::scan(&vfs, "london");
    let def = race_definition(
        event(&catalog, EventTableKind::Checkpoint, 0),
        Difficulty::Amateur,
    )
    .unwrap();

    assert_eq!(def.start_slots.len(), 1, "no authored grid → one slot");
    let slot = def.start_slots[mm2_content::PLAYER_SLOT];
    assert!(slot.position.z > 0.0, "behind the start line: {slot:?}");
    // Facing (sin a, cos a) must point down-course toward -Z.
    let a = slot.yaw_deg.to_radians();
    assert!(
        a.cos() < -0.98,
        "yaw {:.1}° should face -Z (the course)",
        slot.yaw_deg
    );
}

#[test]
fn authored_strtpnts_become_the_start_slots() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/sf/mmcircuitdata.csv",
        &format!("{MM_HEADER}\n{ROW}\n"),
    );
    write(d, "race/sf/circuit0.aimap", "#\n");
    write(
        d,
        "race/sf/circuit0waypoints.csv",
        &format!(
            "{WAYPOINTS}{}{}{}{}",
            row(0.0, 0.0, 15.0),
            row(30.0, 0.0, 8.0),
            row(60.0, 0.0, 9.0),
            row(90.0, 0.0, 10.0)
        ),
    );
    // Retail ships the grid under the short `cir<N>` stem, which the
    // catalog attributes to the `circuit<N>` event by index.
    write(
        d,
        "race/sf/cir0_strtpnts",
        "-489.25,19.59,-55.05,92.36,0,0,0,0,0,\n-499.99,19.25,-54.46,-267.54,0,0,0,0,0,\n",
    );
    let vfs = vfs_of(d);
    let catalog = EventCatalog::scan(&vfs, "sf");
    let def = race_definition(
        event(&catalog, EventTableKind::Circuit, 0),
        Difficulty::Professional,
    )
    .unwrap();

    assert_eq!(def.start_slots.len(), 2, "both authored slots kept");
    assert_eq!(def.start_slots[0].position.x, -489.25);
    assert_eq!(def.start_slots[0].yaw_deg, 92.36, "authored yaw verbatim");
}

#[test]
fn circuit_is_ordered_with_the_start_line_as_each_laps_last_gate() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/sf/mmcircuitdata.csv",
        &format!("{MM_HEADER}\n{ROW}\n"),
    );
    write(d, "race/sf/circuit0.aimap", "#\n");
    write(
        d,
        "race/sf/circuit0waypoints.csv",
        &format!(
            "{WAYPOINTS}{}{}{}{}",
            row(0.0, 0.0, 15.0),
            row(30.0, 0.0, 8.0),
            row(60.0, 0.0, 9.0),
            row(90.0, 0.0, 10.0)
        ),
    );
    let vfs = vfs_of(d);
    let catalog = EventCatalog::scan(&vfs, "sf");
    let def = race_definition(
        event(&catalog, EventTableKind::Circuit, 0),
        Difficulty::Amateur,
    )
    .unwrap();

    assert_eq!(def.rule, CheckpointRule::Ordered);
    assert_eq!(def.laps, 1, "authored NumLaps drives lap count");
    assert_eq!(
        def.checkpoints.len(),
        4,
        "every row participates; the line copy closes the lap"
    );
    assert!(
        def.finish.is_none(),
        "ordered races finish on the last gate"
    );
    let last = def.checkpoints.last().unwrap();
    assert_eq!(last.center.x, 0.0, "start line copy closes the lap");
    assert!(last.center.y > 0.0, "lifted off the actual line");
}

#[test]
fn incomplete_event_is_rejected_not_run_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/london/mmracedata.csv",
        &format!("{MM_HEADER}\n{ROW}\n"),
    );
    write(d, "race/london/race0.aimap", "#\n");
    // No waypoints → Incomplete.
    let vfs = vfs_of(d);
    let catalog = EventCatalog::scan(&vfs, "london");
    let ev = event(&catalog, EventTableKind::Checkpoint, 0);
    assert!(matches!(
        race_definition(ev, Difficulty::Amateur),
        Err(RaceBuildError::NotReady(_))
    ));
}

#[test]
fn crash_course_is_explicitly_unsupported() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/london/mmcrashdata.csv",
        &format!("{MM_HEADER}\n{ROW}\n"),
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
        &format!(
            "{WAYPOINTS}{}{}{}",
            row(0.0, 0.0, 15.0),
            row(30.0, 0.0, 8.0),
            row(60.0, 0.0, 10.0)
        ),
    );
    let vfs = vfs_of(d);
    let catalog = EventCatalog::scan(&vfs, "london");
    let ev = event(&catalog, EventTableKind::CrashCourse, 0);
    assert!(ev.status.is_ready(), "fixture should be a complete event");
    assert!(matches!(
        race_definition(ev, Difficulty::Amateur),
        Err(RaceBuildError::CrashCourseUnsupported)
    ));
}

/// A one-row `race/<city>/` blitz install from a table row.
fn blitz_install(table_row: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/london/mmblitzdata.csv",
        &format!("{MM_HEADER}\n{table_row}\n"),
    );
    write(d, "race/london/blitz0.aimap", "#\n");
    write(
        d,
        "race/london/blitz0waypoints.csv",
        &format!(
            "{WAYPOINTS}{}{}{}",
            row(0.0, 0.0, 15.0),
            row(30.0, 0.0, 8.0),
            row(60.0, 0.0, 10.0)
        ),
    );
    tmp
}

#[test]
fn blitz_binds_the_authored_time_limit_and_event_params() {
    // Amateur 25 s / Professional 18 s like retail london blitz0, with
    // distinct conditions and densities per difficulty.
    let tmp = blitz_install("none,0,2,1,0,0,0.3,0.1,3,25,1,0,3,2,0,0,0.4,0.2,4,18,1");
    let vfs = vfs_of(tmp.path());
    let catalog = EventCatalog::scan(&vfs, "london");
    let ev = event(&catalog, EventTableKind::Blitz, 0);

    let am = race_definition(ev, Difficulty::Amateur).unwrap();
    assert_eq!(
        am.time_limit_ticks,
        Some(25 * mm2_game::RACE_TICK_HZ),
        "authored seconds convert to fixed ticks"
    );
    assert_eq!(am.params.densities.traffic, 0.3);
    assert_eq!(am.params.densities.pedestrians, 0.1);
    assert_eq!(am.params.conditions.time_of_day.get(), 2);
    assert_eq!(am.params.conditions.weather.get(), 1);
    assert_eq!(am.params.opponents, 0);
    assert_eq!(am.params.cops, 0);
    assert_eq!(am.laps, 0, "the Blitz NumLaps column is template junk");

    let pro = race_definition(ev, Difficulty::Professional).unwrap();
    assert_eq!(pro.time_limit_ticks, Some(18 * mm2_game::RACE_TICK_HZ));
    assert_eq!(pro.params.conditions.time_of_day.get(), 3);
    assert_eq!(pro.params.conditions.weather.get(), 2);
    assert_eq!(pro.params.densities.traffic, 0.4);
    assert_eq!(pro.params.densities.pedestrians, 0.2);
}

#[test]
fn non_blitz_events_stay_untimed() {
    // Checkpoint and Circuit rows carry the constant 50/40 TimeLimit
    // template value (UNK-4) — binding it would enforce an unverified
    // rule, so the producer leaves those definitions untimed.
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    for (table, stem) in [("mmracedata", "race0"), ("mmcircuitdata", "circuit0")] {
        write(
            d,
            &format!("race/london/{table}.csv"),
            &format!("{MM_HEADER}\n{ROW}\n"),
        );
        write(d, &format!("race/london/{stem}.aimap"), "#\n");
        write(
            d,
            &format!("race/london/{stem}waypoints.csv"),
            &format!(
                "{WAYPOINTS}{}{}{}{}",
                row(0.0, 0.0, 15.0),
                row(30.0, 0.0, 8.0),
                row(60.0, 0.0, 9.0),
                row(90.0, 0.0, 10.0)
            ),
        );
    }
    let vfs = vfs_of(d);
    let catalog = EventCatalog::scan(&vfs, "london");
    for table in [EventTableKind::Checkpoint, EventTableKind::Circuit] {
        let def = race_definition(event(&catalog, table, 0), Difficulty::Amateur).unwrap();
        assert_eq!(
            def.time_limit_ticks, None,
            "{table:?}'s template TimeLimit must not become a deadline"
        );
    }
}

#[test]
fn unusable_params_fail_the_build_explicitly() {
    // Every out-of-range authored value is a named build error, never
    // a silent clamp: zero/negative TimeLimit, a selector past the
    // authored 0-3, a density past 1.0, a negative actor count.
    let cases = [
        (
            "none,0,0,0,0,0,0.3,0.1,3,0,1,0,0,0,0,0,0.3,0.1,4,18,1",
            "TimeLimit",
        ),
        (
            "none,0,9,0,0,0,0.3,0.1,3,25,1,0,0,0,0,0,0.4,0.1,4,18,1",
            "TimeofDay",
        ),
        (
            "none,0,0,0,0,0,1.5,0.1,3,25,1,0,0,0,0,0,0.4,0.1,4,18,1",
            "Ambient/Peds",
        ),
        (
            "none,0,0,0,-1,0,0.3,0.1,3,25,1,0,0,0,0,0,0.4,0.1,4,18,1",
            "Opponents",
        ),
    ];
    for (table_row, field) in cases {
        let tmp = blitz_install(table_row);
        let vfs = vfs_of(tmp.path());
        let catalog = EventCatalog::scan(&vfs, "london");
        let ev = event(&catalog, EventTableKind::Blitz, 0);
        match race_definition(ev, Difficulty::Amateur) {
            Err(RaceBuildError::BadParam { field: f, .. }) => {
                assert_eq!(f, field, "wrong column named");
            }
            other => panic!("{field}: expected BadParam, got {other:?}"),
        }
    }
}

#[test]
fn too_few_rows_is_an_explicit_error() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/london/mmblitzdata.csv",
        &format!("{MM_HEADER}\n{ROW}\n"),
    );
    write(d, "race/london/blitz0.aimap", "#\n");
    write(
        d,
        "race/london/blitz0waypoints.csv",
        &format!(
            "x,y,z,a,radius,frame rate,state changes,texture changes,msg\n{}{}",
            row(0.0, 0.0, 15.0),
            row(30.0, 0.0, 8.0)
        ),
    );
    let vfs = vfs_of(d);
    let catalog = EventCatalog::scan(&vfs, "london");
    let ev = event(&catalog, EventTableKind::Blitz, 0);
    assert!(matches!(
        race_definition(ev, Difficulty::Amateur),
        Err(RaceBuildError::TooFewRows { .. })
    ));
}
