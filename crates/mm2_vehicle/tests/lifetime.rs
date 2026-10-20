//! NETPLAY-03E: a captured tick may never be written through an id that
//! now means a different actor. One test per way an id's lifetime ends:
//! despawn and slot reuse (traffic recycling), repurposing in place
//! (changed or broken bangers, fragment reuse, a recycled traffic car),
//! moving colliders, trailer detach, and session/reset epochs.

#[allow(dead_code)] // shared helpers; this file uses only some of them
mod common;

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_vehicle::capture::{EpochRef, RestoreError, capture_vehicle};
use mm2_vehicle::lifetime::{ActorIncarnation, LifetimeIssue, ResetEpoch, SessionEpoch};
use mm2_vehicle::physics_world::{WorldCapture, capture_world, restore_world};
use mm2_vehicle::replay::{
    HistoryRing, ReplayError, ReplayStep, recover_missing_history, replay_from,
};
use mm2_vehicle::{VehiclePlugin, VehicleProbeLayers, vehicle_bundle};

use common::{Model, config};

const EPOCH: EpochRef = EpochRef {
    generation: 1,
    reset_epoch: 1,
};

struct Scene {
    app: App,
    car: Entity,
    trailer: Entity,
    joint: Entity,
    prop: Entity,
}

fn cube(w: &mut World, at: Vec3) -> Entity {
    w.spawn((
        RigidBody::Dynamic,
        Collider::cuboid(1.0, 1.0, 1.0),
        Mass(60.0),
        Position(at),
        Transform::from_translation(at),
    ))
    .id()
}

