//! NETPLAY-05C: a contact replay runs against the world as it was, not as it
//! is. Kinematic traffic is put back along its recorded path, an actor that
//! spawned inside the window stays out until its entry tick, and one that
//! despawned is reported rather than guessed at.
//!
//! Each test drives the production `VehiclePlugin` at 60 Hz, records the
//! whole world every tick, then replays a window and compares with what the
//! live run recorded for the last tick. The control run strips the actor
//! history from the ring (the pre-05C behaviour) and must visibly diverge,
//! so the comparison cannot pass by the actor never mattering.

mod common;

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_vehicle::capture::{EpochRef, VehicleCapture, capture_vehicle};
use mm2_vehicle::physics_world::capture_world;
use mm2_vehicle::replay::{HistoryRing, ReplayStep, replay_from};
use mm2_vehicle::vehicle::VehicleInput;
use mm2_vehicle::{VehiclePlugin, VehicleProbeLayers, vehicle_bundle};

use common::{Model, config};

const EPOCH: EpochRef = EpochRef {
    generation: 1,
    reset_epoch: 1,
};
const TOLERANCE: f32 = 1e-3;
const VELOCITY_TOLERANCE: f32 = 1e-2;
const MOVER_SPEED: f32 = 8.0;
/// The mover brakes to a stop at this tick: a velocity change that only a
/// recorded history knows about.
const MOVER_STOPS: u64 = 130;
const MOVER_Z: f32 = -8.0;

/// Whether the tick-driven mover runs (the "traffic system").
#[derive(Resource)]
struct Tick(u64);

struct Scene {
    app: App,
    car: Entity,
    mover: Option<Entity>,
    ring: HistoryRing,
    tick: u64,
}

fn drive() -> VehicleInput {
    VehicleInput {
        throttle: 1.0,
        brake: 0.0,
        steering: 0.0,
        handbrake: 0.0,
        forced_gear: None,
    }
}

/// A traffic system as the app has it: it lives in `FixedUpdate`, which a
/// replay never runs, and changes a kinematic body's velocity.
fn drive_traffic(tick: Res<Tick>, mut movers: Query<&mut LinearVelocity, With<Mover>>) {
    for mut v in &mut movers {
        v.0 = if tick.0 < MOVER_STOPS {
            Vec3::X * MOVER_SPEED
        } else {
            Vec3::ZERO
        };
    }
}

#[derive(Component)]
struct Mover;

fn scene(mover_x: Option<f32>) -> Scene {
    scene_of(Model::Original, mover_x)
}

