//! F05-B.3 integration: authored breakaway parts read the real
//! deduplicated impact stream, detach once when the delivered impulse
//! estimate exceeds the part's own `ImpulseLimit2`, spawn a pooled
//! fragment body, hide the intact render node, and come back with the
//! disabled outcome's repair. Remote rigs, authoredless cars and stale
//! generations stay bolted together.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::breakaway::{self, BreakFragment, BreakPartVisual, BreakReport};
use mm2_app::contracts::{self, ImpactFilter};
use mm2_app::damage::{self, DamageReport};
use mm2_app::session::{SessionControl, SpawnPoint};
use mm2_app::stuck::{self, StuckReport};
use mm2_game::{
    BangerDefinition, BangerPhase, BangerPool, BangerStateChanged, BreakPartSpec, DamageEvent,
    DamageSpec, ImpactEvent, ImpactId, ObjectId, ObjectIdentity, PartDetached, Player,
    PlayerControl, Session, SessionConfig, SessionEntity, SessionPhase, StuckEvent, SurfaceState,
    VehicleBreaks, VehicleDamage, advance_session_tick,
};
use mm2_vehicle::{TireConditions, VehicleConfig, VehiclePlugin, vehicle_bundle};

/// `ImpulseLimit2 = mass × detach_speed` on the authored records
/// (≈31.25 or ≈2500 m/s retail) — the test part reads as a 5 m/s
/// threshold so ordinary severities discriminate.
const PART_MASS: f32 = 100.0;
const LIMIT: f32 = 500.0;
const DAMAGE_SPEC: DamageSpec = DamageSpec {
    impact_threshold: 1500.0,
    med_damage: 150_000.0,
    max_damage: 321_300.0,
    regenerate_rate: 0.0,
};
const SPAWN: Vec3 = Vec3::new(0.0, 1.5, 0.0);
const SPAWN_YAW: f32 = 0.25;

fn part(name: &str, limit: f32) -> BreakPartSpec {
    BreakPartSpec {
        name: name.into(),
        def: BangerDefinition {
            name: format!("vpcar_{name}"),
            mass: PART_MASS,
            friction: 0.9,
            elasticity: 0.3,
            impulse_limit2: limit,
            size: [0.8, 0.4, 1.2],
            cg: [0.0, 0.2, 0.0],
            num_parts: 0,
        },
    }
}

/// The production FixedLast order: impacts → damage → stuck →
/// breakaway → disabled → stuck-recovery.
fn add_pipeline(app: &mut App) {
    app.add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (
                contracts::collect_impacts,
                damage::apply_impact_damage,
                stuck::track_stuck,
                breakaway::detach_breaks,
                damage::resolve_disabled,
                stuck::resolve_stuck,
            )
                .chain(),
        );
}

