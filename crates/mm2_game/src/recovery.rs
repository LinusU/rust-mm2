//! Water/out-of-bounds recovery contract (F05-B.5).
//!
//! The spec's safe-recovery requirement ("reset, stuck recovery,
//! rollover recovery, water/out-of-bounds and respawn safe for all
//! vehicle sizes and trailers") has no authored data side: no retail
//! record tunes a water or out-of-bounds rescue — the damage-family
//! census (DMG-5..8) covers `vehcardamage`/`vehstuck`/`vehgyro` only,
//! and DMG-2's documented rules cover destruction, not drowning. So
//! unlike [`crate::stuck`]/[`crate::damage`], every bound here is a
//! disclosed *designed* policy (DSN-23), not decoded original data —
//! the original's water/OOB rules stay unverified (UNK-13).
//!
//! What the component models:
//!
//! - a **dry-grounded anchor**: the last pose where a grounded wheel
//!   rested on a non-water surface. Water colliders are solid in this
//!   engine — a car drives onto the Thames and wades on the F06-B.2
//!   `drag` term — so "in the water" is a surface class under the
//!   wheels, not a missing floor. While any grounded wheel is dry the
//!   car is provably somewhere recoverable, and the anchor tracks it;
//!   - the **submerged** leg: every grounded wheel on a water-class
//!     surface (authored `drag` >= [`RecoveryPolicy::water_min_drag`],
//!     which parts retail `deepwater` 0.5 from shallow `water` 0.119 —
//!     a shallow pond stays wadable) accrues
//!     [`RecoveryPolicy::submerge_dwell`]. The dwell is the escape
//!     window: a car that powers back onto a dry edge inside it keeps
//!     driving; one that cannot is recovered to the anchor — back on
//!     the shore it left, not in place on the water;
//!   - the **out-of-bounds** leg: an airborne car that falls
//!     [`RecoveryPolicy::fall_margin`] below its anchor has left the
//!     world (through the floor, off the map edge) and recovers to
//!     that anchor. A legitimate drop lands on real ground first —
//!     the landing is a dry contact and refreshes the anchor before
//!     the margin can fire — so the bound can be sized generously. A
//!     non-finite pose can never be legitimate and fires at once.
//!
//! The verdict only *detects*: what the session does with it — reset
//! to the landing, spawn fallback when no anchor exists yet, trailer
//! re-seating — is the resolver's choice, the same contract
//! [`crate::stuck`] keeps between detection and recovery. Like every
//! session authority object, a predicted client never declares its
//! own recovery: who may resolve is the session's role check, not
//! this component's.

use bevy::prelude::*;

use crate::ids::ObjectId;

/// The designed recovery bounds — every value is a disclosed designed
/// policy (DSN-23); no authored record exists to decode (UNK-13).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecoveryPolicy {
    /// `TireSurface::drag` at or above which a grounded wheel counts
    /// as drowning water. `0.3` parts the two retail water materials:
    /// `deepwater` (0.5 — the Thames class) drowns, `water` (0.119 —
    /// shallow ponds) stays wadable under the F06-B.2 policy.
    pub water_min_drag: f32,
    /// Continuous submersion before the recovery fires, seconds. The
    /// dwell is the escape window — a car that regains a dry contact
    /// inside it keeps driving.
    pub submerge_dwell: f32,
    /// Vertical fall below the last dry-grounded anchor that declares
    /// the car out of the world, metres. Sized far past the largest
    /// legitimate drop a retail city offers, since any real landing
    /// refreshes the anchor before the margin can fire.
    pub fall_margin: f32,
}

impl Default for RecoveryPolicy {
    fn default() -> Self {
        Self {
            water_min_drag: 0.3,
            submerge_dwell: 2.0,
            fall_margin: 50.0,
        }
    }
}

/// What the grounded wheels say about the surface under the car, one
/// fixed step — the app side classifies `WheelState::surface_drag`
/// against [`RecoveryPolicy::water_min_drag`] so this contract never
/// borrows the sim's wheel type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroundContact {
    /// No wheel touches anything — jumping or falling.
    Airborne,
    /// At least one grounded wheel rests on a non-water surface — the
    /// car is somewhere recoverable, and the anchor tracks it.
    Dry,
    /// Every grounded wheel rests on a water-class surface — the car
    /// floats on water with no dry purchase.
    Submerged,
}

/// What tripped the recovery — the [`RecoveryEvent`] cause and the
/// `rcv=` evidence split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryCause {
    /// Every grounded wheel sat on water past `submerge_dwell`.
    Submerged,
    /// The car fell `fall_margin` below its anchor while airborne, or
    /// its pose went non-finite.
    OutOfBounds,
}

/// What one [`VehicleRecovery::observe`] call decided.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecoveryVerdict {
    /// Nothing to recover — anchored, airborne in bounds, or still
    /// inside the dwell.
    Clear,
    /// Fire the bounded recovery. `landing` is the pose to reset to —
    /// the last dry-grounded anchor. `None` means the car never
    /// touched dry ground (a spawn straight onto water): the resolver
    /// falls back to the session spawn.
    Recover {
        /// Which leg fired.
        cause: RecoveryCause,
        /// `(position, yaw)` to land at — the recorded anchor.
        landing: Option<(Vec3, f32)>,
    },
}

