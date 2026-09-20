//! F01-C session-lifecycle integration: start, quit, failure and restart
//! through the production spawn/teardown systems — the same
//! `load_session_world`/`drive_session`/`despawn_session_entities` wiring
//! the `mm2` binary schedules, driven headlessly under real Avian physics.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::camera::CameraMode;
use mm2_app::contracts::{self, ImpactFilter};
use mm2_app::session::{
    self, ErrorText, Hud, SelectedCar, SessionControl, SpawnPoint, TunedVehicle,
};
use mm2_assets::Vfs;
use mm2_game::{
    DamageSignals, ImpactEvent, ImpactId, Mm2Vfs, ObjectId, ObjectIdentity, PlayerVehicle, Session,
    SessionConfig, SessionEntity, SessionPhase, WorldMode, advance_session_tick,
    despawn_session_entities,
};
use mm2_vehicle::{Vehicle, VehicleConfig, VehicleInput, VehiclePlugin, VehicleState};

/// A headless app wired exactly like the binary's session path: real
/// world spawning, teardown and contract pipeline, minus the window and
/// device input (tests drive `SessionControl`/`VehicleInput` directly).
fn test_app(config: SessionConfig, frame_secs: f64) -> App {
    let mut session = Session::new();
    session.begin(config).unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            frame_secs,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .insert_resource(session)
        .insert_resource(Mm2Vfs(Vfs::new()))
        .insert_resource(TunedVehicle(VehicleConfig::default()))
        .insert_resource(SelectedCar {
            def: None,
            paint: 0,
        })
        .insert_resource(SpawnPoint {
            position: Vec3::new(0.0, 1.5, 0.0),
            yaw: 0.0,
            trailers: Vec::new(),
        })
        .insert_resource(CameraMode::Chase)
        // `load_session_world` fills the real asset stores; without render
        // plugins the stores are inert collections — enough to spawn.
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ImpactFilter>()
        .init_resource::<SessionControl>()
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (
                contracts::collect_impacts,
                contracts::publish_vehicle_telemetry,
            )
                .chain(),
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

fn dev_app() -> App {
    test_app(SessionConfig::default(), 1.0 / 60.0)
}

fn count<F: QueryFilter>(app: &mut App) -> usize {
    let world = app.world_mut();
    world.query_filtered::<Entity, F>().iter(world).count()
}

fn single<F: QueryFilter>(app: &mut App) -> Entity {
    let world = app.world_mut();
    world
        .query_filtered::<Entity, F>()
        .single(world)
        .expect("expected exactly one match")
}

fn drain_impacts(app: &mut App) -> Vec<ImpactEvent> {
    app.world_mut()
        .resource_mut::<Messages<ImpactEvent>>()
        .drain()
        .collect()
}

/// Run updates until `pred` holds or `max` updates pass; returns whether
/// the predicate was reached.
fn run_until(app: &mut App, max: usize, mut pred: impl FnMut(&mut App) -> bool) -> bool {
    for _ in 0..max {
        app.update();
        if pred(app) {
            return true;
        }
    }
    false
}

fn phase_is(app: &mut App, phase: SessionPhase) -> bool {
    *app.world().resource::<Session>().phase() == phase
}

/// AC01: a restart through the real spawn/teardown path leaves exactly
/// one of every session-owned thing — no duplicated cameras, players,
/// physics bodies or UI — and clears session-scoped caches.
#[test]
fn restart_leaves_exactly_one_session() {
    let mut app = dev_app();
    app.update();
    assert!(phase_is(&mut app, SessionPhase::Playing));

    let car = single::<With<PlayerVehicle>>(&mut app);
    let bodies = count::<With<RigidBody>>(&mut app);
    assert_eq!(count::<With<Camera3d>>(&mut app), 2, "chase + free");
    assert_eq!(count::<With<Hud>>(&mut app), 1);
    assert_eq!(count::<With<ErrorText>>(&mut app), 1);
    let active = {
        let world = app.world_mut();
        world
            .query::<&Camera>()
            .iter(world)
            .filter(|c| c.is_active)
            .count()
    };
    assert_eq!(active, 1);

    // Bookkeeping entries must not leak into the next session: plant a
    // trailer record the teardown has to clear.
    let dummy = app.world_mut().spawn_empty().id();
    app.world_mut()
        .resource_mut::<SpawnPoint>()
        .trailers
        .push((dummy, Vec3::ZERO));

    // Drive the restart through the real key path: Backspace latches the
    // intent, then the input is cleared (no InputPlugin runs here, so
    // `just_pressed` would otherwise persist into the next session).
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Backspace);
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .clear();

    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Playing)),
        "restart never returned to Playing"
    );
    // The transition frame itself runs no Playing fixed steps — let the
    // new session's clock advance a little.
    for _ in 0..5 {
        app.update();
    }
    let session = app.world().resource::<Session>();
    assert_eq!(session.generation(), 2);
    // The clock restarted at `begin` and is counting the new session's
    // steps — not continuing the old session's.
    assert!(
        session.tick() > 0 && session.tick() <= 20,
        "tick={}",
        session.tick()
    );

    // One session's worth of entities, all owned by generation 2.
    assert_eq!(count::<With<PlayerVehicle>>(&mut app), 1);
    assert_eq!(count::<With<Camera3d>>(&mut app), 2);
    assert_eq!(count::<With<Hud>>(&mut app), 1);
    assert_eq!(count::<With<ErrorText>>(&mut app), 1);
    assert_eq!(count::<With<RigidBody>>(&mut app), bodies);
    {
        let world = app.world_mut();
        let stale = world
            .query::<&SessionEntity>()
            .iter(world)
            .filter(|e| e.0 != 2)
            .count();
        assert_eq!(stale, 0, "entities from generation 1 leaked");
    }
    assert!(
        app.world().get_entity(car).is_err(),
        "the old player entity was not torn down"
    );
    assert!(
        app.world().resource::<SpawnPoint>().trailers.is_empty(),
        "trailer bookkeeping leaked into the new session"
    );
}