/// A playing session on a ground plane plus the player car carrying a
/// `specs` breakaway rig and one tagged render node per spec. Returns
/// the app, the car entity, its object id and the node entities in
/// spec order.
fn break_app(
    specs: Vec<BreakPartSpec>,
    car_pos: Vec3,
    pool: BangerPool,
) -> (App, Entity, ObjectId, Vec<Entity>) {
    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    let object = session.mint_object_id();
    let player = session.mint_player_id();
    let role = session.authority_role();
    let generation = session.generation();

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
        .insert_resource(TireConditions::default())
        .insert_resource(pool)
        .insert_resource(SpawnPoint {
            position: SPAWN,
            yaw: SPAWN_YAW,
            trailers: Vec::new(),
        })
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .add_message::<DamageEvent>()
        .add_message::<StuckEvent>()
        .add_message::<PartDetached>()
        .add_message::<BangerStateChanged>()
        .insert_resource(ImpactFilter::default())
        .init_resource::<DamageReport>()
        .init_resource::<StuckReport>()
        .init_resource::<BreakReport>()
        .init_resource::<SessionControl>();
    add_pipeline(&mut app);
    app.finish();
    app.cleanup();

    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(400.0, 1.0, 400.0),
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));

    let car = app
        .world_mut()
        .spawn((
            mm2_game::PlayerVehicle,
            SessionEntity(generation),
            ObjectIdentity(object),
            Player {
                id: player,
                control: PlayerControl::Local,
            },
            role,
            mm2_game::DamageSignals::default(),
            VehicleDamage::new(DAMAGE_SPEC),
            VehicleBreaks::new(specs.clone()),
            vehicle_bundle(&VehicleConfig::default()),
            Position(car_pos),
            Transform::from_translation(car_pos),
        ))
        .id();
    let nodes: Vec<Entity> = specs
        .iter()
        .enumerate()
        .map(|(i, spec)| {
            let node = app
                .world_mut()
                .spawn((
                    Transform::from_xyz(0.0, 0.4, -1.0 - i as f32),
                    Visibility::Visible,
                    BreakPartVisual {
                        part: spec.name.clone(),
                        local: Transform::from_xyz(0.0, 0.4, -1.0 - i as f32),
                        collider: Some(Collider::cuboid(0.4, 0.2, 0.6)),
                        centroid: Vec3::ZERO,
                    },
                ))
                .id();
            app.world_mut().entity_mut(car).add_child(node);
            node
        })
        .collect();

    // Settle the car + let Avian publish `ComputedMass`, then wipe the
    // baselines: the settle drop is a real impact and can legitimately
    // damage/detach — every test measures only the deliveries it
    // writes.
    app.update();
    drain_parts(&mut app);
    app.world_mut()
        .entity_mut(car)
        .insert(VehicleDamage::new(DAMAGE_SPEC));
    app.world_mut()
        .entity_mut(car)
        .insert(VehicleBreaks::new(specs));
    for node in &nodes {
        app.world_mut()
            .entity_mut(*node)
            .insert(Visibility::Visible);
    }
    app.world_mut().resource_mut::<DamageReport>().reset();
    app.world_mut().resource_mut::<BreakReport>().reset();
    (app, car, object, nodes)
}

/// Deliver one synthetic impact into the session's stream, stamped
/// with the live generation/tick like `collect_impacts` writes.
fn write_impact(app: &mut App, id: u64, a: ObjectId, b: ObjectId, severity: f32) {
    let (generation, tick) = {
        let session = app.world().resource::<Session>();
        (session.generation(), session.tick())
    };
    app.world_mut()
        .resource_mut::<Messages<ImpactEvent>>()
        .write(ImpactEvent {
            id: ImpactId(id),
            generation,
            tick,
            participants: (a, b),
            point: Vec3::ZERO,
            normal: Vec3::Y,
            severity,
            surface: SurfaceState::default(),
        });
}

fn drain_parts(app: &mut App) -> Vec<PartDetached> {
    app.world_mut()
        .resource_mut::<Messages<PartDetached>>()
        .drain()
        .collect()
}

fn report(app: &App) -> &BreakReport {
    app.world().resource::<BreakReport>()
}

fn run(app: &mut App, frames: usize) {
    for _ in 0..frames {
        app.update();
    }
}

#[test]
fn a_panel_detaches_once_and_its_node_hides() {
    let (mut app, car, object, nodes) = break_app(
        vec![part("break0", LIMIT)],
        Vec3::new(0.0, 1.2, 0.0),
        BangerPool::default(),
    );
    // approach 6.0 m/s > 500/100 = 5 m/s — over the authored limit.
    write_impact(&mut app, 1, object, ObjectId::WORLD, 6.0);
    app.update();

    assert_eq!(report(&app).detached, 1);
    assert_eq!(
        *app.world().get::<Visibility>(nodes[0]).unwrap(),
        Visibility::Hidden,
        "the intact node hides so the part appears once"
    );
    // One bounded event carrying the car, the part stem and the
    // fragment's minted identity — drained inside the message
    // lifetime.
    let events = drain_parts(&mut app);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].object, object);
    assert_eq!(events[0].part, "break0");
    assert!(events[0].fragment.is_some());
    // The spawned fragment is a pooled active dynamic body marked
    // with where it came from.
    let fragment = app
        .world_mut()
        .query::<(Entity, &BreakFragment, &mm2_game::Banger, &RigidBody)>()
        .iter(app.world())
        .map(|(e, f, b, rb)| (e, f.vehicle, f.part, b.phase, *rb))
        .collect::<Vec<_>>();
    assert_eq!(fragment.len(), 1);
    assert_eq!(fragment[0].1, car);
    assert_eq!(fragment[0].2, 0);
    assert_eq!(fragment[0].3, BangerPhase::Active);
    assert_eq!(fragment[0].4, RigidBody::Dynamic);

    // A second hit never detaches the same part again — the bounded
    // stream is structural (F05-AC06).
    write_impact(&mut app, 2, object, ObjectId::WORLD, 20.0);
    run(&mut app, 3);
    assert_eq!(report(&app).detached, 1);
    assert!(drain_parts(&mut app).is_empty());
}

