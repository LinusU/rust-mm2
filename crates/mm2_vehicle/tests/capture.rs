//! NETPLAY-03A: typed capture/restore of complete vehicle state.
//!
//! Every test runs the real production vehicle schedules (`VehiclePlugin`
//! inside Avian's `PhysicsSchedule`) on a ground plane, one 60 Hz fixed
//! step per `app.update()` (`FIXED_HZ` is 60), for BOTH the generic
//! arcade model and the original retail model.
//!
//! Determinism limits (see `docs/netcode-redesign.md`): the vehicle and
//! body state are restored exactly; Avian's internal contact manifolds,
//! solver warm-start impulses and broad-phase caches are not part of the
//! DTO (NETPLAY-03B). The tolerances below are therefore measured, not
//! assumed: `TOLERANCE_*` bound the replay-versus-uninterrupted
//! deviation this harness observes.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_vehicle::capture::{EpochRef, VehicleCapture, capture_vehicle, restore_vehicle};
use mm2_vehicle::config::{
    OriginalAero, OriginalAxle, OriginalEngine, OriginalGearbox, OriginalHandling, OriginalTrain,
    OriginalWheel,
};
use mm2_vehicle::original::{EngineConstants, FIRST_GEAR, gear_ratios, shift_points};
use mm2_vehicle::vehicle::{VehicleInput, VehicleState};
use mm2_vehicle::{
    EngineImpairment, ResetAuthority, ResetVehicle, VehicleConfig, VehiclePlugin, vehicle_bundle,
};

/// Replay-versus-uninterrupted bound on any float of the capture, in the
/// field's own unit (m, m/s, rad, N...). Measured; see the doc note.
const TOLERANCE: f32 = 1e-3;
const EPOCH: EpochRef = EpochRef {
    generation: 3,
    reset_epoch: 7,
};

#[derive(Clone, Copy, Debug)]
enum Model {
    Generic,
    Original,
}

/// A hand-authored retail-shaped car (Beetle-like tokens): synthetic, not
/// retail data, so the test runs in CI without `MM2_RETAIL`.
fn config(model: Model) -> VehicleConfig {
    let mut cfg = VehicleConfig::default();
    if matches!(model, Model::Generic) {
        return cfg;
    }
    let gravity = 19.6;
    let engine = OriginalEngine {
        max_power_w: 260.0 * 746.0,
        idle_rpm: 750.0,
        opt_rpm: 5800.0,
        max_rpm: 8500.0,
        ang_inertia: 1.0,
        gear_change_lag: 0.25,
    };
    let constants = EngineConstants::of(&engine);
    let ratios = gear_ratios(engine.opt_rpm, 0.34, 30.0, 20.0, 90.0, 6, 0.5);
    let shifts = shift_points(&ratios, &constants, engine.max_rpm, 0.1, 0.1, 0.3);
    let wheel = |rear: bool, side: f32| OriginalWheel {
        static_load: cfg.mass * gravity / 4.0,
        rear,
        side,
        width: 0.25,
        suspension_extent: 0.15,
        suspension_limit: 0.05,
        suspension_factor: 1.0,
        suspension_damp_coef: 0.1,
        steering_limit: if rear { 0.0 } else { 0.4 },
        steering_offset: 0.25,
        brake_coef: 0.625,
        handbrake_coef: if rear { 1.0 } else { 0.0 },
        tire_disp_limit_lat: 0.125,
        tire_disp_limit_long: 0.125,
        tire_damp_coef_lat: 0.25,
        tire_damp_coef_long: 0.25,
        tire_drag_coef_lat: 0.05,
        tire_drag_coef_long: 0.02,
        optimum_slip: 0.16,
        static_fric: 3.0,
        sliding_fric: 2.7,
    };
    let train = OriginalTrain {
        ang_inertia: 5000.0,
        brake_dynamic_coef: 1.0,
        brake_static_coef: 1.2,
    };
    cfg.original = Some(OriginalHandling {
        bound_friction: 0.3,
        bound_elasticity: 0.2,
        gravity,
        surface_friction: 0.9,
        wheels: vec![
            wheel(false, -1.0),
            wheel(false, 1.0),
            wheel(true, -1.0),
            wheel(true, 1.0),
        ],
        driven: vec![2, 3],
        free: vec![0, 1],
        axles: vec![
            OriginalAxle {
                wheels: [0, 1],
                torque_coef: 20.0,
                damp_coef: 0.3,
            },
            OriginalAxle {
                wheels: [2, 3],
                torque_coef: 20.0,
                damp_coef: 0.3,
            },
        ],
        drivetrain: train,
        freetrain: train,
        engine,
        gearbox: OriginalGearbox {
            ratios,
            upshift_rpm: shifts.upshift,
            downshift_full_rpm: shifts.downshift_full,
            downshift_zero_rpm: shifts.downshift_zero,
            equal_power_rpm: shifts.equal_power,
            gear_change_time: 0.8,
        },
        aero: OriginalAero {
            ang_c_damp: [1.3, 1.4, 1.0],
            ang_vel_damp: [0.0; 3],
            ang_vel2_damp: [0.14, 2.0, 1.0],
            drag: 0.5,
            down: 0.0,
        },
        max_angular_speed: 4.0 * std::f32::consts::PI,
    });
    cfg.validate().expect("fixture config is valid");
    cfg
}

