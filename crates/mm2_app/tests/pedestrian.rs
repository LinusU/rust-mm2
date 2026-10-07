//! F19-A.5 pedestrian actors through the production lab systems: a
//! synthetic archetype on a mounted install, a ground collider on the
//! world layer and a player vehicle, driven by the same
//! `spawn_ped_lab` / `advance_ped_lab` / `animate_pedestrians` the app
//! schedules.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::layers::GameLayer;
use mm2_app::pedestrian::{
    LAB_HOLD_SECS, PedActor, PedLabSpawned, advance_ped_lab, animate_pedestrians, lab_enabled,
    spawn_ped_lab,
};
use mm2_assets::Vfs;
use mm2_game::{DevOverrides, Mm2Vfs, PlayerVehicle, Session, SessionConfig, SessionPhase};

const SKEL: &str = "NumBones 3\nbone root {\n\toffset 0 1 0\n\tbone a {\n\t\toffset 0 0.5 0\n\t}\n\tbone b {\n\t\toffset 0 0 0.5\n\t}\n}\n";
const CSV: &str = "# header\nSTAND,xstand,1,4,0,0,0,0,STAND\nWALK,xwalk,1,4,0,1,0,0,WALK\n";
const MOD: &str = "\
version: 1.09
verts: 3
normals: 3
colors: 1
tex1s: 1
tex2s: 0
tangents: 0
materials: 2
adjuncts: 3
primitives: 2
matrices: 3

v	0.0	0.0	0.0
v	0.0	-1.0	0.0
v	0.0	1.0	0.0
n	0.0	0.0	1.0
n	0.0	0.0	1.0
n	0.0	0.0	1.0
c	1.0	1.0	1.0	1.0
t1	0.5	0.5

mtl Test1:SKIN {
	packets:	1
	primitives:	1
	textures:	0
	illum: diffuse
	ambient:	0.4 0.3 0.2
	diffuse:	0.7 0.6 0.5
	specular:	0.8 0.7 0.6
}

mtl Test1:HAIR {
	packets:	1
	primitives:	1
	textures:	0
	illum: diffuse
	ambient:	0.1 0.0 0.0
	diffuse:	0.4 0.1 0.1
	specular:	0.6 0.4 0.4
}

packet 2 1 2 {
	adj	0	0	0	0	0	0
	adj	1	1	0	0	0	1
	tri	0	1	0
	mtx 0 1
}

packet 1 1 1 {
	adj	2	2	0	0	0	0
	tri	0	0	0
	mtx 2
}

mtxv 1 1 1
mtxn 1 1 1
";

/// Four frames; bone `a`'s Z rotation grows with the frame so a posed
/// vertex bound to it moves as the clip plays.
fn clip() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&4u32.to_le_bytes());
    b.extend_from_slice(&12u32.to_le_bytes());
    b.extend_from_slice(&0f32.to_le_bytes());
    b.push(1);
    for f in 0..4 {
        let mut floats = [0.0f32; 12];
        floats[1] = 1.0; // root height
        floats[5] = 0.4 * f as f32; // bone 1 rz
        for v in floats {
            b.extend_from_slice(&v.to_le_bytes());
        }
    }
    b
}

fn shaders() -> Vec<u8> {
    let mut s = Vec::new();
    s.extend_from_slice(&1u32.to_le_bytes());
    s.extend_from_slice(&2u32.to_le_bytes());
    for _ in 0..2 {
        s.push(0);
        for c in [0.5f32; 16] {
            s.extend_from_slice(&c.to_le_bytes());
        }
        s.extend_from_slice(&4f32.to_le_bytes());
    }
    s
}

fn install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let put = |name: &str, bytes: &[u8]| {
        let p = d.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    };
    put("anim/pedmodel_man.skel", SKEL.as_bytes());
    put("anim/pedmodel_man.csv", CSV.as_bytes());
    put("anim/pedmodel_man.mod", MOD.as_bytes());
    put("anim/pedmodel_man.shaders", &shaders());
    put("anim/xstand.anim", &clip());
    put("anim/xwalk.anim", &clip());
    tmp
}

