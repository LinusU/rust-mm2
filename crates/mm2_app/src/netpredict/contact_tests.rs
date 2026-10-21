//! NETPLAY-05A-5: two-car contact on the production step, measured against
//! an uninterrupted zero-delay run.
//!
//! The *host* world drives both cars with the true per-tick commands and is
//! the reference. The *client* world drives its own car with the same true
//! commands, but the other human's body only from commands that are
//! `delay` ticks old (hold-last, [`RemotePredictor`]), and corrects both
//! bodies with [`reconcile`] from host snapshots that also arrive `delay`
//! ticks late. Everything is in-process and deterministic; a dropped
//! snapshot is a seeded draw, standing in for the lossy proxy (the sockets
//! themselves are covered by the `network` suite, not here). Run with
//! `--nocapture` for the tables recorded in `docs/netcode-redesign.md`.

use std::time::{Duration, Instant};

use avian3d::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_net::{Message, RemoteCommand, RemoteCommands, RemoteSeat, Source, TickCommand};
use mm2_vehicle::capture::{BodyCapture, capture_vehicle};
use mm2_vehicle::{VehicleConfig, VehiclePlugin, VehicleProbeLayers, vehicle_bundle};

use super::*;
use crate::netremote::{Policy, RemotePredictor};

const EPOCH: EpochRef = EpochRef {
    generation: 1,
    reset_epoch: 0,
};
const GEN: u64 = 1;
const PLAYER: u16 = 2;
/// A one-tick velocity change (m/s) that no pedal can make: an impulse.
const IMPULSE: f32 = 0.6;
/// Position error (m) below which a car counts as converged.
const CONVERGED: f32 = 0.1;

/// Contact starts between the two cars, as the live world raised them
/// (replayed ticks are quarantined, so a replay raises none).
#[derive(Resource, Default)]
struct Pair {
    cars: Option<(Entity, Entity)>,
    starts: u32,
}

fn count_starts(mut reader: MessageReader<CollisionStart>, mut pair: ResMut<Pair>) {
    for m in reader.read() {
        let Some((a, b)) = pair.cars else { continue };
        let ends = (m.body1, m.body2);
        if ends == (Some(a), Some(b)) || ends == (Some(b), Some(a)) {
            pair.starts += 1;
        }
    }
}

fn world() -> App {
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
        .init_resource::<Pair>()
        .add_systems(Update, count_starts);
    app.finish();
    app.cleanup();
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(2000.0, 1.0, 2000.0),
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    app
}

fn spawn_car(app: &mut App, at: Vec3, flip: bool) -> Entity {
    let rot = if flip {
        Quat::from_rotation_y(std::f32::consts::PI)
    } else {
        Quat::IDENTITY
    };
    app.world_mut()
        .spawn((
            vehicle_bundle(&VehicleConfig::default()),
            Position(at),
            Rotation(rot),
            Transform::from_translation(at).with_rotation(rot),
        ))
        .id()
}

fn q(throttle: f32, brake: f32, steer: f32) -> TickCommand {
    TickCommand {
        throttle: (throttle * 255.0) as u8,
        brake: (brake * 255.0) as u8,
        steer: (steer * 127.0) as i8,
        handbrake: 0,
        auto_reverse: true,
        forced_gear: None,
    }
}

/// The unit direction a car at identity rotation drives in (measured, not
/// assumed), and the horizontal direction to its right.
fn forward() -> Vec3 {
    let mut app = world();
    let car = spawn_car(&mut app, Vec3::new(0.0, 1.2, 0.0), false);
    app.update();
    for _ in 0..45 {
        *app.world_mut().get_mut::<VehicleInput>(car).unwrap() =
            decode_tick_command(&q(1.0, 0.0, 0.0));
        app.update();
    }
    let mut v = app.world().get::<LinearVelocity>(car).unwrap().0;
    v.y = 0.0;
    assert!(v.length() > 1.0, "the probe car did not move: {v}");
    v.normalize()
}

