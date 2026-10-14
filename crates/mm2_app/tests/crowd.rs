//! F19-B.2 sidewalk crowd through the production systems: a synthetic
//! city (one 200 m road with a sidewalk each side) beside the synthetic
//! pedestrian archetype, populated, walked, recycled and reset by the
//! same `maintain_pedestrians` / `walk_pedestrians` /
//! `animate_pedestrians` the app schedules.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::crowd::{
    PedCrowd, PedDensity, PedReact, PedWalk, maintain_pedestrians, react_pedestrians,
    walk_pedestrians,
};
use mm2_app::layers::GameLayer;
use mm2_app::pedestrian::{PedActor, animate_pedestrians};
use mm2_assets::Vfs;
use mm2_game::{
    Mm2Vfs, Player, PlayerControl, PlayerId, Session, SessionAuthority, SessionConfig,
    SessionEntity, SessionPhase, WorldMode, despawn_session_entities,
};

use mm2_game::pedreact::Phase;
use mm2_vehicle::vehicle::Vehicle;
use mm2_vehicle::{VehicleConfig, VehicleInput, vehicle_bundle};

use crate::pedestrian::{
    CONTACT_CSV, REACTIVE_CSV, install, install_flat, install_solid, install_with,
};

fn push_v3(d: &mut Vec<u8>, v: [f32; 3]) {
    for f in v {
        d.extend_from_slice(&f.to_le_bytes());
    }
}

/// One road along z from -100 to 100 with a vehicle lane at x ±3.75
/// and a sidewalk at x ±8.5, both ends dead.
fn bai_bytes() -> Vec<u8> {
    let (z0, z1) = (-100.0f32, 100.0f32);
    let mut d = Vec::new();
    d.extend_from_slice(b"CAI1");
    d.extend_from_slice(&0u16.to_le_bytes()); // intersections
    d.extend_from_slice(&1u16.to_le_bytes()); // roads
    d.extend_from_slice(&0u16.to_le_bytes()); // id
    d.extend_from_slice(&2u16.to_le_bytes()); // sections
    d.extend_from_slice(&0u16.to_le_bytes()); // flags
    d.extend_from_slice(&1u16.to_le_bytes()); // rooms
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&7.5f32.to_le_bytes());
    d.extend_from_slice(&15.0f32.to_le_bytes());
    for side in [1f32, -1f32] {
        for n in [1u16, 0, 0, 1, 0] {
            d.extend_from_slice(&n.to_le_bytes());
        }
        for _ in 0..2 {
            for s in [0f32, z1 - z0] {
                d.extend_from_slice(&s.to_le_bytes());
            }
        }
        for edge in [5.0f32, 9.5] {
            d.extend_from_slice(&edge.to_le_bytes());
        }
        d.extend_from_slice(&[0xCDu8; 40]);
        for off in [3.75f32, 8.5] {
            for z in [z0, z1] {
                push_v3(&mut d, [off * side, 0.0, z]);
            }
        }
        for z in [z0, z1] {
            push_v3(&mut d, [7.5 * side, 0.0, z]);
        }
        for z in [z0, z1] {
            push_v3(&mut d, [9.5 * side, 0.0, z]);
        }
    }
    for s in [0f32, z1 - z0] {
        d.extend_from_slice(&s.to_le_bytes());
    }
    for z in [z0, z1] {
        push_v3(&mut d, [0.0, 0.0, z]);
    }
    for axis in [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, 1.0],
    ] {
        for _ in 0..2 {
            push_v3(&mut d, axis);
        }
    }
    for _ in 0..2 {
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&0xCDCDu16.to_le_bytes());
        d.extend_from_slice(&0u16.to_le_bytes());
        d.extend_from_slice(&0u16.to_le_bytes());
        d.extend_from_slice(&mm2_formats::bai::END_FILL.to_le_bytes());
        push_v3(&mut d, [0.0; 3]);
        push_v3(&mut d, [0.0; 3]);
    }
    d.extend_from_slice(&0u32.to_le_bytes()); // culling rooms
    d
}

fn config(seed: u64) -> SessionConfig {
    SessionConfig {
        world: WorldMode::City {
            psdl: "city/walk.psdl".to_string(),
        },
        seed,
        ..SessionConfig::default()
    }
}

fn playing(seed: u64, authority: SessionAuthority) -> Session {
    let mut session = Session::new();
    session
        .begin(SessionConfig {
            authority,
            ..config(seed)
        })
        .unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    session
}

fn mount(dir: &Path) -> Mm2Vfs {
    std::fs::create_dir_all(dir.join("city")).unwrap();
    std::fs::write(dir.join("city/walk.bai"), bai_bytes()).unwrap();
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    Mm2Vfs(vfs)
}

fn app(dir: &Path, seed: u64, density: Option<f32>) -> App {
    app_as(dir, seed, density, SessionAuthority::Local)
}

fn app_as(dir: &Path, seed: u64, density: Option<f32>, authority: SessionAuthority) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(TransformPlugin)
        .init_asset::<StandardMaterial>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(playing(seed, authority))
        .insert_resource(mount(dir))
        .add_message::<mm2_app::audio::PedScream>()
        .add_systems(
            Update,
            (
                despawn_session_entities
                    .run_if(|s: Res<Session>| matches!(s.phase(), SessionPhase::Unloading)),
                maintain_pedestrians,
                react_pedestrians,
                walk_pedestrians,
                animate_pedestrians,
            )
                .chain(),
        );
    if let Some(d) = density {
        app.insert_resource(PedDensity(d));
    }
    app.world_mut().spawn((
        Player {
            id: PlayerId(0),
            control: PlayerControl::Local,
        },
        Position(Vec3::ZERO),
    ));
    app.finish();
    app.cleanup();
    app
}

