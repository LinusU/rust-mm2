//! NETPLAY-07D: the own-car correction smoothing measured at 30, 60 and
//! 144 FPS render rates (and unequal caps between two perspectives) on a
//! repeatable route, plus how many ticks and how much wall time lie
//! between a sampled input and the step and frame that carry it.
//!
//! A *host* world and a *client* world each hold one car on the production
//! step and are driven by the same [`Route`]. At fixed host ticks the host
//! car takes a velocity and spin kick the client could not predict (a
//! contact with something it cannot see). The client reconciles against the
//! host's state `2 * DELAY` ticks late exactly as `reconcile_own` does and
//! feeds the body shift into a [`CorrectionOffset`]. Render frames run on a
//! wall clock of their own (`1 / fps` apart, independent of the 60 Hz
//! step), decay the offset by their delta and "draw" the newest tick's pose
//! plus the offset.
//!
//! Evidence level: synthetic integration on the production step. It is not
//! a rendered capture and not a human comfort judgement (OWNER #1249):
//! frame times are the model's, so the latency columns are exact for the
//! scheduling model (input sampled once per frame, consumed by the next
//! fixed tick, drawn by the frame that ran it) and say nothing about driver
//! queueing or display latency. Run with `--nocapture` for the tables
//! recorded in `docs/netcode-redesign.md`.

use super::contact_tests::{EPOCH, GEN, spawn_car, world};
use super::*;
use crate::netprobe::{DT, Route};
use crate::netvisual::{CorrectionOffset, MAX_DISTANCE};

/// One-way delay in ticks (50 ms): snapshots reach the client `2 * DELAY`
/// ticks after the host tick they describe.
const DELAY: u64 = 3;
/// Host ticks at which the host car takes an unpredicted kick.
const KICKS: [u64; 3] = [90, 210, 330];
const TICKS: u64 = 480;

/// What a run measured.
#[derive(Debug, Default)]
struct Measured {
    /// Corrections that replayed.
    corrections: usize,
    /// Largest body shift of one correction (m).
    worst_shift: f32,
    /// Seconds from each correction until the drawn pose met the body.
    settle: Vec<f32>,
    /// Largest distance between drawn pose and body at any frame (m).
    worst_offset: f32,
    /// Largest displacement the *view* takes that the car's motion does
    /// not explain (m): with smoothing, the most one frame's decay moves
    /// the drawn pose; without it, the whole body shift of a correction.
    worst_slip: f32,
    /// Final distance between the client body and the host car (m).
    final_error: f32,
    /// Frames drawn.
    frames: usize,
}

struct Rig {
    host: App,
    client: App,
    host_car: Entity,
    client_car: Entity,
}

impl Rig {
    fn new() -> Self {
        let mut host = world();
        let mut client = world();
        let at = Vec3::Y * 1.2;
        let host_car = spawn_car(&mut host, at, false);
        let client_car = spawn_car(&mut client, at, false);
        host.update();
        client.update();
        Self {
            host,
            client,
            host_car,
            client_car,
        }
    }
}

fn drive(app: &mut App, car: Entity, input: VehicleInput) {
    *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = input;
    app.update();
}

fn pose(app: &App, car: Entity) -> (Vec3, Quat) {
    (
        app.world().get::<Position>(car).unwrap().0,
        app.world().get::<Rotation>(car).unwrap().0,
    )
}

