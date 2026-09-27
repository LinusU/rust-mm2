//! F22-AC06 pad-driving legs: the designed in-session pad map feeds
//! `VehicleInput` through the production `input::vehicle_input` — the
//! left stick steers analog, the triggers throttle/brake, South is the
//! handbrake — and the same gates the keyboard answers to hold on the
//! pad: `Free` camera and a non-`Playing` phase zero every channel.

use bevy::prelude::*;
use mm2_app::camera::CameraMode;
use mm2_app::input;
use mm2_game::{PlayerVehicle, Session, SessionConfig, SessionPhase};
use mm2_vehicle::VehicleInput;

/// A minimal app wired like the binary's driving-input edge: a real
/// `Session` already `Playing`, the `CameraMode` resource `main`
/// inserts, one player vehicle, and the production
/// `input::vehicle_input` system. No `Window` entity — `vehicle_input`
/// treats "no windows" as focused, like the headless harness.
fn drive_app(cam: CameraMode) -> App {
    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Countdown).unwrap();
    session.transition(SessionPhase::Playing).unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<ButtonInput<KeyCode>>()
        .insert_resource(session)
        .insert_resource(cam)
        .add_systems(Update, input::vehicle_input);
    app.world_mut()
        .spawn((PlayerVehicle, VehicleInput::default()));
    app
}

fn player_input(app: &mut App) -> VehicleInput {
    let mut q = app
        .world_mut()
        .query_filtered::<&VehicleInput, With<PlayerVehicle>>();
    *q.single(app.world()).unwrap()
}

/// Analog axes stay set until rewritten — `analog_mut().set` is bevy's
/// documented gamepad mocking surface.
fn set_axis(app: &mut App, axis: GamepadAxis, value: f32) {
    let mut pads = app.world_mut().query::<&mut Gamepad>();
    for mut pad in pads.iter_mut(app.world_mut()) {
        pad.analog_mut().set(axis, value);
    }
}

fn set_button_axis(app: &mut App, button: GamepadButton, value: f32) {
    let mut pads = app.world_mut().query::<&mut Gamepad>();
    for mut pad in pads.iter_mut(app.world_mut()) {
        pad.analog_mut().set(button, value);
    }
}

fn press(app: &mut App, button: GamepadButton) {
    let mut pads = app.world_mut().query::<&mut Gamepad>();
    for mut pad in pads.iter_mut(app.world_mut()) {
        pad.digital_mut().press(button);
    }
}

/// The pad drives the same `VehicleInput` the keys own: stick steers
/// analog, `RightTrigger2`/`LeftTrigger2` carry analog throttle/brake,
/// South is the handbrake — and a non-neutral stick wins over a held
/// key, exactly the precedence `vehicle_input` documents.
#[test]
fn pad_axes_drive_the_player() {
    let mut app = drive_app(CameraMode::Chase);
    app.world_mut().spawn(Gamepad::default());

    set_axis(&mut app, GamepadAxis::LeftStickX, -0.6);
    set_button_axis(&mut app, GamepadButton::RightTrigger2, 0.7);
    app.update();
    let vi = player_input(&mut app);
    assert_eq!(vi.steering, -0.6, "stick steers analog");
    assert_eq!(vi.throttle, 0.7, "RT2 throttles analog");
    assert_eq!(vi.brake, 0.0);
    assert_eq!(vi.handbrake, 0.0);

    set_axis(&mut app, GamepadAxis::LeftStickX, 0.0);
    set_button_axis(&mut app, GamepadButton::RightTrigger2, 0.0);
    set_button_axis(&mut app, GamepadButton::LeftTrigger2, 0.4);
    press(&mut app, GamepadButton::South);
    app.update();
    let vi = player_input(&mut app);
    assert_eq!(vi.brake, 0.4, "LT2 brakes analog");
    assert_eq!(vi.handbrake, 1.0, "South is the handbrake");
    assert_eq!(vi.throttle, 0.0);

    // A held key plus a non-neutral stick: the pad axis wins like the
    // doc comment promises.
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyA);
    set_axis(&mut app, GamepadAxis::LeftStickX, 0.8);
    app.update();
    let vi = player_input(&mut app);
    assert_eq!(vi.steering, 0.8, "non-neutral stick outranks the key");
}

/// AC06's other half: the free camera flies without driving — a held
/// pad must write a zeroed `VehicleInput` under `CameraMode::Free`,
/// and resuming a drive view reads it again.
#[test]
fn free_camera_detaches_the_pad() {
    let mut app = drive_app(CameraMode::Free);
    app.world_mut().spawn(Gamepad::default());
    set_axis(&mut app, GamepadAxis::LeftStickX, 1.0);
    set_button_axis(&mut app, GamepadButton::RightTrigger2, 1.0);

    app.update();
    let vi = player_input(&mut app);
    assert_eq!(
        (vi.throttle, vi.brake, vi.steering, vi.handbrake),
        (0.0, 0.0, 0.0, 0.0),
        "the pad must not drive while the camera flies"
    );

    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::Chase;
    app.update();
    let vi = player_input(&mut app);
    assert_eq!(vi.steering, 1.0);
    assert_eq!(vi.throttle, 1.0);
}

/// A non-`Playing` phase clears every channel the same way the key
/// path does — stale analog controls cannot carry into a pause.
#[test]
fn non_playing_phase_zeroes_the_pad() {
    let mut app = drive_app(CameraMode::Chase);
    app.world_mut().spawn(Gamepad::default());
    set_button_axis(&mut app, GamepadButton::RightTrigger2, 1.0);
    app.update();
    assert_eq!(player_input(&mut app).throttle, 1.0);

    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    app.update();
    let vi = player_input(&mut app);
    assert_eq!(
        (vi.throttle, vi.brake, vi.steering, vi.handbrake),
        (0.0, 0.0, 0.0, 0.0),
        "paused clears the held trigger"
    );
}
