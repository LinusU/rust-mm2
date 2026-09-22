//! F05-B.1 integration: authored `vehcardamage` specs accumulate
//! through the production impact stream, `DamageEvent`s report each
//! application, and `Disabled` outcomes drive the session's real
//! reset/restart paths — synthetic `ImpactEvent`s for the deterministic
//! legs, a real Avian roof drop for the end-to-end one.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::contracts::{self, ImpactFilter};
use mm2_app::damage::{self, DamageReport};
use mm2_app::session::{SessionControl, SpawnPoint};
use mm2_game::{
    Checkpoint, CheckpointRule, DamageEvent, DamageSpec, DamageTier, EventParams, EventRef,
    EventTableKind, ImpactEvent, ImpactId, ObjectId, ObjectIdentity, Player, PlayerControl,
    RaceDefinition, RacePhase, RaceStart, RaceState, Session, SessionConfig, SessionMode,
    SessionPhase, SurfaceState, VehicleDamage, advance_session_tick,
};
use mm2_vehicle::{TireConditions, VehicleConfig, VehiclePlugin, vehicle_bundle};

/// `VehicleConfig::default().mass` — the impulse a world hit delivers
/// is `severity × MASS`, so the tests pick severities off the authored
/// threshold exactly.
const MASS: f32 = 1300.0;
/// A mid-roster retail-shaped spec (vpbug-shaped bounds).
const SPEC: DamageSpec = DamageSpec {
    impact_threshold: 1500.0,
    med_damage: 150_000.0,
    max_damage: 321_300.0,
    regenerate_rate: 0.0,
};
const SPAWN: Vec3 = Vec3::new(0.0, 1.5, 0.0);
const SPAWN_YAW: f32 = 0.25;

fn cruise_config() -> SessionConfig {
    SessionConfig::default()
}

fn event_config(table: EventTableKind) -> SessionConfig {
    SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "sf".into(),
            table,
            index: 0,
        }),
        ..SessionConfig::default()
    }
}

/// A playing session on a marked ground plane plus the player car with
/// authored damage bounds. Returns the app, the car entity and its
/// minted object id.
fn damage_app(config: SessionConfig, car_pos: Vec3) -> (App, Entity, ObjectId) {
    let mut session = Session::new();
    session.begin(config).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    let object = session.mint_object_id();
    let player = session.mint_player_id();
    let role = session.authority_role();

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
        .insert_resource(SpawnPoint {
            position: SPAWN,
            yaw: SPAWN_YAW,
            trailers: Vec::new(),
        })
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .add_message::<DamageEvent>()
        .insert_resource(ImpactFilter::default())
        .init_resource::<DamageReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<mm2_app::recovery::RecoveryReport>()
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<SessionControl>()
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (
                contracts::collect_impacts,
                damage::apply_impact_damage,
                damage::resolve_disabled,
            )
                .chain(),
        );
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
            ObjectIdentity(object),
            Player {
                id: player,
                control: PlayerControl::Local,
            },
            role,
            mm2_game::DamageSignals::default(),
            VehicleDamage::new(SPEC),
            vehicle_bundle(&VehicleConfig::default()),
            Position(car_pos),
            Transform::from_translation(car_pos),
        ))
        .id();

    // Settle the car + let Avian publish `ComputedMass`, then wipe the
    // baseline: the settle drop is a real impact and may legitimately
    // accumulate — every test measures only the deliveries it writes.
    // The component is replaced rather than repaired so its impact-id
    // watermark restarts too (a settle impact's real id must not make
    // the test's synthetic ids look like re-deliveries).
    app.update();
    drain_damage(&mut app);
    app.world_mut()
        .entity_mut(car)
        .insert(VehicleDamage::new(SPEC));
    app.world_mut().resource_mut::<DamageReport>().reset();
    (app, car, object)
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

fn drain_damage(app: &mut App) -> Vec<DamageEvent> {
    app.world_mut()
        .resource_mut::<Messages<DamageEvent>>()
        .drain()
        .collect()
}

fn report(app: &App) -> &DamageReport {
    app.world().resource::<DamageReport>()
}

