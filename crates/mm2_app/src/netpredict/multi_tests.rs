//! NETPLAY-05B: contact between three or more human cars on the
//! production step, measured against an uninterrupted zero-delay run.
//!
//! The harness is [`super::contact_tests`] generalized to `n` cars. The
//! *host* world drives every car on the true commands and is the
//! reference. The *client* world drives car 0 (its own) on the true
//! commands and every other car only from hold-last commands that are
//! `cmd_delay[seat]` ticks old (unequal per seat: unequal pings), and
//! corrects all bodies together with [`reconcile`] from host snapshots that
//! arrive `snap_delay` ticks late. In-process and deterministic; a dropped
//! snapshot is a seeded draw. Trailers are in
//! [`super::trailer_tests`]; world actors and the real transport are not
//! covered here. Run with `--nocapture` for the tables
//! recorded in `docs/netcode-redesign.md`.

use std::time::Instant;

use avian3d::prelude::*;
use mm2_net::{Message, RemoteCommand, RemoteCommands, RemoteSeat, Source, TickCommand};
use mm2_vehicle::capture::capture_vehicle;
use mm2_vehicle::replay::MAX_REPLAY_STEPS;

use super::contact_tests::{
    Body, CONVERGED, EPOCH, GEN, IMPULSE, PLAYER, Rng, body, forward, input, p, q, spawn_car, v,
    world,
};
use super::*;
use crate::netremote::{Policy, RemotePredictor};

type Driver = Box<dyn Fn(u64) -> TickCommand>;

/// One scripted scene: where each car starts and what each driver does at
/// host tick `t` (1-based). Car 0 is the client's own.
struct Scene {
    name: &'static str,
    cars: Vec<(Vec3, Quat)>,
    /// A static wall (centre, rotation) across the path, if any.
    wall: Option<(Vec3, Quat)>,
    drivers: Vec<Driver>,
    ticks: u64,
    /// Authority resets: at host tick `.0` (before that tick's step) car
    /// `.1` is put at rest at `.2`, `.3`, and its reset epoch advances.
    resets: Vec<(u64, usize, Vec3, Quat)>,
}

impl Scene {
    /// The reset epoch the host reports for car `i` at its tick `t`.
    fn epoch_at(&self, i: usize, t: u64) -> u8 {
        self.resets.iter().filter(|r| r.1 == i && r.0 <= t).count() as u8
    }
}

/// A point `along` the driving axis and `side` to its right, at ride height.
fn at(fwd: Vec3, along: f32, side: f32) -> Vec3 {
    fwd * along + Vec3::Y.cross(fwd) * side + Vec3::Y * 1.2
}

fn facing(fwd: Vec3, dir: Vec3) -> Quat {
    Quat::from_rotation_arc(fwd, dir.normalize())
}

fn spawn(app: &mut App, s: &Scene) -> Vec<Entity> {
    if let Some((c, r)) = s.wall {
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(40.0, 6.0, 1.0),
            Position(c),
            Rotation(r),
            Transform::from_translation(c).with_rotation(r),
        ));
    }
    let cars: Vec<Entity> = s
        .cars
        .iter()
        .map(|(pos, rot)| {
            let e = spawn_car(app, *pos, false);
            app.world_mut().entity_mut(e).insert((
                Rotation(*rot),
                Transform::from_translation(*pos).with_rotation(*rot),
            ));
            e
        })
        .collect();
    for (i, e) in cars.iter().enumerate().skip(1) {
        app.world_mut().entity_mut(*e).insert(PredictedRemoteBody {
            wire: PLAYER + i as u16,
            visual: Entity::PLACEHOLDER,
            epoch: 0,
        });
    }
    app.update();
    cars
}

/// What the host's authority does to car `i` before its step at tick `t`.
fn apply_resets(app: &mut App, s: &Scene, cars: &[Entity], t: u64) {
    for (_, i, pos, rot) in s.resets.iter().filter(|r| r.0 == t) {
        app.world_mut().entity_mut(cars[*i]).insert((
            Position(*pos),
            Rotation(*rot),
            LinearVelocity::ZERO,
            AngularVelocity::ZERO,
            Teleported,
        ));
    }
}

