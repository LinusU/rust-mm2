//! NETPLAY-03C: the controlled replay entry point. A replay re-runs the
//! physics schedule from a restored tick; these tests pin that it does
//! nothing else — no second frame, no clock advance, no duplicated
//! messages — that its work is bounded, and that missing history takes
//! the recovery path rather than a reset.

mod common;

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_vehicle::capture::{EpochRef, VehicleCapture, capture_vehicle};
use mm2_vehicle::physics_world::{WorldCapture, capture_world};
use mm2_vehicle::replay::{
    HistoryRing, MAX_HISTORY, MAX_REPLAY_STEPS, Recovery, ReplayAppExt, ReplayContext, ReplayError,
    ReplayStep, not_replaying, recover_missing_history, replay_from,
};
use mm2_vehicle::vehicle::VehicleInput;
use mm2_vehicle::{
    OriginalContactMaterial, ResetAuthority, ResetVehicle, VehiclePlugin, VehicleProbeLayers,
    vehicle_bundle,
};

use common::{Model, config};

const EPOCH: EpochRef = EpochRef {
    generation: 1,
    reset_epoch: 1,
};
const TOLERANCE: f32 = 1e-3;

/// A message a gameplay system would read, written from inside the
/// physics schedule on every tick.
#[derive(Message)]
struct Probe;

#[derive(Resource, Default)]
struct Counts {
    /// `Update` runs: a stand-in for rendered frames.
    updates: u32,
    /// `Last` runs.
    lasts: u32,
    /// `FixedUpdate` runs: where gameplay, audio and network sends live.
    fixed_updates: u32,
    /// Physics-schedule runs of an ungated system.
    physics_runs: u32,
    /// Physics-schedule runs of a system gated with `not_replaying`.
    gated_runs: u32,
    /// `Probe` messages read by a live consumer.
    probes_read: u32,
    /// `CollisionStart` messages read by a live consumer.
    collisions_read: u32,
}

