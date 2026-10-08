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

/// F23-AC01: a binding remapped in `controls.json` survives a "restart"
/// (a fresh app that loads the file) and drives the normalized input
/// through the production `vehicle_input`; the replaced key no longer
/// does.
#[test]
fn a_persisted_remap_drives_the_player_after_restart() {
    use mm2_app::controls::{ControlSettings, DriveAction, controls_path};

    let dir = tempfile::tempdir().unwrap();
    let path = controls_path(dir.path());
    let mut chosen = ControlSettings::default();
    chosen
        .rebind(DriveAction::Throttle, 0, KeyCode::KeyU)
        .unwrap();
    chosen.unbind(DriveAction::Throttle, 1).unwrap();
    chosen
        .rebind(DriveAction::Handbrake, 0, KeyCode::KeyJ)
        .unwrap();
    chosen.save(&path).unwrap();

    // The "next launch": nothing but the file carries the choice over.
    let mut app = drive_app(CameraMode::Chase);
    app.insert_resource(ControlSettings::load(&path));

    let hold = |app: &mut App, key: KeyCode| {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release_all();
        keys.press(key);
        app.update();
        player_input(app)
    };
    assert_eq!(hold(&mut app, KeyCode::KeyU).throttle, 1.0, "the new key");
    assert_eq!(hold(&mut app, KeyCode::KeyJ).handbrake, 1.0);
    for old in [KeyCode::KeyW, KeyCode::ArrowUp, KeyCode::Space] {
        let vi = hold(&mut app, old);
        assert_eq!(
            (vi.throttle, vi.handbrake),
            (0.0, 0.0),
            "{old:?} was rebound away and must not drive"
        );
    }
    // Untouched actions keep the shipped keys.
    assert_eq!(hold(&mut app, KeyCode::KeyS).brake, 1.0);
    assert_eq!(hold(&mut app, KeyCode::ArrowLeft).steering, -1.0);
}

/// F23-A.4: the handbrake button is a setting. A pad remap saved to
/// `controls.json` survives a restart: the new button holds the
/// handbrake, the shipped one no longer does, and a cleared action
/// leaves the pad without one (the key still works).
#[test]
fn a_persisted_pad_remap_moves_the_handbrake_button() {
    use mm2_app::controls::{ControlSettings, controls_path};
    use mm2_app::pad_map::PadAction;

    let dir = tempfile::tempdir().unwrap();
    let path = controls_path(dir.path());
    let mut chosen = ControlSettings::default();
    chosen.pad.unbind(PadAction::Reset).unwrap();
    chosen
        .pad
        .bind(PadAction::Handbrake, GamepadButton::North)
        .unwrap();
    chosen.save(&path).unwrap();

    let mut app = drive_app(CameraMode::Chase);
    app.world_mut().spawn(Gamepad::default());
    app.insert_resource(ControlSettings::load(&path));
    let held = |app: &mut App, button: GamepadButton| {
        let mut pads = app.world_mut().query::<&mut Gamepad>();
        for mut pad in pads.iter_mut(app.world_mut()) {
            pad.digital_mut().release_all();
            pad.digital_mut().press(button);
        }
        app.update();
        player_input(app).handbrake
    };
    assert_eq!(held(&mut app, GamepadButton::North), 1.0, "the new button");
    assert_eq!(held(&mut app, GamepadButton::South), 0.0, "the old one");

    // Cleared: no pad button holds the handbrake, Space still does.
    let mut cleared = ControlSettings::default();
    cleared.pad.unbind(PadAction::Handbrake).unwrap();
    app.insert_resource(cleared);
    assert_eq!(held(&mut app, GamepadButton::South), 0.0);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Space);
    app.update();
    assert_eq!(player_input(&mut app).handbrake, 1.0);
}

/// F23-A.4: the manual shifts follow their pad buttons — a remapped
/// shift answers on the new button and ignores the shoulder it left.
#[test]
fn rebound_pad_shift_buttons_shift_the_manual_gearbox() {
    use mm2_app::controls::ControlSettings;
    use mm2_app::pad_map::PadAction;

    let mut controls = ControlSettings::default().toggled_transmission();
    controls
        .pad
        .bind(PadAction::ShiftUp, GamepadButton::C)
        .unwrap();
    let mut app = drive_app(CameraMode::Chase);
    app.world_mut().spawn(Gamepad::default());
    spawn_geared_player(&mut app, 2);
    app.insert_resource(controls);
    app.update();
    assert_eq!(player_input(&mut app).forced_gear, Some(2));

    pad_tap(&mut app, GamepadButton::RightTrigger);
    assert_eq!(
        player_input(&mut app).forced_gear,
        Some(2),
        "the old shoulder"
    );
    pad_tap(&mut app, GamepadButton::C);
    assert_eq!(
        player_input(&mut app).forced_gear,
        Some(3),
        "the new button"
    );
}

