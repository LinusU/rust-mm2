//! F23-AC02 device-transition legs: focus loss and controller
//! disconnect/reconnect clear driving input instead of leaving throttle or
//! steering stuck. Unlike `tests/input.rs` (which writes `ButtonInput` and
//! `Gamepad` state directly), these feed bevy's real `InputPlugin` with
//! the raw events the OS backends emit — `KeyboardInput`,
//! `KeyboardFocusLost`, `RawGamepadEvent` and `GamepadConnectionEvent` —
//! so the release-on-focus-loss and remove-on-disconnect behaviour under
//! test is the engine's, reached through the production
//! `input::vehicle_input`. Synthetic events only: no real window or pad.

use bevy::input::ButtonState;
use bevy::input::InputPlugin;
use bevy::input::gamepad::{
    GamepadConnection, GamepadConnectionEvent, RawGamepadAxisChangedEvent,
    RawGamepadButtonChangedEvent, RawGamepadEvent,
};
use bevy::input::keyboard::{Key, KeyboardFocusLost, KeyboardInput};
use bevy::prelude::*;
use mm2_app::camera::CameraMode;
use mm2_app::input;
use mm2_game::{PlayerVehicle, Session, SessionConfig, SessionPhase};
use mm2_vehicle::VehicleInput;

fn drive_app() -> (App, Entity) {
    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Countdown).unwrap();
    session.transition(SessionPhase::Playing).unwrap();

    let mut app = App::new();
    app.add_plugins((MinimalPlugins, InputPlugin))
        .insert_resource(session)
        .insert_resource(CameraMode::Chase)
        .add_systems(Update, input::vehicle_input);
    app.world_mut()
        .spawn((PlayerVehicle, VehicleInput::default()));
    let window = app.world_mut().spawn(Window::default()).id();
    (app, window)
}

fn player_input(app: &mut App) -> VehicleInput {
    let mut q = app
        .world_mut()
        .query_filtered::<&VehicleInput, With<PlayerVehicle>>();
    *q.single(app.world()).unwrap()
}

fn key(app: &mut App, window: Entity, code: KeyCode, state: ButtonState) {
    app.world_mut().write_message(KeyboardInput {
        key_code: code,
        logical_key: Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified),
        state,
        text: None,
        repeat: false,
        window,
    });
}

fn set_focus(app: &mut App, window: Entity, focused: bool) {
    app.world_mut()
        .entity_mut(window)
        .get_mut::<Window>()
        .unwrap()
        .focused = focused;
    if !focused {
        // What bevy_winit sends alongside `Window::focused = false`.
        app.world_mut().write_message(KeyboardFocusLost);
    }
}

fn connect_pad(app: &mut App) -> Entity {
    let pad = app.world_mut().spawn_empty().id();
    app.world_mut().write_message(GamepadConnectionEvent::new(
        pad,
        GamepadConnection::Connected {
            name: "test pad".into(),
            vendor_id: None,
            product_id: None,
        },
    ));
    app.update();
    pad
}

fn disconnect_pad(app: &mut App, pad: Entity) {
    app.world_mut().write_message(GamepadConnectionEvent::new(
        pad,
        GamepadConnection::Disconnected,
    ));
}

fn pad_axis(app: &mut App, pad: Entity, axis: GamepadAxis, value: f32) {
    app.world_mut()
        .write_message(RawGamepadEvent::Axis(RawGamepadAxisChangedEvent::new(
            pad, axis, value,
        )));
}

fn pad_trigger(app: &mut App, pad: Entity, button: GamepadButton, value: f32) {
    app.world_mut()
        .write_message(RawGamepadEvent::Button(RawGamepadButtonChangedEvent::new(
            pad, button, value,
        )));
}

fn assert_neutral(vi: VehicleInput, why: &str) {
    assert_eq!(
        (vi.throttle, vi.brake, vi.steering, vi.handbrake),
        (0.0, 0.0, 0.0, 0.0),
        "{why}"
    );
}

