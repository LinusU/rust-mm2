//! NETPLAY-03B: capture → advance → restore → replay of the whole Avian
//! world, not just the vehicle components.
//!
//! The scene runs the production `VehiclePlugin` schedules at `FIXED_HZ`
//! 60 (one physics step per `app.update()`): an original-model human car
//! that drives into a static wall and stays pinned against it, a generic
//! human car towing a jointed trailer into a sleep-capable prop, child
//! colliders and a sensor on the first car, and a ground plane. A
//! reference world runs the scripted commands uninterrupted and records
//! the world at every tick; a subject world is rewound to several depths
//! and replays, and is compared against the reference tick by tick, not
//! just right after the setters.

use std::time::Duration;

use avian3d::prelude::*;
use avian3d::schedule::LastPhysicsTick;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_vehicle::capture::EpochRef;
use mm2_vehicle::physics_world::{WorldCapture, capture_world, restore_world};
use mm2_vehicle::vehicle::VehicleInput;
use mm2_vehicle::{
    OriginalContactMaterial, VehicleConfig, VehiclePlugin, VehicleProbeLayers, vehicle_bundle,
};

mod common;
use common::{Model, config};

const EPOCH: EpochRef = EpochRef {
    generation: 1,
    reset_epoch: 1,
};

/// Largest replay-versus-reference deviation accepted on any float, in the
/// field's own unit. Measured, see `docs/netcode-redesign.md`.
const TOLERANCE: f32 = 1e-3;

struct Scene {
    app: App,
    a: Entity,
    b: Entity,
    trailer: Entity,
    prop: Entity,
    wall: Entity,
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
    // The wall car A drives into.
    let wall = w
        .spawn((
            RigidBody::Static,
            Collider::cuboid(30.0, 4.0, 1.0),
            OriginalContactMaterial {
                friction: 0.5,
                elasticity: 0.3,
            },
            Position(Vec3::new(0.0, 2.0, WALL_Z)),
            Transform::from_xyz(0.0, 2.0, WALL_Z),
        ))
        .id();
    let cfg_a = config(Model::Original);
    let a = w
        .spawn((
            vehicle_bundle(&cfg_a),
            Position(Vec3::new(0.0, 1.2, 2.0)),
            Transform::from_xyz(0.0, 1.2, 2.0),
        ))
        .id();
    // A child collider and a sensor on car A.
    w.spawn((
        ChildOf(a),
        Collider::sphere(0.4),
        ColliderDensity(0.0),
        child_layers(),
        Transform::from_xyz(0.0, 0.8, 0.0),
    ));
    w.spawn((
        ChildOf(a),
        Collider::sphere(3.0),
        Sensor,
        ColliderDensity(0.0),
        child_layers(),
        Transform::from_xyz(0.0, 0.5, FWD * 2.0),
    ));
    let cfg_b = config(Model::Generic);
    let b_pos = Vec3::new(40.0, 1.2, 0.0);
    let b = w
        .spawn((
            vehicle_bundle(&cfg_b),
            Position(b_pos),
            Transform::from_translation(b_pos),
        ))
        .id();
    let hitch = Vec3::new(0.0, 0.3, -FWD * 2.2);
    let trailer_pos = b_pos + hitch * 2.0;
    let trailer = w
        .spawn((
            vehicle_bundle(&VehicleConfig::default()),
            Position(trailer_pos),
            Transform::from_translation(trailer_pos),
        ))
        .id();
    w.spawn((
        ChildOf(trailer),
        SphericalJoint::new(b, trailer)
            .with_local_anchor1(hitch)
            .with_local_anchor2(-hitch)
            .with_swing_limits(-0.6, 0.6)
            .with_twist_limits(-0.15, 0.15),
        JointCollisionDisabled,
    ));
    // A sleep-capable prop in B's path.
    let prop_pos = Vec3::new(40.0, 0.5, PROP_Z);
    let prop = w
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            Mass(60.0),
            Position(prop_pos),
            Transform::from_translation(prop_pos),
        ))
        .id();
    Scene {
        app,
        a,
        b,
        trailer,
        prop,
        wall,
    }
}

/// +1 / -1: the world-z direction a car with identity rotation drives.
/// Child colliders and sensors sit on their own layer so the wheel rays
/// (which exclude only the body entity) do not see them.
fn child_layers() -> CollisionLayers {
    CollisionLayers::new(0b10u32, LayerMask::ALL)
}

