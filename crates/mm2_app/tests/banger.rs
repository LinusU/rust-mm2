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
use mm2_app::camera::CameraMode;
use mm2_app::contracts::{self, ImpactFilter};
use mm2_app::session::{self, SelectedCar, SessionControl, SpawnPoint, TunedVehicle};
use mm2_assets::Vfs;
use mm2_game::{
    AuthorityRole, Banger, BangerCause, BangerDefinition, BangerPhase, BangerPool,
    BangerStateChanged, CityEntity, ImpactEvent, Mm2Vfs, ObjectId, ObjectIdentity, Session,
    SessionAuthority, SessionConfig, SessionEntity, SessionPhase, WorldMode, advance_session_tick,
    despawn_session_entities,
};
use mm2_vehicle::{StrikeBound, VehicleConfig, VehiclePlugin, vehicle_bundle};

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

    // The corrected `Size` semantics (full bound extents, not
    // half-extents) quarter the inertia estimate — the kick tumbles
    // the prop harder and it needs longer to sleep.
    let events = run(&mut app, FRAMES_PER_SECOND * 30);

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
// Authored-bound strikes: a moving vehicle's `StrikeBound` — the
// unmodified bound the original prop test used — activates dormant
// bangers the snag-safe world hull rides over (a bus over a cone).
// The tests pin the implemented rule, not verified original behaviour.
// ---------------------------------------------------------------------------

/// A striker whose world collider rides *above* the prop while its
/// `StrikeBound` reaches down to it — the high-floor vehicle the
/// authored bound still strikes with. `GravityScale(0)` keeps the
/// separation exact: the world hull can never touch the prop.
fn spawn_bound_striker(app: &mut App, pos: Vec3, vel: Vec3) -> Entity {
    app.world_mut()
        .spawn((
            RigidBody::Dynamic,
            // World hull: floor at pos.y - 0.1 — clears a prop top
            // below that mark entirely.
            Collider::cuboid(0.5, 0.2, 0.5),
            // Authored bound: floor at pos.y - 1.5 — overlaps the prop.
            StrikeBound(Collider::cuboid(0.9, 3.0, 0.9)),
            Mass(1000.0),
            GravityScale(0.0),
            Position(pos),
            Transform::from_translation(pos),
            LinearVelocity(vel),
        ))
        .id()
}

#[test]
fn a_strike_bound_overlap_activates_a_prop_the_hull_clears() {
    let mut app = test_app_with(SessionAuthority::Local, 32);
    let (banger, object) =
        spawn_banger(&mut app, Vec3::new(0.0, 0.5, 0.0), banger_def("cone", 0.0));
    // World-hull floor at 1.3 rides over the prop's 1.0 top without
    // contact; the bound floor at -0.1 overlaps it when aligned.
    let striker = spawn_bound_striker(
        &mut app,
        Vec3::new(-3.0, 1.4, 0.0),
        Vec3::new(10.0, 0.0, 0.0),
    );

    let events = run(&mut app, FRAMES_PER_SECOND);

    let activations: Vec<_> = events
        .iter()
        .filter(|e| e.phase == BangerPhase::Active)
        .collect();
    assert_eq!(
        activations.len(),
        1,
        "the authored bound strikes what the world hull clears: {events:?}"
    );
    assert_eq!(activations[0].object, object);
    match activations[0].cause {
        BangerCause::Impact { severity, estimate } => {
            assert!(severity > 5.0, "approach speed, got {severity}");
            assert!(estimate > 0.0);
        }
        c => panic!("activation must name its impact, got {c:?}"),
    }

    // The world hull never touched: the striker kept its speed and
    // passed cleanly over where the prop stood.
    let pos = app.world().get::<Position>(striker).unwrap().0;
    assert!(pos.x > 3.0, "nothing slowed the striker: {pos:?}");
    assert!(
        (pos.y - 1.4).abs() < 0.01,
        "the striker never fell or deflected: {pos:?}"
    );
    assert_eq!(
        app.world().get::<Banger>(banger).unwrap().phase,
        BangerPhase::Active
    );
}

#[test]
fn a_parked_strike_bound_activates_nothing() {
    // A stationary vehicle overlapping a prop must not detonate it —
    // the overlap carries approach speed, not presence.
    let mut app = test_app_with(SessionAuthority::Local, 32);
    let (banger, _) = spawn_banger(&mut app, Vec3::new(0.0, 0.5, 0.0), banger_def("cone", 0.0));
    spawn_bound_striker(&mut app, Vec3::new(0.0, 1.4, 0.0), Vec3::ZERO);

    let events = run(&mut app, FRAMES_PER_SECOND);
    assert!(events.is_empty(), "no transitions: {events:?}");
    assert_eq!(
        app.world().get::<Banger>(banger).unwrap().phase,
        BangerPhase::Dormant
    );
}

