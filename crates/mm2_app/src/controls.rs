//! User control settings: the driving keys a player can rebind and the
//! gamepad's deadzones, steering sensitivity and inversion (F23-A).
//!
//! [`ControlSettings`] is the one place device mappings live.
//! [`ControlSettings::drive_input`] turns raw keyboard/pad state into the
//! normalized [`VehicleInput`], so [`crate::input::vehicle_input`] and the
//! physics behind it never see a key code. `Default` is exactly the map
//! the game shipped with — `W`/`↑` throttle, `S`/`↓` brake, `A`/`←` and
//! `D`/`→` steer, `Space` handbrake, a 0.05 stick/trigger deadzone — so
//! nobody's controls change until they ask them to.
//!
//! Scope: the seven *driving* actions ([`DriveAction::DRIVING`]) and the
//! thirteen in-session controls ([`DriveAction::IN_GAME`] — camera,
//! mirror, map, reset…, F23-A.5) share one key namespace: a key belongs
//! to one action, so a driving key can never also fire a camera and the
//! refusal names the owner. [`RESERVED_KEYS`] holds the one key that is
//! not rebindable (the pause map). Menu navigation keeps its own keys.
//! The main menu's and the pause overlay's Controls pages rebind through
//! [`ControlSettings::with_key`]; the pad's digital buttons live in
//! [`ControlSettings::pad`] (see [`crate::pad_map`]).
//!
//! The settings are machine-level like [`crate::settings::GraphicsSettings`]
//! and persist beside them as `controls.json` in the profile store's
//! root. A key is stored by its Bevy `KeyCode` name (`"KeyW"`), limited
//! to the [`BINDABLE`] table — letters, digits, arrows, `Space` and
//! `Shift` — so a file can never name a key the app cannot distinguish.
//! Loading validates everything: an invalid piece is replaced by its
//! default and reported, never trusted and never fatal.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use mm2_vehicle::VehicleInput;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::pad_map::{PadAction, PadMap, button_from_name, button_name};
use crate::settings::write_json_atomically;

/// File name inside the settings directory.
pub const CONTROLS_FILE: &str = "controls.json";

/// The controls file inside `root` (the profile store's root).
pub fn controls_path(root: &Path) -> PathBuf {
    root.join(CONTROLS_FILE)
}

/// How many keys one action can hold (a primary and an alternate).
pub const SLOTS: usize = 2;

/// A rebindable key action: one of the [`Self::DRIVING`] controls or an
/// [`Self::IN_GAME`] one. (The name predates the in-game ones.)
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DriveAction {
    Throttle,
    Brake,
    SteerLeft,
    SteerRight,
    Handbrake,
    /// Next gear up — answers only under [`TransmissionPolicy::Manual`].
    ShiftUp,
    /// Next gear down — answers only under [`TransmissionPolicy::Manual`].
    ShiftDown,
    /// Change camera (chase / bumper / ... — the next available view).
    Camera,
    Cockpit,
    /// Rear-view mirror.
    Mirror,
    /// Reset the vehicle to its spawn.
    Reset,
    /// Horn, or the siren on an emergency vehicle.
    Horn,
    MapView,
    MapZoom,
    MapRotate,
    /// Driving HUD on/off.
    Hud,
    /// Opponent indicators on/off.
    Indicators,
    TargetPrev,
    TargetNext,
    Headlights,
}

impl DriveAction {
    /// The controls that drive the car, in the order a rebinding screen
    /// lists them.
    pub const DRIVING: [Self; 7] = [
        Self::Throttle,
        Self::Brake,
        Self::SteerLeft,
        Self::SteerRight,
        Self::Handbrake,
        Self::ShiftUp,
        Self::ShiftDown,
    ];

    /// The in-session controls that do not drive the car.
    pub const IN_GAME: [Self; 13] = [
        Self::Camera,
        Self::Cockpit,
        Self::Mirror,
        Self::Reset,
        Self::Horn,
        Self::MapView,
        Self::MapZoom,
        Self::MapRotate,
        Self::Hud,
        Self::Indicators,
        Self::TargetPrev,
        Self::TargetNext,
        Self::Headlights,
    ];

    /// Every action: [`Self::DRIVING`] then [`Self::IN_GAME`].
    pub const ALL: [Self; 20] = [
        Self::Throttle,
        Self::Brake,
        Self::SteerLeft,
        Self::SteerRight,
        Self::Handbrake,
        Self::ShiftUp,
        Self::ShiftDown,
        Self::Camera,
        Self::Cockpit,
        Self::Mirror,
        Self::Reset,
        Self::Horn,
        Self::MapView,
        Self::MapZoom,
        Self::MapRotate,
        Self::Hud,
        Self::Indicators,
        Self::TargetPrev,
        Self::TargetNext,
        Self::Headlights,
    ];

    /// The pad button action that does the same job, for the controls the
    /// gamepad shares (everything but the headlights).
    pub fn pad_twin(self) -> Option<PadAction> {
        Some(match self {
            Self::Camera => PadAction::Camera,
            Self::Cockpit => PadAction::Cockpit,
            Self::Mirror => PadAction::Mirror,
            Self::Reset => PadAction::Reset,
            Self::Horn => PadAction::Horn,
            Self::MapView => PadAction::MapView,
            Self::MapZoom => PadAction::MapZoom,
            Self::MapRotate => PadAction::MapRotate,
            Self::Hud => PadAction::Hud,
            Self::Indicators => PadAction::Indicators,
            Self::TargetPrev => PadAction::TargetPrev,
            Self::TargetNext => PadAction::TargetNext,
            _ => return None,
        })
    }

    /// The row label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Throttle => "Accelerate",
            Self::Brake => "Brake / reverse",
            Self::SteerLeft => "Steer left",
            Self::SteerRight => "Steer right",
            Self::Handbrake => "Handbrake",
            Self::ShiftUp => "Shift up (manual)",
            Self::ShiftDown => "Shift down (manual)",
            Self::Camera => "Change camera",
            Self::Cockpit => "Cockpit view",
            Self::Mirror => "Rear-view mirror",
            Self::Reset => "Reset vehicle",
            Self::Horn => "Horn / siren",
            Self::MapView => "Map view",
            Self::MapZoom => "Map zoom",
            Self::MapRotate => "Map orientation",
            Self::Hud => "Driving HUD",
            Self::Indicators => "Opponent indicators",
            Self::TargetPrev => "Previous target",
            Self::TargetNext => "Next target",
            Self::Headlights => "Headlights",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|a| *a == self).unwrap_or(0)
    }

    /// The shipped keys: the primary, then the alternate (if any).
    fn default_keys(self) -> [Option<KeyCode>; SLOTS] {
        match self {
            Self::Throttle => [Some(KeyCode::KeyW), Some(KeyCode::ArrowUp)],
            Self::Brake => [Some(KeyCode::KeyS), Some(KeyCode::ArrowDown)],
            Self::SteerLeft => [Some(KeyCode::KeyA), Some(KeyCode::ArrowLeft)],
            Self::SteerRight => [Some(KeyCode::KeyD), Some(KeyCode::ArrowRight)],
            Self::Handbrake => [Some(KeyCode::Space), None],
            // The documented A / Z (CTL-1) collide with the WASD map and
            // the nav-arrow keys, so the shifts get free letters.
            Self::ShiftUp => [Some(KeyCode::KeyG), None],
            Self::ShiftDown => [Some(KeyCode::KeyB), None],
            Self::Camera => [Some(KeyCode::KeyC), None],
            Self::Cockpit => [Some(KeyCode::KeyV), None],
            Self::Mirror => [Some(KeyCode::Backspace), None],
            Self::Reset => [Some(KeyCode::KeyR), None],
            Self::Horn => [Some(KeyCode::Enter), None],
            Self::MapView => [Some(KeyCode::Tab), None],
            Self::MapZoom => [Some(KeyCode::KeyE), None],
            Self::MapRotate => [Some(KeyCode::KeyF), None],
            Self::Hud => [Some(KeyCode::KeyH), None],
            Self::Indicators => [Some(KeyCode::KeyI), None],
            Self::TargetPrev => [Some(KeyCode::KeyZ), None],
            Self::TargetNext => [Some(KeyCode::KeyX), None],
            Self::Headlights => [Some(KeyCode::KeyL), None],
        }
    }
}

/// Every key an action may be bound to, with the name the file stores.
/// Modifiers other than `Shift`, function keys, `Escape` and the numpad
/// are left out: the app's own fixed controls use them. `Enter`, `Tab`
/// and `Backspace` are here because the shipped horn, map-view and mirror
/// keys are those.
pub const BINDABLE: &[(&str, KeyCode)] = &[
    ("KeyA", KeyCode::KeyA),
    ("KeyB", KeyCode::KeyB),
    ("KeyC", KeyCode::KeyC),
    ("KeyD", KeyCode::KeyD),
    ("KeyE", KeyCode::KeyE),
    ("KeyF", KeyCode::KeyF),
    ("KeyG", KeyCode::KeyG),
    ("KeyH", KeyCode::KeyH),
    ("KeyI", KeyCode::KeyI),
    ("KeyJ", KeyCode::KeyJ),
    ("KeyK", KeyCode::KeyK),
    ("KeyL", KeyCode::KeyL),
    ("KeyM", KeyCode::KeyM),
    ("KeyN", KeyCode::KeyN),
    ("KeyO", KeyCode::KeyO),
    ("KeyP", KeyCode::KeyP),
    ("KeyQ", KeyCode::KeyQ),
    ("KeyR", KeyCode::KeyR),
    ("KeyS", KeyCode::KeyS),
    ("KeyT", KeyCode::KeyT),
    ("KeyU", KeyCode::KeyU),
    ("KeyV", KeyCode::KeyV),
    ("KeyW", KeyCode::KeyW),
    ("KeyX", KeyCode::KeyX),
    ("KeyY", KeyCode::KeyY),
    ("KeyZ", KeyCode::KeyZ),
    ("Digit0", KeyCode::Digit0),
    ("Digit1", KeyCode::Digit1),
    ("Digit2", KeyCode::Digit2),
    ("Digit3", KeyCode::Digit3),
    ("Digit4", KeyCode::Digit4),
    ("Digit5", KeyCode::Digit5),
    ("Digit6", KeyCode::Digit6),
    ("Digit7", KeyCode::Digit7),
    ("Digit8", KeyCode::Digit8),
    ("Digit9", KeyCode::Digit9),
    ("ArrowUp", KeyCode::ArrowUp),
    ("ArrowDown", KeyCode::ArrowDown),
    ("ArrowLeft", KeyCode::ArrowLeft),
    ("ArrowRight", KeyCode::ArrowRight),
    ("Space", KeyCode::Space),
    ("Enter", KeyCode::Enter),
    ("Tab", KeyCode::Tab),
    ("Backspace", KeyCode::Backspace),
    ("ShiftLeft", KeyCode::ShiftLeft),
    ("ShiftRight", KeyCode::ShiftRight),
];