/// One scripted duel: where each car starts (along the driving axis and
/// sideways) and what each driver does at host tick `t` (1-based).
struct Scn {
    name: &'static str,
    a_at: f32,
    b_at: f32,
    b_side: f32,
    b_flip: bool,
    ticks: u64,
    a: Box<dyn Fn(u64) -> TickCommand>,
    b: Box<dyn Fn(u64) -> TickCommand>,
}

fn scn(
    name: &'static str,
    (a_at, b_at, b_side, b_flip): (f32, f32, f32, bool),
    a: impl Fn(u64) -> TickCommand + 'static,
    b: impl Fn(u64) -> TickCommand + 'static,
) -> Scn {
    Scn {
        name,
        a_at,
        b_at,
        b_side,
        b_flip,
        ticks: 420,
        a: Box::new(a),
        b: Box::new(b),
    }
}

/// Head-on, B steering at `steer` from tick `from` (0: never), lateral
/// offset `side`.
fn head_on(name: &'static str, side: f32, from: u64, steer: f32) -> Scn {
    scn(
        name,
        (-45.0, 45.0, side, true),
        |_| q(1.0, 0.0, 0.0),
        move |t| q(1.0, 0.0, if from != 0 && t >= from { steer } else { 0.0 }),
    )
}

fn scenarios() -> Vec<Scn> {
    vec![
        head_on("head-on", 0.0, 0, 0.0),
        scn(
            "rear-end, lead brakes",
            (-30.0, 0.0, 0.0, false),
            |_| q(1.0, 0.0, 0.0),
            |t| {
                if t <= 150 {
                    q(0.8, 0.0, 0.0)
                } else {
                    q(0.0, 1.0, 0.0)
                }
            },
        ),
        scn(
            "push a stationary car",
            (-40.0, 0.0, 0.0, false),
            |_| q(1.0, 0.0, 0.0),
            |_| q(0.0, 0.0, 0.0),
        ),
        scn(
            "pushed while stationary",
            (0.0, -40.0, 0.0, false),
            |_| q(0.0, 0.0, 0.0),
            |_| q(1.0, 0.0, 0.0),
        ),
    ]
}

/// One car's host state at one tick.
type Body = BodyCapture;

fn body(app: &App, e: Entity) -> Body {
    capture_vehicle(app.world(), e, EPOCH).unwrap().body
}

fn p(b: &Body) -> Vec3 {
    Vec3::from(b.position)
}
fn v(b: &Body) -> Vec3 {
    Vec3::from(b.linear_velocity)
}

fn input(c: &TickCommand) -> VehicleInput {
    decode_tick_command(c)
}

fn place(s: &Scn, fwd: Vec3) -> (Vec3, Vec3) {
    let side = Vec3::Y.cross(fwd);
    (
        fwd * s.a_at + Vec3::Y * 1.2,
        fwd * s.b_at + side * s.b_side + Vec3::Y * 1.2,
    )
}

/// The reference: both cars on the true commands, zero delay.
fn host_run(s: &Scn, fwd: Vec3) -> (Vec<[Body; 2]>, u32) {
    let mut app = world();
    let (pa, pb) = place(s, fwd);
    let a = spawn_car(&mut app, pa, false);
    let b = spawn_car(&mut app, pb, s.b_flip);
    app.world_mut().resource_mut::<Pair>().cars = Some((a, b));
    app.update();
    let states = (1..=s.ticks)
        .map(|t| {
            *app.world_mut().get_mut::<VehicleInput>(a).unwrap() = input(&(s.a)(t));
            *app.world_mut().get_mut::<VehicleInput>(b).unwrap() = input(&(s.b)(t));
            app.update();
            [body(&app, a), body(&app, b)]
        })
        .collect();
    (states, app.world().resource::<Pair>().starts)
}

/// The frame the host would send at tick `now`: the seat's last 16 ticks.
fn frame(s: &Scn, now: u64) -> Message {
    let first = now.saturating_sub(15).max(1);
    Message::RemoteCommands {
        generation: 1,
        tick: now,
        seats: vec![RemoteSeat {
            player: PLAYER,
            epoch: 0,
            first_tick: first,
            commands: (first..=now)
                .map(|t| RemoteCommand {
                    command: (s.b)(t),
                    source: Source::Command,
                })
                .collect(),
        }],
    }
}

