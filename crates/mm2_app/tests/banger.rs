//! F04-A banger runtime integration: the dormant → active → settled
//! state machine on real Avian physics, plus the WLD-16 name-binding
//! stamp path through `spawn_event_pathsets` — the same headless-app
//! harness `tests/contracts.rs` uses.
//!
//! Threshold semantics are provisional (UNK-22): these tests pin the
//! implemented rule — `approach_speed × striker_mass > ImpulseLimit2` —
//! not a verified original behaviour.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::banger::{activate_bangers, banger_bundle, settle_bangers};
use mm2_game::{
    AuthorityRole, Banger, BangerCause, BangerDefinition, BangerPhase, BangerPool,
    BangerStateChanged, CityEntity, ObjectId, ObjectIdentity, Session, SessionAuthority,
    SessionConfig, SessionEntity, SessionPhase, advance_session_tick, despawn_session_entities,
};

const FRAMES_PER_SECOND: usize = 60;

fn banger_def(name: &str, impulse_limit2: f32) -> BangerDefinition {
    BangerDefinition {
        name: name.into(),
        mass: 40.0,
        friction: 0.9,
        elasticity: 0.5,
        impulse_limit2,
        size: [0.5, 0.5, 0.5],
        cg: [0.0, 0.0, 0.0],
        num_parts: 0,
    }
}

/// A playing session plus the banger systems on the real fixed-step
/// schedule. Returns the minted id of a dormant cube banger at `pos`
/// and the ground beneath it.
fn test_app_with(authority: SessionAuthority, pool: usize) -> App {
    let mut session = Session::new();
    session
        .begin(SessionConfig {
            authority,
            ..SessionConfig::default()
        })
        .unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        // Deterministic: every app.update() is exactly one 60 Hz frame.
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .insert_resource(session)
        .add_plugins(TransformPlugin)
        .add_message::<BangerStateChanged>()
        .insert_resource(BangerPool { max_active: pool })
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(FixedLast, (activate_bangers, settle_bangers).chain());
    app.finish();
    app.cleanup();

    // Flat ground the props and strikers rest/slide on.
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(400.0, 1.0, 400.0),
        Friction::new(0.8),
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    app
}

/// Stamp one dormant banger through the shared bundle — the same shape
/// `city::spawn_banger_prop` produces — and mint its stable id.
fn spawn_banger(app: &mut App, pos: Vec3, def: BangerDefinition) -> (Entity, ObjectId) {
    let (object, role) = {
        let mut session = app.world_mut().resource_mut::<Session>();
        (session.mint_object_id(), session.authority_role())
    };
    let entity = app
        .world_mut()
        .spawn(banger_bundle(
            &def,
            object,
            role,
            SessionEntity(1),
            Collider::cuboid(1.0, 1.0, 1.0),
            Transform::from_translation(pos),
            format!("banger-{}", def.name),
        ))
        .id();
    (entity, object)
}

/// A 1 t dynamic cube sliding at `pos` with `vel` — the striker.
fn spawn_striker(app: &mut App, pos: Vec3, vel: Vec3) -> Entity {
    app.world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(0.9, 0.9, 0.9),
            Mass(1000.0),
            Friction::new(0.8),
            CollisionEventsEnabled,
            Position(pos),
            Transform::from_translation(pos),
            LinearVelocity(vel),
        ))
        .id()
}

fn drain_transitions(app: &mut App) -> Vec<BangerStateChanged> {
    app.world_mut()
        .resource_mut::<Messages<BangerStateChanged>>()
        .drain()
        .collect()
}

/// Run `frames` updates, accumulating every state transition.
fn run(app: &mut App, frames: usize) -> Vec<BangerStateChanged> {
    let mut out = Vec::new();
    for _ in 0..frames {
        app.update();
        out.extend(drain_transitions(app));
    }
    out
}

