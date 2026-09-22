//! F05-B.2 integration: authored `vehstuck` detectors arm off the real
//! impact stream, fire on the authored window, and resolve through the
//! production `ResetVehicle` path — an in-place upright recovery for the
//! local driver, AI opponents and trailer rigs; remote participants and
//! `Disabled` wrecks are someone else's business.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::contracts::{self, ImpactFilter};
use mm2_app::damage::{self, DamageReport};
use mm2_app::session::{SessionControl, SpawnPoint};
use mm2_app::stuck::{self, StuckReport};
use mm2_game::{
    DamageEvent, DamageSpec, ImpactEvent, ImpactId, ObjectId, ObjectIdentity, Player,
    PlayerControl, Session, SessionConfig, SessionPhase, StuckEvent, StuckSpec, SurfaceState,
    VehicleDamage, VehicleStuck, advance_session_tick,
};
use mm2_vehicle::{TireConditions, VehicleConfig, VehiclePlugin, vehicle_bundle};

/// A retail-shaped spec: stock pos/move bounds are uniform; the window
/// is shortened so the tests run in ~30 frames.
const SPEC: StuckSpec = StuckSpec {
    time_thresh: 0.5,
    pos_thresh: 1.25,
    move_thresh: 1.75,
    turn: std::f32::consts::PI,
    rotation: 0.0,
    translation: 0.1,
};
const DAMAGE_SPEC: DamageSpec = DamageSpec {
    impact_threshold: 1500.0,
    med_damage: 150_000.0,
    max_damage: 321_300.0,
    regenerate_rate: 0.0,
};
const SPAWN: Vec3 = Vec3::new(0.0, 1.5, 0.0);
const SPAWN_YAW: f32 = 0.25;

/// A playing session on a marked ground plane plus the player car with
/// an authored stuck spec. Returns the app, the car entity and its
/// minted object id.
fn stuck_app(car_pos: Vec3, spec: StuckSpec) -> (App, Entity, ObjectId) {
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
        .insert_resource(ImpactFilter::default())
        .init_resource::<DamageReport>()
        .init_resource::<StuckReport>()
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
            )
                .chain(),
        );
    app.finish();
    app.cleanup();

    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(400.0, 1.0, 400.0),
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Transform::from_xyz(0.0, -0.5, 0.0),
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
            VehicleStuck::new(spec),
            vehicle_bundle(&VehicleConfig::default()),
            Position(car_pos),
            Transform::from_translation(car_pos),
        ))
        .id();

    // Settle the car, then wipe the baseline: the settle drop is a real
    // impact and legitimately arms the detector — the tests measure
    // only the episodes they start.
    app.update();
    drain_stuck(&mut app);
    app.world_mut()
        .entity_mut(car)
        .insert(VehicleStuck::new(spec));
    app.world_mut().resource_mut::<StuckReport>().reset();
    (app, car, object)
}

/// Deliver one synthetic impact into the session's stream, stamped
/// with the live generation/tick like `collect_impacts` writes.
fn write_impact(app: &mut App, id: u64, a: ObjectId, b: ObjectId, severity: f32) {
    let (generation, tick) = {
        let session = app.world().resource::<Session>();
        (session.generation(), session.tick())
    };
    app.world_mut()
        .resource_mut::<Messages<ImpactEvent>>()
        .write(ImpactEvent {
            id: ImpactId(id),
            generation,
            tick,
            participants: (a, b),
            point: Vec3::ZERO,
            normal: Vec3::Y,
            severity,
            surface: SurfaceState::default(),
        });
}

fn drain_stuck(app: &mut App) -> Vec<StuckEvent> {
    app.world_mut()
        .resource_mut::<Messages<StuckEvent>>()
        .drain()
        .collect()
}

fn report(app: &App) -> &StuckReport {
    app.world().resource::<StuckReport>()
}

/// Run `frames` 60 Hz updates — about `frames / 60` seconds of session
/// time.
fn run(app: &mut App, frames: usize) {
    for _ in 0..frames {
        app.update();
    }
}