/// The crowd app with physics live (F19-B.8b), so a real dynamic car and
/// a walker's kinematic body actually contact. Everything the plain
/// [`app`] runs, plus the default Avian plugins and a static ground the
/// car can drive on; the walker's body ignores the ground (the
/// `Pedestrian` layer does not meet `World`), so no kinematic-vs-trimesh
/// pair is created under every figure.
fn physics_app(dir: &Path, seed: u64, density: f32) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(PhysicsPlugins::default())
        .add_plugins(TransformPlugin)
        .init_asset::<StandardMaterial>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(playing(seed, SessionAuthority::Local))
        .insert_resource(mount(dir))
        .insert_resource(PedDensity(density))
        .add_message::<mm2_app::audio::PedScream>()
        .add_systems(
            Update,
            (
                despawn_session_entities
                    .run_if(|s: Res<Session>| matches!(s.phase(), SessionPhase::Unloading)),
                maintain_pedestrians,
                react_pedestrians,
                walk_pedestrians,
                animate_pedestrians,
            )
                .chain(),
        );
    app.world_mut().spawn((
        Player {
            id: PlayerId(0),
            control: PlayerControl::Local,
        },
        Position(Vec3::ZERO),
    ));
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(200.0, 1.0, 200.0),
        GameLayer::world(),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    app.finish();
    app.cleanup();
    app
}

fn run(app: &mut App, n: usize) {
    for _ in 0..n {
        app.update();
    }
}

fn walkers(app: &mut App) -> Vec<(Vec3, PedWalk)> {
    app.world_mut()
        .query::<(&Transform, &PedWalk)>()
        .iter(app.world())
        .map(|(t, w)| (t.translation, *w))
        .collect()
}

fn actors(app: &mut App) -> usize {
    app.world_mut()
        .query::<&PedActor>()
        .iter(app.world())
        .count()
}

fn sorted_positions(app: &mut App) -> Vec<[i32; 3]> {
    let mut p: Vec<[i32; 3]> = walkers(app)
        .iter()
        .map(|(t, _)| {
            [
                (t.x * 100.0) as i32,
                (t.y * 100.0) as i32,
                (t.z * 100.0) as i32,
            ]
        })
        .collect();
    p.sort_unstable();
    p
}

fn move_player(app: &mut App, to: Vec3) {
    let mut q = app.world_mut().query::<&mut Position>();
    for mut p in q.iter_mut(app.world_mut()) {
        p.0 = to;
    }
}

#[test]
fn the_density_target_fields_session_owned_walkers_on_the_sidewalks() {
    let tmp = install();
    let mut app = app(tmp.path(), 7, Some(0.5));
    run(&mut app, 5);
    let found = walkers(&mut app);
    assert_eq!(found.len(), 24, "round(0.5 * 48) walkers");
    assert_eq!(actors(&mut app), 24);
    for (at, w) in &found {
        assert!(
            (at.x.abs() - 8.5).abs() < 0.01 && at.y.abs() < 0.01,
            "on a sidewalk curve: {at:?}"
        );
        let d = at.length();
        assert!((14.0..=91.0).contains(&d), "inside the spawn annulus: {d}");
        assert!(w.speed > 0.0 && w.speed.is_finite());
    }
    let generation = app.world().resource::<Session>().generation();
    let mut owners = app
        .world_mut()
        .query_filtered::<&SessionEntity, With<PedWalk>>();
    assert!(owners.iter(app.world()).all(|o| o.0 == generation));
    let crowd = app.world().resource::<PedCrowd>();
    assert_eq!((crowd.target, crowd.spawned), (24, 24));
    assert!(crowd.is_active());
}

#[test]
fn no_density_resolved_or_a_zero_density_fields_nobody() {
    let tmp = install();
    let mut none = app(tmp.path(), 7, None);
    run(&mut none, 10);
    assert_eq!(actors(&mut none), 0, "no resolved density, no crowd");
    assert!(none.world().get_resource::<PedCrowd>().is_none());

    let mut zero = app(tmp.path(), 7, Some(0.0));
    run(&mut zero, 10);
    assert_eq!(actors(&mut zero), 0);
    let crowd = zero.world().resource::<PedCrowd>();
    assert!(!crowd.is_active() && !crowd.issues.is_empty());
}

#[test]
fn the_same_seed_fields_the_same_crowd_and_another_seed_a_different_one() {
    let tmp = install();
    let crowd = |seed| {
        let mut a = app(tmp.path(), seed, Some(0.5));
        run(&mut a, 3);
        sorted_positions(&mut a)
    };
    assert_eq!(crowd(11), crowd(11));
    assert_ne!(crowd(11), crowd(12));
}

#[test]
fn walkers_stay_on_their_sidewalk_and_a_pause_freezes_them() {
    let tmp = install();
    let mut app = app(tmp.path(), 3, Some(0.5));
    run(&mut app, 3);
    let before = sorted_positions(&mut app);
    run(&mut app, 120);
    let after = walkers(&mut app);
    assert_ne!(before, sorted_positions(&mut app), "they walk");
    for (at, _) in &after {
        assert!(
            (at.x.abs() - 8.5).abs() < 0.01 && at.z.abs() <= 100.01,
            "still on a curve: {at:?}"
        );
    }
    let turned = app.world().resource::<PedCrowd>().turned_around;
    // The sidewalks are dead ends 100 m away; two seconds at the
    // synthetic clip's pace has not reached them, so no U-turn yet.
    assert_eq!(turned, 0);

    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    app.update();
    let frozen = sorted_positions(&mut app);
    run(&mut app, 60);
    assert_eq!(
        frozen,
        sorted_positions(&mut app),
        "a pause holds the crowd"
    );
}

