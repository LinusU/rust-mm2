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
use mm2_app::police::{PoliceCar, PoliceFleet, PoliceNav, staging_yaw};
use mm2_app::session::{SessionControl, SpawnPoint, spawn_resets};
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
use mm2_game::{
    EmergencyLights, ParticipantState, Pursuit, PursuitPhase, PursuitPolicy, SessionPhase as Phase,
};
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

    // The signals ride the chase: the chasing cop carries them and the
    // idle one does not.
    assert!(app.world().get::<EmergencyLights>(near).is_some());
    assert!(app.world().get::<EmergencyLights>(far).is_none());
}

#[test]
fn a_chasing_cops_light_bar_clock_runs_and_alternates_its_halves() {
    let (_tmp, mut app) = pursuit_app(NEAR);
    run_to_racing(&mut app);
    let cop = cop_at(&mut app, 0);
    // Reacting is quiet: no lights until the chase is committed.
    assert!(app.world().get::<EmergencyLights>(cop).is_none());
    run(&mut app, 600);
    assert!(matches!(phase_of(&app, cop), PursuitPhase::Pursuing(_)));
    let mut sides = std::collections::BTreeSet::new();
    let mut last = app.world().get::<EmergencyLights>(cop).unwrap().elapsed;
    for _ in 0..600 {
        app.update();
        let lights = *app.world().get::<EmergencyLights>(cop).unwrap();
        assert!(lights.elapsed >= last, "the flash clock never rewinds");
        last = lights.elapsed;
        sides.insert(lights.lit_side());
        if sides.len() == 2 {
            break;
        }
    }
    assert_eq!(sides.len(), 2, "both halves lit within the bound");
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
    assert!(
        app.world().get::<EmergencyLights>(cop).is_none(),
        "a cop that gave up switches its signals off"
    );
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
    assert!(app.world().get::<EmergencyLights>(cop).is_none());
}

/// F20 edge case "respawn while chased": the driver's own reset (the
/// `R` key's `ResetVehicle` bundle) is a teleport, not an escape the
/// cop can follow. The chase ends within the bound, the cop is never
/// carried to the respawn, and a later respawn back into sight starts a
/// fresh chase instead of resuming the old one.
#[test]
fn respawning_while_chased_ends_the_chase_and_a_return_starts_a_new_one() {
    let (_tmp, mut app) = pursuit_app(NEAR);
    let player = local_car(&mut app);
    run_to_racing(&mut app);
    run(&mut app, 180);
    let cop = cop_at(&mut app, 0);
    assert!(matches!(phase_of(&app, cop), PursuitPhase::Pursuing(_)));

    let far = SpawnPoint::new(Vec3::new(60.0, 1.5, -900.0), 0.0);
    for reset in spawn_resets(&far, Some(player)) {
        app.world_mut().write_message(reset);
    }
    let policy = *app.world().resource::<PursuitPolicy>();
    run(&mut app, ((policy.lose_after + 2.0) * 60.0) as usize);
    let landed = pos_of(&app, player);
    assert!(
        landed.z < -800.0,
        "the reset teleported the player: {landed:?}"
    );
    assert!(
        matches!(phase_of(&app, cop), PursuitPhase::Lost(_)),
        "{:?}",
        phase_of(&app, cop)
    );
    assert!(
        pos_of(&app, cop).z > -300.0,
        "the cop was not carried along"
    );
    let report = app.world().resource::<PursuitReport>();
    assert_eq!(
        (report.committed, report.gave_up, report.pursuing),
        (1, 1, 0)
    );
    assert!(app.world().get::<EmergencyLights>(cop).is_none());

    // Back in sight of the stood-down cop: nothing happens until the
    // cooldown ends, then a second, separate chase is committed.
    let near = SpawnPoint::new(pos_of(&app, cop) + Vec3::new(40.0, 0.0, 0.0), 0.0);
    for reset in spawn_resets(&near, Some(player)) {
        app.world_mut().write_message(reset);
    }
    run(&mut app, 60);
    assert_eq!(app.world().resource::<PursuitReport>().committed, 1);
    run(
        &mut app,
        ((policy.cooldown + policy.reaction + 2.0) * 60.0) as usize,
    );
    let report = app.world().resource::<PursuitReport>();
    assert_eq!(report.committed, 2, "{report:?}");
    assert_eq!(report.pursuing, 1);
    assert!(matches!(phase_of(&app, cop), PursuitPhase::Pursuing(_)));
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
    assert!(
        app.world().get::<EmergencyLights>(cop).is_none(),
        "the re-fielded cop starts with its signals off"
    );
}