/// Door rubbing: B drives alongside A and drifts into it from tick 120.
fn side_swipe(fwd: Vec3) -> Scene {
    Scene {
        name: "side swipe (door rubbing)",
        cars: vec![
            (at(fwd, 0.0, 0.0), Quat::IDENTITY),
            (at(fwd, 0.0, 3.2), Quat::IDENTITY),
        ],
        wall: None,
        drivers: vec![
            Box::new(|_| q(0.8, 0.0, 0.0)),
            Box::new(|t| q(0.8, 0.0, if t >= 120 { -0.12 } else { 0.0 })),
        ],
        ticks: 420,
        resets: Vec::new(),
    }
}

/// Sustained pinning: B sits against a wall with its pedal down and A
/// drives into its tail for the rest of the run.
fn pinned_on_a_wall(fwd: Vec3) -> Scene {
    Scene {
        name: "pinned against a wall",
        cars: vec![
            (at(fwd, -30.0, 0.0), Quat::IDENTITY),
            (at(fwd, 15.0, 0.0), Quat::IDENTITY),
        ],
        wall: Some((at(fwd, 40.0, 0.0), Quat::from_rotation_arc(Vec3::Z, fwd))),
        drivers: vec![
            Box::new(|_| q(1.0, 0.0, 0.0)),
            Box::new(|_| q(0.6, 0.0, 0.0)),
        ],
        ticks: 420,
        resets: Vec::new(),
    }
}

/// B stands still between A (from behind) and C (head-on).
fn pileup(fwd: Vec3) -> Scene {
    Scene {
        name: "three-car pileup",
        cars: vec![
            (at(fwd, -40.0, 0.0), Quat::IDENTITY),
            (at(fwd, 0.0, 0.0), Quat::IDENTITY),
            (
                at(fwd, 40.0, 0.5),
                Quat::from_rotation_y(std::f32::consts::PI),
            ),
        ],
        wall: None,
        drivers: vec![
            Box::new(|_| q(1.0, 0.0, 0.0)),
            Box::new(|_| q(0.0, 0.0, 0.0)),
            Box::new(|_| q(1.0, 0.0, 0.0)),
        ],
        ticks: 420,
        resets: Vec::new(),
    }
}

/// `n` cars on a ring, all driving at its centre, with a little steering
/// differences so they do not meet in a perfectly symmetric knot.
fn converge(fwd: Vec3, n: usize) -> Scene {
    let side = Vec3::Y.cross(fwd);
    let radius = 35.0;
    let cars = (0..n)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / n as f32;
            let dir = fwd * a.cos() + side * a.sin();
            (dir * radius + Vec3::Y * 1.2, facing(fwd, -dir))
        })
        .collect();
    let drivers = (0..n)
        .map(|i| {
            let bias = (i as f32 * 0.37).sin() * 0.04;
            Box::new(move |_| q(1.0, 0.0, bias)) as Driver
        })
        .collect();
    Scene {
        name: "converging ring",
        cars,
        wall: None,
        drivers,
        ticks: 300,
        resets: Vec::new(),
    }
}

/// The reference: every car on the true commands, zero delay.
fn host_run(s: &Scene) -> Vec<Vec<Body>> {
    let mut app = world();
    let cars = spawn(&mut app, s);
    (1..=s.ticks)
        .map(|t| {
            apply_resets(&mut app, s, &cars, t);
            for (c, d) in cars.iter().zip(&s.drivers) {
                *app.world_mut().get_mut::<VehicleInput>(*c).unwrap() = input(&d(t));
            }
            app.update();
            cars.iter().map(|c| body(&app, *c)).collect()
        })
        .collect()
}

/// The frame seat `i` (>= 1) would send at its tick `now`: its last 16.
fn frame(s: &Scene, i: usize, now: u64) -> Message {
    let first = now.saturating_sub(15).max(1);
    Message::RemoteCommands {
        generation: 1,
        tick: now,
        seats: vec![RemoteSeat {
            player: PLAYER + i as u16,
            epoch: 0,
            first_tick: first,
            commands: (first..=now)
                .map(|t| RemoteCommand {
                    command: (s.drivers[i])(t),
                    source: Source::Command,
                })
                .collect(),
        }],
    }
}

#[derive(Clone)]
struct Cfg {
    /// Ticks the host's snapshots take to reach the client.
    snap_delay: u64,
    /// Ticks each remote seat's commands take (index 0 is car 1).
    cmd_delay: Vec<u64>,
    /// Chance that a snapshot never arrives.
    loss: f64,
    /// Report every seat at epoch 0, as before the epoch was tracked.
    ignore_epochs: bool,
}

