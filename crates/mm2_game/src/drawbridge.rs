//! Drawbridge leaves — the retail `gizBridge` behaviour.
//!
//! London's Tower Bridge, both Waterloo bascules and the St Katharine
//! dock bridge, and SF's Chinatown gate, are not PSDL geometry: the
//! PSDL rooms under them carry their road behind a null texture
//! reference (no render, no collision), and the decks come from
//! `race/<city>/<city>_bridge[_<event>].pathset` through the original's
//! `gizBridgeMgr`. Everything below was recovered from the retail
//! executable (`Midtown2.exe`, see `docs/research/drawbridge.md`):
//!
//! - every path places one leaf at its first point facing its third
//!   (its second on a two-point path) and, on paths of three or more
//!   points, a partner leaf at the third point facing back — the two
//!   meet over the middle;
//! - the leaf's mode comes from the first four characters of the path
//!   name, compared case-insensitively: `inac` → [`DrawbridgeMode::Inactive`],
//!   `prox` → [`DrawbridgeMode::Proximity`], `time` →
//!   [`DrawbridgeMode::Timed`], `open` → [`DrawbridgeMode::Open`];
//!   anything else keeps the constructor's default, `Timed`;
//! - the leaf rotates about its hinge at [`RAISE_RATE`] rad/s between
//!   0 and [`OPEN_ANGLE`]; a timed leaf waits [`CLOSED_WAIT`] seconds
//!   closed and holds [`RAISED_HOLD`] seconds raised, an open leaf
//!   resets raised and never moves, a proximity leaf opens when a car
//!   comes within [`PROXIMITY_RADIUS`] and takes its partner with it.

/// How a leaf is driven, decoded from its path-name prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawbridgeMode {
    /// `inac…` — stays closed.
    Inactive,
    /// `prox…` — opens when a car comes near (no retail path uses it).
    Proximity,
    /// `time…`, and every unprefixed name — cycles on a timer.
    Timed,
    /// `open…` — held raised.
    Open,
}

impl DrawbridgeMode {
    /// Decode a raw path name (`OPEN:giz_bridge01_l`,
    /// `inactive:giz_chinagate_f`, `giz_waterloo_l`). Only the first
    /// four characters are compared, case-insensitively, exactly as the
    /// original's `strncpy(…, 4)` + `stricmp` does.
    pub fn from_path_name(name: &str) -> Self {
        let head = name.get(..4).unwrap_or(name);
        if head.eq_ignore_ascii_case("inac") {
            Self::Inactive
        } else if head.eq_ignore_ascii_case("prox") {
            Self::Proximity
        } else if head.eq_ignore_ascii_case("time") {
            Self::Timed
        } else if head.eq_ignore_ascii_case("open") {
            Self::Open
        } else {
            Self::Timed
        }
    }
}

/// Raised angle, radians (≈27°) — retail constant `0.4712389`.
pub const OPEN_ANGLE: f32 = 0.471_238_9;
/// Hinge rotation speed while opening or closing, rad/s.
pub const RAISE_RATE: f32 = 0.05;
/// Seconds a timed leaf rests closed before it opens.
pub const CLOSED_WAIT: f32 = 10.0;
/// Seconds a leaf holds fully raised before it closes.
pub const RAISED_HOLD: f32 = 10.0;
/// A proximity leaf opens when a car is within this many metres.
pub const PROXIMITY_RADIUS: f32 = 100.0;
/// Every leaf is drawn this far below its authored hinge point —
/// the manager's global `(0, -0.3, 0)` offset. It puts Tower Bridge's
/// deck (0.15 above the leaf origin) at 7.85 against its 7.8 approach.
pub const LEAF_DROP: f32 = 0.3;

/// Where a leaf is in its cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafPhase {
    /// Down (or, for an `Open` leaf, resting raised).
    Resting,
    /// Rising towards [`OPEN_ANGLE`].
    Opening,
    /// Fully raised, counting down [`RAISED_HOLD`].
    Raised,
    /// Lowering towards 0.
    Closing,
}

/// One leaf's animation state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeafMotion {
    /// The leaf's mode.
    pub mode: DrawbridgeMode,
    /// Its phase.
    pub phase: LeafPhase,
    /// Seconds spent in the current resting/raised phase.
    pub timer: f32,
    /// Hinge angle, radians: 0 closed, positive lifts the free end.
    pub angle: f32,
}

impl LeafMotion {
    /// A leaf as the original resets it: resting, timer cleared,
    /// raised only in `Open` mode.
    pub fn new(mode: DrawbridgeMode) -> Self {
        Self {
            mode,
            phase: LeafPhase::Resting,
            timer: 0.0,
            angle: if mode == DrawbridgeMode::Open {
                OPEN_ANGLE
            } else {
                0.0
            },
        }
    }

    /// Start a resting proximity leaf opening. Returns whether it
    /// started — a leaf already in motion ignores the trigger, so the
    /// caller propagates to the partner only on a real start.
    pub fn trigger(&mut self) -> bool {
        if self.phase != LeafPhase::Resting {
            return false;
        }
        self.phase = LeafPhase::Opening;
        self.timer = 0.0;
        true
    }

