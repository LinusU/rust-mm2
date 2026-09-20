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
//! ```
//!
//! With a city name it instead reproduces the plainest possible bug
//! report — spawn and hold the throttle — over real road geometry, and
//! reports every point where the car's *body* touched the world:
//!
//! ```sh
//! cargo run -p mm2_app --example drive_probe -- retail vppanoz --city sf
//! ```

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::city;
use mm2_assets::{InstallMount, Vfs, mount_install};
use mm2_vehicle::vehicle::{VehicleInput, VehicleState};
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
    let want = args.get(2).filter(|a| !a.starts_with("--")).cloned();

    let mut vfs = Vfs::new();
    mount_install(&mut vfs, dir.as_ref(), &InstallMount::default()).unwrap();
    let catalog = mm2_content::VehicleCatalog::scan(&vfs);

    if let Some(city) = city {
        let id = want.expect("--city needs a vehicle id");
        let id = catalog.find(&id).unwrap().id.clone();
        let def = mm2_content::load_vehicle(&vfs, &id, 0).unwrap();
        probe_city(&vfs, &city, &def.config);
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
        let accel = probe_acceleration(&def.config);
        let corner: Vec<String> = [10.0f32, 20.0, 30.0, 40.0]
            .iter()
            .map(|v| format!("{:.2}@{:.0}", probe_corner_g(&def.config, *v), v))
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

/// Spawn in a real city, hold the throttle, and report every stretch where
/// the chassis touched the world.
///
/// The wheels are raycasts and never collide, so any contact at all is the
/// *body* hitting road, kerb or prop — which is what "it catches on a
/// seam" looks like from inside the simulation.
fn probe_city(vfs: &Vfs, city_name: &str, cfg: &VehicleConfig) {
    let (mut app, car) = headless_city(vfs, city_name, cfg.clone());
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

fn headless_city(vfs: &Vfs, city_name: &str, cfg: VehicleConfig) -> (App, Entity) {
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
    println!(
        "{city_name}: spawn ({:.0}, {:.1}, {:.0}) yaw {:.0}°",
        spawn.x,
        spawn.y,
        spawn.z,
        yaw.to_degrees()
    );

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
    (app, car)
}

fn settle(app: &mut App, car: Entity) {
    set_input(app, car, VehicleInput::default());
    for _ in 0..HZ * 2 {
        app.update();
    }
}

fn set_input(app: &mut App, car: Entity, input: VehicleInput) {
    *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = input;
}
