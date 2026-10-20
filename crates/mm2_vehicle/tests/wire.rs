//! NETPLAY-03D: an authoritative snapshot encoded at host tick T, decoded
//! and applied to a *different* ECS world — another entity layout, a car
//! history that diverged, contacts that disagree — must converge when both
//! sides then run the same authoritative inputs. A local clone proves
//! nothing here: the receiver shares no `Entity` with the host.
//!
//! Scene: an original-model car driving into a wall and pinning there, a
//! generic car towing a jointed trailer into a sleep-capable prop (see
//! `physics_world.rs`), production `VehiclePlugin` at 60 Hz.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_vehicle::vehicle::VehicleInput;
use mm2_vehicle::wire::{
    AuthoritativeState, MAX_PROPS, PropSlot, VehicleSlot, WireError, apply_authoritative,
    capture_authoritative,
};
use mm2_vehicle::{
    OriginalContactMaterial, VehicleConfig, VehiclePlugin, VehicleProbeLayers, vehicle_bundle,
};

mod common;
use common::{Model, config};

const GENERATION: u32 = 7;
/// Largest float difference accepted after convergence when no contact is
/// in play, in the field's own unit (m, m/s, rad, N). Measured; see
/// `docs/netcode-redesign.md`.
const TOLERANCE: f32 = 1e-3;
/// Bounds once car-wall / car-prop contacts were cold-started by the apply
/// (measured 5.1e-3 m, 9.6e-3 m/s, 64 N; about 2x headroom, and the force
/// scalars are the solver-order noise of 03B's new-contact-onset row).
const POSITION_BOUND_COLD_CONTACTS: f32 = 1e-2;
const VELOCITY_BOUND_COLD_CONTACTS: f32 = 2e-2;
const FORCE_BOUND_COLD_CONTACTS: f32 = 130.0;
const FWD: f32 = -1.0;
const WALL_Z: f32 = FWD * 5.0;
const PROP_Z: f32 = FWD * 14.0;

const CAR_A: VehicleSlot = VehicleSlot(0);
const CAR_B: VehicleSlot = VehicleSlot(1);
const TRAILER: VehicleSlot = VehicleSlot(2);
const PROP: PropSlot = PropSlot(0);

#[derive(Clone, Copy, PartialEq)]
enum Layout {
    /// Spawn order a, b, trailer, prop.
    Host,
    /// Junk entities first, then the reverse order, so no `Entity` of the
    /// host is the same thing here.
    Receiver,
}

struct Scene {
    app: App,
    a: Entity,
    b: Entity,
    prop: Entity,
}

fn scene(layout: Layout) -> Scene {
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
    if layout == Layout::Receiver {
        for _ in 0..37 {
            let junk = w.spawn_empty().id();
            w.despawn(junk);
        }
        for _ in 0..11 {
            w.spawn_empty();
        }
    }
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
        Position(Vec3::new(0.0, 2.0, WALL_Z)),
        Transform::from_xyz(0.0, 2.0, WALL_Z),
    ));
    let b_pos = Vec3::new(40.0, 1.2, 0.0);
    let hitch = Vec3::new(0.0, 0.3, -FWD * 2.2);
    let trailer_pos = b_pos + hitch * 2.0;
    let prop_pos = Vec3::new(40.0, 0.5, PROP_Z);
    let spawn_a = |w: &mut World| {
        w.spawn((
            vehicle_bundle(&config(Model::Original)),
            CAR_A,
            Position(Vec3::new(0.0, 1.2, 2.0)),
            Transform::from_xyz(0.0, 1.2, 2.0),
        ))
        .id()
    };
    let spawn_b = |w: &mut World| {
        w.spawn((
            vehicle_bundle(&config(Model::Generic)),
            CAR_B,
            Position(b_pos),
            Transform::from_translation(b_pos),
        ))
        .id()
    };
    let spawn_trailer = |w: &mut World| {
        w.spawn((
            vehicle_bundle(&VehicleConfig::default()),
            TRAILER,
            Position(trailer_pos),
            Transform::from_translation(trailer_pos),
        ))
        .id()
    };
    let spawn_prop = |w: &mut World| {
        w.spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            Mass(60.0),
            PROP,
            Position(prop_pos),
            Transform::from_translation(prop_pos),
        ))
        .id()
    };
    let (a, b, trailer, prop);
    match layout {
        Layout::Host => {
            a = spawn_a(w);
            b = spawn_b(w);
            trailer = spawn_trailer(w);
            prop = spawn_prop(w);
        }
        Layout::Receiver => {
            prop = spawn_prop(w);
            trailer = spawn_trailer(w);
            b = spawn_b(w);
            a = spawn_a(w);
        }
    }
    w.spawn((
        ChildOf(trailer),
        SphericalJoint::new(b, trailer)
            .with_local_anchor1(hitch)
            .with_local_anchor2(-hitch)
            .with_swing_limits(-0.6, 0.6)
            .with_twist_limits(-0.15, 0.15),
        JointCollisionDisabled,
    ));
    Scene { app, a, b, prop }
}

