//! NETPLAY-03 follow-up (#1799): capture → restore → replay across an
//! airborne jump and landing.
//!
//! Each car (the generic model and the original-handling model, in one
//! world, far apart) drives full throttle up its own static ramp, leaves
//! the lip, flies and lands on the ground plane. A reference world runs
//! uninterrupted and records the world after every tick; a subject is run
//! past the landing, rewound to before takeoff, between takeoff and
//! landing, and after landing, and replays the same commands. It is
//! compared against the reference at **every** replayed tick.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_vehicle::capture::EpochRef;
use mm2_vehicle::physics_world::{WorldCapture, capture_world, restore_world};
use mm2_vehicle::vehicle::{VehicleInput, VehicleState};
use mm2_vehicle::{VehiclePlugin, VehicleProbeLayers, vehicle_bundle};

mod common;
use common::{Model, config};

const EPOCH: EpochRef = EpochRef {
    generation: 1,
    reset_epoch: 1,
};

/// Same bound as the other replay proofs (`docs/netcode-redesign.md`).
const TOLERANCE: f32 = 1e-3;

const SLOPE: f32 = 0.2;
const RAMP_START_Z: f32 = -10.0;
const RAMP_LEN: f32 = 24.0;
const TICKS: usize = 420;
/// Ticks replayed past the end of each rewind's flight.
const TAIL: usize = 40;

struct Scene {
    app: App,
    cars: [Entity; 2],
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
    // The ramp's top face starts at ground level at `RAMP_START_Z` and
    // rises along -Z (the way an identity-rotation car drives).
    let tilt = Quat::from_rotation_x(SLOPE);
    let along = tilt * Vec3::NEG_Z;
    let normal = tilt * Vec3::Y;
    let mut cars = [Entity::PLACEHOLDER; 2];
    for (i, (model, x)) in [(Model::Generic, 0.0), (Model::Original, 40.0)]
        .into_iter()
        .enumerate()
    {
        let centre = Vec3::new(x, 0.0, RAMP_START_Z) + along * (RAMP_LEN / 2.0) - normal * 0.5;
        w.spawn((
            RigidBody::Static,
            Collider::cuboid(8.0, 1.0, RAMP_LEN),
            Position(centre),
            Rotation(tilt),
            Transform::from_translation(centre).with_rotation(tilt),
        ));
        let at = Vec3::new(x, 1.2, 2.0);
        cars[i] = w
            .spawn((
                vehicle_bundle(&config(model)),
                Position(at),
                Transform::from_translation(at),
            ))
            .id();
    }
    Scene { app, cars }
}

/// The command at tick `t`: full throttle, a small steering wiggle on the
/// approach, then a brake after the landing. Pure in `t`.
fn command(t: usize) -> VehicleInput {
    let steering = if (30..50).contains(&t) { 0.03 } else { 0.0 };
    let (throttle, brake) = if t < 300 { (1.0, 0.0) } else { (0.0, 1.0) };
    VehicleInput {
        throttle,
        brake,
        steering,
        handbrake: 0.0,
        forced_gear: None,
    }
}

fn step(s: &mut Scene, t: usize) {
    for car in s.cars {
        *s.app.world_mut().get_mut::<VehicleInput>(car).unwrap() = command(t);
    }
    s.app.update();
}

fn snap(s: &mut Scene) -> WorldCapture {
    capture_world(s.app.world_mut(), EPOCH)
}

/// Whether car `i` has no wheel on the ground.
fn airborne(s: &Scene, i: usize) -> bool {
    s.app
        .world()
        .get::<VehicleState>(s.cars[i])
        .unwrap()
        .wheels
        .iter()
        .all(|w| !w.grounded)
}

/// The reference run: the world after each step (index `i` = after `i`
/// steps), plus for each car the first and last tick of its longest
/// all-wheels-off-the-ground flight.
fn reference() -> (Vec<WorldCapture>, [(usize, usize); 2]) {
    let mut s = scene();
    s.app.update(); // the first update has no elapsed time: no step
    let mut history = vec![snap(&mut s)];
    let mut flights = [(0usize, 0usize); 2];
    let mut open: [Option<usize>; 2] = [None; 2];
    for t in 0..TICKS {
        step(&mut s, t);
        history.push(snap(&mut s));
        for i in 0..2 {
            match (airborne(&s, i), open[i]) {
                (true, None) => open[i] = Some(t + 1),
                (false, Some(from)) => {
                    open[i] = None;
                    if t + 1 - from > flights[i].1 - flights[i].0 {
                        flights[i] = (from, t + 1);
                    }
                }
                _ => {}
            }
        }
    }
    (history, flights)
}

/// Largest deviation of the vehicles between two captures; infinity on a
/// discrete mismatch.
fn deviation(a: &WorldCapture, b: &WorldCapture) -> (f32, String) {
    let mut worst = (0.0f32, String::from("-"));
    assert_eq!(a.vehicles.len(), b.vehicles.len());
    for ((ea, va), (eb, vb)) in a.vehicles.iter().zip(&b.vehicles) {
        assert_eq!(ea, eb);
        let (d, what) = match va.deviation(vb) {
            Ok(d) => (d.max_abs, format!("vehicle {ea:?} {}", d.field)),
            Err(f) => (f32::INFINITY, format!("vehicle {ea:?} discrete {f}")),
        };
        if d.is_nan() || d > worst.0 {
            worst = (if d.is_nan() { f32::INFINITY } else { d }, what);
        }
    }
    worst
}

#[test]
fn replay_across_an_airborne_jump_and_landing_matches_the_uninterrupted_run() {
    let (reference, flights) = reference();
    for (i, (takeoff, landing)) in flights.into_iter().enumerate() {
        assert!(
            landing - takeoff >= 10,
            "car {i} never had a real flight: {takeoff}..{landing}"
        );
        eprintln!("car {i}: airborne ticks {takeoff}..{landing}");
    }
    // One subject, run past every landing, rewound to each point in turn.
    let end = flights.iter().map(|f| f.1).max().unwrap() + 20;
    let mut subject = scene();
    subject.app.update();
    let mut history = vec![snap(&mut subject)];
    for t in 0..end {
        step(&mut subject, t);
        history.push(snap(&mut subject));
    }
    // Rewind points for each car's own flight; the whole world is restored
    // and both cars are compared every time.
    let mut rewinds = Vec::new();
    for (i, (takeoff, landing)) in flights.into_iter().enumerate() {
        rewinds.push((format!("car {i} before takeoff"), takeoff - 15));
        rewinds.push((format!("car {i} mid flight"), (takeoff + landing) / 2));
        rewinds.push((format!("car {i} after landing"), landing + 5));
    }
    for (name, from) in rewinds {
        assert!(from < end, "{name}: rewind point {from} past the run");
        let report = restore_world(subject.app.world_mut(), &history[from]);
        assert!(report.refused.is_empty() && report.missing.is_empty());
        let mut worst = (0.0f32, String::from("-"));
        for t in from..end + TAIL {
            step(&mut subject, t);
            let now = snap(&mut subject);
            let (d, what) = deviation(&now, &reference[t + 1]);
            assert!(
                d <= TOLERANCE,
                "{name} (from {from}): tick {} deviates {d:e} in {what}",
                t + 1
            );
            if d > worst.0 {
                worst = (d, what);
            }
        }
        eprintln!("{name} (from {from}): worst {:e} ({})", worst.0, worst.1);
    }
}
