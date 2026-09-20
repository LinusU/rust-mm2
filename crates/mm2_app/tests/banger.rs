//! F04 banger runtime integration: the dormant → active/broken →
//! settled state machine on real Avian physics, plus the WLD-16
//! name-binding stamp path through `spawn_event_pathsets` — the same
//! headless-app harness `tests/contracts.rs` uses.
//!
//! Threshold and fragment semantics are provisional (UNK-22): these
//! tests pin the implemented rules — `approach_speed × striker_mass >
//! ImpulseLimit2` and BREAK-chunk spawning on the activation edge —
//! not verified original behaviour.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::banger::{
    BangerPieces, FragmentPiece, activate_bangers, banger_bundle, settle_bangers,
};
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

    // Flat ground the props and strikers rest/slide on — wide enough
    // that break fragments scattering at approach speed stay on it.
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(2000.0, 1.0, 2000.0),
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
    let name = format!("banger-{}", def.name);
    let entity = app
        .world_mut()
        .spawn(banger_bundle(
            Banger::new(def),
            object,
            role,
            SessionEntity(1),
            Collider::cuboid(1.0, 1.0, 1.0),
            Transform::from_translation(pos),
            name,
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

// ---------------------------------------------------------------------------
// F04-B: BREAK<NN> pieces — a bound prop whose PKG carries break chunks
// shatters on activation instead of tipping over.
// ---------------------------------------------------------------------------

/// The distilled def one fragment piece carries — its own record's
/// shape, small and light next to the parent prop.
fn piece_def(name: &str, mass: f32) -> BangerDefinition {
    BangerDefinition {
        name: name.into(),
        mass,
        friction: 0.9,
        elasticity: 0.5,
        impulse_limit2: 0.0,
        size: [0.2, 0.2, 0.2],
        cg: [0.0, 0.1, 0.0],
        num_parts: 0,
    }
}

/// A dormant banger carrying `n` authored break pieces (each with a
/// collidable cube chunk and a mesh part) plus one unified mesh child —
/// the shape `spawn_banger_prop` produces for a prop whose PKG has
/// `BREAK<NN>` chunks.
fn spawn_breakable(
    app: &mut App,
    pos: Vec3,
    def: BangerDefinition,
    n: usize,
) -> (Entity, ObjectId) {
    let (entity, object) = spawn_banger(app, pos, def);
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::new(0.3, 0.3, 0.3));
    if !app.world().contains_resource::<Assets<StandardMaterial>>() {
        app.world_mut().init_resource::<Assets<StandardMaterial>>();
    }
    let mat = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    // The intact prop renders one unified mesh child.
    let child = app
        .world_mut()
        .spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(mat.clone()),
            Transform::IDENTITY,
        ))
        .id();
    app.world_mut().entity_mut(entity).add_child(child);
    let pieces: Vec<FragmentPiece> = (1..=n)
        .map(|i| FragmentPiece {
            index: format!("{i:02}"),
            def: piece_def(&format!("frag{i:02}"), 5.0),
            parts: vec![(mesh.clone(), mat.clone())],
            collider: Some(Collider::cuboid(0.3, 0.3, 0.3)),
        })
        .collect();
    app.world_mut()
        .entity_mut(entity)
        .insert(BangerPieces { fragments: pieces });
    (entity, object)
}

/// Every banger entity other than `except`, with its identity.
fn other_bangers(app: &mut App, except: Entity) -> Vec<(Entity, ObjectId, BangerPhase)> {
    let mut q = app
        .world_mut()
        .query_filtered::<(Entity, &Banger, &ObjectIdentity), With<Collider>>();
    q.iter(app.world())
        .filter(|(e, _, _)| *e != except)
        .map(|(e, b, id)| (e, id.0, b.phase))
        .collect()
}

