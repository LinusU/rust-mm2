//! F05-B.5 integration: water/OOB detectors read the real wheel
//! contacts the physics step leaves, fire on the designed dwell/margin
//! and resolve through the production `ResetVehicle` path — back to the
//! last dry-grounded pose for the local driver, AI opponents and remote
//! drivers alike; `Disabled` wrecks and predicted sessions are someone
//! else's business.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::city::WorldFloor;
use mm2_app::contracts::{self, ImpactFilter};
use mm2_app::damage::{self, DamageReport};
use mm2_app::recovery::{self, RecoveryReport};
use mm2_app::session::{SessionControl, SpawnPoint};
use mm2_app::stuck::{self, StuckReport};
use mm2_game::{
    DamageEvent, DamageSpec, ImpactEvent, ImpactId, ObjectId, ObjectIdentity, Player,
    PlayerControl, RecoveryCause, RecoveryEvent, RecoveryPolicy, Session, SessionAuthority,
    SessionConfig, SessionPhase, StuckEvent, StuckSpec, VehicleDamage, VehicleRecovery,
    VehicleStuck, advance_session_tick,
};
use mm2_vehicle::{TireConditions, TireSurface, VehicleConfig, VehiclePlugin, vehicle_bundle};

/// A test policy: dwell and fall margin shortened so episodes resolve
/// in tens of frames; `water_min_drag` keeps the production split
/// (deepwater 0.5 drowns, shallow `water` 0.119 wades).
const POLICY: RecoveryPolicy = RecoveryPolicy {
    water_min_drag: 0.3,
    submerge_dwell: 0.5,
    fall_margin: 8.0,
    floor_margin: 2.0,
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
    recovery_app_with(SessionConfig::default(), car_pos)
}

/// `recovery_app` under a caller-chosen session config — the authority
/// legs need a `Host`/`Remote` stamp.
fn recovery_app_with(config: SessionConfig, car_pos: Vec3) -> (App, Entity, ObjectId) {
    let mut session = Session::new();
    session.begin(config).unwrap();
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
        .insert_resource(SpawnPoint::new(SPAWN, SPAWN_YAW))
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
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<mm2_app::texel_fx::TexelDamageReport>()
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
        )
        // The binary's trailer-reseat follower — reads the resolvers'
        // `ResetVehicle`s ahead of the apply like the real schedule.
        .add_systems(
            Update,
            mm2_app::session::reseat_towed_trailers.before(mm2_vehicle::systems::vehicle_reset),
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

/// The retail-city shape of the same rule: a hill jump drops past the
/// fall margin, but the world floor is far below, so the car is left to
/// land on the street instead of being snatched back mid-air.
#[test]
fn a_fall_past_the_margin_but_over_the_floor_is_left_to_land() {
    let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    app.world_mut().insert_resource(WorldFloor(-60.0));
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(20.0, 1.0, 20.0),
        Position(Vec3::new(0.0, -26.5, 60.0)),
        Transform::from_xyz(0.0, -26.5, 60.0),
    ));
    teleport(&mut app, car, Vec3::new(0.0, 1.0, 60.0));
    run(&mut app, 180);
    assert_eq!(
        report(&app).out_of_bounds,
        0,
        "no mid-air reset over ground"
    );
    assert_eq!(report(&app).recovered, 0);
    assert!(
        position(&app, car).y < -24.0,
        "the car really landed 26 m down (past the 8 m margin), at {}",
        position(&app, car).y
    );
}