fn test_app(cfg: &VehicleConfig, position: Vec3, yaw: f32) -> (App, Entity) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        // Production `FIXED_HZ`: one physics step per update.
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin);
    app.finish();
    app.cleanup();
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(400.0, 1.0, 400.0),
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    let rotation = Quat::from_rotation_y(yaw);
    let car = app
        .world_mut()
        .spawn((
            vehicle_bundle(cfg),
            Position(position),
            Rotation(rotation),
            Transform::from_translation(position).with_rotation(rotation),
        ))
        .id();
    (app, car)
}

fn fresh(model: Model) -> (App, Entity, VehicleConfig) {
    let cfg = config(model);
    let (app, car) = test_app(&cfg, Vec3::new(0.0, 1.2, 0.0), 0.0);
    (app, car, cfg)
}

fn input(throttle: f32, brake: f32, steering: f32, handbrake: f32) -> VehicleInput {
    VehicleInput {
        throttle,
        brake,
        steering,
        handbrake,
        forced_gear: None,
    }
}

/// `(frames, command)` segments; frame `i` of a segment applies its
/// command to one step.
type Route = Vec<(usize, VehicleInput)>;

/// Launch and settle: builds tyre, suspension and drivetrain history.
fn lead_in() -> Route {
    vec![
        (60, input(0.0, 0.0, 0.0, 0.0)),
        (120, input(1.0, 0.0, 0.0, 0.0)),
    ]
}

/// Steer, brake, change gear, coast.
fn route(model: Model) -> Route {
    let gear = match model {
        // Generic gears are 0-indexed forward gears; original 2.. (R, N, 1st).
        Model::Generic => 1,
        Model::Original => FIRST_GEAR,
    };
    vec![
        (40, input(1.0, 0.0, 0.6, 0.0)),
        (30, input(0.0, 1.0, -0.5, 0.0)),
        (
            30,
            VehicleInput {
                forced_gear: Some(gear),
                ..input(1.0, 0.0, 0.2, 0.0)
            },
        ),
        (30, input(0.3, 0.0, -0.8, 0.5)),
        (30, input(0.0, 0.0, 0.0, 0.0)),
    ]
}

fn run(app: &mut App, car: Entity, route: &Route) {
    for (frames, command) in route {
        *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = *command;
        for _ in 0..*frames {
            app.update();
        }
    }
}

fn cap(app: &App, car: Entity) -> VehicleCapture {
    capture_vehicle(app.world(), car, EPOCH).expect("a vehicle")
}

