//! Input-device capability record and pad connection log (F23-AC06).
//!
//! F23 req 5 asks for wheel / force-feedback support to be audited "by
//! actual platform/device capability" and reported honestly. This module
//! is that report as data: [`RECORDS`] names every device capability the
//! game has an opinion on and says whether it exists and what evidence
//! backs it. Nothing here claims a physical device was ever driven — the
//! only evidence level any record reaches is [`Status::SyntheticOnly`]
//! (raw events fed through the production systems), and everything the
//! game does not do is [`Status::NotImplemented`] rather than omitted, so
//! the denominator stays visible. `docs/research/input-devices.md` carries
//! the same table with the test that backs each row; a unit test keeps
//! the two in step.
//!
//! [`log_input_capabilities`] writes the table to the log at startup and
//! [`log_pad_connections`] names each pad as it comes and goes, so a run's
//! log says which controllers the OS reported — the one fact a wheel
//! report needs and the code cannot assume.

use std::collections::HashMap;

use bevy::input::gamepad::{GamepadConnection, GamepadConnectionEvent};
use bevy::prelude::*;
use tracing::info;

/// One device-level capability the game reports on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    KeyboardDriving,
    KeyRebinding,
    GamepadDriving,
    GamepadMenus,
    GamepadHotplug,
    FocusLossRelease,
    PadRebinding,
    MouseDriving,
    ManualTransmission,
    SteeringWheel,
    ForceFeedback,
}

impl Capability {
    /// Every capability, in report order.
    pub const ALL: [Self; 11] = [
        Self::KeyboardDriving,
        Self::KeyRebinding,
        Self::GamepadDriving,
        Self::GamepadMenus,
        Self::GamepadHotplug,
        Self::FocusLossRelease,
        Self::PadRebinding,
        Self::MouseDriving,
        Self::ManualTransmission,
        Self::SteeringWheel,
        Self::ForceFeedback,
    ];

    /// The name the report and the doc table use.
    pub fn label(self) -> &'static str {
        match self {
            Self::KeyboardDriving => "keyboard driving",
            Self::KeyRebinding => "key rebinding",
            Self::GamepadDriving => "gamepad driving",
            Self::GamepadMenus => "gamepad menus",
            Self::GamepadHotplug => "gamepad hot-plug",
            Self::FocusLossRelease => "focus-loss release",
            Self::PadRebinding => "pad rebinding",
            Self::MouseDriving => "mouse driving",
            Self::ManualTransmission => "manual transmission",
            Self::SteeringWheel => "steering wheel",
            Self::ForceFeedback => "force feedback",
        }
    }
}

/// How far a capability has been shown to work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The code does not do this.
    NotImplemented,
    /// Implemented and exercised only by synthetic events through the
    /// production systems — no physical device or real window.
    SyntheticOnly,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::NotImplemented => "not implemented",
            Self::SyntheticOnly => "synthetic only",
        }
    }
}

/// A capability's status plus the scope of what that status means.
#[derive(Debug, Clone, Copy)]
pub struct CapabilityRecord {
    pub capability: Capability,
    pub status: Status,
    /// What is and is not established, in a sentence.
    pub note: &'static str,
}

/// The record, one entry per [`Capability`] in [`Capability::ALL`] order.
pub const RECORDS: [CapabilityRecord; 11] = [
    CapabilityRecord {
        capability: Capability::KeyboardDriving,
        status: Status::SyntheticOnly,
        note: "bound keys drive the normalized input; no real keyboard session recorded",
    },
    CapabilityRecord {
        capability: Capability::KeyRebinding,
        status: Status::SyntheticOnly,
        note: "main-menu and pause Controls pages persist controls.json; driven by synthetic key events",
    },
    CapabilityRecord {
        capability: Capability::GamepadDriving,
        status: Status::SyntheticOnly,
        note: "stick, triggers and South through the deadzone/sensitivity map; any physical pad is untested",
    },
    CapabilityRecord {
        capability: Capability::GamepadMenus,
        status: Status::SyntheticOnly,
        note: "D-pad, stick edges, South/East/West/Start over every connected pad; synthetic events only",
    },
    CapabilityRecord {
        capability: Capability::GamepadHotplug,
        status: Status::SyntheticOnly,
        note: "connection and disconnection events clear and restore driving; no physical unplug performed",
    },
    CapabilityRecord {
        capability: Capability::FocusLossRelease,
        status: Status::SyntheticOnly,
        note: "KeyboardFocusLost and unfocused windows release held input; no real window focus change",
    },
    CapabilityRecord {
        capability: Capability::PadRebinding,
        status: Status::SyntheticOnly,
        note: "every digital in-session pad button (handbrake, manual shifts, camera, mirror, map, HUD...) rebinds on the main-menu and pause Gamepad buttons pages and persists in controls.json; stick and triggers stay analog, Start/Mode are app-owned; synthetic pad events only",
    },
    CapabilityRecord {
        capability: Capability::MouseDriving,
        status: Status::NotImplemented,
        note: "the original's mouse steering and button throttle (CTL-2) is not built; mouse is menu-only",
    },
    CapabilityRecord {
        capability: Capability::ManualTransmission,
        status: Status::SyntheticOnly,
        note: "a Controls-page policy pins the gearbox through the shift keys (G/B) or the pad shoulders (right up, left down; they stop cycling the nav-arrow target while manual), inert on a predicted multiplayer client; the brake pedal still doubles as reverse once stopped",
    },
    CapabilityRecord {
        capability: Capability::SteeringWheel,
        status: Status::NotImplemented,
        note: "no wheel-specific mapping; a wheel the OS exposes as a gamepad would be read as a pad, axis layout untested and no wheel hardware available",
    },
    CapabilityRecord {
        capability: Capability::ForceFeedback,
        status: Status::NotImplemented,
        note: "no rumble or force-feedback requests are sent to any device; untested on hardware",
    },
];

