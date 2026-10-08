//! The gamepad's button map — every digital in-session pad control
//! (F23-A.4, DSN-95, designed).
//!
//! [`crate::input::pad`] names the *shipped* buttons; [`PadMap`] is what
//! the player has made of them, owned by
//! [`crate::controls::ControlSettings`] and persisted in `controls.json`.
//! The analog controls (left stick steering, the two triggers as
//! throttle and brake) are not buttons and stay where they are, as do
//! the menu's navigation buttons.
//!
//! Unlike the keyboard map, the pad has no spare buttons: every face,
//! shoulder, stick-click, d-pad and `Select` button already carries a
//! control, so "refuse a taken button" alone would leave nothing to
//! rebind to. The rules are therefore: a button belongs to one action
//! (the refusal names its owner), an action can be cleared outright
//! (every pad action also has its key, so an unbound pad action is a
//! choice, not a dead control), and the one deliberate sharing is the
//! nav-arrow target buttons with the manual shift buttons — the target
//! action yields while the pad is shifting (see
//! [`PadMap::is_shift_button`]).

use std::fmt;

use bevy::prelude::GamepadButton;
use serde::{Deserialize, Serialize};

use crate::input::pad;

/// A digital in-session pad control.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PadAction {
    Handbrake,
    ShiftUp,
    ShiftDown,
    Camera,
    Cockpit,
    Mirror,
    Reset,
    Horn,
    MapView,
    MapZoom,
    MapRotate,
    Hud,
    Indicators,
    TargetPrev,
    TargetNext,
}

impl PadAction {
    /// Every action, in the order the rebinding page lists them.
    pub const ALL: [Self; 15] = [
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
    ];

    /// The row label.
    pub fn label(self) -> &'static str {
        match self {
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
        }
    }

    /// The shipped button ([`pad`] holds the documented table).
    pub fn default_button(self) -> GamepadButton {
        match self {
            Self::Handbrake => pad::HANDBRAKE,
            Self::ShiftUp => pad::SHIFT_UP,
            Self::ShiftDown => pad::SHIFT_DOWN,
            Self::Camera => pad::CAMERA,
            Self::Cockpit => pad::COCKPIT,
            Self::Mirror => pad::MIRROR,
            Self::Reset => pad::RESET,
            Self::Horn => pad::HORN,
            Self::MapView => pad::MAP_VIEW,
            Self::MapZoom => pad::MAP_ZOOM,
            Self::MapRotate => pad::MAP_ROTATE,
            Self::Hud => pad::HUD,
            Self::Indicators => pad::INDICATORS,
            Self::TargetPrev => pad::TARGET_PREV,
            Self::TargetNext => pad::TARGET_NEXT,
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|a| *a == self).unwrap_or(0)
    }

    fn is_shift(self) -> bool {
        matches!(self, Self::ShiftUp | Self::ShiftDown)
    }

    fn is_target(self) -> bool {
        matches!(self, Self::TargetPrev | Self::TargetNext)
    }

    /// Whether `self` and `other` may sit on one button: a nav-target
    /// action and a shift action, which never act together.
    fn may_share(self, other: Self) -> bool {
        (self.is_shift() && other.is_target()) || (self.is_target() && other.is_shift())
    }
}

/// Every button an action may take, with the name the file stores.
/// `Start` (pause), `Mode` and the two analog triggers (throttle and
/// brake) are left out: the app's own fixed controls use them.
pub const BUTTONS: &[(&str, GamepadButton)] = &[
    ("South", GamepadButton::South),
    ("East", GamepadButton::East),
    ("North", GamepadButton::North),
    ("West", GamepadButton::West),
    ("C", GamepadButton::C),
    ("Z", GamepadButton::Z),
    ("LeftBumper", GamepadButton::LeftTrigger),
    ("RightBumper", GamepadButton::RightTrigger),
    ("LeftStick", GamepadButton::LeftThumb),
    ("RightStick", GamepadButton::RightThumb),
    ("Select", GamepadButton::Select),
    ("DPadUp", GamepadButton::DPadUp),
    ("DPadDown", GamepadButton::DPadDown),
    ("DPadLeft", GamepadButton::DPadLeft),
    ("DPadRight", GamepadButton::DPadRight),
];