struct Rng(u64);
impl Rng {
    fn chance(&mut self, p: f64) -> bool {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) < p
    }
}

#[derive(Clone, Copy)]
struct Cfg {
    /// Ticks both commands and snapshots take to reach the client.
    delay: u64,
    /// Chance that a snapshot never arrives.
    loss: f64,
    /// Correct with [`reconcile`] (else the client just runs on).
    reconcile: bool,
    /// The remote body is a kinematic copy that follows the delayed host
    /// pose (the pre-05A-3 client), shoving with infinite mass.
    kinematic: bool,
}

#[derive(Default, Debug)]
struct Report {
    /// First tick of a velocity impulse (host / client), per car.
    host_hit: [Option<u64>; 2],
    client_hit: [Option<u64>; 2],
    /// Contact starts raised by the live world: host, client.
    starts: [u32; 2],
    /// Impulse ticks over the run, per car.
    host_impulses: [u32; 2],
    client_impulses: [u32; 2],
    /// Largest jump of the present pose a correction caused (m), per car.
    max_correction: [f32; 2],
    corrections: u32,
    replays: u64,
    recoveries: u64,
    /// Ticks from the host's first impulse to the last tick the car was
    /// off by more than [`CONVERGED`] (None: never off).
    settle: [Option<u64>; 2],
    /// Largest position error after the host's first impulse (m).
    peak_error: [f32; 2],
    final_error: [f32; 2],
    final_speed_gap: [f32; 2],
    /// Highest speed (m/s) each car reaches over the run: `[host][car]`
    /// and `[client][car]`. A car shoved from rest shows how hard.
    peak_speed: [[f32; 2]; 2],
    replay_ms: Vec<f64>,
}

impl Report {
    fn line(&self) -> String {
        format!(
            "hit h{:?} c{:?} starts {:?} imp h{:?} c{:?} corr {:.2}/{:.2} n{} rep{} rec{} settle {:?} peak {:.2}/{:.2} final {:.2}/{:.2} vgap {:.2}/{:.2} peak_v host {:.1}/{:.1} client {:.1}/{:.1} replay_ms {:.2}",
            self.host_hit,
            self.client_hit,
            self.starts,
            self.host_impulses,
            self.client_impulses,
            self.max_correction[0],
            self.max_correction[1],
            self.corrections,
            self.replays,
            self.recoveries,
            self.settle,
            self.peak_error[0],
            self.peak_error[1],
            self.final_error[0],
            self.final_error[1],
            self.final_speed_gap[0],
            self.final_speed_gap[1],
            self.peak_speed[0][0],
            self.peak_speed[0][1],
            self.peak_speed[1][0],
            self.peak_speed[1][1],
            self.replay_ms.iter().sum::<f64>() / self.replay_ms.len().max(1) as f64,
        )
    }
}