impl Cfg {
    fn even(delay: u64, remotes: usize) -> Self {
        Self {
            snap_delay: delay,
            cmd_delay: vec![delay; remotes],
            loss: 0.0,
            ignore_epochs: false,
        }
    }
}

#[derive(Default, Debug)]
struct Report {
    /// Largest position error against the reference over the run, per car.
    peak_error: Vec<f32>,
    final_error: Vec<f32>,
    final_speed_gap: Vec<f32>,
    /// Impulse ticks over the run (host, client), summed over the cars.
    host_impulses: u32,
    client_impulses: u32,
    /// Ticks from the first tick any car was off to the last tick any car
    /// was off by more than [`CONVERGED`] (None: never off).
    settle: Option<u64>,
    replays: u64,
    recoveries: u64,
    replay_ms: Vec<f64>,
    /// Deepest replay (steps).
    deepest: usize,
    remote_resets: u64,
    stale_remote: u64,
    /// Highest point any car reached above where the host's did (m): a
    /// launch.
    peak_lift: f32,
}

impl Report {
    fn line(&self) -> String {
        let n = self.replay_ms.len().max(1) as f64;
        format!(
            "lift {:.2} resets {} stale {} peak {:.2?} final {:.2?} vgap {:.2?} imp h{} c{} settle {:?} rep{} rec{} deepest {} replay_ms mean {:.2} max {:.2}",
            self.peak_lift,
            self.remote_resets,
            self.stale_remote,
            self.peak_error,
            self.final_error,
            self.final_speed_gap,
            self.host_impulses,
            self.client_impulses,
            self.settle,
            self.replays,
            self.recoveries,
            self.deepest,
            self.replay_ms.iter().sum::<f64>() / n,
            self.replay_ms.iter().cloned().fold(0.0, f64::max),
        )
    }
}

fn run(s: &Scene, cfg: &Cfg, host: &[Vec<Body>]) -> Report {
    let n = s.cars.len();
    assert_eq!(cfg.cmd_delay.len(), n - 1);
    let mut app = world();
    let cars = spawn(&mut app, s);
    let mut pred = OwnPrediction::default();
    pred.restart(GEN);
    let mut store = RemoteCommands::new();
    store.set_generation(Some(1));
    let mut predictor = RemotePredictor::new(Policy::HoldLast);
    // What the live step applied to each remote, indexed by tick.
    let mut applied: Vec<Vec<TickCommand>> = vec![vec![q(0.0, 0.0, 0.0)]; n];
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut r = Report {
        peak_error: vec![0.0; n],
        final_error: vec![0.0; n],
        final_speed_gap: vec![0.0; n],
        ..Report::default()
    };
    let mut first_off = None::<u64>;
    let mut last_off = None::<u64>;
    let mut prev_host: Vec<Option<Vec3>> = vec![None; n];
    let mut prev_client: Vec<Option<Vec3>> = vec![None; n];
    for t in 1..=s.ticks {
        let ti = t as usize - 1;
        *app.world_mut().get_mut::<VehicleInput>(cars[0]).unwrap() = input(&(s.drivers[0])(t));
        for i in 1..n {
            let d = cfg.cmd_delay[i - 1];
            if t > d {
                assert!(store.absorb(&frame(s, i, t - d)));
            }
            let got = predictor.command_for(&mut store, PLAYER + i as u16, t);
            applied[i].push(got.command);
            *app.world_mut().get_mut::<VehicleInput>(cars[i]).unwrap() = input(&got.command);
        }
        app.update();
        pred.ring.record(t, capture_world(app.world_mut(), EPOCH));

        let snap_at = t.checked_sub(cfg.snap_delay).filter(|&x| x >= 1);
        if let Some(tt) = snap_at
            && !rng.chance(cfg.loss)
        {
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
            let cmds: Vec<(u64, VehicleInput)> = (tt + 1..=t)
                .map(|k| (k, input(&(s.drivers[0])(k))))
                .collect();
            let drives: Vec<RemoteDrive> = (1..n)
                .map(|i| RemoteDrive {
                    body: cars[i],
                    assertion: Some(RemoteAssertion {
                        wire: PLAYER + i as u16,
                        epoch: if cfg.ignore_epochs {
                            0
                        } else {
                            s.epoch_at(i, tt)
                        },
                        pos: p(&h[i]),
                        rot: Quat::from_array(h[i].rotation),
                        vel: v(&h[i]),
                        ang: Vec3::from(h[i].angular_velocity),
                    }),
                    // A command the client has received is the true one;
                    // the rest are what the live step applied.
                    commands: Some(
                        (tt + 1..=t)
                            .map(|k| {
                                input(&if k + cfg.cmd_delay[i - 1] <= t {
                                    (s.drivers[i])(k)
                                } else {
                                    applied[i][k as usize]
                                })
                            })
                            .collect(),
                    ),
                })
                .collect();
            let started = Instant::now();
            let out = reconcile(
                app.world_mut(),
                &mut pred,
                &Request {
                    own: cars[0],
                    assertion: &own,
                    newest: t,
                    commands: &cmds,
                    remotes: &drives,
                    trailers: &[],
                    epoch: EPOCH,
                },
            );
            if let Outcome::Replayed { steps, .. } = out {
                r.replay_ms.push(started.elapsed().as_secs_f64() * 1e3);
                r.deepest = r.deepest.max(steps);
            }
        }

        let mut off = false;
        for i in 0..n {
            let now = body(&app, cars[i]);
            let (hv, cv) = (v(&host[ti][i]), v(&now));
            if let Some(pv) = prev_host[i]
                && (hv - pv).length() > IMPULSE
            {
                r.host_impulses += 1;
            }
            if let Some(pv) = prev_client[i]
                && (cv - pv).length() > IMPULSE
            {
                r.client_impulses += 1;
            }
            prev_host[i] = Some(hv);
            prev_client[i] = Some(cv);
            r.peak_lift = r.peak_lift.max(p(&now).y - p(&host[ti][i]).y);
            let err = p(&now).distance(p(&host[ti][i]));
            r.peak_error[i] = r.peak_error[i].max(err);
            r.final_error[i] = err;
            r.final_speed_gap[i] = (hv - cv).length();
            off |= err > CONVERGED;
        }
        if off {
            first_off.get_or_insert(t);
            last_off = Some(t);
        }
    }
    r.settle = first_off.zip(last_off).map(|(f, l)| l + 1 - f);
    r.replays = pred.replays;
    r.recoveries = pred.recoveries;
    r.remote_resets = pred.remote_resets;
    r.stale_remote = pred.stale_remote;
    r
}