#[test]
fn walkers_turn_round_at_a_dead_end_and_never_leave_the_net() {
    let tmp = install();
    let mut app = app(tmp.path(), 5, Some(1.0));
    // The player rides the road so the dead ends stay in the bubble.
    move_player(&mut app, Vec3::new(0.0, 0.0, 60.0));
    for _ in 0..40 {
        run(&mut app, 60);
        for (at, _) in walkers(&mut app) {
            assert!(at.is_finite() && at.z.abs() <= 100.01, "{at:?}");
        }
    }
    let crowd = app.world().resource::<PedCrowd>();
    assert!(crowd.turned_around > 0, "someone reached a dead end");
}

#[test]
fn a_walker_that_leaves_every_bubble_is_recycled_and_the_target_refilled() {
    let tmp = install();
    let mut app = app(tmp.path(), 9, Some(0.5));
    run(&mut app, 3);
    // Drive 150 m down the road: the first crowd is beyond 90 m.
    move_player(&mut app, Vec3::new(0.0, 0.0, 95.0));
    run(&mut app, 120);
    let player = Vec3::new(0.0, 0.0, 95.0);
    let found = walkers(&mut app);
    assert!(!found.is_empty());
    for (at, _) in &found {
        assert!(at.distance(player) <= 91.0, "inside the bubble: {at:?}");
    }
    let crowd = app.world().resource::<PedCrowd>();
    assert!(crowd.recycled > 0);
    assert!(found.len() <= 24, "never past the target: {}", found.len());
    assert!(
        actors(&mut app) == found.len(),
        "no orphan actors accumulate"
    );
}

#[test]
fn restart_removes_the_old_crowd_and_fields_a_fresh_one() {
    let tmp = install();
    let mut app = app(tmp.path(), 4, Some(0.5));
    run(&mut app, 3);
    let first = sorted_positions(&mut app);
    assert_eq!(first.len(), 24);
    let old = app.world().resource::<Session>().generation();

    // Teardown as the session does it: Unloading despawns the
    // session-owned roots and drops the crowd resource.
    {
        let mut s = app.world_mut().resource_mut::<Session>();
        s.transition(SessionPhase::Unloading).unwrap();
    }
    app.update();
    app.world_mut().remove_resource::<PedCrowd>();
    assert_eq!(actors(&mut app), 0, "nothing survives the unload");

    {
        let mut s = app.world_mut().resource_mut::<Session>();
        s.transition(SessionPhase::Menu).unwrap();
        s.begin(config(4)).unwrap();
        s.transition(SessionPhase::Ready).unwrap();
        s.transition(SessionPhase::Playing).unwrap();
    }
    run(&mut app, 3);
    let new = app.world().resource::<Session>().generation();
    assert_ne!(old, new);
    assert_eq!(app.world().resource::<PedCrowd>().generation(), new);
    assert_eq!(actors(&mut app), 24, "the target again, not doubled");
    assert_eq!(sorted_positions(&mut app), first, "same seed, same crowd");
}

#[test]
fn a_stale_crowd_from_another_generation_is_rebuilt_not_reused() {
    let tmp = install();
    let mut app = app(tmp.path(), 4, Some(0.5));
    run(&mut app, 3);
    let old = app.world().resource::<Session>().generation();
    {
        let mut s = app.world_mut().resource_mut::<Session>();
        s.transition(SessionPhase::Unloading).unwrap();
    }
    app.update();
    {
        let mut s = app.world_mut().resource_mut::<Session>();
        s.transition(SessionPhase::Menu).unwrap();
        s.begin(config(4)).unwrap();
        s.transition(SessionPhase::Ready).unwrap();
        s.transition(SessionPhase::Playing).unwrap();
    }
    run(&mut app, 3);
    let crowd = app.world().resource::<PedCrowd>();
    assert_ne!(crowd.generation(), old);
    assert_eq!(actors(&mut app), 24);
}

/// A reacting archetype that walks at 3 m/s, so a walk back is quick.
fn reactive() -> tempfile::TempDir {
    install_with(&REACTIVE_CSV.replace("WALK,xwalk,1,4,0,1,", "WALK,xwalk,1,4,0,0.4,"))
}

/// A car (default chassis, 1.85 × 4.4 m) the test steps by hand: a
/// player-driven car, so walkers sense it (F19/UNK-43).
fn spawn_car(app: &mut App, at: Vec3, velocity: Vec3) -> Entity {
    spawn_car_with(app, at, velocity, PlayerControl::Local, 0.0)
}

/// A car the test steps by hand with an explicit driver and steering
/// input (F19/UNK-43: only non-AI players are sensed, and the dive side
/// follows the sensed car's own steering).
fn spawn_car_with(
    app: &mut App,
    at: Vec3,
    velocity: Vec3,
    control: PlayerControl,
    steering: f32,
) -> Entity {
    app.world_mut()
        .spawn((
            Vehicle {
                config: Default::default(),
            },
            VehicleInput {
                steering,
                ..Default::default()
            },
            Player {
                id: PlayerId(0),
                control,
            },
            Position(at),
            LinearVelocity(velocity),
        ))
        .id()
}

fn step_car(app: &mut App, car: Entity) {
    let dt = 1.0 / 60.0;
    let mut e = app.world_mut().entity_mut(car);
    let v = e.get::<LinearVelocity>().unwrap().0;
    e.get_mut::<Position>().unwrap().0 += v * dt;
}