/// A participant's water/OOB recovery detector — session authority
/// state like [`crate::VehicleDamage`]/[`crate::VehicleStuck`], but
/// carried unconditionally: there is no authored record to gate the
/// spawn on, so every local and AI participant gets the designed
/// policy and a predicted client's authority owns its own.
///
/// `anchor` is the last pose a dry grounded wheel proved recoverable;
/// it is seeded at spawn so the out-of-bounds leg has a reference
/// before the first landing.
#[derive(Component, Debug, Clone, Copy)]
pub struct VehicleRecovery {
    /// The designed bounds.
    pub policy: RecoveryPolicy,
    anchor: Option<(Vec3, f32)>,
    submerged_for: f32,
    /// One OOB fire per airborne episode — the latch clears on any
    /// ground contact so a still-falling car emits once, not every
    /// step until the reset lands.
    fall_latched: bool,
}

impl VehicleRecovery {
    /// A fresh detector with no anchor — the first dry contact (or the
    /// resolver's spawn fallback) supplies the landing.
    pub fn new(policy: RecoveryPolicy) -> Self {
        Self {
            policy,
            anchor: None,
            submerged_for: 0.0,
            fall_latched: false,
        }
    }

    /// A fresh detector pre-anchored at `position`/`yaw` — the spawn
    /// pose, so a car that leaves the world before its first landing
    /// still has a landing to return to.
    pub fn with_anchor(policy: RecoveryPolicy, position: Vec3, yaw: f32) -> Self {
        Self {
            anchor: Some((position, yaw)),
            ..Self::new(policy)
        }
    }

    /// The last dry-grounded `(position, yaw)` — the recovery landing.
    pub fn anchor(&self) -> Option<(Vec3, f32)> {
        self.anchor
    }

    /// Seconds accrued on water this episode.
    pub fn submerged_for(&self) -> f32 {
        self.submerged_for
    }

    /// Clear the episode after a recovery landed the car at
    /// `position`/`yaw`: re-anchor there, zero the dwell, unlatch the
    /// fall. The resolver calls this so a landing back on water starts
    /// a fresh dwell rather than carrying stale accrual — and a
    /// degenerate wet landing re-fires on the next full dwell instead
    /// of latching silent.
    pub fn recovered(&mut self, position: Vec3, yaw: f32) {
        self.anchor = Some((position, yaw));
        self.submerged_for = 0.0;
        self.fall_latched = false;
    }

    /// Advance the detector one fixed step. `contact` is the app-side
    /// classification of the wheels' grounded surfaces; `yaw` is the
    /// heading the anchor records (a recovered car keeps its heading,
    /// the same rule every other recovery path follows).
    pub fn observe(
        &mut self,
        position: Vec3,
        yaw: f32,
        contact: GroundContact,
        dt: f32,
    ) -> RecoveryVerdict {
        // A non-finite pose can never be legitimate and cannot refresh
        // an anchor — recover to the last good one at once.
        if !position.is_finite() || !yaw.is_finite() {
            return RecoveryVerdict::Recover {
                cause: RecoveryCause::OutOfBounds,
                landing: self.anchor,
            };
        }
        match contact {
            GroundContact::Dry => {
                self.anchor = Some((position, yaw));
                self.submerged_for = 0.0;
                self.fall_latched = false;
            }
            GroundContact::Submerged => {
                self.fall_latched = false;
                if dt.is_finite() && dt > 0.0 {
                    self.submerged_for += dt;
                }
                // Fixed-step accumulation lands a hair under the bound
                // — the fire edge tolerates it (same rule `vehstuck`'s
                // window uses).
                if self.submerged_for + 1e-3 >= self.policy.submerge_dwell {
                    self.submerged_for = 0.0;
                    return RecoveryVerdict::Recover {
                        cause: RecoveryCause::Submerged,
                        landing: self.anchor,
                    };
                }
            }
            GroundContact::Airborne => {
                self.submerged_for = 0.0;
                if !self.fall_latched
                    && let Some((anchor_pos, _)) = self.anchor
                    && position.y < anchor_pos.y - self.policy.fall_margin
                {
                    self.fall_latched = true;
                    return RecoveryVerdict::Recover {
                        cause: RecoveryCause::OutOfBounds,
                        landing: self.anchor,
                    };
                }
            }
        }
        RecoveryVerdict::Clear
    }
}

/// One fired recovery detection — the resolver and a future HUD cue
/// consume it. Bounded: the submerged leg needs a fresh dwell per
/// fire and the OOB leg latches until the car touches ground, so the
/// stream stays at most one event per episode per vehicle.
#[derive(Message, Debug, Clone, Copy)]
pub struct RecoveryEvent {
    /// The vehicle that needs recovering.
    pub object: ObjectId,
    /// Session generation the detection belongs to.
    pub generation: u64,
    /// Fixed-step session tick the detection fired on.
    pub tick: u64,
    /// Which leg fired.
    pub cause: RecoveryCause,
    /// The detector's landing — the last dry-grounded anchor, or
    /// `None` to ask the resolver for the session spawn.
    pub landing: Option<(Vec3, f32)>,
}