/// …and with the same floor a car that really leaves the world still
/// comes back, once it is under the floor rather than merely past the
/// margin.
#[test]
fn a_fall_under_the_world_floor_recovers_to_the_anchor() {
    let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    let anchor = position(&app, car);
    app.world_mut().insert_resource(WorldFloor(-20.0));
    teleport(&mut app, car, Vec3::new(500.0, 5.0, 0.0));
    run(&mut app, 60); // ~1 s: past the 8 m margin, still above the floor
    assert_eq!(report(&app).out_of_bounds, 0);
    run(&mut app, 200); // well under the floor
    assert_eq!(report(&app).out_of_bounds, 1);
    assert_eq!(report(&app).recovered, 1);
    let pos = position(&app, car);
    assert!(
        pos.distance(Vec3::new(anchor.x, pos.y, anchor.z)) < 1.0,
        "recovered to the last dry pose, got {pos} vs {anchor}"
    );
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
fn a_remote_driver_is_tracked_and_recovered_by_the_authority() {
    // F25-A.4: under a hosted session the remote car is this
    // authority's simulated participant — a dunk fires the same
    // recovery episode the AI leg gets, back to its own anchor.
    let hosted = SessionConfig {
        authority: SessionAuthority::Host,
        ..SessionConfig::default()
    };
    let (mut app, _car, _object) = recovery_app_with(hosted, Vec3::new(0.0, 1.2, 0.0));
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

    teleport(&mut app, remote, Vec3::new(WATER_AT.x, 1.0, WATER_AT.z));
    run(&mut app, 90);
    assert_eq!(report(&app).submerged, 1);
    assert_eq!(report(&app).recovered, 1);
    let pos = app.world().get::<Position>(remote).unwrap().0;
    assert!(
        pos.distance(Vec3::new(5.0, pos.y, 5.0)) < 1.0,
        "the remote car lands back on its dry anchor, got {pos}"
    );
}

#[test]
fn a_predicted_session_never_tracks_or_recovers() {
    // The complement: under a `Remote` (predicted) session the
    // detectors are inert — a dunked copy accrues no dwell and the
    // buffered recovery event drains unresolved.
    let predicted = SessionConfig {
        authority: SessionAuthority::Remote,
        ..SessionConfig::default()
    };
    let (mut app, car, object) = recovery_app_with(predicted, Vec3::new(0.0, 1.2, 0.0));
    run(&mut app, 30);
    drain_recovery(&mut app);
    app.world_mut().resource_mut::<RecoveryReport>().reset();

    teleport(&mut app, car, Vec3::new(WATER_AT.x, 1.0, WATER_AT.z));
    run(&mut app, 90);
    assert_eq!(report(&app).submerged, 0);
    assert_eq!(report(&app).recovered, 0);
    // Even a hand-written event cannot resolve under a predicted
    // session — the drain drops it.
    let generation = app.world().resource::<Session>().generation();
    app.world_mut()
        .resource_mut::<Messages<RecoveryEvent>>()
        .write(RecoveryEvent {
            object,
            generation,
            tick: 0,
            cause: RecoveryCause::Submerged,
            landing: Some((Vec3::new(0.0, 1.0, 0.0), 0.0)),
        });
    app.update();
    assert_eq!(report(&app).recovered, 0);
    assert!(
        app.world().get::<mm2_vehicle::Teleported>(car).is_none(),
        "a predicted session never resets its own car"
    );
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

// ---------------------------------------------------------------------------
// F18-A.6 — the authored `.water` deadly-room overlay
// ---------------------------------------------------------------------------

/// A `CityWater` bound over a synthetic one-room PSDL: room 1 covers
/// authored `x ∈ [x0,x1], z ∈ [z0,z1]` as a flat plane at `room_y` and
/// is the `.water` file's only ref — the Thames-room shape the retail
/// refs describe (1-based room ids, level just above the plane).
fn deadly_water(x: [f32; 2], z: [f32; 2], room_y: f32, level: f32) -> mm2_app::water::CityWater {
    use mm2_formats::psdl::{PerimeterPoint, Psdl, PsdlRoom};
    let psdl = Psdl {
        target_size: 2,
        vertices: vec![
            [x[0], room_y, z[0]],
            [x[1], room_y, z[0]],
            [x[1], room_y, z[1]],
            [x[0], room_y, z[1]],
        ],
        heights: Vec::new(),
        textures: Vec::new(),
        rooms: vec![PsdlRoom {
            perimeter: (0..4)
                .map(|k| PerimeterPoint { vertex: k, room: 0 })
                .collect(),
            attributes: Vec::new(),
            unparsed_attributes: Vec::new(),
        }],
        room_flags: Vec::new(),
        prop_rules: Vec::new(),
        junction_count: 0,
        bounds_min: [0.0; 3],
        bounds_max: [0.0; 3],
        bounds_center: [0.0; 3],
        bounds_radius: 0.0,
        paths: Vec::new(),
    };
    let def = mm2_formats::water::WaterDef {
        level,
        refs: vec![1],
    };
    mm2_app::water::CityWater::build(&def, &psdl, None)
}

/// A dragless slab inside a listed deadly room at/below its bound
/// drowns even though `surface_drag` alone reads as ordinary ground —
/// the room is authored deadly, not the material.
#[test]
fn a_dry_floor_inside_a_deadly_water_room_still_drowns() {
    let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    // Deadly room over x∈[45,75], z∈[-35,35] (symmetric z — the mirror
    // is a no-op); bound = max(level 0.5, room top 0.0) = 0.5.
    app.world_mut()
        .insert_resource(deadly_water([45.0, 75.0], [-35.0, 35.0], 0.0, 0.5));
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(20.0, 1.0, 20.0),
        Position(Vec3::new(60.0, -0.5, 25.0)),
        Transform::from_xyz(60.0, -0.5, 25.0),
    ));
    teleport(&mut app, car, Vec3::new(60.0, 1.0, 25.0));
    run(&mut app, 80);
    assert_eq!(report(&app).submerged, 1);
    assert_eq!(report(&app).recovered, 1);
}

/// A deck above the bound inside the room's XZ perimeter stays dry —
/// the bound still applies vertically, so a bridge over deadly water
/// is not a kill.
#[test]
fn a_deck_above_the_bound_inside_a_deadly_room_stays_dry() {
    let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    app.world_mut()
        .insert_resource(deadly_water([45.0, 75.0], [-35.0, 35.0], 0.0, 0.5));
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(20.0, 1.0, 20.0),
        Position(Vec3::new(60.0, 4.5, 25.0)),
        Transform::from_xyz(60.0, 4.5, 25.0),
    ));
    teleport(&mut app, car, Vec3::new(60.0, 5.6, 25.0));
    run(&mut app, 60);
    assert_eq!(report(&app).submerged, 0);
    assert_eq!(report(&app).recovered, 0);
    assert!(
        position(&app, car).y > 4.5,
        "the car really stands on the deck, at {}",
        position(&app, car).y
    );
}

