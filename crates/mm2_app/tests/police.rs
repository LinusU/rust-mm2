//! F20-A.2 police integration: an authored `[Police]` lineup on a
//! synthetic install is fielded through the production
//! `load_session_world` → `police_roster_from_aimap` → `load_opponent`
//! path as session-owned, non-participant cars standing at their
//! authored poses; the F20-A.3 half below adds `police_pursuit` and
//! drives a racing player past them (trigger, control, occlusion,
//! escape, cap, stand-down, restart).

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

// ---------------------------------------------------------------------------
// F20-A.3 — the detect → engage → pursue → lost machine
// ---------------------------------------------------------------------------

use mm2_app::layers::GameLayer;
use mm2_app::police::{PursuitReport, police_pursuit};
use mm2_game::{ParticipantState, Pursuit, PursuitPhase, PursuitPolicy, SessionPhase as Phase};
use std::f32::consts::FRAC_PI_2;

/// The authored lineup under the production schedule plus the pursuit
/// system (the shared harness predates it).
fn pursuit_app(rows: &str) -> (tempfile::TempDir, App) {
    let n = rows.lines().filter(|l| !l.trim().is_empty()).count() as i64;
    let tmp = install(n, rows);
    let mut app = event_app(event_config(), vfs_of(tmp.path()));
    app.add_systems(Update, police_pursuit);
    app.update();
    assert_eq!(phase(&app), Phase::Countdown);
    (tmp, app)
}

fn local_car(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<PlayerVehicle>>()
        .single(app.world())
        .unwrap()
}

fn phase_of(app: &App, cop: Entity) -> PursuitPhase {
    app.world().get::<Pursuit>(cop).unwrap().phase
}

fn pos_of(app: &App, e: Entity) -> Vec3 {
    app.world().get::<Position>(e).unwrap().0
}

/// Run until the race is live (cops are still held through the
/// countdown — asserted by the callers that care).
fn run_to_racing(app: &mut App) {
    let player = local_car(app);
    for _ in 0..1500 {
        app.update();
        if app
            .world()
            .get::<RaceProgress>(player)
            .is_some_and(|p| p.state == ParticipantState::Racing)
        {
            return;
        }
    }
    panic!("the race never went live");
}

/// The player starts on the course at (60, 140): one cop 60 m due west
/// facing it and one 180 m off, beyond any sight.
const NEAR_AND_FAR: &str = "vpcop -120 0 140 90 0 15 0.5 50\nvpcop 0 0 140 -90 0 15 0.5 50\n";
const NEAR: &str = "vpcop 0 0 140 -90 0 15 0.5 50\n";

fn cop_at(app: &mut App, index: usize) -> Entity {
    app.world_mut()
        .query::<(Entity, &PoliceCar)>()
        .iter(app.world())
        .find(|(_, c)| c.index == index)
        .map(|(e, _)| e)
        .unwrap()
}

#[test]
fn a_cop_in_sight_chases_the_racing_player_and_a_far_one_does_not() {
    let (_tmp, mut app) = pursuit_app(NEAR_AND_FAR);
    let (far, near) = (cop_at(&mut app, 0), cop_at(&mut app, 1));
    let player = local_car(&mut app);

    // The countdown holds everyone: no eligible (racing) target yet.
    run(&mut app, 20);
    assert_eq!(phase_of(&app, near), PursuitPhase::Idle);
    assert_eq!(
        app.world().get::<VehicleInput>(near).unwrap().handbrake,
        1.0
    );

    run_to_racing(&mut app);
    let start = pos_of(&app, near).distance(pos_of(&app, player));
    run(&mut app, 600);
    assert!(
        matches!(phase_of(&app, near), PursuitPhase::Pursuing(_)),
        "{:?}",
        phase_of(&app, near)
    );
    let closed = pos_of(&app, near).distance(pos_of(&app, player));
    assert!(
        closed < start - 15.0,
        "the cop closed {start:.1} → {closed:.1}"
    );
    assert_eq!(
        app.world().get::<VehicleInput>(near).unwrap().handbrake,
        0.0
    );

    // The far one never saw anyone (180 m > the detect range) and
    // stays on its mark.
    assert_eq!(phase_of(&app, far), PursuitPhase::Idle);
    assert_eq!(app.world().get::<VehicleInput>(far).unwrap().handbrake, 1.0);

    let report = app.world().resource::<PursuitReport>();
    assert_eq!((report.committed, report.gave_up, report.peak), (1, 0, 1));
}

#[test]
fn the_control_scenario_with_every_cop_out_of_range_never_chases() {
    let (_tmp, mut app) = pursuit_app("vpcop -120 0 140 90 0 15 0.5 50\n");
    run_to_racing(&mut app);
    run(&mut app, 600);
    let cop = cop_at(&mut app, 0);
    assert_eq!(phase_of(&app, cop), PursuitPhase::Idle);
    assert_eq!(
        *app.world().resource::<PursuitReport>(),
        PursuitReport::default()
    );
}