fn scenes(fwd: Vec3) -> Vec<Scene> {
    vec![
        side_swipe(fwd),
        pinned_on_a_wall(fwd),
        pileup(fwd),
        converge(fwd, 4),
    ]
}

/// A client with no delay is the reference: the cars stay on it. The two
/// worlds are separate simulations and contact solving is not bit
/// reproducible, so a tolerance is allowed; the state must agree.
#[test]
fn a_zero_delay_client_is_the_reference_with_three_or_more_cars() {
    let fwd = forward();
    for s in &scenes(fwd) {
        let host = host_run(s);
        let r = run(s, &Cfg::even(0, s.cars.len() - 1), &host);
        println!("zero delay, {}: {}", s.name, r.line());
        assert!(
            r.peak_error.iter().all(|&e| e < 0.1) && r.final_error.iter().all(|&e| e < 0.1),
            "{}: {}",
            s.name,
            r.line()
        );
        assert_eq!(r.recoveries, 0, "{}", s.name);
    }
}

/// Under delay every body converges to the host's after the contact and
/// no replay exceeds the budget.
#[test]
fn delayed_multi_car_contacts_converge_within_the_replay_budget() {
    let fwd = forward();
    for s in &scenes(fwd) {
        let host = host_run(s);
        for d in [3u64, 6, 12, 30] {
            let r = run(s, &Cfg::even(d, s.cars.len() - 1), &host);
            println!("delay {d}, {}: {}", s.name, r.line());
            assert!(r.deepest <= MAX_REPLAY_STEPS, "{}", s.name);
            assert_eq!(r.recoveries, 0, "{} delay {d}: {}", s.name, r.line());
            assert!(
                r.final_error.iter().all(|&e| e < 0.5),
                "{} delay {d}: {}",
                s.name,
                r.line()
            );
        }
    }
}

/// Seats with different pings (a fast seat, a slow seat) and 30 % of the
/// snapshots lost.
#[test]
fn unequal_seat_delays_and_lost_snapshots_still_converge() {
    let fwd = forward();
    let s = pileup(fwd);
    let host = host_run(&s);
    let cfg = Cfg {
        snap_delay: 6,
        cmd_delay: vec![3, 24],
        loss: 0.3,
        ignore_epochs: false,
    };
    let r = run(&s, &cfg, &host);
    println!("unequal delays + 30% loss, {}: {}", s.name, r.line());
    assert_eq!(r.recoveries, 0, "{}", r.line());
    assert!(r.final_error.iter().all(|&e| e < 0.5), "{}", r.line());
}