/// AC01: quit also routes through teardown — the world despawns, the
/// session reaches `Menu`, and the app is asked to exit.
#[test]
fn quit_unloads_to_menu_and_writes_app_exit() {
    let mut app = dev_app();
    app.update();
    assert!(phase_is(&mut app, SessionPhase::Playing));

    app.world_mut().resource_mut::<SessionControl>().quit = true;
    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Menu)),
        "quit never reached Menu"
    );
    assert_eq!(count::<With<SessionEntity>>(&mut app), 0);
    assert_eq!(count::<With<PlayerVehicle>>(&mut app), 0);
    // The intent is consumed on a later update, once the phase is Menu —
    // watch for the exit message before it ages out of the buffer.
    let wrote_exit = run_until(&mut app, 6, |a| {
        !a.world().resource::<Messages<AppExit>>().is_empty()
    });
    assert!(wrote_exit, "quit should request AppExit");
    let exits: Vec<AppExit> = app
        .world_mut()
        .resource_mut::<Messages<AppExit>>()
        .drain()
        .collect();
    assert!(
        exits.iter().any(|e| matches!(e, AppExit::Success)),
        "quit should request AppExit::Success, got {exits:?}"
    );
}

/// AC02: a failed load leaves no active player simulation — and the
/// failure is still a session that can be retried or quit through the
/// same lifecycle.
#[test]
fn failed_load_leaves_no_player_simulation() {
    let config = SessionConfig {
        world: WorldMode::City {
            psdl: "city/bogus.psdl".into(),
        },
        ..SessionConfig::default()
    };
    let mut app = test_app(config, 1.0 / 60.0);
    app.update();
    assert!(
        matches!(
            app.world().resource::<Session>().phase(),
            SessionPhase::Failed(_)
        ),
        "a missing city must fail the load, got {:?}",
        app.world().resource::<Session>().phase()
    );
    // No player simulation exists in the incomplete world — the vehicle
    // is only spawned after the world reports Ready.
    assert_eq!(count::<With<PlayerVehicle>>(&mut app), 0);
    assert_eq!(count::<With<Vehicle>>(&mut app), 0);
    assert_eq!(count::<With<RigidBody>>(&mut app), 0);

    // Retry fails the same way (same config), still without a player.
    app.world_mut().resource_mut::<SessionControl>().restart = true;
    assert!(
        run_until(&mut app, 12, |a| matches!(
            a.world().resource::<Session>().phase(),
            SessionPhase::Failed(_)
        )),
        "retry never re-failed"
    );
    assert_eq!(app.world().resource::<Session>().generation(), 2);
    assert_eq!(count::<With<PlayerVehicle>>(&mut app), 0);

    // Quit still tears down cleanly: every session-owned entity goes.
    app.world_mut().resource_mut::<SessionControl>().quit = true;
    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Menu)),
        "failed session never unloaded"
    );
    assert_eq!(count::<With<SessionEntity>>(&mut app), 0);
}