/// Bindable keys no action may take: `Q`, which opens (and closes) the
/// full-screen pause map and has no rebindable twin yet.
pub const RESERVED_KEYS: &[KeyCode] = &[KeyCode::KeyQ];

/// The file/UI name of a bindable key, `None` for any other key.
pub fn key_name(key: KeyCode) -> Option<&'static str> {
    BINDABLE.iter().find(|(_, k)| *k == key).map(|(n, _)| *n)
}

/// The bindable key a file name stands for.
pub fn key_from_name(name: &str) -> Option<KeyCode> {
    BINDABLE.iter().find(|(n, _)| *n == name).map(|(_, k)| *k)
}

/// Why a key cannot take a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindError {
    /// Not in [`BINDABLE`].
    NotBindable,
    /// One of [`RESERVED_KEYS`].
    Reserved,
    /// Already bound to this other action — unbind it there first.
    Conflict(DriveAction),
    /// Already this action's other key.
    AlreadyBound,
    /// Clearing would leave the action with no key at all.
    LastKey,
    /// The slot index is not below [`SLOTS`].
    NoSuchSlot,
}

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotBindable => write!(f, "that key cannot be used in the game"),
            Self::Reserved => write!(f, "that key is used by an in-game control"),
            Self::Conflict(a) => write!(f, "already bound to {}", a.label()),
            Self::AlreadyBound => write!(f, "already this action's other key"),
            Self::LastKey => write!(f, "an action needs at least one key"),
            Self::NoSuchSlot => write!(f, "no such key slot"),
        }
    }
}

/// Deadzones are fractions of the axis; past 0.9 a stick could not
/// reach full lock.
pub const DEADZONE_RANGE: (f32, f32) = (0.0, 0.9);
/// Steering sensitivity scales the stick before the −1…1 clamp.
pub const SENSITIVITY_RANGE: (f32, f32) = (0.25, 2.0);

const DEFAULT_DEADZONE: f32 = 0.05;

/// Who changes gear. The sim's gearbox is automatic; `Manual` pins it
/// through [`mm2_vehicle::VehicleInput::forced_gear`] (CTL-1 documents an
/// automatic/manual choice; the keys that drive it are designed, DSN-77).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransmissionPolicy {
    #[default]
    Automatic,
    Manual,
}

impl TransmissionPolicy {
    /// The row value.
    pub fn label(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic",
            Self::Manual => "Manual",
        }
    }

    /// The other policy.
    pub fn toggled(self) -> Self {
        match self {
            Self::Automatic => Self::Manual,
            Self::Manual => Self::Automatic,
        }
    }
}

/// Forward speed, m/s, at or below which a car counts as stopped for the
/// auto-reverse policy — a little above the sim's own reverse engage
/// speed (0.25) so the brake is already a handbrake when the sim would
/// otherwise swap it for the reverse gear.
const STOPPED_SPEED: f32 = 0.5;
/// A brake axis at or below this is released (the sim's own engage level).
const BRAKE_PRESSED: f32 = 0.05;

/// Whether the driver's current brake press began while the car was still
/// moving — the memory behind "auto reverse: off" (F23-A.7, DSN-98).
///
/// The sim reverses a car whenever the brake is held at a standstill. With
/// auto reverse off, [`Self::apply`] turns a brake carried through the
/// stop into a handbrake hold, so the car stops and stays; releasing the
/// brake and pressing it again once stopped is the deliberate reverse.
/// It rewrites the normalized input only, so the physics systems and the
/// wire (which carries brake and handbrake, not a reverse flag) are
/// unchanged and a predicted copy behaves like the authority.
#[derive(Debug, Default)]
pub struct BrakeCarry {
    carried: bool,
}

impl BrakeCarry {
    /// Forget the press (a pause, a countdown, a respawned car).
    pub fn release(&mut self) {
        self.carried = false;
    }

    /// Apply the policy for one frame. `forward_speed` is the car's
    /// signed speed along its heading, or `None` for a car without sim
    /// state (nothing to judge by, so the input passes untouched).
    pub fn apply(
        &mut self,
        controls: &ControlSettings,
        input: &mut VehicleInput,
        speed: Option<f32>,
    ) {
        if controls.auto_reverse || input.brake <= BRAKE_PRESSED {
            self.carried = false;
            return;
        }
        let Some(speed) = speed else { return };
        if speed > STOPPED_SPEED {
            self.carried = true;
        }
        if self.carried && speed <= STOPPED_SPEED {
            input.handbrake = input.handbrake.max(input.brake);
            input.brake = 0.0;
        }
    }
}

/// The user's driving controls. `Default` is the shipped map.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct ControlSettings {
    /// Keys per action (indexed by [`DriveAction::ALL`]), primary first.
    bindings: [[Option<KeyCode>; SLOTS]; DriveAction::ALL.len()],
    /// Left-stick |x| at or below this steers nothing.
    pub steer_deadzone: f32,
    /// Trigger value at or below this is released.
    pub trigger_deadzone: f32,
    /// Stick steering gain (1.0 = the raw axis).
    pub steer_sensitivity: f32,
    /// Flip the stick's steering direction. Keys are never inverted —
    /// that would just swap the two bindings.
    pub invert_steering: bool,
    /// Automatic gearbox, or manual with the shift keys.
    pub transmission: TransmissionPolicy,
    /// Drive with the mouse (CTL-2, F23-A.6): the cursor's offset from
    /// the window centre steers, the left button throttles and the right
    /// brakes. Off by default — the keys, and any pad, keep working
    /// either way.
    pub mouse_driving: bool,
    /// Auto reverse (CTL-3, F23-A.7): on, holding the brake at a
    /// standstill puts the car in reverse (the shipped behaviour); off,
    /// a brake held *through* the stop only holds the car, and reverse
    /// takes a fresh press once stopped (see [`BrakeCarry`]).
    pub auto_reverse: bool,
    /// The gamepad's digital buttons (F23-A.4).
    pub pad: PadMap,
}

impl Default for ControlSettings {
    fn default() -> Self {
        Self {
            bindings: DriveAction::ALL.map(DriveAction::default_keys),
            steer_deadzone: DEFAULT_DEADZONE,
            trigger_deadzone: DEFAULT_DEADZONE,
            steer_sensitivity: 1.0,
            invert_steering: false,
            transmission: TransmissionPolicy::Automatic,
            mouse_driving: false,
            auto_reverse: true,
            pad: PadMap::default(),
        }
    }
}

/// What the mouse says this frame, already reduced to the three facts
/// [`ControlSettings::apply_mouse`] needs — the system that reads the
/// window and the buttons owns the device, this table owns the mapping.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MouseDrive {
    /// The cursor's horizontal offset from the window centre, -1 at the
    /// left edge to 1 at the right; `None` when it is outside the window.
    pub offset: Option<f32>,
    pub left: bool,
    pub right: bool,
}

