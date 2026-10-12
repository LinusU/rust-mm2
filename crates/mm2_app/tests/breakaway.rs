//! F05-B.3 integration: authored breakaway parts read the real
//! deduplicated impact stream and detach — one part per qualifying
//! impact, the attached part nearest the contact point — when the
//! delivered impulse reaches the recovered manager-wide gate (REC-2),
//! spawn a pooled fragment body, hide the intact render node, and come
//! back with the disabled outcome's repair. A predicted session drains
//! the stream without touching a rig (detach truth belongs to the
//! authority), while authoredless cars and stale generations stay
//! bolted together.

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
/// (≈31.25 or ≈2500 m/s retail) — fragment data under the recovered
/// rule, which never gates detachment on it. Kept so the fragment
/// physicals read like the real records.
const PART_MASS: f32 = 100.0;
const LIMIT: f32 = 500.0;
const DAMAGE_SPEC: DamageSpec = DamageSpec {
    impact_threshold: 1500.0,
    med_damage: 150_000.0,
    max_damage: 321_300.0,
    regenerate_rate: 0.0,
};
/// The dev car's authored mass (`VehicleConfig::default`) — a wall
/// hit's delivered impulse is `severity × 1300`, so the recovered
/// manager gate (10 000 kg·m/s) sits at ≈7.7 m/s of approach speed.
const CAR_MASS: f32 = 1300.0;
/// A wall-hit approach speed comfortably over the gate (9 × 1300 =
/// 11 700 kg·m/s).
const OVER: f32 = 9.0;
/// Under the gate but over `ImpactThreshold` (5 × 1300 = 6 500) — the
/// hit damages the car yet takes no part.
const UNDER: f32 = 5.0;
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
            audio_id: 0,
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
    break_app_as(specs, car_pos, pool, SessionConfig::default())
}

/// [`break_app`] with a caller-chosen session config — the predicted
/// leg needs `authority: SessionAuthority::Remote`.
fn break_app_as(
    specs: Vec<BreakPartSpec>,
    car_pos: Vec3,
    pool: BangerPool,
    config: SessionConfig,
) -> (App, Entity, ObjectId, Vec<Entity>) {
    let mut session = Session::new();
    session.begin(config).unwrap();
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
        .insert_resource(SpawnPoint::new(SPAWN, SPAWN_YAW))
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
        .init_resource::<mm2_app::recovery::RecoveryReport>()
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<mm2_app::texel_fx::TexelDamageReport>()
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
/// with the live generation/tick like `collect_impacts` writes. The
/// contact point is the world origin — fine for a one-part rig or when
/// proximity does not matter.
fn write_impact(app: &mut App, id: u64, a: ObjectId, b: ObjectId, severity: f32) {
    write_impact_at(app, id, a, b, Vec3::ZERO, severity);
}

/// [`write_impact`] with an explicit contact point — the nearest-part
/// scan (REC-2) keys on it.
fn write_impact_at(app: &mut App, id: u64, a: ObjectId, b: ObjectId, point: Vec3, severity: f32) {
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
            point,
            normal: Vec3::Y,
            severity,
            surface: SurfaceState::default(),
        });
}

