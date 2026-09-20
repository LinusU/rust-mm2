//! Keyboard/gamepad → [`VehicleInput`] mapping.
//!
//! Controls are routed by mode: free-camera navigation must not drive the
//! car, and driving input is cleared while the session is not `Playing`
//! or the window has lost focus (so alt-tabbing away doesn't keep the
//! throttle pinned).

use bevy::prelude::*;
use mm2_game::{PlayerVehicle, RaceState, Session};
use mm2_vehicle::VehicleInput;

use crate::camera::CameraMode;

/// Fill `VehicleInput` on the player vehicle from keyboard and the first
/// connected gamepad (gamepad axes take precedence when non-neutral).
pub fn vehicle_input(
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    mut vehicles: Query<&mut VehicleInput, With<PlayerVehicle>>,
    cam_mode: Res<CameraMode>,
    session: Res<Session>,
    race: Option<Res<RaceState>>,
    windows: Query<&Window>,
) {
    // Driving controls are active only in chase-cam driving mode, with a
    // playing session and a focused window. Anything else writes a zeroed
    // input so the car rolls to a stop instead of holding stale controls.
    // A race countdown locks input the same way (AC03) — the session is
    // already `Countdown` in the normal flow, but `input_locked` also
    // covers a race resource that outlives its gate.
    let race_locked = race.is_some_and(|r| r.input_locked() && !r.is_stale(session.generation()));
    let focused = windows.iter().all(|w| w.focused);
    let driving = *cam_mode == CameraMode::Chase && session.is_playing() && focused && !race_locked;
    if !driving {
        for mut vi in &mut vehicles {
            *vi = VehicleInput::default();
        }
        return;
    }

    let mut input = VehicleInput::default();
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        input.throttle = 1.0;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        input.brake = 1.0;
    }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        input.steering -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        input.steering += 1.0;
    }
    if keys.pressed(KeyCode::Space) {
        input.handbrake = 1.0;
    }

    if let Some(pad) = gamepads.iter().next() {
        if let Some(x) = pad.get(GamepadAxis::LeftStickX)
            && x.abs() > 0.05
        {
            input.steering = x;
        }
        if let Some(rt) = pad.get(GamepadButton::RightTrigger2)
            && rt > 0.05
        {
            input.throttle = rt;
        }
        if let Some(lt) = pad.get(GamepadButton::LeftTrigger2)
            && lt > 0.05
        {
            input.brake = lt;
        }
        if pad.pressed(GamepadButton::South) {
            input.handbrake = 1.0;
        }
    }

    for mut vi in &mut vehicles {
        *vi = input;
    }
}
