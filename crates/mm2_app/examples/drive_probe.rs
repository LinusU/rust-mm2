//! Drive a real MM2 car headlessly on flat ground and report what it does.
//!
//! [`mm2_vehicle::HandlingMetrics`] answers what a config *implies*; this
//! answers what the car *does* once the simulation has run. It exists for
//! the handling questions closed-form analysis cannot settle — how evenly
//! a car accelerates through its gears, and how much yaw it can actually
//! produce at a given speed.
//!
//! ```sh
//! cargo run -p mm2_app --example drive_probe -- retail vpdb7
//! cargo run -p mm2_app --example drive_probe -- retail            # roster
//! cargo run -p mm2_app --example drive_probe -- retail --controls # launch/brake/reverse/reset
//! cargo run -p mm2_app --example drive_probe -- retail --drop     # level-drop landing leg
//! ```
//!
//! With a city name it instead reproduces the plainest possible bug
//! report — spawn and hold the throttle — over real road geometry, and
//! reports every point where the car's *body* touched the world:
//!
//! ```sh
//! cargo run -p mm2_app --example drive_probe -- retail vppanoz --city sf
//! ```
//!
//! `--city <name> --clearance` instead runs the F02-AC05 spawn leg: settle
//! at the authored spawn, prove the wheels carry the car with the hull
//! clear of the world, `ResetVehicle` back to the spawn and re-check —
//! articulated cars do the same with their trailer spawned through the
//! production [`car_visual::spawn_trailer`] path.
//!
//! ```sh
//! cargo run -p mm2_app --example drive_probe -- retail --city sf --clearance
//! ```
//!
//! `--config <toml>` applies a `--vehicle-config`-style handling override
//! through the same `apply_handling_override` the app uses (F02-AC04);
//! `--dump-config <toml>` writes one car's effective config so an override
//! file can be authored from it.
//!
//! ```sh
//! cargo run -p mm2_app --example drive_probe -- retail vpbug --dump-config /tmp/vpbug.toml
//! cargo run -p mm2_app --example drive_probe -- retail vpbug --config /tmp/vpbug-heavy.toml
//! ```

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::{car_visual, city};
use mm2_assets::{InstallMount, Vfs, mount_install};
use mm2_content::VehicleDef;
use mm2_vehicle::vehicle::{
    DriveDirection, ResetVehicle, Teleported, Vehicle, VehicleInput, VehicleState,
};
use mm2_vehicle::{VehicleConfig, VehiclePlugin, vehicle_bundle};

