//! F19-B.2 sidewalk crowd through the production systems: a synthetic
//! city (one 200 m road with a sidewalk each side) beside the synthetic
//! pedestrian archetype, populated, walked, recycled and reset by the
//! same `maintain_pedestrians` / `walk_pedestrians` /
//! `animate_pedestrians` the app schedules.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::{LinearVelocity, Position};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::crowd::{
    PedCrowd, PedDensity, PedReact, PedWalk, maintain_pedestrians, react_pedestrians,
    walk_pedestrians,
};
use mm2_app::pedestrian::{PedActor, animate_pedestrians};
use mm2_assets::Vfs;
use mm2_game::{
    Mm2Vfs, Player, PlayerControl, PlayerId, Session, SessionConfig, SessionEntity, SessionPhase,
    WorldMode, despawn_session_entities,
};

use mm2_game::pedreact::Phase;
use mm2_vehicle::vehicle::Vehicle;

use crate::pedestrian::{REACTIVE_CSV, install, install_with};

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

fn playing(seed: u64) -> Session {
    let mut session = Session::new();
    session.begin(config(seed)).unwrap();
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
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(TransformPlugin)
        .init_asset::<StandardMaterial>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(playing(seed))
        .insert_resource(mount(dir))
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

/// A car (default chassis, 1.85 × 4.4 m) the test steps by hand.
fn spawn_car(app: &mut App, at: Vec3, velocity: Vec3) -> Entity {
    app.world_mut()
        .spawn((
            Vehicle {
                config: Default::default(),
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