fn app(dir: &Path, lab: bool) -> App {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    let mut session = Session::new();
    session
        .begin(SessionConfig {
            dev: DevOverrides {
                ped_lab: lab,
                ..DevOverrides::default()
            },
            ..SessionConfig::default()
        })
        .unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();

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
        .insert_resource(session)
        .insert_resource(Mm2Vfs(vfs))
        .init_resource::<PedLabSpawned>()
        .add_systems(
            Update,
            (
                spawn_ped_lab.run_if(lab_enabled),
                advance_ped_lab.run_if(lab_enabled),
                animate_pedestrians,
            )
                .chain(),
        );
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(200.0, 1.0, 200.0),
        GameLayer::world(),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    app.world_mut()
        .spawn((PlayerVehicle, Transform::from_xyz(0.0, 0.6, 0.0)));
    app.finish();
    app.cleanup();
    app
}

fn run(app: &mut App, n: usize) {
    for _ in 0..n {
        app.update();
    }
}

/// Every actor's mesh positions, flattened.
fn positions(app: &mut App) -> Vec<f32> {
    let handles: Vec<Handle<Mesh>> = app
        .world_mut()
        .query::<&Mesh3d>()
        .iter(app.world())
        .map(|m| m.0.clone())
        .collect();
    let meshes = app.world().resource::<Assets<Mesh>>();
    handles
        .iter()
        .flat_map(|h| {
            meshes
                .get(h)
                .and_then(|m| m.attribute(Mesh::ATTRIBUTE_POSITION))
                .and_then(|a| a.as_float3())
                .unwrap()
                .iter()
                .flatten()
                .copied()
                .collect::<Vec<_>>()
        })
        .collect()
}

fn actors(app: &mut App) -> usize {
    app.world_mut()
        .query::<&PedActor>()
        .iter(app.world())
        .count()
}

#[test]
fn the_lab_stays_empty_unless_the_session_asked() {
    let tmp = install();
    let mut app = app(tmp.path(), false);
    run(&mut app, 30);
    assert_eq!(actors(&mut app), 0);
}

#[test]
fn the_lab_spawns_each_loadable_archetype_once_on_the_ground_ahead_of_the_player() {
    let tmp = install();
    let mut app = app(tmp.path(), true);
    run(&mut app, 30);
    // Only `pedmodel_man` ships here; the other stock stems are skipped
    // with a warning, never substituted.
    assert_eq!(actors(&mut app), 1);
    let at = app
        .world_mut()
        .query_filtered::<&Transform, With<PedActor>>()
        .single(app.world())
        .unwrap()
        .translation;
    assert!(at.y.abs() < 0.05, "stands on the collider's top: {at:?}");
    assert!(at.z < -5.0, "ahead of a player facing -Z: {at:?}");
    run(&mut app, 30);
    assert_eq!(actors(&mut app), 1, "one line-up per session generation");
}

#[test]
fn figures_deform_while_playing_and_freeze_while_paused() {
    let tmp = install();
    let mut app = app(tmp.path(), true);
    run(&mut app, 10);
    assert_eq!(actors(&mut app), 1);
    let before = positions(&mut app);
    run(&mut app, 12);
    let moved = positions(&mut app);
    assert_ne!(before, moved, "the clip's bone rotation re-poses the skin");
    assert!(moved.iter().all(|v| v.is_finite()));

    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    app.update();
    let frozen = positions(&mut app);
    run(&mut app, 30);
    assert_eq!(frozen, positions(&mut app), "a pause holds the pose");
}

#[test]
fn the_tour_requests_the_next_authored_state_after_the_hold() {
    let tmp = install();
    let mut app = app(tmp.path(), true);
    run(&mut app, 10);
    let state = |app: &mut App| {
        app.world_mut()
            .query::<&PedActor>()
            .single(app.world())
            .unwrap()
            .state()
            .to_string()
    };
    assert_eq!(state(&mut app), "STAND");
    // The first tour request is `WALK` (an authored state here); the
    // states this archetype lacks later in the tour are refused, so the
    // figure keeps animating rather than faking them.
    run(&mut app, (LAB_HOLD_SECS * 60.0) as usize + 10);
    assert_eq!(state(&mut app), "WALK");
    assert!(positions(&mut app).iter().all(|v| v.is_finite()));
}