/// Spawn a marked dynamic box that falls onto the dev-world ground and
/// produce its `ImpactEvent`. The player car's raycast wheels catch its
/// spawn drop without a chassis contact, so a box gives a deterministic
/// reportable impact (same trick as the contracts tests).
fn drop_box_and_collect(app: &mut App, pos: Vec3) -> (ObjectId, Vec<ImpactEvent>) {
    let object = app.world_mut().resource_mut::<Session>().mint_object_id();
    app.world_mut().spawn((
        ObjectIdentity(object),
        DamageSignals::default(),
        RigidBody::Dynamic,
        Collider::cuboid(1.0, 1.0, 1.0),
        CollisionEventsEnabled,
        Position(pos),
        Transform::from_translation(pos),
    ));
    let mut found = Vec::new();
    run_until(app, 90, |a| {
        found.extend(
            drain_impacts(a)
                .into_iter()
                .filter(|e| e.participants.0 == object || e.participants.1 == object),
        );
        !found.is_empty()
    });
    (object, found)
}

/// Teardown clears the impact pipeline's session-scoped state: the new
/// session's impact stream restarts its ids, and its evidence counters
/// describe only the new session.
#[test]
fn restart_resets_the_impact_stream() {
    let mut app = dev_app();
    app.update();
    let (_, gen1) = drop_box_and_collect(&mut app, Vec3::new(3.0, 2.0, 0.0));
    assert!(
        !gen1.is_empty(),
        "a dropped box should produce a generation-1 impact"
    );
    assert!(gen1.iter().all(|e| e.generation == 1));

    app.world_mut().resource_mut::<SessionControl>().restart = true;
    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Playing)),
        "restart never returned to Playing"
    );
    // Anything buffered from the old session is generation-1; drain it
    // before collecting the new stream.
    drain_impacts(&mut app);
    let (_, gen2) = drop_box_and_collect(&mut app, Vec3::new(6.0, 2.0, 0.0));
    assert!(
        !gen2.is_empty(),
        "the respawned session should produce its own impact"
    );
    assert!(gen2.iter().all(|e| e.generation == 2));
    assert_eq!(
        gen2[0].id,
        ImpactId(1),
        "impact ids restart per session — teardown must reset the stream"
    );
    assert_eq!(
        app.world().resource::<ImpactFilter>().emitted as usize,
        gen2.len(),
        "emitted counts only the current session's stream"
    );
}

/// AC03: fixed-tick input produces equivalent gameplay regardless of how
/// updates are batched. Two apps pinned to identical throttle — one at
/// 60 updates/s (2 fixed steps each), one at 30/s (4 each) — must reach
/// the same state after the same number of fixed steps.
#[test]
fn fixed_tick_driving_is_update_rate_independent() {
    fn driven(update_secs: f64) -> (Vec3, f32, u64) {
        let mut app = test_app(SessionConfig::default(), update_secs);
        app.update();
        let car = single::<With<PlayerVehicle>>(&mut app);
        if let Some(mut vi) = app.world_mut().get_mut::<VehicleInput>(car) {
            vi.throttle = 1.0;
        }
        run_until(&mut app, 300, |a| {
            a.world().resource::<Session>().tick() >= 240
        });
        let s = app.world().resource::<Session>();
        assert_eq!(s.tick(), 240, "expected exactly 240 fixed steps");
        (
            app.world().get::<Position>(car).unwrap().0,
            app.world().get::<VehicleState>(car).unwrap().forward_speed,
            s.tick(),
        )
    }

    let (p60, v60, t60) = driven(1.0 / 60.0);
    let (p30, v30, t30) = driven(1.0 / 30.0);
    assert_eq!(t60, t30);
    assert!(
        (p60 - p30).length() < 1e-3,
        "same fixed steps, different batching diverged: {p60} vs {p30}"
    );
    assert!((v60 - v30).abs() < 1e-3, "speeds diverged: {v60} vs {v30}");
    // Sanity: the car actually drove, not two identically parked states.
    assert!(v60 > 5.0, "the drive probe should be moving ({v60} m/s)");
}