fn describe(a: &VehicleCapture, b: &VehicleCapture) -> String {
    match a.deviation(b) {
        Ok(d) => format!("max |Δ| {:e} in {}", d.max_abs, d.field),
        Err(field) => format!("discrete mismatch in {field}"),
    }
}

fn assert_within(a: &VehicleCapture, b: &VehicleCapture, tolerance: f32, what: &str) {
    match a.deviation(b) {
        Ok(d) if d.max_abs <= tolerance => {
            eprintln!("{what}: max |Δ| {:e} ({})", d.max_abs, d.field)
        }
        Ok(d) => panic!(
            "{what}: {} deviates {:e} > {tolerance:e}",
            d.field, d.max_abs
        ),
        Err(field) => panic!("{what}: discrete state differs in {field}"),
    }
}

fn steps_elapsed(app: &App) -> u64 {
    (app.world().resource::<Time<Physics>>().elapsed_secs_f64() * 60.0).round() as u64
}

fn both() -> [Model; 2] {
    [Model::Generic, Model::Original]
}

#[test]
fn one_update_is_one_production_step() {
    let (mut app, _car, _) = fresh(Model::Original);
    // The first update has no elapsed time, so it runs no step.
    app.update();
    let start = steps_elapsed(&app);
    for _ in 0..30 {
        app.update();
    }
    assert_eq!(steps_elapsed(&app) - start, 30);
}

#[test]
fn the_capture_actually_holds_history() {
    // Guards the harness: the state being restored must be non-trivial.
    for model in both() {
        let (mut app, car, _) = fresh(model);
        run(&mut app, car, &lead_in());
        let c = cap(&app, car);
        assert!(c.body.linear_velocity.iter().any(|v| v.abs() > 1.0));
        assert!(
            c.wheels
                .iter()
                .any(|w| w.spin != 0.0 || w.suspension_force > 0.0)
        );
        match &c.original {
            Some(o) => {
                assert!(o.powertrain.engine_omega > 0.0);
                assert!(o.wheels.iter().any(|w| w.omega.abs() > 1.0));
            }
            None => assert!(c.rpm > 0.0),
        }
        assert_eq!(c.original.is_some(), matches!(model, Model::Original));
    }
}

#[test]
fn capture_survives_a_serde_round_trip() {
    for model in both() {
        let (mut app, car, _) = fresh(model);
        run(&mut app, car, &lead_in());
        let c = cap(&app, car);
        let text = toml::to_string(&c).unwrap();
        let back: VehicleCapture = toml::from_str(&text).unwrap();
        assert_eq!(c.deviation(&back).map(|d| d.max_abs), Ok(0.0), "{model:?}");
    }
}

#[test]
fn restore_then_capture_is_identity() {
    for model in both() {
        let (mut app, car, _) = fresh(model);
        run(&mut app, car, &lead_in());
        let mut c = cap(&app, car);
        c.impairment = Some(0.5);
        // Run on, then restore: the world must read back exactly `c`.
        run(&mut app, car, &route(model));
        restore_vehicle(app.world_mut(), car, &c).unwrap();
        assert_eq!(cap(&app, car), c, "{model:?}");
    }
}

#[test]
fn replay_from_a_capture_matches_the_uninterrupted_run() {
    for model in both() {
        // Reference: one world, never interrupted.
        let (mut reference, rcar, _) = fresh(model);
        run(&mut reference, rcar, &lead_in());
        let at_t = cap(&reference, rcar);
        let mut checkpoints = Vec::new();
        for segment in route(model) {
            run(&mut reference, rcar, &vec![segment]);
            checkpoints.push(cap(&reference, rcar));
        }

        // Subject: same lead-in, capture at T, run the route, rewind to
        // T, replay the same commands.
        let (mut app, car, _) = fresh(model);
        run(&mut app, car, &lead_in());
        let captured = cap(&app, car);
        assert_eq!(captured, at_t, "{model:?}: capture at T is deterministic");
        run(&mut app, car, &route(model));
        restore_vehicle(app.world_mut(), car, &captured).unwrap();
        assert_eq!(cap(&app, car), captured, "{model:?}: restore at T");
        for (segment, expect) in route(model).into_iter().zip(&checkpoints) {
            run(&mut app, car, &vec![segment]);
            assert_within(
                &cap(&app, car),
                expect,
                TOLERANCE,
                &format!("{model:?} segment end"),
            );
        }
        // Multiple ticks, including mid-route: a second rewind to T.
        restore_vehicle(app.world_mut(), car, &captured).unwrap();
        run(&mut app, car, &route(model));
        assert_within(
            &cap(&app, car),
            checkpoints.last().unwrap(),
            TOLERANCE,
            &format!("{model:?} second replay"),
        );
    }
}

