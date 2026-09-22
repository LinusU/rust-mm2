//! F05-B.5 integration: water/OOB detectors read the real wheel
//! contacts the physics step leaves, fire on the designed dwell/margin
//! and resolve through the production `ResetVehicle` path — back to the
//! last dry-grounded pose for the local driver, AI opponents and
//! trailer rigs; remote participants and `Disabled` wrecks are someone
//! else's business.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::contracts::{self, ImpactFilter};
use mm2_app::damage::{self, DamageReport};
use mm2_app::recovery::{self, RecoveryReport};
use mm2_app::session::{SessionControl, SpawnPoint};
use mm2_app::stuck::{self, StuckReport};
use mm2_game::{
    DamageEvent, DamageSpec, ImpactEvent, ImpactId, ObjectId, ObjectIdentity, Player,
    PlayerControl, RecoveryCause, RecoveryEvent, RecoveryPolicy, Session, SessionConfig,
    SessionPhase, StuckEvent, StuckSpec, VehicleDamage, VehicleRecovery, VehicleStuck,
    advance_session_tick,
};
use mm2_vehicle::{TireConditions, TireSurface, VehicleConfig, VehiclePlugin, vehicle_bundle};

/// A test policy: dwell and fall margin shortened so episodes resolve
/// in tens of frames; `water_min_drag` keeps the production split
/// (deepwater 0.5 drowns, shallow `water` 0.119 wades).
const POLICY: RecoveryPolicy = RecoveryPolicy {
    water_min_drag: 0.3,
    submerge_dwell: 0.5,
    fall_margin: 8.0,
};
const DAMAGE_SPEC: DamageSpec = DamageSpec {
    impact_threshold: 1500.0,
    med_damage: 150_000.0,
    max_damage: 321_300.0,
    regenerate_rate: 0.0,
};
const STUCK_SPEC: StuckSpec = StuckSpec {
    time_thresh: 30.0,
    pos_thresh: 1.25,
    move_thresh: 1.75,
    turn: std::f32::consts::PI,
    rotation: 0.0,
    translation: 0.1,
};
const SPAWN: Vec3 = Vec3::new(0.0, 1.5, 0.0);
const SPAWN_YAW: f32 = 0.25;
/// The water slab sits clear of the dry floor's centre.
const WATER_AT: Vec3 = Vec3::new(60.0, -0.5, 0.0);

/// A playing session on a dry ground plane plus a deep-water slab and
/// the player car carrying the recovery detector. Returns the app, the
/// car entity, its object id and the settled pose (the live anchor).
fn recovery_app(car_pos: Vec3) -> (App, Entity, ObjectId) {
    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    let object = session.mint_object_id();
    let player = session.mint_player_id();
    let role = session.authority_role();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .insert_resource(session)
        .insert_resource(TireConditions::default())
        .insert_resource(SpawnPoint {
            position: SPAWN,
            yaw: SPAWN_YAW,
            trailers: Vec::new(),
        })
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .add_message::<DamageEvent>()
        .add_message::<StuckEvent>()
        .add_message::<RecoveryEvent>()
        .insert_resource(ImpactFilter::default())
        .init_resource::<DamageReport>()
        .init_resource::<StuckReport>()
        .init_resource::<RecoveryReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<SessionControl>()
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (
                contracts::collect_impacts,
                damage::apply_impact_damage,
                stuck::track_stuck,
                damage::resolve_disabled,
                stuck::resolve_stuck,
                recovery::track_recovery,
                recovery::resolve_recovery,
            )
                .chain(),
        );
    app.finish();
    app.cleanup();

    // Dry ground under the spawn area…
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(80.0, 1.0, 80.0),
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    // …and a deep-water slab clear of it — the `drag` the Thames
    // class authors, consumed through the wheel raycast like retail.
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(20.0, 1.0, 20.0),
        Position(WATER_AT),
        Transform::from_translation(WATER_AT),
        TireSurface {
            grip: 1.0,
            drag: 0.5,
        },
    ));

    let car = app
        .world_mut()
        .spawn((
            mm2_game::PlayerVehicle,
            ObjectIdentity(object),
            Player {
                id: player,
                control: PlayerControl::Local,
            },
            role,
            mm2_game::DamageSignals::default(),
            VehicleRecovery::with_anchor(POLICY, car_pos, SPAWN_YAW),
            vehicle_bundle(&VehicleConfig::default()),
            Position(car_pos),
            Transform::from_translation(car_pos),
        ))
        .id();

    // Settle onto the dry floor so the live anchor is the resting
    // pose, not the spawn-air pose.
    run(&mut app, 30);
    (app, car, object)
}

