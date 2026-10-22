//! NETPLAY-05D: a trailer rig in the predicted contact replay, measured
//! against an uninterrupted zero-delay run.
//!
//! The harness is [`super::multi_tests`] with a jointed trailer behind car
//! 0 (the client's own car). The *host* world drives both cars on the true
//! commands and is the reference. The *client* world drives car 0 on the
//! true commands and car 1 on hold-last commands that are `delay` ticks
//! old, and corrects all three bodies (two cars and the trailer) with
//! [`reconcile`] from host snapshots that arrive `delay` ticks late. Car 1
//! catches the trailer from behind, so the contact under test is a car
//! against a trailer rather than a car against a car. Run with
//! `--nocapture` for the numbers recorded in `docs/netcode-redesign.md`.

use avian3d::prelude::*;
use mm2_net::{Message, RemoteCommand, RemoteCommands, RemoteSeat, Source, TickCommand};
use mm2_vehicle::VehicleConfig;
use mm2_vehicle::replay::MAX_REPLAY_STEPS;

use super::contact_tests::{
    Body, EPOCH, GEN, IMPULSE, PLAYER, body, forward, input, p, q, spawn_car, v, world,
};
use super::*;
use crate::car_visual::Trailer;
use crate::netremote::{Policy, RemotePredictor};

/// Car-space hitch anchors, the way a retail rig sits: the car's behind
/// its origin, the trailer's ahead of its own.
const CAR_HITCH: f32 = 1.9;
const TRAILER_HITCH: f32 = 2.4;
const TICKS: u64 = 420;

/// A car with a jointed trailer behind it, `back` being the world
/// direction away from where the car drives. Returns `(car, trailer,
/// joint)`.
pub(super) fn spawn_rig(app: &mut App, at: Vec3, back: Vec3) -> (Entity, Entity, Entity) {
    let car = spawn_car(app, at, false);
    let rest_offset = back * (CAR_HITCH + TRAILER_HITCH);
    let pos = at + rest_offset;
    let w = app.world_mut();
    let trailer = w
        .spawn((
            Trailer {
                towing: car,
                rest_offset,
            },
            mm2_vehicle::vehicle_bundle(&VehicleConfig {
                trailer: true,
                ..VehicleConfig::default()
            }),
            Position(pos),
            Transform::from_translation(pos),
        ))
        .id();
    let joint = w
        .spawn((
            ChildOf(trailer),
            SphericalJoint::new(car, trailer)
                .with_local_anchor1(back * CAR_HITCH)
                .with_local_anchor2(-back * TRAILER_HITCH)
                .with_swing_limits(-0.6, 0.6)
                .with_twist_limits(-0.15, 0.15),
            JointCollisionDisabled,
        ))
        .id();
    (car, trailer, joint)
}

struct Rig {
    app: App,
    cars: [Entity; 2],
    trailer: Entity,
    joint: Entity,
}

/// Car 0 tows a trailer along the driving axis; car 1 starts behind the
/// rig, a little to one side so it clips the trailer's corner.
fn scene() -> Rig {
    let fwd = forward();
    let side = Vec3::Y.cross(fwd);
    let mut app = world();
    let (a, trailer, joint) = spawn_rig(&mut app, Vec3::Y * 1.2, -fwd);
    let b = spawn_car(&mut app, fwd * -40.0 + side * 0.9 + Vec3::Y * 1.2, false);
    app.update();
    Rig {
        app,
        cars: [a, b],
        trailer,
        joint,
    }
}

/// Car 0 cruises; car 1 floors it and swerves into the trailer late.
fn driver(i: usize, t: u64) -> TickCommand {
    match i {
        0 => q(0.35, 0.0, 0.0),
        _ => q(1.0, 0.0, if (150..170).contains(&t) { -0.1 } else { 0.0 }),
    }
}

fn frame(now: u64) -> Message {
    let first = now.saturating_sub(15).max(1);
    Message::RemoteCommands {
        generation: 1,
        tick: now,
        seats: vec![RemoteSeat {
            player: PLAYER + 1,
            epoch: 0,
            first_tick: first,
            commands: (first..=now)
                .map(|t| RemoteCommand {
                    command: driver(1, t),
                    source: Source::Command,
                })
                .collect(),
        }],
    }
}

/// Set what the live `trailer_input` system would: the trailer brakes with
/// its tractor. The test world does not run it.
fn mirror_brakes(app: &mut App, car: Entity, trailer: Entity) {
    let c = *app.world().get::<VehicleInput>(car).unwrap();
    let mut t = app.world_mut().get_mut::<VehicleInput>(trailer).unwrap();
    t.brake = c.brake;
    t.handbrake = c.handbrake;
}