impl ControlSettings {
    /// The keys bound to `action`, primary first.
    pub fn keys(&self, action: DriveAction) -> impl Iterator<Item = KeyCode> + '_ {
        self.bindings[action.index()].iter().flatten().copied()
    }

    /// The key in `slot`, if any.
    pub fn key_at(&self, action: DriveAction, slot: usize) -> Option<KeyCode> {
        self.bindings[action.index()].get(slot).copied().flatten()
    }

    /// The action `key` is bound to, for conflict display.
    pub fn owner_of(&self, key: KeyCode) -> Option<DriveAction> {
        DriveAction::ALL
            .into_iter()
            .find(|a| self.keys(*a).any(|k| k == key))
    }

    /// Check `key` may go in `action`'s `slot` without changing anything.
    pub fn check_bind(
        &self,
        action: DriveAction,
        slot: usize,
        key: KeyCode,
    ) -> Result<(), BindError> {
        if slot >= SLOTS {
            return Err(BindError::NoSuchSlot);
        }
        if key_name(key).is_none() {
            return Err(BindError::NotBindable);
        }
        if RESERVED_KEYS.contains(&key) {
            return Err(BindError::Reserved);
        }
        // Re-binding the key a slot already holds is a harmless no-op.
        if self.key_at(action, slot) == Some(key) {
            return Ok(());
        }
        match self.owner_of(key) {
            Some(owner) if owner != action => Err(BindError::Conflict(owner)),
            Some(_) => Err(BindError::AlreadyBound),
            None => Ok(()),
        }
    }

    /// Bind `key` to `action`'s `slot`, replacing what was there. A key
    /// another action holds is refused with [`BindError::Conflict`] so
    /// the screen can name the owner; nothing is ever silently stolen.
    pub fn rebind(
        &mut self,
        action: DriveAction,
        slot: usize,
        key: KeyCode,
    ) -> Result<(), BindError> {
        self.check_bind(action, slot, key)?;
        self.bindings[action.index()][slot] = Some(key);
        Ok(())
    }

    /// Clear `action`'s `slot`. The last remaining key of an action
    /// cannot be cleared — an unbound action is a dead control.
    pub fn unbind(&mut self, action: DriveAction, slot: usize) -> Result<(), BindError> {
        if slot >= SLOTS {
            return Err(BindError::NoSuchSlot);
        }
        if self.keys(action).count() <= 1 && self.key_at(action, slot).is_some() {
            return Err(BindError::LastKey);
        }
        self.bindings[action.index()][slot] = None;
        Ok(())
    }

    /// Every key bound to more than one action, with its owners. Empty
    /// for any set built through [`Self::rebind`] or [`Self::load`].
    pub fn conflicts(&self) -> Vec<(KeyCode, DriveAction, DriveAction)> {
        let mut out = Vec::new();
        for (i, a) in DriveAction::ALL.iter().enumerate() {
            for b in &DriveAction::ALL[i + 1..] {
                for key in self.keys(*a) {
                    if self.keys(*b).any(|k| k == key) {
                        out.push((key, *a, *b));
                    }
                }
            }
        }
        out
    }

    fn pressed(&self, action: DriveAction, keys: &ButtonInput<KeyCode>) -> bool {
        self.keys(action).any(|k| keys.pressed(k))
    }

    /// Whether this process pins the gearbox from the player's shift
    /// presses: the manual policy, on a session this process is the
    /// authority for (the wire carries no gear, so a predicted car stays
    /// automatic). While true the pad's shoulders shift and
    /// `input::pad::TARGET_PREV`/`TARGET_NEXT` yield to them — no free
    /// button is left, and the keyboard's `Z`/`X` still cycle the nav
    /// arrow.
    pub fn pad_shifts(&self, authority: bool) -> bool {
        authority && self.transmission == TransmissionPolicy::Manual
    }

    /// The `(up, down)` shift keys pressed this frame — edges, not holds,
    /// so one press is one gear.
    pub fn shift_edges(&self, keys: &ButtonInput<KeyCode>) -> (bool, bool) {
        let edge = |a| self.keys(a).any(|k| keys.just_pressed(k));
        (edge(DriveAction::ShiftUp), edge(DriveAction::ShiftDown))
    }

    /// Normalize raw device state into a [`VehicleInput`]: the bound keys
    /// first, then the analog axes of the pad being driven with on top (a
    /// stick/trigger past its deadzone outranks the keys, the precedence
    /// the game has always had). Of several connected pads the first one
    /// actually deflected, triggered or holding the handbrake owns the
    /// car this frame — an idle spare pad must not mute the one in the
    /// player's hands, and two pads never mix axes. Pure — callers own
    /// the context gates (focus, camera, session phase).
    pub fn drive_input<'a>(
        &self,
        keys: &ButtonInput<KeyCode>,
        pads: impl IntoIterator<Item = &'a Gamepad>,
    ) -> VehicleInput {
        let mut input = VehicleInput::default();
        if self.pressed(DriveAction::Throttle, keys) {
            input.throttle = 1.0;
        }
        if self.pressed(DriveAction::Brake, keys) {
            input.brake = 1.0;
        }
        if self.pressed(DriveAction::SteerLeft, keys) {
            input.steering -= 1.0;
        }
        if self.pressed(DriveAction::SteerRight, keys) {
            input.steering += 1.0;
        }
        if self.pressed(DriveAction::Handbrake, keys) {
            input.handbrake = 1.0;
        }

        for pad in pads {
            if self.apply_pad(&mut input, pad) {
                break;
            }
        }
        input
    }

    /// Fill the channels nothing else touched from the mouse, when
    /// mouse driving is on: the left button is full throttle, the right
    /// full brake (which reverses once stopped, like the brake key), and
    /// the cursor's offset steers through the stick's deadzone and
    /// sensitivity. A key or pad already holding a channel keeps it, so
    /// the mouse never fights the other devices. Steering inversion is
    /// the stick's setting and does not apply.
    pub fn apply_mouse(&self, input: &mut VehicleInput, mouse: MouseDrive) {
        if !self.mouse_driving {
            return;
        }
        if input.throttle == 0.0 && mouse.left {
            input.throttle = 1.0;
        }
        if input.brake == 0.0 && mouse.right {
            input.brake = 1.0;
        }
        if input.steering == 0.0
            && let Some(x) = mouse.offset
            && x.abs() > self.steer_deadzone
        {
            input.steering = (x * self.steer_sensitivity).clamp(-1.0, 1.0);
        }
    }

    /// Lay `pad`'s analog state over `input`; whether the pad is being
    /// used at all (past a deadzone, or the handbrake button held).
    fn apply_pad(&self, input: &mut VehicleInput, pad: &Gamepad) -> bool {
        let mut used = false;
        if let Some(x) = pad.get(GamepadAxis::LeftStickX)
            && x.abs() > self.steer_deadzone
        {
            let steer = (x * self.steer_sensitivity).clamp(-1.0, 1.0);
            input.steering = if self.invert_steering { -steer } else { steer };
            used = true;
        }
        if let Some(rt) = pad.get(GamepadButton::RightTrigger2)
            && rt > self.trigger_deadzone
        {
            input.throttle = rt;
            used = true;
        }
        if let Some(lt) = pad.get(GamepadButton::LeftTrigger2)
            && lt > self.trigger_deadzone
        {
            input.brake = lt;
            used = true;
        }
        if self
            .pad
            .button(PadAction::Handbrake)
            .is_some_and(|b| pad.pressed(b))
        {
            input.handbrake = 1.0;
            used = true;
        }
        used
    }
}

/// Deadzone values the Controls screen steps through (all inside
/// [`DEADZONE_RANGE`]).
pub const DEADZONE_STEPS: [f32; 6] = [0.0, 0.05, 0.10, 0.15, 0.20, 0.30];
/// Sensitivity values the Controls screen steps through (all inside
/// [`SENSITIVITY_RANGE`]).
pub const SENSITIVITY_STEPS: [f32; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];

/// The step after (or before) the one nearest `current`, wrapping — a
/// hand-edited off-grid value moves to a neighbouring step instead of
/// being stuck.
fn stepped(steps: &[f32], current: f32, forward: bool) -> f32 {
    let nearest = steps
        .iter()
        .enumerate()
        .min_by(|a, b| (a.1 - current).abs().total_cmp(&(b.1 - current).abs()))
        .map_or(0, |(i, _)| i);
    let next = if forward {
        (nearest + 1) % steps.len()
    } else {
        (nearest + steps.len() - 1) % steps.len()
    };
    steps[next]
}

/// The rebinding screen's editing and display helpers. Each `cycled_*`
/// returns a copy so the caller decides whether to adopt and save it.
impl ControlSettings {
    /// These controls with the stick deadzone stepped.
    pub fn cycled_steer_deadzone(&self, forward: bool) -> Self {
        Self {
            steer_deadzone: stepped(&DEADZONE_STEPS, self.steer_deadzone, forward),
            ..self.clone()
        }
    }

    /// These controls with the trigger deadzone stepped.
    pub fn cycled_trigger_deadzone(&self, forward: bool) -> Self {
        Self {
            trigger_deadzone: stepped(&DEADZONE_STEPS, self.trigger_deadzone, forward),
            ..self.clone()
        }
    }

    /// These controls with the steering sensitivity stepped.
    pub fn cycled_sensitivity(&self, forward: bool) -> Self {
        Self {
            steer_sensitivity: stepped(&SENSITIVITY_STEPS, self.steer_sensitivity, forward),
            ..self.clone()
        }
    }

    /// These controls with the transmission policy flipped.
    pub fn toggled_transmission(&self) -> Self {
        Self {
            transmission: self.transmission.toggled(),
            ..self.clone()
        }
    }

    /// These controls with mouse driving flipped.
    pub fn toggled_mouse_driving(&self) -> Self {
        Self {
            mouse_driving: !self.mouse_driving,
            ..self.clone()
        }
    }

    /// These controls with auto reverse flipped.
    pub fn toggled_auto_reverse(&self) -> Self {
        Self {
            auto_reverse: !self.auto_reverse,
            ..self.clone()
        }
    }

    /// These controls with the stick inversion flipped.
    pub fn toggled_inversion(&self) -> Self {
        Self {
            invert_steering: !self.invert_steering,
            ..self.clone()
        }
    }