const HZ: usize = 60;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args.get(1).cloned().unwrap_or_else(|| "retail".into());
    let city = args
        .iter()
        .position(|a| a == "--city")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let controls = args.iter().any(|a| a == "--controls");
    let drop_leg = args.iter().any(|a| a == "--drop");
    let clearance = args.iter().any(|a| a == "--clearance");
    let config_path = args
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let dump_config = args
        .iter()
        .position(|a| a == "--dump-config")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let want = args.get(2).filter(|a| !a.starts_with("--")).cloned();

    let mut vfs = Vfs::new();
    mount_install(&mut vfs, dir.as_ref(), &InstallMount::default()).unwrap();
    let catalog = mm2_content::VehicleCatalog::scan(&vfs);

    if let Some(city) = city {
        // The clearance leg is a per-car matrix; the contact probe stays
        // single-car because it prints a contact timeline, not a table.
        let ids: Vec<String> = match &want {
            Some(q) => vec![catalog.find(q).unwrap().id.clone()],
            None if clearance => catalog
                .entries
                .iter()
                .filter(|e| e.is_ready())
                .map(|e| e.id.clone())
                .collect(),
            None => panic!("--city needs a vehicle id"),
        };
        if clearance {
            println!(
                "{city}: {:<13} {:>9} {:>9} {:>30}   result",
                "id", "spawn", "reset", "trailer",
            );
        }
        for id in &ids {
            let def = match mm2_content::load_vehicle(&vfs, id, 0) {
                Ok(d) => d,
                Err(e) => {
                    println!("{id:<14} load failed: {e}");
                    continue;
                }
            };
            let cfg = effective_config(&def, config_path.as_deref());
            if clearance {
                probe_clearance(&vfs, &city, &def, cfg);
            } else {
                probe_city(&vfs, &city, &cfg);
            }
        }
        return;
    }

    let ids: Vec<String> = match &want {
        Some(q) => vec![catalog.find(q).unwrap().id.clone()],
        None => catalog
            .entries
            .iter()
            .filter(|e| e.is_ready())
            .map(|e| e.id.clone())
            .collect(),
    };

    if let Some(path) = dump_config {
        let id = ids.first().expect("--dump-config needs a vehicle id");
        let def = mm2_content::load_vehicle(&vfs, id, 0).unwrap();
        let cfg = effective_config(&def, config_path.as_deref());
        std::fs::write(&path, cfg.to_toml()).unwrap();
        println!("wrote {}: {}", def.id, path);
        return;
    }

    if drop_leg {
        println!(
            "{:<14} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6}   result",
            "id", "air_s", "sink", "settle", "up", "drive", "finite",
        );
        for id in &ids {
            let def = match mm2_content::load_vehicle(&vfs, id, 0) {
                Ok(d) => d,
                Err(e) => {
                    println!("{id:<14} load failed: {e}");
                    continue;
                }
            };
            let cfg = effective_config(&def, config_path.as_deref());
            let d = probe_drop(&cfg);
            let mut legs: Vec<&str> = Vec::new();
            if d.air_s.is_nan() {
                legs.push("land");
            }
            if d.up < 0.9 {
                legs.push("upright");
            }
            if d.drove_m < 1.0 {
                legs.push("drive");
            }
            if !d.finite {
                legs.push("finite");
            }
            println!(
                "{:<14} {:>6.2} {:>6.1} {:>6.2} {:>6.2} {:>6.1} {:>6}   {}",
                def.id,
                d.air_s,
                d.peak_sink,
                d.settle_s,
                d.up,
                d.drove_m,
                if d.finite { "ok" } else { "FAIL" },
                if legs.is_empty() {
                    "ok".to_string()
                } else {
                    format!("FAIL({})", legs.join(","))
                },
            );
        }
        return;
    }

    if controls {
        println!(
            "{:<14} {:>6} {:>6} {:>6} {:>6} {:>5} {:>6}   result",
            "id", "launch", "stop_s", "dist_m", "rev", "reset", "finite",
        );
        for id in &ids {
            let def = match mm2_content::load_vehicle(&vfs, id, 0) {
                Ok(d) => d,
                Err(e) => {
                    println!("{id:<14} load failed: {e}");
                    continue;
                }
            };
            let c = probe_controls(&effective_config(&def, config_path.as_deref()));
            let mut legs: Vec<&str> = Vec::new();
            if c.launch_speed < LAUNCH_TARGET * 0.8 {
                legs.push("drive");
            }
            if c.brake_time.is_nan() {
                legs.push("stop");
            }
            if !(c.reversed && c.reverse_speed <= -0.5) {
                legs.push("rev");
            }
            if !c.reset_ok {
                legs.push("reset");
            }
            if !c.finite {
                legs.push("finite");
            }
            println!(
                "{:<14} {:>5.1} {:>5.1} {:>6.1} {:>6.1} {:>5} {:>6}   {}",
                def.id,
                c.launch_speed,
                c.brake_time,
                c.brake_distance,
                c.reverse_speed,
                if c.reset_ok { "ok" } else { "FAIL" },
                if c.finite { "ok" } else { "FAIL" },
                if legs.is_empty() {
                    "ok".to_string()
                } else {
                    format!("FAIL({})", legs.join(","))
                },
            );
        }
        return;
    }

    println!(
        "{:<14} {:>7} {:>7} {:>7} {:>8} {:>6}   cornering (g) by speed (m/s)",
        "id", "0-100", "top", "worst", "stall", "drift",
    );
    for id in &ids {
        let def = match mm2_content::load_vehicle(&vfs, id, 0) {
            Ok(d) => d,
            Err(e) => {
                println!("{id:<14} load failed: {e}");
                continue;
            }
        };
        let cfg = effective_config(&def, config_path.as_deref());
        let accel = probe_acceleration(&cfg);
        let corner: Vec<String> = [10.0f32, 20.0, 30.0, 40.0]
            .iter()
            .map(|v| format!("{:.2}@{:.0}", probe_corner_g(&cfg, *v), v))
            .collect();
        println!(
            "{:<14} {:>6.1}s {:>6.1} {:>6.2} {:>7.2}s {:>5.0}°   {}",
            def.id,
            accel.to_100_kmh,
            accel.top_speed,
            accel.worst_interval_ratio,
            accel.longest_stall,
            accel.heading_drift,
            corner.join("  "),
        );
    }
}

/// Resolve the config a probe runs: the authored import, or the
/// `--config` TOML applied through the same
/// [`mm2_content::assemble::apply_handling_override`] the app's
/// `--vehicle-config` uses — wheel positions/radii and collision
/// geometry stay pinned to the imported rig, everything else is the
/// override's. A bad file or mismatched rig is a hard error, matching
/// the app's exit-2 policy.
fn effective_config(def: &VehicleDef, config_path: Option<&str>) -> VehicleConfig {
    match config_path {
        Some(path) => {
            let over = VehicleConfig::load(Path::new(path))
                .unwrap_or_else(|e| panic!("invalid --config: {e}"));
            mm2_content::assemble::apply_handling_override(&def.config, over)
                .unwrap_or_else(|e| panic!("incompatible --config for {}: {e}", def.id))
        }
        None => def.config.clone(),
    }
}