/// CPU, state and bandwidth growth for 2..8 seats. The replay depth is
/// bounded by [`MAX_REPLAY_STEPS`] whatever the seat count; the table is
/// what `docs/netcode-redesign.md` quotes.
#[test]
fn costs_grow_with_seats_up_to_eight() {
    let fwd = forward();
    for n in 2..=8usize {
        let s = converge(fwd, n);
        let host = host_run(&s);

        // Physics tick and capture cost with n cars in play.
        let mut app = world();
        let cars = spawn(&mut app, &s);
        let mut times = Vec::new();
        for t in 1..=60u64 {
            for (c, d) in cars.iter().zip(&s.drivers) {
                *app.world_mut().get_mut::<VehicleInput>(*c).unwrap() = input(&d(t));
            }
            let started = Instant::now();
            app.update();
            times.push(started.elapsed().as_secs_f64() * 1e3);
        }
        times.sort_by(f64::total_cmp);
        let tick_ms = times[times.len() / 2];
        let started = Instant::now();
        for _ in 0..100 {
            std::hint::black_box(capture_world(app.world_mut(), EPOCH));
        }
        let capture_ms = started.elapsed().as_secs_f64() * 1e3 / 100.0;
        let capture_bytes = capture_vehicle(app.world(), cars[0], EPOCH)
            .map(|_| std::mem::size_of::<mm2_vehicle::capture::VehicleCapture>());

        let r = run(&s, &Cfg::even(12, n - 1), &host);
        let mean = r.replay_ms.iter().sum::<f64>() / r.replay_ms.len().max(1) as f64;

        let snap = Message::Snap {
            generation: 1,
            tick: 1000,
            entries: (0..n as u16)
                .map(|i| mm2_net::SnapEntry {
                    player: i,
                    pos: [1.0, 0.5, 1.0],
                    rot: [0.0, 0.7, 0.0, 0.7],
                    vel: [18.0, 0.0, 6.0],
                    angvel: [0.0, 0.02, 0.0],
                    prog_ticks: 1000,
                    input_ack: 1000,
                    rpm: 3000,
                    ..mm2_net::SnapEntry::default()
                })
                .collect(),
            trailers: Vec::new(),
            impacts: Vec::new(),
            race: None,
        }
        .encode()
        .unwrap()
        .len();
        let cmds = Message::RemoteCommands {
            generation: 1,
            tick: 1000,
            seats: (1..n as u16)
                .map(|i| RemoteSeat {
                    player: PLAYER + i,
                    epoch: 0,
                    first_tick: 985,
                    commands: (0..16)
                        .map(|_| RemoteCommand {
                            command: q(1.0, 0.0, 0.3),
                            source: Source::Command,
                        })
                        .collect(),
                })
                .collect(),
        }
        .encode()
        .unwrap()
        .len();
        println!(
            "{n} seats: tick {tick_ms:.3} ms, capture {capture_ms:.4} ms ({capture_bytes:?} B/car), replay depth<={} mean {mean:.2} ms max {:.2} ms, Snap {snap} B, RemoteCommands 16 ticks {cmds} B ({:.1} kB/s snapshot + commands at 60 Hz)",
            r.deepest,
            r.replay_ms.iter().cloned().fold(0.0, f64::max),
            (snap + cmds) as f64 * 60.0 / 1000.0,
        );
        assert!(r.deepest <= MAX_REPLAY_STEPS);
        assert_eq!(r.recoveries, 0, "{n} seats: {}", r.line());
    }
}

/// A is driving down the road; B stands idle off to the side until the host
/// resets it, at tick 150, onto the road 25 m ahead of A and pointing back
/// at it. A reaches B's new place well after the reset on the host and
/// earlier than that on a client that runs ahead of the host.
fn reset_into_the_path(fwd: Vec3) -> Scene {
    Scene {
        name: "reset into the path",
        cars: vec![
            (at(fwd, -60.0, 0.0), Quat::IDENTITY),
            (at(fwd, 40.0, 30.0), Quat::IDENTITY),
        ],
        wall: None,
        drivers: vec![
            Box::new(|_| q(0.8, 0.0, 0.0)),
            Box::new(|_| q(0.0, 0.0, 0.0)),
        ],
        ticks: 420,
        resets: vec![(150, 1, at(fwd, 10.0, 0.0), facing(fwd, -fwd))],
    }
}