/// Run `route` for [`TICKS`] ticks with the client drawing at `fps`.
/// `smoothing == false` draws the body itself (the pre-NETPLAY-07 view).
fn run(route: Route, fps: f64, smoothing: bool) -> Measured {
    let mut rig = Rig::new();
    let mut pred = OwnPrediction::default();
    pred.restart(GEN);
    let mut host_history: Vec<WorldCapture> = vec![];
    let mut offset = CorrectionOffset::default();
    let mut m = Measured::default();
    let frame_dt = 1.0 / fps;
    let mut next_frame = frame_dt;
    let mut open: Vec<f64> = vec![];

    for t in 1..=TICKS {
        let input = route.input(t - 1);
        drive(&mut rig.host, rig.host_car, input);
        if KICKS.contains(&t) {
            let world = rig.host.world_mut();
            world.get_mut::<LinearVelocity>(rig.host_car).unwrap().0 += Vec3::X * 1.2;
            world.get_mut::<AngularVelocity>(rig.host_car).unwrap().0 += Vec3::Y * 0.5;
        }
        host_history.push(capture_world(rig.host.world_mut(), EPOCH));

        drive(&mut rig.client, rig.client_car, input);
        let cap = capture_world(rig.client.world_mut(), EPOCH);
        pred.record(t, cap);

        // The host state of tick `ack` reaches the client now.
        if t > 2 * DELAY {
            let ack = t - 2 * DELAY;
            let (pos, rot, vel, ang) =
                own_body(&host_history[ack as usize - 1], rig.host_car).expect("host car captured");
            let assertion = OwnAssertion {
                generation: GEN,
                server_tick: ack,
                ack,
                pos,
                rot,
                vel,
                ang,
            };
            let commands: Vec<(u64, VehicleInput)> =
                (ack + 1..=t).map(|k| (k, route.input(k - 1))).collect();
            let before = pose(&rig.client, rig.client_car);
            let outcome = reconcile(
                rig.client.world_mut(),
                &mut pred,
                &Request {
                    own: rig.client_car,
                    assertion: &assertion,
                    newest: t,
                    commands: &commands,
                    remotes: &[],
                    trailers: &[],
                    epoch: EPOCH,
                },
            );
            if let Outcome::Replayed { shift, .. } = outcome {
                let after = pose(&rig.client, rig.client_car);
                if shift > 0.005 {
                    m.corrections += 1;
                    m.worst_shift = m.worst_shift.max(shift);
                    open.push(t as f64 * DT);
                }
                if smoothing {
                    offset.absorb(before, after);
                } else {
                    m.worst_slip = m.worst_slip.max(shift);
                }
            }
        }

        // Frames whose wall time falls inside this tick draw its pose.
        let tick_end = t as f64 * DT;
        while next_frame <= tick_end + 1e-9 {
            let held = offset.translation;
            offset.decay(frame_dt as f32);
            m.frames += 1;
            m.worst_offset = m.worst_offset.max(offset.distance());
            m.worst_slip = m.worst_slip.max((offset.translation - held).length());
            if offset.is_rest() {
                for start in open.drain(..) {
                    m.settle.push((next_frame - start) as f32);
                }
            }
            next_frame += frame_dt;
        }
    }
    let client_now = pose(&rig.client, rig.client_car).0;
    // The client is `DELAY` ticks ahead of what the host has seen; compare
    // against the host's own record of the same tick.
    let lagged = own_body(&host_history[TICKS as usize - 1], rig.host_car)
        .unwrap()
        .0;
    m.final_error = (client_now - lagged).length();
    m
}

fn row(label: &str, m: &Measured) {
    let worst_settle = m.settle.iter().copied().fold(0.0f32, f32::max);
    println!(
        "{label:<28} corr {:>2}  shift {:.3}  settle {:.3}  offset {:.3}  view slip {:.4}  err {:.3}  frames {}",
        m.corrections,
        m.worst_shift,
        worst_settle,
        m.worst_offset,
        m.worst_slip,
        m.final_error,
        m.frames
    );
}

#[test]
fn convergence_and_decay_are_the_same_at_30_60_and_144_fps() {
    for route in [Route::Slalom, Route::Accel] {
        let mut settles = vec![];
        for fps in [30.0, 60.0, 144.0] {
            let m = run(route, fps, true);
            row(&format!("{} {fps} fps smoothed", route.as_str()), &m);
            assert!(
                m.corrections >= KICKS.len(),
                "{route:?}: the kicks were not corrected"
            );
            assert!(m.worst_offset <= MAX_DISTANCE + 1e-4);
            assert_eq!(m.settle.len(), m.corrections, "every correction settled");
            let worst = m.settle.iter().copied().fold(0.0f32, f32::max);
            assert!(worst < 1.0, "{route:?} {fps} fps settled in {worst} s");
            settles.push(worst);
        }
        let (lo, hi) = settles
            .iter()
            .fold((f32::MAX, 0.0f32), |(l, h), &s| (l.min(s), h.max(s)));
        // Tick granularity (1/60 s) plus one 30 fps frame.
        assert!(hi - lo < 0.15, "{route:?} settle spread {lo}..{hi}");
    }
}

#[test]
fn the_body_still_converges_and_the_view_steps_less_than_the_body_does() {
    for fps in [30.0, 60.0, 144.0] {
        let smooth = run(Route::Slalom, fps, true);
        let raw = run(Route::Slalom, fps, false);
        row(&format!("slalom {fps} fps smoothed"), &smooth);
        row(&format!("slalom {fps} fps raw"), &raw);
        // Smoothing is presentation only: the body ends where it would.
        assert!((smooth.final_error - raw.final_error).abs() < 1e-5);
        assert!(smooth.final_error < 0.25, "{}", smooth.final_error);
        assert_eq!(smooth.worst_shift, raw.worst_shift);
        // The drawn pose never jumps by the correction the body took.
        assert!(
            smooth.worst_slip < 0.5 * raw.worst_slip,
            "{fps} fps: smoothed {} raw {}",
            smooth.worst_slip,
            raw.worst_slip
        );
    }
}

#[test]
fn two_perspectives_with_unequal_caps_settle_alike() {
    // Both non-host seats at once: one capped at 30 FPS, one at 144 FPS,
    // each on the slalom with the host's kicks. Neither frame rate may
    // change how long its correction takes or what the body does.
    let slow = run(Route::Slalom, 30.0, true);
    let fast = run(Route::Slalom, 144.0, true);
    row("seat A slalom 30 fps", &slow);
    row("seat B slalom 144 fps", &fast);
    assert_eq!(slow.corrections, fast.corrections);
    assert_eq!(slow.worst_shift, fast.worst_shift);
    assert!((slow.final_error - fast.final_error).abs() < 1e-5);
    let worst = |m: &Measured| m.settle.iter().copied().fold(0.0f32, f32::max);
    assert!((worst(&slow) - worst(&fast)).abs() < 0.15);
}