// ---------- F20-B.2: the chase follows the road ----------

/// A one-road graph bending gently from the cop's post, around the
/// north of the wall the road tests raise at x = 30, to the player's
/// start: centre line (0,140) → (30,127) → (60,140), one lane per side.
/// (The shared synthetic car barely yaws under full lock, so the bend is
/// shallow; the corner-following law itself is covered by the pure
/// `chase_input`/`ChaseRoute` tests and the retail runs.)
fn bend_graph(dz: f32) -> mm2_game::NavGraph {
    use mm2_formats::bai::{Bai, Culling, END_FILL, Road, RoadEnd, RoadSection, RoadSide};
    let centre: [[f32; 3]; 3] = [
        [0.0, 0.0, 140.0 + dz],
        [30.0, 0.0, 127.0 + dz],
        [60.0, 0.0, 140.0 + dz],
    ];
    let mut at = 0.0;
    let mut dist = vec![0.0f32];
    for w in centre.windows(2) {
        at += (w[1][0] - w[0][0]).hypot(w[1][2] - w[0][2]);
        dist.push(at);
    }
    let shifted =
        |dx: f32| -> Vec<[f32; 3]> { centre.iter().map(|p| [p[0] + dx, p[1], p[2]]).collect() };
    let side = |dx: f32| RoadSide {
        lane_count: 1,
        tram_count: 0,
        train_count: 0,
        sidewalk_count: 0,
        ambient_types: 0,
        lane_distances: vec![dist.clone()],
        edge_distances: vec![dx.abs()],
        misc: [0xCD; 40],
        lane_vertices: vec![shifted(dx)],
        tram_vertices: Vec::new(),
        train_vertices: Vec::new(),
        sidewalk_inner: vec![[0.0; 3]; 3],
        sidewalk_outer: vec![[0.0; 3]; 3],
    };
    let dead_end = || RoadEnd {
        intersection: 0,
        fill0: 0xCDCD,
        vehicle_rule_code: 0,
        unknown1: 0,
        intersection_road_index: END_FILL,
        traffic_light_origin: [0.0; 3],
        traffic_light_axis: [0.0; 3],
    };
    let sections = centre
        .iter()
        .zip(&dist)
        .map(|(p, d)| RoadSection {
            distance: *d,
            origin: *p,
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0],
        })
        .collect();
    let bai = Bai {
        roads: vec![Road {
            id: 0,
            flags: 0,
            rooms: vec![1],
            half_width: 7.5,
            base_speed: 15.0,
            right: side(3.75),
            left: side(-3.75),
            sections,
            end: dead_end(),
            start: dead_end(),
        }],
        intersections: Vec::new(),
        culling: Culling {
            large: vec![Vec::new()],
            small: vec![Vec::new()],
        },
    };
    mm2_game::NavGraph::build(&bai).graph
}

/// A wall across the straight line between the cop at x = 0 and the
/// player at x = 60, standing clear of the road's bend.
fn raise_wall(app: &mut App) {
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(1.0, 20.0, 60.0),
        mm2_app::layers::GameLayer::world(),
        Transform::from_xyz(30.0, 5.0, 168.0),
    ));
}