/// The host reset B under A's feet while A was already at the place: a
/// reset that lands inside the predicted window of a client.
#[test]
fn a_remote_reset_into_the_path_is_replayed_not_launched() {
    let fwd = forward();
    let s = reset_into_the_path(fwd);
    let host = host_run(&s);
    for d in [0u64, 3, 12, 30] {
        let r = run(&s, &Cfg::even(d, 1), &host);
        println!("epochs tracked, delay {d}: {}", r.line());
        assert_eq!(r.recoveries, 0, "delay {d}: {}", r.line());
        assert!(r.remote_resets >= 1, "delay {d}: {}", r.line());
        assert!(r.peak_lift < 0.5, "delay {d}: {}", r.line());
        assert!(
            r.final_error.iter().all(|&e| e < 0.5),
            "delay {d}: {}",
            r.line()
        );
    }
}

/// Lost snapshots delay when the reset is first seen, never its effect.
#[test]
fn a_remote_reset_survives_lost_snapshots() {
    let fwd = forward();
    let s = reset_into_the_path(fwd);
    let host = host_run(&s);
    for seed_loss in [0.3, 0.6] {
        let cfg = Cfg {
            snap_delay: 12,
            cmd_delay: vec![12],
            loss: seed_loss,
            ignore_epochs: false,
        };
        let r = run(&s, &cfg, &host);
        println!("epochs tracked, loss {seed_loss}: {}", r.line());
        assert_eq!(r.recoveries, 0, "loss {seed_loss}: {}", r.line());
        assert!(r.remote_resets >= 1, "loss {seed_loss}: {}", r.line());
        assert!(r.peak_lift < 0.5, "loss {seed_loss}: {}", r.line());
        assert!(
            r.final_error.iter().all(|&e| e < 0.5),
            "loss {seed_loss}: {}",
            r.line()
        );
    }
}

/// The same scene as the baseline: seats reported at epoch 0 throughout.
#[test]
fn the_reset_scene_without_epochs_is_measured_for_comparison() {
    let fwd = forward();
    let s = reset_into_the_path(fwd);
    let host = host_run(&s);
    for d in [3u64, 12, 30] {
        let mut cfg = Cfg::even(d, 1);
        cfg.ignore_epochs = true;
        let r = run(&s, &cfg, &host);
        println!("epochs ignored, delay {d}: {}", r.line());
    }
}

// ---------------------------------------------------------------------
// NETPLAY-05G: the local step is never what a replay spends.
// ---------------------------------------------------------------------

/// The own driver of the responsiveness runs: throttle, then a hard turn
/// at [`TURN_AT`].
const TURN_AT: u64 = 170;

fn own_driver(t: u64) -> TickCommand {
    q(0.8, 0.0, if t >= TURN_AT { 0.5 } else { 0.0 })
}

#[derive(Default)]
struct Pace {
    /// The own car's body after each tick's step, index `t - 1`.
    bodies: Vec<Body>,
    outcomes: Vec<Outcome>,
    /// Wall time of the reconcile that ran before each step (0 if none).
    reconcile_ms: Vec<f64>,
    /// Wall time of each step.
    step_ms: Vec<f64>,
}

