//! F01-B contract integration: the impact pipeline and telemetry
//! publisher running against real Avian physics in a headless app —
//! the same harness `mm2_app::smoke::headless_smoke` uses.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::contracts::{self, ImpactFilter};
use mm2_game::{
    AuthorityRole, DamageSignals, ImpactEvent, ImpactId, ImpactPolicy, ObjectId, ObjectIdentity,
    Player, PlayerControl, Session, SessionConfig, SessionPhase, SurfaceMaterial, VehicleTelemetry,
    advance_session_tick,
};
use mm2_vehicle::{VehicleConfig, VehiclePlugin, vehicle_bundle};

const FRAMES_PER_SECOND: usize = 60;

/// A playing session plus a marked ground plane and the player's car,
/// fully stamped with contract identities. Returns the minted object id.
fn test_app(car_pos: Vec3) -> (App, Entity, ObjectId) {
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
        // Deterministic: every app.update() is exactly one 60 Hz frame.
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .insert_resource(session)
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .init_resource::<ImpactFilter>()
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (
                contracts::collect_impacts,
                contracts::publish_vehicle_telemetry,
            )
                .chain(),
        );
    app.finish();
    app.cleanup();

    // Ground marked with an authored surface code — unverified meaning,
    // carried as data.
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(400.0, 1.0, 400.0),
        SurfaceMaterial::Authored(3),
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));

    let car = app
        .world_mut()
        .spawn((
            ObjectIdentity(object),
            Player {
                id: player,
                control: PlayerControl::Local,
            },
            role,
            DamageSignals::default(),
            vehicle_bundle(&VehicleConfig::default()),
            Position(car_pos),
            Transform::from_translation(car_pos),
        ))
        .id();
    (app, car, object)
}

fn drain_impacts(app: &mut App) -> Vec<ImpactEvent> {
    app.world_mut()
        .resource_mut::<Messages<ImpactEvent>>()
        .drain()
        .collect()
}

#[test]
fn telemetry_publishes_a_stamped_snapshot_each_step() {
    let (mut app, car, object) = test_app(Vec3::new(0.0, 1.2, 0.0));
    for _ in 0..FRAMES_PER_SECOND {
        app.update();
    }
    let t = app
        .world()
        .get::<VehicleTelemetry>(car)
        .expect("telemetry must be published while playing");
    // Identity, tick and authority are the contract's stamps.
    assert_eq!(t.object, object);
    assert_eq!(t.authority, AuthorityRole::Authority);
    let tick = app.world().resource::<Session>().tick();
    assert_eq!(t.tick, tick, "snapshot names the step it came from");
    // Settled on the marked ground: every wheel reports contact and the
    // authored surface code under it.
    assert!(t.wheels.iter().all(|w| w.grounded), "car should settle");
    for w in &t.wheels {
        assert_eq!(w.surface.material, SurfaceMaterial::Authored(3));
        assert_eq!(w.surface.traction, 1.0);
    }
    assert!(t.position.is_finite() && t.linear_velocity.is_finite());
    assert!(t.rpm > 0.0);
    assert!(!t.reverse);
}

#[test]
fn a_hard_landing_emits_bounded_impacts_with_stable_participants() {
    // The wheels are raycasts — they never produce a collider contact —
    // so a meaningful chassis impact needs the body itself to arrive:
    // drop the car on its roof from 4 m.
    let inverted = Quat::from_rotation_x(std::f32::consts::PI);
    let (mut app, car, object) = test_app(Vec3::new(0.0, 4.0, 0.0));
    {
        let world = app.world_mut();
        world.get_mut::<Rotation>(car).unwrap().0 = inverted;
        world.get_mut::<Transform>(car).unwrap().rotation = inverted;
    }
    // Messages live two frame updates, so drain each update — a single
    // drain after the run would see only the last frames' events.
    let mut events = Vec::new();
    for _ in 0..FRAMES_PER_SECOND * 2 {
        app.update();
        events.extend(drain_impacts(&mut app));
    }
    assert!(
        !events.is_empty(),
        "a roof-first 4 m drop should produce an impact"
    );
    // Every event: unique monotonic id, session stamps, real severity.
    let mut last = 0;
    for e in &events {
        assert_eq!(e.id, ImpactId(last + 1), "ids are dense and ordered");
        last = e.id.0;
        assert_eq!(e.generation, 1);
        assert!(e.tick > 0);
        assert!(e.severity >= ImpactPolicy::default().min_severity);
        assert!(e.point.is_finite() && e.normal.is_finite());
        // Participants are stable ids — the car's minted ObjectId vs. the
        // unmarked static world's WORLD sentinel.
        let pair = [e.participants.0, e.participants.1];
        assert!(pair.contains(&object), "the car is a participant");
        assert!(pair.contains(&ObjectId::WORLD), "the world resolves");
        // The passive side was the marked ground.
        assert_eq!(e.surface.material, SurfaceMaterial::Authored(3));
    }
    let worst = events.iter().map(|e| e.severity).fold(0.0f32, f32::max);
    assert!(
        worst > 2.0,
        "a 4 m roof drop is a real impact, worst severity {worst} m/s"
    );
    // The car's damage signal accumulated the impacts that involved it.
    let damage = app.world().get::<DamageSignals>(car).unwrap();
    assert!(damage.impact_count >= 1);
    assert!(damage.impact_total >= worst);
}

#[test]
fn paused_session_publishes_nothing_and_drains_contact_edges() {
    let (mut app, car, _) = test_app(Vec3::new(0.0, 1.2, 0.0));
    for _ in 0..FRAMES_PER_SECOND {
        app.update();
    }
    let before = app.world().get::<VehicleTelemetry>(car).unwrap().tick;
    let emitted = app.world().resource::<ImpactFilter>().emitted;
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    for _ in 0..FRAMES_PER_SECOND {
        app.update();
    }
    // Telemetry freezes with the session clock rather than stamping a
    // stale tick onto still-running physics.
    let after = app.world().get::<VehicleTelemetry>(car).unwrap().tick;
    assert_eq!(before, after, "paused session publishes no new snapshot");
    assert_eq!(
        app.world().resource::<ImpactFilter>().emitted,
        emitted,
        "paused session emits no impacts"
    );
}