/// A remapped key is still gated like any other: the free camera, a
/// non-`Playing` session and the countdown zero it, so a rebind cannot
/// open a path that drives while a menu or the fly camera owns the keys.
#[test]
fn a_remapped_key_is_gated_like_the_default() {
    use mm2_app::controls::{ControlSettings, DriveAction};

    let mut controls = ControlSettings::default();
    controls
        .rebind(DriveAction::Throttle, 0, KeyCode::KeyU)
        .unwrap();
    let mut app = drive_app(CameraMode::Free);
    app.insert_resource(controls);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyU);
    app.update();
    assert_eq!(player_input(&mut app).throttle, 0.0, "Free detaches it");

    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::Chase;
    app.update();
    assert_eq!(player_input(&mut app).throttle, 1.0);

    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    app.update();
    assert_eq!(player_input(&mut app).throttle, 0.0, "a pause clears it");
}

/// The pad's deadzones, steering gain and inversion are the settings'
/// own: a stick inside a widened deadzone steers nothing, sensitivity
/// scales then clamps, inversion flips the stick (not the keys), and a
/// trigger under its deadzone is released.
#[test]
fn pad_deadzone_sensitivity_and_inversion_apply() {
    use mm2_app::controls::ControlSettings;

    let mut controls = ControlSettings::default();
    controls.steer_deadzone = 0.3;
    controls.trigger_deadzone = 0.2;
    controls.steer_sensitivity = 1.5;
    controls.invert_steering = true;
    let mut app = drive_app(CameraMode::Chase);
    app.insert_resource(controls);
    app.world_mut().spawn(Gamepad::default());

    set_axis(&mut app, GamepadAxis::LeftStickX, 0.25);
    set_button_axis(&mut app, GamepadButton::RightTrigger2, 0.15);
    app.update();
    let vi = player_input(&mut app);
    assert_eq!(vi.steering, 0.0, "inside the widened stick deadzone");
    assert_eq!(vi.throttle, 0.0, "inside the trigger deadzone");

    set_axis(&mut app, GamepadAxis::LeftStickX, 0.4);
    set_button_axis(&mut app, GamepadButton::RightTrigger2, 0.5);
    app.update();
    let vi = player_input(&mut app);
    assert!(
        (vi.steering + 0.6).abs() < 1e-6,
        "0.4 * 1.5, inverted: {}",
        vi.steering
    );
    assert_eq!(vi.throttle, 0.5);

    set_axis(&mut app, GamepadAxis::LeftStickX, -0.9);
    app.update();
    assert_eq!(
        player_input(&mut app).steering,
        1.0,
        "gain clamps at full lock"
    );

    // Keys are never inverted: D still steers right with the flag on.
    set_axis(&mut app, GamepadAxis::LeftStickX, 0.0);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyD);
    app.update();
    assert_eq!(player_input(&mut app).steering, 1.0);
}