// The per-car arrays (host/client, car 0/1) are indexed in lockstep.
#[allow(clippy::needless_range_loop)]
fn run(s: &Scn, cfg: Cfg, (host, host_starts): &(Vec<[Body; 2]>, u32), fwd: Vec3) -> Report {
    let mut app = world();
    let (pa, pb) = place(s, fwd);
    let a = spawn_car(&mut app, pa, false);
    let b = spawn_car(&mut app, pb, s.b_flip);
    app.world_mut().resource_mut::<Pair>().cars = Some((a, b));
    if cfg.kinematic {
        app.world_mut().entity_mut(b).insert(RigidBody::Kinematic);
    }
    app.update();
    let mut pred = OwnPrediction::default();
    pred.restart(GEN);
    let mut store = RemoteCommands::new();
    store.set_generation(Some(1));
    let mut predictor = RemotePredictor::new(Policy::HoldLast);
    let mut applied: Vec<TickCommand> = vec![q(0.0, 0.0, 0.0)];
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut r = Report::default();
    let mut last_off: [Option<u64>; 2] = [None, None];
    let mut prev_host = [None::<Vec3>; 2];
    let mut prev_client = [None::<Vec3>; 2];
    for t in 1..=s.ticks {
        let ti = t as usize - 1;
        if t > cfg.delay {
            assert!(store.absorb(&frame(s, t - cfg.delay)));
        }
        let got = predictor.command_for(&mut store, PLAYER, t);
        applied.push(got.command);
        *app.world_mut().get_mut::<VehicleInput>(a).unwrap() = input(&(s.a)(t));
        if cfg.kinematic {
            // The delayed copy: the host's pose `delay` ticks ago.
            let at = (t.saturating_sub(cfg.delay).max(1) - 1) as usize;
            let h = &host[at][1];
            let w = app.world_mut();
            w.get_mut::<Position>(b).unwrap().0 = p(h);
            w.get_mut::<Rotation>(b).unwrap().0 = Quat::from_array(h.rotation);
            w.get_mut::<LinearVelocity>(b).unwrap().0 = v(h);
        } else {
            *app.world_mut().get_mut::<VehicleInput>(b).unwrap() = input(&got.command);
        }
        app.update();
        let cap = capture_world(app.world_mut(), EPOCH);
        pred.ring.record(t, cap);

        let snap_at = t.checked_sub(cfg.delay).filter(|&x| x >= 1);
        if let (Some(tt), true) = (snap_at, cfg.reconcile)
            && !rng.chance(cfg.loss)
        {
            let h = &host[tt as usize - 1];
            let own = |bd: &Body| OwnAssertion {
                generation: GEN,
                server_tick: tt,
                ack: tt,
                pos: p(bd),
                rot: Quat::from_array(bd.rotation),
                vel: v(bd),
                ang: Vec3::from(bd.angular_velocity),
            };
            let ra = |bd: &Body| RemoteAssertion {
                wire: PLAYER,
                pos: p(bd),
                rot: Quat::from_array(bd.rotation),
                vel: v(bd),
                ang: Vec3::from(bd.angular_velocity),
            };
            let cmds: Vec<(u64, VehicleInput)> =
                (tt + 1..=t).map(|k| (k, input(&(s.a)(k)))).collect();
            // A command the client has received is the true one; the
            // rest are what the live step applied.
            let remote_cmds: Vec<VehicleInput> = (tt + 1..=t)
                .map(|k| {
                    input(&if k + cfg.delay <= t {
                        (s.b)(k)
                    } else {
                        applied[k as usize]
                    })
                })
                .collect();
            let before = [pos_of(&app, a), pos_of(&app, b)];
            let drive = RemoteDrive {
                body: b,
                assertion: Some(ra(&h[1])),
                commands: Some(remote_cmds),
            };
            let remotes: &[RemoteDrive] = if cfg.kinematic { &[] } else { &[drive] };
            let started = Instant::now();
            let out = reconcile(
                app.world_mut(),
                &mut pred,
                &Request {
                    own: a,
                    assertion: &own(&h[0]),
                    newest: t,
                    commands: &cmds,
                    remotes,
                    epoch: EPOCH,
                },
            );
            if let Outcome::Replayed { .. } = out {
                r.replay_ms.push(started.elapsed().as_secs_f64() * 1e3);
                r.corrections += 1;
                let after = [pos_of(&app, a), pos_of(&app, b)];
                for i in 0..2 {
                    r.max_correction[i] = r.max_correction[i].max(before[i].distance(after[i]));
                }
            }
        }

        // Compare with the reference at the same tick.
        let now = [body(&app, a), body(&app, b)];
        for i in 0..2 {
            let hv = v(&host[ti][i]);
            let cv = v(&now[i]);
            if let Some(pv) = prev_host[i]
                && (hv - pv).length() > IMPULSE
            {
                r.host_impulses[i] += 1;
                r.host_hit[i].get_or_insert(t);
            }
            if let Some(pv) = prev_client[i]
                && (cv - pv).length() > IMPULSE
            {
                r.client_impulses[i] += 1;
                r.client_hit[i].get_or_insert(t);
            }
            prev_host[i] = Some(hv);
            prev_client[i] = Some(cv);
            let err = p(&now[i]).distance(p(&host[ti][i]));
            if r.host_hit.iter().flatten().min().is_some_and(|&h| t >= h) {
                r.peak_error[i] = r.peak_error[i].max(err);
                if err > CONVERGED {
                    last_off[i] = Some(t);
                }
            }
            r.final_error[i] = err;
            r.final_speed_gap[i] = (hv - cv).length();
        }
        for i in 0..2 {
            r.peak_speed[0][i] = r.peak_speed[0][i].max(v(&host[ti][i]).length());
            r.peak_speed[1][i] = r.peak_speed[1][i].max(v(&now[i]).length());
        }
    }
    if let Some(h) = r.host_hit.iter().flatten().min().copied() {
        for i in 0..2 {
            r.settle[i] = last_off[i].map(|l| l + 1 - h);
        }
    }
    r.starts = [*host_starts, app.world().resource::<Pair>().starts];
    r.replays = pred.replays;
    r.recoveries = pred.recoveries;
    r
}