/// A real dynamic car (the default chassis) placed for Avian to
/// integrate: the production bundle plus a `Transform` (for rendering
/// and propagated bounds) and a `Position`. The initial velocity is set
/// through a follow-up insert because the bundle already carries
/// `LinearVelocity::ZERO`. It carries the player markers, so walkers
/// sense it (F19/UNK-43).
fn spawn_physics_car(app: &mut App, at: Vec3, velocity: Vec3) -> Entity {
    let car = app
        .world_mut()
        .spawn((
            vehicle_bundle(&VehicleConfig::default()),
            Player {
                id: PlayerId(0),
                control: PlayerControl::Local,
            },
            Transform::from_translation(at),
            Position(at),
            Rotation(Quat::IDENTITY),
        ))
        .id();
    app.world_mut()
        .entity_mut(car)
        .insert(LinearVelocity(velocity));
    car
}

fn first_walker(app: &mut App) -> Entity {
    let mut q = app.world_mut().query_filtered::<Entity, With<PedWalk>>();
    q.iter(app.world()).min().unwrap()
}

fn phase(app: &App, walker: Entity) -> Phase {
    app.world()
        .entity(walker)
        .get::<PedReact>()
        .unwrap()
        .reaction
        .phase
}

fn at(app: &App, walker: Entity) -> Transform {
    *app.world().entity(walker).get::<Transform>().unwrap()
}

/// What one drive-at-a-walker run observed (F19-B.8b).
#[derive(Debug)]
struct Approach {
    /// Closest the car's centre came to the walker's, horizontally.
    min_gap: f32,
    /// The car's end z minus the walker's start z: how far past the
    /// walker's line the car got.
    car_progress: f32,
    /// Greatest lateral offset the walker reached from where it started.
    lateral: f32,
    /// Whether the walker stayed on the ground the whole run.
    grounded: bool,
    /// Whether every sampled position stayed finite.
    finite: bool,
    /// Whether the walker ever left its curve (the authored avoidance ran).
    reacted: bool,
    /// Greatest gap between the walker's `Transform` and its physics
    /// `Position` at a sample: the body rides the authored transform
    /// (F19-B.8b) one physics step behind it, never stranded at spawn and
    /// never moved by the car.
    body_lag: f32,
    dives: u64,
}

/// Drive a real dynamic car straight at the first walker along its
/// sidewalk line — from 6 m back at 15 m/s, the same shape as the
/// existing hand-stepped car tests — for 5 s and report the closest
/// approach and the walker's response.
fn drive_at_first_walker(dir: &Path, seed: u64) -> Approach {
    let mut app = physics_app(dir, seed, 0.1);
    run(&mut app, 3);
    let walker = first_walker(&mut app);
    let start = at(&app, walker).translation;
    let car = spawn_physics_car(
        &mut app,
        Vec3::new(start.x, 0.3, start.z - 6.0),
        Vec3::new(0.0, 0.0, 15.0),
    );
    let mut r = Approach {
        min_gap: f32::INFINITY,
        car_progress: f32::NEG_INFINITY,
        lateral: 0.0,
        grounded: true,
        finite: true,
        reacted: false,
        body_lag: 0.0,
        dives: 0,
    };
    for _ in 0..300 {
        app.update();
        let now = at(&app, walker).translation;
        let body = app.world().entity(walker).get::<Position>().map(|p| p.0);
        let car_pos = app.world().entity(car).get::<Position>().unwrap().0;
        if let Some(body) = body {
            r.body_lag = r.body_lag.max((body - now).length());
        }
        r.min_gap = r
            .min_gap
            .min(Vec3::new(car_pos.x - now.x, 0.0, car_pos.z - now.z).length());
        r.car_progress = r.car_progress.max(car_pos.z - start.z);
        r.lateral = r.lateral.max((now.x - start.x).abs());
        r.grounded &= now.y.abs() < 0.1;
        r.finite &= now.is_finite() && car_pos.is_finite();
        r.reacted |= phase(&app, walker) != Phase::Walking;
    }
    let crowd = app.world().resource::<PedCrowd>();
    r.dives = crowd.dives;
    r
}

#[test]
fn a_car_bearing_down_makes_a_walker_look_dive_clear_and_walk_back() {
    let tmp = reactive();
    let mut app = app(tmp.path(), 7, Some(0.1));
    run(&mut app, 3);
    let walker = first_walker(&mut app);
    let start = at(&app, walker).translation;
    let curve_x = start.x;
    // 40 m up the walker's sidewalk, driving at it at 15 m/s.
    let car = spawn_car(
        &mut app,
        start + Vec3::new(0.0, 0.0, -40.0),
        Vec3::new(0.0, 0.0, 15.0),
    );
    let mut seen = vec![Phase::Walking];
    let mut widest = 0.0f32;
    let mut facing_at_dive = None;
    for _ in 0..240 {
        step_car(&mut app, car);
        app.update();
        let p = phase(&app, walker);
        if *seen.last().unwrap() != p {
            if p == Phase::Diving {
                let t = at(&app, walker);
                facing_at_dive = Some((t.rotation * Vec3::NEG_Z).dot(Vec3::NEG_Z));
            }
            seen.push(p);
        }
        widest = widest.max((at(&app, walker).translation.x - curve_x).abs());
    }
    app.world_mut().despawn(car);
    run(&mut app, 900);
    assert_eq!(
        seen,
        [
            Phase::Walking,
            Phase::Wary,
            Phase::Diving,
            Phase::Rejoining,
            Phase::Walking
        ],
        "stop and look, dive, walk back, carry on"
    );
    assert!(
        facing_at_dive.unwrap() > 0.9,
        "it faced the oncoming car: {facing_at_dive:?}"
    );
    assert!(
        (4.2..4.5).contains(&widest),
        "the authored chain carries it ~4.4 m across the car's line: {widest}"
    );
    assert_eq!(phase(&app, walker), Phase::Walking, "back on its curve");
    let end = at(&app, walker).translation;
    assert!(
        (end.x - curve_x).abs() < 0.3,
        "rejoined its sidewalk: {end:?}"
    );
    let crowd = app.world().resource::<PedCrowd>();
    assert!(crowd.alerts >= 1 && crowd.dives >= 1 && crowd.rejoined >= 1);
    let found = walkers(&mut app);
    assert!(found.iter().all(|(t, _)| t.is_finite()));
    assert_eq!(actors(&mut app), found.len(), "no actor leaked");
}