    /// Advance by `dt` seconds.
    pub fn step(&mut self, dt: f32) {
        match self.phase {
            LeafPhase::Resting => {
                if self.mode == DrawbridgeMode::Timed {
                    self.timer += dt;
                    if self.timer > CLOSED_WAIT {
                        self.timer = 0.0;
                        self.phase = LeafPhase::Opening;
                    }
                }
            }
            LeafPhase::Raised => {
                self.timer += dt;
                if self.timer > RAISED_HOLD {
                    self.timer = 0.0;
                    self.phase = LeafPhase::Closing;
                }
            }
            LeafPhase::Opening => {
                self.angle = (self.angle + RAISE_RATE * dt).min(OPEN_ANGLE);
                if self.angle >= OPEN_ANGLE {
                    self.timer = 0.0;
                    self.phase = LeafPhase::Raised;
                }
            }
            LeafPhase::Closing => {
                self.angle = (self.angle - RAISE_RATE * dt).max(0.0);
                if self.angle <= 0.0 {
                    self.timer = 0.0;
                    self.phase = LeafPhase::Resting;
                }
            }
        }
    }
}

/// The leaves one bridge path places, as `(hinge, toward)` point
/// pairs: the first point facing the third (the second on a
/// two-point path), plus — on three or more points — the third facing
/// the first. A path of fewer than two points places nothing.
pub fn leaf_hinges(points: &[[f32; 3]]) -> Vec<([f32; 3], [f32; 3])> {
    match points.len() {
        0 | 1 => Vec::new(),
        2 => vec![(points[0], points[1])],
        _ => vec![(points[0], points[2]), (points[2], points[0])],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_prefixes_decode_case_insensitively() {
        use DrawbridgeMode::*;
        assert_eq!(DrawbridgeMode::from_path_name("OPEN:giz_bridge01_l"), Open);
        assert_eq!(DrawbridgeMode::from_path_name("open:giz_bridge02_l"), Open);
        assert_eq!(
            DrawbridgeMode::from_path_name("inactive:giz_chinagate_f"),
            Inactive
        );
        assert_eq!(
            DrawbridgeMode::from_path_name("prox:giz_waterloo_l"),
            Proximity
        );
        assert_eq!(DrawbridgeMode::from_path_name("time:giz_waterloo_l"), Timed);
        // Unprefixed names keep the constructor default.
        assert_eq!(DrawbridgeMode::from_path_name("giz_bridge01_l"), Timed);
        assert_eq!(DrawbridgeMode::from_path_name("gi"), Timed);
    }

    #[test]
    fn timed_leaf_cycles_wait_open_hold_close() {
        let mut leaf = LeafMotion::new(DrawbridgeMode::Timed);
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        let mut opened_at = None;
        let mut raised_at = None;
        let mut closing_at = None;
        let mut closed_at = None;
        while t < 60.0 {
            leaf.step(dt);
            t += dt;
            match leaf.phase {
                LeafPhase::Opening if opened_at.is_none() => opened_at = Some(t),
                LeafPhase::Raised if raised_at.is_none() => raised_at = Some(t),
                LeafPhase::Closing if closing_at.is_none() => closing_at = Some(t),
                LeafPhase::Resting if closing_at.is_some() && closed_at.is_none() => {
                    closed_at = Some(t)
                }
                _ => {}
            }
        }
        let open_secs = OPEN_ANGLE / RAISE_RATE;
        assert!((opened_at.unwrap() - CLOSED_WAIT).abs() < 0.05);
        assert!((raised_at.unwrap() - (CLOSED_WAIT + open_secs)).abs() < 0.05);
        assert!((closing_at.unwrap() - (CLOSED_WAIT + open_secs + RAISED_HOLD)).abs() < 0.1);
        assert!((closed_at.unwrap() - (CLOSED_WAIT + 2.0 * open_secs + RAISED_HOLD)).abs() < 0.1);
        assert!(leaf.angle <= OPEN_ANGLE && leaf.angle >= 0.0);
    }

    #[test]
    fn open_and_inactive_leaves_never_move() {
        let mut open = LeafMotion::new(DrawbridgeMode::Open);
        let mut inactive = LeafMotion::new(DrawbridgeMode::Inactive);
        for _ in 0..10_000 {
            open.step(0.1);
            inactive.step(0.1);
        }
        assert_eq!(open.angle, OPEN_ANGLE);
        assert_eq!(open.phase, LeafPhase::Resting);
        assert_eq!(inactive.angle, 0.0);
    }

    #[test]
    fn proximity_leaf_waits_for_its_trigger() {
        let mut leaf = LeafMotion::new(DrawbridgeMode::Proximity);
        for _ in 0..1000 {
            leaf.step(0.1);
        }
        assert_eq!(leaf.angle, 0.0);
        assert!(leaf.trigger());
        assert!(!leaf.trigger(), "a moving leaf ignores the trigger");
        leaf.step(1.0);
        assert!((leaf.angle - RAISE_RATE).abs() < 1e-6);
    }

    #[test]
    fn hinges_pair_the_outer_points() {
        let p = |x: f32| [x, 0.0, 0.0];
        assert!(leaf_hinges(&[p(0.0)]).is_empty());
        assert_eq!(leaf_hinges(&[p(0.0), p(1.0)]), vec![(p(0.0), p(1.0))]);
        assert_eq!(
            leaf_hinges(&[p(0.0), p(1.0), p(2.0)]),
            vec![(p(0.0), p(2.0)), (p(2.0), p(0.0))]
        );
    }
}