/// The file/UI name of a bindable button, `None` for any other.
pub fn button_name(button: GamepadButton) -> Option<&'static str> {
    BUTTONS.iter().find(|(_, b)| *b == button).map(|(n, _)| *n)
}

/// The bindable button a file name stands for.
pub fn button_from_name(name: &str) -> Option<GamepadButton> {
    BUTTONS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

/// Why a button cannot take an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadBindError {
    /// Not in [`BUTTONS`] — pause, the analog triggers, or a button the
    /// app cannot tell apart.
    NotBindable,
    /// Already bound to this other action — clear it there first.
    Conflict(PadAction),
    /// The action has no button to clear.
    NothingBound,
}

impl fmt::Display for PadBindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotBindable => write!(f, "that button cannot be used in-game"),
            Self::Conflict(a) => write!(f, "already bound to {}", a.label()),
            Self::NothingBound => write!(f, "no button is bound"),
        }
    }
}

/// The button (if any) each [`PadAction`] answers to. `Default` is the
/// shipped map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PadMap {
    buttons: [Option<GamepadButton>; PadAction::ALL.len()],
}

impl Default for PadMap {
    fn default() -> Self {
        Self {
            buttons: PadAction::ALL.map(|a| Some(a.default_button())),
        }
    }
}

impl PadMap {
    /// The button bound to `action`.
    pub fn button(&self, action: PadAction) -> Option<GamepadButton> {
        self.buttons[action.index()]
    }