fn scene_of(model: Model, mover_x: Option<f32>) -> Scene {
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
        .insert_resource(VehicleProbeLayers(LayerMask(0b01)))
        .insert_resource(Tick(0))
        .add_systems(FixedUpdate, drive_traffic);
    app.finish();
    app.cleanup();
    let w = app.world_mut();
    w.spawn((
        RigidBody::Static,
        Collider::cuboid(600.0, 1.0, 600.0),
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    let car = w
        .spawn((
            vehicle_bundle(&config(model)),
            Position(Vec3::new(0.0, 1.2, 2.0)),
            Transform::from_xyz(0.0, 1.2, 2.0),
        ))
        .id();
    let mover = mover_x.map(|x| {
        let pos = Vec3::new(x, 1.0, MOVER_Z);
        w.spawn((
            Mover,
            RigidBody::Kinematic,
            Collider::cuboid(4.0, 2.0, 6.0),
            Position(pos),
            Transform::from_translation(pos),
            LinearVelocity(Vec3::X * MOVER_SPEED),
        ))
        .id()
    });
    app.update(); // the first update has no elapsed time: no step
    let mut s = Scene {
        app,
        car,
        mover,
        ring: HistoryRing::default(),
        tick: 0,
    };
    record(&mut s);
    s
}

fn record(s: &mut Scene) {
    let cap = capture_world(s.app.world_mut(), EPOCH);
    s.ring.record(s.tick, cap);
}

fn live_tick(s: &mut Scene) {
    *s.app.world_mut().get_mut::<VehicleInput>(s.car).unwrap() = drive();
    s.app.world_mut().resource_mut::<Tick>().0 = s.tick;
    s.app.update();
    s.tick += 1;
    record(s);
}

fn steps(car: Entity, from: u64, to: u64) -> Vec<ReplayStep> {
    (from..to)
        .map(|_| ReplayStep {
            inputs: vec![(car, drive())],
        })
        .collect()
}

fn car_state(s: &Scene) -> VehicleCapture {
    capture_vehicle(s.app.world(), s.car, EPOCH).unwrap()
}

fn recorded_car(s: &Scene, tick: u64) -> VehicleCapture {
    s.ring.get(tick).unwrap().vehicles[0].1.clone()
}

/// Largest pose and velocity gap of the car bodies. A contact onset is
/// solver-order noise at the 1e-2 m/s level (docs/netcode-redesign.md,
/// NETPLAY-03B), so contact scenes compare the body, not every internal
/// accumulator. A different discrete state counts as infinitely far.
fn deviation(a: &VehicleCapture, b: &VehicleCapture) -> (f32, f32) {
    if a.deviation(b).is_err() {
        return (f32::INFINITY, f32::INFINITY);
    }
    let gap = |x: [f32; 3], y: [f32; 3]| Vec3::from_array(x).distance(Vec3::from_array(y));
    (
        gap(a.body.position, b.body.position),
        gap(a.body.linear_velocity, b.body.linear_velocity),
    )
}

fn assert_replayed(a: &VehicleCapture, b: &VehicleCapture) {
    let (pos, vel) = deviation(a, b);
    assert!(
        pos <= TOLERANCE && vel <= VELOCITY_TOLERANCE,
        "{pos} m, {vel} m/s"
    );
}

/// The tick the car's nose first reaches the mover's lane, found on a
/// scene with no mover.
fn arrival_tick() -> u64 {
    let mut s = scene(None);
    for _ in 0..600 {
        live_tick(&mut s);
        if s.app.world().get::<Position>(s.car).unwrap().0.z < MOVER_Z + 6.0 {
            return s.tick;
        }
    }
    panic!("the car never reached the lane");
}

#[test]
fn traffic_is_replayed_along_its_recorded_path_not_from_today() {
    let arrive = arrival_tick();
    // Put the mover across the car's path as the car arrives.
    let x0 = -MOVER_SPEED * arrive as f32 / 60.0;
    let mut s = scene(Some(x0));
    let end = arrive + 40;
    for _ in 0..end {
        live_tick(&mut s);
    }
    let mover = s.mover.unwrap();
    let live_mover = s.app.world().get::<Position>(mover).unwrap().0;
    let from = arrive - 25;

    // Contact happened inside the window, or the test proves nothing.
    let hit = (from..=end).any(|t| {
        let c = s.ring.get(t).unwrap();
        c.pairs.iter().any(|(_, p)| p.touching()) && c.pairs.len() > 1 // ground contact is one pair per wheel group
    });
    assert!(hit, "the car never touched the mover in the window");

    // Control: a ring without the mover history replays against today's
    // scene, where the mover has already stopped elsewhere.
    let mut blind = s.ring.clone();
    for t in from..=end {
        // `HistoryRing::get` is read-only: rebuild the capture without movers.
        let mut c = s.ring.get(t).unwrap().clone();
        c.movers = None;
        blind.record(t, c);
    }
    let report = replay_from(s.app.world_mut(), &blind, from, &steps(s.car, from, end)).unwrap();
    assert!(report.entered.is_empty());
    let (blind_pos, _) = deviation(&car_state(&s), &recorded_car(&s, end));
    assert!(
        blind_pos > 0.05,
        "the control replay should diverge, deviation {blind_pos}"
    );

    // Put the world back on the live timeline by replaying with history,
    // starting from a restore of the recorded tick.
    let report = replay_from(s.app.world_mut(), &s.ring, from, &steps(s.car, from, end)).unwrap();
    assert!(report.entered.is_empty());
    assert!(report.restore.missing.is_empty());
    assert_replayed(&car_state(&s), &recorded_car(&s, end));
    let mover_now = s.app.world().get::<Position>(mover).unwrap().0;
    assert!(
        (mover_now - live_mover).length() <= TOLERANCE,
        "the mover ends where the live run left it: {mover_now} vs {live_mover}"
    );
    let v = s.app.world().get::<LinearVelocity>(mover).unwrap().0;
    assert_eq!(v, Vec3::ZERO, "and with the velocity of that tick");
}

#[test]
fn a_prop_that_spawned_in_the_window_enters_at_its_tick() {
    let mut s = scene(None);
    for _ in 0..50 {
        live_tick(&mut s);
    }
    let from = s.tick;
    for _ in 0..10 {
        live_tick(&mut s);
    }
    // A crate appears 3 m ahead of the car, on the road.
    let car_pos = s.app.world().get::<Position>(s.car).unwrap().0;
    let pos = car_pos + Vec3::new(0.0, 0.0, -3.0);
    let prop = s
        .app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            Position(pos),
            Transform::from_translation(pos),
        ))
        .id();
    // Captured at the spawn tick, as the app's per-tick record would.
    record(&mut s);
    let entry = s.tick;
    for _ in 0..70 {
        live_tick(&mut s);
    }
    let end = s.tick;
    let prop_live = s.app.world().get::<Position>(prop).unwrap().0;
    assert!(
        (prop_live - pos).length() > 0.5,
        "the car pushed the prop: {prop_live} vs {pos}"
    );

    let report = replay_from(s.app.world_mut(), &s.ring, from, &steps(s.car, from, end)).unwrap();
    assert_eq!(report.restore.uncaptured, vec![prop]);
    assert_eq!(report.entered, vec![(prop, Some(entry))]);
    assert!(!s.app.world().entity(prop).contains::<RigidBodyDisabled>());
    assert_replayed(&car_state(&s), &recorded_car(&s, end));
    let prop_now = s.app.world().get::<Position>(prop).unwrap().0;
    let recorded = s.ring.get(end).unwrap().bodies[0].1.position;
    assert!(
        (prop_now - recorded).length() <= 5.0 * TOLERANCE,
        "the prop took one impulse, not two: {prop_now} vs {recorded}"
    );
}

#[test]
fn a_prop_that_despawned_in_the_window_is_reported_missing() {
    let mut s = scene_of(Model::Generic, None);
    let pos = Vec3::new(0.0, 0.5, -30.0);
    let prop = s
        .app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            Position(pos),
            Transform::from_translation(pos),
        ))
        .id();
    record(&mut s);
    let from = s.tick;
    for _ in 0..20 {
        live_tick(&mut s);
    }
    s.app.world_mut().despawn(prop);
    for _ in 0..5 {
        live_tick(&mut s);
    }
    let end = s.tick;
    let report = replay_from(s.app.world_mut(), &s.ring, from, &steps(s.car, from, end)).unwrap();
    assert_eq!(report.restore.missing, vec![prop]);
    assert!(report.entered.is_empty());
}