/// A break node's anchor in the world — the car's live physics pose
/// times the node's stored local transform, the exact quantity
/// `detach_breaks` scans (REC-2). Read it *after* settling so a test's
/// contact point is scoped to the pose the system will use.
fn anchor(app: &App, car: Entity, node: Entity) -> Vec3 {
    let pos = app
        .world()
        .get::<Position>(car)
        .map(|p| p.0)
        .unwrap_or(Vec3::ZERO);
    let rot = app
        .world()
        .get::<Rotation>(car)
        .map(|r| r.0)
        .unwrap_or(Quat::IDENTITY);
    let local = app.world().get::<BreakPartVisual>(node).unwrap().local;
    pos + rot * local.translation
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
    // 9 m/s against the world: 9 × 1300 = 11 700 kg·m/s, over the
    // manager gate (REC-2).
    write_impact(&mut app, 1, object, ObjectId::WORLD, OVER);
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
    // The reported estimate is the impact impulse that tripped the
    // gate, not the part's authored limit.
    assert!((events[0].estimate - OVER * CAR_MASS).abs() < 1.0);
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
    write_impact(&mut app, 2, object, ObjectId::WORLD, OVER * 2.0);
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

    write_impact(&mut app, 1, object, ObjectId::WORLD, OVER);
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
fn below_the_manager_gate_the_panel_stays() {
    let (mut app, _car, object, nodes) = break_app(
        vec![part("break0", LIMIT)],
        Vec3::new(0.0, 1.2, 0.0),
        BangerPool::default(),
    );
    // 5 m/s against the world: 6 500 kg·m/s, under the manager gate (and
    // well over the authored `ImpulseLimit2` reading the old gate used).
    write_impact(&mut app, 1, object, ObjectId::WORLD, UNDER);
    run(&mut app, 3);
    assert_eq!(report(&app).detached, 0);
    assert_eq!(
        *app.world().get::<Visibility>(nodes[0]).unwrap(),
        Visibility::Visible
    );
    assert!(drain_parts(&mut app).is_empty());
}

#[test]
fn the_dev_car_resolves_the_mass_the_gate_arithmetic_assumes() {
    // The constants above derive their impulses from `CAR_MASS`; pin it
    // to the live body so a changed dev car fails here, loudly, rather
    // than silently shifting what `OVER`/`UNDER` mean.
    let (app, car, _object, _nodes) = break_app(
        vec![part("break0", LIMIT)],
        Vec3::new(0.0, 1.2, 0.0),
        BangerPool::default(),
    );
    let mass = app.world().get::<ComputedMass>(car).map(|m| m.value());
    assert_eq!(mass, Some(CAR_MASS));
}

#[test]
fn one_qualifying_impact_takes_one_panel_the_nearest() {
    let (mut app, car, object, nodes) = break_app(
        vec![part("break0", LIMIT), part("break1", LIMIT)],
        Vec3::new(0.0, 1.2, 0.0),
        BangerPool::default(),
    );
    // A contact by the second part's anchor: it is the one that leaves,
    // even though both parts are equally detachable (REC-2).
    let near = anchor(&app, car, nodes[1]);
    write_impact_at(&mut app, 1, object, ObjectId::WORLD, near, OVER);
    app.update();
    assert_eq!(report(&app).detached, 1, "one part per impact");
    assert_eq!(
        *app.world().get::<Visibility>(nodes[0]).unwrap(),
        Visibility::Visible
    );
    assert_eq!(
        *app.world().get::<Visibility>(nodes[1]).unwrap(),
        Visibility::Hidden
    );
    let events = drain_parts(&mut app);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].part, "break1");

    // The next qualifying impact takes the remaining attached part.
    let near = anchor(&app, car, nodes[0]);
    write_impact_at(&mut app, 2, object, ObjectId::WORLD, near, OVER);
    app.update();
    assert_eq!(report(&app).detached, 2);
    assert_eq!(
        *app.world().get::<Visibility>(nodes[0]).unwrap(),
        Visibility::Hidden
    );
}

#[test]
fn the_nearest_part_detaches_whatever_its_authored_limit() {
    // The part nearest the contact authors an effectively infinite
    // `ImpulseLimit2`; the far part authors a tiny one. The recovered
    // rule takes the nearest *regardless* — the authored limit is not
    // consulted (the old per-part gate would have taken the far one).
    let (mut app, car, object, nodes) = break_app(
        vec![part("break0", 1.0), part("break1", 1_000_000_000.0)],
        Vec3::new(0.0, 1.2, 0.0),
        BangerPool::default(),
    );
    let near = anchor(&app, car, nodes[1]);
    write_impact_at(&mut app, 1, object, ObjectId::WORLD, near, OVER);
    run(&mut app, 3);
    assert_eq!(report(&app).detached, 1);
    assert_eq!(
        *app.world().get::<Visibility>(nodes[1]).unwrap(),
        Visibility::Hidden,
        "the nearest part leaves despite its huge authored limit"
    );
    assert_eq!(
        *app.world().get::<Visibility>(nodes[0]).unwrap(),
        Visibility::Visible
    );
}

#[test]
fn repair_restores_the_rig_and_retires_the_fragment() {
    let (mut app, car, object, nodes) = break_app(
        vec![part("break0", LIMIT)],
        Vec3::new(0.0, 1.2, 0.0),
        BangerPool::default(),
    );
    write_impact(&mut app, 1, object, ObjectId::WORLD, OVER);
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
    write_impact(&mut app, 1, object, ObjectId::WORLD, OVER);
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
fn a_remote_seats_rig_sheds_on_the_authority() {
    // F25-B (protocol v11): on the authority a remote driver's seat is
    // a simulated participant like an AI — this app *is* that
    // authority, so the rig sheds here and the detached bitmask rides
    // its `SnapEntry` down to the copies, which reconcile it as state.
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

    write_impact(&mut app, 7, remote_object, ObjectId::WORLD, 60.0);
    app.update();
    assert_eq!(report(&app).detached, 1);
    assert_eq!(
        *app.world().get::<Visibility>(node).unwrap(),
        Visibility::Hidden
    );
    // The authority still reports the detach on its own stream —
    // replication rides the `SnapEntry` bitmask, not this event.
    let events = drain_parts(&mut app);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].object, remote_object);
}