#[test]
fn a_hard_impact_activates_then_settles_once() {
    let mut app = test_app_with(SessionAuthority::Local, 32);
    let (banger, object) =
        spawn_banger(&mut app, Vec3::new(0.0, 0.5, 0.0), banger_def("test", 0.0));
    // 20 m/s of approach speed × 1000 kg is far above a zero limit.
    spawn_striker(
        &mut app,
        Vec3::new(-6.0, 0.5, 0.0),
        Vec3::new(20.0, 0.0, 0.0),
    );

    let events = run(&mut app, FRAMES_PER_SECOND * 8);

    let activations: Vec<_> = events
        .iter()
        .filter(|e| e.phase == BangerPhase::Active)
        .collect();
    assert_eq!(
        activations.len(),
        1,
        "one qualifying impact activates exactly once: {events:?}"
    );
    match activations[0].cause {
        BangerCause::Impact { severity, estimate } => {
            assert!(severity > 1.0, "real approach speed, got {severity}");
            assert!(estimate > 0.0);
        }
        c => panic!("activation must name its impact, got {c:?}"),
    }
    assert_eq!(activations[0].object, object);
    assert_eq!(
        activations[0].generation,
        app.world().resource::<Session>().generation()
    );
    assert!(activations[0].tick > 0);

    // Whatever the striker does next — re-edges included — the prop
    // transitions once and ends settled at a finite pose.
    assert!(
        events
            .iter()
            .any(|e| e.phase == BangerPhase::Settled && e.cause == BangerCause::Slept),
        "an active prop that comes to rest settles: {events:?}"
    );
    let b = app.world().get::<Banger>(banger).unwrap();
    assert_eq!(b.phase, BangerPhase::Settled);
    assert_eq!(
        *app.world().get::<RigidBody>(banger).unwrap(),
        RigidBody::Static,
        "a settled prop is a static collider again"
    );
    let pos = app.world().get::<Position>(banger).unwrap().0;
    assert!(pos.is_finite());
    assert!(pos.x > 0.2, "the impact knocked the prop loose: {pos:?}");
}

#[test]
fn a_monument_limit_never_activates() {
    let mut app = test_app_with(SessionAuthority::Local, 32);
    // Bridge-gate-class limit (retail authors ≈1e30 for fixed objects):
    // a fixed monument must not go dynamic from a generic collision.
    let (banger, _) = spawn_banger(&mut app, Vec3::new(0.0, 0.5, 0.0), banger_def("monu", 1e30));
    spawn_striker(
        &mut app,
        Vec3::new(-6.0, 0.5, 0.0),
        Vec3::new(20.0, 0.0, 0.0),
    );

    let events = run(&mut app, FRAMES_PER_SECOND * 3);
    assert!(events.is_empty(), "no transitions: {events:?}");
    assert_eq!(
        app.world().get::<Banger>(banger).unwrap().phase,
        BangerPhase::Dormant
    );
    assert_eq!(
        *app.world().get::<RigidBody>(banger).unwrap(),
        RigidBody::Static
    );
}

#[test]
fn a_midrange_limit_discriminates_the_impact() {
    // Same striker both runs: estimate ≈ severity × 1000. A limit above
    // it leaves the prop dormant; below it activates.
    let dormant_limit = {
        let mut app = test_app_with(SessionAuthority::Local, 32);
        let (banger, _) = spawn_banger(&mut app, Vec3::new(0.0, 0.5, 0.0), banger_def("lim", 1e9));
        spawn_striker(
            &mut app,
            Vec3::new(-6.0, 0.5, 0.0),
            Vec3::new(20.0, 0.0, 0.0),
        );
        let events = run(&mut app, FRAMES_PER_SECOND * 3);
        assert!(events.is_empty());
        app.world().get::<Banger>(banger).unwrap().phase
    };
    assert_eq!(dormant_limit, BangerPhase::Dormant);

    let mut app = test_app_with(SessionAuthority::Local, 32);
    let (banger, _) = spawn_banger(&mut app, Vec3::new(0.0, 0.5, 0.0), banger_def("lim", 100.0));
    spawn_striker(
        &mut app,
        Vec3::new(-6.0, 0.5, 0.0),
        Vec3::new(20.0, 0.0, 0.0),
    );
    let events = run(&mut app, FRAMES_PER_SECOND * 3);
    assert_eq!(
        events
            .iter()
            .filter(|e| e.phase == BangerPhase::Active)
            .count(),
        1
    );
    assert_eq!(
        app.world().get::<Banger>(banger).unwrap().phase,
        BangerPhase::Active
    );
}