#[test]
fn sub_threshold_and_garbage_impacts_never_accumulate() {
    let (mut app, car, object) = damage_app(cruise_config(), Vec3::new(0.0, 1.2, 0.0));
    // F05-AC01: severities whose impulse sits at/below the authored
    // floor never reach the accumulator — resting contact and curb
    // taps cannot damage.
    for (id, severity) in [(1, 0.5), (2, 1.0), (3, f32::NAN), (4, f32::NEG_INFINITY)] {
        write_impact(&mut app, id, object, ObjectId::WORLD, severity);
    }
    // A world-world event involves no damageable participant at all.
    write_impact(&mut app, 5, ObjectId::WORLD, ObjectId::WORLD, 50.0);
    app.update();

    assert_eq!(report(&app).rejected, 4);
    assert_eq!(report(&app).applied, 0);
    assert!(drain_damage(&mut app).is_empty());
    assert_eq!(app.world().get::<VehicleDamage>(car).unwrap().total(), 0.0);
    // Just under: 1500/1300 ≈ 1.1538 m/s — the authored floor is in
    // impulse units, not approach speed.
    write_impact(&mut app, 6, object, ObjectId::WORLD, 1.15);
    app.update();
    assert_eq!(report(&app).rejected, 5);
    assert_eq!(app.world().get::<VehicleDamage>(car).unwrap().total(), 0.0);
}

#[test]
fn accepted_impacts_accumulate_tiers_and_emit_events() {
    let (mut app, car, object) = damage_app(cruise_config(), Vec3::new(0.0, 1.2, 0.0));
    // A 2 m/s wall hit: 2600 impulse — over the 1500 floor, still Intact.
    write_impact(&mut app, 1, object, ObjectId::WORLD, 2.0);
    app.update();
    let events = drain_damage(&mut app);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].object, object);
    assert_eq!(events[0].impact, ImpactId(1));
    assert_eq!(events[0].severity, 2.0 * MASS);
    assert_eq!(events[0].total, 2.0 * MASS);
    assert_eq!(events[0].tier, DamageTier::Intact);

    // A 120 m/s hit: +156 000 impulse — past MedDamage, into the band.
    write_impact(&mut app, 2, object, ObjectId::WORLD, 120.0);
    app.update();
    let events = drain_damage(&mut app);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].tier, DamageTier::Damaged);
    assert_eq!(events[0].total, 2.0 * MASS + 120.0 * MASS);
    assert_eq!(report(&app).applied, 2);
    assert_eq!(
        app.world().get::<VehicleDamage>(car).unwrap().condition(),
        DamageTier::Damaged
    );
}

#[test]
fn duplicate_and_stale_impact_ids_never_double_apply() {
    let (mut app, car, object) = damage_app(cruise_config(), Vec3::new(0.0, 1.2, 0.0));
    write_impact(&mut app, 5, object, ObjectId::WORLD, 10.0);
    app.update();
    // Re-delivery of the same impact and a stale earlier id: F05-AC06
    // says neither may accumulate again.
    write_impact(&mut app, 5, object, ObjectId::WORLD, 10.0);
    write_impact(&mut app, 3, object, ObjectId::WORLD, 10.0);
    app.update();

    let damage = app.world().get::<VehicleDamage>(car).unwrap();
    assert_eq!(damage.total(), 10.0 * MASS, "applied exactly once");
    assert_eq!(report(&app).applied, 1);
    assert_eq!(report(&app).duplicate, 2);
    // Only the accepted delivery emitted.
    assert_eq!(drain_damage(&mut app).len(), 1);
}

#[test]
fn a_car_without_authored_damage_is_undamageable() {
    let (mut app, _car, _object) = damage_app(cruise_config(), Vec3::new(0.0, 1.2, 0.0));
    // An authored absence (retail's vpmoonrover ships no vehcardamage)
    // means no spec — the impact pipeline skips the entity entirely
    // rather than borrowing a fabricated bound.
    let bare_object = app.world_mut().resource_mut::<Session>().mint_object_id();
    let bare = app
        .world_mut()
        .spawn((
            ObjectIdentity(bare_object),
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            Mass(800.0),
            Position(Vec3::new(5.0, 0.5, 0.0)),
            Transform::from_xyz(5.0, 0.5, 0.0),
        ))
        .id();
    write_impact(&mut app, 1, bare_object, ObjectId::WORLD, 300.0);
    app.update();
    assert_eq!(report(&app).applied, 0);
    assert!(app.world().get::<VehicleDamage>(bare).is_none());
    assert!(drain_damage(&mut app).is_empty());
}

