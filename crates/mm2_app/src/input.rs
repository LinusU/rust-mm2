//! Keyboard/gamepad → [`VehicleInput`] mapping.

use bevy::prelude::*;
use mm2_game::PlayerVehicle;
use mm2_vehicle::VehicleInput;

/// Fill `VehicleInput` on the player vehicle from keyboard and the first
/// connected gamepad (gamepad axes take precedence when non-neutral).
pub fn vehicle_input(
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    mut vehicles: Query<&mut VehicleInput, With<PlayerVehicle>>,
) {
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