/// Teleport the car: the same writes the stuck/damage suites use —
/// `Position` + `Transform` with motion cleared.
fn teleport(app: &mut App, car: Entity, pos: Vec3) {
    let world = app.world_mut();
    world.get_mut::<Position>(car).unwrap().0 = pos;
    world.get_mut::<Transform>(car).unwrap().translation = pos;
    world.get_mut::<LinearVelocity>(car).unwrap().0 = Vec3::ZERO;
    world.get_mut::<AngularVelocity>(car).unwrap().0 = Vec3::ZERO;
}

fn drain_recovery(app: &mut App) -> Vec<RecoveryEvent> {
    app.world_mut()
        .resource_mut::<Messages<RecoveryEvent>>()
        .drain()
        .collect()
}

fn report(app: &App) -> &RecoveryReport {
    app.world().resource::<RecoveryReport>()
}

fn position(app: &App, car: Entity) -> Vec3 {
    app.world().get::<Position>(car).unwrap().0
}

/// Run `frames` 60 Hz updates — about `frames / 60` seconds of session
/// time.
fn run(app: &mut App, frames: usize) {
    for _ in 0..frames {
        app.update();
    }
}

#[test]
fn a_car_on_deep_water_recovers_to_the_last_dry_anchor() {
    let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    let dry_anchor = position(&app, car);
    assert!(
        app.world()
            .get::<VehicleRecovery>(car)
            .unwrap()
            .anchor()
            .is_some_and(|(p, _)| p.distance(dry_anchor) < 0.1),
        "settling on the dry floor anchors the detector"
    );
    drain_recovery(&mut app);

    // Washed out onto the deep-water slab: every grounded wheel reads
    // the authored `drag` and the dwell accrues.
    teleport(&mut app, car, Vec3::new(WATER_AT.x, 1.0, WATER_AT.z));
    run(&mut app, 80); // settle ~0.5 s + dwell 0.5 s

    assert_eq!(report(&app).submerged, 1);
    assert_eq!(report(&app).out_of_bounds, 0);
    assert_eq!(report(&app).recovered, 1);
    let pos = position(&app, car);
    assert!(
        pos.distance(Vec3::new(dry_anchor.x, pos.y, dry_anchor.z)) < 1.0,
        "recovery lands on the last dry pose, got {pos} vs {dry_anchor}"
    );
    assert!(
        app.world().get::<mm2_vehicle::Teleported>(car).is_some(),
        "the recovery hop must break swept segments like every reset"
    );
    // Back on dry ground the episode is over — no second fire while
    // the car stays put.
    run(&mut app, 80);
    assert_eq!(report(&app).submerged, 1);
    assert_eq!(report(&app).recovered, 1);
}

#[test]
fn wading_shallow_water_never_starts_the_dwell() {
    // The `water` class (drag 0.119) stays wadable under the designed
    // split — parked on it reads as dry purchase, the anchor even
    // tracks it, and nothing fires.
    let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(20.0, 1.0, 20.0),
        Position(Vec3::new(-60.0, -0.5, 0.0)),
        Transform::from_xyz(-60.0, -0.5, 0.0),
        TireSurface {
            grip: 1.0,
            drag: 0.119,
        },
    ));
    teleport(&mut app, car, Vec3::new(-60.0, 1.0, 0.0));
    run(&mut app, 80);
    assert_eq!(report(&app).submerged, 0);
    assert_eq!(report(&app).recovered, 0);
    assert_eq!(
        app.world()
            .get::<VehicleRecovery>(car)
            .unwrap()
            .submerged_for(),
        0.0
    );
}

#[test]
fn reaching_dry_ground_inside_the_dwell_escapes() {
    let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    teleport(&mut app, car, Vec3::new(WATER_AT.x, 1.0, WATER_AT.z));
    run(&mut app, 20); // on the water, inside the 0.5 s dwell
    assert!(
        report(&app).submerged == 0,
        "still inside the dwell — nothing fired yet"
    );
    // Powered back onto the shore before the dwell ran out.
    teleport(&mut app, car, Vec3::new(5.0, 1.0, 0.0));
    run(&mut app, 80);
    assert_eq!(report(&app).submerged, 0);
    assert_eq!(report(&app).recovered, 0);
}