#[test]
fn stale_generation_impacts_are_ignored() {
    let (mut app, car, object) = damage_app(cruise_config(), Vec3::new(0.0, 1.2, 0.0));
    // A buffered event from a dead session must never flush damage
    // into the live one — generation is the stamp that tells them
    // apart.
    app.world_mut()
        .resource_mut::<Messages<ImpactEvent>>()
        .write(ImpactEvent {
            id: ImpactId(1),
            generation: 99,
            tick: 0,
            participants: (object, ObjectId::WORLD),
            point: Vec3::ZERO,
            normal: Vec3::Y,
            severity: 300.0,
            surface: SurfaceState::default(),
        });
    app.update();
    assert_eq!(report(&app).applied, 0);
    assert_eq!(app.world().get::<VehicleDamage>(car).unwrap().total(), 0.0);
}

#[test]
fn disabled_in_cruise_resets_to_spawn_and_repairs() {
    let (mut app, car, object) = damage_app(cruise_config(), Vec3::new(50.0, 1.2, 30.0));
    // One hit past MaxDamage — destruction in free roam is the
    // documented free recovery (DMG-2's designed cruise leg).
    write_impact(&mut app, 1, object, ObjectId::WORLD, 300.0);
    app.update();

    // The tier event fired…
    let events = drain_damage(&mut app);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].tier, DamageTier::Disabled);
    // …the authority reset the car to the spawn point through the
    // production ResetVehicle path and repaired the damage.
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!(
        pos.distance(SPAWN) < 0.01,
        "car must be teleported to spawn, got {pos}"
    );
    assert_eq!(
        app.world().get::<LinearVelocity>(car).unwrap().0,
        Vec3::ZERO
    );
    let damage = app.world().get::<VehicleDamage>(car).unwrap();
    assert_eq!(damage.total(), 0.0);
    assert_eq!(damage.condition(), DamageTier::Intact);
    assert_eq!(report(&app).recovered, 1);
    assert!(
        !app.world().resource::<SessionControl>().restart,
        "a cruise recovery never restarts the session"
    );
}

#[test]
fn disabled_in_circuit_penalizes_the_clock_and_resets_in_place() {
    let def = RaceDefinition {
        checkpoints: vec![Checkpoint {
            center: Vec3::new(0.0, 0.0, 100.0),
            radius: 8.0,
            height: 8.0,
            heading_deg: 0.0,
            require_direction: false,
        }],
        finish: None,
        rule: CheckpointRule::Ordered,
        laps: 2,
        time_limit_ticks: None,
        params: EventParams::default(),
        countdown_ticks: 0,
        start_slots: vec![RaceStart {
            position: SPAWN,
            yaw_deg: Some(0.0),
        }],
    };
    let (mut app, car, object) = damage_app(
        event_config(EventTableKind::Circuit),
        Vec3::new(40.0, 1.2, -20.0),
    );
    let generation = app.world().resource::<Session>().generation();
    let mut race = RaceState::new(def, generation);
    race.phase = RacePhase::Running;
    race.clock = 1200;
    app.insert_resource(race);

    write_impact(&mut app, 1, object, ObjectId::WORLD, 300.0);
    app.update();

    // RACE-5/DMG-2: destruction in Circuit = time penalty + reset.
    // The penalty rides the real race clock; the reset is in place.
    let race = app.world().resource::<RaceState>();
    assert_eq!(
        race.clock,
        1200 + mm2_game::DISABLED_PENALTY_TICKS,
        "the designed penalty lands on the live clock"
    );
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!(
        pos.distance(Vec3::new(40.0, pos.y, -20.0)) < 1.0,
        "in-place reset keeps the car where it wrecked, got {pos}"
    );
    assert_eq!(app.world().get::<VehicleDamage>(car).unwrap().total(), 0.0);
    assert_eq!(report(&app).recovered, 1);
    assert!(!app.world().resource::<SessionControl>().restart);
}

#[test]
fn disabled_in_blitz_queues_the_session_restart() {
    let (mut app, _car, object) = damage_app(
        event_config(EventTableKind::Blitz),
        Vec3::new(0.0, 1.2, 0.0),
    );
    write_impact(&mut app, 1, object, ObjectId::WORLD, 300.0);
    app.update();
    // RACE-5/DMG-2: destruction in Blitz restarts the event — the
    // session's own restart intent drives the production teardown +
    // re-`begin`, never a damage-specific shortcut.
    assert!(
        app.world().resource::<SessionControl>().restart,
        "the event-restart intent must be queued"
    );
    assert_eq!(report(&app).recovered, 0, "a restart is not a repair");
}