/// London's real BAI lanes run down to y ≈ −22 under the −3.8 level:
/// the bound is scoped to the listed rooms, never global — a dragless
/// floor below the level but outside every listed room is ordinary
/// ground.
#[test]
fn a_below_grade_road_outside_the_rooms_never_drowns() {
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
    app.world_mut()
        .insert_resource(deadly_water([45.0, 75.0], [-35.0, 35.0], 0.0, 0.5));
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(20.0, 1.0, 20.0),
        Position(Vec3::new(-60.0, -6.5, 0.0)),
        Transform::from_xyz(-60.0, -6.5, 0.0),
    ));
    teleport(&mut app, car, Vec3::new(-60.0, -5.0, 0.0));
    run(&mut app, 60);
    assert_eq!(report(&app).submerged, 0);
    assert_eq!(report(&app).out_of_bounds, 0);
    assert_eq!(report(&app).recovered, 0);
    assert!(
        position(&app, car).y < -5.0,
        "the car really sits below the level, at {}",
        position(&app, car).y
    );
}

/// A car under a listed room's bound with no ground contact at all —
/// clipped through the water plane — drowns on the dwell rather than
/// falling forever or tripping the out-of-bounds leg.
#[test]
fn an_airborne_car_under_a_deadly_rooms_bound_drowns() {
    let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    // A deadly room over empty air — no collider anywhere under it.
    app.world_mut()
        .insert_resource(deadly_water([100.0, 120.0], [-15.0, 15.0], 0.0, 0.5));
    teleport(&mut app, car, Vec3::new(110.0, -1.0, 0.0));
    run(&mut app, 45); // ~0.75 s — past the 0.5 s dwell
    assert_eq!(report(&app).submerged, 1);
    assert_eq!(report(&app).out_of_bounds, 0);
    assert_eq!(report(&app).recovered, 1);
}

#[test]
fn a_trailer_rig_recovers_with_the_tractor() {
    // F05-AC05's articulated leg: the authored offsets seat the
    // trailer behind the recovered tractor — the `resolve_stuck`
    // pattern on the anchor pose.
    let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    let trailer_pos = Vec3::new(0.0, 1.0, 5.0);
    let offset = Vec3::new(0.0, -0.2, 4.0);
    // The real rig declares the tow through `Trailer` — the stream
    // follower reads it, not `SpawnPoint.trailers` bookkeeping (which
    // production keeps in parallel for the camera occluder).
    let trailer = app
        .world_mut()
        .spawn((
            mm2_app::car_visual::Trailer {
                towing: car,
                rest_offset: offset,
            },
            vehicle_bundle(&VehicleConfig::default()),
            Position(trailer_pos),
            Transform::from_translation(trailer_pos),
        ))
        .id();
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

/// Hold the car still in `pose` at `pos` — a kinematic body keeps the
/// wheel raycasts (and so the dry contacts the anchor rule reads) while
/// nothing moves it.
fn pin(app: &mut App, car: Entity, pos: Vec3, pose: Quat) {
    let world = app.world_mut();
    world.entity_mut(car).insert(RigidBody::Kinematic);
    world.get_mut::<Position>(car).unwrap().0 = pos;
    world.get_mut::<Rotation>(car).unwrap().0 = pose;
    let mut xf = world.get_mut::<Transform>(car).unwrap();
    xf.translation = pos;
    xf.rotation = pose;
    world.get_mut::<LinearVelocity>(car).unwrap().0 = Vec3::ZERO;
    world.get_mut::<AngularVelocity>(car).unwrap().0 = Vec3::ZERO;
}

/// A plane of static triangles rising along +X at `deg`, through
/// `through`, 20 m each way — plain two-sided like the city's banks.
fn bank(app: &mut App, through: Vec3, deg: f32) {
    let t = deg.to_radians().tan();
    let at = |dx: f32, dz: f32| through + Vec3::new(dx, dx * t, dz);
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::trimesh(
            vec![
                at(-20.0, -20.0),
                at(20.0, -20.0),
                at(20.0, 20.0),
                at(-20.0, 20.0),
            ],
            vec![[0, 2, 1], [0, 3, 2]],
        ),
    ));
}

fn anchor_of(app: &App, car: Entity) -> Vec3 {
    app.world()
        .get::<VehicleRecovery>(car)
        .unwrap()
        .anchor()
        .expect("anchored")
        .0
}

fn any_wheel_grounded(app: &App, car: Entity) -> bool {
    app.world()
        .get::<mm2_vehicle::VehicleState>(car)
        .unwrap()
        .wheels
        .iter()
        .any(|w| w.grounded)
}