/// Commit the chase, then wall the target off and let the cop run on
/// what it last saw; returns the cop's furthest x after `frames`.
fn chase_past_a_wall(with_roads: bool, frames: usize) -> (f32, PursuitReport) {
    let (_tmp, mut app) = pursuit_app(NEAR);
    if with_roads {
        app.insert_resource(PoliceNav(bend_graph(0.0)));
    }
    run_to_racing(&mut app);
    run(&mut app, 120);
    let cop = cop_at(&mut app, 0);
    assert!(matches!(phase_of(&app, cop), PursuitPhase::Pursuing(_)));
    raise_wall(&mut app);
    let mut furthest = f32::MIN;
    for _ in 0..frames {
        app.update();
        furthest = furthest.max(pos_of(&app, cop).x);
    }
    (furthest, app.world().resource::<PursuitReport>().clone())
}

#[test]
fn a_cop_walled_off_from_the_target_follows_the_road_around_it() {
    let (with_roads, report) = chase_past_a_wall(true, 420);
    assert!(
        with_roads > 36.0,
        "the cop got past the wall along the road: x = {with_roads:.1}"
    );
    assert!(report.planned >= 1, "{report:?}");
    assert_eq!(report.unrouted, 0, "{report:?}");
    // The same scene with no road knowledge drives straight at the last
    // sighting and is stopped by the wall.
    let (without, report) = chase_past_a_wall(false, 420);
    assert!(
        without < 30.0,
        "without roads the wall holds the cop: x = {without:.1}"
    );
    assert_eq!((report.planned, report.unrouted), (0, 0));
}

#[test]
fn an_unconnected_goal_counts_a_failed_query_and_still_chases() {
    // The cop and its target are both far from the graph's one road
    // (snap radius 64 m): the router has nothing to snap to.
    let (_tmp, mut app) = pursuit_app(NEAR);
    app.insert_resource(PoliceNav(bend_graph(400.0)));
    run_to_racing(&mut app);
    // Out of the direct range's sight line: a wall, so the cop is on
    // the router's road line law, not driving at a target in view.
    run(&mut app, 120);
    raise_wall(&mut app);
    let cop = cop_at(&mut app, 0);
    let start = pos_of(&app, cop);
    run(&mut app, 240);
    let report = app.world().resource::<PursuitReport>();
    assert!(report.unrouted >= 1 && report.planned == 0, "{report:?}");
    // One failed query per wait (4 s), not one per frame.
    assert!(report.unrouted <= 2, "{report:?}");
    // And the cop kept chasing straight rather than idling.
    assert!(pos_of(&app, cop).x > start.x + 5.0);
}

#[test]
fn the_road_graph_dies_with_the_session() {
    let (_tmp, mut app) = pursuit_app(NEAR);
    app.insert_resource(PoliceNav(bend_graph(0.0)));
    app.world_mut().resource_mut::<SessionControl>().restart = true;
    run(&mut app, 30);
    // The restart reloads the session; the stand-in graph (inserted
    // outside the loader) is removed by teardown and not re-created for
    // a world with no routing graph.
    assert!(app.world().get_resource::<PoliceNav>().is_none());
}

// ---------------------------------------------------------------------------
// The loader hands the road graph to the cops (F20-B.2, via the real path)
// ---------------------------------------------------------------------------

/// `install` plus a synthetic city: the PSDL and the `CAI1` road fixture
/// `tests/traffic.rs` uses, under the stem `test`. The event stays at
/// `race/testcity/`; the city world is the PSDL's, so the loader's own
/// `load_routing_nav_graph` path runs.
fn city_install(cops: i64, rows: &str, with_bai: bool) -> tempfile::TempDir {
    let tmp = install(cops, rows);
    write(
        tmp.path(),
        "city/test.psdl",
        crate::traffic::synthetic_psdl(),
    );
    if with_bai {
        write(tmp.path(), "city/test.bai", crate::traffic::bai_bytes());
    }
    tmp
}

fn city_event_config() -> SessionConfig {
    SessionConfig {
        world: mm2_game::WorldMode::City {
            psdl: "city/test.psdl".into(),
        },
        ..event_config()
    }
}