#[test]
fn a_breakable_prop_shatters_into_its_authored_pieces() {
    let mut app = test_app_with(SessionAuthority::Local, 32);
    let (banger, object) = spawn_breakable(
        &mut app,
        Vec3::new(0.0, 0.5, 0.0),
        banger_def("bench", 0.0),
        3,
    );
    // A modest hit still clears the zero threshold while keeping the
    // pieces inside the test ground — the striker starts close since
    // ground friction brakes it on approach.
    spawn_striker(
        &mut app,
        Vec3::new(-3.0, 0.5, 0.0),
        Vec3::new(10.0, 0.0, 0.0),
    );

    let events = run(&mut app, FRAMES_PER_SECOND * 10);

    // One logical break event for the placement — the parent itself
    // never goes Active (AC02/AC03).
    let breaks: Vec<_> = events
        .iter()
        .filter(|e| e.phase == BangerPhase::Broken)
        .collect();
    assert_eq!(breaks.len(), 1, "one break event: {events:?}");
    assert_eq!(breaks[0].object, object);
    match breaks[0].cause {
        BangerCause::Impact { severity, .. } => {
            assert!(severity > 1.0, "the break names its impact");
        }
        c => panic!("a break must name its impact, got {c:?}"),
    }
    assert!(
        !events
            .iter()
            .any(|e| e.object == object && e.phase == BangerPhase::Active),
        "the shattered parent never activates: {events:?}"
    );

    // The placement is a collider-less identity husk: no collider, no
    // unified mesh children.
    assert_eq!(
        app.world().get::<Banger>(banger).unwrap().phase,
        BangerPhase::Broken
    );
    assert!(app.world().get::<Collider>(banger).is_none());
    assert_eq!(
        app.world()
            .get::<Children>(banger)
            .map(|c| c.len())
            .unwrap_or(0),
        0,
        "the unified mesh is replaced by the pieces"
    );

    // Exactly three fragment bodies exist, each session-owned with its
    // own minted identity and a collider — and they settle where they
    // lie like any other active.
    let fragments = other_bangers(&mut app, banger);
    assert_eq!(fragments.len(), 3, "one body per collidable piece");
    assert!(
        fragments.iter().all(|(_, id, _)| *id != object),
        "pieces mint their own ids"
    );
    let settled = events
        .iter()
        .filter(|e| e.phase == BangerPhase::Settled)
        .count();
    assert!(
        settled >= 1,
        "fragments come to rest and settle: {events:?}"
    );

    // Session teardown removes the husk and every piece it spawned.
    app.world_mut()
        .run_system_once(despawn_session_entities)
        .expect("teardown runs");
    assert_eq!(
        app.world_mut()
            .query::<&CityEntity>()
            .iter(app.world())
            .count(),
        0,
        "no placement or fragment survives teardown"
    );
}

#[test]
fn the_pool_bounds_fragment_spawns() {
    let mut app = test_app_with(SessionAuthority::Local, 1);
    let (banger, _) = spawn_breakable(
        &mut app,
        Vec3::new(0.0, 0.5, 0.0),
        banger_def("bench", 0.0),
        3,
    );
    spawn_striker(
        &mut app,
        Vec3::new(-6.0, 0.5, 0.0),
        Vec3::new(20.0, 0.0, 0.0),
    );

    let events = run(&mut app, FRAMES_PER_SECOND);
    assert!(
        events.iter().any(|e| e.phase == BangerPhase::Broken),
        "the prop still shatters: {events:?}"
    );
    // With a pool of one and nothing reclaimable (the just-spawned
    // pieces are pending, not in the world), exactly one piece becomes
    // a body — the cap is never exceeded.
    let fragments = other_bangers(&mut app, banger);
    assert_eq!(fragments.len(), 1, "the pool bound caps the pieces");
}

#[test]
fn pieces_without_collision_tip_over_instead() {
    let mut app = test_app_with(SessionAuthority::Local, 32);
    let (banger, object) =
        spawn_banger(&mut app, Vec3::new(0.0, 0.5, 0.0), banger_def("limp", 0.0));
    // A BangerPieces component whose pieces all lack collision carries
    // no spawnable body — the placement takes the ordinary activation
    // path rather than shattering into nothing.
    app.world_mut().entity_mut(banger).insert(BangerPieces {
        fragments: vec![FragmentPiece {
            index: "01".into(),
            def: piece_def("limp_break01", 5.0),
            parts: Vec::new(),
            collider: None,
        }],
    });
    spawn_striker(
        &mut app,
        Vec3::new(-6.0, 0.5, 0.0),
        Vec3::new(20.0, 0.0, 0.0),
    );

    let events = run(&mut app, FRAMES_PER_SECOND * 3);
    assert!(
        events
            .iter()
            .any(|e| e.object == object && e.phase == BangerPhase::Active),
        "no collidable pieces → ordinary activation: {events:?}"
    );
    assert!(
        !events.iter().any(|e| e.phase == BangerPhase::Broken),
        "nothing shatters: {events:?}"
    );
    assert!(other_bangers(&mut app, banger).is_empty());
}

// ---------------------------------------------------------------------------
// The stamp path for breakables: `BREAK<NN>` chunks become `BangerPieces`.
// ---------------------------------------------------------------------------

/// `banger_record` with an explicit `NumParts` — a breakable's record.
fn breakable_record(limit: f32, num_parts: i64) -> String {
    format!(
        "type: a\ndgBangerData {{\n  AudioId 0\n  Size 0.5 1.0 0.5\n  CG 0.0 0.5 0.0\n  Mass 40.0\n  Elasticity 0.5\n  Friction 0.9\n  ImpulseLimit2 {limit}\n  SpinAxis 0\n  Flash 0\n  NumParts {num_parts}\n  TexNumber 0\n  BillFlags 0\n  YRadius 0.5\n}}\n"
    )
}