#[test]
fn mesh_children_respawn_under_the_fragment() {
    // The fragment's visuals are the intact node's mesh children,
    // re-spawned under the fragment body with cloned handles — the
    // originals stay on the hidden node so nothing renders twice.
    let (mut app, _car, object, nodes) = break_app(
        vec![part("break0", LIMIT)],
        Vec3::new(0.0, 1.2, 0.0),
        BangerPool::default(),
    );
    if !app.world().contains_resource::<Assets<StandardMaterial>>() {
        app.world_mut().init_resource::<Assets<StandardMaterial>>();
    }
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::new(0.4, 0.2, 0.6));
    let mat = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    let panel_child = app
        .world_mut()
        .spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(mat.clone()),
            Transform::IDENTITY,
        ))
        .id();
    app.world_mut().entity_mut(nodes[0]).add_child(panel_child);

    write_impact(&mut app, 1, object, ObjectId::WORLD, 6.0);
    app.update();
    assert_eq!(report(&app).detached, 1);

    let fragments: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<BreakFragment>>()
        .iter(app.world())
        .collect();
    assert_eq!(fragments.len(), 1);
    let children = app
        .world()
        .get::<Children>(fragments[0])
        .expect("the fragment body carries the re-spawned mesh children");
    assert_eq!(children.len(), 1);
    let respawned = children[0];
    assert_ne!(
        respawned, panel_child,
        "a fresh render entity under the fragment, not a reparent"
    );
    assert_eq!(app.world().get::<Mesh3d>(respawned).unwrap().0, mesh);
    assert_eq!(
        app.world()
            .get::<MeshMaterial3d<StandardMaterial>>(respawned)
            .unwrap()
            .0,
        mat
    );
    // The hidden intact node keeps its original child.
    assert_eq!(
        app.world().get::<ChildOf>(panel_child).unwrap().parent(),
        nodes[0]
    );
}

#[test]
fn below_the_authored_limit_the_panel_stays() {
    let (mut app, _car, object, nodes) = break_app(
        vec![part("break0", LIMIT)],
        Vec3::new(0.0, 1.2, 0.0),
        BangerPool::default(),
    );
    // approach 3.0 m/s < 5 m/s — under the authored limit.
    write_impact(&mut app, 1, object, ObjectId::WORLD, 3.0);
    run(&mut app, 3);
    assert_eq!(report(&app).detached, 0);
    assert_eq!(
        *app.world().get::<Visibility>(nodes[0]).unwrap(),
        Visibility::Visible
    );
    assert!(drain_parts(&mut app).is_empty());
}

#[test]
fn each_part_weighs_its_own_limit() {
    let (mut app, _car, object, nodes) = break_app(
        vec![part("break0", LIMIT), part("break1", LIMIT * 10.0)],
        Vec3::new(0.0, 1.2, 0.0),
        BangerPool::default(),
    );
    // 6 m/s exceeds break0's 5 m/s threshold but not break1's 50.
    write_impact(&mut app, 1, object, ObjectId::WORLD, 6.0);
    run(&mut app, 3);
    assert_eq!(report(&app).detached, 1);
    assert_eq!(
        *app.world().get::<Visibility>(nodes[0]).unwrap(),
        Visibility::Hidden
    );
    assert_eq!(
        *app.world().get::<Visibility>(nodes[1]).unwrap(),
        Visibility::Visible,
        "the heavier part rides out the same hit"
    );
    // A harder hit takes the second panel too.
    write_impact(&mut app, 2, object, ObjectId::WORLD, 60.0);
    run(&mut app, 3);
    assert_eq!(report(&app).detached, 2);
    assert_eq!(
        *app.world().get::<Visibility>(nodes[1]).unwrap(),
        Visibility::Hidden
    );
}