/// The mouse drives through the production system: the cursor's offset
/// from the window centre steers, the buttons throttle and brake, a
/// cursor outside the window steers nothing, and with the setting off
/// (the default) none of it reaches the car. Synthetic window and button
/// state only — no real mouse.
#[test]
fn the_mouse_drives_when_enabled() {
    use mm2_app::controls::ControlSettings;

    let mut app = drive_app(CameraMode::Chase);
    app.init_resource::<ButtonInput<MouseButton>>();
    let mut window = Window::default();
    window.resolution.set(800.0, 600.0);
    window.set_cursor_position(Some(Vec2::new(600.0, 300.0)));
    let window = app.world_mut().spawn(window).id();
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);

    app.update();
    let vi = player_input(&mut app);
    assert_eq!(
        (vi.throttle, vi.steering),
        (0.0, 0.0),
        "mouse driving is off by default"
    );

    let mut controls = ControlSettings::default();
    controls.mouse_driving = true;
    app.insert_resource(controls);
    app.update();
    let vi = player_input(&mut app);
    assert_eq!(vi.throttle, 1.0, "left button throttles");
    assert_eq!(vi.steering, 0.5, "600 of 800 is halfway right of centre");

    let mut buttons = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
    buttons.release(MouseButton::Left);
    buttons.press(MouseButton::Right);
    app.world_mut()
        .get_mut::<Window>(window)
        .unwrap()
        .set_cursor_position(Some(Vec2::new(0.0, 300.0)));
    app.update();
    let vi = player_input(&mut app);
    assert_eq!((vi.throttle, vi.brake), (0.0, 1.0), "right button brakes");
    assert_eq!(vi.steering, -1.0, "the left edge is full left lock");

    app.world_mut()
        .get_mut::<Window>(window)
        .unwrap()
        .set_cursor_position(None);
    app.update();
    assert_eq!(player_input(&mut app).steering, 0.0, "off the window");

    // The same gates as the keys: an unfocused window releases the mouse.
    app.world_mut()
        .get_mut::<Window>(window)
        .unwrap()
        .set_cursor_position(Some(Vec2::new(800.0, 300.0)));
    app.world_mut().get_mut::<Window>(window).unwrap().focused = false;
    app.update();
    let vi = player_input(&mut app);
    assert_eq!(
        (vi.throttle, vi.brake, vi.steering),
        (0.0, 0.0, 0.0),
        "focus loss clears the mouse's hold"
    );
}

/// A player car with a real config and gearbox state, in `gear`.
fn spawn_geared_player(app: &mut App, gear: usize) {
    use mm2_vehicle::{Vehicle, VehicleConfig, VehicleState};

    let config = VehicleConfig::default();
    let mut state = VehicleState::new(&config);
    state.gear = gear;
    let mut q = app
        .world_mut()
        .query_filtered::<Entity, With<PlayerVehicle>>();
    let car = q.single(app.world()).unwrap();
    app.world_mut()
        .entity_mut(car)
        .insert((Vehicle { config }, state));
}

fn tap(app: &mut App, key: KeyCode) {
    let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
    keys.release_all();
    keys.clear();
    keys.press(key);
    app.update();
    let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
    keys.release_all();
    keys.clear();
}

/// F23 req 1's transmission policy: automatic leaves the gearbox to the
/// sim (`forced_gear` stays `None` however the shift keys are hit);
/// manual pins the gear the car is in, each shift-key *press* moves one
/// gear (a held key does not run through them), the ends clamp, and a
/// pause/respawn re-seeds from the car instead of replaying a stale gear.
#[test]
fn manual_transmission_pins_and_steps_the_gear() {
    use mm2_app::controls::{ControlSettings, TransmissionPolicy};

    let mut app = drive_app(CameraMode::Chase);
    spawn_geared_player(&mut app, 2);
    app.insert_resource(ControlSettings::default());

    tap(&mut app, KeyCode::KeyG);
    assert_eq!(
        player_input(&mut app).forced_gear,
        None,
        "automatic ignores the shift keys"
    );

    let manual = ControlSettings::default().toggled_transmission();
    assert_eq!(manual.transmission, TransmissionPolicy::Manual);
    app.insert_resource(manual);
    app.update();
    assert_eq!(
        player_input(&mut app).forced_gear,
        Some(2),
        "seeded from the car"
    );

    tap(&mut app, KeyCode::KeyG);
    assert_eq!(player_input(&mut app).forced_gear, Some(3));
    // Held across frames: one press is one gear.
    {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.press(KeyCode::KeyG);
    }
    app.update();
    // `InputPlugin` clears the edges each frame; this minimal app does it by hand.
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .clear();
    app.update();
    assert_eq!(player_input(&mut app).forced_gear, Some(4));
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release_all();
    for _ in 0..8 {
        tap(&mut app, KeyCode::KeyG);
    }
    assert_eq!(
        player_input(&mut app).forced_gear,
        Some(5),
        "top of six gears"
    );
    for _ in 0..9 {
        tap(&mut app, KeyCode::KeyB);
    }
    assert_eq!(player_input(&mut app).forced_gear, Some(0), "first gear");

    // A pause drops the hold: the next playing frame reads the car again.
    tap(&mut app, KeyCode::KeyG);
    assert_eq!(player_input(&mut app).forced_gear, Some(1));
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    app.update();
    assert_eq!(
        player_input(&mut app).forced_gear,
        None,
        "paused: no command"
    );
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Playing)
        .unwrap();
    app.update();
    assert_eq!(
        player_input(&mut app).forced_gear,
        Some(2),
        "re-seeded from the car's own gear"
    );
}