fn cmd(throttle: f32, brake: f32, steering: f32, handbrake: f32) -> VehicleInput {
    VehicleInput {
        throttle,
        brake,
        steering,
        handbrake,
        forced_gear: None,
    }
}

/// The authority's commands for (A, B) at tick `t`. Pure in `t`.
fn host_commands(t: usize) -> (VehicleInput, VehicleInput) {
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

/// What the receiver did with its own cars before the snapshot arrived: a
/// different history, so different contacts, tyre state and prop state.
fn receiver_commands(t: usize) -> (VehicleInput, VehicleInput) {
    let a = match t {
        0..60 => cmd(1.0, 0.0, 0.3, 0.0),
        60..120 => cmd(0.0, 0.0, -0.4, 1.0),
        _ => cmd(0.6, 0.0, 0.0, 0.0),
    };
    let b = match t {
        0..30 => cmd(0.2, 0.0, 0.0, 0.0),
        _ => cmd(1.0, 0.0, 0.1, 0.0),
    };
    (a, b)
}

fn step(s: &mut Scene, (a, b): (VehicleInput, VehicleInput)) {
    *s.app.world_mut().get_mut::<VehicleInput>(s.a).unwrap() = a;
    *s.app.world_mut().get_mut::<VehicleInput>(s.b).unwrap() = b;
    s.app.update();
}

fn running(
    layout: Layout,
    ticks: usize,
    commands: fn(usize) -> (VehicleInput, VehicleInput),
) -> Scene {
    let mut s = scene(layout);
    s.app.update(); // the first update has no elapsed time: no step
    for t in 0..ticks {
        step(&mut s, commands(t));
    }
    s
}

fn snapshot(s: &mut Scene, tick: usize) -> AuthoritativeState {
    capture_authoritative(s.app.world_mut(), GENERATION, &|_| 1, tick as u64).unwrap()
}

/// Encode and decode through a lossless text format, as the wire would.
fn wire(state: &AuthoritativeState) -> (Vec<u8>, AuthoritativeState) {
    let bytes = serde_json::to_vec(state).unwrap();
    let decoded: AuthoritativeState = serde_json::from_slice(&bytes).unwrap();
    (bytes, decoded)
}

/// Largest float deviation of two snapshots of the same slots; infinity on
/// a discrete mismatch.
fn deviation(a: &AuthoritativeState, b: &AuthoritativeState) -> (f32, String) {
    assert_eq!(a.vehicles.len(), b.vehicles.len());
    assert_eq!(a.props.len(), b.props.len());
    let mut worst = (0.0f32, String::from("-"));
    let mut note = |d: f32, what: String| {
        if d > worst.0 || d.is_nan() {
            worst = (if d.is_nan() { f32::INFINITY } else { d }, what);
        }
    };
    for ((sa, va), (sb, vb)) in a.vehicles.iter().zip(&b.vehicles) {
        assert_eq!(sa, sb);
        match va.deviation(vb) {
            Ok(d) => note(d.max_abs, format!("{sa:?} {}", d.field)),
            Err(f) => note(f32::INFINITY, format!("{sa:?} discrete {f}")),
        }
    }
    for ((sa, pa), (_, pb)) in a.props.iter().zip(&b.props) {
        assert_eq!(pa.sleeping, pb.sleeping, "{sa:?} sleep flag");
        let diff = |x: &[f32], y: &[f32]| {
            x.iter()
                .zip(y)
                .fold(0.0f32, |m, (p, q)| m.max((p - q).abs()))
        };
        let d = diff(&pa.position, &pb.position)
            .max(diff(&pa.linear_velocity, &pb.linear_velocity))
            .max(diff(&pa.angular_velocity, &pb.angular_velocity))
            .max(diff(&pa.rotation, &pb.rotation));
        note(d, format!("{sa:?} body"));
    }
    worst
}

fn touching_pairs(s: &Scene) -> usize {
    s.app
        .world()
        .resource::<ContactGraph>()
        .iter_active_touching()
        .count()
}

/// Host to tick `t`, snapshot, wire, apply to a receiver that ran its own
/// different history, then both run the host's commands for `tail` ticks.
/// Returns the worst deviation over every tick and the encoded size.
fn round_trip(t: usize, tail: usize) -> Convergence {
    let mut host = running(Layout::Host, t, host_commands);
    let mut peer = running(Layout::Receiver, t + 25, receiver_commands);
    assert_ne!(host.a, peer.a, "the layouts must not share entity ids");
    assert_ne!(host.prop, peer.prop);

    let sent = snapshot(&mut host, t);
    let (bytes, received) = wire(&sent);
    assert_eq!(received, sent, "the DTO survives the wire exactly");

    // Before: the receiver really disagrees with the host.
    let before = snapshot(&mut peer, t);
    assert!(
        deviation(&sent, &before).0 > 1.0,
        "the receiver's own history must have diverged"
    );
    let peer_contacts = touching_pairs(&peer);

    let report =
        apply_authoritative(peer.app.world_mut(), &received, GENERATION, &|_| Some(1)).unwrap();
    assert!(report.unknown_vehicles.is_empty() && report.unknown_props.is_empty());
    assert!(report.unmentioned_vehicles.is_empty() && report.refused.is_empty());
    assert!(report.world.uncaptured.is_empty() && report.world.missing.is_empty());
    assert_eq!(
        report.world.pairs_restored, 0,
        "no contact graph crosses the wire"
    );
    if peer_contacts > 0 {
        assert!(report.world.pairs_invalidated > 0);
    }

    // Applied state equals what was sent, field for field.
    let after = snapshot(&mut peer, t);
    assert_eq!(deviation(&sent, &after).0, 0.0, "apply puts the state back");

    let mut worst = Convergence {
        bytes: bytes.len(),
        ..Convergence::default()
    };
    for k in t..t + tail {
        step(&mut host, host_commands(k));
        step(&mut peer, host_commands(k));
        let (h, p) = (snapshot(&mut host, k + 1), snapshot(&mut peer, k + 1));
        let (d, what) = deviation(&h, &p);
        if d > worst.any.0 {
            worst.any = (d, format!("tick {}: {what}", k + 1));
        }
        let (pos, vel) = pose_deviation(&h, &p);
        worst.position = worst.position.max(pos);
        worst.velocity = worst.velocity.max(vel);
    }
    worst
}

#[derive(Default)]
struct Convergence {
    /// Worst float of any field, with where.
    any: (f32, String),
    /// Worst body position difference (m) over cars and props.
    position: f32,
    /// Worst linear velocity difference (m/s) over cars and props.
    velocity: f32,
    bytes: usize,
}

fn pose_deviation(a: &AuthoritativeState, b: &AuthoritativeState) -> (f32, f32) {
    let (mut pos, mut vel) = (0.0f32, 0.0f32);
    for ((_, x), (_, y)) in a.vehicles.iter().zip(&b.vehicles) {
        pos = pos.max((Vec3::from(x.body.position) - Vec3::from(y.body.position)).length());
        vel = vel.max(
            (Vec3::from(x.body.linear_velocity) - Vec3::from(y.body.linear_velocity)).length(),
        );
    }
    for ((_, x), (_, y)) in a.props.iter().zip(&b.props) {
        pos = pos.max((Vec3::from(x.position) - Vec3::from(y.position)).length());
        vel = vel.max((Vec3::from(x.linear_velocity) - Vec3::from(y.linear_velocity)).length());
    }
    (pos, vel)
}

#[test]
fn a_decoded_snapshot_converges_in_a_different_world() {
    // 120: nothing touches but the ground, the prop asleep: bit-exact.
    let c = round_trip(120, 60);
    eprintln!("T=120: any {:e}, {} bytes JSON", c.any.0, c.bytes);
    assert!(c.any.0 <= TOLERANCE, "{} {}", c.any.0, c.any.1);

    // 236: car A at the wall and the rig reaching the prop; 300: A pinned.
    // The wire carries no contact graph, so the receiver cold-starts every
    // pair and the impacts are bounded rather than exact.
    for t in [236, 300] {
        let c = round_trip(t, 60);
        eprintln!(
            "T={t}: any {:e} ({}), position {:e} m, velocity {:e} m/s, {} bytes JSON",
            c.any.0, c.any.1, c.position, c.velocity, c.bytes
        );
        assert!(c.position <= POSITION_BOUND_COLD_CONTACTS, "T={t}");
        assert!(c.velocity <= VELOCITY_BOUND_COLD_CONTACTS, "T={t}");
        assert!(c.any.0 <= FORCE_BOUND_COLD_CONTACTS, "T={t}: {}", c.any.1);
    }
}

#[test]
fn applying_nothing_leaves_the_worlds_apart() {
    let t = 236;
    let mut host = running(Layout::Host, t, host_commands);
    let mut peer = running(Layout::Receiver, t + 25, receiver_commands);
    for k in t..t + 10 {
        step(&mut host, host_commands(k));
        step(&mut peer, host_commands(k));
    }
    let (d, _) = deviation(&snapshot(&mut host, t + 10), &snapshot(&mut peer, t + 10));
    assert!(d > 1.0, "without the snapshot the worlds stay apart: {d}");
}

#[test]
fn a_stale_generation_or_reset_epoch_is_refused_whole() {
    let mut host = running(Layout::Host, 100, host_commands);
    let mut peer = running(Layout::Receiver, 100, receiver_commands);
    let state = snapshot(&mut host, 100);
    let before = snapshot(&mut peer, 100);

    // The peer already applied reset epoch 2 for car B.
    let err = apply_authoritative(peer.app.world_mut(), &state, GENERATION, &|s| {
        Some(if s == CAR_B { 2 } else { 1 })
    })
    .unwrap_err();
    assert!(
        matches!(err, WireError::StaleResetEpoch { slot, got: 1, expected: 2 } if slot == CAR_B)
    );

    // A snapshot from another session generation.
    assert!(matches!(
        apply_authoritative(peer.app.world_mut(), &state, GENERATION + 1, &|_| None),
        Err(WireError::StaleGeneration {
            got: 7,
            expected: 8
        })
    ));

    // A vehicle stamped with another generation than the snapshot.
    let mut mixed = state.clone();
    mixed.vehicles[0].1.epoch.generation += 1;
    assert!(matches!(
        apply_authoritative(peer.app.world_mut(), &mixed, GENERATION, &|_| None),
        Err(WireError::MixedGeneration { .. })
    ));
    assert_eq!(snapshot(&mut peer, 100), before, "nothing was written");
}

#[test]
fn corrupt_snapshots_are_rejected_before_any_write() {
    let mut host = running(Layout::Host, 100, host_commands);
    let mut peer = running(Layout::Receiver, 100, receiver_commands);
    let good = snapshot(&mut host, 100);
    let before = snapshot(&mut peer, 100);
    let apply = |peer: &mut Scene, s: &AuthoritativeState| {
        apply_authoritative(peer.app.world_mut(), s, GENERATION, &|_| None)
    };

    let mut nan = good.clone();
    nan.props[0].1.position[1] = f32::NAN;
    assert!(matches!(
        apply(&mut peer, &nan),
        Err(WireError::NonFinite(_))
    ));

    let mut nan_vehicle = good.clone();
    nan_vehicle.vehicles[0].1.body.position[0] = f32::INFINITY;
    assert!(matches!(
        apply(&mut peer, &nan_vehicle),
        Err(WireError::NonFinite(_))
    ));

    let mut dup = good.clone();
    dup.vehicles.push(dup.vehicles[0].clone());
    assert!(matches!(
        apply(&mut peer, &dup),
        Err(WireError::DuplicateSlot(_))
    ));

    let mut many = good.clone();
    many.props = (0..=MAX_PROPS as u16)
        .map(|i| (PropSlot(i), good.props[0].1))
        .collect();
    assert!(matches!(
        apply(&mut peer, &many),
        Err(WireError::TooMany { what: "props", .. })
    ));
    assert_eq!(snapshot(&mut peer, 100), before, "nothing was written");
}

#[test]
fn unknown_and_unmentioned_slots_are_reported_not_guessed() {
    let mut host = running(Layout::Host, 100, host_commands);
    let mut peer = running(Layout::Receiver, 100, receiver_commands);
    let mut state = snapshot(&mut host, 100);
    state.vehicles.retain(|(s, _)| *s != TRAILER);
    let mut extra = state.vehicles[0].1.clone();
    extra.body.position[0] += 1.0;
    state.vehicles.push((VehicleSlot(9), extra));
    state.props.push((PropSlot(9), state.props[0].1));
    let report = apply_authoritative(peer.app.world_mut(), &state, GENERATION, &|_| None).unwrap();
    assert_eq!(report.unknown_vehicles, vec![VehicleSlot(9)]);
    assert_eq!(report.unknown_props, vec![PropSlot(9)]);
    assert_eq!(report.unmentioned_vehicles, vec![TRAILER]);
}

#[test]
fn a_vehicle_the_peers_cannot_name_is_an_error() {
    let mut s = running(Layout::Host, 5, host_commands);
    let (a, prop) = (s.a, s.prop);
    s.app.world_mut().entity_mut(a).remove::<VehicleSlot>();
    assert_eq!(
        capture_authoritative(s.app.world_mut(), GENERATION, &|_| 1, 5),
        Err(WireError::UnslottedVehicle(a))
    );
    s.app.world_mut().entity_mut(a).insert(CAR_A);
    s.app.world_mut().entity_mut(prop).remove::<PropSlot>();
    assert_eq!(
        capture_authoritative(s.app.world_mut(), GENERATION, &|_| 1, 5),
        Err(WireError::UnslottedProp(prop))
    );
}

#[test]
fn a_sleeping_prop_stays_asleep_across_the_wire() {
    let t = 100;
    let mut host = running(Layout::Host, t, host_commands);
    let mut peer = running(Layout::Receiver, t, receiver_commands);
    let state = snapshot(&mut host, t);
    assert!(state.props[0].1.sleeping, "the prop is asleep at T");
    let prop = peer.prop;
    avian3d::dynamics::solver::islands::WakeBody(prop)
        .apply(peer.app.world_mut())
        .unwrap();
    assert!(peer.app.world().get::<Sleeping>(prop).is_none());
    apply_authoritative(peer.app.world_mut(), &state, GENERATION, &|_| None).unwrap();
    assert!(peer.app.world().get::<Sleeping>(peer.prop).is_some());
}