/// The anchor that looped the Tower Bridge recovery: a car righted level
/// into a steep bank, its downhill wheels still touching the grass while
/// the rest of it — centre of mass included — lay inside the bank. A dry
/// wheel there must not anchor the recovery, or every landing puts the
/// car back inside the bank to fall again.
#[test]
fn a_car_sunk_into_a_bank_never_anchors_there() {
    let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    let floor = anchor_of(&app, car);
    let through = Vec3::new(200.0, 10.0, 0.0);
    bank(&mut app, through, 37.0);
    // Level, nose uphill, origin 0.4 m under the bank's surface: the
    // rear wheels reach the slope behind it, the centre of mass is
    // buried.
    pin(
        &mut app,
        car,
        through - Vec3::Y * 0.4,
        Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2),
    );
    run(&mut app, 10);
    assert!(
        any_wheel_grounded(&app, car),
        "the rear wheels touch the bank"
    );
    assert!(
        anchor_of(&app, car).distance(floor) < 0.1,
        "a buried car must keep its anchor on the floor, got {}",
        anchor_of(&app, car)
    );
}

/// Riding something that moves is not standing anywhere: a ferry deck or
/// a drawbridge leaf can carry the car off, and an anchor left on it
/// would land the car over the open water.
#[test]
fn a_car_on_a_moving_deck_keeps_its_anchor_on_static_ground() {
    let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    let floor = anchor_of(&app, car);
    app.world_mut().spawn((
        RigidBody::Kinematic,
        Collider::cuboid(20.0, 1.0, 20.0),
        Position(Vec3::new(200.0, 9.5, 0.0)),
        Transform::from_xyz(200.0, 9.5, 0.0),
    ));
    teleport(&mut app, car, Vec3::new(200.0, 11.0, 0.0));
    run(&mut app, 60);
    assert!(any_wheel_grounded(&app, car), "the car stands on the deck");
    assert!(
        anchor_of(&app, car).distance(floor) < 0.1,
        "the deck must not anchor the car, got {}",
        anchor_of(&app, car)
    );
}

/// Tipped steeply up a bank the car could not be set down level where it
/// stands — the anchor stays on the last ground it stood level-ish on.
#[test]
fn a_car_tipped_up_a_bank_keeps_its_anchor() {
    let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    let floor = anchor_of(&app, car);
    let through = Vec3::new(200.0, 10.0, 0.0);
    bank(&mut app, through, 40.0);
    // Resting on its wheels along the slope, nose uphill.
    let tilt = Quat::from_rotation_z(40f32.to_radians());
    let ground_y = mm2_vehicle::HandlingMetrics::of(&VehicleConfig::default()).ground_y;
    pin(
        &mut app,
        car,
        through + tilt * Vec3::new(0.0, -ground_y, 0.0),
        tilt * Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2),
    );
    run(&mut app, 10);
    assert!(any_wheel_grounded(&app, car), "the wheels touch the bank");
    assert!(
        anchor_of(&app, car).distance(floor) < 0.1,
        "a car tipped 40° must keep its anchor, got {}",
        anchor_of(&app, car)
    );
}

/// Fire one out-of-bounds recovery for `object` landing at `landing`.
fn recover_to(app: &mut App, object: ObjectId, landing: (Vec3, f32)) {
    let generation = app.world().resource::<Session>().generation();
    app.world_mut()
        .resource_mut::<Messages<RecoveryEvent>>()
        .write(RecoveryEvent {
            object,
            generation,
            tick: 0,
            cause: RecoveryCause::OutOfBounds,
            landing: Some(landing),
        });
    app.update();
}

/// The deepest any hull corner of the car, as it stands now, lies under
/// the plane `y = through.y + (x − through.x) · tan(deg)`.
fn depth_under_bank(app: &App, car: Entity, through: Vec3, deg: f32) -> f32 {
    let pos = position(app, car);
    let rot = app.world().get::<Rotation>(car).unwrap().0;
    mm2_vehicle::hull_points(&VehicleConfig::default())
        .into_iter()
        .map(|p| pos + rot * Vec3::from(p))
        .map(|w| through.y + (w.x - through.x) * deg.to_radians().tan() - w.y)
        .fold(f32::MIN, f32::max)
}