impl CapabilityRecord {
    /// `label: status — note`, the form the log and the doc table share.
    pub fn line(&self) -> String {
        format!(
            "{}: {} — {}",
            self.capability.label(),
            self.status.label(),
            self.note
        )
    }
}

/// One line for a freshly reported pad.
pub fn connected_line(name: &str, vendor_id: Option<u16>, product_id: Option<u16>) -> String {
    let id = match (vendor_id, product_id) {
        (Some(v), Some(p)) => format!("{v:04x}:{p:04x}"),
        _ => "id unknown".to_string(),
    };
    format!(
        "gamepad connected: {name} ({id}); read as a standard pad — wheel layout and force feedback unsupported"
    )
}

/// `Startup`: write the capability record to the log.
pub fn log_input_capabilities() {
    for record in &RECORDS {
        info!("input capability — {}", record.line());
    }
}

/// The log line for one connection event, remembering the connected
/// names by entity because the disconnect event carries none.
pub fn describe_event(
    names: &mut HashMap<Entity, String>,
    event: &GamepadConnectionEvent,
) -> String {
    match &event.connection {
        GamepadConnection::Connected {
            name,
            vendor_id,
            product_id,
        } => {
            names.insert(event.gamepad, name.clone());
            connected_line(name, *vendor_id, *product_id)
        }
        GamepadConnection::Disconnected => {
            let name = names.remove(&event.gamepad);
            format!(
                "gamepad disconnected: {}",
                name.as_deref().unwrap_or("unknown pad")
            )
        }
    }
}

/// `Update`: name each pad the OS reports and note its removal.
pub fn log_pad_connections(
    mut events: MessageReader<GamepadConnectionEvent>,
    mut names: Local<HashMap<Entity, String>>,
) {
    for event in events.read() {
        info!("{}", describe_event(&mut names, event));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_cover_every_capability_once_in_order() {
        assert_eq!(RECORDS.len(), Capability::ALL.len());
        for (record, capability) in RECORDS.iter().zip(Capability::ALL) {
            assert_eq!(record.capability, capability);
        }
        let mut labels: Vec<_> = Capability::ALL.iter().map(|c| c.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), Capability::ALL.len(), "labels are distinct");
    }

    #[test]
    fn wheel_and_force_feedback_are_never_claimed() {
        for capability in [Capability::SteeringWheel, Capability::ForceFeedback] {
            let record = RECORDS.iter().find(|r| r.capability == capability).unwrap();
            assert_eq!(record.status, Status::NotImplemented);
            assert!(!record.note.is_empty());
        }
    }

    #[test]
    fn the_doc_table_lists_every_record_line() {
        let doc = include_str!("../../../docs/research/input-devices.md");
        for record in &RECORDS {
            assert!(
                doc.contains(&record.line()),
                "docs/research/input-devices.md is missing: {}",
                record.line()
            );
        }
    }

    #[test]
    fn connected_line_names_the_pad_and_its_ids() {
        let line = connected_line("Test Pad", Some(0x045e), Some(0x028e));
        assert!(line.contains("Test Pad"));
        assert!(line.contains("045e:028e"));
        assert!(connected_line("X", None, Some(1)).contains("id unknown"));
    }

    #[test]
    fn a_disconnect_is_named_after_its_connect() {
        let pad = Entity::from_raw_u32(7).unwrap();
        let other = Entity::from_raw_u32(8).unwrap();
        let mut names = HashMap::new();
        let connect = |gamepad| {
            GamepadConnectionEvent::new(
                gamepad,
                GamepadConnection::Connected {
                    name: "Test Pad".into(),
                    vendor_id: None,
                    product_id: None,
                },
            )
        };
        let gone = |gamepad| GamepadConnectionEvent::new(gamepad, GamepadConnection::Disconnected);
        assert!(describe_event(&mut names, &connect(pad)).contains("Test Pad"));
        assert!(describe_event(&mut names, &gone(other)).contains("unknown pad"));
        assert!(describe_event(&mut names, &gone(pad)).contains("Test Pad"));
        // The name is forgotten once the pad is gone.
        assert!(describe_event(&mut names, &gone(pad)).contains("unknown pad"));
    }
}