#[test]
fn a_parked_car_after_an_impact_recovers_in_place_once() {
    let (mut app, car, object) = stuck_app(Vec3::new(12.0, 1.2, -8.0), SPEC);
    write_impact(&mut app, 1, object, ObjectId::WORLD, 30.0);
    run(&mut app, 40); // ~0.67 s — past the 0.5 s authored window.

    assert_eq!(report(&app).armed, 1, "the impact anchored the detector");
    assert_eq!(report(&app).detections, 1);
    assert_eq!(report(&app).recovered, 1);
    // In place: the reset keeps the car where it was stuck, upright on
    // its heading, with its motion cleared.
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!(
        pos.distance(Vec3::new(12.0, pos.y, -8.0)) < 0.5,
        "in-place reset keeps the stuck spot, got {pos}"
    );
    assert!(
        app.world().get::<mm2_vehicle::Teleported>(car).is_some(),
        "the recovery hop must break swept segments like every reset"
    );

    // One-shot: still parked where it was, but disarmed — no second
    // detection until another impact lands. The reset cleared its
    // motion — a settled car stays at rest.
    run(&mut app, 60);
    assert_eq!(report(&app).detections, 1);
    assert_eq!(report(&app).recovered, 1);
    assert!(
        app.world().get::<LinearVelocity>(car).unwrap().0.length() < 0.25,
        "the recovered car settles to rest"
    );

    // A new impact re-arms the episode — the window counts again.
    write_impact(&mut app, 2, object, ObjectId::WORLD, 30.0);
    run(&mut app, 40);
    assert_eq!(report(&app).armed, 2);
    assert_eq!(report(&app).detections, 2);
    assert_eq!(report(&app).recovered, 2);
}

#[test]
fn a_car_that_drives_away_escapes_the_window() {
    let (mut app, car, object) = stuck_app(Vec3::new(0.0, 1.2, 0.0), SPEC);
    write_impact(&mut app, 1, object, ObjectId::WORLD, 30.0);
    app.update();
    assert_eq!(report(&app).armed, 1);
    // It got away — past move_thresh the episode is over.
    let away = Vec3::new(4.0, 1.2, 0.0);
    {
        let world = app.world_mut();
        world.get_mut::<Position>(car).unwrap().0 = away;
        world.get_mut::<Transform>(car).unwrap().translation = away;
    }
    run(&mut app, 60);
    assert_eq!(report(&app).detections, 0);
    assert_eq!(report(&app).recovered, 0);
}

#[test]
fn an_unimpacted_car_never_detects() {
    // Parked forever is not stuck — only a post-impact failure to get
    // away is (the recovered struct's `m_LastImpactPos` anchor).
    let (mut app, _car, _object) = stuck_app(Vec3::new(0.0, 1.2, 0.0), SPEC);
    run(&mut app, 90);
    assert_eq!(report(&app).armed, 0);
    assert_eq!(report(&app).detections, 0);
    assert_eq!(report(&app).recovered, 0);
}

#[test]
fn a_roofed_car_is_righted_in_place() {
    // The rollover leg: the car is armed by the impact that put it on
    // its roof, holds inside pos_thresh for time_thresh and is set
    // back on its wheels — the same landing `vehicle_self_right`
    // computes (which would fire at its own, unauthored-default delay).
    let inverted = Quat::from_rotation_x(std::f32::consts::PI);
    let (mut app, car, object) = stuck_app(Vec3::new(3.0, 0.8, 2.0), SPEC);
    {
        let world = app.world_mut();
        world.get_mut::<Rotation>(car).unwrap().0 = inverted;
        world.get_mut::<Transform>(car).unwrap().rotation = inverted;
        // Fresh detector anchored on nothing yet — the impact below arms it.
        world.entity_mut(car).insert(VehicleStuck::new(SPEC));
    }
    write_impact(&mut app, 1, object, ObjectId::WORLD, 30.0);
    // The roof landing can yaw the car past `turn` once — the tumbling
    // leg re-anchors on the pose it settles into and the window counts
    // from there (the authored semantic, not a bug to hide).
    run(&mut app, 75);

    assert_eq!(report(&app).detections, 1);
    assert_eq!(report(&app).recovered, 1);
    let up = (app.world().get::<Rotation>(car).unwrap().0 * Vec3::Y).y;
    assert!(up > 0.9, "the car must be back on its wheels, up={up}");
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!(
        pos.distance(Vec3::new(3.0, pos.y, 2.0)) < 0.5,
        "still where it rolled, got {pos}"
    );
}