/// Spawn in a real city, hold the throttle, and report every stretch where
/// the chassis touched the world.
///
/// The wheels are raycasts and never collide, so any contact at all is the
/// *body* hitting road, kerb or prop — which is what "it catches on a
/// seam" looks like from inside the simulation.
fn probe_city(vfs: &Vfs, city_name: &str, cfg: &VehicleConfig) {
    let (mut app, car, spawn, yaw) = headless_city(vfs, city_name, cfg.clone());
    println!(
        "{city_name}: spawn ({:.0}, {:.1}, {:.0}) yaw {:.0}°",
        spawn.x,
        spawn.y,
        spawn.z,
        yaw.to_degrees()
    );
    settle(&mut app, car);
    set_input(
        &mut app,
        car,
        VehicleInput {
            throttle: 1.0,
            ..default()
        },
    );

    println!(
        "{:>7} {:>7} {:>7} {:>6} {:>6} {:>5}  position / what it hit",
        "t", "km/h", "lost", "for", "sink", "pitch",
    );
    struct Contact {
        t: f32,
        speed: f32,
        pos: Vec3,
        sink: f32,
        pitch: f32,
        what: String,
    }
    let mut open: Option<Contact> = None;
    let mut contacts = 0usize;
    let mut airborne_until = -1.0f32;
    for frame in 0..HZ * 30 {
        app.update();
        let t = frame as f32 / HZ as f32;
        let world = app.world();
        let state = world.get::<VehicleState>(car).unwrap();
        let speed = state.forward_speed;
        if !state.grounded {
            airborne_until = t;
        }
        let pos = world.get::<Position>(car).unwrap().0;
        let rot = world.get::<Rotation>(car).unwrap().0;
        let touching = world.get::<CollidingEntities>(car).unwrap();
        match (touching.iter().next().copied(), &open) {
            (Some(other), None) => {
                let forward = rot * Vec3::NEG_Z;
                open = Some(Contact {
                    t,
                    speed,
                    pos,
                    sink: world.get::<LinearVelocity>(car).unwrap().0.y,
                    pitch: forward.y.asin().to_degrees(),
                    what: world
                        .get::<Name>(other)
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| format!("{other}")),
                });
            }
            (None, Some(c)) => {
                contacts += 1;
                let flew = if airborne_until > c.t - 0.5 {
                    " (after air)"
                } else {
                    ""
                };
                println!(
                    "{:>6.2}s {:>7.1} {:>6.0}% {:>5.2}s {:>5.1} {:>5.0}°  ({:.0}, {:.1}, {:.0}) {}{}",
                    c.t,
                    c.speed * 3.6,
                    (1.0 - speed / c.speed.max(0.1)) * 100.0,
                    t - c.t,
                    c.sink,
                    c.pitch,
                    c.pos.x,
                    c.pos.y,
                    c.pos.z,
                    c.what,
                    flew,
                );
                open = None;
            }
            _ => {}
        }
    }
    let state = app.world().get::<VehicleState>(car).unwrap();
    println!(
        "\n{contacts} body contact(s) in 30 s; ended at {:.0} km/h",
        state.forward_speed * 3.6
    );
}

/// What full throttle from rest looks like.
struct AccelProbe {
    to_100_kmh: f32,
    /// Highest speed reached, not the speed at the end — a car that spins
    /// or trips over itself would otherwise report its crash as its top
    /// speed.
    top_speed: f32,
    /// Slowest half-second of gain divided by the average — a car that
    /// accelerates evenly is near `1.0`, one that stalls between gears
    /// approaches `0.0`.
    worst_interval_ratio: f32,
    /// Longest continuous stretch gaining under a tenth of the average.
    longest_stall: f32,
    /// Degrees the car wandered off its launch heading. A car that is
    /// spinning is not measuring its gearbox.
    heading_drift: f32,
}

fn probe_acceleration(cfg: &VehicleConfig) -> AccelProbe {
    let (mut app, car) = headless(cfg.clone());
    settle(&mut app, car);

    let seconds = 25usize;
    let mut speeds = Vec::with_capacity(seconds * HZ);
    let mut drift = 0.0f32;
    set_input(
        &mut app,
        car,
        VehicleInput {
            throttle: 1.0,
            ..default()
        },
    );
    for _ in 0..seconds * HZ {
        app.update();
        speeds.push(app.world().get::<VehicleState>(car).unwrap().forward_speed);
        let heading = app.world().get::<Rotation>(car).unwrap().0 * Vec3::NEG_Z;
        drift = drift.max(heading.x.atan2(-heading.z).abs());
    }

    let top = speeds.iter().copied().fold(0.0f32, f32::max);
    let to_100 = speeds
        .iter()
        .position(|s| *s >= 100.0 / 3.6)
        .map(|i| i as f32 / HZ as f32)
        .unwrap_or(f32::NAN);

    // Judge evenness only while the car is still pulling. Once it is near
    // its top speed every interval is small for honest reasons, and
    // including that tail would score a fast car as a stalling one.
    let pulling_until = speeds
        .iter()
        .position(|s| *s >= top * 0.9)
        .unwrap_or(speeds.len());
    let window = HZ / 2;
    let pulling: Vec<f32> = speeds[..pulling_until.max(window + 1)]
        .windows(window + 1)
        .step_by(window)
        .map(|w| w[window] - w[0])
        .collect();
    let mean = if pulling.is_empty() {
        0.0
    } else {
        pulling.iter().sum::<f32>() / pulling.len() as f32
    };
    let worst = pulling.iter().copied().fold(f32::MAX, f32::min);
    let mut stall = 0.0f32;
    let mut run = 0.0f32;
    for g in &pulling {
        if *g < mean * 0.1 {
            run += window as f32 / HZ as f32;
            stall = stall.max(run);
        } else {
            run = 0.0;
        }
    }

    AccelProbe {
        to_100_kmh: to_100,
        top_speed: top,
        worst_interval_ratio: if mean > 0.0 { worst / mean } else { 0.0 },
        longest_stall: stall,
        heading_drift: drift.to_degrees(),
    }
}