fn pos_of(app: &App, e: Entity) -> Vec3 {
    app.world().get::<Position>(e).unwrap().0
}

fn all_scenarios() -> Vec<Scn> {
    let mut all = scenarios();
    // B swerves out of the way 27 ticks before the contact it would have
    // made; the client learns of it `delay` ticks late.
    all.push(head_on("false contact (opponent evades)", 1.5, 200, 1.0));
    // B starts clear of A's lane and turns into it 17 ticks before the
    // pass.
    all.push(head_on(
        "missed contact (opponent turns in)",
        2.5,
        210,
        -1.0,
    ));
    all
}

const DELAYS: [u64; 4] = [3, 6, 12, 30];

fn cfg(delay: u64) -> Cfg {
    Cfg {
        delay,
        loss: 0.0,
        reconcile: true,
        kinematic: false,
    }
}

fn lag(r: &Report, i: usize) -> Option<i64> {
    Some(r.client_hit[i]? as i64 - r.host_hit[i]? as i64)
}

/// Delay 0 is the uninterrupted reference: the client is the host, its
/// cars stay on the reference, and every contact lands on the same tick.
/// The two worlds are separate simulations, so a brush between the cars
/// (the false-contact pass) may differ by a hair on another machine and
/// cause a tolerance replay; the state must still agree.
#[test]
fn a_zero_delay_client_is_the_reference_in_every_contact() {
    let fwd = forward();
    for s in &all_scenarios() {
        let host = host_run(s, fwd);
        let r = run(s, cfg(0), &host, fwd);
        assert!(
            r.peak_error.iter().chain(&r.final_error).all(|&e| e < 0.05),
            "{}: {}",
            s.name,
            r.line()
        );
        assert!(r.final_speed_gap.iter().all(|&e| e < 0.05), "{}", s.name);
        assert_eq!(r.client_hit, r.host_hit, "{}", s.name);
        assert_eq!(r.client_impulses, r.host_impulses, "{}", s.name);
        if !s.name.starts_with("false") {
            assert_eq!(r.replays, 0, "{}: nothing to correct", s.name);
            assert_eq!(r.starts[0], r.starts[1], "{}", s.name);
        }
    }
}