#[test]
fn a_predicted_session_detaches_nothing_locally() {
    // The other half of the contract: a client never runs detach —
    // `detach_breaks` drains the stream under a predicted session and
    // the replicated `SnapEntry.breaks` bitmask is a copy's only
    // detach truth (`netdrive::apply_snapshots`).
    let (mut app, _car, object, nodes) = break_app_as(
        vec![part("break0", LIMIT)],
        Vec3::new(0.0, 1.2, 0.0),
        BangerPool::default(),
        SessionConfig {
            authority: mm2_game::SessionAuthority::Remote,
            ..SessionConfig::default()
        },
    );
    write_impact(&mut app, 1, object, ObjectId::WORLD, 60.0);
    run(&mut app, 3);
    assert_eq!(report(&app).detached, 0);
    assert_eq!(
        *app.world().get::<Visibility>(nodes[0]).unwrap(),
        Visibility::Visible
    );
    assert!(drain_parts(&mut app).is_empty());
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
    write_impact(&mut app, 1, object, ObjectId::WORLD, OVER);
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

    write_impact(&mut app, 1, object, ObjectId::WORLD, OVER);
    run(&mut app, 3);
    assert_eq!(report(&app).detached, 0);
    assert!(
        app.world().get::<VehicleBreaks>(car).unwrap().parts[0].attached,
        "no node, no detach — the inconsistency stays visible in the rig"
    );
}

/// F05-B.3 / REC-2 / DSN-21 original-content check: the authored
/// vehicle fragment ladder is `ImpulseLimit2 = fragment mass × a
/// per-part detach speed` (≈31.25 m/s on the reachable panels), which
/// the retired DSN-21 gate read per part. The recovered rule never
/// consults it — one part leaves when the impact's delivered impulse
/// reaches the manager-wide `10000` kg·m/s, and the nearest authored
/// anchor wins. This loads the real `vpcoop` rig, pins that every
/// authored limit is ignored, and that the retail anchors are the
/// scan's inputs. Skipped without the operator's install
/// (`MM2_RETAIL=<dir>`).
#[test]
fn retail_breakaway_gate_is_manager_wide_not_per_part() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set; retail breakaway gate NOT checked");
        return;
    };
    let mut vfs = mm2_assets::Vfs::new();
    mm2_assets::mount_install(&mut vfs, &retail, &mm2_assets::InstallMount::default())
        .expect("the install mounts");
    let Ok(def) = mm2_content::load_vehicle(&vfs, "vpcoop", 0) else {
        eprintln!("vpcoop not in the install; skipping the retail breakaway check");
        return;
    };
    assert!(
        def.breaks.len() >= 2,
        "vpcoop authors multiple break parts, got {}",
        def.breaks.len()
    );
    let specs: Vec<BreakPartSpec> = def
        .breaks
        .iter()
        .map(|b| BreakPartSpec {
            name: b.name.clone(),
            def: b.def.clone(),
        })
        .collect();

    // Every authored `ImpulseLimit2` sits below the recovered manager
    // gate (the whole retail vehicle ladder does: ≈31.25 m/s panels,
    // the ≈2500 m/s "never detach" class on other cars). A single-part
    // rig flips at exactly `DETACH_IMPULSE` whatever its authored limit
    // — and does *not* flip on that limit, which is the DSN-21 reading
    // this change retires.
    for spec in &specs {
        let limit = spec.def.impulse_limit2;
        assert!(
            limit < VehicleBreaks::DETACH_IMPULSE,
            "{} authors ImpulseLimit2={limit} at/over the manager gate",
            spec.def.name
        );
        let rig = VehicleBreaks::new(vec![spec.clone()]);
        assert!(
            !rig.detaches_on(limit),
            "{} still detaches on its authored ImpulseLimit2 ({limit})",
            spec.def.name
        );
        assert!(!rig.detaches_on(VehicleBreaks::DETACH_IMPULSE - 1.0));
        assert!(rig.detaches_on(VehicleBreaks::DETACH_IMPULSE));
    }

    // The nearest authored anchor wins. `BreakPartVisual.local` is the
    // model part's `origin` (`car_visual` breaks), so the retail model
    // origins are exactly the scan's inputs.
    let anchor_of = |name: &str| -> Vec3 {
        def.model
            .parts
            .iter()
            .find(|p| p.name == name)
            .and_then(|p| p.origin)
            .map(Vec3::from)
            .unwrap_or(Vec3::ZERO)
    };
    let anchors: Vec<(usize, Vec3)> = specs
        .iter()
        .enumerate()
        .map(|(i, s)| (i, anchor_of(&s.name)))
        .collect();
    let rig = VehicleBreaks::new(specs.clone());
    for (i, anchor) in &anchors {
        let pick = rig
            .nearest_attached(*anchor, anchors.iter().copied())
            .expect("an attached part is nearest");
        assert!(
            (anchors[pick].1 - *anchor).length() < 1.0e-3,
            "a contact on part {i}'s anchor {} chose part {pick} anchored {:.3} m away",
            anchor,
            (anchors[pick].1 - *anchor).length(),
        );
    }

    // The calibration the constants imply: the authored panels needed
    // their per-part ≈31.25 m/s, the recovered manager gate is a far
    // lower speed for the same car.
    let mass = def.config.mass;
    let authored_speed = specs
        .iter()
        .map(|s| s.def.impulse_limit2 / s.def.mass)
        .fold(0.0f32, f32::max);
    eprintln!(
        "vpcoop: {} parts, mass {mass:.0} kg; authored panels ≈{authored_speed:.1} m/s, recovered gate {:.1} m/s",
        specs.len(),
        VehicleBreaks::DETACH_IMPULSE / mass,
    );
}