/// Every reset sets a car down level, but an anchor can lie on a slope
/// the car stood on tilted: level at the anchor its uphill end would be
/// inside the slope, on city ground solid only from above. The landing
/// is seated on the highest ground under the car's level footprint.
#[test]
fn a_recovery_anchored_on_a_slope_lands_on_top_of_it() {
    let (mut app, car, object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    let through = Vec3::new(200.0, 10.0, 0.0);
    bank(&mut app, through, 25.0);
    // Where a car resting nose-up on the 25° slope had its origin.
    let tilt = Quat::from_rotation_z(25f32.to_radians());
    let ground_y = mm2_vehicle::HandlingMetrics::of(&VehicleConfig::default()).ground_y;
    let anchor = through + tilt * Vec3::new(0.0, -ground_y, 0.0);
    recover_to(&mut app, object, (anchor, -std::f32::consts::FRAC_PI_2));
    assert_eq!(report(&app).recovered, 1);
    let depth = depth_under_bank(&app, car, through, 25.0);
    assert!(
        depth <= 0.0,
        "the landing put a hull corner {depth} m inside the slope"
    );
    assert!(
        depth > -1.5,
        "seated on the slope, not high above it ({depth})"
    );
}

/// A landing with nothing under it would only fall again and recover
/// onto the same spot for ever — the session spawn takes the car instead.
#[test]
fn a_landing_with_no_ground_under_it_falls_back_to_the_spawn() {
    let (mut app, car, object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    recover_to(&mut app, object, (Vec3::new(500.0, 5.0, 500.0), 0.0));
    assert_eq!(report(&app).recovered, 1);
    let pos = position(&app, car);
    assert!(
        pos.distance(Vec3::new(SPAWN.x, pos.y, SPAWN.z)) < 0.1,
        "a groundless landing recovers to the spawn, got {pos}"
    );
    // Settled there, the car anchors on the spawn's ground.
    run(&mut app, 60);
    let anchor = anchor_of(&app, car);
    assert!(
        anchor.distance(Vec3::new(SPAWN.x, anchor.y, SPAWN.z)) < 0.5,
        "the detector re-anchors at the spawn, got {anchor}"
    );
}

/// A fielded cop's identity as the loader stamps it — no `Player`: a
/// cop is not a race participant.
fn cop_marker() -> mm2_app::police::PoliceCar {
    mm2_app::police::PoliceCar {
        index: 0,
        spec: mm2_game::PoliceSpec {
            vehicle: "vpcop".into(),
            position: Vec3::ZERO,
            heading_deg: Some(0.0),
            params: vec![0.0],
            line: 1,
        },
    }
}

/// A cop that falls out of the world recovers to its own anchor — and
/// never to the session spawn, which is the player's start: a groundless
/// landing would otherwise teleport it onto its target (F20-C).
#[test]
fn a_cop_is_recovered_to_its_own_landing_never_the_players_spawn() {
    let (mut app, _car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    let cop_object = app.world_mut().resource_mut::<Session>().mint_object_id();
    let role = app.world().resource::<Session>().authority_role();
    let cop_pos = Vec3::new(40.0, 1.2, 40.0);
    let cop = app
        .world_mut()
        .spawn((
            ObjectIdentity(cop_object),
            cop_marker(),
            role,
            mm2_game::DamageSignals::default(),
            mm2_game::VehicleRecovery::with_anchor(POLICY, cop_pos, 0.0),
            vehicle_bundle(&VehicleConfig::default()),
            Position(cop_pos),
            Transform::from_translation(cop_pos),
        ))
        .id();
    app.update();
    drain_recovery(&mut app);
    let before = report(&app).recovered;

    // Over the ground it lands where its landing says...
    recover_to(&mut app, cop_object, (Vec3::new(-40.0, 1.2, 40.0), 0.0));
    assert_eq!(report(&app).recovered, before + 1);
    let pos = position(&app, cop);
    assert!(
        pos.distance(Vec3::new(-40.0, pos.y, 40.0)) < 0.5,
        "a cop recovers to its landing, got {pos}"
    );
    // ...and with nothing under it, it is not sent to the spawn.
    recover_to(&mut app, cop_object, (Vec3::new(500.0, 5.0, 500.0), 0.0));
    let pos = position(&app, cop);
    assert!(
        pos.distance(Vec3::new(SPAWN.x, pos.y, SPAWN.z)) > 100.0,
        "a cop must never be set down at the player's spawn, got {pos}"
    );
}

/// Only ground that anchored the car seats its landing: another car
/// parked alongside must not lift it onto its roof.
#[test]
fn a_landing_beside_another_car_is_not_lifted_onto_it() {
    let (mut app, car, object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    let floor = position(&app, car);
    let beside = floor + Vec3::new(1.5, 0.0, 0.0);
    app.world_mut().spawn((
        vehicle_bundle(&VehicleConfig::default()),
        Position(beside),
        Transform::from_translation(beside),
    ));
    run(&mut app, 30);
    recover_to(&mut app, object, (floor, 0.0));
    let pos = position(&app, car);
    assert!(
        pos.y < floor.y + 0.3,
        "landed on the neighbour's roof at {pos} (floor pose {floor})"
    );
}

/// Fixed steps outrun frames (two per frame here), so a recovery fired
/// on a frame's first step is applied only after that frame's second:
/// the detector must not look at the pose the reset is about to replace.
#[test]
fn a_recovery_is_not_observed_again_before_its_reset_lands() {
    let (mut app, car, object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
    let landing = Vec3::new(30.0, 0.0, 10.0);
    recover_to(&mut app, object, (landing, 0.5));

    assert_eq!(report(&app).recovered, 1);
    assert!(
        app.world().get::<mm2_vehicle::Teleported>(car).is_some(),
        "the reset landed within the frame"
    );
    assert!(
        app.world().get::<mm2_vehicle::ResetPending>(car).is_none(),
        "landing the reset clears the pending marker"
    );
    let anchor = anchor_of(&app, car);
    assert!(
        Vec2::new(anchor.x - landing.x, anchor.z - landing.z).length() < 0.5,
        "the anchor is the landing, not the pre-reset pose the second step saw: {anchor}"
    );
}

#[test]
fn an_out_of_bounds_fall_recovers_once_whatever_the_step_parity() {
    // Heights shift the step the fall trips on across both halves of a
    // frame; the second step must never re-fire on the old pose.
    for drop in [0.0, 0.01, 0.02, 0.03] {
        let (mut app, car, _object) = recovery_app(Vec3::new(0.0, 1.2, 0.0));
        teleport(&mut app, car, Vec3::new(500.0, 5.0 + drop, 0.0));
        run(&mut app, 150);
        assert_eq!(report(&app).out_of_bounds, 1, "drop {drop}");
        assert_eq!(report(&app).recovered, 1, "drop {drop}");
    }
}

// ---- AC04 (F28): a rescue cannot manufacture event completion -------

/// `recovery_app` for an event session with one gate — the race's only
/// checkpoint, so crossing it finishes — standing between the dry
/// anchor (x = 0) and wherever the car is washed or dropped. The
/// production race driver runs beside the recovery pair; the car is
/// released and anchored before the function returns.
fn gated_recovery_app(gate_x: f32, gate_height: f32) -> (App, Entity) {
    use mm2_app::race::{advance_race, reanchor_teleported_participants};
    use mm2_game::{
        Checkpoint, CheckpointRule, EventParams, EventRef, EventTableKind, RaceDefinition,
        RaceProgress, RaceStarted, RaceState, ResultLedger, SessionMode,
    };

    let config = SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "london".into(),
            table: EventTableKind::Checkpoint,
            index: 0,
        }),
        ..SessionConfig::default()
    };
    let (mut app, car, _object) = recovery_app_with(config, Vec3::new(0.0, 1.2, 0.0));
    let def = RaceDefinition {
        checkpoints: vec![Checkpoint {
            center: Vec3::new(gate_x, 0.0, 0.0),
            radius: 15.0,
            height: gate_height,
            heading_deg: -90.0,
            require_direction: false,
        }],
        finish: None,
        rule: CheckpointRule::AnyOrder,
        laps: 1,
        time_limit_ticks: None,
        params: EventParams::default(),
        countdown_ticks: 0,
        start_slots: Vec::new(),
    };
    let generation = app.world().resource::<Session>().generation();
    app.world_mut()
        .insert_resource(RaceState::new(def.clone(), generation));
    app.world_mut().init_resource::<ResultLedger>();
    app.add_message::<RaceStarted>();
    app.add_systems(
        FixedLast,
        (reanchor_teleported_participants, advance_race).chain(),
    );
    app.world_mut()
        .entity_mut(car)
        .insert(RaceProgress::new(&def));
    run(&mut app, 4); // countdown 0 → Running, the car Racing and anchored
    (app, car)
}

/// The gate's standing after a rescue: how many gates the car holds,
/// whether it still races and how many results were minted.
fn standing(app: &App, car: Entity) -> (usize, bool, usize) {
    use mm2_game::{ParticipantState, RaceProgress, ResultLedger};
    let progress = app.world().get::<RaceProgress>(car).unwrap();
    (
        progress.cleared_count(),
        matches!(progress.state, ParticipantState::Racing),
        app.world().resource::<ResultLedger>().len(),
    )
}

/// Carry the car to `to` without sweeping the ground between — a
/// disclosed teleport (a ferry deck, a cutscene), the one way to stand
/// on the far side of a gate without driving through it.
fn carry_unswept(app: &mut App, car: Entity, to: Vec3) {
    app.world_mut()
        .get_mut::<mm2_game::RaceProgress>(car)
        .unwrap()
        .break_segment();
    teleport(app, car, to);
}

#[test]
fn drowning_across_a_gate_does_not_clear_it_on_the_way_home() {
    // The gate spans x 15..45 between the anchor and the Thames slab; a
    // car drowned at x = 60 is rescued back across it.
    let (mut app, car) = gated_recovery_app(30.0, 8.0);
    assert_eq!(
        standing(&app, car),
        (0, true, 0),
        "released, nothing cleared"
    );

    carry_unswept(&mut app, car, Vec3::new(WATER_AT.x, 1.0, WATER_AT.z));
    run(&mut app, 80);
    assert_eq!(report(&app).submerged, 1);
    assert_eq!(report(&app).recovered, 1, "the rescue ran");
    assert!(
        position(&app, car).x.abs() < 2.0,
        "and landed at the anchor"
    );
    run(&mut app, 30);
    assert_eq!(
        standing(&app, car),
        (0, true, 0),
        "the rescue's hop back over the gate neither clears it nor finishes the event"
    );

    // Control: the gate is live — driving through it still finishes.
    teleport(&mut app, car, Vec3::new(30.0, 1.2, 0.0));
    run(&mut app, 4);
    assert_eq!(
        standing(&app, car).2,
        1,
        "a real crossing records the result"
    );
}

#[test]
fn falling_out_of_the_world_past_a_gate_does_not_finish_the_event() {
    // The gate stands tall between the anchor and the edge the car
    // falls from, so the rescue's path crosses it at any height.
    let (mut app, car) = gated_recovery_app(300.0, 400.0);
    carry_unswept(&mut app, car, Vec3::new(500.0, 5.0, 0.0));
    run(&mut app, 150);
    assert_eq!(report(&app).out_of_bounds, 1);
    assert_eq!(report(&app).recovered, 1, "the rescue ran");
    assert!(
        position(&app, car).x.abs() < 2.0,
        "and landed at the anchor"
    );
    run(&mut app, 30);
    assert_eq!(
        standing(&app, car),
        (0, true, 0),
        "the fall's rescue neither clears the gate nor finishes the event"
    );

    teleport(&mut app, car, Vec3::new(300.0, 1.2, 0.0));
    run(&mut app, 4);
    assert_eq!(standing(&app, car).2, 1, "driving it still finishes");
}

#[test]
fn a_rescue_keeps_the_gates_already_earned() {
    // Clear gate 0 by driving, then drown beyond it: the rescue neither
    // un-clears it nor finishes anything that was not driven.
    let (mut app, car) = gated_recovery_app(30.0, 8.0);
    // A second gate on the far side keeps the event open after gate 0.
    {
        use mm2_game::{Checkpoint, RaceProgress, RaceState};
        let far = Checkpoint {
            center: Vec3::new(-30.0, 0.0, 0.0),
            radius: 15.0,
            height: 8.0,
            heading_deg: -90.0,
            require_direction: false,
        };
        let mut race = app.world_mut().resource_mut::<RaceState>();
        race.definition.checkpoints.push(far);
        let def = race.definition.clone();
        let mut fresh = RaceProgress::new(&def);
        fresh.state = mm2_game::ParticipantState::Racing; // released already
        *app.world_mut().get_mut::<RaceProgress>(car).unwrap() = fresh;
    }
    run(&mut app, 2);
    teleport(&mut app, car, Vec3::new(30.0, 1.2, 0.0)); // swept: gate 0 clears
    run(&mut app, 4);
    assert_eq!(standing(&app, car), (1, true, 0), "one of two gates driven");

    carry_unswept(&mut app, car, Vec3::new(WATER_AT.x, 1.0, WATER_AT.z));
    run(&mut app, 100);
    assert_eq!(report(&app).recovered, 1);
    assert_eq!(
        standing(&app, car),
        (1, true, 0),
        "the earned gate stays, the unvisited one is not credited"
    );
}

/// Every retail city's authored race points (start slots, checkpoint
/// gates and the anchors of every wired opponent `.opp` route, both
/// difficulties of every ready event) against the city's
/// real `CityWater` built through the production path (F28-AC04).
/// Crash Course lessons go through the production `lesson_race_setup`:
/// every leg's start slots and gates, the lead-car route anchors and
/// the police posts of both difficulties. Deadly water is scoped to
/// the listed rooms, so a start slot or gate inside one would drown a
/// car on the authored route; the audit reports its denominators and
/// fails when a city has no usable water or a point sits in it. (Six
/// London route anchors lie over a water footprint on bridges 3.6 m
/// above the surface and are rightly dry.) A positive control samples
/// a grid so the check cannot pass over empty water. Skipped without
/// the operator's install (`MM2_RETAIL=<dir>`).
#[test]
fn retail_no_authored_race_point_stands_in_deadly_water() {
    use mm2_content::{EventCatalog, race_definition};
    use mm2_formats::{psdl::Psdl, water::WaterDef};
    use mm2_game::Difficulty;

    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut vfs = mm2_assets::Vfs::new();
    mm2_assets::mount_install(&mut vfs, &retail, &mm2_assets::InstallMount::default()).unwrap();
    let surfaces = mm2_content::load_surface_tables(&vfs).unwrap();

    for city in ["sf", "london"] {
        let (psdl_bytes, _) = vfs.read_path(&format!("city/{city}.psdl")).unwrap();
        let psdl = Psdl::parse(&psdl_bytes).unwrap();
        let (water_bytes, _) = vfs.read_path(&format!("city/{city}.water")).unwrap();
        let def = WaterDef::parse(std::str::from_utf8(&water_bytes).unwrap()).unwrap();
        let water = mm2_app::water::CityWater::build(&def, &psdl, surfaces.as_ref());
        assert!(water.room_count() > 0, "{city}: no deadly rooms resolved");
        assert_eq!(water.skipped(), 0, "{city}: a water ref did not resolve");

        // Positive control: the water really covers ground somewhere,
        // sampled a metre under the level on a 10 m grid.
        let (lo, hi) = (psdl.bounds_min, psdl.bounds_max);
        let mut wet = 0usize;
        let mut z = lo[2];
        while z <= hi[2] {
            let mut x = lo[0];
            while x <= hi[0] {
                wet += usize::from(water.is_deadly(Vec3::new(x, water.level() - 1.0, z)));
                x += 10.0;
            }
            z += 10.0;
        }
        assert!(wet > 0, "{city}: the control grid found no deadly water");

        let catalog = EventCatalog::scan(&vfs, city);
        assert!(!catalog.events.is_empty(), "{city}: no events");
        let (mut events, mut points, mut wading) = (0usize, 0usize, Vec::new());
        let (mut routes, mut route_points) = (0usize, 0usize);
        for event in catalog.events.iter().filter(|e| e.status.is_ready()) {
            for difficulty in [Difficulty::Amateur, Difficulty::Professional] {
                let Ok(race) = race_definition(event, difficulty) else {
                    continue; // Crash Courses are lessons, not race definitions
                };
                events += 1;
                let gates = race.checkpoints.iter().chain(race.finish.iter());
                let spots = race
                    .start_slots
                    .iter()
                    .map(|s| ("start", s.position))
                    .chain(gates.map(|g| ("gate", g.center)));
                for (what, p) in spots {
                    points += 1;
                    if water.is_deadly(p) {
                        wading.push(format!("{:?} {what} at {p}", event.event_ref));
                    }
                }
                // The wired opponent driving lines: a route anchor in
                // deadly water would send the field into the harbour.
                let roster = mm2_content::opponent_roster(&vfs, event, difficulty).unwrap();
                for (i, entry) in roster.entries.iter().enumerate() {
                    let Some(route) = &entry.route else { continue };
                    routes += 1;
                    for (n, row) in route.points.iter().enumerate() {
                        route_points += 1;
                        if water.is_deadly(row.position) {
                            wading.push(format!(
                                "{:?} {difficulty:?} opponent {i} anchor {n} at {}",
                                event.event_ref, row.position
                            ));
                        }
                    }
                }
            }
        }

        // Crash Course lessons: the same points through the lesson
        // setup the menu launches (the race loop above skips them).
        let (mut legs, mut lesson_points, mut lead_anchors, mut posts) = (0usize, 0, 0, 0);
        let mut lessons = 0usize;
        for event in catalog
            .events
            .iter()
            .filter(|e| e.event_ref.table == mm2_game::EventTableKind::CrashCourse)
        {
            for difficulty in [Difficulty::Amateur, Difficulty::Professional] {
                let setup = mm2_app::race::lesson_race_setup(&vfs, &event.event_ref, difficulty);
                let setup = match setup {
                    Ok(setup) => setup,
                    Err(e) => {
                        // An unbuildable lesson is a finding, not a skip.
                        wading.push(format!(
                            "{:?} {difficulty:?} lesson did not build: {e}",
                            event.event_ref
                        ));
                        continue;
                    }
                };
                lessons += 1;
                for leg in &setup.legs {
                    legs += 1;
                    let def = &leg.definition;
                    let gates = def.checkpoints.iter().chain(def.finish.iter());
                    let spots = def
                        .start_slots
                        .iter()
                        .map(|s| ("start", s.position))
                        .chain(gates.map(|g| ("gate", g.center)));
                    for (what, p) in spots {
                        lesson_points += 1;
                        if water.is_deadly(p) {
                            wading.push(format!(
                                "{:?} {difficulty:?} {} {what} at {p}",
                                event.event_ref, leg.source
                            ));
                        }
                    }
                }
                for (i, entry) in setup.lead_cars.entries.iter().enumerate() {
                    let Some(route) = &entry.route else { continue };
                    for (n, row) in route.points.iter().enumerate() {
                        lead_anchors += 1;
                        if water.is_deadly(row.position) {
                            wading.push(format!(
                                "{:?} {difficulty:?} lead car {i} anchor {n} at {}",
                                event.event_ref, row.position
                            ));
                        }
                    }
                }
                for (i, cop) in setup.police.entries.iter().enumerate() {
                    posts += 1;
                    if water.is_deadly(cop.position) {
                        wading.push(format!(
                            "{:?} {difficulty:?} police post {i} at {}",
                            event.event_ref, cop.position
                        ));
                    }
                }
            }
        }
        eprintln!(
            "{city}: {} water rooms, {wet} wet grid cells, {events} race definitions, \
             {points} start/gate points, {routes} opponent routes / {route_points} anchors, \
             {lessons} lesson setups / {legs} legs / {lesson_points} points / \
             {lead_anchors} lead-car anchors / {posts} police posts, \
             {} in deadly water",
            water.room_count(),
            wading.len()
        );
        assert!(events > 0 && points > 0, "{city}: nothing audited");
        assert!(
            routes > 0 && route_points > 0,
            "{city}: no opponent route audited"
        );
        assert!(
            lessons > 0 && legs > 0 && lesson_points > 0,
            "{city}: no Crash Course lesson audited"
        );
        assert!(wading.is_empty(), "{city}: {wading:?}");
    }
}