#[test]
fn a_below_limit_strike_bound_overlap_leaves_the_prop_dormant() {
    // The authored impulse gate applies to bound strikes the same as
    // contacts: a monument-class prop ignores an overlap.
    let mut app = test_app_with(SessionAuthority::Local, 32);
    let (banger, _) = spawn_banger(&mut app, Vec3::new(0.0, 0.5, 0.0), banger_def("gate", 1e30));
    spawn_bound_striker(
        &mut app,
        Vec3::new(-3.0, 1.4, 0.0),
        Vec3::new(10.0, 0.0, 0.0),
    );

    let events = run(&mut app, FRAMES_PER_SECOND);
    assert!(events.is_empty(), "no transitions: {events:?}");
    assert_eq!(
        app.world().get::<Banger>(banger).unwrap().phase,
        BangerPhase::Dormant
    );
}

#[test]
fn vehicle_bundle_stamps_the_authored_bound_as_strike_surface() {
    // The production bundle carries `striker_points` as a StrikeBound;
    // a config without one strikes with the world hull's shape.
    let mut app = test_app_with(SessionAuthority::Local, 32);
    let mut cfg = VehicleConfig {
        striker_points: Some(vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ]),
        ..VehicleConfig::default()
    };
    let with_bound = app.world_mut().spawn(vehicle_bundle(&cfg)).id();
    assert!(
        app.world().get::<StrikeBound>(with_bound).is_some(),
        "striker_points becomes the entity's strike surface"
    );

    cfg.striker_points = None;
    let without_bound = app.world_mut().spawn(vehicle_bundle(&cfg)).id();
    assert!(
        app.world().get::<StrikeBound>(without_bound).is_some(),
        "the fallback strike surface is the world collider itself"
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

    // Same corrected-inertia note as above: the pieces scatter harder
    // and need a longer window to come to rest.
    let events = run(&mut app, FRAMES_PER_SECOND * 30);

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

// ---------------------------------------------------------------------------
// F04-C: restart restores the stamped placements (AC05) — a break
// through the real `load_session_world` city path, then
// `SessionControl.restart` through `drive_session`, must restamp the
// same dormant placement and leave no fragment or husk behind.
// ---------------------------------------------------------------------------

fn push_lp(out: &mut Vec<u8>, s: &str) {
    out.push(s.len() as u8 + 1);
    out.extend_from_slice(s.as_bytes());
    out.push(0);
}

fn push_f32s(out: &mut Vec<u8>, v: &[f32]) {
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
}

/// The same one-room synthetic PSDL `import_pipeline` stamps — a road
/// (x −5..5, z 0..20) plus a ground fan — copied so this test stays
/// self-contained (each `tests/` file is a crate).
fn city_psdl() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"PSD0");
    d.extend_from_slice(&2u32.to_le_bytes()); // target_size
    let verts: &[[f32; 3]] = &[
        [-5., 0., 0.],
        [-3., 0., 0.],
        [3., 0., 0.],
        [5., 0., 0.], // road section 0: sw_l, rl, rr, sw_r
        [-5., 0., 20.],
        [-3., 0., 20.],
        [3., 0., 20.],
        [5., 0., 20.], // road section 1
        [10., 0., 0.],
        [10., 0., 10.],
        [20., 0., 10.],
        [20., 0., 0.], // fan (clockwise in x,z)
        [30., 0., 0.],
        [30., 0., 10.], // wall edge
        [35., 5., 0.],
        [45., 5., 0.],
        [45., 5., 10.],
        [35., 5., 10.], // roof
    ];
    d.extend_from_slice(&(verts.len() as u32).to_le_bytes());
    for v in verts {
        push_f32s(&mut d, v);
    }
    let heights = [0.15f32, 2.0, 6.0];
    d.extend_from_slice(&(heights.len() as u32).to_le_bytes());
    push_f32s(&mut d, &heights);
    d.extend_from_slice(&2u32.to_le_bytes());
    push_lp(&mut d, "test_road");
    d.extend_from_slice(&2u32.to_le_bytes()); // nRooms
    d.extend_from_slice(&0u32.to_le_bytes()); // junctions
    let mut attr_words: Vec<u16> = Vec::new();
    let attr = |words: &mut Vec<u16>, word: u16, data: &[u16]| {
        words.push(word);
        words.extend_from_slice(data);
    };
    attr(&mut attr_words, 0x0a << 3, &[1]); // texture ref → textures[0]
    attr(&mut attr_words, 0x00, &[2, 0, 1, 2, 3, 4, 5, 6, 7]); // counted road
    attr(&mut attr_words, 0x06 << 3, &[2, 8, 9, 10, 11]); // counted fan
    attr(&mut attr_words, (0x0b << 3) | 6, &[1, 2, 3, 2, 12, 13]); // facade
    attr(&mut attr_words, (0x07 << 3) | 4, &[0, 2, 12, 13]); // facade bound
    attr(&mut attr_words, (0x0c << 3) | 3, &[2, 14, 15, 16, 17]); // roof fan
    attr(&mut attr_words, (0x03 << 3) | 4, &[2, 0, 12, 13]); // sliver
    attr(&mut attr_words, (0x09 << 3) | 3 | 0x80, &[9, 9, 9]); // tunnel (last)
    let mut room = Vec::new();
    room.extend_from_slice(&4u32.to_le_bytes()); // nPerimeter
    room.extend_from_slice(&(attr_words.len() as u32).to_le_bytes());
    for v in [0u16, 1, 2, 3] {
        room.extend_from_slice(&v.to_le_bytes());
        room.extend_from_slice(&0u16.to_le_bytes()); // neighbour room
    }
    for w in &attr_words {
        room.extend_from_slice(&w.to_le_bytes());
    }
    d.extend_from_slice(&room);
    d.extend_from_slice(&[0u8; 2]); // room flags (nRooms entries)
    d.extend_from_slice(&[0u8; 2]); // prop rules
    push_f32s(&mut d, &[-5., 0., 0.]); // bounds min
    push_f32s(&mut d, &[45., 6., 20.]); // bounds max
    push_f32s(&mut d, &[20., 3., 10.]); // bounds centre
    push_f32s(&mut d, &[30.]); // radius
    d.extend_from_slice(&0u32.to_le_bytes()); // nPaths
    d
}