#[test]
fn restore_rejects_a_mismatched_or_poisoned_capture() {
    let (mut generic, gcar, _) = fresh(Model::Generic);
    let (mut original, ocar, _) = fresh(Model::Original);
    generic.update();
    original.update();
    let g = cap(&generic, gcar);
    let o = cap(&original, ocar);
    let before = cap(&original, ocar);
    assert!(restore_vehicle(original.world_mut(), ocar, &g).is_err());
    assert!(restore_vehicle(generic.world_mut(), gcar, &o).is_err());
    let mut nan = o.clone();
    nan.body.position[1] = f32::NAN;
    assert!(restore_vehicle(original.world_mut(), ocar, &nan).is_err());
    let mut short = o.clone();
    short.wheels.pop();
    assert!(restore_vehicle(original.world_mut(), ocar, &short).is_err());
    assert_eq!(
        cap(&original, ocar),
        before,
        "a refused restore writes nothing"
    );
}

#[test]
fn carried_mass_and_impairment_travel_with_the_capture() {
    for model in both() {
        let (mut app, car, _) = fresh(model);
        run(&mut app, car, &lead_in());
        app.world_mut()
            .entity_mut(car)
            .insert(EngineImpairment(0.4));
        app.world_mut().get_mut::<Mass>(car).unwrap().0 += 250.0;
        let c = cap(&app, car);
        assert_eq!(c.impairment, Some(0.4));
        app.world_mut().entity_mut(car).remove::<EngineImpairment>();
        app.world_mut().get_mut::<Mass>(car).unwrap().0 -= 250.0;
        restore_vehicle(app.world_mut(), car, &c).unwrap();
        assert_eq!(
            app.world().get::<EngineImpairment>(car).map(|e| e.0),
            Some(0.4)
        );
        assert_eq!(cap(&app, car), c);
    }
}

/// The host's real reset path: `ResetVehicle` through `vehicle_reset`.
fn host_reset(app: &mut App, car: Entity, position: Vec3, yaw: f32) {
    app.world_mut().insert_resource(ResetAuthority(true));
    app.world_mut().write_message(ResetVehicle {
        entity: Some(car),
        position,
        yaw,
    });
    // `Update` applies it; no physics step has seen the new pose yet.
    app.update();
}

/// What a pose-only snap does: the pose and velocities land, every bit
/// of tyre, suspension, force and powertrain history stays.
fn pose_only_snap(app: &mut App, car: Entity, position: Vec3, yaw: f32) {
    let rotation = Quat::from_rotation_y(yaw);
    let mut e = app.world_mut().entity_mut(car);
    e.get_mut::<Position>().unwrap().0 = position;
    e.get_mut::<Rotation>().unwrap().0 = rotation;
    let mut t = e.get_mut::<Transform>().unwrap();
    t.translation = position;
    t.rotation = rotation;
    e.get_mut::<LinearVelocity>().unwrap().0 = Vec3::ZERO;
    e.get_mut::<AngularVelocity>().unwrap().0 = Vec3::ZERO;
}