fn loaded_city_app(cops: i64, rows: &str, with_bai: bool) -> (tempfile::TempDir, App) {
    let tmp = city_install(cops, rows, with_bai);
    let mut app = event_app(city_event_config(), vfs_of(tmp.path()));
    app.update();
    (tmp, app)
}

#[test]
fn the_loader_gives_fielded_cops_the_citys_road_graph() {
    let (_tmp, app) = loaded_city_app(1, NEAR, true);
    assert_eq!(app.world().resource::<PoliceFleet>().spawned, 1);
    let nav = app
        .world()
        .get_resource::<PoliceNav>()
        .expect("cops in a city with roads get its graph");
    assert!(
        nav.0.stats().vehicle_arcs > 0,
        "the fixture's roads are routable"
    );
}

#[test]
fn a_copless_city_event_keeps_no_second_copy_of_the_roads() {
    let (_tmp, app) = loaded_city_app(0, "", true);
    assert_eq!(app.world().resource::<PoliceFleet>().spawned, 0);
    assert!(app.world().get_resource::<PoliceNav>().is_none());
}

#[test]
fn a_city_with_no_road_graph_still_fields_its_cops_without_one() {
    let (_tmp, app) = loaded_city_app(1, NEAR, false);
    assert_eq!(app.world().resource::<PoliceFleet>().spawned, 1);
    assert!(
        app.world().get_resource::<PoliceNav>().is_none(),
        "no graph is an honest absence, not an invented one"
    );
}

// ---------------------------------------------------------------------------
// `--police-debug`
// ---------------------------------------------------------------------------

#[test]
fn the_debug_overlay_runs_only_when_the_session_asked_and_survives_a_live_chase() {
    use bevy::ecs::system::RunSystemOnce;
    use mm2_app::police_debug::{draw_police_debug, enabled};

    let tmp = install(1, NEAR);
    let off = event_app(event_config(), vfs_of(tmp.path()));
    assert!(
        !run_enabled(off),
        "the overlay is off unless --police-debug set it"
    );

    let mut config = event_config();
    config.dev.police_debug = true;
    let mut app = event_app(config, vfs_of(tmp.path()));
    app.add_systems(Update, (police_pursuit, draw_police_debug.run_if(enabled)));
    app.update();
    run_to_racing(&mut app);
    app.insert_resource(PoliceNav(bend_graph(0.0)));
    run(&mut app, 120);
    assert!(app.world_mut().run_system_once(enabled).unwrap());
    assert!(
        app.world().resource::<PursuitReport>().committed > 0,
        "the drawn cop was really chasing"
    );

    fn run_enabled(mut app: App) -> bool {
        app.update();
        app.world_mut().run_system_once(enabled).unwrap()
    }
}

// ---------------------------------------------------------------------------
// Cruise cops (F20-B.3b): the city's `roam` lineup through the real loader
// ---------------------------------------------------------------------------

/// A Cruise session on the synthetic city (`city/test.psdl`, so the
/// roam record is `race/test/roam.aimap`), with `roam` authored when
/// given. The player spawns at the harness's origin pose.
fn cruise_install(roam: Option<&str>) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/test.psdl", crate::traffic::synthetic_psdl());
    write(d, "city/test.bai", crate::traffic::bai_bytes());
    write_car(d, "vpcop", 1500.0, None);
    if let Some(rows) = roam {
        let n = rows.lines().filter(|l| !l.trim().is_empty()).count();
        write(d, "race/test/roam.aimap", format!("[Police]\n{n}\n{rows}"));
    }
    tmp
}

fn cruise_config(authority: SessionAuthority) -> SessionConfig {
    SessionConfig {
        world: mm2_game::WorldMode::City {
            psdl: "city/test.psdl".into(),
        },
        authority,
        ..SessionConfig::default()
    }
}

fn cruise_app(roam: Option<&str>, authority: SessionAuthority) -> (tempfile::TempDir, App) {
    let tmp = cruise_install(roam);
    let mut app = event_app(cruise_config(authority), vfs_of(tmp.path()));
    app.add_systems(Update, police_pursuit);
    app.update();
    (tmp, app)
}

