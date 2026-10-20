//! The one development selector for the fixed-tick multiplayer timeline
//! (NETPLAY-02A, `docs/netplay-timeline.md`).
//!
//! NETPLAY-04..08 stage their incomplete behaviour behind this single
//! resource instead of separate overlapping switches; NETPLAY-09 enables
//! the completed path for ordinary host and join and then retires it. It
//! defaults **off**: with it off every session behaves exactly as before.

use bevy::prelude::*;

/// Environment variable that turns the timeline on (`1`/`true`/`on`).
pub const ENV_VAR: &str = "MM2_NETPLAY_TIMELINE";

/// Whether the fixed-tick command/ack timeline is active. Default off.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NetplayTimeline {
    /// True when the staged timeline path is selected.
    pub enabled: bool,
}

impl NetplayTimeline {
    /// Resolve the selector from the `--netplay-timeline` flag and the
    /// [`ENV_VAR`] value (`None` when unset). Either one enables it.
    pub fn resolve(flag: bool, env: Option<&str>) -> Self {
        let env_on = env.is_some_and(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "on" | "yes"
            )
        });
        Self {
            enabled: flag || env_on,
        }
    }

    /// [`Self::resolve`] reading the process environment.
    pub fn from_flag_and_env(flag: bool) -> Self {
        Self::resolve(flag, std::env::var(ENV_VAR).ok().as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_off() {
        assert!(!NetplayTimeline::default().enabled);
        assert!(!NetplayTimeline::resolve(false, None).enabled);
    }

    #[test]
    fn flag_or_env_enables() {
        assert!(NetplayTimeline::resolve(true, None).enabled);
        for on in ["1", "true", "ON", " yes "] {
            assert!(NetplayTimeline::resolve(false, Some(on)).enabled, "{on}");
        }
        for off in ["", "0", "false", "off", "2"] {
            assert!(!NetplayTimeline::resolve(false, Some(off)).enabled, "{off}");
        }
    }
}
