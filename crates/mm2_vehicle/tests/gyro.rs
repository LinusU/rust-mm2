//! Headless integration tests for the authored `vehGyro` consumption
//! (F05-B.4): the Spinable latch (handbrake 180 / reverse 180), the
//! Driftable slide relief, and the Rightable airborne channels.
//!
//! The same fixed-clock setup as `drive.rs`: every `app.update()` is one
//! 60 Hz frame = two 120 Hz physics steps.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_vehicle::config::GyroConfig;
use mm2_vehicle::vehicle::{VehicleInput, VehicleState};
use mm2_vehicle::{VehicleConfig, VehiclePlugin, vehicle_bundle};

const FRAMES_PER_SECOND: usize = 60;

/// A gyro record shaped like the retail roster's (vpcop-like): real
/// spin rates, mild drift, the authored 0.0 righting axes.
fn cop_gyro() -> Option<GyroConfig> {
    Some(GyroConfig {
        spin180: 1.5,
        reverse180: 4.0,
        drift: 0.14,
        pitch: Some(0.0),
        roll: Some(0.0),
    })
}

fn test_app_with(cfg: VehicleConfig, pos: Vec3, rot: Quat) -> (App, Entity) {
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

    let car = app
        .world_mut()
        .spawn((
            vehicle_bundle(&cfg),
            Position(pos),
            Transform::from_translation(pos) * Transform::from_rotation(rot),
        ))
        .id();
    (app, car)
}

fn grounded_app(cfg: VehicleConfig) -> (App, Entity) {
    test_app_with(cfg, Vec3::new(0.0, 1.2, 0.0), Quat::IDENTITY)
}

fn set_input(app: &mut App, car: Entity, input: VehicleInput) {
    *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = input;
}

fn drive(app: &mut App, car: Entity, frames: usize, input: VehicleInput) {
    set_input(app, car, input);
    for _ in 0..frames {
        app.update();
    }
}

/// Heading angle about world Y (radians), unwrapped sign.
fn heading(app: &App, car: Entity) -> f32 {
    let rot = app.world().get::<Rotation>(car).unwrap().0;
    let fwd = rot * Vec3::NEG_Z;
    fwd.x.atan2(-fwd.z)
}

/// Up-axis alignment with world up, 1 = level.
fn up_y(app: &App, car: Entity) -> f32 {
    let rot = app.world().get::<Rotation>(car).unwrap().0;
    (rot * Vec3::Y).y
}

fn state(app: &App, car: Entity) -> &VehicleState {
    app.world().get::<VehicleState>(car).unwrap()
}

/// Get the car rolling: settle, then accelerate to travelling speed.
fn up_to_speed(app: &mut App, car: Entity) {
    drive(
        app,
        car,
        FRAMES_PER_SECOND * 2,
        VehicleInput {
            throttle: 1.0,
            ..default()
        },
    );
    assert!(
        state(app, car).forward_speed > 8.0,
        "car should be travelling, fwd={}",
        state(app, car).forward_speed
    );
}

const SPIN_INPUT: VehicleInput = VehicleInput {
    throttle: 0.0,
    brake: 0.0,
    steering: 1.0,
    handbrake: 1.0,
    forced_gear: None,
};

#[test]
fn a_car_with_no_gyro_record_latches_nothing() {
    let cfg = VehicleConfig::default();
    assert!(cfg.gyro.is_none(), "no record means no assist");
    let (mut app, car) = grounded_app(cfg);
    up_to_speed(&mut app, car);
    let h0 = heading(&app, car);
    drive(&mut app, car, FRAMES_PER_SECOND * 3, SPIN_INPUT);
    let st = state(&app, car);
    assert_eq!(st.gyro_spins, 0, "no record, no servo");
    assert!(st.gyro_spin.is_none());
    assert_eq!(st.gyro_completed, 0);
    // The ordinary handbrake slide still yaws — this test pins the
    // *assist* boundary, not the tire physics.
    let _ = h0;
}