/// Steady-state cornering at `speed` under full lock, in g.
///
/// Measured from how fast the *path* bends, not from the body's yaw rate:
/// a car that has broken away spins faster than it corners, so yaw would
/// report a turn the tires are not producing. Compare the result against
/// the car's `grip_g` from `mm2-inspect handling` — that is the ceiling.
fn probe_corner_g(cfg: &VehicleConfig, speed: f32) -> f32 {
    let (mut app, car) = headless(cfg.clone());
    settle(&mut app, car);

    // Launch at the target speed and coast. Driving up to it would measure
    // the engine as much as the steering, and pinning the speed there
    // artificially just feeds a spin — the car pirouettes on the spot and
    // reports cornering no tire could deliver.
    {
        let world = app.world_mut();
        let rot = world.get::<Rotation>(car).unwrap().0;
        world.get_mut::<LinearVelocity>(car).unwrap().0 = rot * Vec3::NEG_Z * speed;
    }
    // Hold the speed with the pedals rather than by rewriting the
    // velocity: the force still goes through the tires, so it competes
    // for grip the way it would under a driver, and nothing injects the
    // energy that lets a broken-away car spin on the spot.
    let hold = |app: &mut App| {
        let speed_now = app.world().get::<VehicleState>(car).unwrap().forward_speed;
        let error = speed - speed_now;
        set_input(
            app,
            car,
            VehicleInput {
                throttle: (error * 0.5).clamp(0.0, 1.0),
                brake: (-error * 0.5).clamp(0.0, 1.0),
                steering: 1.0,
                ..default()
            },
        );
        app.update();
    };
    for _ in 0..HZ * 2 {
        hold(&mut app);
    }

    let sample = |app: &App| {
        let v = app.world().get::<LinearVelocity>(car).unwrap().0;
        (v.x.atan2(v.z), Vec3::new(v.x, 0.0, v.z).length())
    };
    let (mut prev_heading, _) = sample(&app);
    let (mut swept, mut speed_sum) = (0.0f32, 0.0f32);
    let frames = HZ / 2;
    for _ in 0..frames {
        hold(&mut app);
        let (heading, v) = sample(&app);
        let mut d = heading - prev_heading;
        while d > std::f32::consts::PI {
            d -= std::f32::consts::TAU;
        }
        while d < -std::f32::consts::PI {
            d += std::f32::consts::TAU;
        }
        swept += d.abs();
        speed_sum += v;
        prev_heading = heading;
    }
    let turn_rate = swept / (frames as f32 / HZ as f32);
    turn_rate * (speed_sum / frames as f32) / 9.81
}

/// The standing-control check F02-AC02 asks of every stock car: launch,
/// brake to a stop, hold the brake through the standstill into reverse,
/// then reset — each leg through the same `vehicle_bundle` +
/// `VehiclePlugin` systems gameplay runs. The measured numbers are
/// reported rather than hidden behind a verdict, so a regression reads
/// as drift in the table rather than a bare pass/fail flip.
struct ControlProbe {
    /// Forward speed the launch reached, m/s.
    launch_speed: f32,
    /// Seconds of full brake to `|forward_speed| <= STOP_SPEED`; NaN
    /// when the car never stopped inside `BRAKE_CAP`.
    brake_time: f32,
    /// Ground covered while braking, metres; NaN on the same timeout.
    brake_distance: f32,
    /// Deepest signed forward speed while the brake stayed held after
    /// the stop — a negative number is reverse actually engaging.
    reverse_speed: f32,
    /// Whether `DriveDirection::Reverse` latched at any point.
    reversed: bool,
    /// Whether `ResetVehicle` put the car on the requested pose with
    /// motion cleared and `Teleported` stamped.
    reset_ok: bool,
    /// Every sampled speed and position stayed finite.
    finite: bool,
}