/// Head-on, rear-end and push: the drivers hold their pedals into the
/// contact, so the held-last body predicts it exactly. At every delay the
/// first local response is the host's tick, each contact is raised once
/// (no duplicate impulse from the replays), and nothing needs correcting.
/// Without reconcile the rear-end shunt lands late and the cars end metres
/// apart from the reference, so the correction is what holds them together.
#[test]
fn predictable_contacts_land_once_on_the_hosts_tick() {
    let fwd = forward();
    for s in &scenarios() {
        let host = host_run(s, fwd);
        for d in DELAYS {
            let r = run(s, cfg(d), &host, fwd);
            println!("{} d{d}: {}", s.name, r.line());
            assert_eq!(r.client_hit, r.host_hit, "{} d{d}: first response", s.name);
            assert_eq!(
                r.client_impulses, r.host_impulses,
                "{} d{d}: duplicate impulse",
                s.name
            );
            assert_eq!(r.starts[0], r.starts[1], "{} d{d}: contact events", s.name);
            assert!(
                r.max_correction.iter().all(|&c| c < 0.2),
                "{} d{d}: {}",
                s.name,
                r.line()
            );
            assert!(r.final_error.iter().all(|&e| e < 0.05), "{} d{d}", s.name);
            assert!(
                r.final_speed_gap.iter().all(|&e| e < 0.05),
                "{} d{d}",
                s.name
            );
            assert_eq!(r.recoveries, 0);
        }
    }
    let host = host_run(&scenarios()[1], fwd);
    let off = run(
        &scenarios()[1],
        Cfg {
            reconcile: false,
            ..cfg(6)
        },
        &host,
        fwd,
    );
    assert!(off.final_error.iter().all(|&e| e > 1.0), "{}", off.line());
}

/// The opponent evades after the client has already predicted the hit: the
/// contact is false for as long as the command is in flight. Inside the
/// lead time it never happens; past it the client shoves for a moment and
/// the correction is metres, but both cars end on the host's state.
/// (`starts` is the brush the host's cars do make as they pass.)
#[test]
fn a_false_predicted_contact_is_corrected() {
    let fwd = forward();
    let s = head_on("false", 1.5, 200, 1.0);
    let host = host_run(&s, fwd);
    // The host's cars only brush (no velocity impulse): `starts` counts
    // that touch, so it is compared per delay, not against zero.
    let mut last_correction = 0.0;
    for d in DELAYS {
        let r = run(&s, cfg(d), &host, fwd);
        println!("false contact d{d}: {}", r.line());
        assert_eq!(r.host_hit, [None, None]);
        assert!(
            r.final_error.iter().all(|&e| e < 0.05),
            "d{d}: {}",
            r.line()
        );
        assert!(r.final_speed_gap.iter().all(|&e| e < 0.05), "d{d}");
        if d <= 12 {
            assert_eq!(r.client_hit, [None, None], "d{d}: the news arrives in time");
            assert_eq!(r.starts[1], r.starts[0], "d{d}");
            assert!(r.client_impulses == [0, 0], "d{d}");
            assert!(r.max_correction[1] < 0.1, "d{d}");
        } else {
            assert!(r.client_hit.iter().all(Option::is_some), "d{d}: false hit");
            assert!(
                r.client_impulses.iter().all(|&n| n > 0),
                "d{d}: false shove"
            );
            assert!(
                r.max_correction.iter().all(|&c| c > 1.0),
                "d{d}: {}",
                r.line()
            );
            assert!(r.max_correction[0] > last_correction);
        }
        last_correction = r.max_correction[0];
    }
    // Uncorrected, the false hit is permanent.
    let off = run(
        &s,
        Cfg {
            reconcile: false,
            ..cfg(30)
        },
        &host,
        fwd,
    );
    assert!(off.final_error.iter().all(|&e| e > 10.0), "{}", off.line());
}

