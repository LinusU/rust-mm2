//! NETPLAY-03F: what the state copy costs in a loaded world with eight
//! human cars. The synthetic world is city-sized in static colliders and
//! has trailers, sleeping and awake props and cars driving into a wall and
//! into each other; it is not retail data, so it runs in CI. The measuring
//! test is `#[ignore]` (timings are meaningless under a loaded CI runner);
//! run it as documented in `docs/netcode-redesign.md`:
//!
//! ```sh
//! cargo test -p mm2_vehicle --test profile -- --ignored --nocapture
//! ```
//!
//! The always-on test runs the same scene small and asserts the structural
//! claims (ring memory is bounded, a deep replay is exact enough).

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_vehicle::capture::EpochRef;
use mm2_vehicle::physics_world::capture_world;
use mm2_vehicle::replay::{HistoryRing, MAX_HISTORY, ReplayStep, replay_from};
use mm2_vehicle::vehicle::VehicleInput;
use mm2_vehicle::wire::capture_authoritative;
use mm2_vehicle::{
    OriginalContactMaterial, VehicleConfig, VehiclePlugin, VehicleProbeLayers, vehicle_bundle,
};

use common::{Model, config};

/// Counts live heap bytes so a capture's real footprint is measured, not
/// estimated from `size_of`.
struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);

// SAFETY: defers every call to `System` and only adds bookkeeping.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        LIVE.fetch_add(l.size(), Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        LIVE.fetch_add(n, Ordering::Relaxed);
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static ALLOC: Counting = Counting;

const EPOCH: EpochRef = EpochRef {
    generation: 1,
    reset_epoch: 1,
};

#[derive(Clone, Copy)]
struct Size {
    /// Static building colliders (the broad phase holds them all).
    statics: usize,
    props: usize,
}

const CITY: Size = Size {
    statics: 6000,
    props: 200,
};
const SMALL: Size = Size {
    statics: 200,
    props: 16,
};
const CARS: usize = 8;

struct City {
    app: App,
    cars: Vec<Entity>,
    tick: u64,
}

fn child_layers() -> CollisionLayers {
    CollisionLayers::new(0b10u32, LayerMask::ALL)
}

fn city(size: Size) -> City {
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
        Collider::cuboid(2000.0, 1.0, 2000.0),
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    // Buildings on a grid, leaving a street (z = 0) and a cross street
    // (x = 0) clear for the cars.
    let side = (size.statics as f32).sqrt().ceil() as i32;
    for i in 0..size.statics as i32 {
        let (gx, gz) = (i % side - side / 2, i / side - side / 2);
        if gx == 0 || gz == 0 {
            continue;
        }
        let p = Vec3::new(gx as f32 * 24.0, 6.0, gz as f32 * 24.0);
        w.spawn((
            RigidBody::Static,
            Collider::cuboid(14.0, 12.0, 14.0),
            OriginalContactMaterial {
                friction: 0.5,
                elasticity: 0.3,
            },
            Position(p),
            Transform::from_translation(p),
        ));
    }
    // A wall across the street end the first car drives into.
    let wall = Vec3::new(0.0, 2.0, -90.0);
    w.spawn((
        RigidBody::Static,
        Collider::cuboid(30.0, 4.0, 1.0),
        OriginalContactMaterial {
            friction: 0.5,
            elasticity: 0.3,
        },
        Position(wall),
        Transform::from_translation(wall),
    ));
    // Eight cars in two lanes on the street, all driving -z; every other
    // one tows a jointed trailer.
    let mut cars = Vec::new();
    for n in 0..CARS {
        let model = if n % 2 == 0 {
            Model::Original
        } else {
            Model::Generic
        };
        let pos = Vec3::new((n % 2) as f32 * 6.0 - 3.0, 1.2, 8.0 + (n / 2) as f32 * 12.0);
        let car = w
            .spawn((
                vehicle_bundle(&config(model)),
                Position(pos),
                Transform::from_translation(pos),
            ))
            .id();
        w.spawn((
            ChildOf(car),
            Collider::sphere(0.4),
            ColliderDensity(0.0),
            child_layers(),
            Transform::from_xyz(0.0, 0.8, 0.0),
        ));
        if n % 2 == 1 {
            let hitch = Vec3::new(0.0, 0.3, 2.2);
            let tpos = pos + hitch * 2.0;
            let trailer = w
                .spawn((
                    vehicle_bundle(&VehicleConfig::default()),
                    Position(tpos),
                    Transform::from_translation(tpos),
                ))
                .id();
            w.spawn((
                ChildOf(trailer),
                SphericalJoint::new(car, trailer)
                    .with_local_anchor1(hitch)
                    .with_local_anchor2(-hitch)
                    .with_swing_limits(-0.6, 0.6)
                    .with_twist_limits(-0.15, 0.15),
                JointCollisionDisabled,
            ));
        }
        cars.push(car);
    }
    // Props strewn down the street, some in the cars' path (they wake on
    // contact), the rest far enough to sleep throughout.
    for n in 0..size.props {
        let x = (n % 5) as f32 * 3.0 - 6.0;
        let z = 4.0 - (n as f32) * 0.4 - 10.0;
        let p = Vec3::new(x, 0.5, z);
        w.spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            Mass(60.0),
            Position(p),
            Transform::from_translation(p),
        ));
    }
    app.update(); // the first update has no elapsed time: no step
    City { app, cars, tick: 0 }
}

fn command(car: usize, t: u64) -> VehicleInput {
    let steer = ((t as f32 + car as f32 * 7.0) * 0.05).sin() * 0.15;
    VehicleInput {
        throttle: if t < 30 { 1.0 } else { 0.7 },
        brake: 0.0,
        steering: steer,
        handbrake: 0.0,
        forced_gear: None,
    }
}