/// Speed the launch leg aims for before braking, m/s — comfortably
/// under every stock car's measured top speed (slowest: vpbus, 29.0).
const LAUNCH_TARGET: f32 = 15.0;
/// Frames the launch gets before the check brakes from whatever speed
/// it reached — heavy vehicles need the runway.
const LAUNCH_CAP: usize = HZ * 10;
/// `|forward_speed|` that counts as stopped, m/s.
const STOP_SPEED: f32 = 0.2;
/// Frames of full brake before the stop leg times out.
const BRAKE_CAP: usize = HZ * 12;
/// Frames the brake stays held after the stop for the reverse leg.
const REVERSE_HOLD: usize = HZ * 5;

fn probe_controls(cfg: &VehicleConfig) -> ControlProbe {
    let (mut app, car) = headless(cfg.clone());
    settle(&mut app, car);
    let mut finite = true;
    let mut sample = |app: &App| {
        let s = app.world().get::<VehicleState>(car).unwrap();
        let p = app.world().get::<Position>(car).unwrap().0;
        finite &= s.forward_speed.is_finite() && p.is_finite();
        (s.forward_speed, s.direction, p)
    };

    // Launch leg — full throttle until the target or the runway ends.
    set_input(
        &mut app,
        car,
        VehicleInput {
            throttle: 1.0,
            ..default()
        },
    );
    let mut launch_speed = 0.0f32;
    for _ in 0..LAUNCH_CAP {
        app.update();
        let (v, _, _) = sample(&app);
        launch_speed = v;
        if v >= LAUNCH_TARGET {
            break;
        }
    }

    // Brake leg — full brake until stopped or the timeout.
    set_input(
        &mut app,
        car,
        VehicleInput {
            brake: 1.0,
            ..default()
        },
    );
    let brake_start = sample(&app).2;
    let mut brake_time = f32::NAN;
    let mut stopped = false;
    for f in 0..BRAKE_CAP {
        app.update();
        let (v, _, _) = sample(&app);
        if v.abs() <= STOP_SPEED {
            brake_time = (f + 1) as f32 / HZ as f32;
            stopped = true;
            break;
        }
    }
    let brake_distance = if stopped {
        let stop = sample(&app).2;
        let d = stop - brake_start;
        (d.x * d.x + d.z * d.z).sqrt()
    } else {
        f32::NAN
    };

    // Reverse leg — the brake stays held through the standstill; the
    // direction state machine must latch `Reverse` and back the car up,
    // not oscillate at the threshold.
    let mut reverse_speed = 0.0f32;
    let mut reversed = false;
    for _ in 0..REVERSE_HOLD {
        app.update();
        let (v, dir, _) = sample(&app);
        reverse_speed = reverse_speed.min(v);
        reversed |= dir == DriveDirection::Reverse;
    }

    // Reset leg — the production `ResetVehicle` path must teleport to
    // the requested pose with motion cleared and `Teleported` stamped.
    let target = Vec3::new(3.0, 1.2, -7.0);
    app.world_mut().write_message(ResetVehicle {
        entity: Some(car),
        position: target,
        yaw: 0.5,
    });
    app.update();
    let world = app.world();
    let pos = world.get::<Position>(car).unwrap().0;
    let vel = world.get::<LinearVelocity>(car).unwrap().0;
    finite &= pos.is_finite() && vel.is_finite();
    let reset_ok = (pos - target).length() < 0.05
        && vel.length() < 1e-3
        && world.get::<Teleported>(car).is_some();

    ControlProbe {
        launch_speed,
        brake_time,
        brake_distance,
        reverse_speed,
        reversed,
        reset_ok,
        finite,
    }
}

/// The landing leg F02-AC02 asks of every stock car: a level drop onto
/// flat ground through the production systems, then proof the car still
/// drives. Articulated rigs drop as the tractor alone — the hitch is the
/// `--clearance` leg's job.
struct DropProbe {
    /// Seconds airborne before the first wheel contact; NaN when the car
    /// never landed inside the cap.
    air_s: f32,
    /// Deepest downward speed while falling, m/s (positive = sinking).
    peak_sink: f32,
    /// Seconds of settling before the car held still for half a second
    /// (counted from the first wheel contact); NaN when it never settled.
    settle_s: f32,
    /// Chassis up-alignment at rest — 1.0 is level, a car on its roof is
    /// negative.
    up: f32,
    /// Ground covered under two seconds of full throttle after the
    /// landing — a landed car still drives.
    drove_m: f32,
    /// Every sampled state stayed finite.
    finite: bool,
}

/// Height the drop leg releases the car at, metres — sized so touchdown
/// is a real impact (~8 m/s sink) but a routine one.
const DROP_HEIGHT: f32 = 4.0;
/// Fall/run-out caps, frames.
const DROP_AIR_CAP: usize = HZ * 4;
const DROP_SETTLE_CAP: usize = HZ * 4;
const DROP_DRIVE: usize = HZ * 2;