/// A predicted (`Remote`) session never pins the gearbox: the wire carries
/// no gear, so a locally pinned copy would drift from the host's
/// automatic one.
#[test]
fn manual_transmission_is_inert_on_a_predicted_session() {
    use mm2_app::controls::ControlSettings;
    use mm2_game::SessionAuthority;

    let mut session = Session::new();
    session
        .begin(SessionConfig {
            authority: SessionAuthority::Remote,
            ..SessionConfig::default()
        })
        .unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Countdown).unwrap();
    session.transition(SessionPhase::Playing).unwrap();

    let mut app = drive_app(CameraMode::Chase);
    app.insert_resource(session);
    spawn_geared_player(&mut app, 1);
    app.insert_resource(ControlSettings::default().toggled_transmission());
    tap(&mut app, KeyCode::KeyG);
    assert_eq!(player_input(&mut app).forced_gear, None);
}

/// One pad-button edge: press for one update, then end it like the
/// event loop's `clear` would.
fn pad_tap(app: &mut App, button: GamepadButton) {
    press(app, button);
    app.update();
    let mut pads = app.world_mut().query::<&mut Gamepad>();
    for mut pad in pads.iter_mut(app.world_mut()) {
        pad.digital_mut().release(button);
        pad.digital_mut().clear();
    }
}

/// F23-A.3: a gamepad-only driver on the manual box shifts with the
/// shoulders — right up, left down, one gear per press, clamped — and an
/// automatic box ignores them.
#[test]
fn the_pad_shoulders_shift_a_manual_gearbox() {
    use mm2_app::controls::ControlSettings;

    let mut app = drive_app(CameraMode::Chase);
    app.world_mut().spawn(Gamepad::default());
    spawn_geared_player(&mut app, 2);
    app.insert_resource(ControlSettings::default());

    pad_tap(&mut app, input::pad::SHIFT_UP);
    assert_eq!(player_input(&mut app).forced_gear, None, "automatic");

    app.insert_resource(ControlSettings::default().toggled_transmission());
    app.update();
    assert_eq!(player_input(&mut app).forced_gear, Some(2));
    pad_tap(&mut app, input::pad::SHIFT_UP);
    assert_eq!(player_input(&mut app).forced_gear, Some(3));
    pad_tap(&mut app, input::pad::SHIFT_UP);
    assert_eq!(player_input(&mut app).forced_gear, Some(4));
    // Held across frames: one press is one gear.
    press(&mut app, input::pad::SHIFT_UP);
    app.update();
    let mut pads = app.world_mut().query::<&mut Gamepad>();
    for mut pad in pads.iter_mut(app.world_mut()) {
        pad.digital_mut().clear();
    }
    app.update();
    assert_eq!(player_input(&mut app).forced_gear, Some(5));
    let mut pads = app.world_mut().query::<&mut Gamepad>();
    for mut pad in pads.iter_mut(app.world_mut()) {
        pad.digital_mut().release_all();
        pad.digital_mut().clear();
    }
    for _ in 0..3 {
        pad_tap(&mut app, input::pad::SHIFT_UP);
    }
    assert_eq!(player_input(&mut app).forced_gear, Some(5), "top gear");
    for _ in 0..7 {
        pad_tap(&mut app, input::pad::SHIFT_DOWN);
    }
    assert_eq!(player_input(&mut app).forced_gear, Some(0), "first gear");
}

/// An unfocused window mutes the shoulders like every other pad control:
/// the gear a held pad shifted while the window was away does not count.
#[test]
fn an_unfocused_window_ignores_the_pad_shift() {
    use mm2_app::controls::ControlSettings;

    let mut app = drive_app(CameraMode::Chase);
    app.world_mut().spawn(Gamepad::default());
    let window = app
        .world_mut()
        .spawn(Window {
            focused: false,
            ..default()
        })
        .id();
    spawn_geared_player(&mut app, 2);
    app.insert_resource(ControlSettings::default().toggled_transmission());
    pad_tap(&mut app, input::pad::SHIFT_UP);
    assert_eq!(player_input(&mut app).forced_gear, None, "no command");
    app.world_mut().get_mut::<Window>(window).unwrap().focused = true;
    app.update();
    assert_eq!(
        player_input(&mut app).forced_gear,
        Some(2),
        "the missed press never counted"
    );
}

