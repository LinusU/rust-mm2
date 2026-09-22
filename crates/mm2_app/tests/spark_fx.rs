//! F05-B.8 integration — the `asLineSparks` reading runs through the
//! production `emit_sparks`/`advance_sparks` systems: reportable
//! impacts burst bounded streaks at the authored contact point along
//! the participant's rebound normal, remote participants and paused
//! sessions emit nothing, and streaks expire on their designed life.

use std::time::Duration;

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::spark_fx::{self, SparkAssets, SparkFx, SparkFxReport};
use mm2_game::{
    ImpactEvent, ImpactId, ObjectId, ObjectIdentity, Player, PlayerControl, Session, SessionConfig,
    SessionEntity, SessionPhase, Spark, SparkPolicy, SurfaceState, VehicleSparks,
};

const POINT: Vec3 = Vec3::new(3.0, 0.8, -5.0);
const NORMAL: Vec3 = Vec3::new(0.0, 0.0, 1.0);

/// A playing session with the spark systems and synthetic streak
/// assets (a plain quad + a blended material — UVs/blend mode are
/// render detail the headless tests don't read). `fx` toggles the
/// `SparkFx` resource so the absent-assets case is covered.
fn spark_app(fx: bool) -> (App, ObjectId, ObjectId) {
    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    let car_a = session.mint_object_id();
    let car_b = session.mint_object_id();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(session)
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<SparkFxReport>()
        .add_message::<ImpactEvent>()
        .add_systems(
            Update,
            (spark_fx::emit_sparks, spark_fx::advance_sparks).chain(),
        );
    app.finish();
    app.cleanup();

    if fx {
        let material = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                alpha_mode: AlphaMode::Add,
                unlit: true,
                ..default()
            });
        let mesh = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(Rectangle::new(1.0, 1.0));
        app.insert_resource(SparkFx {
            assets: SparkAssets { mesh, material },
        });
    }
    (app, car_a, car_b)
}

/// A spark-rigged participant — the `vehCarDamage`-present gate the
/// production spawn paths use (Local or Remote `PlayerControl`).
fn rigged_car(app: &mut App, object: ObjectId, control: PlayerControl) -> Entity {
    let generation = app.world().resource::<Session>().generation();
    app.world_mut()
        .spawn((
            SessionEntity(generation),
            ObjectIdentity(object),
            Player {
                id: mm2_game::PlayerId(1),
                control,
            },
            VehicleSparks::new(SparkPolicy::default(), 42),
        ))
        .id()
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
            point: POINT,
            normal: NORMAL,
            severity,
            surface: SurfaceState::default(),
        });
}

fn sparks(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query_filtered::<Entity, With<Spark>>()
        .iter(app.world())
        .collect()
}

fn report(app: &App) -> &SparkFxReport {
    app.world().resource::<SparkFxReport>()
}

fn run(app: &mut App, frames: usize) {
    for _ in 0..frames {
        app.update();
    }
}

#[test]
fn a_reportable_impact_sparks_at_the_contact_point() {
    let (mut app, car, world) = spark_app(true);
    let entity = rigged_car(&mut app, car, PlayerControl::Local);
    write_impact(&mut app, 1, car, world, 10.0);
    app.update();

    let r = report(&app);
    assert_eq!(r.bursts, 1);
    let policy = SparkPolicy::default();
    assert_eq!(r.emitted as usize, policy.burst_count(10.0));

    let generation = app.world().resource::<Session>().generation();
    for e in sparks(&mut app) {
        let s = app.world().get::<Spark>(e).unwrap();
        assert_eq!(s.position, POINT, "sparks are born at the contact point");
        // The car is participant 0: its burst rebounds along −normal.
        assert!(s.velocity.dot(NORMAL) < 0.0, "{s:?}");
        assert_eq!(s.emitter, entity);
        assert_eq!(
            app.world().get::<SessionEntity>(e),
            Some(&SessionEntity(generation)),
            "every streak is session-owned so teardown takes it"
        );
    }
}

#[test]
fn both_rigged_participants_spark_on_their_own_sides() {
    let (mut app, a, b) = spark_app(true);
    let ea = rigged_car(&mut app, a, PlayerControl::Local);
    let eb = rigged_car(&mut app, b, PlayerControl::Local);
    write_impact(&mut app, 1, a, b, 10.0);
    app.update();

    assert_eq!(report(&app).bursts, 2, "each rig fires its own burst");
    let mut saw_a = false;
    let mut saw_b = false;
    for e in sparks(&mut app) {
        let s = app.world().get::<Spark>(e).unwrap();
        if s.emitter == ea {
            saw_a = true;
            assert!(s.velocity.dot(NORMAL) < 0.0, "{s:?}");
        } else if s.emitter == eb {
            saw_b = true;
            assert!(s.velocity.dot(NORMAL) > 0.0, "{s:?}");
        }
    }
    assert!(saw_a && saw_b);
}