/// Drive the hand-stepped car straight at the first walker from 40 m at
/// 15 m/s for 4 s and report the crowd's reaction counters plus the
/// first authored dive state the walker entered. `control` and
/// `steering` are the sensed car's driver and steering input.
fn drive_reactions(seed: u64, control: PlayerControl, steering: f32) -> (u64, u64, Option<String>) {
    let tmp = reactive();
    let mut app = app(tmp.path(), seed, Some(0.1));
    run(&mut app, 3);
    let walker = first_walker(&mut app);
    let start = at(&app, walker).translation;
    let car = spawn_car_with(
        &mut app,
        start + Vec3::new(0.0, 0.0, -40.0),
        Vec3::new(0.0, 0.0, 15.0),
        control,
        steering,
    );
    let mut dive = None;
    for _ in 0..240 {
        step_car(&mut app, car);
        app.update();
        if dive.is_none() {
            dive = app
                .world()
                .entity(walker)
                .get::<PedActor>()
                .map(|a| a.state().to_string())
                .filter(|s| s.contains("DIVE"));
        }
    }
    let crowd = app.world().resource::<PedCrowd>();
    (crowd.alerts, crowd.dives, dive)
}

/// F19/UNK-43: the walker senses the original's *player* cars only
/// (`0x6b1f60` nodes carry a player-slot index). The identical drive is
/// sensed whether the driver is local or a networked human, but an AI
/// opponent is not in the player table and never scares anyone.
#[test]
fn only_player_driven_cars_scare_the_crowd() {
    let (_, local_dives, _) = drive_reactions(7, PlayerControl::Local, 0.0);
    assert!(
        local_dives >= 1,
        "the local player's car makes the walker dive: {local_dives}"
    );
    let (_, remote_dives, _) = drive_reactions(7, PlayerControl::Remote, 0.0);
    assert!(
        remote_dives >= 1,
        "a networked human's car is still a player car: {remote_dives}"
    );
    let (ai_alerts, ai_dives, ai_dive) = drive_reactions(7, PlayerControl::Ai, 0.0);
    assert_eq!(
        (ai_alerts, ai_dives),
        (0, 0),
        "an AI opponent is not in the player table (dive {ai_dive:?})"
    );
}

/// F19/UNK-43: the dive side follows the *sensed car's* steering past
/// full lock (`0x54dc3f`: `> 0.85` right, `< -0.85` left, both strict).
/// The same approach differing only in the sign of the car's steering
/// must therefore pick opposite dive states — the own-side fallback is
/// the same for both, so the sign is the only thing that can flip them.
#[test]
fn a_hard_steering_car_picks_the_walkers_dive_side() {
    let (_, right_dives, right) = drive_reactions(7, PlayerControl::Local, 1.0);
    let (_, left_dives, left) = drive_reactions(7, PlayerControl::Local, -1.0);
    assert!(
        right_dives >= 1 && left_dives >= 1,
        "both cars made the walker dive: {right_dives}, {left_dives}"
    );
    assert_eq!(
        right.as_deref(),
        Some("ANTIC_RDIVE"),
        "steering +1 (right lock) dives right"
    );
    assert_eq!(
        left.as_deref(),
        Some("ANTIC_LDIVE"),
        "steering -1 (left lock) dives left"
    );
}

/// F19-B.9/AUD-15: the reaction *decisions* are what write the walker
/// scream stream — the wary/back-away entry and the dive selection, and
/// nothing else. Every scream coincides with a `Wary`/`Diving` phase and
/// the counts match the crowd's own alert/dive tallies one for one.
#[test]
fn a_wary_or_diving_walker_writes_a_scream_per_transition() {
    let tmp = reactive();
    let mut app = app(tmp.path(), 7, Some(0.1));
    run(&mut app, 3);
    let walker = first_walker(&mut app);
    let start = at(&app, walker).translation;
    let car = spawn_car(
        &mut app,
        start + Vec3::new(0.0, 0.0, -40.0),
        Vec3::new(0.0, 0.0, 15.0),
    );
    let (mut alerts, mut dives) = (0u64, 0u64);
    for _ in 0..240 {
        step_car(&mut app, car);
        app.update();
        let msgs = app
            .world()
            .resource::<Messages<mm2_app::audio::PedScream>>();
        for s in msgs.iter_current_update_messages() {
            // The synthetic install ships only `pedmodel_man`, whose stem
            // carries no "woman": the male voice table.
            assert!(!s.female, "pedmodel_man reads the male voice table");
            assert!(s.position.is_finite(), "a real walker position");
            match phase(&app, s.walker) {
                Phase::Wary => alerts += 1,
                Phase::Diving => dives += 1,
                other => panic!("a scream outside a dive/back-away: {other:?}"),
            }
        }
    }
    let crowd = app.world().resource::<PedCrowd>();
    assert_eq!(alerts, crowd.alerts, "one scream per back-away decision");
    assert_eq!(dives, crowd.dives, "one scream per dive decision");
    assert!(
        alerts >= 1 && dives >= 1,
        "the car made it react: {} alert(s), {} dive(s)",
        crowd.alerts,
        crowd.dives
    );
}