/// A PKG with one intact chunk plus two `BREAK<NN>` pieces — the
/// `sp_benchwood_f` layout in miniature.
fn breakable_pkg() -> Vec<u8> {
    let geo = |verts: &[([f32; 3], [f32; 3], [f32; 2])], indices: &[u16]| {
        let mut d = Vec::new();
        d.extend_from_slice(&1u32.to_le_bytes()); // nSections
        d.extend_from_slice(&(verts.len() as u32).to_le_bytes());
        d.extend_from_slice(&(indices.len() as u32).to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes()); // sections duplicate
        d.extend_from_slice(&0x112u32.to_le_bytes()); // fvf: XYZ|NORMAL|1 tex
        d.extend_from_slice(&1u16.to_le_bytes()); // nStrips
        d.extend_from_slice(&0u16.to_le_bytes()); // section flags
        d.extend_from_slice(&(-1i32).to_le_bytes()); // shader offset → fallback
        d.extend_from_slice(&3i32.to_le_bytes()); // prim type: triangles
        d.extend_from_slice(&(verts.len() as u32).to_le_bytes());
        for &(p, n, uv) in verts {
            for c in p {
                d.extend_from_slice(&c.to_le_bytes());
            }
            for c in n {
                d.extend_from_slice(&c.to_le_bytes());
            }
            for c in uv {
                d.extend_from_slice(&c.to_le_bytes());
            }
        }
        d.extend_from_slice(&(indices.len() as u32).to_le_bytes());
        for &i in indices {
            d.extend_from_slice(&i.to_le_bytes());
        }
        d
    };
    let tetra = [
        ([0., 0., 0.], [0., 1., 0.], [0., 0.]),
        ([1., 0., 0.], [0., 1., 0.], [1., 0.]),
        ([0., 0., 1.], [0., 1., 0.], [0., 1.]),
        ([0., 1., 0.], [0., 1., 0.], [0.5, 0.5]),
    ];
    let tris = [0u16, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3];
    let chunk = |name: &str, data: Vec<u8>| {
        let mut d = Vec::new();
        d.extend_from_slice(b"FILE");
        d.push(name.len() as u8 + 1);
        d.extend_from_slice(name.as_bytes());
        d.push(0);
        d.extend_from_slice(&(data.len() as u32).to_le_bytes());
        d.extend_from_slice(&data);
        d
    };
    let mut d = b"PKG3".to_vec();
    d.extend_from_slice(&chunk("breakpkg_h", geo(&tetra, &tris)));
    d.extend_from_slice(&chunk("BREAK01_H", geo(&tetra, &tris)));
    d.extend_from_slice(&chunk("BREAK02_H", geo(&tetra, &tris)));
    d
}

#[test]
fn breakable_props_stamp_their_authored_pieces() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "geometry/breakpkg.pkg", breakable_pkg());
    write(
        d,
        "tune/banger/breakpkg.dgbangerdata",
        breakable_record(0.0, 2),
    );
    // break01 has its own record (a light piece); break02 does not and
    // must fall back to the parent def.
    write(
        d,
        "tune/banger/breakpkg_break01.dgbangerdata",
        breakable_record(0.0, 0).replace("Mass 40.0", "Mass 7.0"),
    );
    write(
        d,
        "race/t/overlay.pathset",
        pth1(&[pth1_path("breakpkg", &[[0.0, 0.0, 0.0]], 0, 0)]),
    );

    let (report, mut world) = stamp_overlay(d);
    assert_eq!(report.stats.bangers, 1);
    assert_eq!(report.stats.pieces, 2, "two collidable pieces stamped");

    let bangers: Vec<Entity> = world
        .query_filtered::<Entity, With<Banger>>()
        .iter(&world)
        .collect();
    assert_eq!(bangers.len(), 1);
    let pieces = world.get::<BangerPieces>(bangers[0]).unwrap();
    assert_eq!(pieces.fragments.len(), 2);
    assert_eq!(pieces.fragments[0].index, "01");
    assert_eq!(pieces.fragments[0].def.mass, 7.0, "the piece's own record");
    assert_eq!(pieces.fragments[0].def.name, "breakpkg_break01");
    assert_eq!(pieces.fragments[1].index, "02");
    assert_eq!(
        pieces.fragments[1].def.mass, 40.0,
        "a piece without a record inherits the parent def"
    );
    assert_eq!(pieces.fragments[1].def.name, "breakpkg_break02");
    assert!(
        pieces.fragments.iter().all(|f| f.collider.is_some()),
        "every authored piece got a collider"
    );

    // The dormant prop keeps only its intact mesh — the BREAK chunks
    // are not part of its surface or collider.
    assert_eq!(world.get::<Children>(bangers[0]).unwrap().len(), 1);
}
