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

/// Presence enables the parked driver: `--parked` inserts it, and
/// [`parked_drive`] owns the player vehicle's [`VehicleInput`] while it
/// exists. A resource (not a CLI argument threaded everywhere) so both
/// the windowed app and the headless smoke gate the same system — the
/// same contract [`crate::scripted::ScriptedDrive`] holds for `--bot`.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct ParkedDrive;

/// The stationary evidence driver (F13-C.2): writes the parked input —
/// handbrake held, no throttle/steering — every frame. Scheduled `after`
/// [`vehicle_input`], so while `--parked` is on it deterministically owns
/// the input exactly like the scripted driver owns it under `--bot`.
///
/// "Parked" means the local participant never races: it stays on the
/// grid (a handbrake, not the brake pedal — the pedal is the reverse
/// throttle once stopped) so an event session measures what the
/// *opponents* do without a competing local driver — the control leg a
/// blind full-throttle `Hold` run cannot provide.
pub fn parked_drive(mut vehicles: Query<&mut VehicleInput, With<PlayerVehicle>>) {
    for mut vi in &mut vehicles {
        *vi = VehicleInput {
            handbrake: 1.0,
            ..default()
        };
    }
}

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
    // Driving controls are active in every drive view — the chase
    // lenses and the cockpit (HUD-3's three views are all drive views);
    // only Free (fly camera) detaches input, with a playing session and
    // a focused window. Anything else writes a zeroed input so the car
    // rolls to a stop instead of holding stale controls.
    // A race countdown locks input the same way (AC03) — the session is
    // already `Countdown` in the normal flow, but `input_locked` also
    // covers a race resource that outlives its gate.
    let race_locked = race.is_some_and(|r| r.input_locked() && !r.is_stale(session.generation()));
    let focused = windows.iter().all(|w| w.focused);
    let driving =
        !matches!(*cam_mode, CameraMode::Free) && session.is_playing() && focused && !race_locked;
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