/// `[car 0, car 1, trailer]` after every tick, with every command true and
/// no delay.
fn host_run() -> Vec<[Body; 3]> {
    let mut s = scene();
    (1..=TICKS)
        .map(|t| {
            for i in 0..2 {
                *s.app
                    .world_mut()
                    .get_mut::<VehicleInput>(s.cars[i])
                    .unwrap() = input(&driver(i, t));
            }
            mirror_brakes(&mut s.app, s.cars[0], s.trailer);
            s.app.update();
            [
                body(&s.app, s.cars[0]),
                body(&s.app, s.cars[1]),
                body(&s.app, s.trailer),
            ]
        })
        .collect()
}

fn assertion(wire: u16, b: &Body) -> RemoteAssertion {
    RemoteAssertion {
        wire,
        epoch: 0,
        pos: p(b),
        rot: Quat::from_array(b.rotation),
        vel: v(b),
        ang: Vec3::from(b.angular_velocity),
    }
}

#[derive(Debug, Default)]
struct Report {
    peak: [f32; 3],
    last: [f32; 3],
    replays: u64,
    recoveries: u64,
    deepest: usize,
    /// Ticks the host trailer changed velocity by more than [`IMPULSE`].
    host_trailer_impulses: u32,
    client_trailer_impulses: u32,
    /// Largest distance between the hitch anchors, a stretched joint being
    /// the symptom of a car and a trailer restored to different ticks.
    peak_hitch_gap: f32,
}

/// The client run. `trailer_partner` includes the trailer in the request;
/// without it the replay restores the trailer from history while the car
/// takes the host's state.
fn run(delay: u64, host: &[[Body; 3]], trailer_partner: bool) -> Report {
    let mut s = scene();
    let [a, b] = s.cars;
    let mut pred = OwnPrediction::default();
    pred.restart(GEN);
    let mut store = RemoteCommands::new();
    store.set_generation(Some(1));
    let mut predictor = RemotePredictor::new(Policy::HoldLast);
    let mut applied: Vec<TickCommand> = vec![q(0.0, 0.0, 0.0)];
    let mut r = Report::default();
    let mut prev_host = None::<Vec3>;
    let mut prev_client = None::<Vec3>;
    let back = -forward();
    for t in 1..=TICKS {
        let ti = t as usize - 1;
        *s.app.world_mut().get_mut::<VehicleInput>(a).unwrap() = input(&driver(0, t));
        if t > delay {
            assert!(store.absorb(&frame(t - delay)));
        }
        let got = predictor.command_for(&mut store, PLAYER + 1, t);
        applied.push(got.command);
        *s.app.world_mut().get_mut::<VehicleInput>(b).unwrap() = input(&got.command);
        mirror_brakes(&mut s.app, a, s.trailer);
        s.app.update();
        pred.ring.record(t, capture_world(s.app.world_mut(), EPOCH));

        if let Some(tt) = t.checked_sub(delay).filter(|&x| x >= 1) {
            let h = &host[tt as usize - 1];
            let own = OwnAssertion {
                generation: GEN,
                server_tick: tt,
                ack: tt,
                pos: p(&h[0]),
                rot: Quat::from_array(h[0].rotation),
                vel: v(&h[0]),
                ang: Vec3::from(h[0].angular_velocity),
            };
            let cmds: Vec<(u64, VehicleInput)> =
                (tt + 1..=t).map(|k| (k, input(&driver(0, k)))).collect();
            let drives = [RemoteDrive {
                body: b,
                assertion: Some(assertion(PLAYER + 1, &h[1])),
                commands: Some(
                    (tt + 1..=t)
                        .map(|k| {
                            input(&if k + delay <= t {
                                driver(1, k)
                            } else {
                                applied[k as usize]
                            })
                        })
                        .collect(),
                ),
            }];
            // The trailer's row is keyed by the towing seat (the own car).
            let trailers = [TrailerDrive {
                body: s.trailer,
                towing: a,
                assertion: Some(assertion(PLAYER, &h[2])),
            }];
            let out = reconcile(
                s.app.world_mut(),
                &mut pred,
                &Request {
                    own: a,
                    assertion: &own,
                    newest: t,
                    commands: &cmds,
                    remotes: &drives,
                    trailers: if trailer_partner { &trailers } else { &[] },
                    epoch: EPOCH,
                },
            );
            if let Outcome::Replayed { steps, .. } = out {
                r.deepest = r.deepest.max(steps);
            }
        }

        let bodies = [body(&s.app, a), body(&s.app, b), body(&s.app, s.trailer)];
        for i in 0..3 {
            let err = p(&bodies[i]).distance(p(&host[ti][i]));
            r.peak[i] = r.peak[i].max(err);
            r.last[i] = err;
        }
        let (hv, cv) = (v(&host[ti][2]), v(&bodies[2]));
        if let Some(pv) = prev_host
            && (hv - pv).length() > IMPULSE
        {
            r.host_trailer_impulses += 1;
        }
        if let Some(pv) = prev_client
            && (cv - pv).length() > IMPULSE
        {
            r.client_trailer_impulses += 1;
        }
        prev_host = Some(hv);
        prev_client = Some(cv);
        let anchor = |b: &Body, local: Vec3| p(b) + Quat::from_array(b.rotation) * local;
        r.peak_hitch_gap = r.peak_hitch_gap.max(
            anchor(&bodies[0], back * CAR_HITCH)
                .distance(anchor(&bodies[2], -back * TRAILER_HITCH)),
        );
    }
    r.replays = pred.replays;
    r.recoveries = pred.recoveries;
    r
}

