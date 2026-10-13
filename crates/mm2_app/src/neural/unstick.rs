//! Backing out of a dead end.
//!
//! Driving into something and stopping against it is the one situation
//! the policy is not left to solve: the way out is to reverse, which a
//! driver rewarded for progress almost never tries. So a car that has
//! held the throttle for [`STILL_DECISIONS`] without moving a metre is
//! taken over for a moment and backed up, swinging its nose toward its
//! line — the original opponent's own remedy (`aiVehiclePhysics` state
//! 1: reverse for 66 frames steering `−2.857 ×` the bearing, ending
//! early once the nose is within 0.1 rad; `docs/research/opponent-ai.md`).
//! Then the policy drives again, from somewhere it can.
//!
//! The same [`Unstick`] runs in training and in the game, once per
//! decision, so the policy learns from the states a back-up leaves it
//! in. A car this cannot free is the bounded re-anchor's business.

use bevy::math::Vec3;

use super::observe::ACT_DIM;

/// Decisions (at 20 Hz) of throttle without a metre of movement before
/// the car is backed up — 1.5 s.
pub const STILL_DECISIONS: u16 = 30;
/// Decisions a back-up lasts at most — the original's 66 frames.
pub const BACKING_DECISIONS: u16 = 22;
/// Movement (m) that counts as not being stuck.
const MOVED: f32 = 1.0;
/// Pedal demand above which the driver is trying to go forward.
const TRYING: f32 = 0.3;
/// Steering per radian of bearing while backing — the original's gain.
const BACK_STEER: f32 = 2.857;
/// Bearing (rad) inside which the nose points near enough at the line
/// to stop backing — the original's.
const ALIGNED: f32 = 0.1;

/// Per-car stuck watch and back-up state.
#[derive(Debug, Clone, Copy, Default)]
pub struct Unstick {
    /// Where the current stillness window started.
    anchor: Vec3,
    /// Decisions spent trying to go forward inside [`MOVED`] of `anchor`.
    still: u16,
    /// Decisions of backing left.
    backing: u16,
    /// Back-ups started — evidence for the record.
    pub count: u32,
}

impl Unstick {
    /// One decision. `action` is what the policy wants, `pos` where the
    /// car is and `bearing` the signed angle (rad, positive right) from
    /// its nose to its line a little way ahead. Returns the action to
    /// apply instead while the car is being backed up.
    pub fn step(
        &mut self,
        pos: Vec3,
        action: &[f32; ACT_DIM],
        bearing: f32,
    ) -> Option<[f32; ACT_DIM]> {
        if self.backing > 0 {
            self.backing -= 1;
            if bearing.abs() < ALIGNED {
                self.backing = 0;
            }
            self.anchor = pos;
            // Reverse (the brake pedal, for an AI car at rest), steering
            // the tail away so the nose comes round to the line.
            return Some([(-BACK_STEER * bearing).clamp(-1.0, 1.0), -1.0, -1.0]);
        }
        if !pos.is_finite() || pos.distance(self.anchor) > MOVED {
            self.anchor = pos;
            self.still = 0;
        } else if action[1] > TRYING {
            self.still += 1;
            if self.still >= STILL_DECISIONS {
                self.still = 0;
                self.backing = BACKING_DECISIONS;
                self.count += 1;
            }
        }
        None
    }

    /// Whether the car is being backed up right now.
    pub fn is_backing(&self) -> bool {
        self.backing > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GO: [f32; ACT_DIM] = [0.0, 1.0, -1.0];

    #[test]
    fn a_car_that_moves_is_left_alone() {
        let mut u = Unstick::default();
        for k in 0..200 {
            let pos = Vec3::new(k as f32 * 0.5, 0.0, 0.0);
            assert_eq!(u.step(pos, &GO, 0.4), None);
        }
        assert_eq!(u.count, 0);
    }

    #[test]
    fn a_car_that_waits_without_throttle_is_left_alone() {
        let mut u = Unstick::default();
        for _ in 0..200 {
            assert_eq!(u.step(Vec3::ZERO, &[0.0, -0.5, -1.0], 0.4), None);
        }
    }

    #[test]
    fn a_stalled_car_backs_up_toward_its_line_then_drives_again() {
        let mut u = Unstick::default();
        for _ in 0..STILL_DECISIONS {
            assert_eq!(u.step(Vec3::ZERO, &GO, 0.5), None);
        }
        // The line is to the right: reverse with left lock.
        let back = u.step(Vec3::ZERO, &GO, 0.5).expect("backing");
        assert!(back[0] < -0.9 && back[1] == -1.0);
        let mut backed = 1;
        while u.step(Vec3::ZERO, &GO, 0.5).is_some() {
            backed += 1;
        }
        assert_eq!(backed, BACKING_DECISIONS);
        assert_eq!(u.count, 1);
    }

    #[test]
    fn backing_ends_once_the_nose_is_on_the_line() {
        let mut u = Unstick::default();
        for _ in 0..STILL_DECISIONS {
            u.step(Vec3::ZERO, &GO, 0.5);
        }
        assert!(u.step(Vec3::ZERO, &GO, 0.5).is_some());
        assert!(u.step(Vec3::ZERO, &GO, 0.05).is_some());
        assert_eq!(u.step(Vec3::ZERO, &GO, 0.05), None, "aligned: done");
    }
}