/// One client in the production order of a fixed tick: `reconcile` between
/// steps (`FixedFirst`), then the live input is written (`FixedUpdate`'s
/// `apply_player_input`), then the step, then the history records it
/// (`FixedLast`). From tick `after` on, every tick gets a host assertion
/// for `newest - depth`, `offset` metres off the recorded state so that the
/// replay is forced every time.
fn paced(depth: Option<u64>, after: u64, offset: f32) -> Pace {
    let mut app = world();
    let car = spawn_car(&mut app, Vec3::Y * 1.2, false);
    app.update();
    let mut pred = OwnPrediction::default();
    pred.restart(GEN);
    let mut out = Pace::default();
    for t in 1..=TURN_AT + 20 {
        let started = Instant::now();
        let mut ran = false;
        if let Some(d) = depth
            && t > after
            && t > d
        {
            let newest = t - 1;
            // The assertion is the recorded state at the ack, moved.
            let ack = newest - d;
            let past = pred.ring.get(ack).map(|c| own_body(c, car));
            let (pos, rot, vel, ang) = match past {
                Some(Some(s)) => s,
                // Evicted from the history (depth past MAX_HISTORY): the
                // host still asserts a state; any will do.
                _ => (Vec3::Y, Quat::IDENTITY, Vec3::ZERO, Vec3::ZERO),
            };
            let assertion = OwnAssertion {
                generation: GEN,
                server_tick: ack,
                ack,
                pos: pos + Vec3::X * offset,
                rot,
                vel,
                ang,
            };
            let cmds: Vec<(u64, VehicleInput)> = (ack + 1..=newest)
                .map(|k| (k, input(&own_driver(k))))
                .collect();
            let outcome = reconcile(
                app.world_mut(),
                &mut pred,
                &Request {
                    own: car,
                    assertion: &assertion,
                    newest,
                    commands: &cmds,
                    remotes: &[],
                    trailers: &[],
                    epoch: EPOCH,
                },
            );
            out.outcomes.push(outcome);
            ran = true;
        }
        out.reconcile_ms.push(if ran {
            started.elapsed().as_secs_f64() * 1e3
        } else {
            0.0
        });

        let live = input(&own_driver(t));
        *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = live;
        let stepped = Instant::now();
        app.update();
        out.step_ms.push(stepped.elapsed().as_secs_f64() * 1e3);
        // The step ran on this tick's command, not one the replay left.
        let used = app.world().get::<VehicleInput>(car).unwrap();
        assert_eq!(
            (used.throttle, used.steering),
            (live.throttle, live.steering),
            "tick {t}"
        );
        pred.ring.record(t, capture_world(app.world_mut(), EPOCH));
        out.bodies.push(body(&app, car));
    }
    out
}

fn yaw_rate(b: &Body) -> f32 {
    b.angular_velocity[1]
}

/// The own car answers a steering change on the very step that carries it,
/// as strongly after a replay of any depth, including the depths that end in
/// a named recovery, as with no replay at all. The replay is synchronous
/// between two steps: it can take frame time, it cannot move the input.
#[test]
fn the_local_step_answers_input_at_every_replay_depth() {
    let base = paced(None, 0, 0.0);
    let turn = TURN_AT as usize - 1;
    let want = yaw_rate(&base.bodies[turn]) - yaw_rate(&base.bodies[turn - 1]);
    assert!(want.abs() > 0.01, "the control run did not turn: {want}");
    println!("no replay: yaw-rate response to the turn {want:.4} rad/s");

    for depth in [
        0u64,
        1,
        30,
        MAX_REPLAY_STEPS as u64,
        MAX_REPLAY_STEPS as u64 + 10,
    ] {
        // Replays on every tick from 125 on, including the turn's.
        let p = paced(Some(depth), 125, 0.5);
        let got = yaw_rate(&p.bodies[turn]) - yaw_rate(&p.bodies[turn - 1]);
        let replays = p
            .outcomes
            .iter()
            .filter(|o| matches!(o, Outcome::Replayed { .. }))
            .count();
        let recoveries = p
            .outcomes
            .iter()
            .filter(|o| matches!(o, Outcome::Recovered(_)))
            .count();
        let worst = |v: &[f64]| v.iter().cloned().fold(0.0, f64::max);
        println!(
            "depth {depth}: {replays} replays, {recoveries} recoveries; turn response {got:.4} rad/s; reconcile max {:.1} ms, step max {:.2} ms (no replay: step max {:.2} ms)",
            worst(&p.reconcile_ms),
            worst(&p.step_ms),
            worst(&base.step_ms),
        );
        if depth <= MAX_REPLAY_STEPS as u64 {
            assert!(replays > 0 && recoveries == 0, "depth {depth}");
        } else {
            assert!(recoveries > 0, "depth {depth}: {:?}", p.outcomes.last());
        }
        assert!(
            got.signum() == want.signum() && (got - want).abs() <= want.abs() * 0.25,
            "depth {depth}: response {got} against {want} with no replay"
        );
    }
}