struct Scene {
    app: App,
    car: Entity,
    other: Entity,
    ring: HistoryRing,
    tick: u64,
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
        .insert_resource(VehicleProbeLayers(LayerMask(0b01)))
        .init_resource::<Counts>()
        .add_message::<Probe>()
        .quarantine_message_in_replay::<Probe>()
        .add_systems(Update, |mut c: ResMut<Counts>| c.updates += 1)
        .add_systems(Last, |mut c: ResMut<Counts>| c.lasts += 1)
        .add_systems(FixedUpdate, |mut c: ResMut<Counts>| c.fixed_updates += 1)
        .add_systems(
            PhysicsSchedule,
            (
                |mut c: ResMut<Counts>, mut w: MessageWriter<Probe>| {
                    c.physics_runs += 1;
                    w.write(Probe);
                },
                (|mut c: ResMut<Counts>| c.gated_runs += 1).run_if(not_replaying),
            )
                .chain()
                // Probes only: they touch nothing the physics step reads.
                .ambiguous_with_all(),
        )
        .add_systems(
            Update,
            |mut c: ResMut<Counts>,
             mut probes: MessageReader<Probe>,
             mut hits: MessageReader<CollisionStart>| {
                c.probes_read += probes.read().count() as u32;
                c.collisions_read += hits.read().count() as u32;
            },
        );
    app.finish();
    app.cleanup();
    let w = app.world_mut();
    w.spawn((
        RigidBody::Static,
        Collider::cuboid(600.0, 1.0, 600.0),
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    w.spawn((
        RigidBody::Static,
        Collider::cuboid(30.0, 4.0, 1.0),
        OriginalContactMaterial {
            friction: 0.5,
            elasticity: 0.3,
        },
        Position(Vec3::new(0.0, 2.0, -5.0)),
        Transform::from_xyz(0.0, 2.0, -5.0),
    ));
    let car = w
        .spawn((
            vehicle_bundle(&config(Model::Original)),
            Position(Vec3::new(0.0, 1.2, 2.0)),
            Transform::from_xyz(0.0, 1.2, 2.0),
        ))
        .id();
    let other = w
        .spawn((
            vehicle_bundle(&config(Model::Generic)),
            Position(Vec3::new(40.0, 1.2, 0.0)),
            Transform::from_xyz(40.0, 1.2, 0.0),
        ))
        .id();
    app.update(); // the first update has no elapsed time: no step
    let mut s = Scene {
        app,
        car,
        other,
        ring: HistoryRing::default(),
        tick: 0,
    };
    record(&mut s);
    s
}

fn command(t: u64) -> (VehicleInput, VehicleInput) {
    let drive = |throttle, steering| VehicleInput {
        throttle,
        brake: 0.0,
        steering,
        handbrake: 0.0,
        forced_gear: None,
    };
    match t {
        0..40 => (drive(1.0, 0.0), drive(1.0, 0.0)),
        40..60 => (drive(0.6, 0.3), drive(0.5, -0.2)),
        _ => (drive(1.0, 0.0), drive(0.3, 0.1)),
    }
}

fn record(s: &mut Scene) {
    let cap = capture_world(s.app.world_mut(), EPOCH);
    s.ring.record(s.tick, cap);
}

/// One live tick: the scripted command, one `update()`, capture.
fn live_tick(s: &mut Scene) {
    let (a, b) = command(s.tick);
    *s.app.world_mut().get_mut::<VehicleInput>(s.car).unwrap() = a;
    *s.app.world_mut().get_mut::<VehicleInput>(s.other).unwrap() = b;
    s.app.update();
    s.tick += 1;
    record(s);
}

fn steps(s: &Scene, from: u64, to: u64) -> Vec<ReplayStep> {
    (from..to)
        .map(|t| {
            let (a, b) = command(t);
            ReplayStep {
                inputs: vec![(s.car, a), (s.other, b)],
                ..Default::default()
            }
        })
        .collect()
}

fn counts(s: &Scene) -> &Counts {
    s.app.world().resource::<Counts>()
}

fn cap(s: &Scene, e: Entity) -> VehicleCapture {
    capture_vehicle(s.app.world(), e, EPOCH).unwrap()
}

fn assert_close(a: &VehicleCapture, b: &VehicleCapture) {
    let d = a.deviation(b).expect("same discrete state");
    assert!(d.max_abs <= TOLERANCE, "deviation {d:?}");
}

/// Live ticks until the car's first wall contact, returning that tick.
fn run_to_first_hit(s: &mut Scene) -> u64 {
    for _ in 0..400 {
        live_tick(s);
        if counts(s).collisions_read > 0 {
            return s.tick;
        }
    }
    panic!("the car never reached the wall");
}

#[test]
fn a_replay_runs_the_physics_schedule_and_nothing_else() {
    let hit = {
        let mut probe = scene();
        run_to_first_hit(&mut probe)
    };
    let total = hit + 30;
    let from = hit - 10;

    let mut reference = scene();
    while reference.tick < total {
        live_tick(&mut reference);
    }

    let mut s = scene();
    while s.tick < hit + 5 {
        live_tick(&mut s);
    }
    let before = (
        counts(&s).updates,
        counts(&s).lasts,
        counts(&s).fixed_updates,
        counts(&s).gated_runs,
    );
    let (probes, collisions) = (counts(&s).probes_read, counts(&s).collisions_read);
    let time = (
        s.app.world().resource::<Time<Virtual>>().elapsed(),
        s.app.world().resource::<Time<Fixed>>().elapsed(),
        s.app.world().resource::<Time<Physics>>().elapsed(),
        s.app.world().resource::<Time<Substeps>>().elapsed(),
        s.app.world().resource::<Time>().elapsed(),
    );
    let physics_before = counts(&s).physics_runs;

    let n = s.tick - from;
    let plan = steps(&s, from, s.tick);
    let report = replay_from(s.app.world_mut(), &s.ring, from, &plan).unwrap();
    assert_eq!(report.steps as u64, n);
    // The replay ran exactly `n` physics steps: the ungated system saw
    // them, the gated one did not.
    assert_eq!(counts(&s).physics_runs - physics_before, n as u32);
    assert_eq!(counts(&s).gated_runs, before.3);
    // No render-ish frame, no fixed tick, no `Last`.
    assert_eq!(
        (
            counts(&s).updates,
            counts(&s).lasts,
            counts(&s).fixed_updates
        ),
        (before.0, before.1, before.2)
    );
    // The replay wrote `Probe` and collision messages; none reached the
    // live queues (a live consumer runs next update).
    assert!(report.suppressed_messages >= n as usize, "{report:?}");
    // Every clock is exactly where it was.
    assert_eq!(
        time,
        (
            s.app.world().resource::<Time<Virtual>>().elapsed(),
            s.app.world().resource::<Time<Fixed>>().elapsed(),
            s.app.world().resource::<Time<Physics>>().elapsed(),
            s.app.world().resource::<Time<Substeps>>().elapsed(),
            s.app.world().resource::<Time>().elapsed(),
        )
    );
    assert!(!s.app.world().resource::<ReplayContext>().active());
    assert_eq!(s.app.world().resource::<ReplayContext>().replays, 1);

    // Carry on live: every consumer count equals an uninterrupted run.
    while s.tick < total {
        live_tick(&mut s);
    }
    let (r, c) = (counts(&reference), counts(&s));
    assert_eq!(
        (
            c.updates,
            c.lasts,
            c.fixed_updates,
            c.physics_runs - n as u32,
            c.gated_runs,
            c.probes_read,
            c.collisions_read
        ),
        (
            r.updates,
            r.lasts,
            r.fixed_updates,
            r.physics_runs,
            r.gated_runs,
            r.probes_read,
            r.collisions_read
        )
    );
    assert!(c.probes_read > probes && c.collisions_read >= collisions);
    // And the replayed car is where the uninterrupted one is.
    assert_close(&cap(&s, s.car), &cap(&reference, reference.car));
    assert_close(&cap(&s, s.other), &cap(&reference, reference.other));
}

#[test]
fn a_replay_reproduces_the_uninterrupted_run_tick_for_tick() {
    let mut reference = scene();
    while reference.tick < 120 {
        live_tick(&mut reference);
    }
    let mut s = scene();
    while s.tick < 100 {
        live_tick(&mut s);
    }
    // Replay 100 -> 120 from the ring, then compare with the reference's
    // own ring tick by tick through a second, shorter replay.
    let plan = steps(&s, 60, 120);
    replay_from(s.app.world_mut(), &s.ring, 60, &plan).unwrap();
    assert_close(&cap(&s, s.car), &cap(&reference, reference.car));
    for k in 1..=20 {
        let plan = steps(&s, 100, 100 + k);
        replay_from(s.app.world_mut(), &s.ring, 100, &plan).unwrap();
        let want = reference.ring.get(100 + k).unwrap();
        assert_close(
            &cap(&s, s.car),
            &want
                .vehicles
                .iter()
                .find(|(e, _)| *e == reference.car)
                .unwrap()
                .1,
        );
    }
}

#[test]
fn replay_work_is_bounded() {
    let mut s = scene();
    for _ in 0..3 {
        live_tick(&mut s);
    }
    let one = steps(&s, 0, 1);
    let before: WorldCapture = capture_world(s.app.world_mut(), EPOCH);

    assert_eq!(
        replay_from(s.app.world_mut(), &s.ring, 0, &[]).unwrap_err(),
        ReplayError::NoSteps
    );
    let long = vec![one[0].clone(); MAX_REPLAY_STEPS + 1];
    assert_eq!(
        replay_from(s.app.world_mut(), &s.ring, 0, &long).unwrap_err(),
        ReplayError::TooManySteps {
            requested: MAX_REPLAY_STEPS + 1,
            max: MAX_REPLAY_STEPS
        }
    );
    // Neither refusal touched the world.
    let after = capture_world(s.app.world_mut(), EPOCH);
    assert_eq!(before.vehicles, after.vehicles);
    assert_eq!(s.app.world().resource::<ReplayContext>().replays, 0);

    // The longest allowed replay runs and runs exactly that many steps.
    let mut long_run = scene();
    while long_run.tick < MAX_REPLAY_STEPS as u64 {
        live_tick(&mut long_run);
    }
    let plan = steps(&long_run, 0, MAX_REPLAY_STEPS as u64);
    let report = replay_from(long_run.app.world_mut(), &long_run.ring, 0, &plan).unwrap();
    assert_eq!(report.steps, MAX_REPLAY_STEPS);
}

#[test]
fn the_history_ring_is_bounded_and_rewrites_the_future() {
    let mut ring = HistoryRing::default();
    for t in 0..(MAX_HISTORY as u64 + 50) {
        ring.record(t, WorldCapture::default());
    }
    assert_eq!(ring.len(), MAX_HISTORY);
    assert_eq!(ring.oldest(), Some(50));
    assert_eq!(ring.newest(), Some(MAX_HISTORY as u64 + 49));
    assert_eq!(ring.evicted, 50);
    assert!(ring.get(49).is_none() && ring.get(50).is_some());

    // Recording an old tick drops everything from it on.
    ring.record(100, WorldCapture::default());
    assert_eq!(ring.newest(), Some(100));
    assert!(ring.get(101).is_none());

    let mut tiny = HistoryRing::new(0);
    tiny.record(1, WorldCapture::default());
    tiny.record(2, WorldCapture::default());
    assert_eq!((tiny.len(), tiny.oldest()), (1, Some(2)));
    assert_eq!(HistoryRing::new(usize::MAX).len(), 0);
    ring.clear();
    assert!(ring.is_empty());
}

#[test]
fn missing_history_is_not_a_reset_and_recovers_from_the_authority() {
    let mut s = scene();
    while s.tick < 90 {
        live_tick(&mut s);
    }
    // The authority's complete state for tick 90, from a twin run.
    let authority = cap(&s, s.car);
    // The client has been running elsewhere and has lost the history.
    for _ in 0..20 {
        live_tick(&mut s);
    }
    s.ring.clear();
    let drifted = cap(&s, s.car);
    let before = capture_world(s.app.world_mut(), EPOCH);

    let plan = steps(&s, 90, 100);
    let err = replay_from(s.app.world_mut(), &s.ring, 90, &plan).unwrap_err();
    assert!(
        matches!(err, ReplayError::MissingHistory { tick: 90, .. }),
        "{err:?}"
    );
    // Writing nothing: no restore, no reset.
    let after = capture_world(s.app.world_mut(), EPOCH);
    assert_eq!(before.vehicles, after.vehicles);
    assert_eq!(cap(&s, s.car), drifted);

    assert_eq!(
        recover_missing_history(s.app.world_mut(), s.car, &authority).unwrap(),
        Recovery::AdoptedAuthoritative
    );
    assert_eq!(cap(&s, s.car), authority);

    // A genuine reset is the one thing that rebuilds a fresh state, and
    // it lands somewhere else entirely.
    s.app.world_mut().insert_resource(ResetAuthority(true));
    s.app.world_mut().write_message(ResetVehicle {
        entity: Some(s.car),
        position: Vec3::new(0.0, 1.2, 2.0),
        yaw: 0.0,
    });
    s.app.update();
    let reset = cap(&s, s.car);
    let d = reset.deviation(&authority);
    assert!(d.is_err() || d.unwrap().max_abs > 1.0, "reset kept history");

    // A refused authority capture writes nothing and does not reset.
    let mut bad = authority.clone();
    bad.wheels.pop();
    let held = cap(&s, s.car);
    assert!(recover_missing_history(s.app.world_mut(), s.car, &bad).is_err());
    assert_eq!(cap(&s, s.car), held);
}

/// The live run continued after a replay matches an uninterrupted run
/// exactly. This is what running the whole `FixedPostUpdate` buys: a replay
/// that ran only the inner `PhysicsSchedule` matched tick for tick inside
/// the window and then diverged by thousands of newtons on the next live
/// step, because Avian's outer preparation had not run.
#[test]
fn live_ticks_after_a_replay_match_an_uninterrupted_run() {
    let mut reference = scene();
    while reference.tick < 170 {
        live_tick(&mut reference);
    }
    for rewind in [1u64, 10] {
        let mut s = scene();
        // Rewinds across the wall contact onset (first hit near tick 108).
        while s.tick < 112 {
            live_tick(&mut s);
        }
        let from = 112 - rewind;
        let plan = steps(&s, from, 112);
        replay_from(s.app.world_mut(), &s.ring, from, &plan).unwrap();
        while s.tick < 170 {
            live_tick(&mut s);
            let want = &reference.ring.get(s.tick).unwrap();
            for (e, got) in [(s.car, cap(&s, s.car)), (s.other, cap(&s, s.other))] {
                let want = want.vehicles.iter().find(|(r, _)| r.index() == e.index());
                let d = got
                    .deviation(&want.unwrap().1)
                    .expect("same discrete state");
                assert!(
                    d.max_abs <= TOLERANCE,
                    "rewind {rewind} tick {}: {d:?}",
                    s.tick
                );
            }
        }
    }
}