/// A drivable synthetic city whose `props.pathset` stamps one
/// breakable `breakpkg` prop on the road at (0, 0, 15).
fn city_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/test.psdl", city_psdl());
    std::fs::create_dir_all(d.join("texture")).unwrap();
    std::fs::write(
        d.join("texture/test_road.png"),
        include_bytes!("../../../assets/texture/dev_road.png"),
    )
    .unwrap();
    write(d, "geometry/breakpkg.pkg", breakable_pkg());
    write(
        d,
        "tune/banger/breakpkg.dgbangerdata",
        breakable_record(0.0, 2),
    );
    write(
        d,
        "city/test/props.pathset",
        pth1(&[pth1_path("breakpkg", &[[0.0, 0.0, 15.0]], 0, 0)]),
    );
    tmp
}

/// The production session wiring for a cruise city — the same systems
/// `headless_smoke` and the binary schedule, including the banger
/// driver and the lifecycle driver a restart rides.
fn city_app(vfs: Vfs) -> App {
    let mut session = Session::new();
    session
        .begin(SessionConfig {
            world: WorldMode::City {
                psdl: "city/test.psdl".into(),
            },
            ..SessionConfig::default()
        })
        .unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .insert_resource(session)
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<BangerStateChanged>()
        .add_message::<ImpactEvent>()
        .init_resource::<ImpactFilter>()
        .init_resource::<mm2_app::damage::DamageReport>()
        .init_resource::<mm2_app::stuck::StuckReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<SessionControl>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<BangerPool>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(CameraMode::Chase)
        .insert_resource(SpawnPoint {
            position: Vec3::new(0.0, 1.5, 0.0),
            yaw: 0.0,
            trailers: Vec::new(),
        })
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(TunedVehicle(VehicleConfig::default()))
        .insert_resource(SelectedCar {
            def: None,
            paint: 0,
        })
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (contracts::collect_impacts, activate_bangers, settle_bangers).chain(),
        )
        .add_systems(
            Update,
            (
                session::load_session_world.run_if(session::loading),
                session::session_control_input,
                (
                    despawn_session_entities.run_if(session::unloading),
                    session::drive_session,
                )
                    .chain(),
            ),
        );
    app.finish();
    app.cleanup();
    app
}

/// Every `Banger` entity with its phase and owning generation.
fn banger_states(app: &mut App) -> Vec<(BangerPhase, SessionEntity)> {
    let mut q = app
        .world_mut()
        .query_filtered::<(&Banger, &SessionEntity), ()>();
    q.iter(app.world())
        .map(|(b, owner)| (b.phase, *owner))
        .collect()
}