/// Eight seats, each a car with a jointed trailer, 120 ticks apart from the
/// host: what one maximum replay costs, and what the rest of the tick costs.
#[test]
fn eight_rigs_cost_one_bounded_replay() {
    let fwd = forward();
    let side = Vec3::Y.cross(fwd);
    for n in [2usize, 4, 8] {
        let mut app = world();
        let mut cars = Vec::new();
        let mut trailers = Vec::new();
        for i in 0..n {
            // Side by side, 12 m apart, all driving the same way: no
            // contact, so the cost is the bodies', not the solver's.
            let at = side * (12.0 * i as f32) + Vec3::Y * 1.2;
            let (c, t, _) = super::trailer_tests::spawn_rig(&mut app, at, -fwd);
            cars.push(c);
            trailers.push(t);
        }
        for (i, c) in cars.iter().enumerate().skip(1) {
            app.world_mut().entity_mut(*c).insert(PredictedRemoteBody {
                wire: PLAYER + i as u16,
                visual: Entity::PLACEHOLDER,
                epoch: 0,
            });
        }
        app.update();
        let mut pred = OwnPrediction::default();
        pred.restart(GEN);
        let drive = |i: usize, t: u64| {
            q(
                0.8,
                0.0,
                if i.is_multiple_of(2) { 0.02 } else { -0.02 } * (t % 7) as f32 / 7.0,
            )
        };
        let total = MAX_REPLAY_STEPS as u64 + 10;
        let mut ticks = Vec::new();
        for t in 1..=total {
            for (i, c) in cars.iter().enumerate() {
                *app.world_mut().get_mut::<VehicleInput>(*c).unwrap() = input(&drive(i, t));
            }
            let started = Instant::now();
            app.update();
            ticks.push(started.elapsed().as_secs_f64() * 1e3);
            pred.ring.record(t, capture_world(app.world_mut(), EPOCH));
        }
        ticks.sort_by(f64::total_cmp);
        let tick_ms = ticks[ticks.len() / 2];
        let started = Instant::now();
        for _ in 0..50 {
            std::hint::black_box(capture_world(app.world_mut(), EPOCH));
        }
        let capture_ms = started.elapsed().as_secs_f64() * 1e3 / 50.0;

        let newest = total;
        let ack = newest - MAX_REPLAY_STEPS as u64;
        let past = pred.ring.get(ack).unwrap().clone();
        let state = |e: Entity, shift: f32| {
            let (pos, rot, vel, ang) = own_body(&past, e).unwrap();
            RemoteAssertion {
                wire: PLAYER,
                epoch: 0,
                pos: pos + Vec3::X * shift,
                rot,
                vel,
                ang,
            }
        };
        let (pos, rot, vel, ang) = own_body(&past, cars[0]).unwrap();
        let own = OwnAssertion {
            generation: GEN,
            server_tick: ack,
            ack,
            pos: pos + Vec3::X * 0.5,
            rot,
            vel,
            ang,
        };
        let cmds: Vec<(u64, VehicleInput)> = (ack + 1..=newest)
            .map(|k| (k, input(&drive(0, k))))
            .collect();
        let remotes: Vec<RemoteDrive> = (1..n)
            .map(|i| RemoteDrive {
                body: cars[i],
                assertion: Some(state(cars[i], 0.5)),
                commands: Some((ack + 1..=newest).map(|k| input(&drive(i, k))).collect()),
            })
            .collect();
        let rigs: Vec<TrailerDrive> = (0..n)
            .map(|i| TrailerDrive {
                body: trailers[i],
                towing: cars[i],
                assertion: Some(state(trailers[i], 0.5)),
            })
            .collect();
        let started = Instant::now();
        let out = reconcile(
            app.world_mut(),
            &mut pred,
            &Request {
                own: cars[0],
                assertion: &own,
                newest,
                commands: &cmds,
                remotes: &remotes,
                trailers: &rigs,
                epoch: EPOCH,
            },
        );
        let replay_ms = started.elapsed().as_secs_f64() * 1e3;
        let Outcome::Replayed { steps, .. } = out else {
            panic!("{n} rigs: {out:?}");
        };
        assert_eq!(steps, MAX_REPLAY_STEPS);
        let bytes: usize =
            past.vehicles.len() * std::mem::size_of::<mm2_vehicle::capture::VehicleCapture>();
        println!(
            "{n} rigs ({} bodies): tick {tick_ms:.2} ms, capture {capture_ms:.3} ms, ring of {} captures ~{} kB, max replay ({steps} steps) {replay_ms:.1} ms = {:.1} ticks of frame time",
            2 * n,
            MAX_REPLAY_STEPS + 1,
            bytes * (MAX_REPLAY_STEPS + 1) / 1000,
            replay_ms / (1000.0 / 60.0),
        );
    }
}