#[test]
fn a_fall_off_the_world_recovers_to_the_anchor() {
    let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    let anchor = position(&app, car);
    // Over the floor's edge there is no ground: the car falls past
    // `fall_margin` below its anchor — through-the-world, not a jump.
    teleport(&mut app, car, Vec3::new(500.0, 5.0, 0.0));
    run(&mut app, 150); // ~2.5 s of falling — well past the 8 m margin

    assert_eq!(report(&app).out_of_bounds, 1);
    assert_eq!(report(&app).recovered, 1);
    let pos = position(&app, car);
    assert!(
        pos.distance(Vec3::new(anchor.x, pos.y, anchor.z)) < 1.0,
        "recovered to the last dry pose, got {pos} vs {anchor}"
    );
    // One event per fall: back at rest on the anchor, no re-fire.
    run(&mut app, 60);
    assert_eq!(report(&app).out_of_bounds, 1);
}

#[test]
fn a_legitimate_big_drop_lands_instead_of_firing() {
    // Falling onto real ground refreshes the anchor at the landing —
    // the margin follows the car down and never fires. This car
    // carries a wider margin so the drop lands inside it: the
    // anchor-relative bound fires on *any* airborne fall deeper than
    // `fall_margin`, legit or not — the bound is sized past retail
    // drops precisely so a real landing always wins.
    let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    let settled = position(&app, car);
    app.world_mut()
        .entity_mut(car)
        .insert(VehicleRecovery::with_anchor(
            RecoveryPolicy {
                fall_margin: 30.0,
                ..POLICY
            },
            settled,
            SPAWN_YAW,
        ));
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(20.0, 1.0, 20.0),
        Position(Vec3::new(0.0, -16.5, 60.0)),
        Transform::from_xyz(0.0, -16.5, 60.0),
    ));
    teleport(&mut app, car, Vec3::new(0.0, 1.0, 60.0));
    run(&mut app, 120);
    assert_eq!(report(&app).out_of_bounds, 0);
    assert_eq!(report(&app).recovered, 0);
    assert!(
        position(&app, car).y < -14.0,
        "the car really landed 16 m down, at {}",
        position(&app, car).y
    );
    // And the landing refreshed the anchor — a *further* fall is
    // measured from here, not from the spawn height.
    let anchor = app.world().get::<VehicleRecovery>(car).unwrap().anchor();
    assert!(
        anchor.is_some_and(|(p, _)| p.y < -14.0),
        "the anchor followed the car down: {anchor:?}"
    );
}

// No integration leg for a non-finite `Position`: the physics
// raycasts (`obvhs`) panic on a NaN origin upstream of `track_recovery`
// in the same fixed step, so the detector's non-finite arm can only
// ever answer poses written between steps — it is exercised at the
// `mm2_game` unit level (`a_non_finite_pose_recovers_to_the_anchor_at_once`)
// and stays as defence-in-depth, not a demonstrated pipeline path.

#[test]
fn recovery_is_not_a_repair() {
    // A damaged car recovered from the water keeps its damage — only
    // the disabled outcome's repair heals (F05-B.1 contract).
    let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    {
        let world = app.world_mut();
        let mut damage = VehicleDamage::new(DAMAGE_SPEC);
        damage.apply(ImpactId(1), 50_000.0);
        world.entity_mut(car).insert(damage);
    }
    teleport(&mut app, car, Vec3::new(WATER_AT.x, 1.0, WATER_AT.z));
    run(&mut app, 80);
    assert_eq!(report(&app).recovered, 1);
    let damage = app.world().get::<VehicleDamage>(car).unwrap();
    assert_eq!(damage.total(), 50_000.0, "recovery never heals");
}