fn probe_drop(cfg: &VehicleConfig) -> DropProbe {
    let mut app = base_app();
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(40_000.0, 1.0, 40_000.0),
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    let car = app
        .world_mut()
        .spawn((
            vehicle_bundle(cfg),
            Position(Vec3::new(0.0, DROP_HEIGHT, 0.0)),
            Transform::from_xyz(0.0, DROP_HEIGHT, 0.0),
        ))
        .id();

    let mut finite = true;
    let mut sample = |app: &App| {
        let world = app.world();
        let st = world.get::<VehicleState>(car).unwrap();
        let v = world.get::<LinearVelocity>(car).unwrap().0;
        let av = world.get::<AngularVelocity>(car).unwrap().0;
        let p = world.get::<Position>(car).unwrap().0;
        finite &= st.forward_speed.is_finite() && v.is_finite() && av.is_finite() && p.is_finite();
        (st.grounded, v, av, p)
    };

    let mut peak_sink = 0.0f32;
    let mut air_s = f32::NAN;
    for f in 0..DROP_AIR_CAP {
        app.update();
        let (grounded, v, ..) = sample(&app);
        peak_sink = peak_sink.max(-v.y);
        if grounded {
            air_s = f as f32 / HZ as f32;
            break;
        }
    }

    // Settle: sustained stillness — the bounce after touchdown keeps a
    // clean landing moving for a while.
    let mut settle_s = f32::NAN;
    let mut still = 0usize;
    for f in 0..DROP_SETTLE_CAP {
        app.update();
        let (grounded, v, av, _) = sample(&app);
        if grounded && v.length() < 0.3 && av.length() < 0.5 {
            still += 1;
        } else {
            still = 0;
        }
        if still >= HZ / 2 {
            settle_s = f as f32 / HZ as f32;
            break;
        }
    }

    let up = {
        let world = app.world();
        let rot = world.get::<Rotation>(car).unwrap().0;
        (rot * Vec3::Y).y
    };

    // Drive-away: a landed car still answers the throttle.
    let start = sample(&app).3;
    set_input(
        &mut app,
        car,
        VehicleInput {
            throttle: 1.0,
            ..default()
        },
    );
    for _ in 0..DROP_DRIVE {
        app.update();
    }
    let end = sample(&app).3;
    let dv = end - start;
    let drove_m = (dv.x * dv.x + dv.z * dv.z).sqrt();

    DropProbe {
        air_s,
        peak_sink,
        settle_s,
        up,
        drove_m,
        finite,
    }
}

/// One body at rest on its wheels. Wheels are raycasts — at rest the
/// hull hangs above the road, so a nonzero contact count means the body
/// is propped on geometry: the spawn (or reset) penetrated something.
struct RestState {
    grounded: usize,
    wheels: usize,
    /// Ungrounded wheel indices, for the report.
    ungrounded: Vec<usize>,
    /// Names of the entities the hull still contacts.
    contacts: Vec<String>,
    /// The rig's other body is among the contacts.
    touches_other: bool,
    finite: bool,
}

fn rest_state(app: &App, body: Entity, other: Option<Entity>) -> RestState {
    let world = app.world();
    let st = world.get::<VehicleState>(body).unwrap();
    let grounded = st.wheels.iter().filter(|w| w.grounded).count();
    let ungrounded = st
        .wheels
        .iter()
        .enumerate()
        .filter(|(_, w)| !w.grounded)
        .map(|(i, _)| i)
        .collect();
    let contacts = world.get::<CollidingEntities>(body).unwrap();
    let touches_other = other.is_some_and(|o| contacts.contains(&o));
    let names = contacts
        .iter()
        .map(|e| {
            world
                .get::<Name>(*e)
                .map(|n| n.to_string())
                .unwrap_or_else(|| format!("{e}"))
        })
        .collect();
    let finite = world.get::<Position>(body).unwrap().0.is_finite();
    RestState {
        grounded,
        wheels: st.wheels.len(),
        ungrounded,
        contacts: names,
        touches_other,
        finite,
    }
}

/// `g/n` wheel summary, appending ungrounded indices and contact names
/// when the rest state is not clean.
fn rest_summary(s: &RestState) -> String {
    let mut out = format!("{}/{} c{}", s.grounded, s.wheels, s.contacts.len());
    if !s.ungrounded.is_empty() {
        out += &format!(
            " w{}",
            s.ungrounded
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join("+")
        );
    }
    if !s.contacts.is_empty() {
        out += &format!(" [{}]", s.contacts.join(","));
    }
    out
}