    /// What a row shows: the button's name, or `-` when unbound.
    pub fn label(&self, action: PadAction) -> &'static str {
        self.button(action).and_then(button_name).unwrap_or("-")
    }

    /// The first action (other than `action`) that holds `button` and
    /// may not share it.
    fn blocking_owner(&self, action: PadAction, button: GamepadButton) -> Option<PadAction> {
        PadAction::ALL.into_iter().find(|other| {
            *other != action && self.button(*other) == Some(button) && !action.may_share(*other)
        })
    }

    /// Check `button` may go to `action` without changing anything.
    pub fn check_bind(&self, action: PadAction, button: GamepadButton) -> Result<(), PadBindError> {
        if button_name(button).is_none() {
            return Err(PadBindError::NotBindable);
        }
        match self.blocking_owner(action, button) {
            Some(owner) => Err(PadBindError::Conflict(owner)),
            None => Ok(()),
        }
    }

    /// Bind `button` to `action`, replacing what it had. A button
    /// another action holds is refused naming the owner; nothing is
    /// silently stolen.
    pub fn bind(&mut self, action: PadAction, button: GamepadButton) -> Result<(), PadBindError> {
        self.check_bind(action, button)?;
        self.buttons[action.index()] = Some(button);
        Ok(())
    }

    /// Leave `action` without a pad button.
    pub fn unbind(&mut self, action: PadAction) -> Result<(), PadBindError> {
        if self.button(action).is_none() {
            return Err(PadBindError::NothingBound);
        }
        self.buttons[action.index()] = None;
        Ok(())
    }

    /// Set `action`'s button straight from a stored value (`None` =
    /// unbound), skipping the conflict check — the loader validates the
    /// whole map afterwards with [`Self::conflicts`].
    pub(crate) fn set_raw(&mut self, action: PadAction, button: Option<GamepadButton>) {
        self.buttons[action.index()] = button;
    }

    /// Every button two actions that may not share claim.
    pub fn conflicts(&self) -> Vec<(GamepadButton, PadAction, PadAction)> {
        let mut out = Vec::new();
        for (i, a) in PadAction::ALL.iter().enumerate() {
            for b in &PadAction::ALL[i + 1..] {
                if let Some(button) = self.button(*a)
                    && self.button(*b) == Some(button)
                    && !a.may_share(*b)
                {
                    out.push((button, *a, *b));
                }
            }
        }
        out
    }

    /// Whether `button` is bound to a manual shift — the buttons the
    /// nav-target actions yield while the pad is shifting.
    pub fn is_shift_button(&self, button: GamepadButton) -> bool {
        PadAction::ALL
            .into_iter()
            .any(|a| a.is_shift() && self.button(a) == Some(button))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_map_is_the_documented_one_and_conflict_free() {
        let map = PadMap::default();
        assert!(map.conflicts().is_empty());
        for a in PadAction::ALL {
            assert_eq!(map.button(a), Some(a.default_button()));
            assert!(
                button_name(a.default_button()).is_some(),
                "{a:?}'s shipped button must be a bindable one"
            );
        }
    }

    #[test]
    fn button_names_round_trip_and_are_unique() {
        for (name, button) in BUTTONS {
            assert_eq!(button_from_name(name), Some(*button));
            assert_eq!(button_name(*button), Some(*name));
        }
        let mut names: Vec<_> = BUTTONS.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), BUTTONS.len());
        for fixed in [
            GamepadButton::Start,
            GamepadButton::Mode,
            GamepadButton::LeftTrigger2,
            GamepadButton::RightTrigger2,
        ] {
            assert_eq!(button_name(fixed), None, "{fixed:?} is app-owned");
        }
    }

    #[test]
    fn a_taken_button_is_refused_naming_its_owner() {
        let mut map = PadMap::default();
        assert_eq!(
            map.bind(PadAction::Handbrake, GamepadButton::North),
            Err(PadBindError::Conflict(PadAction::Reset))
        );
        assert_eq!(map.button(PadAction::Handbrake), Some(GamepadButton::South));
        // Free the button, and the same bind goes through.
        map.unbind(PadAction::Reset).unwrap();
        map.bind(PadAction::Handbrake, GamepadButton::North)
            .unwrap();
        assert_eq!(map.button(PadAction::Handbrake), Some(GamepadButton::North));
        assert_eq!(map.label(PadAction::Reset), "-");
        assert_eq!(map.label(PadAction::Handbrake), "North");
    }

    #[test]
    fn reserved_buttons_cannot_be_bound() {
        let mut map = PadMap::default();
        for fixed in [GamepadButton::Start, GamepadButton::RightTrigger2] {
            assert_eq!(
                map.bind(PadAction::Camera, fixed),
                Err(PadBindError::NotBindable)
            );
        }
    }

    #[test]
    fn re_binding_an_actions_own_button_is_a_no_op() {
        let mut map = PadMap::default();
        assert_eq!(map.bind(PadAction::Camera, pad::CAMERA), Ok(()));
        assert_eq!(map, PadMap::default());
    }

    #[test]
    fn only_a_target_and_a_shift_may_share_a_button() {
        let map = PadMap::default();
        // Shipped: the bumpers carry both a shift and a target action.
        assert_eq!(
            map.button(PadAction::ShiftUp),
            map.button(PadAction::TargetNext)
        );
        assert!(map.is_shift_button(GamepadButton::RightTrigger));
        assert!(!map.is_shift_button(GamepadButton::South));
        // A target action may also move onto a shift's button...
        let mut m = PadMap::default();
        m.unbind(PadAction::TargetPrev).unwrap();
        m.unbind(PadAction::TargetNext).unwrap();
        m.bind(PadAction::TargetPrev, GamepadButton::RightTrigger)
            .unwrap();
        // ...but two targets never share, and neither does a target with
        // an ordinary action's button.
        assert_eq!(
            m.bind(PadAction::TargetNext, GamepadButton::RightTrigger),
            Err(PadBindError::Conflict(PadAction::TargetPrev))
        );
        assert_eq!(
            m.bind(PadAction::TargetPrev, GamepadButton::West),
            Err(PadBindError::Conflict(PadAction::Cockpit))
        );
        // And two ordinary actions never share, even via a shift button.
        assert_eq!(
            m.bind(PadAction::Camera, GamepadButton::RightTrigger),
            Err(PadBindError::Conflict(PadAction::ShiftUp))
        );
    }

    #[test]
    fn clearing_an_unbound_action_says_so() {
        let mut map = PadMap::default();
        map.unbind(PadAction::Horn).unwrap();
        assert_eq!(map.unbind(PadAction::Horn), Err(PadBindError::NothingBound));
        assert_eq!(map.button(PadAction::Horn), None);
    }

    #[test]
    fn conflicts_lists_a_hand_made_clash() {
        let mut map = PadMap::default();
        map.set_raw(PadAction::Camera, Some(GamepadButton::South));
        assert_eq!(
            map.conflicts(),
            vec![(
                GamepadButton::South,
                PadAction::Handbrake,
                PadAction::Camera
            )]
        );
    }
}