    /// What a key slot shows: the key's name, or `-` when empty.
    pub fn slot_label(&self, action: DriveAction, slot: usize) -> &'static str {
        self.key_at(action, slot).and_then(key_name).unwrap_or("-")
    }

    /// These controls with `key` bound to `action`'s `slot`, and the
    /// status line to show. `Err` is the refusal (reserved, taken by
    /// another action...) worded for a screen that keeps listening.
    pub fn with_key(
        &self,
        action: DriveAction,
        slot: usize,
        key: KeyCode,
    ) -> Result<(Self, String), String> {
        let mut next = self.clone();
        match next.rebind(action, slot, key) {
            Ok(()) => {
                let line = format!(
                    "{} is now {}",
                    action.label(),
                    next.slot_label(action, slot)
                );
                Ok((next, line))
            }
            Err(e) => Err(format!(
                "{}: {e} - press another key (Esc cancels)",
                key_name(key).unwrap_or("that key")
            )),
        }
    }

    /// These controls with `action`'s `slot` cleared, and the status line
    /// to show; `Err` is the refusal (the last key stays).
    pub fn without_key(&self, action: DriveAction, slot: usize) -> Result<(Self, String), String> {
        let mut next = self.clone();
        match next.unbind(action, slot) {
            Ok(()) => Ok((next, format!("{} key {} cleared", action.label(), slot + 1))),
            Err(e) => Err(format!("{}: {e}", action.label())),
        }
    }

    /// The tuning rows every controls page ends with — stick and trigger
    /// deadzones, steering sensitivity, inversion, then a reset that
    /// disables itself, with its reason, at the shipped map. The main
    /// menu's Controls screen and the pause overlay's both list these, so
    /// the two cannot drift apart.
    pub fn tuning_rows(&self) -> Vec<ControlRow> {
        let on_off = |on: bool| if on { "On" } else { "Off" };
        let row = |text: String, item| ControlRow {
            text,
            item,
            enabled: Ok(()),
        };
        vec![
            row(
                format!("Transmission: {}", self.transmission.label()),
                ControlItem::Transmission,
            ),
            row(
                format!("Stick deadzone: {:.0}%", self.steer_deadzone * 100.0),
                ControlItem::SteerDeadzone,
            ),
            row(
                format!("Trigger deadzone: {:.0}%", self.trigger_deadzone * 100.0),
                ControlItem::TriggerDeadzone,
            ),
            row(
                format!("Steering sensitivity: {:.2}x", self.steer_sensitivity),
                ControlItem::Sensitivity,
            ),
            row(
                format!("Invert stick steering: {}", on_off(self.invert_steering)),
                ControlItem::InvertSteering,
            ),
            row(
                format!("Mouse driving: {}", on_off(self.mouse_driving)),
                ControlItem::MouseDriving,
            ),
            row(
                format!("Auto reverse: {}", on_off(self.auto_reverse)),
                ControlItem::AutoReverse,
            ),
            ControlRow {
                text: "Reset to defaults".to_string(),
                item: ControlItem::Reset,
                enabled: if *self == Self::default() {
                    Err("already at the defaults".to_string())
                } else {
                    Ok(())
                },
            },
        ]
    }

    /// These controls with a tuning `item` stepped (`forward` picks the
    /// direction; a toggle ignores it, reset restores the shipped map).
    /// `None` for a key row — those rebind by listening, not by stepping.
    pub fn adjusted(&self, item: ControlItem, forward: bool) -> Option<Self> {
        Some(match item {
            ControlItem::SteerDeadzone => self.cycled_steer_deadzone(forward),
            ControlItem::TriggerDeadzone => self.cycled_trigger_deadzone(forward),
            ControlItem::Sensitivity => self.cycled_sensitivity(forward),
            ControlItem::InvertSteering => self.toggled_inversion(),
            ControlItem::Transmission => self.toggled_transmission(),
            ControlItem::MouseDriving => self.toggled_mouse_driving(),
            ControlItem::AutoReverse => self.toggled_auto_reverse(),
            ControlItem::Reset => Self::default(),
            ControlItem::Key { .. } => return None,
        })
    }
}

/// The button `action` answers to under `controls`, or its shipped
/// button for a harness app that never inserted the settings. `None`
/// when the player cleared it.
pub fn pad_button(controls: Option<&ControlSettings>, action: PadAction) -> Option<GamepadButton> {
    match controls {
        Some(c) => c.pad.button(action),
        None => Some(action.default_button()),
    }
}

/// The keys `action` answers to under `controls` (primary first), or its
/// shipped keys for a harness app that never inserted the settings.
pub fn bound_keys(
    controls: Option<&ControlSettings>,
    action: DriveAction,
) -> [Option<KeyCode>; SLOTS] {
    match controls {
        Some(c) => c.bindings[action.index()],
        None => action.default_keys(),
    }
}

/// What a row of the gamepad-buttons page edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadItem {
    /// One action's button — rebinds by listening for a pad button.
    Button(PadAction),
    /// Restore the shipped button map (the keys and tuning stay).
    Reset,
}

/// A row of the gamepad-buttons page: its label, what it edits and why
/// it is disabled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PadRow {
    pub text: String,
    pub item: PadItem,
    pub enabled: Result<(), String>,
}

impl ControlSettings {
    /// These controls with `button` bound to the pad `action`, and the
    /// status line to show; `Err` is the refusal, worded for a page that
    /// keeps listening.
    pub fn with_pad_button(
        &self,
        action: PadAction,
        button: GamepadButton,
    ) -> Result<(Self, String), String> {
        let mut next = self.clone();
        match next.pad.bind(action, button) {
            Ok(()) => {
                let line = format!("{} is now {}", action.label(), next.pad.label(action));
                Ok((next, line))
            }
            Err(e) => Err(format!(
                "{}: {e} - press another button (Esc cancels)",
                button_name(button).unwrap_or("that button")
            )),
        }
    }

    /// These controls with the pad `action` cleared, and the status line.
    pub fn without_pad_button(&self, action: PadAction) -> Result<(Self, String), String> {
        let mut next = self.clone();
        match next.pad.unbind(action) {
            Ok(()) => Ok((next, format!("{} button cleared", action.label()))),
            Err(e) => Err(format!("{}: {e}", action.label())),
        }
    }

    /// These controls with the gamepad buttons back at the shipped map.
    pub fn with_default_pad(&self) -> Self {
        Self {
            pad: PadMap::default(),
            ..self.clone()
        }
    }

    /// The rows of the gamepad-buttons page: one per [`PadAction`], then
    /// a reset that disables itself, with its reason, at the shipped map.
    pub fn pad_rows(&self) -> Vec<PadRow> {
        let mut rows: Vec<PadRow> = PadAction::ALL
            .into_iter()
            .map(|a| PadRow {
                text: format!("{}: {}", a.label(), self.pad.label(a)),
                item: PadItem::Button(a),
                enabled: Ok(()),
            })
            .collect();
        rows.push(PadRow {
            text: "Reset gamepad buttons".to_string(),
            item: PadItem::Reset,
            enabled: if self.pad == PadMap::default() {
                Err("already at the shipped buttons".to_string())
            } else {
                Ok(())
            },
        });
        rows
    }
}

/// What a row of a driving-controls page edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlItem {
    /// One key slot of an action — rebinds by listening for a key.
    Key {
        action: DriveAction,
        slot: usize,
    },
    SteerDeadzone,
    TriggerDeadzone,
    Sensitivity,
    InvertSteering,
    Transmission,
    MouseDriving,
    AutoReverse,
    Reset,
}

/// A tuning row: its label, what it edits, and why it is disabled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlRow {
    pub text: String,
    pub item: ControlItem,
    pub enabled: Result<(), String>,
}

/// Where the running app saves its driving controls — `None` keeps them
/// for this run only (an evidence run). A resource so the pause overlay
/// can save a change the way the main menu does.
#[derive(Resource, Clone, Debug, Default)]
pub struct ControlsSave(pub Option<PathBuf>);

impl ControlsSave {
    /// Save `controls`. `Err` is a line for a status bar; the controls
    /// still apply for the run.
    pub fn save(&self, controls: &ControlSettings) -> Result<(), String> {
        let Some(path) = &self.0 else { return Ok(()) };
        controls.save(path).map_err(|e| {
            warn!(path = %path.display(), error = %e, "driving controls not saved");
            format!("controls not saved: {e}")
        })
    }
}

/// The on-disk shape. Every field is optional so a hand-written partial
/// file keeps the defaults for the rest.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct ControlsFile {
    bindings: BTreeMap<DriveAction, Vec<String>>,
    steer_deadzone: Option<f32>,
    trigger_deadzone: Option<f32>,
    steer_sensitivity: Option<f32>,
    invert_steering: Option<bool>,
    transmission: Option<String>,
    mouse_driving: Option<bool>,
    auto_reverse: Option<bool>,
    /// Gamepad buttons by action name; `null` is a cleared action. Keyed
    /// by string so an action this build does not know is dropped on its
    /// own rather than failing the whole file (and the key bindings in it).
    pad: BTreeMap<String, Option<String>>,
}