#[test]
fn a_car_passing_in_its_lane_or_standing_still_does_not_disturb_the_crowd() {
    let tmp = reactive();
    let mut app = app(tmp.path(), 7, Some(0.5));
    run(&mut app, 3);
    // Down the road centre, 8.5 m from either sidewalk.
    let car = spawn_car(
        &mut app,
        Vec3::new(0.0, 0.0, -95.0),
        Vec3::new(0.0, 0.0, 25.0),
    );
    // And a car parked right on a sidewalk.
    spawn_car(&mut app, Vec3::new(8.5, 0.0, 30.0), Vec3::ZERO);
    for _ in 0..480 {
        step_car(&mut app, car);
        app.update();
    }
    let crowd = app.world().resource::<PedCrowd>();
    assert_eq!((crowd.alerts, crowd.dives), (0, 0));
}

/// F19/UNK-43 (`0x54ba4a`): the walker senses only the nearest
/// player-controlled car, so a parked car beside it hides a car bearing
/// down from farther away; an AI car is not in the table at all.
#[test]
fn a_walker_senses_only_the_nearest_player_car() {
    let tmp = reactive();
    // Drive a car at the first walker from 50 m with `extra` placed at
    // `offset` from it; report whether that walker (alerted, dived).
    let run_with = |extra: Option<(Vec3, PlayerControl)>| {
        let mut app = app(tmp.path(), 7, Some(0.1));
        run(&mut app, 3);
        let walker = first_walker(&mut app);
        let start = at(&app, walker).translation;
        let car = spawn_car(
            &mut app,
            start + Vec3::new(0.0, 0.0, -50.0),
            Vec3::new(0.0, 0.0, 15.0),
        );
        if let Some((offset, control)) = extra {
            let parked = spawn_car(&mut app, start + offset, Vec3::ZERO);
            app.world_mut().entity_mut(parked).insert(Player {
                id: PlayerId(2),
                control,
            });
        }
        // Other walkers far from the parked car still sense the mover.
        let mut dived = false;
        let mut alerted = false;
        // Stop with the mover still 7.5 m out, farther than the 4 m
        // parked car: past that it would become the nearest.
        for _ in 0..170 {
            step_car(&mut app, car);
            app.update();
            let p = phase(&app, walker);
            alerted |= p != Phase::Walking;
            dived |= p == Phase::Diving;
        }
        (alerted, dived)
    };
    let off = Vec3::new(0.0, 0.0, 4.0);
    assert_eq!(run_with(None), (true, true), "control: a real threat");
    assert_eq!(run_with(Some((off, PlayerControl::Local))), (false, false));
    assert_eq!(run_with(Some((off, PlayerControl::Remote))), (false, false));
    assert_eq!(
        run_with(Some((off, PlayerControl::Ai))),
        (true, true),
        "an AI car is not sensed"
    );
}

#[test]
fn an_archetype_without_the_reaction_states_walks_on_obliviously() {
    let tmp = install();
    let mut app = app(tmp.path(), 7, Some(0.1));
    run(&mut app, 3);
    let walker = first_walker(&mut app);
    let start = at(&app, walker).translation;
    let car = spawn_car(
        &mut app,
        start + Vec3::new(0.0, 0.0, -40.0),
        Vec3::new(0.0, 0.0, 15.0),
    );
    for _ in 0..240 {
        step_car(&mut app, car);
        app.update();
        assert_eq!(phase(&app, walker), Phase::Walking);
        assert!((at(&app, walker).translation.x - start.x).abs() < 0.01);
    }
    let crowd = app.world().resource::<PedCrowd>();
    assert_eq!((crowd.alerts, crowd.dives), (0, 0));
}

#[test]
fn a_pause_freezes_a_walker_mid_dive() {
    let tmp = reactive();
    let mut app = app(tmp.path(), 7, Some(0.1));
    run(&mut app, 3);
    let walker = first_walker(&mut app);
    let start = at(&app, walker).translation;
    let car = spawn_car(
        &mut app,
        start + Vec3::new(0.0, 0.0, -25.0),
        Vec3::new(0.0, 0.0, 15.0),
    );
    while phase(&app, walker) != Phase::Diving {
        step_car(&mut app, car);
        app.update();
        assert!(app.world().resource::<Time>().elapsed_secs() < 10.0);
    }
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    app.update();
    let frozen = at(&app, walker);
    for _ in 0..60 {
        step_car(&mut app, car);
        app.update();
    }
    assert_eq!(at(&app, walker), frozen, "paused mid-dive");
    assert_eq!(phase(&app, walker), Phase::Diving);
}

/// Every count a soak bounds, sampled after one frame.
#[derive(Debug, PartialEq)]
struct Census {
    walkers: usize,
    actors: usize,
    meshes: usize,
    entities: usize,
    finite: bool,
}

fn census(app: &mut App) -> Census {
    let found = walkers(app);
    Census {
        walkers: found.len(),
        actors: actors(app),
        meshes: app.world().resource::<Assets<Mesh>>().len(),
        entities: app.world_mut().query::<Entity>().iter(app.world()).count(),
        finite: found.iter().all(|(t, _)| t.is_finite()),
    }
}

/// Slack above the settled mesh count for the frame a recycled figure's
/// handles and its replacement's overlap in `Assets<Mesh>`.
const MESH_TRANSIENT: usize = 8;