/// The opponent turns in after the client predicted a clean pass: the
/// first local response is late by up to the delay, the correction grows
/// with the delay, and both cars converge on the host afterwards.
#[test]
fn a_missed_predicted_contact_is_corrected() {
    let fwd = forward();
    let s = head_on("missed", 2.5, 210, -1.0);
    let host = host_run(&s, fwd);
    assert!(host.1 > 0, "the host touches");
    let mut last = (0i64, 0.0f32);
    for d in DELAYS {
        let r = run(&s, cfg(d), &host, fwd);
        println!("missed contact d{d}: {}", r.line());
        for i in 0..2 {
            let l = lag(&r, i).unwrap_or_else(|| panic!("d{d}: car {i} never touched"));
            assert!((0..=d as i64).contains(&l), "d{d}: late by {l}");
            assert!(l >= last.0, "d{d}: lateness grows with the delay");
            assert!(r.settle[i].is_none_or(|n| n <= 2 * d), "d{d}: {}", r.line());
        }
        assert!(r.max_correction[0] >= last.1, "d{d}");
        // Found here: a contact the client only learns of by replay is
        // quarantined, so the live world raises none (`starts` [2, 0] at
        // d12/d30). See docs/netcode-redesign.md, NETPLAY-05A-5.
        assert!(
            r.final_error.iter().all(|&e| e < 0.05),
            "d{d}: {}",
            r.line()
        );
        assert!(r.final_speed_gap.iter().all(|&e| e < 0.05), "d{d}");
        assert_eq!(r.recoveries, 0);
        last = (lag(&r, 0).unwrap(), r.max_correction[0]);
    }
    // Inside the lead time (17 ticks) nothing is missed at all.
    let r = run(&s, cfg(6), &host, fwd);
    assert_eq!(r.client_hit, r.host_hit);
    // Not corrected, the pass is never contact.
    let off = run(
        &s,
        Cfg {
            reconcile: false,
            ..cfg(6)
        },
        &host,
        fwd,
    );
    assert_eq!(off.client_hit, [None, None], "{}", off.line());
}

/// Dropped snapshots (seeded 40 %) leave the client on older assertions, so
/// replays are deeper, but every contact still ends on the reference and
/// is raised once.
#[test]
fn contacts_survive_dropped_snapshots() {
    let fwd = forward();
    for s in &all_scenarios() {
        let host = host_run(s, fwd);
        for d in [6, 30] {
            let lossy = Cfg {
                loss: 0.4,
                ..cfg(d)
            };
            let r = run(s, lossy, &host, fwd);
            println!("{} d{d} loss 40%: {}", s.name, r.line());
            assert!(
                r.final_error.iter().all(|&e| e < 0.05),
                "{} d{d}: {}",
                s.name,
                r.line()
            );
            assert!(
                r.final_speed_gap.iter().all(|&e| e < 0.05),
                "{} d{d}",
                s.name
            );
            assert_eq!(r.recoveries, 0, "{} d{d}", s.name);
            if d == 6 && !s.name.starts_with("false") && !s.name.starts_with("missed") {
                assert_eq!(
                    r.starts[0], r.starts[1],
                    "{} d{d}: duplicate contact",
                    s.name
                );
            }
        }
    }
}

/// The pre-05A-3 client held the remote human car as a kinematic copy,
/// which shoves with infinite mass. A stationary car hit by the copy
/// leaves faster than the host sent it; the dynamic contact body matches
/// the host.
#[test]
fn an_infinite_mass_copy_shoves_harder_than_the_host() {
    let fwd = forward();
    let s = &scenarios()[3];
    let host = host_run(s, fwd);
    let dynamic = run(s, cfg(6), &host, fwd);
    let kinematic = run(
        s,
        Cfg {
            kinematic: true,
            reconcile: false,
            ..cfg(6)
        },
        &host,
        fwd,
    );
    let (h, d, k) = (
        dynamic.peak_speed[0][0],
        dynamic.peak_speed[1][0],
        kinematic.peak_speed[1][0],
    );
    println!(
        "shoved car peak speed: host {h:.2}, dynamic body {d:.2}, kinematic copy {k:.2} m/s ({:.2}x)",
        k / h
    );
    assert!((d - h).abs() < 0.05, "dynamic body {d} vs host {h}");
    assert!(k > 1.1 * h, "kinematic {k} vs host {h}");
    assert!(kinematic.final_error[0] > 1.0, "{}", kinematic.line());
}