/// The harness spawns the Cruise player at (0, _, 100): cop 0 stands
/// 30 m beyond it in the open, cop 1 is 135 m off the other end of the
/// synthetic ground — past the 90 m notice range.
const ROAM: &str = "vpcop 0 0 130 0 0 15 0.5 50\nvpcop 0 0 -35 180 0 15 0.5 50\n";

#[test]
fn a_cruise_city_fields_its_roam_lineup_with_the_road_graph() {
    let (_tmp, app) = cruise_app(Some(ROAM), SessionAuthority::Local);
    assert_eq!(
        phase(&app),
        SessionPhase::Playing,
        "Cruise has no countdown"
    );
    let fleet = app.world().resource::<PoliceFleet>();
    assert_eq!((fleet.authored, fleet.spawned), (2, 2));
    let owned = app
        .world()
        .iter_entities()
        .filter(|e| e.contains::<PoliceCar>() && e.contains::<SessionEntity>())
        .count();
    assert_eq!(owned, 2, "session-owned like every other car");
    assert!(
        app.world().get_resource::<PoliceNav>().is_some(),
        "the city's roads are handed to the cops"
    );
    assert!(app.world().get_resource::<PursuitReport>().is_some());
}

#[test]
fn a_cruise_city_with_no_roam_record_fields_no_police() {
    let (_tmp, app) = cruise_app(None, SessionAuthority::Local);
    let fleet = app.world().resource::<PoliceFleet>();
    assert_eq!((fleet.authored, fleet.spawned), (0, 0));
    assert!(!fleet.any(), "nothing authored, so the smoke stays silent");
    assert!(app.world().get_resource::<PoliceNav>().is_none());
}

#[test]
fn a_networked_cruise_fields_no_police() {
    for authority in [SessionAuthority::Host, SessionAuthority::Remote] {
        let (_tmp, app) = cruise_app(Some(ROAM), authority);
        assert!(
            app.world().get_resource::<PoliceFleet>().is_none(),
            "{authority:?}: MP-4 fields no cops, and Cruise adds no exception"
        );
    }
}

#[test]
fn a_dev_world_cruise_fields_no_police() {
    let tmp = cruise_install(Some(ROAM));
    let mut app = event_app(SessionConfig::default(), vfs_of(tmp.path()));
    app.update();
    assert!(app.world().get_resource::<PoliceFleet>().is_none());
}

#[test]
fn a_cruising_player_in_sight_is_chased_and_a_distant_one_is_not() {
    let (_tmp, mut app) = cruise_app(Some(ROAM), SessionAuthority::Local);
    let (near, far) = (cop_at(&mut app, 0), cop_at(&mut app, 1));
    run(&mut app, 600);
    assert!(
        matches!(phase_of(&app, near), PursuitPhase::Pursuing(_)),
        "{:?}",
        phase_of(&app, near)
    );
    assert_eq!(phase_of(&app, far), PursuitPhase::Idle);
    assert!(app.world().get::<EmergencyLights>(near).is_some());
    let report = app.world().resource::<PursuitReport>();
    assert_eq!((report.committed, report.peak), (1, 1));
}

#[test]
fn restarting_a_cruise_refields_the_roam_lineup() {
    let (_tmp, mut app) = cruise_app(Some(ROAM), SessionAuthority::Local);
    let before: Vec<Entity> = cops(&mut app);
    assert_eq!(before.len(), 2);
    app.world_mut().resource_mut::<SessionControl>().restart = true;
    let mut reached = false;
    for _ in 0..30 {
        app.update();
        if phase(&app) == SessionPhase::Playing
            && app.world().resource::<Session>().generation() == 2
        {
            reached = true;
            break;
        }
    }
    assert!(reached, "restart never returned to Playing");
    let after = cops(&mut app);
    assert_eq!(after.len(), 2, "exactly the authored lineup, no duplicates");
    for e in &after {
        assert_eq!(app.world().get::<SessionEntity>(*e).unwrap().0, 2);
    }
    for e in before {
        assert!(
            app.world().get_entity(e).is_err(),
            "the old generation's cop is gone"
        );
    }
    assert_eq!(app.world().resource::<PoliceFleet>().spawned, 2);
}