#[test]
fn a_held_maneuver_spins_the_car_around() {
    let cfg = VehicleConfig {
        gyro: cop_gyro(),
        ..Default::default()
    };
    let (mut app, car) = grounded_app(cfg);
    up_to_speed(&mut app, car);
    let h0 = heading(&app, car);
    drive(&mut app, car, FRAMES_PER_SECOND * 7, SPIN_INPUT);

    let st = state(&app, car);
    assert!(st.gyro_spins >= 1, "the maneuver should latch");
    let turned = (heading(&app, car) - h0).abs();
    assert!(
        turned > 1.2,
        "a held spin should rotate the car substantially; turned {turned:.2} rad"
    );
    assert!(
        st.gyro_completed >= 1,
        "a sustained hold should run the maneuver to ~180°: rotated {}, spins {}",
        st.gyro_spin.map(|s| s.rotated).unwrap_or(-1.0),
        st.gyro_spins
    );
}

#[test]
fn a_tap_doses_the_spin() {
    let cfg = VehicleConfig {
        gyro: cop_gyro(),
        ..Default::default()
    };
    let (mut app, car) = grounded_app(cfg);
    up_to_speed(&mut app, car);
    let h0 = heading(&app, car);
    // Half a second of handbrake — a tap, not a held spin.
    drive(&mut app, car, FRAMES_PER_SECOND / 2, SPIN_INPUT);
    // Let go; the latch must release with the inputs.
    drive(
        &mut app,
        car,
        FRAMES_PER_SECOND * 2,
        VehicleInput::default(),
    );

    let st = state(&app, car);
    assert_eq!(st.gyro_spins, 1, "exactly one maneuver latched");
    assert_eq!(st.gyro_completed, 0, "a tap cannot complete a 180");
    assert!(st.gyro_spin.is_none(), "released inputs drop the latch");
    let turned = (heading(&app, car) - h0).abs();
    assert!(
        turned < 2.6,
        "a tapped spin should be partial, not a full 180; turned {turned:.2} rad"
    );
}

#[test]
fn the_reverse180_rate_drives_the_j_turn() {
    let cfg = VehicleConfig {
        gyro: cop_gyro(),
        ..Default::default()
    };
    let (mut app, car) = grounded_app(cfg);
    // Hold the brake through the standstill — it becomes reverse
    // throttle by the drivetrain's deliberate policy.
    drive(
        &mut app,
        car,
        FRAMES_PER_SECOND * 3,
        VehicleInput {
            brake: 1.0,
            ..default()
        },
    );
    assert!(
        state(&app, car).forward_speed < -3.0,
        "car should be reversing, fwd={}",
        state(&app, car).forward_speed
    );

    // Steer left + handbrake while reversing: the latch should arm at
    // the authored Reverse180 rate, not the forward Spin180 one.
    drive(
        &mut app,
        car,
        12,
        VehicleInput {
            steering: -1.0,
            handbrake: 1.0,
            ..default()
        },
    );
    let st = state(&app, car);
    assert_eq!(st.gyro_spins, 1, "the J-turn should latch");
    let spin = st.gyro_spin.expect("maneuver latched");
    assert!(
        (spin.rate - 4.0).abs() < 0.01,
        "reverse spin should target the authored Reverse180 rate, got {}",
        spin.rate
    );
    // Steering left while spinning is a *positive* commanded yaw rate
    // (steer -1 → +rate): the nose swings left.
    assert!(spin.rate > 0.0);
}