/// AC05: a session restart through the real teardown/reload cycle
/// restores the same initial stamped placements and removes every
/// fragment the break spawned — no leaked bodies, no leftover husks.
#[test]
fn restart_restores_stamped_placements_after_a_break() {
    let tmp = city_install();
    let mut vfs = Vfs::new();
    vfs.mount_dir(tmp.path(), 0).unwrap();
    let mut app = city_app(vfs);
    app.update();
    assert!(matches!(
        app.world().resource::<Session>().phase(),
        SessionPhase::Playing
    ));

    // The authored breakable stamped once, dormant, carrying pieces.
    let states = banger_states(&mut app);
    assert_eq!(states, vec![(BangerPhase::Dormant, SessionEntity(1))]);
    let banger = app
        .world_mut()
        .query_filtered::<Entity, With<Banger>>()
        .iter(app.world())
        .next()
        .unwrap();
    assert_eq!(
        app.world()
            .get::<BangerPieces>(banger)
            .unwrap()
            .fragments
            .len(),
        2
    );

    // A striker breaks it: the husk goes Broken and two fragment
    // bodies appear — the state a restart must not leave behind.
    spawn_striker(
        &mut app,
        Vec3::new(0.0, 0.5, 8.0),
        Vec3::new(0.0, 0.0, 15.0),
    );
    let events = run(&mut app, FRAMES_PER_SECOND * 3);
    assert!(
        events.iter().any(|e| e.phase == BangerPhase::Broken),
        "the stamped breakable shatters: {events:?}"
    );
    let states = banger_states(&mut app);
    assert_eq!(
        states
            .iter()
            .filter(|(p, _)| *p == BangerPhase::Broken)
            .count(),
        1,
        "one broken husk: {states:?}"
    );
    assert_eq!(states.len(), 3, "husk + two fragments: {states:?}");

    // Restart through the real lifecycle: teardown must remove husk
    // and fragments, the reload restamps the placement dormant.
    app.world_mut().resource_mut::<SessionControl>().restart = true;
    let mut restarted = false;
    for _ in 0..40 {
        app.update();
        let s = app.world().resource::<Session>();
        if s.generation() == 2 && matches!(s.phase(), SessionPhase::Playing) {
            restarted = true;
            break;
        }
    }
    assert!(restarted, "restart never returned to Playing");

    let states = banger_states(&mut app);
    assert_eq!(
        states,
        vec![(BangerPhase::Dormant, SessionEntity(2))],
        "the placement restamps dormant under the new generation"
    );
    let banger = app
        .world_mut()
        .query_filtered::<Entity, With<Banger>>()
        .iter(app.world())
        .next()
        .unwrap();
    assert!(
        app.world().get::<Collider>(banger).is_some(),
        "the restamped prop is a whole collider again"
    );
    assert_eq!(
        app.world()
            .get::<BangerPieces>(banger)
            .unwrap()
            .fragments
            .len(),
        2,
        "its authored pieces are ready to break again"
    );
    let leaked = app
        .world_mut()
        .query::<&SessionEntity>()
        .iter(app.world())
        .filter(|owner| **owner != SessionEntity(2))
        .count();
    assert_eq!(leaked, 0, "no generation-1 entity may survive restart");
}

// ---------------------------------------------------------------------------
// Placement height (operator-reported defect): stamped props rendered
// with their mesh *centre* on the path point, half-buried. Retail
// `dgBangerData` records pin the authored convention — the bound box is
// `CG ± Size/2` and `cg.y = size.y/2` on every measured record (cone
// 0.425/0.85, sawhorse 0.727/1.453, lamp 3.862/7.702, tree 3.5/7.0),
// so the bound's *base* rests on the instance origin while the PKG
// geometry is authored centred at the bound's centre. Stamping must
// offset content by `+CG`; a name with no record lifts so its lowest
// authored vertex rests on the point. INST placements keep their
// authored basis verbatim.
// ---------------------------------------------------------------------------

