//! F20-A.2 police integration: an authored `[Police]` lineup on a
//! synthetic install is fielded through the production
//! `load_session_world` → `police_roster_from_aimap` → `load_opponent`
//! path as session-owned, non-participant cars standing at their
//! authored poses. Nothing here claims pursuit — that is F20-A.3.

use crate::opponents::{
    COURSE_Z, event_app, event_config, phase, run, vfs_of, waypoint_row, write, write_car,
};
use crate::support::{MM_HEADER, WAYPOINTS};

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_app::opponents::OpponentDriver;
use mm2_app::police::{PoliceCar, PoliceFleet, staging_yaw};
use mm2_app::session::SessionControl;
use mm2_game::{
    ObjectIdentity, Player, PlayerVehicle, PoliceSpec, RaceProgress, Session, SessionAuthority,
    SessionConfig, SessionEntity, SessionPhase,
};
use mm2_vehicle::{Vehicle, VehicleInput};

/// A checkpoint event whose table row authors `cops` police (both
/// difficulties) and whose aimap wires `rows` as its `[Police]`
/// section. `vpcop` (mass 1500) is installed; anything else is not.
fn install(cops: i64, rows: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(
        d,
        "race/testcity/mmracedata.csv",
        format!("{MM_HEADER}\nnone,0,0,0,0,{cops},0.1,0.0,1,50,1,0,0,0,0,{cops},0.2,0.0,1,40,1\n"),
    );
    let n = rows.lines().filter(|l| !l.trim().is_empty()).count();
    write(
        d,
        "race/testcity/race0.aimap",
        format!("[Police]\n{n}\n{rows}"),
    );
    write(
        d,
        "race/testcity/race0waypoints.csv",
        format!(
            "{WAYPOINTS}{}{}{}",
            waypoint_row(60.0, COURSE_Z),
            waypoint_row(140.0, COURSE_Z),
            waypoint_row(180.0, COURSE_Z),
        ),
    );
    write_car(d, "vpcop", 1500.0, None);
    tmp
}

const TWO_COPS: &str = "vpcop 40 0 100 90 0 15 0.5 50\nvpcop -30 0 60 535 0 15 0.5 50\n";

fn cops(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query_filtered::<Entity, With<PoliceCar>>()
        .iter(app.world())
        .collect()
}

#[test]
fn the_authored_lineup_is_fielded_as_session_owned_non_participants() {
    let tmp = install(2, TWO_COPS);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    assert_eq!(phase(&app), SessionPhase::Countdown);

    let cars = cops(&mut app);
    assert_eq!(cars.len(), 2, "both authored rows became cars");
    let fleet = app.world().resource::<PoliceFleet>();
    assert_eq!(
        (
            fleet.authored,
            fleet.spawned,
            fleet.unplaceable,
            fleet.load_failed
        ),
        (2, 2, 0, 0)
    );
    assert_eq!(fleet.smoke_detail(), "2/2");

    let mut ids = Vec::new();
    for &e in &cars {
        let world = app.world();
        assert_eq!(world.get::<SessionEntity>(e).unwrap().0, 1, "session-owned");
        ids.push(world.get::<ObjectIdentity>(e).unwrap().0);
        // A cop is simulated scenery-with-a-driver-to-be, never a
        // participant: no player identity, progress, opponent driver or
        // player-vehicle marker.
        assert!(world.get::<Player>(e).is_none());
        assert!(world.get::<RaceProgress>(e).is_none());
        assert!(world.get::<OpponentDriver>(e).is_none());
        assert!(world.get::<PlayerVehicle>(e).is_none());
        // Its own authored vehicle, held in place.
        assert_eq!(world.get::<Vehicle>(e).unwrap().config.mass, 1500.0);
        assert_eq!(world.get::<VehicleInput>(e).unwrap().handbrake, 1.0);
        assert!(world.get::<Position>(e).is_some());
    }
    assert_ne!(ids[0], ids[1], "distinct object ids");

    // Authored poses (read the spec off the component, in file order).
    let mut by_index: Vec<(usize, PoliceSpec, Entity)> = cars
        .iter()
        .map(|&e| {
            let c = app.world().get::<PoliceCar>(e).unwrap();
            (c.index, c.spec.clone(), e)
        })
        .collect();
    by_index.sort_by_key(|(i, ..)| *i);
    assert_eq!(by_index[0].1.position, Vec3::new(40.0, 0.0, 100.0));
    assert_eq!(by_index[1].1.position, Vec3::new(-30.0, 0.0, 60.0));
    for (_, spec, e) in &by_index {
        let p = app.world().get::<Position>(*e).unwrap().0;
        assert!(
            (p.x - spec.position.x).abs() < 0.5 && (p.z - spec.position.z).abs() < 0.5,
            "staged at {:?}, found {p:?}",
            spec.position
        );
        // Facing: the heading is a vehicle yaw — forward (−sin h, −cos h).
        let h = staging_yaw(spec);
        let want = Vec3::new(-h.sin(), 0.0, -h.cos());
        let fwd = app.world().get::<Transform>(*e).unwrap().rotation * Vec3::NEG_Z;
        assert!(fwd.dot(want) > 0.99, "facing {fwd:?}, wanted {want:?}");
    }
    // Heading 90° faces −X; the authored 535 reduced to 175° faces +Z-ish.
    assert!((by_index[0].1.heading_deg.unwrap() - 90.0).abs() < 1e-3);
    assert!((by_index[1].1.heading_deg.unwrap() - 175.0).abs() < 1e-3);

    // The held cars stay on their marks through the countdown and into
    // the race (nothing drives them).
    let starts: Vec<Vec3> = cars
        .iter()
        .map(|&e| app.world().get::<Position>(e).unwrap().0)
        .collect();
    run(&mut app, 300);
    for (&e, start) in cars.iter().zip(starts) {
        let p = app.world().get::<Position>(e).unwrap().0;
        assert!(
            (p - start).length() < 2.0,
            "a parked cop stays put: {start:?} → {p:?}"
        );
    }
}