#[test]
fn steering_the_other_way_mid_spin_re_arms() {
    let cfg = VehicleConfig {
        gyro: cop_gyro(),
        ..Default::default()
    };
    let (mut app, car) = grounded_app(cfg);
    up_to_speed(&mut app, car);
    drive(&mut app, car, FRAMES_PER_SECOND / 2, SPIN_INPUT);
    assert_eq!(state(&app, car).gyro_spins, 1);
    // Flick the wheel the other way while still travelling — the old
    // latch drops and a fresh one arms next step.
    drive(
        &mut app,
        car,
        30,
        VehicleInput {
            steering: -1.0,
            handbrake: 1.0,
            ..default()
        },
    );
    let st = state(&app, car);
    assert!(
        st.gyro_spins >= 2,
        "an opposite flick should re-arm the maneuver, spins={}",
        st.gyro_spins
    );
}

#[test]
fn authored_drift_relief_holds_the_slide() {
    // Identical cars, identical moderate handbrake slide below the spin
    // trigger: the one the record says drifts keeps more of its yaw
    // freedom — the slip term barely tightens its damper, so it rotates
    // less while sideways (a held drift, not a spin-out).
    let drifting = VehicleConfig {
        gyro: Some(GyroConfig {
            drift: 0.9,
            ..cop_gyro().unwrap()
        }),
        ..Default::default()
    };
    let gripped = VehicleConfig {
        gyro: Some(GyroConfig {
            drift: 0.0,
            ..cop_gyro().unwrap()
        }),
        ..Default::default()
    };

    let slide = VehicleInput {
        steering: 0.3, // below the 0.5 spin trigger — a slide, not a spin
        handbrake: 1.0,
        ..default()
    };
    let mut headings = [0.0f32; 2];
    for (i, cfg) in [drifting, gripped].into_iter().enumerate() {
        let (mut app, car) = grounded_app(cfg);
        up_to_speed(&mut app, car);
        let h0 = heading(&app, car);
        drive(&mut app, car, FRAMES_PER_SECOND, slide);
        headings[i] = (heading(&app, car) - h0).abs();
        assert_eq!(
            state(&app, car).gyro_spins,
            0,
            "steering below the trigger must not latch a spin"
        );
    }
    let [drift_turn, grip_turn] = headings;
    assert!(
        drift_turn < grip_turn,
        "the drift-record car should hold a tighter slide: {drift_turn:.3} vs {grip_turn:.3} rad"
    );
}

#[test]
fn authored_pitch_and_roll_right_the_car_in_the_air() {
    // No designed air control, authored righting axes instead: the
    // vehgyro channel alone should level a tilted airborne car.
    let mut cfg = VehicleConfig {
        gyro: Some(GyroConfig {
            pitch: Some(2.5),
            roll: Some(2.5),
            ..cop_gyro().unwrap()
        }),
        ..Default::default()
    };
    cfg.assists.air_control = 0.0;
    // Spawned 8 m up, pitched and rolled ~40° — clearly airborne.
    let tilt = Quat::from_euler(EulerRot::XZY, 0.7, 0.0, 0.6);
    let (mut app, car) = test_app_with(cfg, Vec3::new(0.0, 8.0, 0.0), tilt);
    let mut min_up = 1.0f32;
    let mut final_up = 0.0f32;
    for _ in 0..FRAMES_PER_SECOND {
        app.update();
        let u = up_y(&app, car);
        min_up = min_up.min(u);
        final_up = u;
    }
    assert!(
        final_up > 0.9,
        "authored righting should level the car before landing; final up.y={final_up:.3} (min {min_up:.3})"
    );

    // Control: the same drop with no gyro record and no air control
    // must *not* level — the difference is the authored channel, not
    // gravity or the tires.
    let mut bare = VehicleConfig::default();
    bare.assists.air_control = 0.0;
    let (mut app2, car2) = test_app_with(bare, Vec3::new(0.0, 8.0, 0.0), tilt);
    for _ in 0..FRAMES_PER_SECOND {
        app2.update();
    }
    let unaided = up_y(&app2, car2);
    assert!(
        unaided < final_up - 0.05,
        "without the record nothing levels the car: {unaided:.3} vs {final_up:.3}"
    );
}