/// A cube-ish prop authored *centred* on the origin — verts y ∈
/// [−0.5, 0.5] — the way retail prop PKGs are actually modelled
/// (`sp_cone_f`, `sp_sawhrslt_f`, …). `banger_record`'s `CG 0 0.5 0`
/// is exactly its bound centre.
fn centred_pkg() -> Vec<u8> {
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
        ([-0.5, -0.5, -0.5], [0., 1., 0.], [0., 0.]),
        ([0.5, -0.5, 0.5], [0., 1., 0.], [1., 0.]),
        ([-0.5, -0.5, 0.5], [0., 1., 0.], [0., 1.]),
        ([0.0, 0.5, 0.0], [0., 1., 0.], [0.5, 0.5]),
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
    d.push(b"centred_h".len() as u8 + 1);
    d.extend_from_slice(b"centred_h");
    d.push(0);
    d.extend_from_slice(&(geo.len() as u32).to_le_bytes());
    d.extend_from_slice(&geo);
    d
}

/// A minimal `.inst` file stamping each `(name, location)` as a simple
/// placement — unit heading, scale 1.
fn inst_file(comps: &[(&str, [f32; 3])]) -> Vec<u8> {
    let mut d = Vec::new();
    for (name, loc) in comps {
        d.extend_from_slice(&1u16.to_le_bytes()); // room
        d.extend_from_slice(&0x100u16.to_le_bytes()); // modifiers
        d.push(0x80 | (name.len() as u8 + 1)); // simple placement; name + NUL
        d.extend_from_slice(name.as_bytes());
        d.push(0);
        d.extend_from_slice(&1.0f32.to_le_bytes()); // x_delta → unit heading
        d.extend_from_slice(&0.0f32.to_le_bytes()); // z_delta
        for c in loc {
            d.extend_from_slice(&c.to_le_bytes());
        }
    }
    d
}

/// A city that stamps the centred prop three ways: bound via
/// `props.pathset` at (0,0,15), unbound at (3,0,15), and verbatim
/// through `.inst` at (5,0,15) — the road's y is 0 everywhere.
fn centred_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/test.psdl", city_psdl());
    std::fs::create_dir_all(d.join("texture")).unwrap();
    std::fs::write(
        d.join("texture/test_road.png"),
        include_bytes!("../../../assets/texture/dev_road.png"),
    )
    .unwrap();
    write(d, "geometry/centred.pkg", centred_pkg());
    write(d, "geometry/plain.pkg", centred_pkg());
    write(d, "tune/banger/centred.dgbangerdata", banger_record(0.0));
    write(
        d,
        "city/test.inst",
        inst_file(&[("centred", [5.0, 0.0, 15.0])]),
    );
    write(
        d,
        "city/test/props.pathset",
        pth1(&[
            pth1_path("centred", &[[0.0, 0.0, 15.0]], 0, 0),
            pth1_path("plain", &[[3.0, 0.0, 15.0]], 0, 0),
        ]),
    );
    tmp
}

fn aabb_of(app: &mut App, name: &str) -> ColliderAabb {
    let mut q = app.world_mut().query::<(&Name, &ColliderAabb)>();
    q.iter(app.world())
        .find(|(n, _)| n.as_str() == name)
        .map(|(_, a)| *a)
        .unwrap_or_else(|| panic!("no ColliderAabb on {name}"))
}

#[test]
fn stamped_props_rest_their_bound_base_on_the_path_point() {
    let tmp = centred_install();
    let mut vfs = Vfs::new();
    vfs.mount_dir(tmp.path(), 0).unwrap();
    let mut app = city_app(vfs);
    // One update loads the city; a second lets the collider pipeline
    // publish world AABBs.
    app.update();
    app.update();

    // Bound prop at (0, 0, 15): `CG 0 0.5 0` lifts the centred mesh so
    // the bound's *base* rests on the path point — y 0..1, not the
    // half-buried y −0.5..0.5 a verbatim stamp produced.
    let a = aabb_of(&mut app, "pathset-centred-0-0");
    assert!(
        (a.min.y - 0.0).abs() < 0.05,
        "bound base on the point: {a:?}"
    );
    assert!((a.max.y - 1.0).abs() < 0.05, "{a:?}");

    // No record: the same convention recovered from the geometry —
    // the lowest authored vertex rests on the point.
    let p = aabb_of(&mut app, "pathset-plain-1-0-collider");
    assert!((p.min.y - 0.0).abs() < 0.05, "unbound ground lift: {p:?}");
    assert!((p.max.y - 1.0).abs() < 0.05, "{p:?}");

    // INST placements keep their authored basis verbatim — the
    // stamping offset must not leak into that channel.
    let i = aabb_of(&mut app, "prop-centred-collider");
    assert!((i.min.y + 0.5).abs() < 0.05, "INST stays verbatim: {i:?}");
    assert!((i.max.y - 0.5).abs() < 0.05, "{i:?}");
}