/// Sampled input to the step that consumes it, and to the frame that draws
/// it, under the production schedule: the input is sampled once per render
/// frame (`PreUpdate`), consumed by the first fixed tick that runs at or
/// after that frame, and the frame that ran the tick draws it. A command
/// is never delayed by the network: the client tick that consumes seq `k`
/// is tick `k` online and offline alike, which `online_consumes...` checks.
fn frame_times(fps: f64, seconds: f64) -> Vec<f64> {
    (1..=(seconds * fps) as usize)
        .map(|i| i as f64 / fps)
        .collect()
}

/// Wall time from an input press at `p` to the frame that steps and
/// submits it. `FixedUpdate` runs before `Update` and rendering inside one
/// frame: the frame at `T` samples input, runs every tick due by `T` with
/// that sample, then submits. The press is consumed by the first frame at
/// or after it that has a tick due. Returns `(input->step, input->render
/// submission)`; in this model they end at the same frame, so the render
/// column adds only the zero-cost submission.
fn press_to_render(fps: f64, press: f64) -> (f64, f64) {
    let frames = frame_times(fps, 2.0);
    let ticks_due = |t: f64| (t / DT + 1e-9).floor();
    let consumed = frames
        .iter()
        .enumerate()
        .find(|&(i, &f)| {
            f >= press
                && ticks_due(f)
                    > if i == 0 {
                        0.0
                    } else {
                        ticks_due(frames[i - 1])
                    }
        })
        .map(|(_, &f)| f)
        .unwrap();
    (consumed - press, consumed - press)
}

#[test]
fn input_to_physics_and_render_latency_by_frame_rate() {
    for fps in [30.0, 60.0, 144.0] {
        let (mut phys, mut render) = (0.0f64, 0.0f64);
        let (mut worst_phys, mut worst_render) = (0.0f64, 0.0f64);
        let n = 600;
        for i in 0..n {
            let press = 0.5 + i as f64 * 0.0007;
            let (p, r) = press_to_render(fps, press);
            phys += p;
            render += r;
            worst_phys = worst_phys.max(p);
            worst_render = worst_render.max(r);
            assert!(r >= p - 1e-9 && p >= 0.0);
        }
        println!(
            "{fps:>5} fps  input->physics mean {:.1} ms worst {:.1} ms   input->render mean {:.1} ms worst {:.1} ms",
            1000.0 * phys / n as f64,
            1000.0 * worst_phys,
            1000.0 * render / n as f64,
            1000.0 * worst_render
        );
        // Never worse than one frame plus one tick of waiting.
        assert!(worst_render <= 1.0 / fps + DT + 1e-6, "{worst_render}");
    }
}

#[test]
fn online_consumes_each_command_on_the_tick_offline_does() {
    // Offline: the car driven by the route alone. Online: the same car with
    // hostile corrections arriving. The first tick on which the route's
    // brake makes the car slow down is the same, and the online car never
    // waits for the network to start responding.
    let response = |online: bool| -> u64 {
        let mut rig = Rig::new();
        let mut pred = OwnPrediction::default();
        pred.restart(GEN);
        let mut speed_before = 0.0;
        let mut onset = 0;
        for t in 1..=600 {
            let input = Route::Slalom.input(t - 1);
            drive(&mut rig.client, rig.client_car, input);
            let cap = capture_world(rig.client.world_mut(), EPOCH);
            pred.record(t, cap);
            let v = rig
                .client
                .world()
                .get::<LinearVelocity>(rig.client_car)
                .unwrap()
                .0
                .length();
            // The route starts braking at tick 480 of its cycle; look for
            // the first deceleration at or after its first brake tick.
            if t > 480 - 1 && onset == 0 && v < speed_before - 1e-4 {
                onset = t;
            }
            speed_before = v;
            if online && t > 2 * DELAY {
                // A correction every tick towards an unchanged host state
                // must be a no-op for response timing.
                let ack = t - 2 * DELAY;
                let (pos, rot, vel, ang) =
                    own_body(pred.ring.get(ack).unwrap(), rig.client_car).unwrap();
                let a = OwnAssertion {
                    generation: GEN,
                    server_tick: ack,
                    ack,
                    pos,
                    rot,
                    vel,
                    ang,
                };
                let commands: Vec<_> = (ack + 1..=t)
                    .map(|k| (k, Route::Slalom.input(k - 1)))
                    .collect();
                reconcile(
                    rig.client.world_mut(),
                    &mut pred,
                    &Request {
                        own: rig.client_car,
                        assertion: &a,
                        newest: t,
                        commands: &commands,
                        remotes: &[],
                        trailers: &[],
                        epoch: EPOCH,
                    },
                );
            }
        }
        onset
    };
    let offline = response(false);
    let online = response(true);
    println!("brake response onset: offline tick {offline}, online tick {online}");
    assert_eq!(offline, online);
}