/// With no delay the client is the reference: all three bodies stay on it,
/// and the rig really is hit (the host trailer takes an impulse).
#[test]
fn a_zero_delay_client_follows_the_reference_through_a_trailer_contact() {
    let host = host_run();
    let r = run(0, &host, true);
    println!("zero delay: {r:?}");
    assert!(
        r.host_trailer_impulses > 0,
        "the scene never hit the trailer: {r:?}"
    );
    assert!(r.peak.iter().all(|&e| e < 0.1), "{r:?}");
    assert_eq!(r.recoveries, 0, "{r:?}");
}

/// Under delay the replay carries the trailer with its tractor: every body
/// converges to the host's, the hitch never stretches, and no recovery path
/// is needed.
#[test]
fn a_delayed_trailer_contact_converges_with_the_trailer_in_the_replay() {
    let host = host_run();
    for delay in [3u64, 6, 12, 30] {
        let r = run(delay, &host, true);
        println!("delay {delay}, trailer in replay: {r:?}");
        assert!(r.deepest <= MAX_REPLAY_STEPS, "{r:?}");
        assert_eq!(r.recoveries, 0, "delay {delay}: {r:?}");
        assert!(r.last.iter().all(|&e| e < 0.5), "delay {delay}: {r:?}");
        assert!(r.peak_hitch_gap < 0.5, "delay {delay}: {r:?}");
    }
}

/// The same run with the trailer left out of the request, printed for the
/// design note: the car takes the host's state while the trailer is
/// restored from history, so every correction leaves the rig inconsistent
/// and the next snapshot has to replay again.
#[test]
fn without_the_trailer_partner_the_rig_needs_more_corrections() {
    let host = host_run();
    let with = run(12, &host, true);
    let without = run(12, &host, false);
    println!("delay 12 with trailer: {with:?}");
    println!("delay 12 without trailer: {without:?}");
    assert!(
        without.replays > with.replays && without.peak[2] > with.peak[2],
        "{with:?} vs {without:?}"
    );
}

/// A trailer attached or detached inside the window cannot be replayed:
/// `HitchChanged` makes `replay_from` refuse, the named recovery adopts the
/// host state for the car and the trailer together, and the history is
/// dropped.
#[test]
fn a_hitch_change_inside_the_window_is_a_named_recovery_for_the_whole_rig() {
    let host = host_run();
    let mut s = scene();
    let [a, b] = s.cars;
    let mut pred = OwnPrediction::default();
    pred.restart(GEN);
    for t in 1..=60u64 {
        *s.app.world_mut().get_mut::<VehicleInput>(a).unwrap() = input(&driver(0, t));
        *s.app.world_mut().get_mut::<VehicleInput>(b).unwrap() = input(&driver(1, t));
        mirror_brakes(&mut s.app, a, s.trailer);
        s.app.update();
        pred.ring.record(t, capture_world(s.app.world_mut(), EPOCH));
    }
    // The trailer is unhooked at tick 60, after the compared boundary.
    s.app.world_mut().despawn(s.joint);
    let ack = 40u64;
    let mut h = host[ack as usize - 1];
    // A host that disagrees, so the comparison asks for a replay.
    h[0].position[0] += 0.5;
    h[2].position[0] += 0.5;
    let own = OwnAssertion {
        generation: GEN,
        server_tick: ack,
        ack,
        pos: p(&h[0]),
        rot: Quat::from_array(h[0].rotation),
        vel: v(&h[0]),
        ang: Vec3::from(h[0].angular_velocity),
    };
    let cmds: Vec<(u64, VehicleInput)> =
        (ack + 1..=60).map(|k| (k, input(&driver(0, k)))).collect();
    let drives = [RemoteDrive {
        body: b,
        assertion: Some(assertion(PLAYER + 1, &h[1])),
        commands: Some((ack + 1..=60).map(|k| input(&driver(1, k))).collect()),
    }];
    let trailers = [TrailerDrive {
        body: s.trailer,
        towing: a,
        assertion: Some(assertion(PLAYER, &h[2])),
    }];
    let out = reconcile(
        s.app.world_mut(),
        &mut pred,
        &Request {
            own: a,
            assertion: &own,
            newest: 60,
            commands: &cmds,
            remotes: &drives,
            trailers: &trailers,
            epoch: EPOCH,
        },
    );
    assert_eq!(out, Outcome::Recovered(RecoveryCause::ReplayRefused));
    let pos = |e| s.app.world().get::<Position>(e).unwrap().0;
    assert_eq!(pos(a), own.pos);
    assert_eq!(pos(s.trailer), p(&h[2]));
    assert_eq!(pos(b), p(&h[1]));
    assert!(pred.ring.is_empty());
}