#[test]
fn repair_restores_the_rig_and_retires_the_fragment() {
    let (mut app, car, object, nodes) = break_app(
        vec![part("break0", LIMIT)],
        Vec3::new(0.0, 1.2, 0.0),
        BangerPool::default(),
    );
    write_impact(&mut app, 1, object, ObjectId::WORLD, 6.0);
    run(&mut app, 3);
    assert_eq!(report(&app).detached, 1);

    // A wrecking blow: severity 300 × 1300 = 390 000 > MaxDamage —
    // Cruise's FreeReset repairs, and the repair restores the rig
    // (F05-AC03).
    write_impact(&mut app, 2, object, ObjectId::WORLD, 300.0);
    run(&mut app, 5);

    assert_eq!(report(&app).restored, 1);
    assert_eq!(
        *app.world().get::<Visibility>(nodes[0]).unwrap(),
        Visibility::Visible,
        "the repaired car shows its panel again"
    );
    assert!(
        app.world()
            .get::<VehicleBreaks>(car)
            .unwrap()
            .detached_count()
            == 0,
        "the rig is whole again"
    );
    let fragments = app
        .world_mut()
        .query::<&BreakFragment>()
        .iter(app.world())
        .count();
    assert_eq!(fragments, 0, "the spawned fragment despawned");
}

#[test]
fn a_reset_without_repair_keeps_the_parts_off() {
    // The stuck/teleport resets do not repair — only the disabled
    // outcome does. Exercise the real reset path: ResetVehicle moves
    // the car, the rig stays incomplete.
    let (mut app, car, object, nodes) = break_app(
        vec![part("break0", LIMIT)],
        Vec3::new(0.0, 1.2, 0.0),
        BangerPool::default(),
    );
    write_impact(&mut app, 1, object, ObjectId::WORLD, 6.0);
    run(&mut app, 3);
    assert_eq!(report(&app).detached, 1);

    app.world_mut()
        .resource_mut::<Messages<mm2_vehicle::ResetVehicle>>()
        .write(mm2_vehicle::ResetVehicle {
            entity: Some(car),
            position: SPAWN,
            yaw: SPAWN_YAW,
        });
    run(&mut app, 5);

    assert_eq!(report(&app).restored, 0);
    assert_eq!(
        *app.world().get::<Visibility>(nodes[0]).unwrap(),
        Visibility::Hidden,
        "a plain reset is not a repair"
    );
}

#[test]
fn a_remote_participants_rig_is_not_ours_to_detach() {
    let (mut app, _car, _object, _nodes) =
        break_app(vec![], Vec3::new(0.0, 1.2, 0.0), BangerPool::default());
    let remote_object = app.world_mut().resource_mut::<Session>().mint_object_id();
    let remote_player = app.world_mut().resource_mut::<Session>().mint_player_id();
    let role = app.world().resource::<Session>().authority_role();
    let generation = app.world().resource::<Session>().generation();
    let remote = app
        .world_mut()
        .spawn((
            SessionEntity(generation),
            ObjectIdentity(remote_object),
            Player {
                id: remote_player,
                control: PlayerControl::Remote,
            },
            role,
            mm2_game::DamageSignals::default(),
            VehicleBreaks::new(vec![part("break0", LIMIT)]),
            vehicle_bundle(&VehicleConfig::default()),
            Position(Vec3::new(10.0, 1.2, 5.0)),
            Transform::from_xyz(10.0, 1.2, 5.0),
        ))
        .id();
    let node = app
        .world_mut()
        .spawn((
            Transform::IDENTITY,
            Visibility::Visible,
            BreakPartVisual {
                part: "break0".into(),
                local: Transform::IDENTITY,
                collider: Some(Collider::cuboid(0.4, 0.2, 0.6)),
                centroid: Vec3::ZERO,
            },
        ))
        .id();
    app.world_mut().entity_mut(remote).add_child(node);
    app.update();

    // F05 req 12: the remote rig's detachments are its authority's —
    // not observed here.
    write_impact(&mut app, 7, remote_object, ObjectId::WORLD, 60.0);
    run(&mut app, 3);
    assert_eq!(report(&app).detached, 0);
    assert_eq!(
        *app.world().get::<Visibility>(node).unwrap(),
        Visibility::Visible
    );
}