/// World-space gap between the trailer hitch anchors — the spherical
/// joint holds them coincident, so a gap means the spawn stretched the
/// hitch.
fn hitch_gap(app: &App, car: Entity, trailer: Entity, tdef: &mm2_content::TrailerDef) -> f32 {
    let world = app.world();
    let (cp, cr) = (
        world.get::<Position>(car).unwrap().0,
        world.get::<Rotation>(car).unwrap().0,
    );
    let (tp, tr) = (
        world.get::<Position>(trailer).unwrap().0,
        world.get::<Rotation>(trailer).unwrap().0,
    );
    let a = cp + cr * Vec3::from(tdef.car_hitch);
    let b = tp + tr * Vec3::from(tdef.trailer_hitch);
    (a - b).length()
}

/// The F02-AC05 leg on real road geometry: settle at the authored spawn,
/// prove the wheels carry the car with the hull clear of the world, then
/// `ResetVehicle` back to the spawn and re-check. Articulated cars spawn
/// their trailer through the production [`car_visual::spawn_trailer`]
/// path and re-seat it the way the app's reset consumers do — a second
/// `ResetVehicle` at the authored rest offset.
fn probe_clearance(vfs: &Vfs, city_name: &str, def: &VehicleDef, cfg: VehicleConfig) {
    let (mut app, car, spawn, yaw) = headless_city(vfs, city_name, cfg);
    let yaw_rot = Quat::from_rotation_y(yaw);

    let trailer = def.trailer.as_ref().map(|tdef| {
        let world = app.world_mut();
        let car_tf = *world.get::<Transform>(car).unwrap();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let (mut meshes, mut images, mut materials) = (
            Assets::<Mesh>::default(),
            Assets::<Image>::default(),
            Assets::<StandardMaterial>::default(),
        );
        let ent = {
            let mut commands = Commands::new(&mut queue, world);
            let (e, missing) = car_visual::spawn_trailer(
                &mut commands,
                vfs,
                tdef,
                0,
                &mut meshes,
                &mut images,
                &mut materials,
                car,
                car_tf,
                mm2_game::SessionEntity(1),
            );
            if !missing.is_empty() {
                println!("  trailer missing textures: {}", missing.join(", "));
            }
            commands.entity(e).insert(CollidingEntities::default());
            e
        };
        queue.apply(world);
        ent
    });

    settle_long(&mut app, car);
    let a = rest_state(&app, car, trailer);
    let b = trailer.map(|te| rest_state(&app, te, Some(car)));
    let gap = trailer
        .map(|te| hitch_gap(&app, car, te, def.trailer.as_ref().unwrap()))
        .unwrap_or(f32::NAN);

    app.world_mut().write_message(ResetVehicle {
        entity: Some(car),
        position: spawn,
        yaw,
    });
    if let Some(te) = trailer {
        let rest = app
            .world()
            .get::<car_visual::Trailer>(te)
            .unwrap()
            .rest_offset;
        app.world_mut().write_message(ResetVehicle {
            entity: Some(te),
            position: spawn + yaw_rot * rest,
            yaw,
        });
    }
    settle_long(&mut app, car);
    let a2 = rest_state(&app, car, trailer);
    let b2 = trailer.map(|te| rest_state(&app, te, Some(car)));
    let gap2 = trailer
        .map(|te| hitch_gap(&app, car, te, def.trailer.as_ref().unwrap()))
        .unwrap_or(f32::NAN);

    let body_ok = |s: &RestState| {
        s.grounded == s.wheels && s.contacts.is_empty() && !s.touches_other && s.finite
    };
    let mut legs: Vec<&str> = Vec::new();
    if !body_ok(&a) {
        legs.push("spawn");
    }
    if !body_ok(&a2) {
        legs.push("reset");
    }
    if let (Some(s), Some(s2)) = (&b, &b2) {
        if !body_ok(s) {
            legs.push("trailer-spawn");
        }
        if !body_ok(s2) {
            legs.push("trailer-reset");
        }
        if !(gap < 0.1 && gap2 < 0.1) {
            legs.push("hitch");
        }
    }
    println!(
        "{:<14} {:>9} {:>9} {:>30}   {}",
        def.id,
        rest_summary(&a),
        rest_summary(&a2),
        b.as_ref()
            .map(|s| format!("{} gap{:.2}→{:.2}", rest_summary(s), gap, gap2))
            .unwrap_or_else(|| "-".into()),
        if legs.is_empty() {
            "ok".to_string()
        } else {
            format!("FAIL({})", legs.join(","))
        },
    );
    if !legs.is_empty() {
        let world = app.world();
        let pos = world.get::<Position>(car).unwrap().0;
        let rot = world.get::<Rotation>(car).unwrap().0;
        let up = rot * Vec3::Y;
        let lv = world.get::<LinearVelocity>(car).unwrap().0;
        let av = world.get::<AngularVelocity>(car).unwrap().0;
        println!(
            "    spawn [{:.2},{:.2},{:.2}] yaw {:.2} -> settled [{:.2},{:.2},{:.2}] up [{:.2},{:.2},{:.2}] |v|={:.3} |w|={:.3}",
            spawn.x,
            spawn.y,
            spawn.z,
            yaw,
            pos.x,
            pos.y,
            pos.z,
            up.x,
            up.y,
            up.z,
            lv.length(),
            av.length(),
        );
        // Where the hull actually touches — manifold points in world
        // space locate the geometry propping the body up.
        let graph = world.resource::<ContactGraph>();
        for other in world.get::<CollidingEntities>(car).unwrap().iter() {
            let Some((_, pair)) = graph.get(car, *other) else {
                continue;
            };
            for m in &pair.manifolds {
                for p in &m.points {
                    println!(
                        "    hull contact at [{:.2},{:.2},{:.2}] pen {:.3}",
                        p.point.x, p.point.y, p.point.z, p.penetration,
                    );
                }
            }
        }
        // Per-wheel ground truth: a grounded wheel's contact point is the
        // road; an ungrounded wheel's max reach tells whether the road
        // fell away under it (reach stays above the neighbours' ground)
        // or the suspension failed to extend.
        let veh = world.get::<Vehicle>(car).unwrap();
        let st = world.get::<VehicleState>(car).unwrap();
        for (i, (wc, ws)) in veh.config.wheels.iter().zip(&st.wheels).enumerate() {
            let hardpoint = pos + rot * Vec3::from(wc.position);
            let reach = wc.suspension.as_ref().map(|s| s.travel).unwrap_or(0.0) + wc.radius;
            let bottom = hardpoint - rot * Vec3::Y * reach;
            println!(
                "    w{i} {} hardpoint [{:.2},{:.2},{:.2}] {}",
                if ws.grounded {
                    "grounded "
                } else {
                    "airborne "
                },
                hardpoint.x,
                hardpoint.y,
                hardpoint.z,
                if ws.grounded {
                    format!(
                        "contact [{:.2},{:.2},{:.2}]",
                        ws.contact_point.x, ws.contact_point.y, ws.contact_point.z
                    )
                } else {
                    format!(
                        "reach bottom [{:.2},{:.2},{:.2}]",
                        bottom.x, bottom.y, bottom.z
                    )
                },
            );
        }
    }
}