#[test]
fn a_car_wedged_past_its_turn_bound_keeps_counting() {
    // The `turn` leg: rotating more than the authored bound since the
    // anchor counts as still tumbling — the window restarts on the
    // pose it lands in rather than firing or forgetting.
    let mut app_spec = SPEC;
    app_spec.turn = 0.6; // ~34°
    let (mut app, car, object) = stuck_app(Vec3::new(0.0, 1.2, 0.0), app_spec);
    write_impact(&mut app, 1, object, ObjectId::WORLD, 30.0);
    app.update();
    // Rolled 90° onto its side: past the bound, so the settle window
    // restarts — then it counts to time_thresh and fires.
    let side = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
    {
        let world = app.world_mut();
        world.get_mut::<Rotation>(car).unwrap().0 = side;
        world.get_mut::<Transform>(car).unwrap().rotation = side;
    }
    // Settling onto the flank can re-anchor the tumbling leg a few
    // times before the pose holds — give the authored window room.
    run(&mut app, 75);
    assert_eq!(report(&app).detections, 1);
    let up = (app.world().get::<Rotation>(car).unwrap().0 * Vec3::Y).y;
    assert!(up > 0.9, "righted after the settle window, up={up}");
}

#[test]
fn an_opponent_stuck_recovers_without_touching_the_session() {
    let (mut app, _car, _object) = stuck_app(Vec3::new(0.0, 1.2, 0.0), SPEC);
    let ai_object = app.world_mut().resource_mut::<Session>().mint_object_id();
    let ai_player = app.world_mut().resource_mut::<Session>().mint_player_id();
    let role = app.world().resource::<Session>().authority_role();
    let ai_pos = Vec3::new(10.0, 1.2, 5.0);
    let ai = app
        .world_mut()
        .spawn((
            ObjectIdentity(ai_object),
            Player {
                id: ai_player,
                control: PlayerControl::Ai,
            },
            role,
            mm2_game::DamageSignals::default(),
            VehicleStuck::new(SPEC),
            vehicle_bundle(&VehicleConfig::default()),
            Position(ai_pos),
            Transform::from_translation(ai_pos),
        ))
        .id();
    app.update();
    drain_stuck(&mut app);
    app.world_mut().resource_mut::<StuckReport>().reset();

    write_impact(&mut app, 7, ai_object, ObjectId::WORLD, 30.0);
    run(&mut app, 40);

    // The AI recovers in place like the local driver (designed policy,
    // UNK-13) — and never touches the session's restart intent.
    assert_eq!(report(&app).recovered, 1);
    let pos = app.world().get::<Position>(ai).unwrap().0;
    assert!(
        pos.distance(Vec3::new(ai_pos.x, pos.y, ai_pos.z)) < 0.5,
        "the AI resets where it was stuck, got {pos}"
    );
    assert!(!app.world().resource::<SessionControl>().restart);
}

#[test]
fn a_remote_participant_is_never_armed_or_recovered() {
    let (mut app, _car, _object) = stuck_app(Vec3::new(0.0, 1.2, 0.0), SPEC);
    let remote_object = app.world_mut().resource_mut::<Session>().mint_object_id();
    let remote_player = app.world_mut().resource_mut::<Session>().mint_player_id();
    let role = app.world().resource::<Session>().authority_role();
    let remote_pos = Vec3::new(10.0, 1.2, 5.0);
    app.world_mut().spawn((
        ObjectIdentity(remote_object),
        Player {
            id: remote_player,
            control: PlayerControl::Remote,
        },
        role,
        mm2_game::DamageSignals::default(),
        VehicleStuck::new(SPEC),
        vehicle_bundle(&VehicleConfig::default()),
        Position(remote_pos),
        Transform::from_translation(remote_pos),
    ));
    app.update();
    app.world_mut().resource_mut::<StuckReport>().reset();

    // F05 req 6: a predicted client's detector is its authority's —
    // neither armed nor observed here.
    write_impact(&mut app, 7, remote_object, ObjectId::WORLD, 30.0);
    run(&mut app, 45);
    assert_eq!(report(&app).armed, 0);
    assert_eq!(report(&app).detections, 0);
    assert_eq!(report(&app).recovered, 0);
}