/// F22-B.1 review repair: the cockpit visibility split must never
/// re-show a node the detach path owns. A panel detached under the
/// default Chase mode stays hidden (it used to reappear one frame
/// later, on top of its own fragment), and a panel detached *inside*
/// the cockpit view keeps its `Hidden` after the mode leaves — the
/// detach claims the node by dropping the split's `CockpitHidden` tag.
#[test]
fn detached_panels_stay_hidden_across_cockpit_cycles() {
    use mm2_app::camera::CameraMode;
    use mm2_app::dash::CockpitHidden;

    let (mut app, _car, object, nodes) = break_app(
        vec![part("break0", LIMIT), part("break1", LIMIT * 10.0)],
        Vec3::new(0.0, 1.2, 0.0),
        BangerPool::default(),
    );
    app.insert_resource(CameraMode::Chase);
    app.init_resource::<mm2_app::hud::HudVisible>();
    app.add_systems(Update, mm2_app::dash::sync_dash_visibility);

    // break0 detaches in default Chase.
    write_impact(&mut app, 1, object, ObjectId::WORLD, OVER);
    app.update();
    assert_eq!(report(&app).detached, 1);
    run(&mut app, 3);
    assert_eq!(
        *app.world().get::<Visibility>(nodes[0]).unwrap(),
        Visibility::Hidden,
        "the sweep must not re-show a detached panel in Chase"
    );
    assert_eq!(
        *app.world().get::<Visibility>(nodes[1]).unwrap(),
        Visibility::Visible
    );

    // Cockpit: the intact panel hides under the split's tag; the
    // already-detached one stays its owner's — untagged.
    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::Cockpit;
    app.update();
    assert_eq!(
        *app.world().get::<Visibility>(nodes[1]).unwrap(),
        Visibility::Hidden
    );
    assert!(app.world().get::<CockpitHidden>(nodes[1]).is_some());
    assert!(app.world().get::<CockpitHidden>(nodes[0]).is_none());

    // break1 detaches inside the cockpit view: the detach claims the
    // node's Hidden and drops the split's tag.
    write_impact(&mut app, 2, object, ObjectId::WORLD, 60.0);
    app.update();
    assert_eq!(report(&app).detached, 2);
    assert!(
        app.world().get::<CockpitHidden>(nodes[1]).is_none(),
        "the detach owns the node's Hidden now"
    );
    assert_eq!(
        *app.world().get::<Visibility>(nodes[1]).unwrap(),
        Visibility::Hidden
    );

    // Leaving the mode restores neither detached node.
    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::Chase;
    app.update();
    run(&mut app, 3);
    for node in &nodes {
        assert_eq!(
            *app.world().get::<Visibility>(*node).unwrap(),
            Visibility::Hidden,
            "a detached panel stays hidden after the cockpit cycle"
        );
        assert!(app.world().get::<CockpitHidden>(*node).is_none());
    }
}