fn scene() -> Scene {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .insert_resource(VehicleProbeLayers(LayerMask(0b01)));
    app.finish();
    app.cleanup();
    let w = app.world_mut();
    w.spawn((
        RigidBody::Static,
        Collider::cuboid(600.0, 1.0, 600.0),
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    let cfg = config(Model::Generic);
    let car = w
        .spawn((
            vehicle_bundle(&cfg),
            Position(Vec3::new(0.0, 1.2, 0.0)),
            Transform::from_xyz(0.0, 1.2, 0.0),
        ))
        .id();
    let trailer_at = Vec3::new(0.0, 1.2, -4.0);
    let trailer = w
        .spawn((
            vehicle_bundle(&config(Model::Generic)),
            Position(trailer_at),
            Transform::from_translation(trailer_at),
        ))
        .id();
    let joint = w
        .spawn((
            ChildOf(trailer),
            SphericalJoint::new(car, trailer)
                .with_local_anchor1(Vec3::new(0.0, 0.3, -2.0))
                .with_local_anchor2(Vec3::new(0.0, 0.3, 2.0)),
            JointCollisionDisabled,
        ))
        .id();
    let prop = cube(w, Vec3::new(30.0, 0.6, 0.0));
    app.update();
    Scene {
        app,
        car,
        trailer,
        joint,
        prop,
    }
}

fn tick(s: &mut Scene, n: usize) {
    for _ in 0..n {
        s.app.update();
    }
}

fn snap(s: &mut Scene) -> WorldCapture {
    capture_world(s.app.world_mut(), EPOCH)
}

fn step() -> Vec<ReplayStep> {
    vec![ReplayStep::default()]
}

fn ring(s: &mut Scene) -> HistoryRing {
    let mut r = HistoryRing::default();
    r.record(1, snap(s));
    r
}

fn pos(s: &Scene, e: Entity) -> Vec3 {
    s.app.world().get::<Position>(e).unwrap().0
}

#[test]
fn a_recycled_traffic_slot_never_aliases_the_despawned_car() {
    let mut s = scene();
    let old = cube(s.app.world_mut(), Vec3::new(60.0, 0.6, 0.0));
    tick(&mut s, 5);
    let history = ring(&mut s);
    s.app.world_mut().despawn(old);
    // Bevy parks freed ids in a 128-entry local list and only then hands
    // them out again, in no promised order: churn the allocator (as a
    // busy recycler does) and take the spawn that lands on the old index.
    let at = Vec3::new(-60.0, 0.6, 0.0);
    let churn: Vec<Entity> = (0..130).map(|_| cube(s.app.world_mut(), at)).collect();
    for e in churn {
        s.app.world_mut().despawn(e);
    }
    let reused: Vec<Entity> = (0..130).map(|_| cube(s.app.world_mut(), at)).collect();
    let new = *reused
        .iter()
        .find(|e| e.index() == old.index())
        .expect("the slot was reused");
    assert_ne!(old, new, "with a new generation");
    let before = pos(&s, new);
    let report = restore_world(s.app.world_mut(), history.get(1).unwrap());
    assert!(report.missing.contains(&old), "{report:?}");
    assert!(!report.retired.contains(&new));
    assert_eq!(pos(&s, new), before, "the new occupant was not written");
    // A vanished body is the existing `missing` path, not a stale history.
    assert!(replay_from(s.app.world_mut(), &history, 1, &step()).is_ok());
}

#[test]
fn an_entity_repurposed_in_place_is_a_different_actor() {
    // A banger that broke, a fragment pool slot reused, a traffic car
    // recycled to a new lane: same `Entity`, new `ActorIncarnation`.
    let mut s = scene();
    let history = ring(&mut s);
    tick(&mut s, 3);
    let prop = s.prop;
    s.app
        .world_mut()
        .entity_mut(prop)
        .insert((ActorIncarnation(1), Position(Vec3::new(-50.0, 0.6, 9.0))));
    let before = pos(&s, prop);
    let report = restore_world(s.app.world_mut(), history.get(1).unwrap());
    assert_eq!(report.retired, vec![prop], "{report:?}");
    assert!(report.issues.iter().any(|i| matches!(
        i,
        LifetimeIssue::Retired { entity, captured: 0, current: 1 } if *entity == prop
    )));
    assert_eq!(pos(&s, prop), before, "the new actor was not rewound");

    // A replay refuses before writing anything, car included.
    let car_before = pos(&s, s.car);
    let err = replay_from(s.app.world_mut(), &history, 1, &step()).unwrap_err();
    assert!(
        matches!(&err, ReplayError::StaleHistory(i) if i.iter().any(|i| matches!(i, LifetimeIssue::Retired { .. }))),
        "{err:?}"
    );
    assert_eq!(pos(&s, s.car), car_before);
}

#[test]
fn a_moving_collider_that_changed_hands_cold_starts_its_contacts() {
    let mut s = scene();
    tick(&mut s, 90); // the prop lands and rests on the ground
    let capture = snap(&mut s);
    assert!(!capture.pairs.is_empty());
    // Same wiring: the pair is restored.
    let report = restore_world(s.app.world_mut(), &capture);
    assert!(report.pairs_restored > 0);
    // The body under the collider is replaced in place.
    let prop = s.prop;
    s.app
        .world_mut()
        .entity_mut(prop)
        .insert(ActorIncarnation(7));
    let report = restore_world(s.app.world_mut(), &capture);
    assert_eq!(report.retired, vec![prop]);
    assert!(
        report.pairs_invalidated > 0,
        "the old manifolds must not warm-start another actor: {report:?}"
    );
}

#[test]
fn a_trailer_attach_or_detach_changes_the_topology() {
    let mut s = scene();
    let history = ring(&mut s);
    // Detach.
    s.app.world_mut().despawn(s.joint);
    let err = replay_from(s.app.world_mut(), &history, 1, &step()).unwrap_err();
    assert!(
        matches!(&err, ReplayError::StaleHistory(i)
            if i.contains(&LifetimeIssue::HitchChanged { joint: s.joint, attached: false })),
        "{err:?}"
    );

    // Hitch to a different trailer: the old joint is gone, a new one is in.
    let (car, trailer) = (s.car, s.trailer);
    let new = s
        .app
        .world_mut()
        .spawn(SphericalJoint::new(car, trailer))
        .id();
    let issues = mm2_vehicle::lifetime::check_lifetimes(s.app.world_mut(), history.get(1).unwrap());
    assert!(issues.contains(&LifetimeIssue::HitchChanged {
        joint: new,
        attached: true
    }));
}

#[test]
fn another_session_generation_writes_nothing() {
    let mut s = scene();
    let history = ring(&mut s);
    s.app.world_mut().insert_resource(SessionEpoch(2));
    tick(&mut s, 3);
    let (car, prop) = (pos(&s, s.car), pos(&s, s.prop));
    let report = restore_world(s.app.world_mut(), history.get(1).unwrap());
    assert!(matches!(
        report.issues[0],
        LifetimeIssue::StaleSession {
            captured: 1,
            current: 2
        }
    ));
    assert_eq!((pos(&s, s.car), pos(&s, s.prop)), (car, prop));
    let err = replay_from(s.app.world_mut(), &history, 1, &step()).unwrap_err();
    assert!(matches!(err, ReplayError::StaleHistory(_)));
    assert_eq!(pos(&s, s.car), car);
}

#[test]
fn a_vehicle_reset_since_the_capture_cannot_be_rewound() {
    let mut s = scene();
    let history = ring(&mut s);
    let car = s.car;
    s.app.world_mut().entity_mut(car).insert(ResetEpoch(2));
    let before = pos(&s, car);
    let report = restore_world(s.app.world_mut(), history.get(1).unwrap());
    assert!(
        report.refused.contains(&(car, RestoreError::StaleEpoch)),
        "{report:?}"
    );
    assert_eq!(pos(&s, car), before);
    let err = replay_from(s.app.world_mut(), &history, 1, &step()).unwrap_err();
    assert!(matches!(&err, ReplayError::StaleHistory(i)
        if i.iter().any(|i| matches!(i, LifetimeIssue::StaleReset { .. }))));
    assert_eq!(pos(&s, car), before);

    // The authority's capture, stamped with the new epoch, is adopted.
    let authority = capture_vehicle(
        s.app.world(),
        car,
        EpochRef {
            generation: 1,
            reset_epoch: 2,
        },
    )
    .unwrap();
    assert!(recover_missing_history(s.app.world_mut(), car, &authority).is_ok());
    // And a capture from the old epoch is refused the same way.
    let old = capture_vehicle(s.app.world(), car, EPOCH).unwrap();
    assert_eq!(
        recover_missing_history(s.app.world_mut(), car, &old),
        Err(RestoreError::StaleEpoch)
    );
}

#[test]
fn matching_epochs_and_incarnations_replay_normally() {
    let mut s = scene();
    s.app.world_mut().insert_resource(SessionEpoch(1));
    s.app.world_mut().entity_mut(s.car).insert(ResetEpoch(1));
    s.app
        .world_mut()
        .entity_mut(s.prop)
        .insert(ActorIncarnation(4));
    let history = ring(&mut s);
    tick(&mut s, 3);
    let report = replay_from(s.app.world_mut(), &history, 1, &step()).unwrap();
    assert!(report.restore.issues.is_empty());
    assert!(report.restore.retired.is_empty());
}