#[test]
fn a_host_reset_and_a_pose_only_snap_are_not_the_same_state() {
    let target = Vec3::new(30.0, 1.2, -20.0);
    let yaw = 0.7;
    let after = vec![
        (30, input(0.6, 0.0, 0.3, 0.0)),
        (30, input(1.0, 0.0, -0.2, 0.0)),
    ];
    for model in both() {
        let cfg = config(model);
        let build = || {
            let (mut app, car) = test_app(&cfg, Vec3::new(0.0, 1.2, 0.0), 0.0);
            run(&mut app, car, &lead_in());
            run(&mut app, car, &route(model));
            (app, car)
        };
        let (mut reset_app, rcar) = build();
        let (mut snap_app, scar) = build();
        // `host_reset` costs one frame (its step runs before the
        // teleport lands); give the snap world the same.
        host_reset(&mut reset_app, rcar, target, yaw);
        snap_app.update();
        pose_only_snap(&mut snap_app, scar, target, yaw);

        // Fresh-spawn reference for what a reset must equal.
        let (mut fresh_app, fcar) = test_app(&cfg, target, yaw);
        fresh_app.update();

        let reset = cap(&reset_app, rcar);
        let snap = cap(&snap_app, scar);
        let state_diff = |a: &VehicleCapture, b: &VehicleCapture| match a.deviation(b) {
            Ok(d) => d.max_abs,
            Err(_) => f32::INFINITY,
        };
        eprintln!(
            "{model:?}: reset vs snap right after the teleport: {}",
            describe(&reset, &snap)
        );
        // The detector: the carried history is visible in the capture.
        assert!(
            state_diff(&reset, &snap) > 0.1,
            "{model:?}: a snap must be distinguishable from a reset"
        );
        let carried = snap.original.as_ref().map_or_else(
            || snap.wheels.iter().any(|w| w.spin != 0.0) || snap.rpm > 2000.0,
            |o| o.wheels.iter().any(|w| w.omega.abs() > 1.0) || o.powertrain.engine_omega > 100.0,
        );
        assert!(
            carried,
            "{model:?}: the snap kept prior wheel/engine history"
        );
        // `vehicle_reset` leaves `PreStepVelocity` (rewritten by the
        // next step before anything reads it) and nothing else behind:
        // one more step on each side makes a reset equal a fresh spawn.
        reset_app.update();
        snap_app.update();
        fresh_app.update();
        let (reset, spawned) = (cap(&reset_app, rcar), cap(&fresh_app, fcar));
        assert_within(
            &reset,
            &spawned,
            TOLERANCE,
            &format!("{model:?} reset≈fresh spawn"),
        );

        // The consequence: identical commands, diverging cars.
        run(&mut reset_app, rcar, &after);
        run(&mut snap_app, scar, &after);
        run(&mut fresh_app, fcar, &after);
        let (reset, snap, spawned) = (
            cap(&reset_app, rcar),
            cap(&snap_app, scar),
            cap(&fresh_app, fcar),
        );
        eprintln!(
            "{model:?}: after 60 ticks reset vs snap: {}",
            describe(&reset, &snap)
        );
        assert!(
            state_diff(&reset, &snap) > 0.1,
            "{model:?}: the stale history keeps the cars apart"
        );
        // A reset is still repeatable: it equals a car spawned there.
        // (One extra frame ran in the fresh world's lead, absorbed
        // by the same count above.)
        assert_within(
            &reset,
            &spawned,
            0.05,
            &format!("{model:?} reset≈fresh after route"),
        );
    }
}

#[test]
fn state_resets_equal_vehicle_state_new() {
    for model in both() {
        let (mut app, car, cfg) = fresh(model);
        run(&mut app, car, &lead_in());
        host_reset(&mut app, car, Vec3::new(5.0, 1.2, 5.0), 0.0);
        let state = app.world().get::<VehicleState>(car).unwrap();
        let new = VehicleState::new(&cfg);
        assert_eq!(state.steer_angle, new.steer_angle);
        assert_eq!(state.gear, new.gear);
        assert_eq!(state.original.is_some(), new.original.is_some());
        if let (Some(a), Some(b)) = (&state.original, &new.original) {
            assert_eq!(a.powertrain, b.powertrain);
            assert_eq!(a.drive_omega, b.drive_omega);
        }
    }
}