#[test]
fn an_opponent_disabled_resets_in_place_without_restarting_the_race() {
    let (mut app, _car, _object) = damage_app(
        event_config(EventTableKind::Blitz),
        Vec3::new(0.0, 1.2, 0.0),
    );
    // An AI participant carries the same authored spec — but its
    // wreck must never restart the player's event (designed
    // opponent outcome, UNK-13).
    let ai_object = app.world_mut().resource_mut::<Session>().mint_object_id();
    let ai_player = app.world_mut().resource_mut::<Session>().mint_player_id();
    let role = app.world().resource::<Session>().authority_role();
    let ai_pos = Vec3::new(10.0, 1.2, 5.0);
    let ai = app
        .world_mut()
        .spawn((
            ObjectIdentity(ai_object),
            Player {
                id: ai_player,
                control: PlayerControl::Ai,
            },
            role,
            mm2_game::DamageSignals::default(),
            VehicleDamage::new(SPEC),
            vehicle_bundle(&VehicleConfig::default()),
            Position(ai_pos),
            Transform::from_translation(ai_pos),
        ))
        .id();
    app.update();
    drain_damage(&mut app);

    write_impact(&mut app, 7, ai_object, ObjectId::WORLD, 300.0);
    app.update();

    assert_eq!(app.world().get::<VehicleDamage>(ai).unwrap().total(), 0.0);
    let pos = app.world().get::<Position>(ai).unwrap().0;
    assert!(
        pos.distance(Vec3::new(ai_pos.x, pos.y, ai_pos.z)) < 1.0,
        "the wreck resets in place, got {pos}"
    );
    assert!(
        !app.world().resource::<SessionControl>().restart,
        "an AI wreck never restarts the player's event"
    );
    assert_eq!(report(&app).recovered, 1);
}

#[test]
fn two_disabling_impacts_in_one_tick_resolve_once() {
    let (mut app, car, object) = damage_app(
        event_config(EventTableKind::Circuit),
        Vec3::new(40.0, 1.2, -20.0),
    );
    let generation = app.world().resource::<Session>().generation();
    let mut race = RaceState::new(
        RaceDefinition {
            checkpoints: vec![Checkpoint {
                center: Vec3::new(0.0, 0.0, 100.0),
                radius: 8.0,
                height: 8.0,
                heading_deg: 0.0,
                require_direction: false,
            }],
            finish: None,
            rule: CheckpointRule::Ordered,
            laps: 2,
            time_limit_ticks: None,
            params: EventParams::default(),
            countdown_ticks: 0,
            start_slots: vec![RaceStart {
                position: SPAWN,
                yaw_deg: Some(0.0),
            }],
        },
        generation,
    );
    race.phase = RacePhase::Running;
    race.clock = 1200;
    app.insert_resource(race);

    // Two distinct impacts in the same tick both land past MaxDamage —
    // F05-AC06: the second `Disabled` event finds the state already
    // repaired, so the penalty and the reset fire exactly once.
    write_impact(&mut app, 1, object, ObjectId::WORLD, 300.0);
    write_impact(&mut app, 2, object, ObjectId::WORLD, 300.0);
    app.update();

    assert_eq!(drain_damage(&mut app).len(), 2);
    assert_eq!(
        app.world().resource::<RaceState>().clock,
        1200 + mm2_game::DISABLED_PENALTY_TICKS,
        "one penalty, not two"
    );
    assert_eq!(report(&app).recovered, 1);
    assert_eq!(app.world().get::<VehicleDamage>(car).unwrap().total(), 0.0);
}

#[test]
fn a_roof_drop_damages_through_the_real_pipeline() {
    // End-to-end: a real Avian impact → `collect_impacts` →
    // `apply_impact_damage`. The wheels are raycasts, so a meaningful
    // chassis impact needs the body itself to arrive — drop the car
    // on its roof from 4 m.
    let inverted = Quat::from_rotation_x(std::f32::consts::PI);
    let (mut app, car, _) = damage_app(cruise_config(), Vec3::new(0.0, 4.0, 0.0));
    {
        let world = app.world_mut();
        world.get_mut::<Rotation>(car).unwrap().0 = inverted;
        world.get_mut::<Transform>(car).unwrap().rotation = inverted;
    }
    let mut applied = false;
    for _ in 0..120 {
        app.update();
        applied |= report(&app).applied > 0;
    }
    assert!(
        applied,
        "a 4 m roof drop must accumulate damage through the real pipeline"
    );
    let damage = app.world().get::<VehicleDamage>(car).unwrap();
    assert!(
        damage.total() > SPEC.impact_threshold,
        "the authored floor was crossed: {}",
        damage.total()
    );
}