/// The file key of a pad action: its serde name.
fn pad_action_key(action: PadAction) -> String {
    serde_json::to_value(action)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Take `value` when it is finite and in `range`, else `default` plus a
/// line for the issue list.
fn checked(
    name: &str,
    value: Option<f32>,
    range: (f32, f32),
    default: f32,
    issues: &mut Vec<String>,
) -> f32 {
    match value {
        None => default,
        Some(v) if v.is_finite() && v >= range.0 && v <= range.1 => v,
        Some(v) => {
            issues.push(format!(
                "{name} {v} is outside {}…{}; using {default}",
                range.0, range.1
            ));
            default
        }
    }
}

impl ControlSettings {
    /// Parse and validate a controls file. `Err` only when the bytes are
    /// not the schema at all; every other fault is repaired to its
    /// default and described in the returned issue list.
    pub fn from_json(bytes: &[u8]) -> Result<(Self, Vec<String>), serde_json::Error> {
        let file: ControlsFile = serde_json::from_slice(bytes)?;
        let mut issues = Vec::new();
        let mut out = Self::default();

        for (action, names) in &file.bindings {
            match parse_slots(names) {
                Ok(keys) => out.bindings[action.index()] = keys,
                Err(why) => {
                    issues.push(format!("{}: {why}; using the default keys", action.label()))
                }
            }
        }
        // A key two actions claim is ambiguous: which control would it be?
        // Rejecting the whole binding set (not guessing a loser) keeps the
        // result deterministic and conflict-free.
        if !out.conflicts().is_empty() {
            for (key, a, b) in out.conflicts() {
                issues.push(format!(
                    "{} is bound to both {} and {}",
                    key_name(key).unwrap_or("?"),
                    a.label(),
                    b.label()
                ));
            }
            issues.push("using the default keys".into());
            out.bindings = Self::default().bindings;
        }

        for (action_name, name) in &file.pad {
            let Ok(action) = serde_json::from_value::<PadAction>(action_name.clone().into()) else {
                issues.push(format!(
                    "{action_name:?} is not a gamepad action; ignoring its button"
                ));
                continue;
            };
            match name.as_deref() {
                None => out.pad.set_raw(action, None),
                Some(name) => match button_from_name(name) {
                    Some(button) => out.pad.set_raw(action, Some(button)),
                    None => issues.push(format!(
                        "{} button {name:?} is not a bindable gamepad button; using {}",
                        action.label(),
                        button_name(action.default_button()).unwrap_or("its default")
                    )),
                },
            }
        }
        // As with the keys, a clash rejects the whole button map rather
        // than guessing which action loses.
        if !out.pad.conflicts().is_empty() {
            for (button, a, b) in out.pad.conflicts() {
                issues.push(format!(
                    "{} is bound to both {} and {}",
                    button_name(button).unwrap_or("?"),
                    a.label(),
                    b.label()
                ));
            }
            issues.push("using the default gamepad buttons".into());
            out.pad = PadMap::default();
        }

        let d = Self::default();
        out.steer_deadzone = checked(
            "steer_deadzone",
            file.steer_deadzone,
            DEADZONE_RANGE,
            d.steer_deadzone,
            &mut issues,
        );
        out.trigger_deadzone = checked(
            "trigger_deadzone",
            file.trigger_deadzone,
            DEADZONE_RANGE,
            d.trigger_deadzone,
            &mut issues,
        );
        out.steer_sensitivity = checked(
            "steer_sensitivity",
            file.steer_sensitivity,
            SENSITIVITY_RANGE,
            d.steer_sensitivity,
            &mut issues,
        );
        out.invert_steering = file.invert_steering.unwrap_or(d.invert_steering);
        out.mouse_driving = file.mouse_driving.unwrap_or(d.mouse_driving);
        out.auto_reverse = file.auto_reverse.unwrap_or(d.auto_reverse);
        out.transmission = match file.transmission.as_deref() {
            None => d.transmission,
            Some("automatic") => TransmissionPolicy::Automatic,
            Some("manual") => TransmissionPolicy::Manual,
            Some(other) => {
                issues.push(format!(
                    "transmission {other:?} is not automatic or manual; using automatic"
                ));
                d.transmission
            }
        };
        Ok((out, issues))
    }

    /// Read the controls file. A missing file is the first run and
    /// yields the defaults silently; an unreadable or unparseable one
    /// warns and yields the defaults, and invalid parts of an otherwise
    /// good file warn and fall back individually — a bad controls file
    /// must never leave the player without a working keyboard.
    pub fn load(path: &Path) -> Self {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                warn!(path = %path.display(), error = %e, "controls unreadable; using the defaults");
                return Self::default();
            }
        };
        match Self::from_json(&bytes) {
            Ok((settings, issues)) => {
                for issue in issues {
                    warn!(path = %path.display(), "controls: {issue}");
                }
                settings
            }
            Err(e) => {
                crate::settings::set_aside(path, &e.to_string(), "controls");
                Self::default()
            }
        }
    }

    /// Write the controls file atomically (see
    /// [`crate::settings::GraphicsSettings::save`]).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let file = ControlsFile {
            bindings: DriveAction::ALL
                .into_iter()
                .map(|a| {
                    let names = self
                        .keys(a)
                        .filter_map(key_name)
                        .map(str::to_owned)
                        .collect();
                    (a, names)
                })
                .collect(),
            steer_deadzone: Some(self.steer_deadzone),
            trigger_deadzone: Some(self.trigger_deadzone),
            steer_sensitivity: Some(self.steer_sensitivity),
            invert_steering: Some(self.invert_steering),
            mouse_driving: Some(self.mouse_driving),
            auto_reverse: Some(self.auto_reverse),
            pad: PadAction::ALL
                .into_iter()
                .map(|a| {
                    (
                        pad_action_key(a),
                        self.pad.button(a).and_then(button_name).map(str::to_owned),
                    )
                })
                .collect(),
            transmission: Some(
                match self.transmission {
                    TransmissionPolicy::Automatic => "automatic",
                    TransmissionPolicy::Manual => "manual",
                }
                .to_string(),
            ),
        };
        write_json_atomically(path, &file)
    }
}

