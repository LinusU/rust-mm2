//! Keyboard/gamepad → [`VehicleInput`] mapping.
//!
//! Controls are routed by mode: free-camera navigation must not drive the
//! car, and driving input is cleared while the session is not `Playing`
//! or the window has lost focus (so alt-tabbing away doesn't keep the
//! throttle pinned).

use bevy::prelude::*;
use mm2_game::{PlayerVehicle, RaceState, Session};
use mm2_vehicle::{ResetVehicle, VehicleInput};

use crate::camera::CameraMode;
use crate::session::{self, SpawnPoint};

/// The designed in-session pad map (F22-AC06's bindings leg; the
/// gamepad-only leg F23 req 4 asks for). The original's pad button
/// layout is unrecovered — MM2HELP's joystick/gamepad topics are
/// documented but not yet transcribed into the rules ledger — so
/// these are designed assignments over the same controls the
/// documented keys drive (HUD-3/CTL-1), not a claimed original map.
/// Every binding is additive: the key keeps working, the first
/// connected pad answers — the same first-pad rule
/// [`vehicle_input`] uses for steering. The fullscreen pause map
/// (`Q`), headlights (`L`), the `F1`/`F4` debug keys and the
/// fly-camera axes stay keyboard-only: `Start` is already the
/// menu-owned pause and no designed button is left that doesn't
/// collide with a driving control.
pub mod pad {
    use bevy::prelude::GamepadButton;
    /// Cycle the HUD-3 camera chain (`C`) — right stick click.
    pub const CAMERA: GamepadButton = GamepadButton::RightThumb;
    /// Cockpit/dash toggle (`V`).
    pub const COCKPIT: GamepadButton = GamepadButton::West;
    /// Rear-view mirror strip (BACKSPACE).
    pub const MIRROR: GamepadButton = GamepadButton::East;
    /// Reset the vehicle to spawn (`R`).
    pub const RESET: GamepadButton = GamepadButton::North;
    /// Horn (`ENTER`; the siren toggle on `SIREN_FLAG` cars).
    pub const HORN: GamepadButton = GamepadButton::LeftThumb;
    /// Cycle the corner map's views (`TAB`).
    pub const MAP_VIEW: GamepadButton = GamepadButton::Select;
    /// Map zoom (`E`).
    pub const MAP_ZOOM: GamepadButton = GamepadButton::DPadLeft;
    /// Map orientation (`F`).
    pub const MAP_ROTATE: GamepadButton = GamepadButton::DPadRight;
    /// Driving-HUD master gate (`H`).
    pub const HUD: GamepadButton = GamepadButton::DPadUp;
    /// Opponent indicators (`I`).
    pub const INDICATORS: GamepadButton = GamepadButton::DPadDown;
    /// Nav-arrow target backward (`Z`).
    pub const TARGET_PREV: GamepadButton = GamepadButton::LeftTrigger;
    /// Nav-arrow target forward (`X`).
    pub const TARGET_NEXT: GamepadButton = GamepadButton::RightTrigger;
}

/// Every window focused — headless runs own none and count as
/// focused. The OS never delivers a key press to an unfocused window,
/// but gilrs-style backends keep reporting pad state regardless, so
/// every pad-fed control gates on this to share the keyboard's
/// effective contract.
pub fn windows_focused(windows: &Query<&Window>) -> bool {
    windows.iter().all(|w| w.focused)
}

/// One in-session control on either device: the documented key OR its
/// designed [`pad`] binding — inert while a window is unfocused (the
/// pad's edges would otherwise fire where the key's never could).
/// Menus keep their own pad row, so these only ever fire where the
/// matching system already gates the key — the pad adds a finger,
/// never a new context.
pub fn control_just_pressed(
    keys: &ButtonInput<KeyCode>,
    pads: &Query<&Gamepad>,
    windows: &Query<&Window>,
    key: KeyCode,
    button: GamepadButton,
) -> bool {
    windows_focused(windows)
        && (keys.just_pressed(key) || pads.iter().next().is_some_and(|p| p.just_pressed(button)))
}

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
    let focused = windows_focused(&windows);
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

/// `R` / [`pad::RESET`] resets the player vehicle (and any trailer) to
/// the spawn point. Driving-phase only: a reset while `Paused` would
/// teleport the car under the overlay. Authority only: under a
/// predicted (`Remote`) session the local teleport would move a car the
/// host never reset — a self-teleport its wire copy can never learn —
/// so the remote driver's recovery is the authority's detectors
/// resolving the seat and the reset epoch carrying it back (F25-A.5;
/// a driver-requested reset over the wire is F25-B scope).
pub fn reset_input(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    windows: Query<&Window>,
    session: Res<Session>,
    spawn: Res<SpawnPoint>,
    player: Query<Entity, With<PlayerVehicle>>,
    mut writer: MessageWriter<ResetVehicle>,
) {
    if !session.is_playing()
        || !session.authority_role().is_authority()
        || !control_just_pressed(&keys, &pads, &windows, KeyCode::KeyR, pad::RESET)
    {
        return;
    }
    for msg in session::spawn_resets(&spawn, player.iter().next()) {
        writer.write(msg);
    }
}