#[test]
fn predicted_sessions_never_transition_bangers() {
    // A Remote client predicts: it must not independently activate
    // props — F26 replication owns their state.
    let mut app = test_app_with(SessionAuthority::Remote, 32);
    assert_eq!(
        app.world().resource::<Session>().authority_role(),
        AuthorityRole::Predicted
    );
    let (banger, _) = spawn_banger(&mut app, Vec3::new(0.0, 0.5, 0.0), banger_def("test", 0.0));
    spawn_striker(
        &mut app,
        Vec3::new(-6.0, 0.5, 0.0),
        Vec3::new(20.0, 0.0, 0.0),
    );

    let events = run(&mut app, FRAMES_PER_SECOND * 3);
    assert!(
        events.is_empty(),
        "predicted sessions emit nothing: {events:?}"
    );
    assert_eq!(
        app.world().get::<Banger>(banger).unwrap().phase,
        BangerPhase::Dormant
    );
}

#[test]
fn the_pool_reclaims_oldest_first_at_capacity() {
    let mut app = test_app_with(SessionAuthority::Local, 1);
    let (b1, o1) = spawn_banger(&mut app, Vec3::new(0.0, 0.5, 0.0), banger_def("one", 0.0));
    let (b2, o2) = spawn_banger(&mut app, Vec3::new(60.0, 0.5, 0.0), banger_def("two", 0.0));

    spawn_striker(
        &mut app,
        Vec3::new(-6.0, 0.5, 0.0),
        Vec3::new(20.0, 0.0, 0.0),
    );
    let first = run(&mut app, FRAMES_PER_SECOND);
    assert!(
        first
            .iter()
            .any(|e| e.object == o1 && e.phase == BangerPhase::Active),
        "the first prop activates: {first:?}"
    );

    // At capacity, the second activation settles the oldest slot.
    spawn_striker(
        &mut app,
        Vec3::new(54.0, 0.5, 0.0),
        Vec3::new(20.0, 0.0, 0.0),
    );
    let second = run(&mut app, FRAMES_PER_SECOND);
    assert!(
        second
            .iter()
            .any(|e| e.object == o2 && e.phase == BangerPhase::Active),
        "the second prop activates: {second:?}"
    );
    assert!(
        second.iter().any(|e| e.object == o1
            && e.phase == BangerPhase::Settled
            && e.cause == BangerCause::Reclaimed),
        "the reclaimed slot is the oldest active prop: {second:?}"
    );
    assert_eq!(
        app.world().get::<Banger>(b1).unwrap().phase,
        BangerPhase::Settled
    );
    assert_eq!(
        app.world().get::<Banger>(b2).unwrap().phase,
        BangerPhase::Active
    );
}

// ---------------------------------------------------------------------------
// The WLD-16 stamp path: bound names become banger entities, everything
// else stays an ordinary prop.
// ---------------------------------------------------------------------------