/// Alt-tabbing away with W and D held: the window reports unfocused, bevy
/// releases every cached key, and the car stops being driven. Coming back
/// does not resurrect the held keys — the player has to press again.
#[test]
fn focus_loss_clears_held_keys_and_refocus_does_not_resume_them() {
    let (mut app, window) = drive_app();
    key(&mut app, window, KeyCode::KeyW, ButtonState::Pressed);
    key(&mut app, window, KeyCode::KeyD, ButtonState::Pressed);
    app.update();
    let vi = player_input(&mut app);
    assert_eq!((vi.throttle, vi.steering), (1.0, 1.0), "keys drive");

    set_focus(&mut app, window, false);
    app.update();
    assert_neutral(player_input(&mut app), "focus loss clears the held keys");

    set_focus(&mut app, window, true);
    app.update();
    assert_neutral(
        player_input(&mut app),
        "refocus alone does not re-press a key released during focus loss",
    );

    key(&mut app, window, KeyCode::KeyW, ButtonState::Pressed);
    app.update();
    assert_eq!(player_input(&mut app).throttle, 1.0, "a fresh press drives");
}

/// Pad state survives a focus change (the OS keeps reporting it), so the
/// gate — not the device — has to hold the input at zero while the window is
/// unfocused, and the pad answers again on refocus.
#[test]
fn an_unfocused_window_zeroes_a_held_pad_and_it_answers_on_refocus() {
    let (mut app, window) = drive_app();
    let pad = connect_pad(&mut app);
    pad_axis(&mut app, pad, GamepadAxis::LeftStickX, 1.0);
    pad_trigger(&mut app, pad, GamepadButton::RightTrigger2, 1.0);
    app.update();
    let vi = player_input(&mut app);
    assert_eq!((vi.throttle, vi.steering), (1.0, 1.0), "pad drives");

    set_focus(&mut app, window, false);
    app.update();
    assert_neutral(player_input(&mut app), "unfocused window mutes the pad");

    // The stick is still deflected on the device while the window is away.
    set_focus(&mut app, window, true);
    app.update();
    let vi = player_input(&mut app);
    assert_eq!(
        (vi.throttle, vi.steering),
        (1.0, 1.0),
        "the physically held pad answers again on refocus"
    );
}

/// Unplugging a pad with the stick and trigger held leaves nothing
/// behind: the next frame is neutral, a keyboard still drives, and
/// plugging a pad back in starts from a neutral device rather than the
/// last held values.
#[test]
fn pad_disconnect_clears_held_axes_and_reconnect_starts_neutral() {
    let (mut app, window) = drive_app();
    let pad = connect_pad(&mut app);
    pad_axis(&mut app, pad, GamepadAxis::LeftStickX, -1.0);
    pad_trigger(&mut app, pad, GamepadButton::RightTrigger2, 1.0);
    app.update();
    let vi = player_input(&mut app);
    assert_eq!((vi.throttle, vi.steering), (1.0, -1.0), "pad drives");

    disconnect_pad(&mut app, pad);
    app.update();
    assert_neutral(player_input(&mut app), "disconnect releases the held pad");

    key(&mut app, window, KeyCode::KeyW, ButtonState::Pressed);
    app.update();
    let vi = player_input(&mut app);
    assert_eq!(
        (vi.throttle, vi.steering),
        (1.0, 0.0),
        "the keyboard still drives with no pad"
    );
    key(&mut app, window, KeyCode::KeyW, ButtonState::Released);

    // The same entity reconnects: bevy re-adds a fresh `Gamepad`.
    app.world_mut().write_message(GamepadConnectionEvent::new(
        pad,
        GamepadConnection::Connected {
            name: "test pad".into(),
            vendor_id: None,
            product_id: None,
        },
    ));
    app.update();
    assert_neutral(
        player_input(&mut app),
        "a reconnected pad does not resurrect the held stick or trigger",
    );
}

/// With two pads, unplugging the one driving hands control to the
/// surviving pad — never to a ghost of the unplugged one.
#[test]
fn unplugging_the_first_pad_hands_driving_to_the_next() {
    let (mut app, _window) = drive_app();
    let first = connect_pad(&mut app);
    let second = connect_pad(&mut app);
    pad_axis(&mut app, first, GamepadAxis::LeftStickX, 1.0);
    pad_axis(&mut app, second, GamepadAxis::LeftStickX, -1.0);
    app.update();
    assert_eq!(
        player_input(&mut app).steering,
        1.0,
        "the first pad owns it"
    );

    disconnect_pad(&mut app, first);
    app.update();
    assert_eq!(
        player_input(&mut app).steering,
        -1.0,
        "the surviving pad takes over"
    );
}