const FWD: f32 = -1.0;
const WALL_Z: f32 = FWD * 5.0;
const PROP_Z: f32 = FWD * 14.0;

fn cmd(throttle: f32, brake: f32, steering: f32, handbrake: f32) -> VehicleInput {
    VehicleInput {
        throttle,
        brake,
        steering,
        handbrake,
        forced_gear: None,
    }
}

/// The scripted command for car A and car B at tick `t`: launch in a
/// forced first gear, steer, brake, shift up, then full throttle into the
/// wall and a long pin against it; B drives into the prop towing its
/// trailer. Pure in `t`, so a replay applies the same one.
fn commands(t: usize) -> (VehicleInput, VehicleInput) {
    let first = Some(mm2_vehicle::original::FIRST_GEAR);
    let forced = |g, c: VehicleInput| VehicleInput {
        forced_gear: g,
        ..c
    };
    let a = match t {
        0..40 => forced(first, cmd(1.0, 0.0, 0.0, 0.0)),
        40..70 => forced(first, cmd(1.0, 0.0, 0.05, 0.0)),
        70..85 => cmd(0.0, 1.0, -0.05, 0.0),
        85..100 => forced(first.map(|g| g + 1), cmd(1.0, 0.0, -0.05, 0.0)),
        _ => forced(first.map(|g| g + 1), cmd(1.0, 0.0, 0.0, 0.0)),
    };
    let b = match t {
        0..50 => cmd(1.0, 0.0, 0.0, 0.0),
        50..70 => cmd(0.5, 0.0, 0.4, 0.0),
        70..80 => cmd(0.0, 1.0, 0.0, 0.0),
        _ => cmd(1.0, 0.0, -0.1, 0.0),
    };
    (a, b)
}

fn step(s: &mut Scene, t: usize) {
    let (a, b) = commands(t);
    *s.app.world_mut().get_mut::<VehicleInput>(s.a).unwrap() = a;
    *s.app.world_mut().get_mut::<VehicleInput>(s.b).unwrap() = b;
    s.app.update();
}

fn snap(s: &mut Scene) -> WorldCapture {
    capture_world(s.app.world_mut(), EPOCH)
}

/// Largest component difference of two rotations (`q` and `-q` agree).
fn quat_diff(a: Quat, b: Quat) -> f32 {
    let b = if a.dot(b) < 0.0 { -b } else { b };
    (a - b)
        .to_array()
        .iter()
        .fold(0.0f32, |m, v| m.max(v.abs()))
}

/// Ticks on which any body's sleeping flag differs.
fn sleep_mismatch(a: &WorldCapture, b: &WorldCapture) -> bool {
    a.bodies
        .iter()
        .zip(&b.bodies)
        .any(|(x, y)| x.1.sleeping != y.1.sleeping)
}

