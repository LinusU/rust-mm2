//! Keyboard/gamepad → [`VehicleInput`] mapping.
//!
//! Controls are routed by mode: free-camera navigation must not drive the
//! car, and driving input is cleared while the session is not `Playing`
//! or the window has lost focus (so alt-tabbing away doesn't keep the
//! throttle pinned).

use avian3d::prelude::LinearVelocity;
use bevy::prelude::*;
use mm2_game::{PlayerVehicle, RaceState, Session};
use mm2_vehicle::{ResetVehicle, Vehicle, VehicleInput, VehicleState};

use crate::camera::CameraMode;
use crate::contracts::ImpactFilter;
use crate::controls::{ControlSettings, DriveAction, MouseDrive, SLOTS, bound_keys, pad_button};
use crate::manual_gear::ManualGear;
use crate::pad_map::PadAction;
use crate::session::{self, SpawnPoint};

/// The *shipped* designed in-session pad map (F22-AC06's bindings leg; the
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
/// collide with a driving control. These are the defaults of
/// [`crate::pad_map::PadMap`]; systems read the player's map through
/// [`crate::controls::pad_button`], not these constants.
pub mod pad {
    use bevy::prelude::GamepadButton;
    /// Handbrake (`Space`) — held, like the key.
    pub const HANDBRAKE: GamepadButton = GamepadButton::South;
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

/// A minimized client: the OS reports a window with no pixels. Nothing
/// can be drawn into it and a viewport floored at one pixel would
/// overrun the target, so viewport-bearing cameras sleep while it holds.
pub fn window_minimized(window: &Window) -> bool {
    window.resolution.physical_width() == 0 || window.resolution.physical_height() == 0
}

/// One in-session control on either device: a key bound to `action` OR
/// its [`DriveAction::pad_twin`] button — inert while a window is
/// unfocused (the pad's edges would otherwise fire where the key's never
/// could). Menus keep their own pad row, so these only ever fire where
/// the matching system already gates the key — the pad adds a finger,
/// never a new context. `controls` is `None` in a harness app that never
/// inserted the settings, which reads the shipped keys and buttons.
pub fn control_just_pressed(
    keys: &ButtonInput<KeyCode>,
    pads: &Query<&Gamepad>,
    windows: &Query<&Window>,
    controls: Option<&ControlSettings>,
    action: DriveAction,
) -> bool {
    let button = action.pad_twin().and_then(|a| pad_button(controls, a));
    bound_just_pressed(keys, pads, windows, bound_keys(controls, action), button)
}

/// [`control_just_pressed`] with the key slots and pad button already
/// resolved, for a control that adjusts either (the nav-target buttons
/// yielding to the shift buttons).
pub fn bound_just_pressed(
    keys: &ButtonInput<KeyCode>,
    pads: &Query<&Gamepad>,
    windows: &Query<&Window>,
    bound: [Option<KeyCode>; SLOTS],
    button: Option<GamepadButton>,
) -> bool {
    windows_focused(windows)
        && (bound.into_iter().flatten().any(|k| keys.just_pressed(k))
            || button.is_some_and(|b| pads.iter().any(|p| p.just_pressed(b))))
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

/// Presence enables the ramming driver: `--ram` inserts it, and
/// [`ram_drive`] owns the player vehicle's [`VehicleInput`] while it
/// exists — the same resource-gated contract as [`ParkedDrive`].
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct RamDrive;

/// Steering gain of [`ram_drive`]: bearing radians → normalized lock.
/// A target beside the car (±π/2) saturates the lock; one nearly ahead
/// closes with a gentle correction.
const RAM_STEER_GAIN: f32 = 2.0;

/// Range (m) inside which [`ram_drive`] only steers once the target is
/// within [`RAM_ALIGNED`] of the nose.
const RAM_STANDOFF: f32 = 14.0;

/// Pace (m/s) above which [`ram_drive`] lifts off the throttle: at the
/// dev car's top speed its lock circle is too wide to return on.
const RAM_SPEED: f32 = 11.0;

/// Pace (m/s) [`ram_drive`] holds while the target is off the nose:
/// slow enough that the lock circle fits inside a grid's spacing. Only
/// a pursuer whose inputs act at once (the authority) takes it: through
/// the wire's input latency a joined client's slow turns missed about
/// half of its hits, where the full-pace law landed 8 in 8.
const RAM_TURN_SPEED: f32 = 5.0;

/// Bearing (rad) off the nose that still counts as on a charge line.
const RAM_ALIGNED: f32 = 0.45;

/// The pursuit evidence driver (F25-C): full throttle, steering at the
/// nearest other vehicle in the world — a real driven car-to-car
/// collision through the production `VehicleInput` path (and, on a
/// joined client, through the wire's input stream), which the straight
/// `Hold` driver cannot provide on a side-by-side grid. Paired with
/// `--parked` neighbours it gives a multi-process run a victim that
/// holds still, so the authority's contact response and the replicated
/// impact stream are the only thing that moves it. On the authority the
/// car turns at a slow pace and, once it has struck something after
/// getting up to speed, parks (handbrake, as [`parked_drive`]), so the
/// whole field comes to rest and every process can be asked where each
/// seat ended up; a joined client keeps the full-pace law (see
/// [`RAM_TURN_SPEED`]). An evidence driver,
/// not a gameplay feature. With no other vehicle it drives straight.
pub fn ram_drive(
    mut me: Query<(&GlobalTransform, &LinearVelocity, &mut VehicleInput), With<PlayerVehicle>>,
    others: Query<&GlobalTransform, (With<Vehicle>, Without<PlayerVehicle>)>,
    session: Res<Session>,
    race: Option<Res<RaceState>>,
    impacts: Option<Res<ImpactFilter>>,
    mut strike: Local<RamStrike>,
) {
    // The countdown lock holds for this driver as it does for the
    // keyboard mapping (AC03): outside a live, unlocked session the car
    // gets a neutral input rather than a launch.
    let race_locked = race.is_some_and(|r| r.input_locked() && !r.is_stale(session.generation()));
    let driving = session.is_playing() && !race_locked;
    if !driving {
        *strike = RamStrike::default();
    }
    // A pursuer whose inputs act at once (the authority) turns at the
    // slow pace and parks after its strike; a joined client's inputs
    // reach the authority late, so both would starve the authority's
    // own contact (it would see the handbrake before the impact) — it
    // keeps the full-pace law and drives on.
    let authority = session.authority_role().is_authority();
    let turn_pace = if authority { RAM_TURN_SPEED } else { RAM_SPEED };
    for (at, vel, mut vi) in &mut me {
        *vi = if !driving {
            VehicleInput::default()
        } else if authority
            && strike.observe(vel.length(), impacts.as_ref().map_or(0, |i| i.emitted))
        {
            VehicleInput {
                handbrake: 1.0,
                ..default()
            }
        } else {
            let from = at.translation();
            let nearest = others.iter().map(|t| t.translation()).min_by(|a, b| {
                a.distance_squared(from)
                    .total_cmp(&b.distance_squared(from))
            });
            ram_input(at, vel.length(), nearest, turn_pace)
        };
    }
}

/// Pace (m/s) the ramming car must pass before a later impact counts as
/// its strike — the spawn landing happens at rest and must not park it.
const RAM_LAUNCHED: f32 = 3.0;

/// [`ram_drive`]'s per-session memory: has the car launched, and has it
/// since struck something. Impacts are read off the contract pipeline's
/// running count ([`ImpactFilter::emitted`]) against the count at launch.
#[derive(Debug, Default)]
pub struct RamStrike {
    baseline: u64,
    launched: bool,
    struck: bool,
}

impl RamStrike {
    /// Feed one frame (`speed` m/s, `emitted` impacts so far); true once
    /// the car has struck something after launching.
    pub fn observe(&mut self, speed: f32, emitted: u64) -> bool {
        if !self.launched {
            self.baseline = emitted;
            self.launched = speed > RAM_LAUNCHED;
        } else if emitted > self.baseline {
            self.struck = true;
        }
        self.struck
    }
}

/// One frame of [`ram_drive`]'s law: the car at `at` moving `speed`
/// m/s, pursuing `target` (none = drive straight), holding `turn_pace`
/// while the target is off the nose.
pub fn ram_input(
    at: &GlobalTransform,
    speed: f32,
    target: Option<Vec3>,
    turn_pace: f32,
) -> VehicleInput {
    let from = at.translation();
    let steering = target.map_or(0.0, |to| {
        // Forward is −Z at yaw 0 — the `Quat::from_rotation_y` frame
        // `relative_bearing` is defined on.
        let fwd = at.rotation() * Vec3::NEG_Z;
        let yaw = (-fwd.x).atan2(-fwd.z);
        let bearing = mm2_game::relative_bearing(yaw, from, to);
        // A target close and off the nose cannot be turned onto — a
        // car at full lock orbits a neighbour on the next grid seat
        // without touching it — so open the range straight ahead first
        // and charge once the run-up has put the target on a line the
        // lock can take.
        if from.distance(to) < RAM_STANDOFF && bearing.abs() > RAM_ALIGNED {
            0.0
        } else {
            (bearing * RAM_STEER_GAIN).clamp(-1.0, 1.0)
        }
    });
    // Still turning onto the target: stay at the pace the lock circle
    // can take. On a charge line (or with nothing to chase) the full
    // pace is allowed.
    let turning = target.is_some_and(|to| {
        let fwd = at.rotation() * Vec3::NEG_Z;
        let yaw = (-fwd.x).atan2(-fwd.z);
        mm2_game::relative_bearing(yaw, from, to).abs() > RAM_ALIGNED
    });
    let pace = if turning { turn_pace } else { RAM_SPEED };
    VehicleInput {
        // Lifting off above the pace keeps the turning circle small
        // enough for the car to come back round to the target.
        throttle: if speed > pace { 0.0 } else { 1.0 },
        steering,
        ..default()
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
    mouse: Option<Res<ButtonInput<MouseButton>>>,
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
    let mut input = controls.drive_input(&keys, gamepads.iter());
    controls.apply_mouse(&mut input, mouse_drive(&windows, mouse.as_deref()));
    let manual_box = controls.pad_shifts(session.authority_role().is_authority());
    if !manual_box {
        manual.release();
    }
    let (key_up, key_down) = controls.shift_edges(&keys);
    let pad_edge = |action| {
        controls
            .pad
            .button(action)
            .is_some_and(|b| gamepads.iter().any(|p| p.just_pressed(b)))
    };
    let shift_up = key_up || pad_edge(PadAction::ShiftUp);
    let shift_down = key_down || pad_edge(PadAction::ShiftDown);

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

/// The mouse as [`ControlSettings::apply_mouse`] reads it: the first
/// window holding the cursor gives the horizontal offset from its centre
/// (logical pixels, so scale factor cancels), and the buttons are the
/// held left and right. A harness without the button resource sees none
/// pressed.
fn mouse_drive(windows: &Query<&Window>, buttons: Option<&ButtonInput<MouseButton>>) -> MouseDrive {
    let offset = windows.iter().find_map(|w| {
        let half = w.width() / 2.0;
        let x = w.cursor_position()?.x;
        (half > 0.0).then(|| ((x - half) / half).clamp(-1.0, 1.0))
    });
    MouseDrive {
        offset,
        left: buttons.is_some_and(|b| b.pressed(MouseButton::Left)),
        right: buttons.is_some_and(|b| b.pressed(MouseButton::Right)),
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
// A Bevy system: each parameter is one injected resource or query, and the
// pad map is one more of them.
#[allow(clippy::too_many_arguments)]
pub fn reset_input(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    windows: Query<&Window>,
    session: Res<Session>,
    spawn: Res<SpawnPoint>,
    controls: Option<Res<ControlSettings>>,
    player: Query<Entity, With<PlayerVehicle>>,
    mut writer: MessageWriter<ResetVehicle>,
) {
    if !session.is_playing()
        || !session.authority_role().is_authority()
        || !control_just_pressed(
            &keys,
            &pads,
            &windows,
            controls.as_deref(),
            DriveAction::Reset,
        )
    {
        return;
    }
    for msg in session::spawn_resets(&spawn, player.iter().next()) {
        writer.write(msg);
    }
}