/// One action's stored key names → its slots. Rejects an empty list, more
/// than [`SLOTS`] keys, an unknown/unbindable name, a reserved key or the
/// same key twice.
fn parse_slots(names: &[String]) -> Result<[Option<KeyCode>; SLOTS], String> {
    if names.is_empty() {
        return Err("no keys".into());
    }
    if names.len() > SLOTS {
        return Err(format!("{} keys (at most {SLOTS})", names.len()));
    }
    let mut slots = [None; SLOTS];
    for (slot, name) in names.iter().enumerate() {
        let key = key_from_name(name).ok_or_else(|| format!("unknown key {name:?}"))?;
        if RESERVED_KEYS.contains(&key) {
            return Err(format!("{name} is used by an in-game control"));
        }
        if slots.contains(&Some(key)) {
            return Err(format!("{name} listed twice"));
        }
        slots[slot] = Some(key);
    }
    Ok(slots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_map_is_valid_and_conflict_free() {
        let d = ControlSettings::default();
        assert!(d.conflicts().is_empty());
        for a in DriveAction::ALL {
            assert!(d.keys(a).count() >= 1, "{a:?} ships with a key");
            for k in d.keys(a) {
                assert!(key_name(k).is_some(), "{a:?} default {k:?} is bindable");
                assert!(!RESERVED_KEYS.contains(&k), "{a:?} default {k:?} reserved");
            }
        }
        // The default file round trips to itself.
        let dir = tempfile::tempdir().unwrap();
        let path = controls_path(dir.path());
        d.save(&path).unwrap();
        assert_eq!(ControlSettings::load(&path), d);
    }

    #[test]
    fn bindable_names_are_unique_and_every_reserved_key_is_bindable() {
        for (i, (n, k)) in BINDABLE.iter().enumerate() {
            assert_eq!(key_from_name(n), Some(*k));
            assert_eq!(key_name(*k), Some(*n));
            assert!(
                BINDABLE[i + 1..].iter().all(|(n2, k2)| n2 != n && k2 != k),
                "{n} listed twice"
            );
        }
        // A reserved key outside the table would be dead weight.
        for k in RESERVED_KEYS {
            assert!(key_name(*k).is_some(), "{k:?}");
        }
    }

    #[test]
    fn rebinding_names_the_conflicting_owner_and_never_steals() {
        let mut c = ControlSettings::default();
        assert_eq!(
            c.rebind(DriveAction::Throttle, 0, KeyCode::KeyS),
            Err(BindError::Conflict(DriveAction::Brake))
        );
        assert_eq!(c.key_at(DriveAction::Throttle, 0), Some(KeyCode::KeyW));
        assert_eq!(c.key_at(DriveAction::Brake, 0), Some(KeyCode::KeyS));
        // The alternate key of the same action is `AlreadyBound`.
        assert_eq!(
            c.rebind(DriveAction::Throttle, 0, KeyCode::ArrowUp),
            Err(BindError::AlreadyBound)
        );
        // Re-binding the key already in the slot is accepted unchanged.
        assert_eq!(c.rebind(DriveAction::Throttle, 0, KeyCode::KeyW), Ok(()));
        // Free the key on the brake first and the bind goes through.
        c.rebind(DriveAction::Brake, 0, KeyCode::KeyJ).unwrap();
        c.rebind(DriveAction::Throttle, 0, KeyCode::KeyS).unwrap();
        assert_eq!(c.owner_of(KeyCode::KeyS), Some(DriveAction::Throttle));
        assert_eq!(c.owner_of(KeyCode::KeyW), None);
        assert!(c.conflicts().is_empty());
    }

    #[test]
    fn rebinding_refuses_reserved_unbindable_and_missing_slots() {
        let mut c = ControlSettings::default();
        for key in RESERVED_KEYS {
            assert_eq!(
                c.rebind(DriveAction::Handbrake, 1, *key),
                Err(BindError::Reserved),
                "{key:?}"
            );
        }
        for key in [
            KeyCode::Escape,
            KeyCode::Delete,
            KeyCode::F1,
            KeyCode::Numpad4,
        ] {
            assert_eq!(
                c.rebind(DriveAction::Handbrake, 1, key),
                Err(BindError::NotBindable),
                "{key:?}"
            );
        }
        assert_eq!(
            c.rebind(DriveAction::Handbrake, SLOTS, KeyCode::KeyG),
            Err(BindError::NoSuchSlot)
        );
        assert_eq!(c, ControlSettings::default(), "refusals change nothing");
    }

    #[test]
    fn an_action_keeps_at_least_one_key() {
        let mut c = ControlSettings::default();
        c.unbind(DriveAction::Throttle, 1).unwrap();
        assert_eq!(c.keys(DriveAction::Throttle).count(), 1);
        assert_eq!(c.unbind(DriveAction::Throttle, 0), Err(BindError::LastKey));
        // An already-empty slot clears as a no-op.
        assert_eq!(c.unbind(DriveAction::Throttle, 1), Ok(()));
        assert_eq!(c.unbind(DriveAction::Handbrake, 0), Err(BindError::LastKey));
        assert_eq!(c.unbind(DriveAction::Handbrake, 1), Ok(()));
    }

    #[test]
    fn a_remapped_set_round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = controls_path(&dir.path().join("nested"));
        let mut c = ControlSettings::default();
        c.rebind(DriveAction::Throttle, 0, KeyCode::KeyU).unwrap();
        c.unbind(DriveAction::Throttle, 1).unwrap();
        c.rebind(DriveAction::Handbrake, 1, KeyCode::ShiftLeft)
            .unwrap();
        c.steer_deadzone = 0.2;
        c.trigger_deadzone = 0.1;
        c.steer_sensitivity = 1.5;
        c.invert_steering = true;
        c.save(&path).unwrap();
        assert_eq!(ControlSettings::load(&path), c);
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn the_transmission_policy_persists_and_a_bad_value_is_repaired() {
        let dir = tempfile::tempdir().unwrap();
        let path = controls_path(dir.path());
        assert_eq!(
            ControlSettings::default().transmission,
            TransmissionPolicy::Automatic
        );
        let mut c = ControlSettings::default().toggled_transmission();
        c.rebind(DriveAction::ShiftUp, 0, KeyCode::KeyU).unwrap();
        c.save(&path).unwrap();
        let back = ControlSettings::load(&path);
        assert_eq!(back.transmission, TransmissionPolicy::Manual);
        assert_eq!(back.key_at(DriveAction::ShiftUp, 0), Some(KeyCode::KeyU));

        let (c, issues) = ControlSettings::from_json(br#"{"transmission":"cvt"}"#).unwrap();
        assert_eq!(c.transmission, TransmissionPolicy::Automatic);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0].contains("cvt"));
    }

    #[test]
    fn a_pre_shift_file_keeps_loading_and_a_clash_with_a_new_default_resets_the_keys() {
        // An older file without the shift actions gets the shipped shifts.
        let (c, issues) =
            ControlSettings::from_json(br#"{"bindings":{"throttle":["KeyU"]}}"#).unwrap();
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(c.key_at(DriveAction::ShiftUp, 0), Some(KeyCode::KeyG));
        // One that already spent a shipped shift key elsewhere is a clash:
        // reported, and the keys fall back as a set rather than half-apply.
        let (c, issues) =
            ControlSettings::from_json(br#"{"bindings":{"handbrake":["KeyB"]}}"#).unwrap();
        assert_eq!(c, ControlSettings::default());
        assert!(issues.iter().any(|i| i.contains("KeyB is bound to both")));
    }

    #[test]
    fn shift_keys_are_edges_and_follow_their_binding() {
        let mut c = ControlSettings::default();
        c.rebind(DriveAction::ShiftUp, 1, KeyCode::KeyU).unwrap();
        let mut keys = ButtonInput::<KeyCode>::default();
        assert_eq!(c.shift_edges(&keys), (false, false));
        keys.press(KeyCode::KeyU);
        keys.press(KeyCode::KeyB);
        assert_eq!(c.shift_edges(&keys), (true, true));
        keys.clear();
        assert_eq!(c.shift_edges(&keys), (false, false), "held is not an edge");
    }

    #[test]
    fn the_transmission_row_toggles_and_enables_the_reset() {
        let c = ControlSettings::default();
        let rows = c.tuning_rows();
        assert_eq!(rows[0].item, ControlItem::Transmission);
        assert_eq!(rows[0].text, "Transmission: Automatic");
        let manual = c.adjusted(ControlItem::Transmission, true).unwrap();
        assert_eq!(manual.tuning_rows()[0].text, "Transmission: Manual");
        let reset = manual.tuning_rows().pop().unwrap();
        assert_eq!(reset.item, ControlItem::Reset);
        assert!(reset.enabled.is_ok(), "manual is a change from the default");
        assert_eq!(manual.adjusted(ControlItem::Reset, true).unwrap(), c);
        assert_eq!(
            manual.adjusted(ControlItem::Transmission, false).unwrap(),
            c
        );
    }

    #[test]
    fn a_missing_or_broken_file_loads_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = controls_path(dir.path());
        assert_eq!(ControlSettings::load(&path), ControlSettings::default());
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(ControlSettings::load(&path), ControlSettings::default());
        // An action name the schema does not know is not this schema.
        std::fs::write(&path, br#"{"bindings":{"warp":["KeyW"]}}"#).unwrap();
        assert_eq!(ControlSettings::load(&path), ControlSettings::default());
        // The unusable file was set aside, not left for the next save to
        // overwrite.
        assert!(!path.exists());
        assert_eq!(
            std::fs::read(path.with_extension("json.bad")).unwrap(),
            br#"{"bindings":{"warp":["KeyW"]}}"#
        );
    }

    #[test]
    fn a_partial_file_keeps_the_defaults_for_the_rest() {
        let (c, issues) = ControlSettings::from_json(
            br#"{"bindings":{"throttle":["KeyU"]},"invert_steering":true}"#,
        )
        .unwrap();
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(
            c.keys(DriveAction::Throttle).collect::<Vec<_>>(),
            [KeyCode::KeyU]
        );
        assert_eq!(
            c.keys(DriveAction::Brake).collect::<Vec<_>>(),
            [KeyCode::KeyS, KeyCode::ArrowDown]
        );
        assert!(c.invert_steering);
        assert_eq!(c.steer_deadzone, DEFAULT_DEADZONE);
    }

    #[test]
    fn invalid_pieces_fall_back_one_at_a_time_and_are_reported() {
        let (c, issues) = ControlSettings::from_json(
            br#"{"bindings":{
                "throttle":["Nope"],
                "brake":["KeyQ"],
                "steer_left":[],
                "steer_right":["KeyJ","KeyK","KeyN"],
                "handbrake":["KeyB","KeyB"]},
                "steer_deadzone":0.95,
                "trigger_deadzone":-0.1,
                "steer_sensitivity":99.0}"#,
        )
        .unwrap();
        let d = ControlSettings::default();
        assert_eq!(c, d, "every piece was invalid, so all of it is default");
        assert_eq!(issues.len(), 5 + 3, "{issues:?}");
    }

    #[test]
    fn a_key_two_actions_claim_rejects_the_whole_binding_set() {
        let (c, issues) = ControlSettings::from_json(
            br#"{"bindings":{"throttle":["KeyU"],"brake":["KeyU"],"handbrake":["KeyB"]}}"#,
        )
        .unwrap();
        assert_eq!(c, ControlSettings::default(), "no half-applied remap");
        assert!(issues.iter().any(|i| i.contains("KeyU is bound to both")));
        assert!(c.conflicts().is_empty());
    }

    #[test]
    fn non_finite_numbers_are_rejected() {
        let mut issues = Vec::new();
        for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                checked("x", Some(v), SENSITIVITY_RANGE, 1.0, &mut issues),
                1.0
            );
        }
        assert_eq!(issues.len(), 3);
        // The range ends themselves are valid.
        assert_eq!(
            checked("x", Some(0.25), SENSITIVITY_RANGE, 1.0, &mut issues),
            0.25
        );
        assert_eq!(
            checked("x", Some(2.0), SENSITIVITY_RANGE, 1.0, &mut issues),
            2.0
        );
        assert_eq!(issues.len(), 3);
        // A number too large for f32 is not the schema at all: the whole
        // file is refused and `load` falls back to the defaults.
        assert!(ControlSettings::from_json(br#"{"steer_sensitivity":1e999}"#).is_err());
    }

    #[test]
    fn tuning_steps_wrap_and_an_off_grid_value_moves_to_a_neighbour() {
        let mut c = ControlSettings::default();
        for _ in 0..DEADZONE_STEPS.len() {
            c = c.cycled_steer_deadzone(true);
        }
        assert_eq!(c.steer_deadzone, ControlSettings::default().steer_deadzone);
        assert_eq!(c.cycled_steer_deadzone(false).steer_deadzone, 0.0);
        assert_eq!(
            ControlSettings::default()
                .cycled_sensitivity(false)
                .steer_sensitivity,
            0.75
        );
        // A hand-edited 0.12 sits nearest 0.10, so forward is 0.15.
        let odd = ControlSettings {
            trigger_deadzone: 0.12,
            ..ControlSettings::default()
        };
        assert_eq!(odd.cycled_trigger_deadzone(true).trigger_deadzone, 0.15);
        for v in DEADZONE_STEPS {
            assert!((DEADZONE_RANGE.0..=DEADZONE_RANGE.1).contains(&v));
        }
        for v in SENSITIVITY_STEPS {
            assert!((SENSITIVITY_RANGE.0..=SENSITIVITY_RANGE.1).contains(&v));
        }
        assert!(
            ControlSettings::default()
                .toggled_inversion()
                .invert_steering
        );
    }

    #[test]
    fn with_key_and_without_key_return_the_copy_and_a_status_line() {
        let c = ControlSettings::default();
        let (next, line) = c.with_key(DriveAction::Throttle, 0, KeyCode::KeyT).unwrap();
        assert_eq!(next.key_at(DriveAction::Throttle, 0), Some(KeyCode::KeyT));
        assert_eq!(line, "Accelerate is now KeyT");
        assert_eq!(c, ControlSettings::default(), "the receiver is untouched");

        let refused = c
            .with_key(DriveAction::Throttle, 0, KeyCode::KeyS)
            .unwrap_err();
        assert!(refused.contains("Brake / reverse") && refused.contains("Esc cancels"));
        let reserved = c
            .with_key(DriveAction::Throttle, 0, KeyCode::KeyQ)
            .unwrap_err();
        assert!(reserved.contains("in-game control"));
        // A key the shipped map gives an in-game control names its owner
        // rather than reading as reserved.
        let owned = c
            .with_key(DriveAction::Throttle, 0, KeyCode::KeyR)
            .unwrap_err();
        assert!(owned.contains("Reset vehicle"), "{owned}");

        let (cleared, line) = c.without_key(DriveAction::Throttle, 1).unwrap();
        assert_eq!(cleared.key_at(DriveAction::Throttle, 1), None);
        assert_eq!(line, "Accelerate key 2 cleared");
        assert!(
            c.without_key(DriveAction::Handbrake, 0).is_err(),
            "the last key stays"
        );
    }

    #[test]
    fn tuning_rows_follow_the_values_and_adjusted_steps_them() {
        let c = ControlSettings::default();
        let rows = c.tuning_rows();
        let items: Vec<ControlItem> = rows.iter().map(|r| r.item).collect();
        assert_eq!(
            items,
            [
                ControlItem::Transmission,
                ControlItem::SteerDeadzone,
                ControlItem::TriggerDeadzone,
                ControlItem::Sensitivity,
                ControlItem::InvertSteering,
                ControlItem::MouseDriving,
                ControlItem::AutoReverse,
                ControlItem::Reset,
            ]
        );
        assert_eq!(rows[1].text, "Stick deadzone: 5%");
        assert!(
            rows[7].enabled.is_err(),
            "reset is disabled at the shipped map"
        );

        let tuned = c.adjusted(ControlItem::InvertSteering, true).unwrap();
        assert!(tuned.invert_steering);
        assert!(tuned.tuning_rows()[7].enabled.is_ok());
        assert_eq!(tuned.adjusted(ControlItem::Reset, true), Some(c.clone()));
        assert_eq!(
            c.adjusted(ControlItem::Sensitivity, true),
            Some(c.cycled_sensitivity(true))
        );
        assert_eq!(
            c.adjusted(
                ControlItem::Key {
                    action: DriveAction::Brake,
                    slot: 0
                },
                true
            ),
            None,
            "keys rebind by listening, not by stepping"
        );
    }

    fn mouse_on() -> ControlSettings {
        ControlSettings {
            mouse_driving: true,
            ..ControlSettings::default()
        }
    }

    #[test]
    fn mouse_driving_is_off_until_asked_for() {
        let mut input = VehicleInput::default();
        ControlSettings::default().apply_mouse(
            &mut input,
            MouseDrive {
                offset: Some(1.0),
                left: true,
                right: true,
            },
        );
        assert_eq!(
            (input.throttle, input.brake, input.steering),
            (0.0, 0.0, 0.0)
        );
    }

    #[test]
    fn the_mouse_buttons_throttle_and_brake_and_the_cursor_steers() {
        let c = mouse_on();
        let mut input = VehicleInput::default();
        c.apply_mouse(
            &mut input,
            MouseDrive {
                offset: Some(0.5),
                left: true,
                right: false,
            },
        );
        assert_eq!(
            (input.throttle, input.brake, input.steering),
            (1.0, 0.0, 0.5)
        );

        let mut input = VehicleInput::default();
        c.apply_mouse(
            &mut input,
            MouseDrive {
                offset: Some(-1.0),
                left: false,
                right: true,
            },
        );
        assert_eq!(
            (input.throttle, input.brake, input.steering),
            (0.0, 1.0, -1.0)
        );
    }

    #[test]
    fn the_mouse_respects_the_deadzone_and_sensitivity_and_ignores_inversion() {
        let mut c = mouse_on();
        c.steer_deadzone = 0.2;
        c.steer_sensitivity = 2.0;
        c.invert_steering = true;
        let steer = |c: &ControlSettings, x: f32| {
            let mut input = VehicleInput::default();
            c.apply_mouse(
                &mut input,
                MouseDrive {
                    offset: Some(x),
                    ..MouseDrive::default()
                },
            );
            input.steering
        };
        assert_eq!(steer(&c, 0.15), 0.0, "inside the deadzone");
        assert!((steer(&c, 0.25) - 0.5).abs() < 1e-6, "gain, not inverted");
        assert_eq!(steer(&c, 0.9), 1.0, "gain clamps at full lock");
    }

    #[test]
    fn the_mouse_never_overrides_a_channel_another_device_holds() {
        let c = mouse_on();
        let mut input = VehicleInput {
            throttle: 0.4,
            brake: 0.3,
            steering: -0.7,
            ..VehicleInput::default()
        };
        c.apply_mouse(
            &mut input,
            MouseDrive {
                offset: Some(0.9),
                left: true,
                right: true,
            },
        );
        assert_eq!(
            (input.throttle, input.brake, input.steering),
            (0.4, 0.3, -0.7)
        );
    }

    #[test]
    fn a_cursor_outside_the_window_steers_nothing() {
        let mut input = VehicleInput::default();
        mouse_on().apply_mouse(&mut input, MouseDrive::default());
        assert_eq!(
            (input.throttle, input.brake, input.steering),
            (0.0, 0.0, 0.0)
        );
    }

    #[test]
    fn mouse_driving_persists_and_an_older_file_keeps_it_off() {
        let dir = std::env::temp_dir().join(format!("mm2-mouse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CONTROLS_FILE);
        mouse_on().save(&path).unwrap();
        assert!(ControlSettings::load(&path).mouse_driving);
        std::fs::remove_dir_all(&dir).ok();

        let (c, issues) = ControlSettings::from_json(br#"{"steer_deadzone": 0.1}"#).unwrap();
        assert!(issues.is_empty(), "{issues:?}");
        assert!(!c.mouse_driving);
    }

    fn auto_reverse_off() -> ControlSettings {
        ControlSettings {
            auto_reverse: false,
            ..ControlSettings::default()
        }
    }

    fn brake(level: f32) -> VehicleInput {
        VehicleInput {
            brake: level,
            ..VehicleInput::default()
        }
    }

    #[test]
    fn auto_reverse_is_on_and_leaves_the_brake_alone() {
        let c = ControlSettings::default();
        assert!(c.auto_reverse, "the shipped car reverses off the brake");
        let mut carry = BrakeCarry::default();
        for speed in [12.0, 0.3, 0.0, -2.0] {
            let mut input = brake(1.0);
            carry.apply(&c, &mut input, Some(speed));
            assert_eq!((input.brake, input.handbrake), (1.0, 0.0), "at {speed}");
        }
    }

    #[test]
    fn a_brake_carried_through_the_stop_holds_instead_of_reversing() {
        let c = auto_reverse_off();
        let mut carry = BrakeCarry::default();
        let mut moving = brake(0.8);
        carry.apply(&c, &mut moving, Some(9.0));
        assert_eq!(
            (moving.brake, moving.handbrake),
            (0.8, 0.0),
            "still braking"
        );
        for speed in [0.4, 0.0, -0.1] {
            let mut input = brake(0.8);
            carry.apply(&c, &mut input, Some(speed));
            assert_eq!(input.brake, 0.0, "no reverse pedal at {speed}");
            assert_eq!(input.handbrake, 0.8, "held at {speed}");
        }
    }

    #[test]
    fn releasing_and_pressing_again_once_stopped_reverses() {
        let c = auto_reverse_off();
        let mut carry = BrakeCarry::default();
        let mut moving = brake(1.0);
        carry.apply(&c, &mut moving, Some(5.0));
        let mut held = brake(1.0);
        carry.apply(&c, &mut held, Some(0.0));
        assert_eq!(held.brake, 0.0);

        let mut released = VehicleInput::default();
        carry.apply(&c, &mut released, Some(0.0));
        assert_eq!(released.handbrake, 0.0, "letting go lets go");
        let mut fresh = brake(1.0);
        carry.apply(&c, &mut fresh, Some(0.0));
        assert_eq!((fresh.brake, fresh.handbrake), (1.0, 0.0), "reverse pedal");
        // ...and it stays the reverse pedal while the car backs away.
        let mut backing = brake(1.0);
        carry.apply(&c, &mut backing, Some(-3.0));
        assert_eq!((backing.brake, backing.handbrake), (1.0, 0.0));
    }

    #[test]
    fn the_stop_policy_keeps_a_stronger_handbrake_and_forgets_on_release() {
        let c = auto_reverse_off();
        let mut carry = BrakeCarry::default();
        carry.apply(&c, &mut brake(1.0), Some(6.0));
        let mut input = VehicleInput {
            brake: 0.5,
            handbrake: 1.0,
            ..VehicleInput::default()
        };
        carry.apply(&c, &mut input, Some(0.0));
        assert_eq!((input.brake, input.handbrake), (0.0, 1.0));

        carry.release();
        let mut after = brake(1.0);
        carry.apply(&c, &mut after, Some(0.0));
        assert_eq!(after.brake, 1.0, "a forgotten press is a fresh one");
    }

    #[test]
    fn a_car_without_sim_state_passes_through() {
        let c = auto_reverse_off();
        let mut carry = BrakeCarry::default();
        let mut input = brake(1.0);
        carry.apply(&c, &mut input, None);
        assert_eq!((input.brake, input.handbrake), (1.0, 0.0));
    }

    #[test]
    fn auto_reverse_persists_and_an_older_file_keeps_it_on() {
        let dir = std::env::temp_dir().join(format!("mm2-autorev-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CONTROLS_FILE);
        auto_reverse_off().save(&path).unwrap();
        assert!(!ControlSettings::load(&path).auto_reverse);
        std::fs::remove_dir_all(&dir).ok();

        let (c, issues) = ControlSettings::from_json(br#"{"steer_deadzone": 0.1}"#).unwrap();
        assert!(issues.is_empty(), "{issues:?}");
        assert!(c.auto_reverse);
    }

    #[test]
    fn the_auto_reverse_row_toggles_and_enables_reset() {
        let c = ControlSettings::default();
        let rows = c.tuning_rows();
        let row = rows
            .iter()
            .find(|r| r.item == ControlItem::AutoReverse)
            .expect("an auto reverse row");
        assert_eq!(row.text, "Auto reverse: On");
        let off = c.adjusted(ControlItem::AutoReverse, true).unwrap();
        assert!(!off.auto_reverse);
        assert!(
            off.tuning_rows()
                .iter()
                .any(|r| r.text == "Auto reverse: Off")
        );
        assert!(off.tuning_rows().last().unwrap().enabled.is_ok());
        assert_eq!(off.adjusted(ControlItem::Reset, true), Some(c));
    }

    #[test]
    fn the_mouse_row_toggles_and_enables_reset() {
        let c = ControlSettings::default();
        let rows = c.tuning_rows();
        let row = rows
            .iter()
            .find(|r| r.item == ControlItem::MouseDriving)
            .expect("a mouse row");
        assert_eq!(row.text, "Mouse driving: Off");
        let on = c.adjusted(ControlItem::MouseDriving, true).unwrap();
        assert!(on.mouse_driving);
        assert!(
            on.tuning_rows()
                .iter()
                .any(|r| r.text == "Mouse driving: On"),
            "the row reads the new state"
        );
        assert_eq!(on.adjusted(ControlItem::Reset, true), Some(c));
    }

    #[test]
    fn pad_buttons_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join(format!("mm2-pad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CONTROLS_FILE);
        let mut c = ControlSettings::default();
        c.pad.unbind(PadAction::Reset).unwrap();
        c.pad
            .bind(PadAction::Handbrake, GamepadButton::North)
            .unwrap();
        c.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\"reset\": null"),
            "a cleared action is null: {text}"
        );
        assert!(text.contains("\"handbrake\": \"North\""), "{text}");
        assert_eq!(ControlSettings::load(&path), c);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_without_pad_buttons_keeps_the_shipped_ones() {
        let (c, issues) = ControlSettings::from_json(br#"{"steer_deadzone": 0.1}"#).unwrap();
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(c.pad, PadMap::default());
    }

    #[test]
    fn an_unknown_pad_button_name_is_repaired_to_its_default() {
        let json = br#"{"pad": {"camera": "Triangle", "reset": null}}"#;
        let (c, issues) = ControlSettings::from_json(json).unwrap();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0].contains("Change camera") && issues[0].contains("Triangle"));
        assert_eq!(
            c.pad.button(PadAction::Camera),
            Some(PadAction::Camera.default_button())
        );
        assert_eq!(c.pad.button(PadAction::Reset), None, "null stays cleared");
    }

    #[test]
    fn an_unknown_pad_action_is_dropped_without_losing_the_keys() {
        let json = br#"{"bindings": {"handbrake": ["KeyU"]}, "pad": {"hnadbrake": "North", "camera": null}}"#;
        let (c, issues) = ControlSettings::from_json(json).unwrap();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0].contains("hnadbrake"));
        assert_eq!(c.pad.button(PadAction::Camera), None);
        assert_eq!(c.key_at(DriveAction::Handbrake, 0), Some(KeyCode::KeyU));
    }

    #[test]
    fn a_clashing_pad_map_is_rejected_whole() {
        // Handbrake and Reset both claim North: which one loses is not
        // ours to guess, so every button returns to the shipped map.
        let json = br#"{"pad": {"handbrake": "North", "mirror": "C"}}"#;
        let (c, issues) = ControlSettings::from_json(json).unwrap();
        assert!(issues.iter().any(|i| i.contains("North is bound to both")));
        assert!(issues.iter().any(|i| i.contains("default gamepad buttons")));
        assert_eq!(c.pad, PadMap::default(), "the valid C binding goes too");
    }

    #[test]
    fn a_pad_rebind_is_refused_with_the_owner_named_and_changes_nothing() {
        let c = ControlSettings::default();
        let err = c
            .with_pad_button(PadAction::Handbrake, GamepadButton::East)
            .unwrap_err();
        assert!(
            err.contains("East") && err.contains("Rear-view mirror"),
            "{err}"
        );
        let err = c
            .with_pad_button(PadAction::Handbrake, GamepadButton::Start)
            .unwrap_err();
        assert!(err.contains("cannot be used in-game"), "{err}");
        let (next, line) = c.without_pad_button(PadAction::Reset).unwrap();
        assert_eq!(line, "Reset vehicle button cleared");
        let (next, line) = next
            .with_pad_button(PadAction::Handbrake, GamepadButton::North)
            .unwrap();
        assert_eq!(line, "Handbrake is now North");
        assert_eq!(next.with_default_pad(), c);
    }

    #[test]
    fn the_pad_rows_list_every_action_and_reset_wakes_when_changed() {
        let c = ControlSettings::default();
        let rows = c.pad_rows();
        assert_eq!(rows.len(), PadAction::ALL.len() + 1);
        assert_eq!(rows[0].text, "Handbrake: South");
        assert!(rows.last().unwrap().enabled.is_err());
        let (c, _) = c.without_pad_button(PadAction::Horn).unwrap();
        let rows = c.pad_rows();
        assert_eq!(rows[7].text, "Horn / siren: -");
        assert!(rows.last().unwrap().enabled.is_ok());
    }

    #[test]
    fn the_handbrake_follows_its_pad_button() {
        let held = |button| {
            let mut pad = Gamepad::default();
            pad.digital_mut().press(button);
            pad
        };
        let keys = ButtonInput::<KeyCode>::default();
        let mut c = ControlSettings::default();
        let south = held(GamepadButton::South);
        assert_eq!(c.drive_input(&keys, [&south]).handbrake, 1.0);

        c.pad.unbind(PadAction::Reset).unwrap();
        c.pad
            .bind(PadAction::Handbrake, GamepadButton::North)
            .unwrap();
        let north = held(GamepadButton::North);
        assert_eq!(c.drive_input(&keys, [&north]).handbrake, 1.0);
        assert_eq!(c.drive_input(&keys, [&south]).handbrake, 0.0);

        c.pad.unbind(PadAction::Handbrake).unwrap();
        assert_eq!(c.drive_input(&keys, [&north]).handbrake, 0.0);
    }

    #[test]
    fn a_harness_without_settings_gets_the_shipped_button() {
        assert_eq!(
            pad_button(None, PadAction::Camera),
            Some(PadAction::Camera.default_button())
        );
        let mut c = ControlSettings::default();
        c.pad.unbind(PadAction::Camera).unwrap();
        assert_eq!(pad_button(Some(&c), PadAction::Camera), None);
    }

    #[test]
    fn the_action_lists_agree_and_the_shipped_keys_do_not_clash() {
        let joined: Vec<DriveAction> = DriveAction::DRIVING
            .into_iter()
            .chain(DriveAction::IN_GAME)
            .collect();
        assert_eq!(joined, DriveAction::ALL);
        let d = ControlSettings::default();
        assert!(d.conflicts().is_empty(), "{:?}", d.conflicts());
        for action in DriveAction::ALL {
            assert!(d.key_at(action, 0).is_some(), "{action:?} ships a key");
            for key in d.keys(action) {
                assert!(
                    key_name(key).is_some(),
                    "{action:?} ships an unbindable key"
                );
                assert!(
                    !RESERVED_KEYS.contains(&key),
                    "{action:?} ships a reserved key"
                );
            }
        }
        // The pad twins carry the same labels, so a player sees one name
        // for a control on both pages.
        for action in DriveAction::IN_GAME {
            match action.pad_twin() {
                Some(twin) => assert_eq!(action.label(), twin.label()),
                None => assert_eq!(action, DriveAction::Headlights),
            }
        }
    }

    #[test]
    fn an_in_game_key_rebinds_and_the_old_key_is_freed() {
        let mut c = ControlSettings::default();
        c.rebind(DriveAction::Camera, 0, KeyCode::KeyU).unwrap();
        assert_eq!(c.key_at(DriveAction::Camera, 0), Some(KeyCode::KeyU));
        // C is free again, and a driving action can take it.
        c.rebind(DriveAction::Handbrake, 1, KeyCode::KeyC).unwrap();
        // Tab, Enter and Backspace are bindable now that they ship bound.
        c.rebind(DriveAction::Camera, 1, KeyCode::Backspace)
            .unwrap_err();
        c.unbind(DriveAction::Mirror, 0).unwrap_err();
        c.rebind(DriveAction::Mirror, 1, KeyCode::KeyM).unwrap();
        c.unbind(DriveAction::Mirror, 0).unwrap();
        c.rebind(DriveAction::Camera, 1, KeyCode::Backspace)
            .unwrap();
        assert!(c.conflicts().is_empty());
    }

    #[test]
    fn a_key_in_use_names_its_owner_whichever_side_asks() {
        let c = ControlSettings::default();
        assert_eq!(
            c.check_bind(DriveAction::Camera, 1, KeyCode::KeyW),
            Err(BindError::Conflict(DriveAction::Throttle))
        );
        assert_eq!(
            c.check_bind(DriveAction::Throttle, 0, KeyCode::KeyC),
            Err(BindError::Conflict(DriveAction::Camera))
        );
        assert_eq!(
            c.check_bind(DriveAction::Handbrake, 1, KeyCode::Enter),
            Err(BindError::Conflict(DriveAction::Horn))
        );
        // The pause map's key stays fixed for every action.
        for action in DriveAction::ALL {
            assert_eq!(
                c.check_bind(action, 1, KeyCode::KeyQ),
                Err(BindError::Reserved)
            );
        }
        let msg = c
            .with_key(DriveAction::Camera, 1, KeyCode::KeyW)
            .unwrap_err();
        assert!(msg.contains("Accelerate"), "{msg}");
    }

    #[test]
    fn in_game_keys_persist_and_an_older_file_keeps_the_shipped_ones() {
        let dir = tempfile::tempdir().unwrap();
        let path = controls_path(dir.path());
        let mut c = ControlSettings::default();
        c.rebind(DriveAction::Camera, 0, KeyCode::KeyU).unwrap();
        c.rebind(DriveAction::Horn, 1, KeyCode::KeyJ).unwrap();
        c.rebind(DriveAction::Headlights, 0, KeyCode::Digit1)
            .unwrap();
        c.save(&path).unwrap();
        assert_eq!(ControlSettings::load(&path), c);

        let (older, issues) =
            ControlSettings::from_json(br#"{"bindings":{"throttle":["KeyU"]}}"#).unwrap();
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(older.key_at(DriveAction::Camera, 0), Some(KeyCode::KeyC));
        assert_eq!(
            older.key_at(DriveAction::Mirror, 0),
            Some(KeyCode::Backspace)
        );
    }

    #[test]
    fn a_file_binding_an_in_game_key_twice_resets_the_whole_key_set() {
        let (c, issues) =
            ControlSettings::from_json(br#"{"bindings":{"camera":["KeyW"],"throttle":["KeyW"]}}"#)
                .unwrap();
        assert_eq!(c, ControlSettings::default());
        assert!(issues.iter().any(|i| i.contains("KeyW is bound to both")));
        // An in-game action cannot be handed the reserved key, either.
        let (c, issues) =
            ControlSettings::from_json(br#"{"bindings":{"camera":["KeyQ"]}}"#).unwrap();
        assert_eq!(c.key_at(DriveAction::Camera, 0), Some(KeyCode::KeyC));
        assert_eq!(issues.len(), 1, "{issues:?}");
    }

    #[test]
    fn a_harness_without_settings_gets_the_shipped_in_game_keys() {
        assert_eq!(
            bound_keys(None, DriveAction::Camera),
            [Some(KeyCode::KeyC), None]
        );
        let mut c = ControlSettings::default();
        c.rebind(DriveAction::Camera, 0, KeyCode::KeyU).unwrap();
        assert_eq!(
            bound_keys(Some(&c), DriveAction::Camera),
            [Some(KeyCode::KeyU), None]
        );
    }
}