/// An event with no `[Police]` rows fields none and reports nothing.
#[test]
fn a_copless_event_fields_no_police() {
    let tmp = install(0, "");
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    assert_eq!(phase(&app), SessionPhase::Countdown);
    assert!(cops(&mut app).is_empty());
    let fleet = app.world().resource::<PoliceFleet>();
    assert!(!fleet.any());
    assert_eq!(fleet.spawned, 0);
}

/// MP-4 / single-player: a networked session fields no police — nothing
/// replicates them — but the report keeps the authored denominator.
#[test]
fn a_networked_event_fields_no_police() {
    for authority in [SessionAuthority::Host, SessionAuthority::Remote] {
        let tmp = install(2, TWO_COPS);
        let config = SessionConfig {
            authority,
            ..event_config()
        };
        let mut app = event_app(config, vfs_of(tmp.path()));
        app.update();
        assert_eq!(phase(&app), SessionPhase::Countdown, "{authority:?}");
        assert!(cops(&mut app).is_empty(), "{authority:?}");
        let fleet = app.world().resource::<PoliceFleet>();
        assert_eq!((fleet.authored, fleet.spawned), (2, 0), "{authority:?}");
    }
}

/// An unloadable vehicle or an unusable row skips only its own slot,
/// and the report says which — the lineup is never padded.
#[test]
fn a_bad_row_skips_only_its_slot_and_is_counted() {
    let rows = "vpcop 40 0 100 90 0 15 0.5 50\nvpmissing 10 0 100 0 0 15 0.5 50\nvpcop 20 0 100 inf 0 15 0.5 50\n";
    let tmp = install(3, rows);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    assert_eq!(phase(&app), SessionPhase::Countdown);

    let cars = cops(&mut app);
    assert_eq!(cars.len(), 1, "only the loadable, placeable row spawned");
    assert_eq!(app.world().get::<PoliceCar>(cars[0]).unwrap().index, 0);
    let fleet = app.world().resource::<PoliceFleet>();
    assert_eq!(
        (
            fleet.authored,
            fleet.spawned,
            fleet.unplaceable,
            fleet.load_failed
        ),
        (3, 1, 1, 1)
    );
    assert_eq!(fleet.smoke_detail(), "1/3,uns1,fail1");
}

/// Restart tears the lineup down with the session and fields a fresh
/// one — no stale cop from the old generation survives.
#[test]
fn restart_refields_the_lineup_under_the_new_generation() {
    let tmp = install(2, TWO_COPS);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.update();
    run(&mut app, 5);
    let before = cops(&mut app);
    assert_eq!(before.len(), 2);

    app.world_mut().resource_mut::<SessionControl>().restart = true;
    let mut reached = false;
    for _ in 0..30 {
        app.update();
        if phase(&app) == SessionPhase::Countdown
            && app.world().resource::<Session>().generation() == 2
        {
            reached = true;
            break;
        }
    }
    assert!(reached, "restart never returned to Countdown");

    let after = cops(&mut app);
    assert_eq!(after.len(), 2, "exactly the authored lineup, no duplicates");
    for e in after {
        assert_eq!(app.world().get::<SessionEntity>(e).unwrap().0, 2);
    }
    for e in before {
        assert!(
            app.world().get_entity(e).is_err(),
            "the old generation's cop is gone"
        );
    }
    assert_eq!(app.world().resource::<PoliceFleet>().spawned, 2);
}