fn write(dir: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

fn pth1_path(name: &str, points: &[[f32; 3]], kind: u8, spacing: u8) -> Vec<u8> {
    let mut d = vec![0u8; 32];
    d[..name.len()].copy_from_slice(name.as_bytes());
    d.extend_from_slice(&(points.len() as u32).to_le_bytes());
    d.extend_from_slice(&0u32.to_le_bytes()); // selection
    for p in points {
        d.extend_from_slice(&0u32.to_le_bytes()); // attributes
        for c in p {
            d.extend_from_slice(&c.to_le_bytes());
        }
    }
    d.push(kind);
    d.push(spacing);
    d.extend_from_slice(&[0, 0]);
    d
}

fn pth1(paths: &[Vec<u8>]) -> Vec<u8> {
    let mut d = b"PTH1".to_vec();
    d.extend_from_slice(&(paths.len() as u32).to_le_bytes());
    d.extend_from_slice(&0u32.to_le_bytes()); // current_path
    for p in paths {
        d.extend_from_slice(p);
    }
    d
}

/// The minimal tetrahedron PKG `import_pipeline`/`event` stamp — copied
/// so this test stays self-contained (each `tests/` file is a crate).
fn testprop_pkg() -> Vec<u8> {
    let mut geo = Vec::new();
    geo.extend_from_slice(&1u32.to_le_bytes()); // nSections
    geo.extend_from_slice(&4u32.to_le_bytes()); // total vertices
    geo.extend_from_slice(&12u32.to_le_bytes()); // total indices
    geo.extend_from_slice(&1u32.to_le_bytes()); // sections duplicate
    geo.extend_from_slice(&0x112u32.to_le_bytes()); // fvf: XYZ|NORMAL|1 tex
    geo.extend_from_slice(&1u16.to_le_bytes()); // nStrips
    geo.extend_from_slice(&0u16.to_le_bytes()); // section flags
    geo.extend_from_slice(&(-1i32).to_le_bytes()); // shader offset → fallback
    geo.extend_from_slice(&3i32.to_le_bytes()); // prim type: triangles
    geo.extend_from_slice(&4u32.to_le_bytes()); // strip vertices
    let verts: &[([f32; 3], [f32; 3], [f32; 2])] = &[
        ([0., 0., 0.], [0., 1., 0.], [0., 0.]),
        ([1., 0., 0.], [0., 1., 0.], [1., 0.]),
        ([0., 0., 1.], [0., 1., 0.], [0., 1.]),
        ([0., 1., 0.], [0., 1., 0.], [0.5, 0.5]),
    ];
    for &(p, n, uv) in verts {
        for c in p {
            geo.extend_from_slice(&c.to_le_bytes());
        }
        for c in n {
            geo.extend_from_slice(&c.to_le_bytes());
        }
        for c in uv {
            geo.extend_from_slice(&c.to_le_bytes());
        }
    }
    geo.extend_from_slice(&12u32.to_le_bytes()); // strip indices
    for i in [0u16, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3] {
        geo.extend_from_slice(&i.to_le_bytes());
    }

    let mut d = Vec::new();
    d.extend_from_slice(b"PKG3");
    d.extend_from_slice(b"FILE");
    d.push(b"bangtest_h".len() as u8 + 1);
    d.extend_from_slice(b"bangtest_h");
    d.push(0);
    d.extend_from_slice(&(geo.len() as u32).to_le_bytes());
    d.extend_from_slice(&geo);
    d
}

/// A minimal well-formed `dgBangerData` record — every required field,
/// no optional blocks.
fn banger_record(limit: f32) -> String {
    format!(
        "type: a\ndgBangerData {{\n  AudioId 0\n  Size 0.5 1.0 0.5\n  CG 0.0 0.5 0.0\n  Mass 40.0\n  Elasticity 0.5\n  Friction 0.9\n  ImpulseLimit2 {limit}\n  SpinAxis 0\n  Flash 0\n  NumParts 0\n  TexNumber 0\n  BillFlags 0\n  YRadius 0.5\n}}\n"
    )
}

fn stamp_overlay(dir: &Path) -> (mm2_app::city::EventPathsetReport, World) {
    let vfs = {
        let mut vfs = mm2_assets::Vfs::new();
        vfs.mount_dir(dir, 0).unwrap();
        vfs
    };
    let mut world = World::new();
    let mut queue = bevy::ecs::world::CommandQueue::default();
    let mut meshes: Assets<Mesh> = Assets::default();
    let mut images: Assets<Image> = Assets::default();
    let mut materials: Assets<StandardMaterial> = Assets::default();
    let report = {
        let mut commands = Commands::new(&mut queue, &world);
        let mut session = Session::new();
        mm2_app::city::spawn_event_pathsets(
            &mut commands,
            &vfs,
            &["race/t/overlay.pathset".to_string()],
            &mut meshes,
            &mut images,
            &mut materials,
            SessionEntity(1),
            &mut session,
        )
    };
    queue.apply(&mut world);
    (report, world)
}

#[test]
fn bound_pathset_names_stamp_as_dormant_bangers() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "geometry/bangtest.pkg", testprop_pkg());
    write(d, "geometry/plainprop.pkg", testprop_pkg());
    write(d, "tune/banger/bangtest.dgbangerdata", banger_record(0.0));
    write(
        d,
        "race/t/overlay.pathset",
        pth1(&[
            pth1_path("bangtest", &[[0.0, 0.0, 0.0]], 0, 0),
            pth1_path("plainprop", &[[5.0, 0.0, 0.0]], 0, 0),
        ]),
    );

    let (report, mut world) = stamp_overlay(d);
    assert_eq!(report.stats.spawned, 2);
    assert_eq!(report.stats.bangers, 1, "only the bound name is a banger");
    assert_eq!(report.stats.banger_failed, 0);

    // The bound stamp is one entity: collider + dormant state + stable
    // identity on the root, the render part a child that follows it.
    let bangers: Vec<Entity> = world
        .query_filtered::<Entity, With<Banger>>()
        .iter(&world)
        .collect();
    assert_eq!(bangers.len(), 1);
    let root = bangers[0];
    let b = world.get::<Banger>(root).unwrap();
    assert_eq!(b.phase, BangerPhase::Dormant);
    assert_eq!(b.def.name, "bangtest");
    assert_eq!(b.def.mass, 40.0);
    assert!(world.get::<Collider>(root).is_some());
    assert!(world.get::<ObjectIdentity>(root).is_some());
    assert!(world.get::<SessionEntity>(root).is_some());
    let children = world.get::<Children>(root).unwrap();
    assert_eq!(children.len(), 1, "the mesh part rides the dynamic root");
    assert!(world.get::<Collider>(children[0]).is_none());
    // Exactly one collider for the placement — the loose render/collider
    // pair a static prop gets would duplicate it.
    assert_eq!(
        world
            .query_filtered::<Entity, With<Collider>>()
            .iter(&world)
            .count(),
        2,
        "one collider per stamped prop, none duplicated"
    );

    // The unbound name is the ordinary static prop: no Banger state.
    let statics = world
        .query_filtered::<Entity, (With<CityEntity>, Without<Banger>)>()
        .iter(&world)
        .count();
    assert_eq!(statics, 2, "plainprop's render part + static collider");

    // Session teardown removes the banger root and its children too.
    world
        .run_system_once(despawn_session_entities)
        .expect("teardown runs");
    assert_eq!(world.query::<&CityEntity>().iter(&world).count(), 0);
}

#[test]
fn malformed_and_missing_records_fall_back_to_static_props() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "geometry/bangtest.pkg", testprop_pkg());
    write(d, "geometry/plainprop.pkg", testprop_pkg());
    write(
        d,
        "tune/banger/bangtest.dgbangerdata",
        "garbage, not a tune file",
    );
    write(
        d,
        "race/t/overlay.pathset",
        pth1(&[
            pth1_path("bangtest", &[[0.0, 0.0, 0.0]], 0, 0),
            pth1_path("plainprop", &[[5.0, 0.0, 0.0]], 0, 0),
        ]),
    );

    let (report, mut world) = stamp_overlay(d);
    assert_eq!(report.stats.spawned, 2, "both names still stamp");
    assert_eq!(report.stats.bangers, 0, "no decodable record, no banger");
    assert_eq!(
        report.stats.banger_failed, 1,
        "the corrupt record is counted once per name, not hidden"
    );
    assert!(
        world
            .query_filtered::<Entity, With<Banger>>()
            .iter(&world)
            .next()
            .is_none(),
        "a failed decode stamps an ordinary static prop"
    );
}
