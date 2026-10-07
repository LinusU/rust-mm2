//! Keyboard/gamepad → [`VehicleInput`] mapping.
//!
//! Controls are routed by mode: free-camera navigation must not drive the
//! car, and driving input is cleared while the session is not `Playing`
//! or the window has lost focus (so alt-tabbing away doesn't keep the
//! throttle pinned).

use bevy::prelude::*;
use mm2_game::{PlayerVehicle, RaceState, Session};
use mm2_vehicle::{ResetVehicle, Vehicle, VehicleInput, VehicleState};

use crate::camera::CameraMode;
use crate::controls::ControlSettings;
use crate::manual_gear::ManualGear;
use crate::session::{self, SpawnPoint};

/// The designed in-session pad map (F22-AC06's bindings leg; the
/// gamepad-only leg F23 req 4 asks for). The original's pad button
/// layout is unrecovered — MM2HELP's joystick/gamepad topics are
/// documented but not yet transcribed into the rules ledger — so
/// these are designed assignments over the same controls the
/// documented keys drive (HUD-3/CTL-1), not a claimed original map.
/// Every binding is additive: the key keeps working, any connected pad
/// answers (driving follows the first pad in use, see
/// [`ControlSettings::drive_input`]). The fullscreen pause map
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
    /// Nav-arrow target backward (`Z`). Yields to [`SHIFT_DOWN`] while
    /// the pad is shifting a manual gearbox.
    pub const TARGET_PREV: GamepadButton = GamepadButton::LeftTrigger;
    /// Nav-arrow target forward (`X`). Yields to [`SHIFT_UP`] while the
    /// pad is shifting a manual gearbox.
    pub const TARGET_NEXT: GamepadButton = GamepadButton::RightTrigger;
    /// Manual gearbox: down one gear (`B`). The same shoulder as
    /// [`TARGET_PREV`]; see `ControlSettings::pad_shifts`.
    pub const SHIFT_DOWN: GamepadButton = GamepadButton::LeftTrigger;
    /// Manual gearbox: up one gear (`G`). The same shoulder as
    /// [`TARGET_NEXT`]; see `ControlSettings::pad_shifts`.
    pub const SHIFT_UP: GamepadButton = GamepadButton::RightTrigger;
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
        && (keys.just_pressed(key) || pads.iter().any(|p| p.just_pressed(button)))
}

/// Stick deflection past which a menu stick push counts as a nav press.
const NAV_STICK: f32 = 0.6;

/// The pad edges a menu-like screen (main menu, pause, results) reads
/// this frame, merged over **every** connected pad: an idle first pad —
/// a spare controller, a wheel that registers as a gamepad — must not
/// shadow the one the player is holding, and unplugging a pad leaves the
/// rest answering (F23-AC03's hot-plug leg). Each screen maps the edges
/// it has rows for and ignores the rest.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PadNav {
    /// D-pad up, or the left stick pushed up past the edge latch.
    pub up: bool,
    /// D-pad down, or the left stick pushed down past the edge latch.
    pub down: bool,
    pub left: bool,
    pub right: bool,
    /// `South`.
    pub accept: bool,
    /// `East`.
    pub back: bool,
    /// `West`.
    pub delete: bool,
    /// `Start` — the pause/results screens' second `Back`.
    pub start: bool,
}