/// F25-C's `--ram` pursuit law: a car facing −Z at the origin.
mod ram {
    use super::*;

    fn input_at(speed: f32, target: Option<Vec3>) -> VehicleInput {
        input::ram_input(&GlobalTransform::default(), speed, target, 5.0)
    }

    #[test]
    fn it_charges_a_target_it_can_turn_onto() {
        // Dead ahead and far: full throttle, no steering.
        let ahead = input_at(5.0, Some(Vec3::new(0.0, 0.0, -40.0)));
        assert_eq!((ahead.throttle, ahead.steering), (1.0, 0.0));
        // Ahead and to the right, past the stand-off: steers right.
        let right = input_at(5.0, Some(Vec3::new(10.0, 0.0, -40.0)));
        assert!(right.steering > 0.0, "{right:?}");
        let left = input_at(5.0, Some(Vec3::new(-10.0, 0.0, -40.0)));
        assert!(left.steering < 0.0, "{left:?}");
    }

    #[test]
    fn it_opens_the_range_before_turning_onto_a_neighbour_beside_it() {
        // A grid neighbour 4 m to the right is inside the stand-off and
        // far off the nose: a full-lock orbit would miss it, so the
        // car drives straight first.
        let beside = input_at(5.0, Some(Vec3::new(4.0, 0.0, 0.0)));
        assert_eq!((beside.throttle, beside.steering), (1.0, 0.0));
        // Close but nearly ahead is a charge line — it keeps steering.
        let near = input_at(5.0, Some(Vec3::new(1.5, 0.0, -6.0)));
        assert!(near.steering > 0.0, "{near:?}");
    }

    #[test]
    fn it_lifts_off_above_its_pace_and_drives_straight_alone() {
        let fast = input_at(30.0, Some(Vec3::new(0.0, 0.0, -40.0)));
        assert_eq!(fast.throttle, 0.0);
        let alone = input_at(0.0, None);
        assert_eq!((alone.throttle, alone.steering), (1.0, 0.0));
    }

    #[test]
    fn it_slows_to_the_turning_pace_while_the_target_is_off_the_nose() {
        // Beside the car and far: still turning, so above the turning
        // pace it lifts off; the same speed on a charge line keeps going.
        let turning = input_at(8.0, Some(Vec3::new(30.0, 0.0, -10.0)));
        assert_eq!(turning.throttle, 0.0, "{turning:?}");
        let charging = input_at(8.0, Some(Vec3::new(0.0, 0.0, -40.0)));
        assert_eq!(charging.throttle, 1.0, "{charging:?}");
        // A pursuer given the full pace as its turning pace does not slow.
        let wide = input::ram_input(
            &GlobalTransform::default(),
            8.0,
            Some(Vec3::new(30.0, 0.0, -10.0)),
            11.0,
        );
        assert_eq!(wide.throttle, 1.0, "{wide:?}");
    }

    #[test]
    fn it_parks_after_the_first_impact_it_makes_at_speed() {
        let mut strike = input::RamStrike::default();
        // The spawn landing: an impact at rest does not count.
        assert!(!strike.observe(0.0, 1));
        assert!(!strike.observe(2.0, 1));
        // Launched, then a new impact: struck, and it stays struck.
        assert!(!strike.observe(6.0, 1));
        assert!(strike.observe(5.0, 2));
        assert!(strike.observe(0.0, 2));
    }

    #[test]
    fn it_is_neutral_outside_a_live_session() {
        // The production system on a session still `Ready`: the car
        // gets no throttle, whatever stands in front of it.
        let mut session = Session::new();
        session.begin(SessionConfig::default()).unwrap();
        session.transition(SessionPhase::Ready).unwrap();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(session)
            .add_systems(Update, input::ram_drive);
        app.world_mut().spawn((
            PlayerVehicle,
            GlobalTransform::default(),
            avian3d::prelude::LinearVelocity::default(),
            VehicleInput {
                throttle: 1.0,
                ..default()
            },
        ));
        app.update();
        let got = player_input(&mut app);
        assert_eq!((got.throttle, got.steering), (0.0, 0.0));
    }
}