#[test]
fn a_wall_between_them_blocks_the_sighting() {
    let (_tmp, mut app) = pursuit_app(NEAR);
    // A static city-geometry wall across the sight line.
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(80.0, 20.0, 1.0),
        mm2_app::layers::GameLayer::world(),
        Transform::from_xyz(30.0, 5.0, 140.0).with_rotation(Quat::from_rotation_y(FRAC_PI_2)),
    ));
    run_to_racing(&mut app);
    run(&mut app, 300);
    let cop = cop_at(&mut app, 0);
    assert_eq!(phase_of(&app, cop), PursuitPhase::Idle, "walled off");
    assert_eq!(app.world().resource::<PursuitReport>().noticed, 0);
}

#[test]
fn another_car_does_not_hide_the_target() {
    // The same sight line, blocked by a *default-layer* box (a car):
    // only `World` geometry occludes.
    let (_tmp, mut app) = pursuit_app(NEAR);
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(80.0, 20.0, 1.0),
        CollisionLayers::new(GameLayer::Default, LayerMask::ALL),
        Transform::from_xyz(30.0, 5.0, 140.0).with_rotation(Quat::from_rotation_y(FRAC_PI_2)),
    ));
    run_to_racing(&mut app);
    run(&mut app, 240);
    let cop = cop_at(&mut app, 0);
    assert!(matches!(phase_of(&app, cop), PursuitPhase::Pursuing(_)));
}

#[test]
fn escaping_the_cops_sight_ends_the_chase_within_the_bound() {
    let (_tmp, mut app) = pursuit_app(NEAR);
    let player = local_car(&mut app);
    run_to_racing(&mut app);
    run(&mut app, 180);
    let cop = cop_at(&mut app, 0);
    assert!(matches!(phase_of(&app, cop), PursuitPhase::Pursuing(_)));

    // The player is gone — far beyond the contact range.
    *app.world_mut().get_mut::<Position>(player).unwrap() = Position(Vec3::new(60.0, 1.5, -900.0));
    let policy = *app.world().resource::<PursuitPolicy>();
    let frames = ((policy.lose_after + 2.0) * 60.0) as usize;
    run(&mut app, frames);
    assert!(
        matches!(phase_of(&app, cop), PursuitPhase::Lost(_)),
        "{:?}",
        phase_of(&app, cop)
    );
    let report = app.world().resource::<PursuitReport>();
    assert_eq!(
        (report.committed, report.gave_up, report.pursuing),
        (1, 1, 0)
    );
    // It drove to where it last saw the player, not on to the player's
    // new position (the machine is never told where the target went).
    let p = pos_of(&app, cop);
    assert!(
        p.z > -300.0,
        "the cop did not follow an unseen target: {p:?}"
    );
    let input = app.world().get::<VehicleInput>(cop).unwrap();
    assert_eq!(input.throttle, 0.0, "stood down, not still chasing");
}

#[test]
fn only_the_policy_cap_may_pursue_at_once() {
    let rows = "vpcop 0 0 140 -90 0 15 0.5 50\nvpcop 60 0 200 0 0 15 0.5 50\nvpcop 60 0 80 180 0 15 0.5 50\n";
    let (_tmp, mut app) = pursuit_app(rows);
    app.world_mut().resource_mut::<PursuitPolicy>().max_pursuers = 2;
    run_to_racing(&mut app);
    run(&mut app, 300);
    let report = app.world().resource::<PursuitReport>();
    assert_eq!(report.peak, 2, "capped");
    assert_eq!(report.pursuing, 2);
    let watching = (0..3)
        .filter(|&i| {
            let c = cop_at(&mut app, i);
            matches!(phase_of(&app, c), PursuitPhase::Engaged(_))
        })
        .count();
    assert_eq!(watching, 1, "the third cop waits, watching");
}

#[test]
fn a_player_who_is_no_longer_racing_stands_the_cops_down() {
    let (_tmp, mut app) = pursuit_app(NEAR);
    let player = local_car(&mut app);
    run_to_racing(&mut app);
    run(&mut app, 180);
    let cop = cop_at(&mut app, 0);
    assert!(matches!(phase_of(&app, cop), PursuitPhase::Pursuing(_)));

    // The target is no longer an eligible, racing participant (a
    // finished/timed-out race has the same effect; progress is only
    // ever written by the race authority, so this stands in for it).
    app.world_mut()
        .get_mut::<RaceProgress>(player)
        .unwrap()
        .state = ParticipantState::AwaitingStart;
    run(&mut app, 2);
    assert_eq!(phase_of(&app, cop), PursuitPhase::Idle);
    assert_eq!(app.world().get::<VehicleInput>(cop).unwrap().handbrake, 1.0);
}

#[test]
fn restart_clears_the_pursuit_report_and_re_arms_every_cop() {
    let (_tmp, mut app) = pursuit_app(NEAR);
    run_to_racing(&mut app);
    run(&mut app, 180);
    assert_eq!(app.world().resource::<PursuitReport>().committed, 1);

    app.world_mut().resource_mut::<SessionControl>().restart = true;
    let mut reached = false;
    for _ in 0..30 {
        app.update();
        if phase(&app) == Phase::Countdown && app.world().resource::<Session>().generation() == 2 {
            reached = true;
            break;
        }
    }
    assert!(reached);
    assert_eq!(
        *app.world().resource::<PursuitReport>(),
        PursuitReport::default()
    );
    let cop = cop_at(&mut app, 0);
    assert_eq!(*app.world().get::<Pursuit>(cop).unwrap(), Pursuit::new());
}