/// Read the pad edges from every connected pad. `latch` is the screen's
/// stored last stick Y: the stick navigates on *edge* transitions so
/// holding it doesn't run the list. The strongest deflection among the
/// pads is the stick value, and with no pad connected the latch resets
/// to neutral — a pad plugged in with the stick already held then fires
/// once, never a stale suppressed edge.
pub fn pad_nav<'a>(pads: impl IntoIterator<Item = &'a Gamepad>, latch: &mut f32) -> PadNav {
    let mut nav = PadNav::default();
    let mut y = 0.0_f32;
    for pad in pads {
        nav.up |= pad.just_pressed(GamepadButton::DPadUp);
        nav.down |= pad.just_pressed(GamepadButton::DPadDown);
        nav.left |= pad.just_pressed(GamepadButton::DPadLeft);
        nav.right |= pad.just_pressed(GamepadButton::DPadRight);
        nav.accept |= pad.just_pressed(GamepadButton::South);
        nav.back |= pad.just_pressed(GamepadButton::East);
        nav.delete |= pad.just_pressed(GamepadButton::West);
        nav.start |= pad.just_pressed(GamepadButton::Start);
        let pad_y = pad.get(GamepadAxis::LeftStickY).unwrap_or(0.0);
        if pad_y.abs() > y.abs() {
            y = pad_y;
        }
    }
    if y > NAV_STICK && *latch <= NAV_STICK {
        nav.up = true;
    } else if y < -NAV_STICK && *latch >= -NAV_STICK {
        nav.down = true;
    }
    *latch = y;
    nav
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

/// What [`vehicle_input`] reads and writes on the player car. The gearbox
/// state and config are optional: a harness car without them still drives,
/// it just cannot run a manual gearbox.
type DrivenCar = (
    Entity,
    &'static mut VehicleInput,
    Option<&'static VehicleState>,
    Option<&'static Vehicle>,
);

/// Fill `VehicleInput` on the player vehicle from keyboard and the
/// connected gamepad (gamepad axes take precedence when non-neutral),
/// through the player's [`ControlSettings`] — the bound keys, stick and
/// trigger deadzones, steering sensitivity and inversion. Under the
/// manual [`TransmissionPolicy`](crate::controls::TransmissionPolicy) it also pins the gearbox through
/// `forced_gear` — only where this process is the authority: the wire
/// carries no gear, so a predicted (`Remote`) car pinned locally would
/// diverge from the host's automatic one.
// A Bevy system: each input device and context gate is its own parameter.
#[allow(clippy::too_many_arguments)]
pub fn vehicle_input(
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    mut vehicles: Query<DrivenCar, With<PlayerVehicle>>,
    mut manual: Local<ManualGear>,
    cam_mode: Res<CameraMode>,
    session: Res<Session>,
    race: Option<Res<RaceState>>,
    windows: Query<&Window>,
    controls: Option<Res<ControlSettings>>,
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
        // A pause or countdown ends the held gear: the next frame seeds
        // from whatever the car is in, so a shift key pressed meanwhile
        // never counts.
        manual.release();
        for (_, mut vi, _, _) in &mut vehicles {
            *vi = VehicleInput::default();
        }
        return;
    }

    // A harness app that never inserted the settings drives the shipped map.
    let fallback;
    let controls = match controls.as_deref() {
        Some(controls) => controls,
        None => {
            fallback = ControlSettings::default();
            &fallback
        }
    };
    let input = controls.drive_input(&keys, gamepads.iter());
    let manual_box = controls.pad_shifts(session.authority_role().is_authority());
    if !manual_box {
        manual.release();
    }
    let (key_up, key_down) = controls.shift_edges(&keys);
    let shift_up = key_up || gamepads.iter().any(|p| p.just_pressed(pad::SHIFT_UP));
    let shift_down = key_down || gamepads.iter().any(|p| p.just_pressed(pad::SHIFT_DOWN));

    for (car, mut vi, state, vehicle) in &mut vehicles {
        *vi = input;
        if let (true, Some(state), Some(vehicle)) = (manual_box, state, vehicle) {
            vi.forced_gear = Some(manual.command(
                car,
                state.gear,
                vehicle.config.transmission.gear_ratios.len(),
                shift_up,
                shift_down,
            ));
        }
    }
}

/// `R` / [`pad::RESET`] resets the player vehicle (and any trailer) to
/// the spawn point. Driving-phase only: a reset while `Paused` would
/// teleport the car under the overlay. Authority only: under a
/// predicted (`Remote`) session the local teleport would move a car the
/// host never reset — a self-teleport its wire copy can never learn —
/// so the key instead asks the authority: `netdrive::send_reset_request`
/// sends a `ResetRequest`, and the granted answer returns as the seat's
/// epoch-declared `Snap` (F25-A.5's own-seat reconcile; the wire request
/// is F25-B).
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
