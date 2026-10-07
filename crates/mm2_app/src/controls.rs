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
//! Scope, on purpose: the five *driving* actions. The in-session
//! function keys (camera, mirror, map, reset…) stay on their documented
//! keys; [`RESERVED_KEYS`] keeps a driving action from being bound over
//! one of them. Menu navigation keeps its own keys. There is no
//! rebinding screen yet (F23-B) — the file is the edit surface, and
//! [`ControlSettings::rebind`] is the validated API that screen will call.
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

use crate::settings::write_json_atomically;

/// File name inside the settings directory.
pub const CONTROLS_FILE: &str = "controls.json";

/// The controls file inside `root` (the profile store's root).
pub fn controls_path(root: &Path) -> PathBuf {
    root.join(CONTROLS_FILE)
}

/// How many keys one action can hold (a primary and an alternate).
pub const SLOTS: usize = 2;

/// A rebindable driving action.
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
}

impl DriveAction {
    /// Every action, in the order a rebinding screen lists them.
    pub const ALL: [Self; 7] = [
        Self::Throttle,
        Self::Brake,
        Self::SteerLeft,
        Self::SteerRight,
        Self::Handbrake,
        Self::ShiftUp,
        Self::ShiftDown,
    ];

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
        }
    }
}

/// Every key a driving action may be bound to, with the name the file
/// stores. Modifiers other than `Shift`, function keys, `Enter`,
/// `Escape`, `Tab`, `Backspace` and the numpad are left out: the app's
/// own fixed controls use them.
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
    ("ShiftLeft", KeyCode::ShiftLeft),
    ("ShiftRight", KeyCode::ShiftRight),
];

/// Bindable keys the in-session controls already own: camera `C`, cockpit
/// `V`, reset `R`, map zoom `E` / rotate `F`, HUD `H`, indicators `I`,
/// nav-target `Z`/`X`, pause map `Q`, headlights `L`. A driving action
/// bound to one would fire the car and the control together.
pub const RESERVED_KEYS: &[KeyCode] = &[
    KeyCode::KeyC,
    KeyCode::KeyV,
    KeyCode::KeyR,
    KeyCode::KeyE,
    KeyCode::KeyF,
    KeyCode::KeyH,
    KeyCode::KeyI,
    KeyCode::KeyZ,
    KeyCode::KeyX,
    KeyCode::KeyQ,
    KeyCode::KeyL,
];

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
            Self::NotBindable => write!(f, "that key cannot be used for driving"),
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
        }
    }
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

    /// Lay `pad`'s analog state over `input`; whether the pad is being
    /// used at all (past a deadzone, or South held).
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
        if pad.pressed(GamepadButton::South) {
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
            ControlItem::Reset => Self::default(),
            ControlItem::Key { .. } => return None,
        })
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
                warn!(path = %path.display(), error = %e, "controls unparseable; using the defaults");
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
            KeyCode::Enter,
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
                "brake":["KeyR"],
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
            .with_key(DriveAction::Throttle, 0, KeyCode::KeyR)
            .unwrap_err();
        assert!(reserved.contains("in-game control"));

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
                ControlItem::Reset,
            ]
        );
        assert_eq!(rows[1].text, "Stick deadzone: 5%");
        assert!(
            rows[5].enabled.is_err(),
            "reset is disabled at the shipped map"
        );

        let tuned = c.adjusted(ControlItem::InvertSteering, true).unwrap();
        assert!(tuned.invert_steering);
        assert!(tuned.tuning_rows()[5].enabled.is_ok());
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
}