fn base_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / HZ as f64,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin);
    app.finish();
    app.cleanup();
    app
}

fn headless(cfg: VehicleConfig) -> (App, Entity) {
    let mut app = base_app();
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(40_000.0, 1.0, 40_000.0),
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    let car = app
        .world_mut()
        .spawn((
            vehicle_bundle(&cfg),
            Position(Vec3::new(0.0, 1.5, 0.0)),
            Transform::from_xyz(0.0, 1.5, 0.0),
        ))
        .id();
    (app, car)
}

fn headless_city(vfs: &Vfs, city_name: &str, cfg: VehicleConfig) -> (App, Entity, Vec3, f32) {
    let mut app = base_app();
    // The city importer wants the render-side asset stores even when only
    // its colliders are of interest.
    let (spawn, yaw) = {
        let world = app.world_mut();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let loaded = {
            let mut commands = Commands::new(&mut queue, world);
            let (mut meshes, mut images, mut materials) = (
                Assets::<Mesh>::default(),
                Assets::<Image>::default(),
                Assets::<StandardMaterial>::default(),
            );
            let mut session = mm2_game::Session::new();
            city::load_city(
                &mut commands,
                vfs,
                &format!("city/{city_name}.psdl"),
                &mut meshes,
                &mut images,
                &mut materials,
                mm2_game::SessionEntity(1),
                &mut session,
            )
            .unwrap()
        };
        queue.apply(world);
        (loaded.spawn, loaded.spawn_yaw)
    };

    let car = app
        .world_mut()
        .spawn((
            vehicle_bundle(&cfg),
            CollidingEntities::default(),
            Position(spawn),
            Transform::from_translation(spawn).with_rotation(Quat::from_rotation_y(yaw)),
            Rotation(Quat::from_rotation_y(yaw)),
        ))
        .id();
    (app, car, spawn, yaw)
}

fn settle(app: &mut App, car: Entity) {
    set_input(app, car, VehicleInput::default());
    for _ in 0..HZ * 2 {
        app.update();
    }
}

/// Longer settle used by the clearance leg: the ~1.5 m spawn drop takes
/// a while to damp out on soft-suspension cars (the Moon Rover wallows
/// for several seconds). Runs until the body is still, capped at 15 s —
/// a car still oscillating at the cap gets checked as-is.
fn settle_long(app: &mut App, car: Entity) {
    set_input(app, car, VehicleInput::default());
    let mut still = 0;
    for _ in 0..HZ * 15 {
        app.update();
        let world = app.world();
        let lv = world.get::<LinearVelocity>(car).unwrap().0.length();
        let av = world.get::<AngularVelocity>(car).unwrap().0.length();
        still = if lv < 0.02 && av < 0.02 { still + 1 } else { 0 };
        if still > HZ / 2 {
            break;
        }
    }
}

fn set_input(app: &mut App, car: Entity, input: VehicleInput) {
    *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = input;
}