/// Two-car costs, printed for `docs/netcode-redesign.md`. Wall-clock
/// figures are machine-dependent and asserted only loosely; the wire sizes
/// are exact.
#[test]
fn two_car_costs_are_recorded() {
    let fwd = forward();
    // Per-tick cost of the physics step with one and with two cars.
    let tick_ms = |two: bool| {
        let mut app = world();
        let a = spawn_car(&mut app, Vec3::new(0.0, 1.2, 0.0), false);
        let b = two.then(|| spawn_car(&mut app, Vec3::new(40.0, 1.2, 0.0), false));
        app.update();
        let mut times = Vec::new();
        for _ in 0..240 {
            *app.world_mut().get_mut::<VehicleInput>(a).unwrap() = input(&q(1.0, 0.0, 0.0));
            if let Some(b) = b {
                *app.world_mut().get_mut::<VehicleInput>(b).unwrap() = input(&q(1.0, 0.0, 0.0));
            }
            let t = Instant::now();
            app.update();
            times.push(t.elapsed().as_secs_f64() * 1e3);
        }
        times.sort_by(f64::total_cmp);
        times[times.len() / 2]
    };
    let (one, two) = (tick_ms(false), tick_ms(true));
    println!("physics tick (median): 1 car {one:.3} ms, 2 cars {two:.3} ms");

    // Capture cost and replay cost by depth, with two cars in contact.
    let s = &scenarios()[1];
    let host = host_run(s, fwd);
    let mut app = world();
    let (pa, pb) = place(s, fwd);
    let a = spawn_car(&mut app, pa, false);
    let b = spawn_car(&mut app, pb, false);
    app.update();
    let t = Instant::now();
    for _ in 0..200 {
        std::hint::black_box(capture_world(app.world_mut(), EPOCH));
    }
    let capture_ms = t.elapsed().as_secs_f64() * 1e3 / 200.0;
    println!("capture_world, 2 cars: {capture_ms:.4} ms");
    let _ = (a, b);
    for d in DELAYS {
        let r = run(s, cfg(d), &host, fwd);
        let n = r.replay_ms.len().max(1) as f64;
        let mean = r.replay_ms.iter().sum::<f64>() / n;
        println!(
            "reconcile+replay depth {d}: mean {mean:.2} ms over {} replays (max {:.2} ms)",
            r.replay_ms.len(),
            r.replay_ms.iter().cloned().fold(0.0, f64::max)
        );
        assert!(r.replay_ms.iter().all(|&m| m < 1000.0));
    }

    // Wire: bytes per tick for the second car.
    let entry = |player: u16, tick: u64| mm2_net::SnapEntry {
        player,
        pos: [tick as f32 * 0.3, 0.5, 1.0],
        rot: [0.0, 0.7, 0.0, 0.7],
        vel: [18.0, 0.0, 6.0],
        angvel: [0.0, 0.02, 0.0],
        prog_ticks: tick,
        input_ack: tick,
        rpm: 3000,
        ..mm2_net::SnapEntry::default()
    };
    let snap = |n: u16| {
        Message::Snap {
            generation: 1,
            tick: 1000,
            entries: (0..n).map(|i| entry(i, 1000)).collect(),
            trailers: Vec::new(),
            impacts: Vec::new(),
            race: None,
        }
        .encode()
        .unwrap()
        .len()
    };
    let cmds = |n: usize| {
        Message::RemoteCommands {
            generation: 1,
            tick: 1000,
            seats: vec![RemoteSeat {
                player: PLAYER,
                epoch: 0,
                first_tick: 985,
                commands: (0..n)
                    .map(|_| RemoteCommand {
                        command: q(1.0, 0.0, 0.3),
                        source: Source::Command,
                    })
                    .collect(),
            }],
        }
        .encode()
        .unwrap()
        .len()
    };
    let (s1, s2, c16) = (snap(1), snap(2), cmds(16));
    println!(
        "wire: Snap 1 car {s1} B, 2 cars {s2} B (+{} B for the second); RemoteCommands frame, 1 seat x 16 ticks {c16} B ({:.0} B/s at 60 Hz, {:.0} B/s with the snapshot's second car)",
        s2 - s1,
        c16 as f64 * 60.0,
        (s2 - s1) as f64 * 60.0 + c16 as f64 * 60.0,
    );
    assert!(s2 > s1 && s2 < 2 * s1);
    assert!(c16 < 200);
}