/// One minute of driving up and down the road at 20 m/s with a car
/// running on the sidewalk line 20 m ahead, so the crowd recycles
/// constantly and the reactions fire. Every tick after the crowd has
/// filled is held to the settled census taken once it did: meshes may
/// not grow past it by more than the transient overlap, and entities
/// may only grow by the per-figure entities of the walkers gained.
/// Returns the peak census, the final one, the end state's fingerprint
/// and the counters.
fn soak(dir: &Path, seed: u64) -> (Census, Census, Vec<[i32; 3]>, [u64; 4]) {
    /// Ticks to let the bubble fill before taking the baseline.
    const SETTLE: u32 = 60;
    let mut app = app(dir, seed, Some(1.0));
    run(&mut app, 3);
    let car = spawn_car(&mut app, Vec3::new(8.5, 0.0, -70.0), Vec3::ZERO);
    let first = census(&mut app);
    assert!(first.walkers > 0 && first.meshes > 0, "{first:?}");
    let mut settled = None;
    let mut entities_per_figure = 0;
    let mut peak = Census {
        walkers: 0,
        actors: 0,
        meshes: 0,
        entities: 0,
        finite: true,
    };
    for tick in 0..3600u32 {
        // A triangle wave: 90 m out, 90 m back, 9 s per leg.
        let leg = (tick as f32 / 60.0 * 20.0) % 360.0;
        let z = if leg < 180.0 { leg - 90.0 } else { 270.0 - leg };
        let player = Vec3::new(0.0, 0.0, z);
        move_player(&mut app, player);
        let ahead = if leg < 180.0 { 1.0 } else { -1.0 };
        let mut e = app.world_mut().entity_mut(car);
        e.get_mut::<Position>().unwrap().0 = Vec3::new(8.5, 0.0, z + 20.0 * ahead);
        e.get_mut::<LinearVelocity>().unwrap().0 = Vec3::new(0.0, 0.0, 20.0 * ahead);
        app.update();
        let now = census(&mut app);
        assert_eq!(now.walkers, now.actors, "tick {tick}: no orphan actors");
        assert!(now.finite, "tick {tick}: every walker stays finite");
        if tick + 1 == SETTLE {
            // A figure is a root plus one child per mesh group, and
            // each group owns one mesh.
            entities_per_figure = 1 + now.meshes.div_ceil(now.walkers);
            settled = Some(Census { ..now });
        } else if let Some(base) = &settled {
            assert!(
                now.meshes <= base.meshes + MESH_TRANSIENT,
                "tick {tick}: meshes leaked past the settled crowd: {base:?} {now:?}"
            );
            let allowed =
                base.entities + entities_per_figure * now.walkers.saturating_sub(base.walkers);
            assert!(
                now.entities <= allowed,
                "tick {tick}: entities accumulated: {base:?} {now:?} (+{entities_per_figure}/figure)"
            );
        }
        peak = Census {
            walkers: peak.walkers.max(now.walkers),
            actors: peak.actors.max(now.actors),
            meshes: peak.meshes.max(now.meshes),
            entities: peak.entities.max(now.entities),
            finite: peak.finite && now.finite,
        };
    }
    let crowd = app.world().resource::<PedCrowd>();
    let counters = [
        crowd.recycled as u64,
        crowd.spawned as u64,
        crowd.alerts,
        crowd.dives,
    ];
    let last = census(&mut app);
    (peak, last, sorted_positions(&mut app), counters)
}

#[test]
fn a_crowded_minute_of_driving_stays_finite_and_inside_the_actor_and_mesh_budgets() {
    let tmp = reactive();
    let (peak, last, _, [recycled, spawned, alerts, dives]) = soak(tmp.path(), 5);
    // density 1.0 is the walk policy's cap (48), itself under the
    // actor ceiling.
    assert!(peak.walkers <= 48, "{peak:?}");
    assert!(peak.walkers <= mm2_app::pedestrian::MAX_PED_ACTORS);
    assert!(last.walkers >= 24, "the bubble is kept populated: {last:?}");
    // The soak really recycled and really provoked reactions.
    assert!(recycled > 100 && spawned > recycled, "{recycled} {spawned}");
    assert!(alerts > 0 && dives > 0, "{alerts} {dives}");
}

#[test]
fn a_crowded_minute_replays_identically_from_the_same_seed() {
    let tmp = reactive();
    let a = soak(tmp.path(), 5);
    let b = soak(tmp.path(), 5);
    assert_eq!(a.1, b.1);
    assert_eq!(a.2, b.2, "the same seed, the same crowd after a minute");
    assert_eq!(a.3, b.3, "and the same counters");
}

#[test]
fn two_cars_closing_on_one_walker_from_both_ends_still_let_it_dive_and_walk_back() {
    let tmp = reactive();
    let mut app = app(tmp.path(), 7, Some(0.1));
    run(&mut app, 3);
    let walker = first_walker(&mut app);
    let start = at(&app, walker).translation;
    let ahead = spawn_car(
        &mut app,
        start + Vec3::new(0.0, 0.0, 50.0),
        Vec3::new(0.0, 0.0, -15.0),
    );
    let behind = spawn_car(
        &mut app,
        start + Vec3::new(0.0, 0.0, -50.0),
        Vec3::new(0.0, 0.0, 15.0),
    );
    let mut dived = false;
    for _ in 0..300 {
        step_car(&mut app, ahead);
        step_car(&mut app, behind);
        app.update();
        dived |= phase(&app, walker) == Phase::Diving;
        assert!(at(&app, walker).translation.is_finite());
    }
    app.world_mut().despawn(ahead);
    app.world_mut().despawn(behind);
    run(&mut app, 900);
    assert!(dived, "a walker caught between two cars still dives");
    assert_eq!(phase(&app, walker), Phase::Walking, "and recovers");
    assert!((at(&app, walker).translation.x - start.x).abs() < 0.3);
    let found = walkers(&mut app);
    assert_eq!(actors(&mut app), found.len());
}