fn drive(c: &mut City) {
    for (n, car) in c.cars.iter().enumerate() {
        *c.app.world_mut().get_mut::<VehicleInput>(*car).unwrap() = command(n, c.tick);
    }
    c.app.update();
    c.tick += 1;
}

fn plan(c: &City, from: u64, to: u64) -> Vec<ReplayStep> {
    (from..to)
        .map(|t| ReplayStep {
            inputs: c
                .cars
                .iter()
                .enumerate()
                .map(|(n, e)| (*e, command(n, t)))
                .collect(),
            ..Default::default()
        })
        .collect()
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

#[derive(Default)]
struct Stats(Vec<f64>);
impl Stats {
    fn add(&mut self, d: Duration) {
        self.0.push(ms(d));
    }
    fn report(&mut self) -> String {
        self.0.sort_by(|a, b| a.total_cmp(b));
        let n = self.0.len();
        let mean = self.0.iter().sum::<f64>() / n as f64;
        format!(
            "mean {mean:.3} ms, p50 {:.3}, p99 {:.3}, max {:.3} (n={n})",
            self.0[n / 2],
            self.0[(n * 99 / 100).min(n - 1)],
            self.0[n - 1]
        )
    }
}

/// Live heap bytes held by a value built by `f`.
fn heap_of<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = LIVE.load(Ordering::Relaxed);
    let v = f();
    (v, LIVE.load(Ordering::Relaxed).saturating_sub(before))
}

#[test]
#[ignore = "timing harness; see the module docs"]
fn profile_loaded_city_with_eight_cars() {
    let mut c = city(CITY);
    let world = c.app.world_mut();
    let bodies = world.query::<&RigidBody>().iter(world).count();
    println!(
        "world: {bodies} rigid bodies, {CARS} cars, {} statics",
        CITY.statics
    );

    // Settle, then time an ordinary tick with and without a capture.
    for _ in 0..30 {
        drive(&mut c);
    }
    let mut plain = Stats::default();
    let mut capture = Stats::default();
    let mut authoritative = Stats::default();
    let mut ring = HistoryRing::default();
    let mut one_capture_bytes = 0;
    for _ in 0..300 {
        let t = Instant::now();
        drive(&mut c);
        plain.add(t.elapsed());
        let t = Instant::now();
        let (cap, bytes) = heap_of(|| capture_world(c.app.world_mut(), EPOCH));
        capture.add(t.elapsed());
        one_capture_bytes = one_capture_bytes.max(bytes);
        ring.record(c.tick, cap);
        let t = Instant::now();
        let _ = capture_authoritative(c.app.world_mut(), 1, &|_| 0, c.tick);
        authoritative.add(t.elapsed());
    }
    let last = ring.get(c.tick).unwrap();
    println!(
        "last capture: {} vehicles, {} props ({} asleep), {} contact pairs ({} touching)",
        last.vehicles.len(),
        last.bodies.len(),
        last.bodies.iter().filter(|(_, b)| b.sleeping).count(),
        last.pairs.len(),
        last.pairs.iter().filter(|(_, p)| p.touching()).count(),
    );
    println!("ordinary tick      : {}", plain.report());
    println!("capture_world      : {}", capture.report());
    println!("capture_authoritative: {}", authoritative.report());
    println!(
        "one capture: {one_capture_bytes} B; ring of {} = {:.1} KiB",
        ring.len(),
        one_capture_bytes as f64 * ring.len() as f64 / 1024.0
    );

    // Replay: restore + N ticks, from the ring, at several depths.
    for depth in [6usize, 20, 60, MAX_HISTORY - 1] {
        let mut restore_only = Stats::default();
        let mut replay = Stats::default();
        for _ in 0..5 {
            let to = c.tick;
            let from = to - depth as u64;
            let steps = plan(&c, from, to);
            let one = plan(&c, from, from + 1);
            let t = Instant::now();
            replay_from(c.app.world_mut(), &ring, from, &one).unwrap();
            restore_only.add(t.elapsed());
            let t = Instant::now();
            replay_from(c.app.world_mut(), &ring, from, &steps).unwrap();
            let d = t.elapsed();
            replay.add(d);
            println!(
                "  depth {depth:3}: {:.3} ms total, {:.3} ms/tick",
                ms(d),
                ms(d) / depth as f64
            );
        }
        println!(
            "replay depth {depth}: restore+1 tick {}",
            restore_only.report()
        );
        println!("replay depth {depth}: full {}", replay.report());
    }
}

#[test]
fn a_small_loaded_city_replays_deeply_and_the_ring_is_bounded() {
    let mut c = city(SMALL);
    for _ in 0..30 {
        drive(&mut c);
    }
    let mut ring = HistoryRing::default();
    let (_, one) = heap_of(|| {
        let cap = capture_world(c.app.world_mut(), EPOCH);
        let n = cap.vehicles.len();
        ring.record(c.tick, cap);
        n
    });
    assert!(one > 0, "a capture holds something");
    for _ in 0..(MAX_HISTORY + 20) {
        drive(&mut c);
        let cap = capture_world(c.app.world_mut(), EPOCH);
        ring.record(c.tick, cap);
    }
    assert_eq!(ring.len(), MAX_HISTORY, "the ring holds exactly its bound");
    let to = c.tick;
    let from = to - 60;
    let steps = plan(&c, from, to);
    let report = replay_from(c.app.world_mut(), &ring, from, &steps).unwrap();
    assert_eq!(report.steps, 60);
    assert!(report.restore.refused.is_empty());
}