#[test]
fn a_remote_participant_is_never_tracked_or_recovered() {
    let (mut app, _car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    let remote_object = app.world_mut().resource_mut::<Session>().mint_object_id();
    let remote_player = app.world_mut().resource_mut::<Session>().mint_player_id();
    let role = app.world().resource::<Session>().authority_role();
    let remote = app
        .world_mut()
        .spawn((
            ObjectIdentity(remote_object),
            Player {
                id: remote_player,
                control: PlayerControl::Remote,
            },
            role,
            mm2_game::DamageSignals::default(),
            VehicleRecovery::with_anchor(POLICY, Vec3::new(5.0, 1.2, 5.0), 0.0),
            vehicle_bundle(&VehicleConfig::default()),
            Position(Vec3::new(5.0, 1.2, 5.0)),
            Transform::from_xyz(5.0, 1.2, 5.0),
        ))
        .id();
    run(&mut app, 30);
    drain_recovery(&mut app);
    app.world_mut().resource_mut::<RecoveryReport>().reset();

    // F05 req 6: a predicted client's detector is its authority's —
    // neither observed nor resolved here.
    teleport(&mut app, remote, Vec3::new(WATER_AT.x, 1.0, WATER_AT.z));
    run(&mut app, 90);
    assert_eq!(report(&app).submerged, 0);
    assert_eq!(report(&app).recovered, 0);
}

#[test]
fn a_disabled_wreck_is_not_recovery_observed() {
    // A car at MaxDamage belongs to `resolve_disabled`'s outcome — the
    // recovery detector must not run a second reset underneath it.
    let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    {
        let world = app.world_mut();
        let mut damage = VehicleDamage::new(DAMAGE_SPEC);
        damage.apply(ImpactId(1), DAMAGE_SPEC.max_damage);
        world.entity_mut(car).insert(damage);
    }
    teleport(&mut app, car, Vec3::new(WATER_AT.x, 1.0, WATER_AT.z));
    run(&mut app, 90);
    assert_eq!(
        report(&app).submerged,
        0,
        "the disabled outcome owns the wreck"
    );
    assert_eq!(report(&app).recovered, 0);
}

#[test]
fn stale_generation_events_are_ignored() {
    let (mut app, car, object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    app.world_mut()
        .resource_mut::<Messages<RecoveryEvent>>()
        .write(RecoveryEvent {
            object,
            generation: 99,
            tick: 0,
            cause: RecoveryCause::Submerged,
            landing: Some((Vec3::new(9.0, 1.0, 9.0), 0.0)),
        });
    app.update();
    assert_eq!(report(&app).recovered, 0);
    assert!(
        app.world().get::<mm2_vehicle::Teleported>(car).is_none(),
        "a stale event must not reset the live car"
    );
}

#[test]
fn recovery_disarms_an_armed_stuck_episode() {
    // The recovery owns the pose it lands the car in — a stuck
    // episode armed before the dunking must not fire a second reset
    // into the recovered pose. The episode's escape bound is widened
    // so the water trip stays inside its hysteresis band: without the
    // resolver's disarm the anchor would survive the whole test.
    let mut wide = STUCK_SPEC;
    wide.move_thresh = 1000.0;
    let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    {
        let pos = position(&app, car);
        let mut stuck = VehicleStuck::new(wide);
        stuck.impact(pos, Quat::IDENTITY);
        app.world_mut().entity_mut(car).insert(stuck);
    }
    teleport(&mut app, car, Vec3::new(WATER_AT.x, 1.0, WATER_AT.z));
    run(&mut app, 90);
    assert_eq!(report(&app).recovered, 1);
    assert!(
        !app.world().get::<VehicleStuck>(car).unwrap().armed(),
        "the recovery disarmed the armed episode"
    );
}

#[test]
fn a_trailer_rig_recovers_with_the_tractor() {
    // F05-AC05's articulated leg: the authored offsets seat the
    // trailer behind the recovered tractor — the `resolve_stuck`
    // pattern on the anchor pose.
    let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    let trailer_pos = Vec3::new(0.0, 1.0, 5.0);
    let trailer = app
        .world_mut()
        .spawn((
            vehicle_bundle(&VehicleConfig::default()),
            Position(trailer_pos),
            Transform::from_translation(trailer_pos),
        ))
        .id();
    let offset = Vec3::new(0.0, -0.2, 4.0);
    app.world_mut()
        .resource_mut::<SpawnPoint>()
        .trailers
        .push((trailer, offset));

    teleport(&mut app, car, Vec3::new(WATER_AT.x, 1.0, WATER_AT.z));
    run(&mut app, 90);
    assert_eq!(report(&app).recovered, 1);

    let car_pos = position(&app, car);
    let car_yaw = {
        let f = app.world().get::<Rotation>(car).unwrap().0 * Vec3::NEG_Z;
        (-f.x).atan2(-f.z)
    };
    let expected = car_pos + Quat::from_rotation_y(car_yaw) * offset;
    let t_pos = position(&app, trailer);
    assert!(
        t_pos.distance(expected) < 0.5,
        "trailer re-seated at its authored offset: {t_pos} vs {expected}"
    );
}