#[test]
fn a_remote_participant_never_sparks_locally() {
    let (mut app, car, world) = spark_app(true);
    rigged_car(&mut app, car, PlayerControl::Remote);
    write_impact(&mut app, 1, car, world, 10.0);
    app.update();
    // Remote impacts belong to the remote authority — its own client
    // renders them; nothing local is emitted.
    assert_eq!(report(&app).emitted, 0);
    assert!(sparks(&mut app).is_empty());
}

#[test]
fn unrigged_participants_spark_nothing() {
    let (mut app, car, world) = spark_app(true);
    // No `VehicleSparks` — a vehicle without an authored damage
    // record owns no spark renderer, never a fabricated one.
    let generation = app.world().resource::<Session>().generation();
    app.world_mut()
        .spawn((SessionEntity(generation), ObjectIdentity(car)));
    write_impact(&mut app, 1, car, world, 10.0);
    app.update();
    assert_eq!(report(&app).emitted, 0);
    assert!(sparks(&mut app).is_empty());
}

#[test]
fn stale_events_drain_without_emitting() {
    let (mut app, car, world) = spark_app(true);
    rigged_car(&mut app, car, PlayerControl::Local);

    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    write_impact(&mut app, 1, car, world, 10.0);
    app.update();
    assert_eq!(report(&app).emitted, 0, "a pause drains the feed");
    assert!(sparks(&mut app).is_empty());

    // The drained event never flushes on resume — only new ones emit.
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Playing)
        .unwrap();
    app.update();
    assert_eq!(report(&app).emitted, 0);
    write_impact(&mut app, 2, car, world, 10.0);
    app.update();
    assert!(report(&app).emitted > 0);
}

#[test]
fn the_pool_stays_bounded_under_a_burst_heavy_feed() {
    let (mut app, car, world) = spark_app(true);
    rigged_car(&mut app, car, PlayerControl::Local);
    // A burst-heavy stretch: the live pool never exceeds the designed
    // bound (F05-AC03) even while the feed keeps firing.
    let policy = SparkPolicy::default();
    for id in 1..=40u64 {
        write_impact(&mut app, id, car, world, 60.0);
        app.update();
        assert!(
            sparks(&mut app).len() <= policy.max_live,
            "live pool overran the bound: {}",
            sparks(&mut app).len()
        );
    }
    let r = report(&app);
    // The feed kept producing bursts until the pool saturated, then
    // clipped them — evidence the bound truncates, not the stream.
    assert!(
        r.bursts >= (policy.max_live / policy.max_burst) as u64,
        "{r:?}"
    );
    assert!(r.emitted as usize <= 40 * policy.max_burst, "{r:?}");
    assert!(r.emitted as usize >= policy.max_live, "{r:?}");
}

#[test]
fn sparks_expire_on_their_designed_life() {
    let (mut app, car, world) = spark_app(true);
    rigged_car(&mut app, car, PlayerControl::Local);
    write_impact(&mut app, 1, car, world, 10.0);
    app.update();
    let emitted = report(&app).emitted;
    assert!(emitted > 0);

    // life ≈ 0.5±0.2 s at 60 Hz → every streak expires inside ~1 s.
    run(&mut app, 90);
    assert_eq!(report(&app).expired, emitted);
    assert!(sparks(&mut app).is_empty());
}

#[test]
fn missing_spark_assets_emit_nothing() {
    let (mut app, car, world) = spark_app(false);
    rigged_car(&mut app, car, PlayerControl::Local);
    write_impact(&mut app, 1, car, world, 10.0);
    app.update();
    assert_eq!(report(&app).emitted, 0);
}

#[test]
fn streaks_track_velocity_and_fade() {
    let (mut app, car, world) = spark_app(true);
    rigged_car(&mut app, car, PlayerControl::Local);
    write_impact(&mut app, 1, car, world, 10.0);
    run(&mut app, 6);
    for e in sparks(&mut app) {
        let s = app.world().get::<Spark>(e).unwrap();
        let xf = app.world().get::<Transform>(e).unwrap();
        assert_eq!(xf.translation, s.position);
        // +Y of the streak frame tracks the velocity direction.
        let dir = s.velocity.try_normalize().unwrap_or(Vec3::Y);
        assert!(
            xf.rotation
                .abs_diff_eq(Quat::from_rotation_arc(Vec3::Y, dir), 1e-4)
        );
        // Gravity is pulling the streak: alpha < 1 as it ages.
        assert!(s.alpha() < 1.0);
        let material = app
            .world()
            .get::<MeshMaterial3d<StandardMaterial>>(e)
            .unwrap();
        let alpha = app
            .world()
            .resource::<Assets<StandardMaterial>>()
            .get(&material.0)
            .unwrap()
            .base_color
            .alpha();
        assert!((alpha - s.alpha()).abs() < 1e-4, "{s:?}");
    }
}