/// Largest float deviation between two captures of the same scene;
/// infinity on a discrete mismatch.
fn deviation(a: &WorldCapture, b: &WorldCapture) -> (f32, String) {
    let mut worst = (0.0f32, String::from("-"));
    let mut note = |d: f32, what: String| {
        if d.is_nan() || d > worst.0 {
            worst = (if d.is_nan() { f32::INFINITY } else { d }, what);
        }
    };
    assert_eq!(a.vehicles.len(), b.vehicles.len());
    for ((ea, va), (eb, vb)) in a.vehicles.iter().zip(&b.vehicles) {
        assert_eq!(ea, eb);
        match va.deviation(vb) {
            Ok(d) => note(d.max_abs, format!("vehicle {ea:?} {}", d.field)),
            Err(f) => note(f32::INFINITY, format!("vehicle {ea:?} discrete {f}")),
        }
    }
    assert_eq!(a.bodies.len(), b.bodies.len());
    for ((ea, sa), (_, sb)) in a.bodies.iter().zip(&b.bodies) {
        let d = (sa.position - sb.position)
            .abs()
            .max_element()
            .max(
                (sa.linear_velocity - sb.linear_velocity)
                    .abs()
                    .max_element(),
            )
            .max(
                (sa.angular_velocity - sb.angular_velocity)
                    .abs()
                    .max_element(),
            )
            .max(quat_diff(sa.rotation, sb.rotation));
        note(
            d,
            format!(
                "body {ea:?} {:?}",
                [
                    (sa.position - sb.position).abs().max_element(),
                    (sa.linear_velocity - sb.linear_velocity)
                        .abs()
                        .max_element(),
                    (sa.angular_velocity - sb.angular_velocity)
                        .abs()
                        .max_element(),
                    quat_diff(sa.rotation, sb.rotation),
                ]
            ),
        );
    }
    worst
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Strategy {
    /// `restore_world`: bodies, sleep state, captured contacts.
    Full,
    /// `restore_world` with the contacts left out of the capture: every
    /// pair is cold-started.
    ColdContacts,
    /// Only the typed vehicle and prop state; the contact graph keeps
    /// whatever the abandoned future left in it.
    StateOnly,
}

fn restore(s: &mut Scene, capture: &WorldCapture, strategy: Strategy) {
    match strategy {
        Strategy::Full => {
            let report = restore_world(s.app.world_mut(), capture);
            assert!(report.refused.is_empty() && report.missing.is_empty());
        }
        Strategy::ColdContacts => {
            let mut c = capture.clone();
            c.pairs.clear();
            restore_world(s.app.world_mut(), &c);
        }
        Strategy::StateOnly => {
            for (e, v) in &capture.vehicles {
                mm2_vehicle::capture::restore_vehicle(s.app.world_mut(), *e, v).unwrap();
            }
            for (e, b) in &capture.bodies {
                let mut ent = s.app.world_mut().entity_mut(*e);
                ent.get_mut::<Position>().unwrap().0 = b.position;
                ent.get_mut::<Rotation>().unwrap().0 = b.rotation;
                ent.get_mut::<LinearVelocity>().unwrap().0 = b.linear_velocity;
                ent.get_mut::<AngularVelocity>().unwrap().0 = b.angular_velocity;
            }
        }
    }
}

/// Run the reference: the captured world after each of `ticks` steps
/// (index `i` = state after `i` steps).
fn reference(ticks: usize) -> (Scene, Vec<WorldCapture>) {
    let mut s = scene();
    s.app.update(); // the first update has no elapsed time: no step
    let mut history = vec![snap(&mut s)];
    for t in 0..ticks {
        step(&mut s, t);
        history.push(snap(&mut s));
    }
    (s, history)
}

/// A subject world run to `run_to`, with the world captured after every
/// step (index `i` = after `i` steps).
fn subject(run_to: usize) -> (Scene, Vec<WorldCapture>) {
    reference(run_to)
}

/// Replay from `from` to `to` and call `check` after every step with the
/// tick index and the reference capture of that tick.
fn replay(s: &mut Scene, from: usize, to: usize, mut check: impl FnMut(usize, &WorldCapture)) {
    for t in from..to {
        step(s, t);
        let now = snap(s);
        check(t + 1, &now);
    }
}

const TICKS: usize = 460;

/// Rewind a subject run to `run_to` back to `rewind_to`, replay `tail`
/// ticks past `run_to`, and return the worst float deviation from the
/// reference over **every** replayed tick (so late divergence counts, not
/// just the first step after the setters) plus the number of ticks on
/// which a sleep flag differed.
fn rewind_case(
    reference: &[WorldCapture],
    rewind_to: usize,
    run_to: usize,
    tail: usize,
    strategy: Strategy,
) -> (f32, String, usize) {
    let (mut s, history) = subject(run_to);
    restore(&mut s, &history[rewind_to], strategy);
    let mut worst = (0.0f32, String::from("-"));
    let mut sleep_ticks = 0;
    replay(&mut s, rewind_to, run_to + tail, |t, now| {
        let (d, what) = deviation(now, &reference[t]);
        if d > worst.0 {
            worst = (d, what);
        }
        sleep_ticks += usize::from(sleep_mismatch(now, &reference[t]));
    });
    eprintln!(
        "{strategy:?} {rewind_to}->{run_to}+{tail}: worst {:e} ({}), sleep-flag mismatches {sleep_ticks}",
        worst.0, worst.1
    );
    (worst.0, worst.1, sleep_ticks)
}

/// (rewind to, run to): different depths, each crossing a different
/// event. Tick 15-25: tyres load and the launch gear; 70-100: brake and
/// the forced gear change; ~175: A reaches the wall (contact onset with a
/// static wall); 240+: A is pinned against it; ~225: B and the trailer
/// reach the prop and wake it (contact onset with a sleeping prop).
const NO_NEW_DYNAMIC_CONTACT: [(usize, usize); 7] = [
    (19, 20),   // depth 1, loaded tyres
    (60, 120),  // depth 60, brake + gear transitions
    (100, 120), // depth 20, prop asleep throughout
    (150, 200), // depth 50, into the wall's contact onset
    (236, 300), // depth 64, multi-step wall pin
    (299, 300), // depth 1, pinned
    (300, 380), // depth 80, pinned
];

#[test]
fn replay_after_a_full_restore_matches_the_uninterrupted_run() {
    let (_s, reference) = reference(TICKS);
    for (rewind_to, run_to) in NO_NEW_DYNAMIC_CONTACT {
        let (worst, what, sleep_ticks) =
            rewind_case(&reference, rewind_to, run_to, 40, Strategy::Full);
        assert!(
            worst <= TOLERANCE,
            "{rewind_to}->{run_to}: {what} deviates {worst:e}"
        );
        assert_eq!(sleep_ticks, 0, "{rewind_to}->{run_to}: sleep flags");
    }
}

#[test]
fn rewinding_across_a_new_dynamic_contact_onset_stays_bounded() {
    // B and its trailer hit the sleeping prop at ~tick 225; the subject
    // is at 250 and rewinds to 190, so the replay creates the B-prop
    // contact again in a world whose contact slots and solver ordering
    // were not rebuilt in the reference's order. The positions stay
    // together; per-wheel force scalars carry solver-order noise.
    let (_s, reference) = reference(TICKS);
    let (mut s, history) = subject(250);
    restore(&mut s, &history[190], Strategy::Full);
    let (mut dpos, mut dvel) = (0.0f32, 0.0f32);
    replay(&mut s, 190, 330, |t, now| {
        for ((_, a), (_, b)) in now.vehicles.iter().zip(&reference[t].vehicles) {
            dpos = dpos.max((Vec3::from(a.body.position) - Vec3::from(b.body.position)).length());
            dvel = dvel.max(
                (Vec3::from(a.body.linear_velocity) - Vec3::from(b.body.linear_velocity)).length(),
            );
        }
        for ((_, a), (_, b)) in now.bodies.iter().zip(&reference[t].bodies) {
            dpos = dpos.max((a.position - b.position).length());
        }
        assert!(
            !sleep_mismatch(now, &reference[t]),
            "sleep flag at tick {t}"
        );
    });
    eprintln!("new contact onset after a rewind: position {dpos:e} m, velocity {dvel:e} m/s");
    assert!(dpos <= POSITION_BOUND_AFTER_ONSET, "{dpos:e}");
    assert!(dvel <= VELOCITY_BOUND_AFTER_ONSET, "{dvel:e}");
}

/// Measured over 140 replayed ticks across the onset (position 3e-5 m,
/// velocity 3.5e-3 m/s), with headroom for a different libm.
const POSITION_BOUND_AFTER_ONSET: f32 = 1e-3;
const VELOCITY_BOUND_AFTER_ONSET: f32 = 2e-2;

#[test]
fn skipping_the_world_restore_makes_the_replay_diverge() {
    // Guards the harness: the contacts and the sleep state are what the
    // full restore is for; without them the same replay is off by far
    // more than the tolerance.
    let (_s, reference) = reference(TICKS);
    for strategy in [Strategy::ColdContacts, Strategy::StateOnly] {
        let (worst, ..) = rewind_case(&reference, 299, 300, 40, strategy);
        assert!(
            worst > 100.0 * TOLERANCE,
            "{strategy:?} should diverge, worst {worst:e}"
        );
    }
    let (_, _, sleep_ticks) = rewind_case(&reference, 100, 120, 40, Strategy::StateOnly);
    assert!(sleep_ticks > 0, "a rewritten sleeping prop wakes up");
}

#[test]
fn a_prop_woken_since_the_capture_goes_back_to_sleep_exactly() {
    let (_s, reference) = reference(TICKS);
    let (mut s, history) = subject(260);
    assert!(
        !history[260].bodies[0].1.sleeping,
        "the prop was woken by the trailer rig"
    );
    let report = restore_world(s.app.world_mut(), &history[190]);
    assert!(report.refused.is_empty() && report.missing.is_empty());
    // Replay one step: the prop is asleep again and exactly where the
    // reference had it.
    replay(&mut s, 190, 191, |t, now| {
        assert_eq!(deviation(now, &reference[t]).0, 0.0);
        assert!(now.bodies[0].1.sleeping);
    });
    assert!(s.app.world().get::<Sleeping>(s.prop).is_some());
}

#[test]
fn restore_leaves_avians_change_tick_and_the_cars_awake() {
    let (mut s, history) = subject(250);
    let before = s.app.world().resource::<LastPhysicsTick>().0;
    restore_world(s.app.world_mut(), &history[200]);
    let after = s.app.world().resource::<LastPhysicsTick>().0;
    assert_eq!(
        before.get(),
        after.get(),
        "LastPhysicsTick is a Bevy change-detection tick, never a simulation tick"
    );
    for car in [s.a, s.b, s.trailer] {
        let w = s.app.world();
        assert!(w.get::<SleepingDisabled>(car).is_some());
        assert!(w.get::<Sleeping>(car).is_none());
    }
    replay(&mut s, 200, 260, |_, now| {
        // Cars never sleep, whatever the prop does.
        assert_eq!(now.vehicles.len(), 3);
    });
    for car in [s.a, s.b, s.trailer] {
        assert!(s.app.world().get::<Sleeping>(car).is_none());
    }
}

#[test]
fn child_colliders_and_the_sensor_survive_a_restore() {
    let (_s, reference) = reference(TICKS);
    let (mut s, history) = subject(250);
    let children: Vec<Entity> = {
        let w = s.app.world_mut();
        let mut q = w.query::<(Entity, &ChildOf, &Collider)>();
        q.iter(w)
            .filter(|(_, p, _)| p.parent() == s.a)
            .map(|(e, ..)| e)
            .collect()
    };
    assert_eq!(children.len(), 2);
    restore_world(s.app.world_mut(), &history[236]);
    replay(&mut s, 236, 300, |t, now| {
        assert!(deviation(now, &reference[t]).0 <= TOLERANCE);
    });
    for c in children {
        assert!(s.app.world().get::<Collider>(c).is_some());
    }
}

#[test]
fn bodies_spawned_or_despawned_since_the_capture() {
    let (_s, reference) = reference(TICKS);
    // Spawned since: a dynamic cube dropped onto car A near the wall.
    let (mut s, history) = subject(250);
    let extra = s
        .app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(0.5, 0.5, 0.5),
            Mass(20.0),
            Position(Vec3::new(0.0, 3.0, -2.0)),
            Transform::from_xyz(0.0, 3.0, -2.0),
        ))
        .id();
    s.app.update();
    s.app.update();
    let report = restore_world(s.app.world_mut(), &history[250]);
    assert_eq!(report.uncaptured, vec![extra], "{report:?}");
    // The replay layer owns membership: it removes the body, and the
    // continuation matches the reference.
    s.app.world_mut().despawn(extra);
    // The two extra updates above ran two steps; those belong to the
    // abandoned future and the restore undid them.
    replay(&mut s, 250, 300, |t, now| {
        assert!(deviation(now, &reference[t]).0 <= TOLERANCE, "tick {t}");
    });

    // Despawned since: a captured prop that no longer exists is reported
    // and skipped, the rest still restores.
    let (mut s, history) = subject(250);
    s.app.world_mut().despawn(s.prop);
    s.app.update();
    let report = restore_world(s.app.world_mut(), &history[250]);
    assert_eq!(report.missing, vec![s.prop], "{report:?}");
    assert!(report.refused.is_empty());
    for (e, v) in &history[250].vehicles {
        let now = mm2_vehicle::capture::capture_vehicle(s.app.world(), *e, EPOCH).unwrap();
        assert_eq!(now.deviation(v).map(|d| d.max_abs), Ok(0.0));
    }
}

#[test]
fn the_report_counts_restored_and_invalidated_contacts() {
    let (mut s, history) = subject(300);
    // The pinned car's wall contact is among the captured pairs.
    assert!(
        history[299]
            .pairs
            .iter()
            .any(|(k, p)| (k.0 == s.wall || k.1 == s.wall) && p.touching())
    );
    let report = restore_world(s.app.world_mut(), &history[299]);
    assert!(report.pairs_restored >= 2, "{report:?}");
    // A pair created after the capture has no captured manifolds.
    let (mut s, history) = subject(300);
    let report = restore_world(s.app.world_mut(), &history[100]);
    assert!(report.pairs_invalidated >= 1, "{report:?}");
}