/// F19-B.8b: the retail narrow phase is live — a car that catches a
/// walker contacts its body capsule instead of driving through the
/// figure, while the walker's authored avoidance still runs. The
/// documented deterministic scenario: seed 7, density 0.1, the
/// stunted-dive archetype of [`crate::pedestrian::CONTACT_CSV`], and a
/// real dynamic default car released 6 m behind the first walker at
/// 15 m/s (the same shape as the hand-stepped car tests above). The car
/// is stopped at the body; with the body-less figure the identical
/// release drives straight through, which is what proves the block is
/// the collider and not the reaction.
#[test]
fn a_car_catching_a_walker_contacts_its_body_and_its_authored_avoidance_still_runs() {
    let solid = install_solid(CONTACT_CSV);
    let blocked = drive_at_first_walker(solid.path(), 7);
    let flat = install_flat(CONTACT_CSV);
    let through = drive_at_first_walker(flat.path(), 7);

    assert!(
        blocked.finite && blocked.grounded,
        "no run-over: {blocked:?}"
    );
    assert!(
        blocked.reacted && blocked.dives >= 1,
        "the walker's authored avoidance ran: {blocked:?}"
    );
    assert!(
        blocked.body_lag < 0.5,
        "the body rides the authored transform instead of stranding at spawn \
         (a kinematic body is one physics step behind it, ~0.13 m at this \
         fixture's walk speed): {blocked:?}"
    );
    assert!(through.reacted, "the contrast arm reacts too: {through:?}");
    assert!(
        blocked.lateral < 1.0,
        "the walker stays within its authored dive envelope: {blocked:?}"
    );
    // The car's centre comes to rest one chassis (2.2 m) and one torso
    // radius (0.25 m) short of the walker — the contact surface — not
    // through it.
    assert!(
        (2.0..3.0).contains(&blocked.min_gap),
        "the car stopped at the walker's body: {blocked:?}"
    );
    assert!(
        blocked.car_progress < 0.0,
        "the body kept the car behind the walker's line: {blocked:?}"
    );
    // The identical release through a body-less figure interpenetrates
    // and drives on, so the block is the collider, not the reaction.
    assert!(
        through.min_gap < 0.5,
        "without a body the same release passes through: {through:?}"
    );
    assert!(
        through.car_progress > 10.0,
        "and the car keeps its speed: {through:?}"
    );
}

/// F19-B.5: the crowd is cosmetic, so a `Remote` client fields it too.
#[test]
fn a_remote_client_fields_the_same_crowd_a_host_would_for_its_seed() {
    let tmp = install();
    let mut local = app(tmp.path(), 11, Some(0.5));
    let mut client = app_as(tmp.path(), 11, Some(0.5), SessionAuthority::Remote);
    run(&mut local, 5);
    run(&mut client, 5);
    assert_eq!(
        actors(&mut client),
        24,
        "round(0.5 * 48), not an empty city"
    );
    assert_eq!(
        sorted_positions(&mut client),
        sorted_positions(&mut local),
        "same density, seed and spawn: the same crowd on either role"
    );
    assert!(client.world().resource::<PedCrowd>().is_active());

    // It walks and recycles around the client's own players.
    let before = sorted_positions(&mut client);
    run(&mut client, 120);
    assert_ne!(sorted_positions(&mut client), before, "the crowd walks");
    let player = Vec3::new(0.0, 0.0, 95.0);
    move_player(&mut client, player);
    run(&mut client, 120);
    let found = walkers(&mut client);
    assert!(!found.is_empty());
    for (at, _) in &found {
        assert!(at.distance(player) <= 91.0, "inside the bubble: {at:?}");
    }
    assert!(client.world().resource::<PedCrowd>().recycled > 0);
    assert_eq!(actors(&mut client), 24, "no orphan figures");
}

/// A client's copy of a remote car (kinematic, with the replicated
/// velocity) is a threat like the player's own: the client's walkers
/// look, dive clear and walk back. (A `Remote` client cannot pause —
/// MP-6 — so the host-side pause tests cover freezing.)
#[test]
fn a_remote_clients_walkers_dive_from_a_replicated_car_and_walk_back() {
    let tmp = reactive();
    let mut app = app_as(tmp.path(), 7, Some(0.1), SessionAuthority::Remote);
    run(&mut app, 3);
    let walker = first_walker(&mut app);
    let start = at(&app, walker).translation;
    let car = spawn_car_with(
        &mut app,
        start + Vec3::new(0.0, 0.0, -40.0),
        Vec3::new(0.0, 0.0, 15.0),
        PlayerControl::Remote,
        0.0,
    );
    let mut dived = false;
    for _ in 0..240 {
        step_car(&mut app, car);
        app.update();
        dived |= phase(&app, walker) == Phase::Diving;
    }
    assert!(dived, "the client's walker dove from the replicated car");
    app.world_mut().despawn(car);
    run(&mut app, 900);
    assert_eq!(phase(&app, walker), Phase::Walking, "back on its curve");
    assert!((at(&app, walker).translation.x - start.x).abs() < 0.3);
    let crowd = app.world().resource::<PedCrowd>();
    assert!(crowd.alerts >= 1 && crowd.dives >= 1 && crowd.rejoined >= 1);
    assert_eq!(actors(&mut app), walkers(&mut app).len(), "no actor leaked");
}