#[test]
fn a_car_with_no_authored_rig_detaches_nothing() {
    // No VehicleBreaks component — the authored-absence rule (no
    // dgbangerdata-backed parts → no rig). An impact still lands; it
    // just finds nothing to detach.
    let (mut app, car, object, _nodes) =
        break_app(vec![], Vec3::new(0.0, 1.2, 0.0), BangerPool::default());
    app.world_mut().entity_mut(car).remove::<VehicleBreaks>();
    write_impact(&mut app, 1, object, ObjectId::WORLD, 60.0);
    run(&mut app, 3);
    assert_eq!(report(&app).detached, 0);
    assert!(drain_parts(&mut app).is_empty());
}

#[test]
fn a_pool_bound_part_still_leaves_the_rig() {
    // max_active = 0: no fragment slot exists, so the part detaches
    // without a body — it still left the rig and the event says so.
    let (mut app, car, object, nodes) = break_app(
        vec![part("break0", LIMIT)],
        Vec3::new(0.0, 1.2, 0.0),
        BangerPool { max_active: 0 },
    );
    write_impact(&mut app, 1, object, ObjectId::WORLD, 6.0);
    app.update();
    assert_eq!(report(&app).detached, 1);
    assert_eq!(
        *app.world().get::<Visibility>(nodes[0]).unwrap(),
        Visibility::Hidden
    );
    let events = drain_parts(&mut app);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].fragment, None);
    assert!(
        app.world().get::<VehicleBreaks>(car).unwrap().parts[0]
            .fragment
            .is_none()
    );
    let fragments = app
        .world_mut()
        .query::<&BreakFragment>()
        .iter(app.world())
        .count();
    assert_eq!(fragments, 0);
}

#[test]
fn stale_generation_impacts_detach_nothing() {
    let (mut app, _car, object, nodes) = break_app(
        vec![part("break0", LIMIT)],
        Vec3::new(0.0, 1.2, 0.0),
        BangerPool::default(),
    );
    app.world_mut()
        .resource_mut::<Messages<ImpactEvent>>()
        .write(ImpactEvent {
            id: ImpactId(1),
            generation: 99,
            tick: 0,
            participants: (object, ObjectId::WORLD),
            point: Vec3::ZERO,
            normal: Vec3::Y,
            severity: 60.0,
            surface: SurfaceState::default(),
        });
    run(&mut app, 3);
    assert_eq!(report(&app).detached, 0);
    assert_eq!(
        *app.world().get::<Visibility>(nodes[0]).unwrap(),
        Visibility::Visible
    );
}

#[test]
fn a_part_with_no_render_node_cannot_detach() {
    // The spec was built from the same model the visuals were, so a
    // missing node is an assembly inconsistency — the part stays
    // attached rather than detaching into nothing.
    let (mut app, car, object, _nodes) = break_app(
        vec![part("break0", LIMIT)],
        Vec3::new(0.0, 1.2, 0.0),
        BangerPool::default(),
    );
    // Tag the node with a name the rig does not know.
    let stray = app
        .world_mut()
        .spawn((
            Transform::IDENTITY,
            Visibility::Visible,
            BreakPartVisual {
                part: "break99".into(),
                local: Transform::IDENTITY,
                collider: Some(Collider::cuboid(0.4, 0.2, 0.6)),
                centroid: Vec3::ZERO,
            },
        ))
        .id();
    app.world_mut().entity_mut(car).add_child(stray);
    // Drop the matching node.
    let matching: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &BreakPartVisual)>()
        .iter(app.world())
        .filter(|(_, v)| v.part == "break0")
        .map(|(e, _)| e)
        .collect();
    for e in matching {
        app.world_mut().entity_mut(e).despawn();
    }

    write_impact(&mut app, 1, object, ObjectId::WORLD, 6.0);
    run(&mut app, 3);
    assert_eq!(report(&app).detached, 0);
    assert!(
        app.world().get::<VehicleBreaks>(car).unwrap().parts[0].attached,
        "no node, no detach — the inconsistency stays visible in the rig"
    );
}