// ---------- F20-C.2: a long chase stays bounded (AC04) ----------

/// Four cops ringed around the player's start, a cap of two, and a
/// target that keeps moving for a minute of game time: at every frame
/// the pursuer count is the cap's or less and agrees with the cops'
/// own phases, the light bar rides exactly the pursuing cops, nobody
/// leaks or flies off the map, and the wedged-car escapes and
/// deliberate turn-arounds stay inside what their own timers allow.
#[test]
fn a_long_chase_keeps_the_pursuers_capped_and_the_recovery_bounded() {
    use mm2_app::police::PoliceDrive;

    let rows = "vpcop 0 0 140 -90 0 15 0.5 50\nvpcop 120 0 140 90 0 15 0.5 50\n\
                vpcop 60 0 200 0 0 15 0.5 50\nvpcop 60 0 80 180 0 15 0.5 50\n";
    let (_tmp, mut app) = pursuit_app(rows);
    const CAP: usize = 2;
    app.world_mut().resource_mut::<PursuitPolicy>().max_pursuers = CAP;
    let player = local_car(&mut app);
    run_to_racing(&mut app);

    let frames = 60 * 60;
    let mut most_pursuing = 0;
    for frame in 0..frames {
        // The target keeps circling the start at ~12 m/s, ground-bound,
        // so there is always something to chase and nothing to catch.
        let a = frame as f32 / 60.0 * 0.4;
        let at = Vec3::new(60.0 + 30.0 * a.cos(), 1.5, 140.0 + 30.0 * a.sin());
        *app.world_mut().get_mut::<Position>(player).unwrap() = Position(at);
        *app.world_mut().get_mut::<LinearVelocity>(player).unwrap() = LinearVelocity::ZERO;
        app.update();

        let cars = cops(&mut app);
        assert_eq!(cars.len(), 4, "frame {frame}: no cop leaked or vanished");
        let pursuing: Vec<Entity> = cars
            .iter()
            .copied()
            .filter(|&c| matches!(phase_of(&app, c), PursuitPhase::Pursuing(_)))
            .collect();
        assert!(
            pursuing.len() <= CAP,
            "frame {frame}: {} pursuing",
            pursuing.len()
        );
        assert_eq!(
            app.world().resource::<PursuitReport>().pursuing as usize,
            pursuing.len(),
            "frame {frame}: the report agrees with the cops' phases"
        );
        for &c in &cars {
            assert_eq!(
                app.world().get::<EmergencyLights>(c).is_some(),
                pursuing.contains(&c),
                "frame {frame}: signals exactly on the pursuing cops"
            );
            let p = pos_of(&app, c);
            assert!(
                p.is_finite() && p.distance(Vec3::new(60.0, 0.0, 140.0)) < 600.0,
                "frame {frame}: a cop left the map: {p:?}"
            );
        }
        most_pursuing = most_pursuing.max(pursuing.len());
    }

    assert_eq!(most_pursuing, CAP, "the cap was reached and never exceeded");
    let report = app.world().resource::<PursuitReport>();
    assert_eq!(report.peak as usize, CAP);
    // Past one commit per cop, every commit follows a give-up: the cap
    // holds pursuers back, it does not make the machine churn.
    assert!(
        report.committed <= 4 + report.gave_up,
        "chases flapped: {report:?}"
    );
    // One escape/turn-around needs at least its detection window plus
    // both halves of the manoeuvre, so no cop can exceed that rate.
    let per_manoeuvre = 90 + 72 + 45;
    for c in cops(&mut app) {
        let drive = app.world().get::<PoliceDrive>(c).unwrap();
        let total = (drive.bot.escapes + drive.turnarounds) as usize;
        assert!(
            total <= frames / per_manoeuvre + 1,
            "a cop recovered {total} times in {frames} frames"
        );
    }
}