#[test]
fn a_disabled_wreck_is_not_stuck_observed() {
    // A car already at MaxDamage belongs to `resolve_disabled`'s
    // outcome — the stuck detector must not queue its own recovery on
    // the same wreck.
    let (mut app, car, object) = stuck_app(Vec3::new(0.0, 1.2, 0.0), SPEC);
    {
        let world = app.world_mut();
        let mut damage = VehicleDamage::new(DAMAGE_SPEC);
        damage.apply(ImpactId(1), DAMAGE_SPEC.max_damage);
        world.entity_mut(car).insert(damage);
    }
    write_impact(&mut app, 5, object, ObjectId::WORLD, 30.0);
    run(&mut app, 45);
    assert_eq!(
        report(&app).detections,
        0,
        "the disabled outcome owns the wreck"
    );
}

#[test]
fn stale_generation_stuck_events_are_ignored() {
    let (mut app, car, object) = stuck_app(Vec3::new(0.0, 1.2, 0.0), SPEC);
    app.world_mut()
        .resource_mut::<Messages<StuckEvent>>()
        .write(StuckEvent {
            object,
            generation: 99,
            tick: 0,
        });
    app.update();
    assert_eq!(report(&app).recovered, 0);
    assert!(
        app.world().get::<mm2_vehicle::Teleported>(car).is_none(),
        "a stale event must not reset the live car"
    );
}

#[test]
fn a_trailer_rig_recovers_with_the_tractor() {
    // F05-AC05's articulated leg: the authored offsets seat the
    // trailer behind the recovered tractor — the FreeReset pattern on
    // the live pose.
    let (mut app, car, object) = stuck_app(Vec3::new(0.0, 1.2, 0.0), SPEC);
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

    write_impact(&mut app, 1, object, ObjectId::WORLD, 30.0);
    run(&mut app, 40);
    assert_eq!(report(&app).recovered, 1);

    let car_pos = app.world().get::<Position>(car).unwrap().0;
    let car_yaw = {
        let f = app.world().get::<Rotation>(car).unwrap().0 * Vec3::NEG_Z;
        (-f.x).atan2(-f.z)
    };
    let expected = car_pos + Quat::from_rotation_y(car_yaw) * offset;
    let t_pos = app.world().get::<Position>(trailer).unwrap().0;
    assert!(
        t_pos.distance(expected) < 0.5,
        "trailer re-seated at its authored offset: {t_pos} vs {expected}"
    );
}

#[test]
fn a_real_impact_arms_the_detector() {
    // End-to-end: a real Avian landing → `collect_impacts` →
    // `track_stuck` arms on the delivered event. The wheels are
    // raycasts, so a meaningful chassis impact needs the body itself
    // to arrive — drop the car on its roof like the damage suite does.
    let inverted = Quat::from_rotation_x(std::f32::consts::PI);
    let (mut app, car, _object) = stuck_app(Vec3::new(0.0, 4.0, 0.0), SPEC);
    {
        let world = app.world_mut();
        world.get_mut::<Rotation>(car).unwrap().0 = inverted;
        world.get_mut::<Transform>(car).unwrap().rotation = inverted;
    }
    let mut armed = false;
    for _ in 0..90 {
        app.update();
        armed |= report(&app).armed > 0;
    }
    assert!(
        armed,
        "a 4 m roof drop must arm the detector through the real pipeline"
    );
}
